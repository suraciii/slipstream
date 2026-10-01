use super::{
    Backend, checked_setup_observation, command_until, create_control_directory,
    create_native_gate, create_private_directory, limits_match, observed_exit, verify_slice_phase,
    worker_result,
};
use crate::journal::{ManagerPhase, Record};
use crate::protocol::{
    Cleanup, Config, ErrorCode, Events, Evidence, Limits, Outcome, RESULT_BYTES, Receipt, Runtime,
    State, TerminalSnapshot, Workload,
};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::time::{Duration, Instant};

#[test]
fn persisted_terminal_snapshot_must_match_record_identity_and_receipt_facts() {
    let config = Config {
        version: 1,
        mode: "qualification".into(),
        instance: "0".repeat(32),
        root: "/tmp/slipstream-terminal-evidence".into(),
        socket: "/tmp/slipstream-terminal-evidence.sock".into(),
        peer_uid: 1000,
        image: format!("sha256:{}", "1".repeat(64)),
        memory_bytes: 128 * 1024 * 1024,
        receipt_retention_seconds: 86400,
    };
    let backend = Backend {
        config: config.clone(),
    };
    let launch_id = "2".repeat(32);
    let attempt_unit = format!("slipstreamprocessing{}-{launch_id}.slice", config.instance);
    let container_id = "3".repeat(64);
    let mut record = Record {
        receipt: Receipt {
            incarnation: "4".repeat(32),
            sequence: 5,
            workload: Workload::ProbeSuccess,
            policy: "5".repeat(64),
            bundle: "6".repeat(64),
            state: State::Settling,
            cancellation_requested: false,
            accepted_at_unix_ms: 0,
            deadline_unix_ms: 30000,
            outcome: Some(Outcome::Completed),
            runtime: Some(Runtime {
                launch_id: launch_id.clone(),
                container_id: Some(container_id.clone()),
                attempt_unit: attempt_unit.clone(),
            }),
            limits: Limits::new(config.memory_bytes),
            evidence: None,
            cleanup: Cleanup::Pending,
        },
        launch_id: launch_id.clone(),
        image_id: format!("sha256:{}", "7".repeat(64)),
        unit_invocation: Some("8".repeat(32)),
        cgroup_inode: Some(9),
        mount_id: None,
        released: true,
        termination_reason: None,
        manager_pending: None,
        stop_confirmed: false,
        settled_at_unix_ms: None,
        film: None,
    };
    let cgroup_path = backend.parent_path().join(&attempt_unit);
    record.receipt.evidence = Some(Evidence {
        peak_bytes: 123,
        exit_code: Some(0),
        docker_oom_killed: Some(false),
        attempt_before: Some(Events::default()),
        attempt_after: Some(Events::default()),
        parent_before: Some(Events::default()),
        parent_after: Some(Events::default()),
        populated: Some(false),
        terminal_snapshot: Some(TerminalSnapshot {
            cgroup_path: cgroup_path.to_str().unwrap().to_owned(),
            cgroup_inode: 9,
            unit_invocation: "8".repeat(32),
            launch_id,
            container_id,
            attempt_unit,
            incarnation: record.receipt.incarnation.clone(),
            sequence: record.receipt.sequence,
            memory_peak_raw: "123\n".into(),
            memory_max_raw: format!("{}\n", config.memory_bytes),
            memory_swap_current_raw: "0\n".into(),
            memory_swap_max_raw: "0\n".into(),
            memory_events_raw: "oom 0\noom_kill 0\noom_group_kill 0\n".into(),
            memory_events_local_raw: "oom 0\noom_kill 0\noom_group_kill 0\n".into(),
            io_stat_raw: Some("8:0 rbytes=1 wbytes=2 rios=3 wios=4 cost.usage=9\n".into()),
        }),
    });
    assert_eq!(backend.validate_terminal_evidence(&record), Ok(()));
    let mut before_create = record.clone();
    before_create.receipt.runtime.as_mut().unwrap().container_id = None;
    before_create
        .receipt
        .evidence
        .as_mut()
        .unwrap()
        .terminal_snapshot = None;
    assert_eq!(backend.validate_terminal_evidence(&before_create), Ok(()));

    for change in [
        "missing",
        "peak",
        "events",
        "path",
        "inode",
        "invocation",
        "launch",
        "container",
        "unit",
        "incarnation",
        "sequence",
        "limit",
    ] {
        let mut changed = record.clone();
        let evidence = changed.receipt.evidence.as_mut().unwrap();
        let snapshot = evidence.terminal_snapshot.as_mut().unwrap();
        match change {
            "missing" => evidence.terminal_snapshot = None,
            "peak" => snapshot.memory_peak_raw = "124\n".into(),
            "events" => {
                snapshot.memory_events_local_raw = "oom 1\noom_kill 0\noom_group_kill 0\n".into()
            }
            "path" => snapshot.cgroup_path.push('x'),
            "inode" => snapshot.cgroup_inode += 1,
            "invocation" => snapshot.unit_invocation.push('x'),
            "launch" => snapshot.launch_id.push('x'),
            "container" => snapshot.container_id.push('x'),
            "unit" => snapshot.attempt_unit.push('x'),
            "incarnation" => snapshot.incarnation.push('x'),
            "sequence" => snapshot.sequence += 1,
            "limit" => snapshot.memory_max_raw = "65536\n".into(),
            _ => unreachable!(),
        }
        assert_eq!(
            backend.validate_terminal_evidence(&changed),
            Err(ErrorCode::Uncertain),
            "{change}"
        );
    }
}

