use super::*;

use crate::protocol::{PHOTO_MODE, PHOTO_PROTOCOL_VERSION};
use std::{
    collections::BTreeMap,
    os::unix::fs::OpenOptionsExt,
    time::{SystemTime, UNIX_EPOCH},
};

/// The plan and published artifact are pure functions of the closed
/// workload; an unknown workload admits neither.
#[test]
fn plan_and_output_name_are_derived_from_the_closed_workload() {
    let development = Plan::for_workload(crate::protocol::PHOTO_WORKLOAD).unwrap();
    assert_eq!(development.steps, vec!["develop".to_string()]);
    assert_eq!(development.output, crate::protocol::PHOTO_WORKLOAD);
    assert_eq!(
        output_name(crate::protocol::PHOTO_WORKLOAD),
        Some("output/development.tif")
    );
    let film = Plan::for_workload(crate::protocol::PHOTO_WORKLOAD_FILM).unwrap();
    assert_eq!(film.steps, vec!["develop".to_string(), "film".to_string()]);
    assert_eq!(film.output, crate::protocol::PHOTO_WORKLOAD_FILM);
    assert_eq!(
        output_name(crate::protocol::PHOTO_WORKLOAD_FILM),
        Some("output/finished.jpg")
    );
    // `proxy-film` publishes the same finished JPEG as `film-jpeg`, but
    // its plan has no develop step: the staged proxy already carries it.
    assert_eq!(
        output_name(crate::protocol::PHOTO_WORKLOAD_PROXY_FILM),
        Some("output/finished.jpg")
    );
    let proxy = Plan::for_workload(crate::protocol::PHOTO_WORKLOAD_PROXY_FILM).unwrap();
    assert_eq!(proxy.steps, vec!["film".to_string()]);
    assert_eq!(proxy.output, crate::protocol::PHOTO_WORKLOAD_PROXY_FILM);
    for unknown in [
        "",
        "film",
        "film-tiff",
        "development-jpeg",
        "proxy",
        "proxy-film-tiff",
        "probe-success",
    ] {
        assert!(Plan::for_workload(unknown).is_none(), "{unknown:?}");
        assert_eq!(output_name(unknown), None, "{unknown:?}");
    }
}

/// The canonical manifest digest binds the declared workload: two Start
/// requests that differ only in target never share an attempt identity,
/// and the replay comparison cannot alias one workload onto the other.
#[test]
fn manifest_digest_binds_the_declared_workload() {
    let source = Source {
        kind: "raw".into(),
        profile_id: "sony-ilce-7rm5-arw".into(),
        size: 100,
        sha256: "4".repeat(64),
    };
    let recipe = Recipe {
        exposure_milli_ev: 250,
        white_balance_mode: "as-shot".into(),
    };
    let development =
        manifest_digest_parts(&source, &recipe, "p", "b", crate::protocol::PHOTO_WORKLOAD).unwrap();
    let film = manifest_digest_parts(
        &source,
        &recipe,
        "p",
        "b",
        crate::protocol::PHOTO_WORKLOAD_FILM,
    )
    .unwrap();
    assert_ne!(development, film);
    assert_eq!(
        film,
        manifest_digest_parts(
            &source,
            &recipe,
            "p",
            "b",
            crate::protocol::PHOTO_WORKLOAD_FILM
        )
        .unwrap()
    );
}

/// The receipt a service observes identifies the attempt's admitted
/// workload, not a global constant.
#[test]
fn wire_receipt_carries_the_record_workload() {
    let mut record = record_for(1, Phase::Planned);
    assert_eq!(
        record.wire_receipt(&record.incarnation).workload,
        "development-tiff"
    );
    record.workload = crate::protocol::PHOTO_WORKLOAD_FILM.into();
    assert_eq!(
        record.wire_receipt(&record.incarnation).workload,
        "film-jpeg"
    );
}

fn temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "slipstream-photo-exec-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&path)
        .unwrap();
    path
}

fn test_config(root: &Path) -> Config {
    Config {
        version: PHOTO_PROTOCOL_VERSION,
        mode: PHOTO_MODE.into(),
        instance: "0".repeat(32),
        root: root.display().to_string(),
        socket: root.join("launcher.sock").display().to_string(),
        // The production peer is Web UID 1000; the focused tests
        // authenticate their actual peer so they run under any CI UID.
        peer_uid: unsafe { libc::geteuid() },
        image: format!("sha256:{}", "1".repeat(64)),
        bundle: "2".repeat(64),
        policy: "3".repeat(64),
        source_bytes_max: 4096,
        staged_storage_bytes_max: 8192,
        staged_storage_inodes_max: 64,
        output_bytes_max: 8192,
        memory_bytes: 8 * 1024 * 1024 * 1024,
        cpu_quota_us: 400_000,
        tasks: 256,
        swap_bytes: 0,
        control_reserve_bytes: 1024 * 1024,
        shared_ancestor_headroom_bytes: 1024 * 1024,
        receipt_retention_seconds: 86_400,
    }
}

/// Headroom comfortably above every configured test reserve.
fn satisfied_headroom() -> Headroom {
    Headroom {
        control_free_bytes: 1 << 40,
        ancestor_headroom_bytes: 1 << 40,
    }
}

fn empty_registry(instance: &str) -> Registry {
    Registry {
        version: 1,
        instance: instance.into(),
        incarnation: "a".repeat(32),
        watermark: 0,
        parent_pending: false,
        parent_identity: None,
        active: None,
        records: BTreeMap::new(),
    }
}

