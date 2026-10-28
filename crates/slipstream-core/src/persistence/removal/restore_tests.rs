use super::*;
use crate::persistence::Persistence;
use crate::persistence::admission::StateDirectory;
use crate::persistence::test_support::*;
use crate::{
    ExplicitPhotoRemovalMutation, ExplicitPhotoRestoreMutation, PermanentDeletionSelection,
    PhotoRemovalTarget, PhotoStateField, PhotoStateMutation, PhotoStateValue, SelectionState,
};
use rusqlite::Connection;

#[tokio::test]
async fn explicit_removal_and_restore_replay_stale_evidence_and_restart_safe_receipts() {
    let (base, library, state, name, path) = fixture();
    let canonical_root = library.canonical_path().to_string_lossy().into_owned();
    seed(
        &path,
        include_str!("../../../../../compatibility/sqlite/schema-v8.sql"),
    );
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
            [&canonical_root],
        )
        .unwrap();
    for index in 1..=2 {
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: &format!("original-{index}"),
                photo_id: &format!("photo-{index}"),
                relative_path: &format!("shoot/one-{index}.ARW"),
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        connection
            .execute(
                "UPDATE photos SET selection_state='rejected' WHERE id=?",
                [format!("photo-{index}")],
            )
            .unwrap();
    }
    drop(connection);

    let persistence = Persistence::open(state, name.clone(), canonical_root.clone()).unwrap();
    let stale = persistence
        .photo_receiver("photo-1")
        .unwrap()
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    persistence
        .mutate_photo_state(PhotoStateMutation {
            photo_id: "photo-1".to_owned(),
            field: PhotoStateField::SelectionState,
            value: PhotoStateValue::Selection(SelectionState::Picked),
            expected_current: Some(PhotoStateValue::Selection(SelectionState::Rejected)),
            album_id: None,
        })
        .await
        .unwrap();
    persistence
        .mutate_photo_state(PhotoStateMutation {
            photo_id: "photo-1".to_owned(),
            field: PhotoStateField::SelectionState,
            value: PhotoStateValue::Selection(SelectionState::Rejected),
            expected_current: Some(PhotoStateValue::Selection(SelectionState::Picked)),
            album_id: None,
        })
        .await
        .unwrap();
    let stale_result = persistence
        .remove_photos_explicit_receiver(ExplicitPhotoRemovalMutation {
            operation_id: "remove-stale".to_owned(),
            photos: vec![PhotoRemovalTarget {
                photo_id: "photo-1".to_owned(),
                expected_selection_state: SelectionState::Rejected,
                expected_decision_version: stale.decision_version,
                expected_removed_at_ms: None,
            }],
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stale_result.counts.changed_elsewhere, 1);
    assert!(stale_result.removed.is_empty());

    let current = persistence
        .photo_receiver("photo-1")
        .unwrap()
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let remove = persistence
        .remove_photos_explicit_receiver(ExplicitPhotoRemovalMutation {
            operation_id: "remove-explicit".to_owned(),
            photos: vec![PhotoRemovalTarget {
                photo_id: current.id.clone(),
                expected_selection_state: current.selection_state,
                expected_decision_version: current.decision_version,
                expected_removed_at_ms: None,
            }],
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(remove.counts.removed, 1);
    let marker = remove.removed_markers[0].clone();
    persistence.shutdown().unwrap();
    let reopened_state = StateDirectory::open_or_create(&library, base.0.join("state")).unwrap();
    let persistence = Persistence::open(reopened_state, name, canonical_root).unwrap();
    let replay = persistence
        .photo_removal_operation_receiver("remove-explicit".to_owned())
        .unwrap()
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(replay.removed, remove.removed);
    assert_eq!(replay.removed_markers, remove.removed_markers);
    assert_eq!(replay.counts, remove.counts);

    let restore_mutation = ExplicitPhotoRestoreMutation {
        operation_id: "restore-explicit".to_owned(),
        photos: vec![marker.clone()],
    };
    let restored = persistence
        .restore_photos_explicit_receiver(restore_mutation.clone())
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.counts.restored, 1);
    let restore_replay = persistence
        .photo_restore_operation_receiver("restore-explicit".to_owned())
        .unwrap()
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(restore_replay, restored);

    let removed_after_restore = persistence
        .removed_photos_receiver(0, 10)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert!(removed_after_restore.0.is_empty());

    let current = persistence
        .photo_receiver("photo-1")
        .unwrap()
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let second = persistence
        .remove_photos_explicit_receiver(ExplicitPhotoRemovalMutation {
            operation_id: "remove-again".to_owned(),
            photos: vec![PhotoRemovalTarget {
                photo_id: current.id,
                expected_selection_state: current.selection_state,
                expected_decision_version: current.decision_version,
                expected_removed_at_ms: None,
            }],
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.counts.removed, 1);
    assert!(second.removed_markers[0].removed_at_ms > marker.removed_at_ms);

    let stale_restore = persistence
        .restore_photos_explicit_receiver(ExplicitPhotoRestoreMutation {
            operation_id: "restore-stale-marker".to_owned(),
            photos: vec![marker.clone()],
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stale_restore.counts.restored, 0);
    assert_eq!(stale_restore.changed_elsewhere, vec!["photo-1".to_owned()]);

    let old_restore_replay = persistence
        .restore_photos_explicit_receiver(restore_mutation)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_restore_replay, restored);
    let current_removed = persistence
        .removed_photos_receiver(0, 10)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current_removed.0.len(), 1);
    assert_eq!(
        current_removed.0[0].removed_at_ms,
        second.removed_markers[0].removed_at_ms
    );
    persistence.shutdown().unwrap();
}

#[tokio::test]
async fn trash_restore_all_refuses_capture_over_review_bound() {
    let (_base, library, state, name, path) = fixture();
    seed(
        &path,
        include_str!("../../../../../compatibility/sqlite/schema-v8.sql"),
    );
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
            [library.canonical_path().to_str().unwrap()],
        )
        .unwrap();
    connection.execute_batch("BEGIN").unwrap();
    for index in 0..=crate::PERMANENT_DELETION_MAX {
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: &format!("original-{index}"),
                photo_id: &format!("photo-{index}"),
                relative_path: &format!("shoot/{index}.ARW"),
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
    }
    connection.execute_batch("COMMIT").unwrap();
    drop(connection);
    let persistence = Persistence::open(
        state,
        name,
        library.canonical_path().to_string_lossy().into_owned(),
    )
    .unwrap();
    // Opening the persistence owner migrates the seeded v8 schema before the
    // fixture writes removal markers.
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE photos SET removed_at_ms=1, removed_operation='seed'",
            [],
        )
        .unwrap();
    drop(connection);
    let refused = persistence
        .restore_trash_receiver(
            "too-many".to_owned(),
            PermanentDeletionSelection::All {
                exclude_photo_ids: Vec::new(),
            },
        )
        .unwrap()
        .await
        .unwrap();
    assert!(matches!(refused, Err(MutationError::Conflict)));
    let (_, total, _) = persistence
        .removed_photos_receiver(0, 1)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(total, crate::PERMANENT_DELETION_MAX + 1);
    persistence.shutdown().unwrap();
}
