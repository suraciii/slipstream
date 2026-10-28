//! Persistence tests for Photo removal, restore, Trash, and record cleanup.
use super::*;
use crate::persistence::Persistence;
use crate::persistence::owner::unix_millis;
use crate::persistence::test_support::*;
use crate::{
    AlbumMutation, LibraryRoot, PhotoOperationRemainder, PhotoQuery, PhotoQueryOrder,
    PhotoQuerySource, PhotoRemovalCounts, PhotoRemovalMarker, PhotoRemovalMutation,
    PhotoRestoration, PhotoStateField, PhotoStateMutation, PhotoStateValue, SelectionState,
    TrashAbsence, TrashCleanupBlocker, TrashCleanupItemState, TrashCleanupTarget,
};
use rusqlite::Connection;
use rusqlite::params;
use std::{collections::HashSet, fs};

fn cleanup_target(record: &crate::TrashInspectionRecord) -> TrashCleanupTarget {
    TrashCleanupTarget {
        photo_id: record.photo_id.clone(),
        removed_at_ms: record.removed_at_ms,
        original_id: record.original_id.clone(),
        relative_path: record.relative_path.clone(),
        // The Library must replace this caller-supplied observation with fresh facts.
        absence: TrashAbsence::Absent,
        blockers: Vec::new(),
    }
}

mod cleanup_tests;

#[tokio::test]
async fn removal_and_restore_bump_association_generation() {
    let (_base, root, state, name, path) = fixture();
    fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
    fs::write(root.canonical_path().join("two.JPG"), b"two").unwrap();
    let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
    let snapshot = library.scan().await.unwrap();
    let photo = &snapshot.photos[0].id;
    let sibling = &snapshot.photos[1].id;
    seed_sidecar(&path, photo, "dir/photo.xmp");
    let before = association_generation(&path, photo);
    let sibling_before = association_generation(&path, sibling);
    reject_and_remove(&library, photo).await;
    let removed = association_generation(&path, photo);
    assert!(removed > before);
    assert_eq!(association_generation(&path, sibling), sibling_before);
    assert_eq!(
        library
            .restore_photos(PhotoRestoration::Operation("remove-sidecar".to_owned()))
            .await
            .unwrap()
            .restored,
        vec![photo.clone()]
    );
    assert!(association_generation(&path, photo) > removed);
    assert_eq!(association_generation(&path, sibling), sibling_before);
    library.shutdown().unwrap();
}

#[tokio::test]
async fn expansion_preserves_retired_sidecar_associations() {
    let (_expansion_base, parent_root, state, name, path) = fixture();
    fs::create_dir(parent_root.canonical_path().join("shoot")).unwrap();
    let root = LibraryRoot::open(parent_root.canonical_path().join("shoot")).unwrap();
    let mut config = sidecar_config(&root, &state, &name);
    fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
    fs::write(root.canonical_path().join("two.JPG"), b"two").unwrap();
    let library = crate::Library::open(config.clone()).unwrap();
    let snapshot = library.scan().await.unwrap();
    let photo = &snapshot.photos[0].id;
    let sibling = &snapshot.photos[1].id;
    seed_sidecar(&path, sibling, "dir/sibling.xmp");
    let retired = association_generation(&path, photo);
    seed(
        &path,
        &format!(
            "INSERT INTO retained_sidecar_orphans VALUES('dir/photo.xmp','{}','old.JPG','jpeg',{},7,1234.5,'{}')",
            photo,
            retired,
            "a".repeat(64),
        ),
    );
    let connection = Connection::open(&path).unwrap();
    let generations: Vec<(String, i64)> = connection
        .prepare("SELECT id,association_generation FROM photos ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    drop(connection);
    library.shutdown().unwrap();
    config.library_root = parent_root.canonical_path().to_owned();
    crate::expand_library(config).unwrap();
    for (photo, before) in generations {
        assert_eq!(association_generation(&path, &photo), before + 1);
    }
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT sidecar_path FROM sidecar_associations WHERE photo_id=?",
                [sibling],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "shoot/dir/sibling.xmp"
    );
    assert_retired(&path, photo, "old.JPG", "shoot/dir/photo.xmp", retired);
}

