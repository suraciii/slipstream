use super::*;

#[tokio::test]
async fn explicit_rebind_preserves_every_step_except_original_revision() {
    let seeded = seeded_persistence();
    let submitted = recipe(
        "raw-photo",
        &seeded.source,
        vec![
            step(
                "original",
                "darktable",
                original_input("raw-photo", &seeded.source),
            ),
            step("film", "spektrafilm", artifact_input("immutable-artifact")),
        ],
        Some("film"),
    );
    let ComposableEditRecipeWriteOutcome::Saved(before) = save_recipe(
        &seeded,
        save("raw-photo", "initial", None, &seeded.source, submitted),
    )
    .await
    else {
        panic!("initial save");
    };
    let current = source_revision("shoot/one.ARW", 18, 2000.0).unwrap();
    let connection = Connection::open(&seeded.path).unwrap();
    connection.execute("UPDATE original_files SET size=18,mtime_ms=2000,capture_source_revision=? WHERE id='raw-original'",[format!("{current}\0device\0inode")]).unwrap();
    drop(connection);
    let mut ordinary = before.clone();
    ordinary.source_revision = current.clone();
    for step in &mut ordinary.steps {
        if let ProcessingInput::Original {
            source_revision, ..
        } = &mut step.input
        {
            *source_revision = current.clone();
        }
    }
    assert_eq!(
        save_recipe(
            &seeded,
            save(
                "raw-photo",
                "implicit-rebind",
                Some(&before.revision),
                &current,
                ordinary
            )
        )
        .await,
        ComposableEditRecipeWriteOutcome::SourceChanged(Some(before.clone()))
    );
    let mutation = crate::processing::RebindComposableEditRecipe {
        photo_id: "raw-photo".into(),
        request_id: "rebind".into(),
        expected_recipe_revision: before.revision.clone(),
        new_source_revision: current.clone(),
    };
    let outcome = seeded
        .persistence
        .rebind_composable_edit_recipe_receiver(mutation.clone())
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let ComposableEditRecipeWriteOutcome::Saved(after) = outcome else {
        panic!("rebind should save: {outcome:?}");
    };
    let mut expected = before.clone();
    expected.revision = after.revision.clone();
    expected.source_revision = current.clone();
    for step in &mut expected.steps {
        if let ProcessingInput::Original {
            source_revision, ..
        } = &mut step.input
        {
            *source_revision = current.clone();
        }
    }
    assert_ne!(before.revision, after.revision);
    assert_eq!(after, expected);
    assert_eq!(
        seeded
            .persistence
            .rebind_composable_edit_recipe_receiver(mutation)
            .unwrap()
            .await
            .unwrap()
            .unwrap(),
        ComposableEditRecipeWriteOutcome::Replayed(after)
    );
}

#[tokio::test]
async fn artifact_only_save_retains_unavailable_binding_and_refuses_known_stale_source() {
    let seeded = seeded_persistence();
    let submitted = recipe(
        "raw-photo",
        &seeded.source,
        vec![step("film", "spektrafilm", artifact_input("a1"))],
        Some("film"),
    );
    let ComposableEditRecipeWriteOutcome::Saved(before) = save_recipe(
        &seeded,
        save("raw-photo", "initial", None, &seeded.source, submitted),
    )
    .await
    else {
        panic!("initial");
    };
    let connection = Connection::open(&seeded.path).unwrap();
    connection
        .execute(
            "UPDATE original_files SET available=0 WHERE id='raw-original'",
            [],
        )
        .unwrap();
    let mut changed = before.clone();
    changed.steps[0].input = artifact_input("a2");
    let ComposableEditRecipeWriteOutcome::Saved(after) = save_recipe(
        &seeded,
        save(
            "raw-photo",
            "artifact-edit",
            Some(&before.revision),
            &seeded.source,
            changed,
        ),
    )
    .await
    else {
        panic!("artifact save while unavailable");
    };
    assert_eq!(after.source_revision, before.source_revision);
    let current = source_revision("shoot/one.ARW", 18, 2000.0).unwrap();
    connection.execute("UPDATE original_files SET size=18,mtime_ms=2000,capture_source_revision=? WHERE id='raw-original'",[format!("{current}\0device\0inode")]).unwrap();
    assert_eq!(
        save_recipe(
            &seeded,
            save(
                "raw-photo",
                "stale-artifact",
                Some(&after.revision),
                &seeded.source,
                after.clone()
            )
        )
        .await,
        ComposableEditRecipeWriteOutcome::SourceChanged(Some(after))
    );
}

