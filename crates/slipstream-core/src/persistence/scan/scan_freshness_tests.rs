use super::super::test_support::*;
use super::super::{Persistence, ScanRecoveryPlan};
use crate::OriginalKind;

/// Deferred candidates hold identity without publishing a duplicate, while
/// safe additions in the same scan still appear.
#[tokio::test]
async fn deferred_candidates_hold_identity_and_publish_safe_additions() {
    let (_base, library, state, name, _path) = fixture();
    let persistence = Persistence::open(
        state,
        name,
        library.canonical_path().to_string_lossy().into_owned(),
    )
    .unwrap();
    let published = discovered("shoot/one.JPG", OriginalKind::Jpeg, 4, 1000.0);
    let first = persistence
        .apply_scan(vec![published.clone()], Vec::new())
        .await
        .unwrap();
    let photo_id = first.photos[0].id.clone();
    let original_id = first.originals[0].id.clone();
    let published_facts = first.originals[0].facts;

    let changed = discovered("shoot/one.JPG", OriginalKind::Jpeg, 9, 2000.0);
    let safe = discovered("incoming/safe.JPG", OriginalKind::Jpeg, 3, 2000.0);
    let second = persistence
        .apply_scan_recovered(
            vec![changed, safe],
            Vec::new(),
            ScanRecoveryPlan {
                deferred_paths: ["shoot/one.JPG".to_owned(), "incoming/new.JPG".to_owned()].into(),
                pending_changed_paths: vec!["shoot/one.JPG".to_owned()],
                pending_folders: vec![
                    crate::PendingFolder {
                        relative_path: "incoming".to_owned(),
                        pending_files: 1,
                        pending_bytes: 9,
                    },
                    crate::PendingFolder {
                        relative_path: "shoot".to_owned(),
                        pending_files: 1,
                        pending_bytes: 9,
                    },
                ],
                ..ScanRecoveryPlan::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(second.snapshot.photos.len(), 2);
    assert!(
        second
            .snapshot
            .photos
            .iter()
            .any(|photo| photo.id == photo_id)
    );
    assert!(
        second
            .snapshot
            .originals
            .iter()
            .any(|original| original.id == original_id && original.facts == published_facts)
    );
    assert!(
        !second
            .snapshot
            .originals
            .iter()
            .any(|original| { original.relative_path.as_str() == "incoming/new.JPG" })
    );
    assert!(
        second
            .snapshot
            .originals
            .iter()
            .any(|original| { original.relative_path.as_str() == "incoming/safe.JPG" })
    );
    assert_eq!(second.snapshot.pending_folders.len(), 2);
    assert_eq!(
        second.snapshot.pending_changed_paths,
        ["shoot/one.JPG".to_owned()]
    );
    assert_eq!(
        persistence.snapshot().await.unwrap().pending_folders,
        second.snapshot.pending_folders
    );
    assert_eq!(
        persistence.snapshot().await.unwrap().pending_changed_paths,
        second.snapshot.pending_changed_paths
    );
    persistence.shutdown().unwrap();
}