fn source_file(root: &Path, bytes: &[u8]) -> File {
    let path = root.join(format!(
        "staged-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC)
        .open(&path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    // Seal the staged copy exactly like the service does.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(&path)
        .unwrap()
}

fn start_request(config: &Config, overrides: impl FnOnce(&mut photo::Request)) -> photo::Request {
    let mut request = photo::Request::Start {
        mode: PHOTO_MODE.into(),
        version: PHOTO_PROTOCOL_VERSION,
        instance: config.instance.clone(),
        export_id: "export-1".into(),
        incarnation: "a".repeat(32),
        sequence: 1,
        policy: config.policy.clone(),
        bundle: config.bundle.clone(),
        workload: crate::protocol::PHOTO_WORKLOAD.into(),
        source: Source {
            kind: "raw".into(),
            profile_id: "sony-ilce-7rm5-arw".into(),
            size: 4,
            sha256: "4".repeat(64),
        },
        recipe: Recipe {
            exposure_milli_ev: 0,
            white_balance_mode: "as-shot".into(),
        },
        recipe_digest: String::new(),
        manifest_sha256: String::new(),
    };
    // Fill the canonical digests so a default request admits.
    {
        let photo::Request::Start {
            source: ref source_field,
            recipe: ref recipe_field,
            policy: ref policy_field,
            bundle: ref bundle_field,
            workload: ref workload_field,
            recipe_digest: ref mut recipe_digest_field,
            ref mut manifest_sha256,
            ..
        } = request
        else {
            unreachable!()
        };
        *recipe_digest_field = recipe_digest(recipe_field).unwrap();
        *manifest_sha256 = manifest_digest_parts(
            source_field,
            recipe_field,
            policy_field,
            bundle_field,
            workload_field,
        )
        .unwrap();
    }
    overrides(&mut request);
    // Recompute nothing: overridden digests intentionally mismatch.
    request
}

fn with_canonical_digests(mut request: photo::Request) -> photo::Request {
    if let photo::Request::Start {
        ref source,
        ref recipe,
        ref policy,
        ref bundle,
        ref workload,
        recipe_digest: ref mut recipe_digest_field,
        ref mut manifest_sha256,
        ..
    } = request
    {
        *recipe_digest_field = recipe_digest(recipe).unwrap();
        *manifest_sha256 = manifest_digest_parts(source, recipe, policy, bundle, workload).unwrap();
    }
    request
}

fn record_for(sequence: u64, phase: Phase) -> PhotoRecord {
    let source = Source {
        kind: "raw".into(),
        profile_id: "sony-ilce-7rm5-arw".into(),
        size: 100,
        sha256: "4".repeat(64),
    };
    let recipe = Recipe {
        exposure_milli_ev: 1000,
        white_balance_mode: "as-shot".into(),
    };
    PhotoRecord {
        sequence,
        incarnation: "a".repeat(32),
        export_id: "export-1".into(),
        policy: "3".repeat(64),
        bundle: "2".repeat(64),
        workload: crate::protocol::PHOTO_WORKLOAD.into(),
        state: if phase == Phase::OutputReady {
            State::Settling
        } else {
            State::Accepted
        },
        outcome: None,
        manifest_sha256: manifest_digest_parts(
            &source,
            &recipe,
            "3".repeat(64).as_str(),
            "2".repeat(64).as_str(),
            crate::protocol::PHOTO_WORKLOAD,
        )
        .unwrap(),
        recipe_digest: recipe_digest(&recipe).unwrap(),
        source,
        recipe,
        plan: (phase != Phase::Intent)
            .then(|| Plan::for_workload(crate::protocol::PHOTO_WORKLOAD).unwrap()),
        phase,
        launch_id: "b".repeat(32),
        image_id: Some(format!("sha256:{}", "1".repeat(64))),
        unit_invocation: None,
        cgroup_inode: None,
        mount_id: None,
        container_id: None,
        released: false,
        cancellation_requested: false,
        accepted_at_unix_ms: 1,
        deadline_unix_ms: u64::MAX,
        manager_pending: None,
        stop_confirmed: false,
        evidence: None,
        output: None,
        output_transferred: false,
        validation_ack: None,
        cleanup: Cleanup::Pending,
        settled_at_unix_ms: None,
    }
}

fn executor_with(root: &Path, mut registry: Registry) -> Arc<PhotoExecutor> {
    let config = test_config(root);
    if registry.instance != config.instance {
        registry.instance = config.instance.clone();
    }
    for name in ["attempts", "docker-client"] {
        fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(root.join(name))
            .unwrap();
    }
    persist(root, &registry).unwrap();
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC)
        .open(root.join("owner.lock"))
        .unwrap();
    Arc::new(PhotoExecutor {
        config,
        data: Mutex::new(Data {
            registry,
            available: true,
        }),
        image_id: format!("sha256:{}", "1".repeat(64)),
        _lock: lock.try_clone().expect("owner lock handle"),
        _instance_claim: lock,
    })
}

#[test]
fn descriptor_size_hash_and_identity_mismatch_settle_the_intent_without_a_worker() {
    let root = temp_dir("mismatch");
    let config = test_config(&root);
    let mut registry = empty_registry(&config.instance);

    // The declared digest never matches the sealed copy.
    let request = with_canonical_digests(start_request(&config, |_| {}));
    let descriptor = source_file(&root, b"data");
    let admission = begin_start(
        &mut registry,
        &config,
        &root,
        &request,
        Some(&descriptor),
        &format!("sha256:{}", "1".repeat(64)),
        true,
        &satisfied_headroom(),
    )
    .unwrap();
    let StartAdmission::Intent(record) = admission else {
        panic!("expected a fresh intent");
    };
    let record = *record;
    assert_eq!(record.phase, Phase::Intent);
    assert_eq!(registry.active, Some(1));
    assert_eq!(registry.watermark, 1);

    let mut descriptor = descriptor;
    let sealed = seal_source(&config, &root, &record, &mut descriptor);
    let copied = sealed.unwrap();
    assert_ne!(copied.sha256, record.source.sha256);
    let error = finalize_start(&mut registry, &root, &request, Ok(copied)).unwrap_err();
    assert_eq!(error, ErrorCode::InvalidRequest);
    let settled = &registry.records[&1];
    assert_eq!(settled.state, State::Settled);
    assert_eq!(settled.outcome.as_deref(), Some("refused-source-mismatch"));
    assert_eq!(settled.plan, None);
    assert_eq!(registry.active, None);
    assert!(!settled.workspace(&root).exists());

    // A descriptor whose regular-file facts do not match the declared
    // size is refused by the bounded metadata check before any intent.
    let mut registry = empty_registry(&config.instance);
    let request = with_canonical_digests(start_request(&config, |request| {
        if let photo::Request::Start { source, .. } = request {
            source.size = 8;
            source.sha256 = format!("{:x}", Sha256::digest(b"data"));
        }
    }));
    let descriptor = source_file(&root, b"data");
    assert_eq!(
        begin_start(
            &mut registry,
            &config,
            &root,
            &request,
            Some(&descriptor),
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap_err(),
        ErrorCode::InvalidRequest
    );
    assert!(registry.records.is_empty());
    assert_eq!(registry.active, None);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn verified_source_seals_and_derives_the_one_admitted_plan() {
    let root = temp_dir("planned");
    let config = test_config(&root);
    let mut registry = empty_registry(&config.instance);
    let payload = b"raw-bytes";
    let request = with_canonical_digests(start_request(&config, |request| {
        if let photo::Request::Start { source, .. } = request {
            source.size = payload.len() as u64;
            source.sha256 = format!("{:x}", Sha256::digest(payload));
        }
    }));
    let descriptor = source_file(&root, payload);
    let StartAdmission::Intent(record) = begin_start(
        &mut registry,
        &config,
        &root,
        &request,
        Some(&descriptor),
        &format!("sha256:{}", "1".repeat(64)),
        true,
        &satisfied_headroom(),
    )
    .unwrap() else {
        panic!("expected a fresh intent");
    };
    let mut descriptor = descriptor;
    let copied = seal_source(&config, &root, &record, &mut descriptor).unwrap();
    assert_eq!(copied.sha256, record.source.sha256);
    // The sealed snapshot carries the qualified container extension so
    // the pinned engine selects the qualified decoder, and it is read-only
    // inside a directory the unprivileged worker can traverse.
    let sealed_path = record.workspace(&root).join("source").join("source.ARW");
    let metadata = fs::metadata(&sealed_path).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0o444);
    assert_eq!(fs::read(&sealed_path).unwrap(), payload);
    assert_eq!(
        fs::metadata(record.workspace(&root).join("source"))
            .unwrap()
            .mode()
            & 0o777,
        0o755
    );
    let body = finalize_start(&mut registry, &root, &request, Ok(copied)).unwrap();
    let ResultBody::Receipt { receipt } = body else {
        panic!("expected a receipt");
    };
    assert_eq!(receipt.state, "accepted");
    assert_eq!(receipt.outcome, None);
    assert_eq!(receipt.sequence, 1);
    let planned = &registry.records[&1];
    assert_eq!(planned.phase, Phase::Planned);
    assert_eq!(
        planned.plan.as_ref().map(|plan| plan.steps.as_slice()),
        Some(&["develop".to_string()][..])
    );
    // Provisioning creates the same attempt workspace the seal already
    // made. The two steps must compose: a second creation that failed
    // would settle every attempt as interrupted before the engine
    // container is ever created.
    backend::create_private_directory(&record.workspace(&root)).unwrap();
    assert_eq!(
        fs::metadata(record.workspace(&root)).unwrap().mode() & 0o777,
        0o700
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn unsupported_profiles_sizes_and_digests_are_refused_before_the_descriptor() {
    let root = temp_dir("refusals");
    let config = test_config(&root);
    let mut registry = empty_registry(&config.instance);
    let refuse = |registry: &mut Registry, request: photo::Request| {
        begin_start(
            registry,
            &config,
            &root,
            &request,
            None,
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap_err()
    };
    let canonical = |mut request: photo::Request| {
        if let photo::Request::Start {
            ref source,
            ref recipe,
            ref policy,
            ref bundle,
            ref workload,
            recipe_digest: ref mut recipe_digest_field,
            ref mut manifest_sha256,
            ..
        } = request
        {
            *recipe_digest_field = recipe_digest(recipe).unwrap();
            *manifest_sha256 =
                manifest_digest_parts(source, recipe, policy, bundle, workload).unwrap();
        }
        request
    };

    // An unqualified source class has no admitted plan.
    let foreign = canonical(start_request(&config, |request| {
        if let photo::Request::Start { source, .. } = request {
            source.profile_id = "canon-eos-r5".into();
        }
    }));
    assert_eq!(refuse(&mut registry, foreign), ErrorCode::InvalidRequest);

    // A declared size beyond the configured ceiling is refused even
    // though the closed envelope bound would allow it.
    let oversized = canonical(start_request(&config, |request| {
        if let photo::Request::Start { source, .. } = request {
            source.size = 8192;
        }
    }));
    assert_eq!(refuse(&mut registry, oversized), ErrorCode::InvalidRequest);

    // Exposure outside the qualified bundle range is refused.
    let over_range = canonical(start_request(&config, |request| {
        if let photo::Request::Start { recipe, .. } = request {
            recipe.exposure_milli_ev = 1001;
        }
    }));
    assert_eq!(refuse(&mut registry, over_range), ErrorCode::InvalidRequest);

    // A forged recipe or manifest digest fails closed.
    let forged = start_request(&config, |request| {
        if let photo::Request::Start { recipe_digest, .. } = request {
            *recipe_digest = "e".repeat(64);
        }
    });
    assert_eq!(refuse(&mut registry, forged), ErrorCode::InvalidRequest);
    let forged_manifest = start_request(&config, |request| {
        if let photo::Request::Start {
            manifest_sha256, ..
        } = request
        {
            *manifest_sha256 = "f".repeat(64);
        }
    });
    assert_eq!(
        refuse(&mut registry, forged_manifest),
        ErrorCode::InvalidRequest
    );

    // A different policy or bundle identity is incompatible.
    let foreign_policy = canonical(start_request(&config, |request| {
        if let photo::Request::Start { policy, .. } = request {
            *policy = "9".repeat(64);
        }
    }));
    assert_eq!(
        refuse(&mut registry, foreign_policy),
        ErrorCode::IncompatiblePolicy
    );
    let foreign_bundle = canonical(start_request(&config, |request| {
        if let photo::Request::Start { bundle, .. } = request {
            *bundle = "8".repeat(64);
        }
    }));
    assert_eq!(
        refuse(&mut registry, foreign_bundle),
        ErrorCode::IncompatibleBundle
    );

    // Sequence discipline and a missing descriptor.
    assert_eq!(
        refuse(&mut registry, canonical(start_request(&config, |_| {}))),
        ErrorCode::InvalidRequest
    );
    let skipped = canonical(start_request(&config, |request| {
        if let photo::Request::Start { sequence, .. } = request {
            *sequence = 2;
        }
    }));
    assert_eq!(refuse(&mut registry, skipped), ErrorCode::UnknownAttempt);

    assert!(registry.records.is_empty());
    assert_eq!(registry.active, None);
    fs::remove_dir_all(&root).unwrap();
}

/// `proxy-film` admits exactly one Start shape — a `development-proxy`
/// source with zero exposure — and derives its no-develop plan from it.
/// Every other workload/source-kind pairing, and any nonzero exposure
/// under the proxy workload, is refused before a descriptor is read.
#[test]
fn proxy_film_admission_pairs_source_kind_and_zero_exposure() {
    let root = temp_dir("proxy-admission");
    let config = test_config(&root);
    let mut registry = empty_registry(&config.instance);
    let proxy = with_canonical_digests(start_request(&config, |request| {
        if let photo::Request::Start {
            workload, source, ..
        } = request
        {
            *workload = crate::protocol::PHOTO_WORKLOAD_PROXY_FILM.into();
            source.kind = "development-proxy".into();
            source.sha256 = format!("{:x}", Sha256::digest(b"data"));
        }
    }));
    let descriptor = source_file(&root, b"data");
    let StartAdmission::Intent(record) = begin_start(
        &mut registry,
        &config,
        &root,
        &proxy,
        Some(&descriptor),
        &format!("sha256:{}", "1".repeat(64)),
        true,
        &satisfied_headroom(),
    )
    .unwrap() else {
        panic!("expected a fresh intent");
    };
    assert_eq!(record.workload, crate::protocol::PHOTO_WORKLOAD_PROXY_FILM);
    assert_eq!(record.source.kind, "development-proxy");
    assert_eq!(record.recipe.exposure_milli_ev, 0);

    // The sealed proxy derives the one admitted no-develop plan.
    let mut descriptor = descriptor;
    let copied = seal_source(&config, &root, &record, &mut descriptor).unwrap();
    finalize_start(&mut registry, &root, &proxy, Ok(copied)).unwrap();
    let planned = &registry.records[&1];
    assert_eq!(planned.phase, Phase::Planned);
    assert_eq!(
        planned.plan.as_ref().map(|plan| plan.steps.as_slice()),
        Some(&["film".to_string()][..])
    );

    // Every mismatched pairing of the closed workloads and source kinds
    // is refused before the descriptor, as is a proxy-film request whose
    // exposure would double-apply the staged transform.
    let mut refusals = empty_registry(&config.instance);
    let refuse = |registry: &mut Registry, request: photo::Request| {
        begin_start(
            registry,
            &config,
            &root,
            &request,
            None,
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap_err()
    };
    for (workload, kind) in [
        (crate::protocol::PHOTO_WORKLOAD, "development-proxy"),
        (crate::protocol::PHOTO_WORKLOAD_FILM, "development-proxy"),
        (crate::protocol::PHOTO_WORKLOAD_PROXY_FILM, "raw"),
        (crate::protocol::PHOTO_WORKLOAD_PROXY_FILM, "proxy"),
        (crate::protocol::PHOTO_WORKLOAD_PROXY_FILM, ""),
    ] {
        let mismatched = with_canonical_digests(start_request(&config, |request| {
            if let photo::Request::Start {
                workload: workload_field,
                source,
                ..
            } = request
            {
                *workload_field = workload.into();
                source.kind = kind.into();
            }
        }));
        assert_eq!(
            refuse(&mut refusals, mismatched),
            ErrorCode::InvalidRequest,
            "workload {workload:?} with source kind {kind:?} must be refused"
        );
    }
    let nonzero_exposure = with_canonical_digests(start_request(&config, |request| {
        if let photo::Request::Start {
            workload,
            source,
            recipe,
            ..
        } = request
        {
            *workload = crate::protocol::PHOTO_WORKLOAD_PROXY_FILM.into();
            source.kind = "development-proxy".into();
            recipe.exposure_milli_ev = 1;
        }
    }));
    assert_eq!(
        refuse(&mut refusals, nonzero_exposure),
        ErrorCode::InvalidRequest
    );

    // The refusals never persisted an attempt.
    assert!(refusals.records.is_empty());
    assert_eq!(refusals.active, None);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn reserved_storage_and_inode_bounds_gate_admission() {
    let root = temp_dir("budget");
    let config = test_config(&root);
    let mut registry = empty_registry(&config.instance);
    // One long-running attempt already reserves the whole staged budget.
    let mut hog = record_for(1, Phase::Planned);
    hog.source.size = config.staged_storage_bytes_max;
    registry.records.insert(1, hog);
    registry.active = Some(1);
    registry.watermark = 1;
    let request = with_canonical_digests(start_request(&config, |request| {
        if let photo::Request::Start { sequence, .. } = request {
            *sequence = 2;
        }
    }));
    // The byte ceiling gates admission before anything is staged.
    let descriptor = source_file(&root, b"data");
    assert_eq!(
        begin_start(
            &mut registry,
            &config,
            &root,
            &request,
            Some(&descriptor),
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap_err(),
        ErrorCode::ResourceBudget
    );
    // Below the byte ceiling, the fixed per-attempt inode shape still
    // bounds admission.
    assert_eq!(
        begin_start(
            &mut registry,
            &config,
            &root,
            &request,
            Some(&descriptor),
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap_err(),
        ErrorCode::ResourceBudget
    );
    registry.records.get_mut(&1).unwrap().source.size = 0;
    let mut tight = config.clone();
    tight.staged_storage_inodes_max = INODES_PER_ATTEMPT - 1;
    assert_eq!(
        begin_start(
            &mut registry,
            &tight,
            &root,
            &request,
            Some(&descriptor),
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap_err(),
        ErrorCode::ResourceBudget
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn output_and_validation_are_refused_before_a_validated_transfer() {
    let root = temp_dir("early");
    let mut registry = empty_registry("0".repeat(32).as_str());
    registry.records.insert(1, record_for(1, Phase::Planned));
    registry.active = Some(1);
    registry.watermark = 1;
    registry.records.insert(2, {
        let mut record = record_for(2, Phase::OutputReady);
        record.output = Some(OutputIdentity {
            size: 10,
            sha256: format!("{:x}", Sha256::digest(b"tiff-bytes")),
            width: 4,
            height: 3,
        });
        record
    });
    registry.active = Some(2);
    registry.watermark = 2;
    let executor = executor_with(&root, registry);
    let incarnation = "a".repeat(32);

    // Unknown attempt identities are refused.
    assert_eq!(
        executor.inspect(&incarnation, "export-1", 9).unwrap_err(),
        ErrorCode::UnknownAttempt
    );
    assert_eq!(
        executor.inspect(&incarnation, "export-1", 0).unwrap_err(),
        ErrorCode::Expired
    );
    assert_eq!(
        executor
            .output(&incarnation, "export-1", 9, None)
            .unwrap_err(),
        ErrorCode::UnknownAttempt
    );
    // A planned attempt has no validated output to transfer.
    assert_eq!(
        executor
            .output(&incarnation, "export-1", 1, None)
            .unwrap_err(),
        ErrorCode::InvalidRequest
    );
    // Validation before any transfer is refused.
    assert_eq!(
        executor
            .validate_output(&incarnation, "export-1", 2, 10, &"4".repeat(64), true)
            .unwrap_err(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        executor
            .validate_output(&"b".repeat(32), "export-1", 2, 10, &"4".repeat(64), true)
            .unwrap_err(),
        ErrorCode::StaleIncarnation
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn output_transfer_returns_the_bounded_receipt_exactly_once() {
    let root = temp_dir("transfer");
    let mut registry = empty_registry("0".repeat(32).as_str());
    let mut record = record_for(1, Phase::OutputReady);
    let payload = b"tiff-bytes";
    record.output = Some(OutputIdentity {
        size: payload.len() as u64,
        sha256: format!("{:x}", Sha256::digest(payload)),
        width: 4,
        height: 3,
    });
    registry.records.insert(1, record);
    registry.active = Some(1);
    registry.watermark = 1;
    let executor = executor_with(&root, registry);
    let incarnation = "a".repeat(32);

    // The launcher-owned result file at its attempt path.
    let result_path = root
        .join("attempts")
        .join("b".repeat(32))
        .join("work")
        .join("output");
    fs::create_dir_all(&result_path).unwrap();
    fs::write(result_path.join("development.tif"), payload).unwrap();

    // The service output descriptor: writable, empty, zero offset.
    let output_path = root.join("service-output");
    let descriptor = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC)
        .open(&output_path)
        .unwrap();

    let body = executor
        .output(&incarnation, "export-1", 1, Some(descriptor))
        .unwrap();
    let ResultBody::Output { receipt } = body else {
        panic!("expected an output receipt");
    };
    assert_eq!(receipt.export_id, "export-1");
    assert_eq!(receipt.incarnation, incarnation);
    assert_eq!(receipt.sequence, 1);
    assert_eq!(receipt.target, crate::protocol::PHOTO_WORKLOAD);
    assert_eq!(receipt.size, payload.len() as u64);
    assert_eq!(receipt.sha256, format!("{:x}", Sha256::digest(payload)));
    assert_eq!(fs::read(&output_path).unwrap(), payload);
    // The transfer is durably recorded for the validation acknowledgement.
    assert!(executor.record(1).unwrap().output_transferred);
    // A repeated transfer of the same attempt is refused with the closed
    // conflict outcome: the durable claim admits exactly one service
    // artifact, and the refused descriptor never gains bytes.
    let descriptor = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC)
        .open(root.join("service-output-2"))
        .unwrap();
    assert_eq!(
        executor
            .output(&incarnation, "export-1", 1, Some(descriptor))
            .unwrap_err(),
        ErrorCode::Conflict
    );
    assert_eq!(
        fs::read(root.join("service-output-2")).unwrap(),
        Vec::<u8>::new()
    );
    assert_eq!(fs::read(&output_path).unwrap(), payload);
    fs::remove_dir_all(&root).unwrap();
}

/// A `film-jpeg` attempt publishes the fixed Film artifact from the
/// film path under the film target: the transfer reads `finished.jpg`,
/// never the `development-tiff` handoff path, and the receipt names the
/// attempt's own workload.
#[test]
fn film_jpeg_output_transfers_the_film_artifact_path() {
    let root = temp_dir("film-output");
    let mut registry = empty_registry("0".repeat(32).as_str());
    let mut record = record_for(1, Phase::OutputReady);
    record.state = State::Settling;
    record.workload = crate::protocol::PHOTO_WORKLOAD_FILM.into();
    record.plan = Some(Plan::for_workload(&record.workload).unwrap());
    let payload = b"jpeg-bytes";
    record.output = Some(OutputIdentity {
        size: payload.len() as u64,
        sha256: format!("{:x}", Sha256::digest(payload)),
        width: 64,
        height: 48,
    });
    registry.records.insert(1, record);
    registry.active = Some(1);
    registry.watermark = 1;
    let executor = executor_with(&root, registry);
    let incarnation = "a".repeat(32);

    let result_path = root
        .join("attempts")
        .join("b".repeat(32))
        .join("work")
        .join("output");
    fs::create_dir_all(&result_path).unwrap();
    fs::write(result_path.join("finished.jpg"), payload).unwrap();

    let output_path = root.join("service-output");
    let descriptor = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC)
        .open(&output_path)
        .unwrap();
    let body = executor
        .output(&incarnation, "export-1", 1, Some(descriptor))
        .unwrap();
    let ResultBody::Output { receipt } = body else {
        panic!("expected an output receipt");
    };
    assert_eq!(receipt.target, crate::protocol::PHOTO_WORKLOAD_FILM);
    assert_eq!(receipt.size, payload.len() as u64);
    assert_eq!(receipt.sha256, format!("{:x}", Sha256::digest(payload)));
    assert_eq!(fs::read(&output_path).unwrap(), payload);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn output_side_operations_are_bound_to_the_durable_export_identity() {
    let root = temp_dir("export-bound");
    let mut registry = empty_registry("0".repeat(32).as_str());
    let mut record = record_for(1, Phase::OutputReady);
    record.state = State::Settling;
    let payload = b"tiff-bytes";
    record.output = Some(OutputIdentity {
        size: payload.len() as u64,
        sha256: format!("{:x}", Sha256::digest(payload)),
        width: 4,
        height: 3,
    });
    registry.records.insert(1, record);
    registry.active = Some(1);
    registry.watermark = 1;
    let executor = executor_with(&root, registry);
    let incarnation = "a".repeat(32);

    // The launcher-owned result file at its attempt path.
    let result_path = root
        .join("attempts")
        .join("b".repeat(32))
        .join("work")
        .join("output");
    fs::create_dir_all(&result_path).unwrap();
    fs::write(result_path.join("development.tif"), payload).unwrap();

    // Every output-side operation under a foreign export identity is
    // refused with the closed conflict outcome and changes nothing.
    let foreign = "export-2";
    assert_eq!(
        executor.inspect(&incarnation, foreign, 1).unwrap_err(),
        ErrorCode::Conflict
    );
    assert_eq!(
        executor
            .validate_output(&incarnation, foreign, 1, 10, &"4".repeat(64), true)
            .unwrap_err(),
        ErrorCode::Conflict
    );
    assert_eq!(
        executor.cancel(&incarnation, foreign, 1).unwrap_err(),
        ErrorCode::Conflict
    );
    let refused_descriptor = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC)
        .open(root.join("service-output-refused"))
        .unwrap();
    assert_eq!(
        executor
            .output(&incarnation, foreign, 1, Some(refused_descriptor))
            .unwrap_err(),
        ErrorCode::Conflict
    );
    let record = executor.record(1).unwrap();
    assert!(!record.output_transferred);
    assert_eq!(record.validation_ack, None);
    assert!(!record.cancellation_requested);
    assert_eq!(record.outcome, None);
    assert_eq!(record.state, State::Settling);
    assert_eq!(
        fs::read(root.join("service-output-refused")).unwrap(),
        Vec::<u8>::new()
    );

    // The matching export identity still drives the whole flow: inspect,
    // the one transfer, the validation acknowledgement, and cancellation.
    let ResultBody::Receipt { receipt } = executor.inspect(&incarnation, "export-1", 1).unwrap()
    else {
        panic!("expected a receipt");
    };
    assert_eq!(receipt.export_id, "export-1");
    let descriptor = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC)
        .open(root.join("service-output"))
        .unwrap();
    assert!(
        executor
            .output(&incarnation, "export-1", 1, Some(descriptor))
            .is_ok()
    );
    assert_eq!(fs::read(root.join("service-output")).unwrap(), payload);
    assert!(
        executor
            .validate_output(
                &incarnation,
                "export-1",
                1,
                payload.len() as u64,
                &format!("{:x}", Sha256::digest(payload)),
                true
            )
            .is_ok()
    );
    assert_eq!(executor.record(1).unwrap().validation_ack, Some(true));
    assert!(executor.cancel(&incarnation, "export-1", 1).is_ok());
    assert!(executor.record(1).unwrap().cancellation_requested);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn admission_measures_the_configured_reserve_and_ancestor_headroom() {
    let root = temp_dir("headroom");
    let config = test_config(&root);
    let mut registry = empty_registry(&config.instance);
    let request = with_canonical_digests(start_request(&config, |_| {}));
    let descriptor = source_file(&root, b"data");
    let begin = |registry: &mut Registry,
                 config: &Config,
                 headroom: &Headroom|
     -> Result<StartAdmission, ErrorCode> {
        begin_start(
            registry,
            config,
            &root,
            &request,
            Some(&descriptor),
            &format!("sha256:{}", "1".repeat(64)),
            true,
            headroom,
        )
    };

    // Control-path storage below the configured reserve refuses
    // admission and never persists a start intent.
    let starved = Headroom {
        control_free_bytes: config.control_reserve_bytes - 1,
        ancestor_headroom_bytes: u64::MAX,
    };
    assert_eq!(
        begin(&mut registry, &config, &starved).unwrap_err(),
        ErrorCode::Unavailable
    );
    assert!(registry.records.is_empty());
    assert_eq!(registry.active, None);
    // Shared-ancestor headroom below the configured allowance is the
    // same closed refusal.
    let starved = Headroom {
        control_free_bytes: u64::MAX,
        ancestor_headroom_bytes: config.shared_ancestor_headroom_bytes - 1,
    };
    assert_eq!(
        begin(&mut registry, &config, &starved).unwrap_err(),
        ErrorCode::Unavailable
    );
    assert!(registry.records.is_empty());
    assert_eq!(registry.active, None);

    // Measured values at or above both configured boundaries admit.
    let StartAdmission::Intent(record) =
        begin(&mut registry, &config, &satisfied_headroom()).unwrap()
    else {
        panic!("expected a fresh intent");
    };
    assert_eq!(record.phase, Phase::Intent);
    assert_eq!(registry.active, Some(1));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn headroom_boundaries_are_the_exact_configured_comparisons() {
    let config = test_config(Path::new("/tmp"));
    let met = Headroom {
        control_free_bytes: config.control_reserve_bytes,
        ancestor_headroom_bytes: config.shared_ancestor_headroom_bytes,
    };
    assert!(met.satisfied(&config).is_ok());
    let unmet = Headroom {
        control_free_bytes: config.control_reserve_bytes,
        ancestor_headroom_bytes: config.shared_ancestor_headroom_bytes - 1,
    };
    assert_eq!(
        unmet.satisfied(&config).unwrap_err(),
        ErrorCode::Unavailable
    );
}

#[test]
fn replay_binds_the_declared_source_profile_identity() {
    let root = temp_dir("replay-profile");
    let config = test_config(&root);
    let mut registry = empty_registry(&config.instance);
    let request = with_canonical_digests(start_request(&config, |_| {}));
    let descriptor = source_file(&root, b"data");
    let StartAdmission::Intent(_) = begin_start(
        &mut registry,
        &config,
        &root,
        &request,
        Some(&descriptor),
        &format!("sha256:{}", "1".repeat(64)),
        true,
        &satisfied_headroom(),
    )
    .unwrap() else {
        panic!("expected a fresh intent");
    };

    // The unchanged tuple, including its profile, replays.
    assert!(matches!(
        begin_start(
            &mut registry,
            &config,
            &root,
            &request,
            Some(&descriptor),
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap(),
        StartAdmission::Replay(_)
    ));

    // A different profile under the same attempt identity and the same
    // declared digest is a conflict, never a replay: the durable digest
    // binds the profile that was admitted.
    let mut forged = request.clone();
    if let photo::Request::Start { source, .. } = &mut forged {
        source.profile_id = "sony-ilce-7cm2-arw".into();
    }
    assert_eq!(
        begin_start(
            &mut registry,
            &config,
            &root,
            &forged,
            Some(&descriptor),
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap_err(),
        ErrorCode::Conflict
    );

    // An honestly recomputed digest over the changed profile conflicts
    // as well.
    let foreign = with_canonical_digests(forged);
    assert_eq!(
        begin_start(
            &mut registry,
            &config,
            &root,
            &foreign,
            Some(&descriptor),
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap_err(),
        ErrorCode::Conflict
    );
    assert_eq!(registry.records.len(), 1);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn validate_acknowledgement_is_accounted_exactly_once() {
    let root = temp_dir("ack");
    let mut registry = empty_registry("0".repeat(32).as_str());
    let mut record = record_for(1, Phase::OutputReady);
    record.state = State::Settling;
    record.output = Some(OutputIdentity {
        size: 10,
        sha256: "4".repeat(64),
        width: 4,
        height: 3,
    });
    record.output_transferred = true;
    registry.records.insert(1, record);
    registry.active = Some(1);
    registry.watermark = 1;
    let executor = executor_with(&root, registry);
    let incarnation = "a".repeat(32);

    let body = executor
        .validate_output(&incarnation, "export-1", 1, 10, &"4".repeat(64), true)
        .unwrap();
    let ResultBody::Receipt { receipt } = body else {
        panic!("expected a receipt");
    };
    assert_eq!(receipt.state, "settling");
    assert_eq!(executor.record(1).unwrap().validation_ack, Some(true));

    // Replaying the same acknowledgement resolves to the same receipt.
    assert!(
        executor
            .validate_output(&incarnation, "export-1", 1, 10, &"4".repeat(64), true)
            .is_ok()
    );
    // A different acknowledgement value is a conflict.
    assert_eq!(
        executor
            .validate_output(&incarnation, "export-1", 1, 10, &"4".repeat(64), false)
            .unwrap_err(),
        ErrorCode::Conflict
    );
    // A mismatched output echo never acknowledges.
    let mut conflicting = empty_registry("0".repeat(32).as_str());
    let mut other = record_for(1, Phase::OutputReady);
    other.state = State::Settling;
    other.output_transferred = true;
    other.output = Some(OutputIdentity {
        size: 12,
        sha256: "5".repeat(64),
        width: 4,
        height: 3,
    });
    conflicting.records.insert(1, other);
    conflicting.active = Some(1);
    conflicting.watermark = 1;
    let other_executor = executor_with(&root, conflicting);
    assert_eq!(
        other_executor
            .validate_output(&incarnation, "export-1", 1, 10, &"5".repeat(64), true)
            .unwrap_err(),
        ErrorCode::Conflict
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn cancellation_and_completion_settle_exactly_once() {
    let root = temp_dir("settle");
    let registry = {
        let mut registry = empty_registry("0".repeat(32).as_str());
        let mut record = record_for(1, Phase::Released);
        record.state = State::Running;
        registry.records.insert(1, record);
        registry.active = Some(1);
        registry.watermark = 1;
        registry
    };
    let executor = executor_with(&root, registry);
    let first = {
        let executor = executor.clone();
        thread::spawn(move || {
            let _ = executor.settle(1, Outcome::Cancelled);
        })
    };
    let second = {
        let executor = executor.clone();
        thread::spawn(move || {
            let _ = executor.settle(1, Outcome::Deadline);
        })
    };
    first.join().unwrap();
    second.join().unwrap();
    // Exactly one terminal outcome wins; the loser never overwrites it.
    let settled = executor.record(1).unwrap();
    assert!(matches!(
        settled.outcome.as_deref(),
        Some("cancelled") | Some("deadline")
    ));
    let body = executor.settle(1, Outcome::Completed).unwrap();
    let ResultBody::Receipt { receipt } = body else {
        panic!("expected a receipt");
    };
    assert_eq!(receipt.outcome, settled.outcome);
    // A late Cancel request for a terminal attempt never re-settles.
    let body = executor.cancel(&"a".repeat(32), "export-1", 1).unwrap();
    let ResultBody::Receipt { receipt } = body else {
        panic!("expected a receipt");
    };
    assert_eq!(receipt.outcome, settled.outcome);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn durable_snapshot_survives_restart_with_sequence_and_active_state() {
    let root = temp_dir("restart");
    let config = test_config(&root);
    let mut registry = empty_registry(&config.instance);
    let mut settled = record_for(1, Phase::OutputReady);
    settled.state = State::Settled;
    settled.outcome = Some("completed".into());
    settled.cleanup = Cleanup::Complete;
    settled.settled_at_unix_ms = Some(1);
    settled.output = Some(OutputIdentity {
        size: 10,
        sha256: "4".repeat(64),
        width: 4,
        height: 3,
    });
    let mut active = record_for(2, Phase::Released);
    active.state = State::Running;
    registry.records.insert(1, settled);
    registry.records.insert(2, active);
    registry.active = Some(2);
    registry.watermark = 2;
    registry.parent_identity = Some(ParentIdentity {
        invocation: "c".repeat(32),
        inode: 2,
    });
    persist(&root, &registry).unwrap();

    // The restarted launcher loads the same journal.
    let loaded = load(&root).unwrap().unwrap();
    validate_registry(&loaded, &config).unwrap();
    assert_eq!(loaded.incarnation, registry.incarnation);
    assert_eq!(loaded.watermark, 2);
    assert_eq!(loaded.active, Some(2));
    let capability = capability_body(&loaded, &config, true);
    let ResultBody::Capability {
        next_sequence,
        active: _,
        availability,
        ..
    } = capability
    else {
        panic!("expected a capability");
    };
    assert_eq!(next_sequence, 3);
    assert_eq!(availability, Availability::Available);
    let ResultBody::Capability {
        active: receipt, ..
    } = capability_body(&loaded, &config, true)
    else {
        panic!("expected a capability");
    };
    let receipt = receipt.expect("an active receipt");
    assert_eq!(receipt.sequence, 2);
    assert_eq!(receipt.state, "running");

    // A tampered snapshot is refused, never partially recovered.
    let mut tampered = load(&root).unwrap().unwrap();
    tampered.records.get_mut(&2).unwrap().outcome = Some("completed".into());
    assert_eq!(
        validate_registry(&tampered, &config).unwrap_err(),
        ErrorCode::Uncertain
    );
    let mut fabricated = load(&root).unwrap().unwrap();
    fabricated.records.get_mut(&1).unwrap().cleanup = Cleanup::Pending;
    assert_eq!(
        validate_registry(&fabricated, &config).unwrap_err(),
        ErrorCode::Uncertain
    );
    // An unsettled record never claims a cleanup it cannot have run.
    let mut premature = load(&root).unwrap().unwrap();
    premature.records.get_mut(&2).unwrap().cleanup = Cleanup::Complete;
    assert_eq!(
        validate_registry(&premature, &config).unwrap_err(),
        ErrorCode::Uncertain
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_interrupted_settlement_loads_so_reconcile_can_finish_it() {
    let root = temp_dir("settling-recovery");
    let config = test_config(&root);
    let mut registry = empty_registry(&config.instance);
    // A settlement records the terminal outcome and persists it before the
    // attempt boundary is removed. A launcher killed in that window leaves
    // exactly this state, and `reconcile` retries the pending cleanup, so
    // it must load instead of refusing every later start.
    let mut settling = record_for(1, Phase::Released);
    settling.state = State::Settling;
    settling.outcome = Some("interrupted".into());
    registry.records.insert(1, settling);
    registry.active = Some(1);
    registry.watermark = 1;
    registry.parent_identity = Some(ParentIdentity {
        invocation: "c".repeat(32),
        inode: 2,
    });
    persist(&root, &registry).unwrap();

    let loaded = load(&root).unwrap().unwrap();
    validate_registry(&loaded, &config).unwrap();
    let pending = loaded.records.get(&1).unwrap();
    assert_eq!(pending.cleanup, Cleanup::Pending);
    assert!(pending.outcome.is_some());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn discovery_resolves_a_pending_manager_phase_from_the_observed_attempt() {
    let root = temp_dir("discover-phase");
    // A worker that exits before its release-gate pause leaves that pause
    // recorded: `docker pause` cannot succeed on a worker that already
    // exited. The recorded slice, mount and container identities are what
    // the launcher owns, so reconciliation resolves the marker here instead
    // of blocking settlement and cleanup forever.
    for phase in [
        ManagerPhase::Slice,
        ManagerPhase::Mount,
        ManagerPhase::Start,
        ManagerPhase::Pause,
        ManagerPhase::Unpause,
    ] {
        let mut registry = empty_registry("0".repeat(32).as_str());
        let mut record = record_for(1, Phase::Released);
        record.state = State::Settling;
        record.outcome = Some("interrupted".into());
        record.manager_pending = Some(phase);
        registry.records.insert(1, record);
        registry.active = Some(1);
        registry.watermark = 1;
        registry.parent_identity = Some(ParentIdentity {
            invocation: "c".repeat(32),
            inode: 2,
        });
        let executor = executor_with(&root, registry);
        let mut record = executor.record(1).unwrap();
        executor.discover(&mut record).unwrap();
        assert_eq!(record.manager_pending, None, "{phase:?}");
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn discovery_keeps_a_pending_slice_stop_ambiguous() {
    let root = temp_dir("discover-stop");
    let mut registry = empty_registry("0".repeat(32).as_str());
    let mut record = record_for(1, Phase::Released);
    record.state = State::Settling;
    record.outcome = Some("interrupted".into());
    record.manager_pending = Some(ManagerPhase::SliceStop);
    registry.records.insert(1, record);
    registry.active = Some(1);
    registry.watermark = 1;
    registry.parent_identity = Some(ParentIdentity {
        invocation: "c".repeat(32),
        inode: 2,
    });
    let executor = executor_with(&root, registry);
    let mut record = executor.record(1).unwrap();
    // Whether the attempt boundary still exists decides what cleanup may
    // remove, and only a confirmed stop return clears this phase.
    assert_eq!(
        executor.discover(&mut record).unwrap_err(),
        ErrorCode::Uncertain
    );
    assert_eq!(record.manager_pending, Some(ManagerPhase::SliceStop));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn cleanup_refuses_a_record_whose_manager_phase_is_still_pending() {
    let root = temp_dir("cleanup-phase");
    let registry = empty_registry("0".repeat(32).as_str());
    let executor = executor_with(&root, registry);
    let mut record = record_for(1, Phase::Released);
    record.manager_pending = Some(ManagerPhase::Pause);
    // An unresolved manager effect is never torn down on an assumption.
    assert_eq!(
        executor.cleanup(&mut record).unwrap_err(),
        ErrorCode::Uncertain
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn manager_crash_after_pause_unpauses_before_killing_container() {
    let root = temp_dir("pause-recovery");
    let id = "c".repeat(64);
    let mut registry = empty_registry("0".repeat(32).as_str());
    let mut persisted = record_for(1, Phase::Released);
    persisted.container_id = Some(id.clone());
    persisted.manager_pending = Some(ManagerPhase::Pause);
    registry.records.insert(1, persisted);
    // The restart view contains the durable Pause marker before
    // reconciliation settles the observed attempt.
    let executor = executor_with(&root, registry);
    let mut record = executor.record(1).unwrap();
    assert_eq!(record.manager_pending, Some(ManagerPhase::Pause));
    executor.discover(&mut record).unwrap();
    assert_eq!(record.manager_pending, None);
    let mut paused = true;
    let mut running = true;
    let mut updates = Vec::new();
    let mut commands = Vec::new();
    let pending = std::cell::Cell::new(None);

    stop_container(
        &mut record,
        paused,
        |record| {
            pending.set(record.manager_pending);
            updates.push(record.manager_pending);
            Ok(())
        },
        |args| {
            commands.push(args.to_vec());
            match args.first().map(String::as_str) {
                Some("unpause") => {
                    assert_eq!(pending.get(), Some(ManagerPhase::Unpause));
                    assert!(paused);
                    paused = false;
                }
                Some("kill") => {
                    assert_eq!(pending.get(), None);
                    assert!(!paused);
                    running = false;
                }
                _ => panic!("unexpected Docker command"),
            }
            Ok(String::new())
        },
    )
    .unwrap();

    assert!(!paused);
    assert!(!running);
    assert_eq!(updates, [Some(ManagerPhase::Unpause), None]);
    assert_eq!(
        commands,
        vec![
            backend::strings(&["unpause", &id]),
            backend::strings(&["kill", "--signal", "KILL", &id]),
        ]
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_missing_registry_initializes_a_fresh_incarnation() {
    let root = temp_dir("fresh-registry");
    let config = test_config(&root);
    // A root with no registry restores nothing and initializes a fresh
    // registry, exactly as a first start.
    let registry = restore_registry(&root, &config).unwrap();
    validate_registry(&registry, &config).unwrap();
    assert_eq!(registry.version, 1);
    assert_eq!(registry.instance, config.instance);
    assert!(registry.records.is_empty());
    assert!(!registry.parent_pending);
    assert_eq!(registry.active, None);
    // Each uninitialized root gets its own incarnation.
    let again = restore_registry(&root, &config).unwrap();
    assert_ne!(again.incarnation, registry.incarnation);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_failed_open_removes_only_the_claim_this_process_created() {
    let root = temp_dir("fresh-claim");
    let path = root.join("instance.claim");
    let claim_root = root.display().to_string();
    // The acquisition itself arms the lease: a fresh claim is removed
    // when the open fails after claiming.
    let claim = instance_claim::hold_claim(&path, &root, &claim_root).unwrap();
    drop(claim);
    assert!(!path.try_exists().unwrap());
    // Disarming by value keeps the claim for the executor's lifetime.
    drop(
        instance_claim::hold_claim(&path, &root, &claim_root)
            .unwrap()
            .take(),
    );
    assert!(path.try_exists().unwrap());
    // A claim created by an earlier owner is never a lease, so a failure
    // here can never remove it; the quarantine in the shared claim path
    // is what refuses a registry-less one.
    fs::File::create(root.join("registry.json")).unwrap();
    // Dropping the non-lease keeps the file: a failure here can never
    // remove a claim created by an earlier owner.
    drop(instance_claim::hold_claim(&path, &root, &claim_root).unwrap());
    assert!(path.try_exists().unwrap());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn expired_settled_receipts_leave_retention_while_incomplete_cleanup_stays() {
    let root = temp_dir("expire");
    let config = test_config(&root);
    let mut registry = empty_registry(&config.instance);
    let mut settled = record_for(1, Phase::Intent);
    settled.state = State::Settled;
    settled.outcome = Some("refused-source-mismatch".into());
    settled.cleanup = Cleanup::Complete;
    settled.settled_at_unix_ms = Some(1000);
    let mut stuck = record_for(2, Phase::Released);
    stuck.state = State::Settling;
    stuck.outcome = Some("interrupted".into());
    stuck.cleanup = Cleanup::Uncertain;
    stuck.settled_at_unix_ms = Some(1000);
    registry.records.insert(1, settled);
    registry.records.insert(2, stuck);
    registry.active = Some(2);
    registry.watermark = 2;
    // Well inside the retention window both stay.
    expire(&mut registry, 2000, 86_400).unwrap();
    assert_eq!(registry.records.len(), 2);
    // After retention the complete receipt expires, the uncertain one stays.
    expire(&mut registry, 1000 + 86_400 * 1000, 86_400).unwrap();
    assert!(registry.records.contains_key(&2));
    assert!(!registry.records.contains_key(&1));
    assert_eq!(registry.active, Some(2));
    fs::remove_dir_all(&root).unwrap();
}