#[tokio::test]
async fn expired_save_identity_is_permanently_tombstoned() {
    let seeded = seeded_persistence();
    let mutation = save(
        "raw-photo",
        "expire",
        None,
        &seeded.source,
        recipe("raw-photo", &seeded.source, vec![], None),
    );
    let ComposableEditRecipeWriteOutcome::Saved(committed) =
        save_recipe(&seeded, mutation.clone()).await
    else {
        panic!("initial");
    };
    let connection = Connection::open(&seeded.path).unwrap();
    let key = super::super::composable_recipe_receipt_key("raw-photo", "expire");
    let value: String = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [&key],
            |row| row.get(0),
        )
        .unwrap();
    let mut receipt: serde_json::Value = serde_json::from_str(&value).unwrap();
    receipt["settled_at"] = serde_json::json!(0);
    connection
        .execute(
            "UPDATE library_metadata SET value=? WHERE key=?",
            rusqlite::params![receipt.to_string(), key],
        )
        .unwrap();
    assert_eq!(
        save_recipe(&seeded, mutation.clone()).await,
        ComposableEditRecipeWriteOutcome::ReceiptExpired
    );
    let mut different = mutation;
    different.expected_recipe_revision = Some(committed.revision.clone());
    assert_eq!(
        save_recipe(&seeded, different).await,
        ComposableEditRecipeWriteOutcome::ReceiptExpired
    );
    assert_eq!(read_recipe(&seeded, "raw-photo").await, Some(committed));
}

