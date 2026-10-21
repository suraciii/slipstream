use super::*;
use rusqlite::{Connection, params};

/// A Photo the Photographer moved into Trash keeps its own Restore contract:
/// a scan that discovers an exact copy of its content at a new Location must
/// not relocate its Original, and the scan's unavailable count must exclude
/// it.
#[tokio::test]
async fn scan_recovery_never_follows_a_copy_for_a_trash_removed_photo() {
    let (base, initial_config) = fixture();
    fs::write(base.0.join("originals/a.JPG"), b"jpeg-bytes-a").unwrap();
    let library = Library::open(initial_config).unwrap();
    let first = library.scan().await.unwrap();
    let photo_id = first.photos[0].id.clone();
    Connection::open(base.0.join("state").join("library.sqlite"))
        .unwrap()
        .execute(
            "UPDATE photos SET selection_state='rejected',rating=5,removed_at_ms=1,
                    removed_operation='remove-op' WHERE id=?",
            params![photo_id],
        )
        .unwrap();
    library.shutdown().unwrap();
    seed_fingerprint(&base, "a.JPG", b"jpeg-bytes-a");
    fs::create_dir(base.0.join("originals/moved")).unwrap();
    fs::rename(
        base.0.join("originals/a.JPG"),
        base.0.join("originals/moved/a.JPG"),
    )
    .unwrap();
    let library = Library::open(config(&base)).unwrap();
    let second = library.scan().await.unwrap();
    let photo = second
        .photos
        .iter()
        .find(|photo| photo.id == photo_id)
        .unwrap();
    assert!(photo.removed);
    assert!(!photo.available);
    assert!(
        second
            .originals
            .iter()
            .any(|original| original.relative_path.as_str() == "a.JPG" && !original.available)
    );
    assert_eq!(second.originals.len(), 2);
    assert_eq!(library.scan_outcome().unwrap().unavailable_photos, 0);
    library.shutdown().unwrap();
    drop(base);
}

/// A retire-and-bind reviewed while the destination Photo was still
/// unreferenced must be refused at commit when that Photo gains a retained
/// reference in between. Every reference boundary is revalidated inside the
/// write transaction, so the whole batch stays unapplied.
#[tokio::test]
async fn manual_recovery_refuses_retire_after_destination_gains_a_reference() {
    let (base, config) = manual_recovery_fixture(Some(("moved/a.JPG", 0)), Some(false));
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    let root = LibraryRoot::open(config.library_root.clone()).unwrap();
    let snapshot = library.snapshot().await.unwrap();
    let proposals = crate::plan_manual_relocations(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        "shoot",
        "moved",
    )
    .unwrap();
    assert!(matches!(
        proposals[0].outcome,
        crate::ManualOutcome::Occupied { retire: Some(_) }
    ));
    let connection = Connection::open(base.0.join("state").join("library.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,
                 white_balance_mode) VALUES('occupant-photo','late-recipe','revision',0.5,'as-shot')",
            [],
        )
        .unwrap();
    drop(connection);
    let capability = root
        .original(crate::RelativeOriginalPath::parse("moved/a.JPG").unwrap())
        .unwrap();
    let facts = capability.facts().unwrap();
    assert!(
        library
            .apply_relocations(vec![requested_relocation(
                &proposals[0],
                &survey,
                facts,
                Some("occupant-photo"),
            )])
            .await
            .is_err()
    );
    let snapshot = library.snapshot().await.unwrap();
    let occupant = snapshot
        .photos
        .iter()
        .find(|photo| photo.id == "occupant-photo")
        .unwrap();
    let occupant_original = snapshot
        .originals
        .iter()
        .find(|original| original.id == occupant.original_id)
        .unwrap();
    assert_eq!(occupant_original.relative_path.as_str(), "moved/a.JPG");
    assert!(occupant.available);
    let missing = snapshot
        .photos
        .iter()
        .find(|photo| photo.id == "missing-photo")
        .unwrap();
    assert!(!missing.available);
    library.shutdown().unwrap();
    drop(base);
}

/// A Photo in Trash is not an active-unavailable recovery item.
#[tokio::test]
async fn manual_recovery_counts_only_active_unavailable_photos() {
    let (base, config) = manual_recovery_fixture(None, Some(false));
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    assert_eq!(survey.unavailable.len(), 1);
    assert_eq!(survey.unavailable[0].photo_id, "missing-photo");
    library.shutdown().unwrap();
    let connection = Connection::open(base.0.join("state").join("library.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,
                 capture_metadata_state) VALUES('trash-original','shoot/b.JPG','jpeg',9,1.0,0,'missing')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO photos(id,original_id,available,preview_state,sort_path,
                 selection_state,rating,removed_at_ms,removed_operation)
             VALUES('trash-photo','trash-original',0,'unavailable','shoot/b.JPG','rejected',5,1,'op')",
            [],
        )
        .unwrap();
    drop(connection);
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    assert_eq!(survey.unavailable.len(), 1);
    assert_eq!(survey.unavailable[0].photo_id, "missing-photo");
    library.shutdown().unwrap();
    drop(base);
}
