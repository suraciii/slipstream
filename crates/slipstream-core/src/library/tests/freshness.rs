use super::tests::fixture;
use super::*;
use rusqlite::{Connection, params};
use std::fs;

#[tokio::test]
async fn automatic_budget_defers_candidate_and_full_check_recovers_it() {
    let (_base, config) = fixture();
    fs::create_dir_all(config.library_root.join("shoot")).unwrap();
    fs::write(config.library_root.join("shoot/old.JPG"), b"same-bytes12").unwrap();
    let library = Library::open(config.clone()).unwrap();
    let initial = library.scan().await.unwrap();
    let original = initial.originals[0].clone();
    let photo_id = initial.photos[0].id.clone();
    library.shutdown().unwrap();

    let connection = Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "INSERT OR REPLACE INTO original_fingerprints(original_id,digest,size,mtime_ms) VALUES(?,?,?,?)",
            params![
                original.id,
                crate::digest_bytes(b"same-bytes12"),
                i64::try_from(original.facts.size).unwrap(),
                original.facts.mtime_ms,
            ],
        )
        .unwrap();
    drop(connection);
    fs::remove_file(config.library_root.join("shoot/old.JPG")).unwrap();
    fs::create_dir_all(config.library_root.join("incoming")).unwrap();
    fs::write(
        config.library_root.join("incoming/new.JPG"),
        b"same-bytes12",
    )
    .unwrap();

    let library = Library::open(config.clone()).unwrap();
    let deferred = library
        .scan_with_options(crate::RecoveryOptions {
            verification_scope: None,
            automatic_verification_budget: Some(0),
        })
        .await
        .unwrap();
    assert_eq!(deferred.photos.len(), 1);
    assert!(deferred.photos.iter().all(|photo| photo.id == photo_id));
    assert!(
        deferred
            .originals
            .iter()
            .all(|item| item.relative_path.as_str() != "incoming/new.JPG")
    );
    assert_eq!(
        deferred.pending_folders,
        [crate::PendingFolder {
            relative_path: "incoming".to_owned(),
            pending_files: 1,
            pending_bytes: 12,
        }]
    );

    let verified = library.scan().await.unwrap();
    assert_eq!(verified.photos.len(), 1);
    assert_eq!(verified.photos[0].id, photo_id);
    assert_eq!(
        verified.originals[0].relative_path.as_str(),
        "incoming/new.JPG"
    );
    assert!(verified.pending_folders.is_empty());
    library.shutdown().unwrap();
}

#[tokio::test]
async fn deferred_changed_candidate_stays_pending_until_verified() {
    let (_base, config) = fixture();
    fs::create_dir_all(config.library_root.join("shoot")).unwrap();
    fs::write(config.library_root.join("shoot/a.JPG"), b"aaaa").unwrap();
    fs::write(config.library_root.join("shoot/b.JPG"), b"bbbb").unwrap();
    let library = Library::open(config.clone()).unwrap();
    let initial = library.scan().await.unwrap();
    let a = initial
        .originals
        .iter()
        .find(|original| original.relative_path.as_str() == "shoot/a.JPG")
        .unwrap();
    let photo_id = initial
        .photos
        .iter()
        .find(|photo| photo.original_id == a.id)
        .unwrap()
        .id
        .clone();
    let published = a.facts;
    library.shutdown().unwrap();

    let connection = Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    for original in &initial.originals {
        let bytes = if original.relative_path.as_str().ends_with("a.JPG") {
            b"aaaa".as_slice()
        } else {
            b"bbbb".as_slice()
        };
        connection
            .execute(
                "INSERT OR REPLACE INTO original_fingerprints(original_id,digest,size,mtime_ms) VALUES(?,?,?,?)",
                params![
                    original.id,
                    crate::digest_bytes(bytes),
                    i64::try_from(original.facts.size).unwrap(),
                    original.facts.mtime_ms,
                ],
            )
            .unwrap();
    }
    drop(connection);

    fs::write(config.library_root.join("shoot/a.JPG"), b"AAAAA").unwrap();
    fs::write(config.library_root.join("shoot/b.JPG"), b"BBBBB").unwrap();
    let library = Library::open(config.clone()).unwrap();
    let automatic = crate::RecoveryOptions {
        verification_scope: None,
        automatic_verification_budget: Some(0),
    };
    let deferred = library.scan_with_options(automatic.clone()).await.unwrap();
    assert_eq!(deferred.pending_changed_paths.len(), 2);
    assert_eq!(deferred.pending_folders[0].pending_files, 2);

    fs::remove_file(config.library_root.join("shoot/b.JPG")).unwrap();
    let retained = library.scan_with_options(automatic).await.unwrap();
    assert_eq!(
        retained.pending_changed_paths,
        vec!["shoot/a.JPG".to_owned()]
    );
    assert_eq!(
        retained.pending_folders,
        [crate::PendingFolder {
            relative_path: "shoot".to_owned(),
            pending_files: 1,
            pending_bytes: 5,
        }]
    );
    assert!(
        retained
            .originals
            .iter()
            .any(|original| original.facts == published)
    );

    let verified = library.scan().await.unwrap();
    assert!(verified.pending_changed_paths.is_empty());
    assert!(verified.pending_folders.is_empty());
    assert!(verified.photos.iter().any(|photo| photo.id == photo_id));
    assert!(
        verified
            .originals
            .iter()
            .any(|original| original.facts.size == 5)
    );
    library.shutdown().unwrap();
}

#[tokio::test]
async fn new_candidate_waits_when_a_changed_fingerprint_has_its_shape() {
    let (_base, config) = fixture();
    fs::create_dir_all(config.library_root.join("shoot")).unwrap();
    fs::write(config.library_root.join("shoot/one.JPG"), b"aaaa").unwrap();
    let library = Library::open(config.clone()).unwrap();
    let initial = library.scan().await.unwrap();
    let original = initial.originals[0].clone();
    library.shutdown().unwrap();

    let connection = Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "INSERT OR REPLACE INTO original_fingerprints(original_id,digest,size,mtime_ms) VALUES(?,?,?,?)",
            params![
                original.id,
                crate::digest_bytes(b"aaaa"),
                i64::try_from(original.facts.size).unwrap(),
                original.facts.mtime_ms,
            ],
        )
        .unwrap();
    drop(connection);
    fs::write(config.library_root.join("shoot/one.JPG"), b"bbbbb").unwrap();
    fs::create_dir_all(config.library_root.join("incoming")).unwrap();
    fs::write(config.library_root.join("incoming/new.JPG"), b"aaaa").unwrap();

    let library = Library::open(config.clone()).unwrap();
    let deferred = library
        .scan_with_options(crate::RecoveryOptions {
            verification_scope: None,
            automatic_verification_budget: Some(0),
        })
        .await
        .unwrap();
    assert_eq!(deferred.photos.len(), 1);
    assert!(
        deferred
            .originals
            .iter()
            .all(|item| item.relative_path.as_str() != "incoming/new.JPG")
    );
    assert_eq!(
        deferred.pending_folders,
        [crate::PendingFolder {
            relative_path: "incoming".to_owned(),
            pending_files: 1,
            pending_bytes: 4,
        }]
    );

    let verified = library.scan().await.unwrap();
    assert_eq!(verified.photos.len(), 2);
    assert!(verified.pending_folders.is_empty());
    assert!(
        verified
            .originals
            .iter()
            .any(|item| item.relative_path.as_str() == "incoming/new.JPG")
    );
    library.shutdown().unwrap();
}
