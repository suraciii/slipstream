use super::*;
use crate::persistence::admission::StateDirectory;
use crate::persistence::albums;
use crate::persistence::test_support::*;
use crate::{
    AlbumMembershipMutation, AlbumMutation, AlbumQueryFilter, CheckedAlbumMutation, OriginalKind,
    PhotoStateBatchItem, PhotoStateBatchMutation, PhotoStateField, PhotoStateMutation,
    PhotoStateValue, SelectionState,
};
use std::{fs, os::unix::fs::PermissionsExt};
use tokio::sync::oneshot;

#[tokio::test]
async fn effective_writers_advance_versions_while_noops_and_progress_do_not() {
    let (_base, library, state, name, path) = fixture();
    let persistence = Persistence::open(
        state,
        name,
        library.canonical_path().to_string_lossy().into_owned(),
    )
    .unwrap();
    let snapshot = persistence
        .apply_scan(
            vec![discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0)],
            Vec::new(),
        )
        .await
        .unwrap();
    let photo_id = snapshot.photos[0].id.clone();
    let read_photo = || async {
        persistence
            .photo_receiver(&photo_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    };
    let initial_photo_version = read_photo().await.decision_version;
    persistence
        .mutate_photo_state(PhotoStateMutation {
            photo_id: photo_id.clone(),
            field: PhotoStateField::Rating,
            value: PhotoStateValue::Rating(0),
            expected_current: None,
            album_id: None,
        })
        .await
        .unwrap();
    assert_eq!(read_photo().await.decision_version, initial_photo_version);
    persistence
        .mutate_photo_state(PhotoStateMutation {
            photo_id: photo_id.clone(),
            field: PhotoStateField::SelectionState,
            value: PhotoStateValue::Selection(SelectionState::Picked),
            expected_current: None,
            album_id: None,
        })
        .await
        .unwrap();
    let changed_photo_version = read_photo().await.decision_version;
    assert_ne!(changed_photo_version, initial_photo_version);
    persistence
        .mutate_photo_state_batch_receiver(PhotoStateBatchMutation {
            photos: vec![PhotoStateBatchItem {
                photo_id: photo_id.clone(),
                expected_current: SelectionState::Picked,
            }],
            value: SelectionState::Picked,
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read_photo().await.decision_version, changed_photo_version);
    assert!(
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: photo_id.clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Rejected),
                expected_current: Some(PhotoStateValue::Selection(SelectionState::Unflagged)),
                album_id: None,
            })
            .await
            .is_err()
    );
    assert_eq!(read_photo().await.decision_version, changed_photo_version);
    persistence
        .mutate_photo_state(PhotoStateMutation {
            photo_id: photo_id.clone(),
            field: PhotoStateField::SelectionState,
            value: PhotoStateValue::Selection(SelectionState::Unflagged),
            expected_current: Some(PhotoStateValue::Selection(SelectionState::Picked)),
            album_id: None,
        })
        .await
        .unwrap();
    let changed_back_photo_version = read_photo().await.decision_version;
    assert_ne!(changed_back_photo_version, initial_photo_version);
    assert_ne!(changed_back_photo_version, changed_photo_version);

    let album_id = persistence
        .mutate_album(AlbumMutation::Create {
            name: "Review".to_owned(),
        })
        .await
        .unwrap()
        .album_id;
    let read_album = || async {
        persistence
            .album_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    };
    let initial_album_version = read_album().await.album_version;
    let named_ids = persistence
        .create_album_query_receiver(AlbumQueryFilter::ExactName("review".to_owned()), 10)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(named_ids, vec![album_id.clone()]);
    persistence
        .mutate_album_membership(AlbumMembershipMutation::Add {
            album_id: album_id.clone(),
            photo_ids: vec![photo_id.clone()],
        })
        .await
        .unwrap();
    let membership_version = read_album().await.album_version;
    assert_ne!(membership_version, initial_album_version);
    let containing_ids = persistence
        .create_album_query_receiver(AlbumQueryFilter::ContainsPhoto(photo_id.clone()), 10)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(containing_ids, vec![album_id.clone()]);
    let album_window = persistence
        .albums_by_id_receiver(vec![album_id.clone(), "removed".to_owned()])
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(album_window[0].as_ref().unwrap().photo_count, 1);
    assert!(album_window[1].is_none());
    persistence
        .mutate_album(AlbumMutation::SetProgress {
            album_id: album_id.clone(),
            photo_id: photo_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(read_album().await.album_version, membership_version);
    persistence
        .mutate_album(AlbumMutation::Rename {
            album_id: album_id.clone(),
            name: "Review".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(read_album().await.album_version, membership_version);
    persistence
        .mutate_album(AlbumMutation::Rename {
            album_id: album_id.clone(),
            name: "Final".to_owned(),
        })
        .await
        .unwrap();
    let final_album_version = read_album().await.album_version;
    assert_ne!(final_album_version, membership_version);

    // A sidecar detected at write admission refuses the transaction. The
    // previously issued guards remain valid because no commit occurred.
    fs::write(path.with_file_name("library.sqlite-wal"), b"blocked").unwrap();
    assert!(
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: photo_id.clone(),
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(5),
                expected_current: None,
                album_id: None,
            })
            .await
            .is_err()
    );
    assert_eq!(
        read_photo().await.decision_version,
        changed_back_photo_version
    );
    assert!(
        persistence
            .mutate_album(AlbumMutation::Rename {
                album_id: album_id.clone(),
                name: "Blocked".to_owned(),
            })
            .await
            .is_err()
    );
    assert_eq!(read_album().await.album_version, final_album_version);
}

#[tokio::test]
async fn reopening_persistence_invalidates_process_epoch_versions() {
    let (base, library, state, name, _path) = fixture();
    let canonical_root = library.canonical_path().to_string_lossy().into_owned();
    let persistence = Persistence::open(state, name.clone(), canonical_root.clone()).unwrap();
    let snapshot = persistence
        .apply_scan(
            vec![discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0)],
            Vec::new(),
        )
        .await
        .unwrap();
    let photo_id = snapshot.photos[0].id.clone();
    let first = persistence
        .photo_receiver(&photo_id)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .decision_version;
    let album = persistence
        .create_album_checked("Epoch".to_owned())
        .await
        .unwrap()
        .album;
    persistence.shutdown().unwrap();

    let reopened_state = StateDirectory::open_or_create(&library, base.0.join("state")).unwrap();
    let reopened = Persistence::open(reopened_state, name, canonical_root).unwrap();
    let second = reopened
        .photo_receiver(&photo_id)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .decision_version;
    assert_ne!(first, second);
    let reopened_album = reopened
        .album_receiver(&album.id)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_ne!(album.album_version, reopened_album.album_version);
    assert_eq!(
        reopened
            .mutate_album_checked(CheckedAlbumMutation::Rename {
                album_id: album.id.clone(),
                name: album.name,
                expected_version: album.album_version,
            })
            .await,
        Err(albums::AlbumWriteError::VersionConflict {
            album_id: album.id,
            current_version: reopened_album.album_version,
        })
    );
}