#[test]
fn workspace_creation_admits_the_sealed_attempt_workspace() {
    let root = std::env::temp_dir().join(format!(
        "slipstream-workspace-reuse-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let workspace = root.join("attempt");

    // The seal creates the attempt workspace first; provisioning derives
    // the same path and must be admitted.
    create_private_directory(&workspace).unwrap();
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o777)).unwrap();
    create_private_directory(&workspace).unwrap();
    assert_eq!(fs::metadata(&workspace).unwrap().mode() & 0o777, 0o700);

    // A link or a non-directory is never admitted.
    let link = root.join("link");
    std::os::unix::fs::symlink(&workspace, &link).unwrap();
    assert!(create_private_directory(&link).is_err());
    let file = root.join("file");
    fs::write(&file, b"x").unwrap();
    assert!(create_private_directory(&file).is_err());

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn workspace_control_and_native_gate_modes_ignore_restrictive_umask() {
    const CHILD: &str = "SLIPSTREAM_CONTROL_MODE_TEST_CHILD_7C2A";
    if std::env::var_os(CHILD).is_some() {
        // SAFETY: this test branch runs in a dedicated subprocess, so the
        // process-wide umask cannot affect other parallel tests.
        unsafe { libc::umask(0o077) };
        let root = std::env::temp_dir().join(format!(
            "slipstream-control-mode-test-{}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let workspace = root.join("attempt");
        create_private_directory(&workspace).unwrap();
        let control = workspace.join("control");
        create_control_directory(&control).unwrap();
        let gate = create_native_gate(&control.join("gate")).unwrap();

        assert_eq!(fs::metadata(&workspace).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&control).unwrap().mode() & 0o777, 0o755);
        assert_eq!(
            fs::metadata(control.join("gate")).unwrap().mode() & 0o777,
            0o644
        );

        drop(gate);
        fs::remove_dir_all(root).unwrap();
        return;
    }

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "backend::tests::workspace_control_and_native_gate_modes_ignore_restrictive_umask",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "umask subprocess failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn created_or_never_started_containers_have_no_observed_execution_exit() {
    let mut state =
        serde_json::json!({"Status":"created", "StartedAt":"0001-01-01T00:00:00Z", "ExitCode":0});
    assert_eq!(observed_exit(&state).unwrap(), None);
    state["Status"] = "exited".into();
    assert_eq!(observed_exit(&state).unwrap(), None);
    state["StartedAt"] = "2026-09-22T00:00:00Z".into();
    assert_eq!(observed_exit(&state).unwrap(), Some(0));
    state["ExitCode"] = 137.into();
    assert_eq!(observed_exit(&state).unwrap(), Some(137));
    state["Status"] = "running".into();
    assert_eq!(observed_exit(&state).unwrap(), None);
}

#[test]
fn observed_setup_drift_is_persisted_but_missing_limits_and_bootstrap_exits_are_not_tampering() {
    let root = std::env::temp_dir().join(format!("slipstream-setup-limits-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let backend = Backend {
        config: Config {
            version: 3,
            mode: "film-qualified-fixtures".into(),
            instance: "0".repeat(32),
            root: root.to_string_lossy().into_owned(),
            socket: "/test.sock".into(),
            peer_uid: 0,
            image: format!("sha256:{}", "1".repeat(64)),
            memory_bytes: 8 << 30,
            receipt_retention_seconds: 1,
        },
    };
    let mut record = crate::journal::tests::qualified_record(1);
    let mut persisted = Vec::new();
    let mut save = |record: &Record| {
        persisted.push(serde_json::to_vec(record).unwrap());
        Ok(())
    };
    let absent = limits_match(&root, &backend.config.limits(), 8 << 30);
    assert!(absent.is_err());
    assert!(checked_setup_observation(&mut record, &mut save, absent).is_err());
    assert_eq!(
        record
            .film
            .as_ref()
            .unwrap()
            .qualification_observation_valid,
        Some(true)
    );
    for (key, value) in [
        ("memory.max", (8u64 << 30).to_string()),
        ("memory.swap.max", "0".into()),
        ("cpu.max", "400000 100000".into()),
        ("pids.max", crate::film::limits(8 << 30).tasks.to_string()),
    ] {
        fs::write(root.join(key), value).unwrap();
    }
    let unchanged = limits_match(&root, &backend.config.limits(), 8 << 30);
    assert_eq!(unchanged, Ok(true));
    checked_setup_observation(&mut record, &mut save, unchanged).unwrap();
    assert_eq!(
        record
            .film
            .as_ref()
            .unwrap()
            .qualification_observation_valid,
        Some(true)
    );
    fs::write(root.join("memory.max"), (7u64 << 30).to_string()).unwrap();
    let drift = limits_match(&root, &backend.config.limits(), 8 << 30);
    assert_eq!(drift, Ok(false));
    assert_eq!(
        checked_setup_observation(&mut record, &mut save, drift),
        Err(ErrorCode::Unavailable)
    );
    assert_eq!(persisted.len(), 1);
    let recovered: Record = serde_json::from_slice(&persisted[0]).unwrap();
    assert_eq!(
        recovered.film.unwrap().qualification_observation_valid,
        Some(false)
    );
    let mut record = crate::journal::tests::qualified_record(1);
    assert_eq!(
        checked_setup_observation(&mut record, &mut |_| Err(ErrorCode::Uncertain), Ok(false)),
        Err(ErrorCode::Uncertain)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_result_enforces_sealed_shape_and_launch_binding() {
    let root =
        std::env::temp_dir().join(format!("slipstream-worker-result-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let path = root.join("result");
    let launch = "a".repeat(32);
    let frame = |body: &[u8]| {
        let mut bytes = body.to_vec();
        bytes.resize(RESULT_BYTES, 0);
        fs::write(&path, bytes).unwrap();
    };
    // An absent result file is one unfinished attempt.
    assert_eq!(worker_result(&path, &launch), Ok(None));
    frame(format!(r#"{{"launch_id":"{launch}","outcome":"completed"}}"#).as_bytes());
    assert_eq!(worker_result(&path, &launch), Ok(Some(Outcome::Completed)));
    // An unparseable or over-carried body is still only an absent
    // outcome; the terminal exit path, not the file, decides it.
    frame(b"{");
    assert_eq!(worker_result(&path, &launch), Ok(None));
    frame(format!(r#"{{"launch_id":"{launch}","outcome":"completed","extra":1}}"#).as_bytes());
    assert_eq!(worker_result(&path, &launch), Ok(None));
    // A foreign launch id and every broken seal are tamper evidence.
    let foreign = format!(
        r#"{{"launch_id":"{}","outcome":"completed"}}"#,
        "b".repeat(32)
    );
    frame(foreign.as_bytes());
    assert_eq!(worker_result(&path, &launch), Err(ErrorCode::Uncertain));
    let mut trailing = Vec::new();
    trailing.extend_from_slice(
        format!(r#"{{"launch_id":"{launch}","outcome":"completed"}}"#).as_bytes(),
    );
    trailing.push(0);
    trailing.push(1);
    trailing.resize(RESULT_BYTES, 0);
    fs::write(&path, trailing).unwrap();
    assert_eq!(worker_result(&path, &launch), Err(ErrorCode::Uncertain));
    for size in [RESULT_BYTES - 1, RESULT_BYTES + 1] {
        fs::write(&path, vec![0u8; size]).unwrap();
        assert_eq!(worker_result(&path, &launch), Err(ErrorCode::Uncertain));
    }
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("absent-target", &path).unwrap();
    assert_eq!(worker_result(&path, &launch), Err(ErrorCode::Uncertain));
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(worker_result(&path, &launch), Err(ErrorCode::Uncertain));
    fs::remove_dir(&path).unwrap();
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn unresolved_manager_effects_quarantine_before_any_runtime_lookup() {
    let config = Config {
        version: 1,
        mode: "qualification".into(),
        instance: "0".repeat(32),
        root: "/does-not-exist".into(),
        socket: "/does-not-exist.sock".into(),
        peer_uid: 0,
        image: format!("sha256:{}", "1".repeat(64)),
        memory_bytes: 64 * 1024 * 1024,
        receipt_retention_seconds: 1,
    };
    let backend = Backend { config };
    for phase in [
        ManagerPhase::Slice,
        ManagerPhase::SliceStop,
        ManagerPhase::Mount,
        ManagerPhase::Create,
        ManagerPhase::Start,
        ManagerPhase::Pause,
        ManagerPhase::Unpause,
    ] {
        let mut record:Record=serde_json::from_value(serde_json::json!({
            "receipt":{"incarnation":"11111111111111111111111111111111","sequence":1,"workload":"probe-success","policy":"2".repeat(64),"bundle":"3".repeat(64),"state":"accepted","cancellation_requested":false,"accepted_at_unix_ms":0,"deadline_unix_ms":30000,"outcome":null,"runtime":null,"limits":Limits::new(64*1024*1024),"evidence":null,"cleanup":"pending"},
            "launch_id":"4".repeat(32),"image_id":format!("sha256:{}","5".repeat(64)),"unit_invocation":null,"cgroup_inode":null,"mount_id":null,"released":false,"termination_reason":null,"manager_pending":phase,"settled_at_unix_ms":null
        })).unwrap();
        assert_eq!(backend.discover(&mut record), Err(ErrorCode::Uncertain));
        assert_eq!(record.manager_pending, Some(phase));
        assert!(record.receipt.runtime.is_none());
        record.receipt.outcome = Some(Outcome::Completed);
        record.stop_confirmed = true;
        assert_eq!(
            backend.cleanup(&mut record, |_| panic!(
                "pending cleanup must not persist or execute"
            )),
            Err(ErrorCode::Uncertain)
        );
    }
}

#[test]
fn expired_command_deadline_does_not_spawn_even_a_valid_program() {
    assert_eq!(
        command_until(
            "/usr/bin/true",
            &[],
            Instant::now() - Duration::from_millis(1)
        ),
        Err(ErrorCode::Uncertain)
    );
}

#[test]
fn confirmed_stop_routes_only_to_read_only_convergence_and_pending_always_blocks() {
    let active = std::cell::Cell::new(0);
    let stopped = std::cell::Cell::new(0);
    for stop_confirmed in [false, true] {
        assert_eq!(
            verify_slice_phase(
                None,
                stop_confirmed,
                || {
                    active.set(active.get() + 1);
                    Ok(())
                },
                || {
                    stopped.set(stopped.get() + 1);
                    Ok(())
                }
            ),
            Ok(())
        );
        for pending in [ManagerPhase::SliceStop]
            .into_iter()
            .chain(stop_confirmed.then_some(ManagerPhase::Start))
        {
            assert_eq!(
                verify_slice_phase(
                    Some(pending),
                    stop_confirmed,
                    || panic!("pending cannot validate an active replacement"),
                    || panic!("pending cannot infer stop from absence")
                ),
                Err(ErrorCode::Uncertain)
            );
        }
    }
    assert_eq!((active.get(), stopped.get()), (1, 1));
    assert_eq!(
        verify_slice_phase(
            Some(ManagerPhase::CreateReturned),
            false,
            || Ok(()),
            || panic!("lost create response still uses active ownership")
        ),
        Ok(())
    );
    assert_eq!(
        verify_slice_phase(
            None,
            true,
            || panic!("completed stop cannot require live properties"),
            || Err(ErrorCode::Uncertain)
        ),
        Err(ErrorCode::Uncertain)
    );
}
