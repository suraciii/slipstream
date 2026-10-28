//! Trash absence revalidation, record cleanup, and receipt replay.
use super::*;

#[tokio::test]
async fn cleanup_rechecks_absence_staleness_and_retained_work_and_replays() {
    use std::os::unix::fs::symlink;

    let (_base, root, state, name, path) = fixture();
    let names = [
        "absent.JPG",
        "present.JPG",
        "unsafe.JPG",
        "stale.JPG",
        "workspace.JPG",
        "recipe.JPG",
        "export.JPG",
    ];
    for name in names {
        fs::write(root.canonical_path().join(name), name.as_bytes()).unwrap();
    }
    let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
    let snapshot = library.scan().await.unwrap();
    let ids: Vec<_> = snapshot
        .photos
        .iter()
        .map(|photo| photo.id.clone())
        .collect();
    for photo_id in &ids {
        library
            .mutate_photo_state(PhotoStateMutation {
                photo_id: photo_id.clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Rejected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
    }
    library
        .remove_photos(PhotoRemovalMutation {
            photo_ids: ids,
            operation_id: "mixed-removal".to_owned(),
        })
        .await
        .unwrap();
    let inspection = library.trash_inspection().await.unwrap();
    let record = |name: &str| {
        inspection
            .iter()
            .find(|item| item.relative_path.as_str() == name)
            .unwrap()
    };
    let mut targets: Vec<_> = names
        .iter()
        .map(|name| cleanup_target(record(name)))
        .collect();
    let mut unavailable = targets[0].clone();
    unavailable.photo_id = "no-such-photo".to_owned();
    targets.push(unavailable);
    targets[4].blockers.push(TrashCleanupBlocker {
        kind: "workspace".to_owned(),
        name: "workspace-1".to_owned(),
        action: "Remove this retained work before cleaning up the record.".to_owned(),
    });
    let connection = Connection::open(&path).unwrap();
    connection.execute(
        "INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode) VALUES(?, 'recipe-1', 'source-1', 0.0, 'as-shot')",
        [&record("recipe.JPG").photo_id],
    ).unwrap();
    connection.execute(
        "INSERT INTO exports(id,photo_id,target,state,recipe_revision,exposure_ev,white_balance_mode,source_revision,source_profile_id,source_kind,recipe_digest,policy_id,bundle_id,workload,created_at) VALUES('export-1',?,'film-jpeg','succeeded','recipe-1',0.0,'as-shot','source-1','profile-1','raw',?,?,?,'film-jpeg',1)",
        params![record("export.JPG").photo_id, "d".repeat(64), "e".repeat(64), "f".repeat(64)],
    ).unwrap();
    connection.execute(
        "INSERT INTO xmp_exports(id,photo_id,request_id,payload_digest,recipe_revision,source_revision,exposure_ev,white_balance_mode,created_at,expires_at,filename,byte_length,sha256) VALUES('xmp-history',?,'request-1',?,'recipe-1','source-1',0.0,'as-shot',1,1,'absent.xmp',1,?)",
        params![
            record("absent.JPG").photo_id,
            "a".repeat(64),
            "b".repeat(64)
        ],
    )
    .unwrap();
    connection.execute(
        "INSERT INTO xmp_exports(id,photo_id,request_id,payload_digest,recipe_revision,source_revision,exposure_ev,white_balance_mode,created_at,expires_at,filename,byte_length,sha256) VALUES('xmp-active',?,'request-2',?,'recipe-1','source-1',0.0,'as-shot',1,?, 'active.xmp',1,?)",
        params![
            record("recipe.JPG").photo_id,
            "c".repeat(64),
            i64::MAX,
            "d".repeat(64)
        ],
    )
    .unwrap();
    drop(connection);
    for name in names {
        if name != "present.JPG" {
            fs::remove_file(root.canonical_path().join(name)).unwrap();
        }
    }
    // A replacement and an unsafe entry must not be mistaken for absence.
    fs::write(root.canonical_path().join("present.JPG"), b"replacement").unwrap();
    symlink("absent.JPG", root.canonical_path().join("unsafe.JPG")).unwrap();
    let stale = record("stale.JPG");
    library
        .restore_photos(PhotoRestoration::Photos(vec![PhotoRemovalMarker {
            photo_id: stale.photo_id.clone(),
            removed_at_ms: stale.removed_at_ms,
        }]))
        .await
        .unwrap();
    library
        .remove_photos(PhotoRemovalMutation {
            photo_ids: vec![stale.photo_id.clone()],
            operation_id: "new-removal".to_owned(),
        })
        .await
        .unwrap();
    let result = library
        .cleanup_trash("mixed-cleanup".to_owned(), targets.clone())
        .await
        .unwrap();
    assert_eq!(result.reviewed, names.len() + 1);
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.state)
            .collect::<Vec<_>>(),
        vec![
            TrashCleanupItemState::Cleaned,
            TrashCleanupItemState::Present,
            TrashCleanupItemState::InspectionFailed,
            TrashCleanupItemState::Changed,
            TrashCleanupItemState::Blocked,
            TrashCleanupItemState::Blocked,
            TrashCleanupItemState::Blocked,
            TrashCleanupItemState::Unavailable,
        ]
    );
    assert_eq!(result.counts.cleaned, 1);
    assert_eq!(result.counts.present, 1);
    assert_eq!(result.counts.inspection_failed, 1);
    assert_eq!(result.counts.changed, 1);
    assert_eq!(result.counts.blocked, 3);
    assert_eq!(result.counts.unavailable, 1);
    for (index, kind) in [(4, "workspace"), (5, "edit-recipe"), (6, "export")] {
        assert!(
            result.items[index]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == kind)
        );
    }
    assert!(
        result.items[5]
            .blockers
            .iter()
            .any(|blocker| blocker.kind == "metadata-export")
    );
    assert_eq!(
        fs::read(root.canonical_path().join("present.JPG")).unwrap(),
        b"replacement"
    );
    assert!(
        fs::symlink_metadata(root.canonical_path().join("unsafe.JPG"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let connection = Connection::open(&path).unwrap();
    let remaining_xmp: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM xmp_exports WHERE photo_id=?",
            [&record("absent.JPG").photo_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining_xmp, 0);
    drop(connection);
    assert_eq!(
        library.trash_inspection().await.unwrap().len(),
        names.len() - 1
    );
    // Replay is immutable even when the previously absent Location is present again.
    fs::write(root.canonical_path().join("absent.JPG"), b"new arrival").unwrap();
    assert_eq!(
        library
            .cleanup_trash("mixed-cleanup".to_owned(), targets.clone())
            .await
            .unwrap(),
        result
    );
    assert_eq!(
        library
            .read_trash_cleanup("mixed-cleanup".to_owned())
            .await
            .unwrap(),
        result
    );
    assert!(
        library
            .cleanup_trash(
                "duplicate".to_owned(),
                vec![targets[0].clone(), targets[0].clone()]
            )
            .await
            .is_err()
    );
    assert!(
        library
            .cleanup_trash("empty".to_owned(), Vec::new())
            .await
            .is_err()
    );
    targets[0].removed_at_ms += 1;
    assert!(
        library
            .cleanup_trash("mixed-cleanup".to_owned(), targets)
            .await
            .is_err()
    );
    library.shutdown().unwrap();
    let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
    assert_eq!(
        library
            .read_trash_cleanup("mixed-cleanup".to_owned())
            .await
            .unwrap(),
        result
    );
    library.shutdown().unwrap();
}

#[tokio::test]
async fn cleanup_storage_failure_rolls_back_every_record_and_receipt() {
    let (_base, root, state, name, path) = fixture();
    for name in ["first.JPG", "second.JPG"] {
        fs::write(root.canonical_path().join(name), name.as_bytes()).unwrap();
    }
    let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
    let ids: Vec<_> = library
        .scan()
        .await
        .unwrap()
        .photos
        .into_iter()
        .map(|photo| photo.id)
        .collect();
    for photo_id in &ids {
        library
            .mutate_photo_state(PhotoStateMutation {
                photo_id: photo_id.clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Rejected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
    }
    library
        .remove_photos(PhotoRemovalMutation {
            photo_ids: ids,
            operation_id: "rollback-removal".to_owned(),
        })
        .await
        .unwrap();
    let targets: Vec<_> = library
        .trash_inspection()
        .await
        .unwrap()
        .iter()
        .map(cleanup_target)
        .collect();
    for name in ["first.JPG", "second.JPG"] {
        fs::remove_file(root.canonical_path().join(name)).unwrap();
    }
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(&format!(
        "CREATE TRIGGER fail_cleanup BEFORE DELETE ON photos WHEN OLD.id='{}' BEGIN SELECT RAISE(ABORT, 'injected storage failure'); END;",
        targets[1].photo_id
    )).unwrap();
    drop(connection);
    assert!(
        library
            .cleanup_trash("rollback-cleanup".to_owned(), targets)
            .await
            .is_err()
    );
    assert_eq!(library.trash_inspection().await.unwrap().len(), 2);
    assert!(
        library
            .read_trash_cleanup("rollback-cleanup".to_owned())
            .await
            .is_err()
    );
    library.shutdown().unwrap();
}

#[tokio::test]
async fn cleanup_refuses_when_library_folder_binding_changes() {
    let (base, root, state, name, _path) = fixture();
    fs::write(root.canonical_path().join("photo.JPG"), b"original").unwrap();
    let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
    let photo = library.scan().await.unwrap().photos[0].id.clone();
    reject_and_remove(&library, &photo).await;
    let target = cleanup_target(&library.trash_inspection().await.unwrap()[0]);
    fs::remove_file(root.canonical_path().join("photo.JPG")).unwrap();
    let moved_root = base.0.join("former-originals");
    fs::rename(root.canonical_path(), &moved_root).unwrap();
    fs::create_dir(root.canonical_path()).unwrap();
    let result = library
        .cleanup_trash("changed-root".to_owned(), vec![target])
        .await
        .unwrap();
    assert_eq!(
        result.items[0].state,
        TrashCleanupItemState::InspectionFailed
    );
    assert_eq!(library.trash_inspection().await.unwrap().len(), 1);
    library.shutdown().unwrap();
}

#[tokio::test]
async fn legacy_receipts_never_resume_unlink_or_lock_restore() {
    let (_base, root, state, name, path) = fixture();
    fs::write(
        root.canonical_path().join("uncertain.JPG"),
        b"keep uncertain",
    )
    .unwrap();
    fs::write(
        root.canonical_path().join("confirmed.JPG"),
        b"keep confirmed",
    )
    .unwrap();
    let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
    let snapshot = library.scan().await.unwrap();
    let ids: Vec<_> = snapshot
        .photos
        .iter()
        .map(|photo| photo.id.clone())
        .collect();
    for photo_id in &ids {
        library
            .mutate_photo_state(PhotoStateMutation {
                photo_id: photo_id.clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Rejected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
    }
    library
        .remove_photos(PhotoRemovalMutation {
            photo_ids: ids,
            operation_id: "legacy-removal".to_owned(),
        })
        .await
        .unwrap();
    let inspection = library.trash_inspection().await.unwrap();
    let uncertain = inspection
        .iter()
        .find(|item| item.relative_path.as_str() == "uncertain.JPG")
        .unwrap();
    let confirmed = inspection
        .iter()
        .find(|item| item.relative_path.as_str() == "confirmed.JPG")
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)",
            params![
                format!("permanent_deletion_unsettled:{}", uncertain.photo_id),
                "legacy-attempt"
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)",
            params![
                format!("permanent_deletion_deleted:{}", confirmed.photo_id),
                "legacy-attempt"
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)",
            params![
                format!(
                    "permanent_deletion_deleted_original:{}",
                    confirmed.original_id
                ),
                "legacy-attempt"
            ],
        )
        .unwrap();
    let receipt = serde_json::json!({
        "operation_id": "legacy-attempt",
        "items": [{
            "photo_id": confirmed.photo_id, "removed_at_ms": confirmed.removed_at_ms,
            "original_id": confirmed.original_id, "relative_path": "confirmed.JPG",
            "kind": "jpeg", "size": 14, "mtime_bits": 0, "device": 0, "inode": 0, "albums": []
        }, {
            "photo_id": uncertain.photo_id, "removed_at_ms": uncertain.removed_at_ms,
            "original_id": uncertain.original_id, "relative_path": "uncertain.JPG",
            "kind": "jpeg", "size": 14, "mtime_bits": 0, "device": 0, "inode": 0, "albums": []
        }],
        "rejected": []
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)",
            params!["permanent_deletion_operation:legacy-attempt", receipt],
        )
        .unwrap();
    for (item, state) in [(confirmed, "Deleted"), (uncertain, "Deleting")] {
        let value =
            serde_json::json!({"original_id": item.original_id, "state": state, "message": null})
                .to_string();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES(?,?)",
                params![
                    format!("permanent_deletion_item:legacy-attempt:{}", item.photo_id),
                    value
                ],
            )
            .unwrap();
    }
    drop(connection);
    library.shutdown().unwrap();
    let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
    assert_eq!(
        fs::read(root.canonical_path().join("uncertain.JPG")).unwrap(),
        b"keep uncertain"
    );
    assert_eq!(
        fs::read(root.canonical_path().join("confirmed.JPG")).unwrap(),
        b"keep confirmed"
    );
    // Legacy scanning reserved the old row's location when a new occupant arrived.
    library.scan().await.unwrap();
    let retained = library.trash_inspection().await.unwrap();
    let recovered = retained
        .iter()
        .find(|item| item.photo_id == confirmed.photo_id)
        .unwrap();
    assert_eq!(recovered.relative_path.as_str(), "confirmed.JPG");
    let refused = library
        .cleanup_trash("legacy-present".to_owned(), vec![cleanup_target(recovered)])
        .await
        .unwrap();
    assert_eq!(refused.items[0].state, TrashCleanupItemState::Present);
    assert_eq!(
        fs::read(root.canonical_path().join("confirmed.JPG")).unwrap(),
        b"keep confirmed"
    );
    let connection = Connection::open(&path).unwrap();
    connection.execute("UPDATE library_metadata SET value='not-json' WHERE key='permanent_deletion_operation:legacy-attempt'", []).unwrap();
    drop(connection);
    let unresolved = library.trash_inspection().await.unwrap();
    let unresolved = unresolved
        .iter()
        .find(|item| item.photo_id == confirmed.photo_id)
        .unwrap();
    assert_ne!(unresolved.relative_path.as_str(), "confirmed.JPG");
    let refused = library
        .cleanup_trash(
            "legacy-unresolved".to_owned(),
            vec![cleanup_target(unresolved)],
        )
        .await
        .unwrap();
    assert_eq!(
        refused.items[0].state,
        TrashCleanupItemState::InspectionFailed
    );
    assert_eq!(
        fs::read(root.canonical_path().join("confirmed.JPG")).unwrap(),
        b"keep confirmed"
    );
    Connection::open(&path).unwrap().execute(
        "UPDATE library_metadata SET value=? WHERE key='permanent_deletion_operation:legacy-attempt'", [&receipt],
    ).unwrap();
    assert_eq!(library.trash_inspection().await.unwrap().len(), 2);
    let restored = library
        .restore_photos(PhotoRestoration::Photos(vec![
            PhotoRemovalMarker {
                photo_id: uncertain.photo_id.clone(),
                removed_at_ms: uncertain.removed_at_ms,
            },
            PhotoRemovalMarker {
                photo_id: confirmed.photo_id.clone(),
                removed_at_ms: confirmed.removed_at_ms,
            },
        ]))
        .await
        .unwrap();
    assert_eq!(restored.restored, vec![uncertain.photo_id.clone()]);
    assert_eq!(restored.changed_elsewhere, vec![confirmed.photo_id.clone()]);
    fs::remove_file(root.canonical_path().join("confirmed.JPG")).unwrap();
    let result = library
        .cleanup_trash("legacy-cleanup".to_owned(), vec![cleanup_target(recovered)])
        .await
        .unwrap();
    assert_eq!(result.items[0].state, TrashCleanupItemState::Cleaned);
    assert_eq!(
        fs::read(root.canonical_path().join("uncertain.JPG")).unwrap(),
        b"keep uncertain"
    );
    library.shutdown().unwrap();
}

#[tokio::test]
async fn cleanup_compacts_album_retains_sidecar_and_rediscovery_gets_new_identity() {
    let (_base, root, state, name, path) = fixture();
    for name in ["first.JPG", "middle.JPG", "last.JPG"] {
        fs::write(root.canonical_path().join(name), name.as_bytes()).unwrap();
    }
    fs::write(root.canonical_path().join("middle.xmp"), b"sidecar bytes").unwrap();
    let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
    let snapshot = library.scan().await.unwrap();
    let photo_at = |name: &str| {
        let original = snapshot
            .originals
            .iter()
            .find(|original| original.relative_path.as_str() == name)
            .unwrap();
        snapshot
            .photos
            .iter()
            .find(|photo| photo.original_id == original.id)
            .unwrap()
            .id
            .clone()
    };
    let first = photo_at("first.JPG");
    let middle = photo_at("middle.JPG");
    let last = photo_at("last.JPG");
    let old_original = snapshot
        .photos
        .iter()
        .find(|photo| photo.id == middle)
        .unwrap()
        .original_id
        .clone();
    let album = library
        .mutate_album(AlbumMutation::Create {
            name: "Album".to_owned(),
        })
        .await
        .unwrap();
    library
        .mutate_album(AlbumMutation::AddMembers {
            album_id: album.album_id.clone(),
            photo_ids: vec![first.clone(), middle.clone(), last.clone()],
        })
        .await
        .unwrap();
    seed_sidecar(&path, &middle, "middle.xmp");
    reject_and_remove(&library, &middle).await;
    let captured = library
        .trash_inspection()
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.photo_id == middle)
        .unwrap();
    let album_version = library
        .album(&album.album_id)
        .await
        .unwrap()
        .unwrap()
        .album_version;
    let generation = association_generation(&path, &middle);
    fs::remove_file(root.canonical_path().join("middle.JPG")).unwrap();
    let result = library
        .cleanup_trash("cleanup-middle".to_owned(), vec![cleanup_target(&captured)])
        .await
        .unwrap();
    assert_eq!(result.items[0].state, TrashCleanupItemState::Cleaned);
    assert_ne!(
        library
            .album(&album.album_id)
            .await
            .unwrap()
            .unwrap()
            .album_version,
        album_version
    );
    assert_eq!(
        fs::read(root.canonical_path().join("middle.xmp")).unwrap(),
        b"sidecar bytes"
    );
    assert_retired(&path, &middle, "middle.JPG", "middle.xmp", generation);
    let connection = Connection::open(&path).unwrap();
    let members = connection
        .prepare("SELECT photo_id,position FROM album_members WHERE album_id=? ORDER BY position")
        .unwrap()
        .query_map([&album.album_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(members, vec![(first, 0), (last, 1)]);
    assert!(
        !connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM original_files WHERE id=?)",
                [&old_original],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
    drop(connection);
    let restored = library
        .restore_photos(PhotoRestoration::Photos(vec![PhotoRemovalMarker {
            photo_id: middle.clone(),
            removed_at_ms: captured.removed_at_ms,
        }]))
        .await
        .unwrap();
    assert_eq!(restored.missing, vec![middle.clone()]);
    fs::write(
        root.canonical_path().join("middle.JPG"),
        b"new independent file",
    )
    .unwrap();
    let rescanned = library.scan().await.unwrap();
    let new_original = rescanned
        .originals
        .iter()
        .find(|original| original.relative_path.as_str() == "middle.JPG")
        .unwrap();
    let new_photo = rescanned
        .photos
        .iter()
        .find(|photo| photo.original_id == new_original.id)
        .unwrap();
    assert_ne!(new_original.id, old_original);
    assert_ne!(new_photo.id, middle);
    assert_eq!(
        fs::read(root.canonical_path().join("middle.xmp")).unwrap(),
        b"sidecar bytes"
    );
    library.shutdown().unwrap();
}
