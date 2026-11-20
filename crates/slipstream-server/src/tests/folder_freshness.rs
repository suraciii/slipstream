use super::*;

#[tokio::test]
async fn folder_check_reports_scope_refuses_a_different_cycle_and_joins_its_scope() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;

    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let receive = application
        .admit_scan_cycle(
            slipstream_core::RecoveryOptions::folder("shoot".to_owned()),
            Some(gate_receiver),
            None,
        )
        .unwrap();
    assert_eq!(application.scan_status().scope.as_deref(), Some("shoot"));
    assert!(
        application
            .admit_scan_cycle(slipstream_core::RecoveryOptions::full(), None, None,)
            .is_err()
    );
    let joined = application
        .admit_scan_cycle(
            slipstream_core::RecoveryOptions::folder("shoot".to_owned()),
            None,
            None,
        )
        .unwrap();

    gate_sender.send(()).unwrap();
    let status = receive.await.unwrap().unwrap();
    let joined_status = joined.await.unwrap().unwrap();
    assert_eq!(status.state, joined_status.state);
    assert_eq!(status.scope, joined_status.scope);
    assert_eq!(status.state, "idle");
    assert_eq!(status.scope, None);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