/// One removal reports exactly one outcome per requested Photo, a retried
/// request adopts what its own operation already removed, and restore
/// compares against the current marker instead of overwriting it.
#[tokio::test]
async fn a_library_without_the_marker_high_water_never_repeats_a_marker_it_still_holds() {
    let (_base, library, state, name, path) = fixture();
    seed(
        &path,
        include_str!("../../../../../compatibility/sqlite/schema-v9.sql"),
    );
    // A Library written before the high water mark existed carries removal
    // markers but no row for them. The greatest marker it still holds is
    // the floor for the next one, so the marker already stored for the
    // Photo cannot be assigned to a newer removal of it.
    let held = unix_millis() + 60_000;
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
            [library.canonical_path().to_str().unwrap()],
        )
        .unwrap();
    for index in [1, 2] {
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
    connection
            .execute(
                "UPDATE photos SET removed_at_ms=?,removed_operation='operation-held' WHERE id='photo-1'",
                [held],
            )
            .unwrap();
    let high_water: Option<String> = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key='removal_marker_high_water'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(high_water, None);
    drop(connection);

    let persistence = Persistence::open(
        state,
        name,
        library.canonical_path().to_string_lossy().into_owned(),
    )
    .unwrap();
    let removed = persistence
        .remove_photos_receiver(PhotoRemovalMutation {
            photo_ids: vec!["photo-2".to_owned()],
            operation_id: "operation-next".to_owned(),
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(removed.counts.removed, 1);
    let (records, _, _) = persistence
        .removed_photos_receiver(0, 10)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let marker = records
        .iter()
        .find(|record| record.photo_id == "photo-2")
        .unwrap()
        .removed_at_ms;
    assert!(marker > held);
    let held_marker = records
        .iter()
        .find(|record| record.photo_id == "photo-1")
        .unwrap()
        .removed_at_ms;
    assert_eq!(held_marker, held);
}

#[tokio::test]
async fn removal_markers_never_repeat_even_when_the_clock_does_not_advance() {
    let (_base, library, state, name, path) = fixture();
    seed(
        &path,
        include_str!("../../../../../compatibility/sqlite/schema-v8.sql"),
    );
    // The high water mark is seeded far ahead of the clock, so a marker
    // derived from the clock alone would repeat the marker already stored
    // for the Photo and this test would see a stale listing clear a newer
    // removal.
    let ahead = unix_millis() + 60_000;
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
            [library.canonical_path().to_str().unwrap()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES('removal_marker_high_water',?)",
            [ahead.to_string()],
        )
        .unwrap();
    add_recipe_test_photo(
        &connection,
        RecipeTestPhoto {
            original_id: "original-1",
            photo_id: "photo-1",
            relative_path: "shoot/one-1.ARW",
            kind: "raw",
            available: true,
            size: 17,
            mtime_ms: 1_000.0,
        },
    );
    connection
        .execute(
            "UPDATE photos SET selection_state='rejected' WHERE id='photo-1'",
            [],
        )
        .unwrap();
    drop(connection);

    let persistence = Persistence::open(
        state,
        name,
        library.canonical_path().to_string_lossy().into_owned(),
    )
    .unwrap();
    let remove = |operation_id: &str| {
        persistence
            .remove_photos_receiver(PhotoRemovalMutation {
                photo_ids: vec!["photo-1".to_owned()],
                operation_id: operation_id.to_owned(),
            })
            .unwrap()
    };
    let restore = |marker: i64| {
        persistence
            .restore_photos_receiver(PhotoRestoration::Photos(vec![PhotoRemovalMarker {
                photo_id: "photo-1".to_owned(),
                removed_at_ms: marker,
            }]))
            .unwrap()
    };
    let marker_of = async |persistence: &Persistence| -> Option<i64> {
        let (records, _, _) = persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        records
            .iter()
            .find(|record| record.photo_id == "photo-1")
            .map(|record| record.removed_at_ms)
    };

    assert_eq!(
        remove("operation-one")
            .await
            .unwrap()
            .unwrap()
            .counts
            .removed,
        1
    );
    let first = marker_of(&persistence).await.unwrap();
    assert!(first > ahead);
    assert_eq!(restore(first).await.unwrap().unwrap().counts.restored, 1);

    assert_eq!(
        remove("operation-two")
            .await
            .unwrap()
            .unwrap()
            .counts
            .removed,
        1
    );
    let second = marker_of(&persistence).await.unwrap();
    assert!(second > first);

    // The listing read under the first removal names a marker the Library
    // no longer assigns to this Photo, so it restores nothing.
    let superseded = restore(first).await.unwrap().unwrap();
    assert_eq!(superseded.counts.restored, 0);
    assert_eq!(superseded.changed_elsewhere, vec!["photo-1".to_owned()]);
    assert_eq!(marker_of(&persistence).await, Some(second));
    assert_eq!(restore(second).await.unwrap().unwrap().counts.restored, 1);
    assert_eq!(marker_of(&persistence).await, None);
}

#[tokio::test]
async fn removal_outcomes_operation_identity_and_restore_are_exact() {
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
    for (index, state_value) in [(1, "rejected"), (2, "undecided"), (3, "rejected")] {
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
                "UPDATE photos SET selection_state=? WHERE id=?",
                params![state_value, format!("photo-{index}")],
            )
            .unwrap();
    }
    drop(connection);

    let persistence = Persistence::open(
        state,
        name,
        library.canonical_path().to_string_lossy().into_owned(),
    )
    .unwrap();
    let remove = |photo_ids: Vec<&str>, operation_id: &str| {
        persistence
            .remove_photos_receiver(PhotoRemovalMutation {
                photo_ids: photo_ids.into_iter().map(str::to_owned).collect(),
                operation_id: operation_id.to_owned(),
            })
            .unwrap()
    };
    let result = remove(vec!["photo-1", "photo-2", "photo-missing"], "operation-one")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.removed, vec!["photo-1".to_owned()]);
    assert_eq!(result.changed_elsewhere, vec!["photo-2".to_owned()]);
    assert_eq!(result.missing, vec!["photo-missing".to_owned()]);
    assert!(result.already_removed.is_empty());
    // One outcome per requested Photo: the counts are the lists.
    assert_eq!(
        result.counts,
        PhotoRemovalCounts {
            removed: 1,
            changed_elsewhere: 1,
            missing: 1,
            already_removed: 0,
        }
    );

    // A retried request repeats its own operation instead of reporting a
    // second outcome set.
    let retried = remove(vec!["photo-1", "photo-2", "photo-missing"], "operation-one")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retried.counts.removed, 1);
    assert_eq!(retried.changed_elsewhere, vec!["photo-2".to_owned()]);
    assert_eq!(retried.missing, vec!["photo-missing".to_owned()]);
    assert!(retried.already_removed.is_empty());

    let other = remove(vec!["photo-1"], "operation-two")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(other.counts.removed, 0);
    assert_eq!(other.counts.already_removed, 1);
    assert_eq!(other.already_removed, vec!["photo-1".to_owned()]);

    let (records, total, _) = persistence
        .removed_photos_receiver(0, 10)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(records.len(), 1);
    assert!(records.iter().all(|record| record.removed_at_ms >= 0));

    // Restore by operation returns the group that operation still owns.
    let restored = persistence
        .restore_photos_receiver(PhotoRestoration::Operation("operation-one".to_owned()))
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.counts.restored, 1);
    assert_eq!(restored.counts.missing, 0);
    assert_eq!(
        restored.restored.iter().cloned().collect::<HashSet<_>>(),
        HashSet::from(["photo-1".to_owned()])
    );
    // The durable receipt makes a retry after an explicit restore return
    // the original outcome without removing the Photo again.
    let retry_after_restore = remove(vec!["photo-1", "photo-2", "photo-missing"], "operation-one")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry_after_restore.counts.removed, 1);
    assert_eq!(
        persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .1,
        0
    );
    let second = persistence
        .restore_photos_receiver(PhotoRestoration::Operation("operation-one".to_owned()))
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.counts.restored, 0);

    // A named restore compares and sets against the marker it reviewed:
    // a Photo already in the Library, and a Photo that does not exist, are
    // reported instead of cleared.
    let named = persistence
        .restore_photos_receiver(PhotoRestoration::Photos(vec![
            PhotoRemovalMarker {
                photo_id: "photo-1".to_owned(),
                removed_at_ms: 0,
            },
            PhotoRemovalMarker {
                photo_id: "photo-missing".to_owned(),
                removed_at_ms: 0,
            },
        ]))
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(named.changed_elsewhere, vec!["photo-1".to_owned()]);
    assert_eq!(named.missing, vec!["photo-missing".to_owned()]);
    assert!(named.operations.is_empty());

    // A stale marker never overwrites a newer removal: the Photo is
    // reported as changed elsewhere and stays removed by its own
    // operation, whose remaining count the response carries.
    persistence
        .mutate_photo_state(PhotoStateMutation {
            photo_id: "photo-2".to_owned(),
            field: PhotoStateField::SelectionState,
            value: PhotoStateValue::Selection(SelectionState::Rejected),
            expected_current: None,
            album_id: None,
        })
        .await
        .unwrap();
    let re_removed = remove(vec!["photo-2"], "operation-three")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(re_removed.counts.removed, 1);
    let (records, _, _) = persistence
        .removed_photos_receiver(0, 10)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let stale_marker = records
        .iter()
        .find(|record| record.photo_id == "photo-2")
        .unwrap()
        .removed_at_ms;
    let stale = persistence
        .restore_photos_receiver(PhotoRestoration::Photos(vec![PhotoRemovalMarker {
            photo_id: "photo-2".to_owned(),
            removed_at_ms: stale_marker - 1,
        }]))
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stale.counts.restored, 0);
    assert_eq!(stale.changed_elsewhere, vec!["photo-2".to_owned()]);
    assert!(stale.operations.is_empty());
    let (records, total, _) = persistence
        .removed_photos_receiver(0, 10)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(records[0].photo_id, "photo-2");

    // The reviewed marker restores exactly that removal, and the response
    // reports that the operation now owns nothing.
    let exact = persistence
        .restore_photos_receiver(PhotoRestoration::Photos(vec![PhotoRemovalMarker {
            photo_id: "photo-2".to_owned(),
            removed_at_ms: stale_marker,
        }]))
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exact.restored, vec!["photo-2".to_owned()]);
    assert_eq!(
        exact.operations,
        vec![PhotoOperationRemainder {
            operation_id: "operation-three".to_owned(),
            removed: 0,
        }]
    );
    let (records, total, _) = persistence
        .removed_photos_receiver(0, 10)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(total, 0);
    assert!(records.is_empty());

    // A marker is never reused. A Photo restored and removed again carries
    // a strictly greater marker, so the listing read under the first
    // removal can never clear the second one — even when both removals
    // fall inside the same clock millisecond.
    let re_removed = remove(vec!["photo-2"], "operation-four")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(re_removed.counts.removed, 1);
    let (records, _, _) = persistence
        .removed_photos_receiver(0, 10)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let second_marker = records
        .iter()
        .find(|record| record.photo_id == "photo-2")
        .unwrap()
        .removed_at_ms;
    assert!(second_marker > stale_marker);
    let superseded = persistence
        .restore_photos_receiver(PhotoRestoration::Photos(vec![PhotoRemovalMarker {
            photo_id: "photo-2".to_owned(),
            removed_at_ms: stale_marker,
        }]))
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(superseded.counts.restored, 0);
    assert_eq!(superseded.changed_elsewhere, vec!["photo-2".to_owned()]);
    let (records, total, _) = persistence
        .removed_photos_receiver(0, 10)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(records[0].removed_at_ms, second_marker);

    // Removal is Library state only: decisions and identity are untouched.
    let snapshot = persistence
        .snapshot_receiver()
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let photo = snapshot
        .photos
        .iter()
        .find(|photo| photo.id == "photo-1")
        .unwrap();
    assert_eq!(photo.selection_state, SelectionState::Rejected);
    assert!(!photo.removed);
    assert_eq!(photo.original_id, "original-1");
    persistence.shutdown().unwrap();
}

