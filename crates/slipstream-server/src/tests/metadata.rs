use super::*;

#[tokio::test]
async fn metadata_save_serializes_with_removal_and_rejects_removed_evidence() {
    use crate::metadata_service::{read_metadata, save_metadata};
    use crate::metadata_wire::MetadataErrorCode;
    use slipstream_core::{PhotoRemovalMutation, PhotoStateMutation};
    use std::os::unix::net::UnixListener;

    let (base, config) = prepare_fixture();
    let root = config.library_root;
    let original = root.join("one.JPG");
    fs::write(&original, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
    let library = Arc::new(
        Library::open(LibraryConfig {
            library_root: root.clone(),
            state_directory: config.state_directory,
            ..LibraryConfig::default()
        })
        .unwrap(),
    );
    let snapshot = library.scan().await.unwrap();
    let id = snapshot.photos[0].id.clone();
    let read = read_metadata(&library, &root, "lifecycle", None, &id)
        .await
        .unwrap();
    library
        .mutate_photo_state(PhotoStateMutation {
            photo_id: id.clone(),
            field: PhotoStateField::SelectionState,
            value: PhotoStateValue::Selection(SelectionState::Rejected),
            expected_current: None,
            album_id: None,
        })
        .await
        .unwrap();
    let request = || {
        serde_json::from_value(serde_json::json!({
            "evidence": read.evidence,
            "changes": {"xmp:Label": {"op":"set", "value":"review"}}
        }))
        .unwrap()
    };
    let socket = base.join("supervisor.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let supervisor = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        let mut byte = [0];
        loop {
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        let message: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(message["operation"], "status");
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        stream
            .write_all(b"{\"available\":false,\"reason\":\"maintenance\"}\n")
            .unwrap();
    });
    let save_library = Arc::clone(&library);
    let save_root = root.clone();
    let save_id = id.clone();
    let save_request = request();
    let save = tokio::spawn(async move {
        save_metadata(
            &save_library,
            &save_root,
            "lifecycle",
            Some(&socket),
            &save_id,
            save_request,
        )
        .await
    });
    entered_rx.await.unwrap();
    let removal = library.remove_photos(PhotoRemovalMutation {
        photo_ids: vec![id.clone()],
        operation_id: "metadata-removal".into(),
    });
    tokio::pin!(removal);
    let early = tokio::time::timeout(Duration::from_millis(100), &mut removal).await;
    release_tx.send(()).unwrap();
    let removed_early = early.is_ok();
    let removed = match early {
        Ok(result) => result,
        Err(_) => removal.await,
    }
    .unwrap();
    assert_eq!(
        save.await.unwrap().unwrap_err().code,
        MetadataErrorCode::SaveUnavailable
    );
    supervisor.join().unwrap();
    assert!(!removed_early, "Remove must queue behind the admitted Save");
    assert_eq!(removed.counts.removed, 1);
    let failure = save_metadata(&library, &root, "lifecycle", None, &id, request())
        .await
        .unwrap_err();
    assert_eq!(failure.code, MetadataErrorCode::PhotoRemoved);
    assert!(!root.join("one.xmp").exists());
    assert_eq!(fs::read(&original).unwrap(), [0xff, 0xd8, 0xff, 0xd9]);
    library.shutdown().unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn cancelled_external_metadata_retains_admission_until_supervisor_finishes() {
    use crate::metadata_wire::MetadataErrorCode;
    use std::os::unix::net::UnixListener;

    for saving in [false, true] {
        let (base, mut config) = prepare_fixture();
        jpeg_fixture(&config.library_root.join("one.JPG"), 8, 4, [1, 2, 3]);
        let socket = base.join("supervisor.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        config.metadata_supervisor = Some(socket);
        let application = Application::open(&config).await.unwrap();
        wait_for_scan_settled(&application).await;
        let id = browse_photo_ids(&application, BrowseSourceRequest::Library).await[0].clone();
        let deadline = Instant::now() + Duration::from_secs(5);
        while application.library.fingerprint_counts().enrolled != 1 {
            assert!(Instant::now() < deadline, "enrollment did not settle");
            tokio::task::yield_now().await;
        }
        let read = crate::metadata_service::read_metadata(
            &application.library,
            &config.library_root,
            "cancel-test",
            None,
            &id,
        )
        .await
        .unwrap();
        let request = serde_json::from_value(serde_json::json!({
            "evidence": read.evidence,
            "changes": {"xmp:Label": {"op":"set", "value":"cancelled"}}
        }))
        .unwrap();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let supervisor = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            let mut byte = [0];
            loop {
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            let message: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(message["operation"], "status");
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            stream
                .write_all(b"{\"available\":false,\"reason\":\"maintenance\"}\n")
                .unwrap();
        });
        let occupied = application.library.try_admit_native_work().unwrap();
        let worker = Arc::clone(&application);
        let worker_id = id.clone();
        let pending = tokio::spawn(async move {
            if saving {
                crate::metadata_service::save_metadata(
                    &worker.library,
                    &config.library_root,
                    "cancel-test",
                    config.metadata_supervisor.as_deref(),
                    &worker_id,
                    request,
                )
                .await
                .map(|_| ())
            } else {
                worker.external_metadata_read(&worker_id).await.map(|_| ())
            }
        });
        entered_rx.await.unwrap();
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        let refusal = tokio::time::timeout(
            Duration::from_millis(100),
            application.external_metadata_read(&id),
        )
        .await;
        release_tx.send(()).unwrap();
        supervisor.join().unwrap();
        assert_eq!(
            refusal
                .expect("cancelled work must retain capacity")
                .unwrap_err()
                .code,
            MetadataErrorCode::ResourceLimit
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(released) = application.library.try_admit_native_work() {
                drop(released);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "completed work retained admission"
            );
            tokio::task::yield_now().await;
        }
        drop(occupied);
        application.shutdown().await.unwrap();
        fs::remove_dir_all(base).unwrap();
    }
}