#[test]
fn legacy_migration_retains_unsupported_white_balance_and_historical_rows() {
    let seeded = seeded_persistence();
    let mut connection = Connection::open(&seeded.path).unwrap();
    connection.execute("INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode,temperature_kelvin,tint_milli) VALUES('raw-photo','legacy-revision',?,0.5,'temperature-tint',6500,-12)",[&seeded.source]).unwrap();
    connection
        .execute(
            "DELETE FROM library_metadata WHERE key='composable_recipe_migration_v1'",
            [],
        )
        .unwrap();
    let transaction = connection.transaction().unwrap();
    super::super::migrate_legacy_recipes(&transaction).unwrap();
    let migrated = super::super::read_composable_edit_recipe(&transaction, "raw-photo")
        .unwrap()
        .unwrap();
    assert_eq!(migrated.revision, "legacy-revision");
    assert_eq!(migrated.source_revision, seeded.source);
    assert_eq!(
        migrated.steps[0].parameters.tree["stack"][1]["params"],
        serde_json::json!({"temperatureKelvin":6500,"tintMilli":-12})
    );
    assert_eq!(
        migrated.steps[0].parameters.tree["stack"][0]["params"]["exposure"],
        serde_json::json!(0.5)
    );
    assert_eq!(
        transaction
            .query_row(
                "SELECT COUNT(*) FROM edit_recipes WHERE photo_id='raw-photo'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    super::super::migrate_legacy_recipes(&transaction).unwrap();
    assert_eq!(
        super::super::read_composable_edit_recipe(&transaction, "raw-photo").unwrap(),
        Some(migrated)
    );
    transaction.commit().unwrap();
}

#[tokio::test]
async fn identical_request_ids_are_scoped_to_each_photo() {
    let seeded = seeded_persistence();
    let connection = Connection::open(&seeded.path).unwrap();
    add_recipe_test_photo(
        &connection,
        RecipeTestPhoto {
            original_id: "second-original",
            photo_id: "second-photo",
            relative_path: "shoot/two.ARW",
            kind: "raw",
            available: true,
            size: 17,
            mtime_ms: 1000.0,
        },
    );
    let second = source_revision("shoot/two.ARW", 17, 1000.0).unwrap();
    connection.execute("UPDATE original_files SET capture_metadata_state='missing',capture_source_revision=? WHERE id='second-original'",[format!("{second}\0device\0inode")]).unwrap();
    for (photo, source) in [
        ("raw-photo", seeded.source.as_str()),
        ("second-photo", second.as_str()),
    ] {
        assert!(matches!(
            save_recipe(
                &seeded,
                save(
                    photo,
                    "shared-request",
                    None,
                    source,
                    recipe(photo, source, vec![], None)
                )
            )
            .await,
            ComposableEditRecipeWriteOutcome::Saved(_)
        ));
    }
}

#[tokio::test]
async fn pre_retention_composable_receipts_migrate_to_photo_scope() {
    let seeded = seeded_persistence();
    let mutation = save(
        "raw-photo",
        "legacy-receipt",
        None,
        &seeded.source,
        recipe("raw-photo", &seeded.source, vec![], None),
    );
    let ComposableEditRecipeWriteOutcome::Saved(committed) =
        save_recipe(&seeded, mutation.clone()).await
    else {
        panic!("save");
    };
    let mut connection = Connection::open(&seeded.path).unwrap();
    let scoped_key = super::super::composable_recipe_receipt_key("raw-photo", "legacy-receipt");
    let value: String = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [&scoped_key],
            |row| row.get(0),
        )
        .unwrap();
    let mut legacy: serde_json::Value = serde_json::from_str(&value).unwrap();
    legacy.as_object_mut().unwrap().remove("settled_at");
    connection
        .execute(
            "DELETE FROM library_metadata WHERE key=? OR key='composable_recipe_migration_v1'",
            [scoped_key],
        )
        .unwrap();
    connection.execute("INSERT INTO library_metadata(key,value) VALUES('composable_edit_recipe_receipt:legacy-receipt',?)",[legacy.to_string()]).unwrap();
    let transaction = connection.transaction().unwrap();
    super::super::migrate_legacy_recipes(&transaction).unwrap();
    transaction.commit().unwrap();
    assert_eq!(
        save_recipe(&seeded, mutation).await,
        ComposableEditRecipeWriteOutcome::Replayed(committed)
    );
}

#[tokio::test]
async fn unavailable_current_capture_preserves_saved_recipe_without_observed_binding() {
    let seeded = seeded_persistence();
    let submitted = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "develop",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("develop"),
    );
    let ComposableEditRecipeWriteOutcome::Saved(saved) = save_recipe(
        &seeded,
        save("raw-photo", "capture-gap", None, &seeded.source, submitted),
    )
    .await
    else {
        panic!("initial recipe saved");
    };
    let connection = Connection::open(&seeded.path).unwrap();
    connection
        .execute(
            "UPDATE original_files SET capture_metadata_state='pending',capture_source_revision=NULL WHERE id='raw-original'",
            [],
        )
        .unwrap();
    let read = super::super::read_composable_edit_recipe_facts(&connection, "raw-photo")
        .unwrap()
        .unwrap();
    assert_eq!(read.recipe, Some(saved.clone()));
    assert_eq!(read.current_source_revision, None);
    assert!(read.source_available);
    connection
        .execute(
            "UPDATE original_files SET available=0 WHERE id='raw-original'",
            [],
        )
        .unwrap();
    let read = super::super::read_composable_edit_recipe_facts(&connection, "raw-photo")
        .unwrap()
        .unwrap();
    assert_eq!(read.recipe, Some(saved));
    assert_eq!(read.current_source_revision, None);
    assert!(!read.source_available);
}