/// A removed Photo leaves every normal source and Album count while its
/// membership rows stay intact for restore.
#[tokio::test]
async fn removed_photos_leave_normal_sources_and_album_counts() {
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

    let persistence = Persistence::open(
        state,
        name,
        library.canonical_path().to_string_lossy().into_owned(),
    )
    .unwrap();
    let album = persistence
        .mutate_album_receiver(AlbumMutation::Create {
            name: "Keepers".to_owned(),
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    persistence
        .mutate_album_receiver(AlbumMutation::AddMembers {
            album_id: album.album_id.clone(),
            photo_ids: vec!["photo-1".to_owned(), "photo-2".to_owned()],
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    persistence
        .remove_photos_receiver(PhotoRemovalMutation {
            photo_ids: vec!["photo-1".to_owned()],
            operation_id: "operation-one".to_owned(),
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();

    let summaries = persistence
        .list_album_summaries_receiver()
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].photo_count, 1);
    let album_read = persistence
        .album_receiver(&album.album_id)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(album_read.photo_count, 1);
    let target = persistence
        .album_browse_target_receiver(&album.album_id)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        target
            .members
            .iter()
            .map(|member| member.photo_id.clone())
            .collect::<Vec<_>>(),
        vec!["photo-2".to_owned()]
    );
    let albums = persistence
        .list_albums_receiver()
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(albums[0].members.len(), 1);

    // The membership row survives for restore.
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM album_members WHERE album_id=?",
                [&album.album_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
    drop(connection);

    let snapshot = persistence
        .snapshot_receiver()
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let projection = query_projection(&snapshot);
    let ids = persistence
        .create_photo_query_receiver(
            PhotoQuery {
                source: PhotoQuerySource::AllPhotos,
                selection_state: None,
                rating_minimum: None,
                rating_maximum: None,
                original_kind: None,
                original_available: None,
                captured_from: None,
                captured_before: None,
                order: PhotoQueryOrder::CaptureTimeAscending,
            },
            projection,
            100,
        )
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ids, vec!["photo-2".to_owned()]);
    persistence.shutdown().unwrap();
}