#[tokio::test]
async fn unavailable_members_keep_state_and_sidecar_admission_blocks_writes() {
    let (_base, library, state, name, path) = fixture();
    let persistence = Persistence::open(
        state,
        name,
        library.canonical_path().to_string_lossy().into_owned(),
    )
    .unwrap();
    let snapshot = persistence
        .apply_scan(
            vec![discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0)],
            Vec::new(),
        )
        .await
        .unwrap();
    let photo_id = snapshot.photos[0].id.clone();
    persistence
        .mutate_photo_state(PhotoStateMutation {
            photo_id: photo_id.clone(),
            field: PhotoStateField::Rating,
            value: PhotoStateValue::Rating(4),
            expected_current: None,
            album_id: None,
        })
        .await
        .unwrap();
    let album_id = persistence
        .mutate_album(AlbumMutation::Create {
            name: "Keep".to_owned(),
        })
        .await
        .unwrap()
        .album_id;
    persistence
        .mutate_album(AlbumMutation::AddMembers {
            album_id: album_id.clone(),
            photo_ids: vec![photo_id.clone()],
        })
        .await
        .unwrap();
    persistence
        .apply_scan(Vec::new(), Vec::new())
        .await
        .unwrap();
    let member = &persistence.list_albums().await.unwrap()[0].members[0];
    assert!(!member.available);
    assert_eq!(member.rating, 4);
    let sidecar = path.with_file_name("library.sqlite-journal");
    fs::write(&sidecar, b"operator recovery data").unwrap();
    fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).unwrap();
    let before = fs::read(&path).unwrap();
    assert_eq!(
        persistence
            .mutate_album(AlbumMutation::Create {
                name: "Blocked".to_owned()
            })
            .await,
        Err(MutationError::Persistence)
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read(&sidecar).unwrap(), b"operator recovery data");
    persistence.shutdown().unwrap();
}

#[tokio::test]
async fn shutdown_rejects_library_mutations_after_lifecycle_close() {
    let (_base, library_root, state, name, _path) = fixture();
    let library = crate::Library::open(crate::LibraryConfig {
        library_root: library_root.canonical_path().to_owned(),
        state_directory: state.canonical_path().to_owned(),
        database_basename: name.as_os_str().to_string_lossy().into_owned(),
        ..crate::LibraryConfig::default()
    })
    .unwrap();
    library.shutdown().unwrap();
    assert!(matches!(
        library.list_albums().await,
        Err(crate::LibraryError::Closed)
    ));
    assert!(matches!(
        library
            .mutate_album(AlbumMutation::Create {
                name: "Nope".to_owned()
            })
            .await,
        Err(crate::LibraryError::Closed)
    ));
}

/// Builds the scan-owned query projection one Published Library would
/// share, from the same persisted snapshot the server publishes.
#[tokio::test]
async fn saturation_and_shutdown_drain_are_explicit() {
    let (_base, library, state, name, _path) = fixture();
    let persistence = Persistence::open_with_capacity(
        state,
        name,
        library.canonical_path().to_string_lossy().into_owned(),
        NonZeroUsize::new(1).unwrap(),
    )
    .unwrap();
    let (entered_send, entered_receive) = oneshot::channel();
    let (release_send, release_receive) = std::sync::mpsc::channel();
    let (reply, receive) = oneshot::channel();
    persistence
        .submit(Command::Block {
            entered: entered_send,
            release: release_receive,
            reply,
        })
        .unwrap();
    entered_receive.await.unwrap();
    let (queued_reply, queued_receive) = oneshot::channel();
    persistence.submit(Command::Probe(queued_reply)).unwrap();
    let (full_reply, _) = oneshot::channel();
    assert!(matches!(
        persistence.submit(Command::Probe(full_reply)),
        Err(PersistenceError::Saturated)
    ));

    let shutdown_handle = persistence.clone();
    let shutdown = tokio::task::spawn_blocking(move || shutdown_handle.shutdown());
    tokio::task::yield_now().await;
    release_send.send(()).unwrap();
    assert!(receive.await.unwrap().is_ok());
    assert!(queued_receive.await.unwrap().is_ok());
    assert!(shutdown.await.unwrap().is_ok());
    assert!(persistence.shutdown().is_ok());
    assert!(matches!(
        persistence.probe().await,
        Err(PersistenceError::Closed)
    ));
}
