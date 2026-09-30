use super::qualification::{
    availability as qualified_availability, failure as qualification_failure,
    plan as plan_qualified_start, restore as restore_qualifications,
};
use super::*;

fn events(kills: u64) -> Events {
    Events {
        oom: kills,
        oom_kill: kills,
        oom_group_kill: kills,
        local_oom: 0,
        local_oom_kill: 0,
        local_oom_group_kill: 0,
    }
}
fn evidence(code: u8, flag: bool, before: Option<u64>, after: Option<u64>) -> Evidence {
    Evidence {
        peak_bytes: 1,
        exit_code: Some(code),
        docker_oom_killed: Some(flag),
        attempt_before: before.map(events),
        attempt_after: after.map(events),
        parent_before: None,
        parent_after: None,
        populated: Some(false),
        terminal_snapshot: None,
    }
}
#[test]
fn foreign_symlink_directory_is_rejected_before_chmod() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join(format!(
        "slipstream-processing-symlink-{}",
        random_id().unwrap()
    ));
    fs::create_dir(&root).unwrap();
    let target = root.join("foreign");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o751)).unwrap();
    symlink(&target, root.join("attempts")).unwrap();
    assert_eq!(
        prepare_private_directory(&root.join("attempts")),
        Err(ErrorCode::Unavailable)
    );
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o751);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oom_requires_kernel_delta_and_runtime_evidence() {
    assert_eq!(
        classify(&evidence(137, true, Some(0), Some(1)), None, None),
        Outcome::Oom
    );
    for value in [
        evidence(137, true, Some(1), Some(1)),
        evidence(137, true, None, Some(1)),
        evidence(137, true, Some(2), Some(1)),
        evidence(137, false, Some(0), Some(0)),
    ] {
        assert_eq!(classify(&value, None, None), Outcome::EngineFailed);
    }
    assert_eq!(
        classify(&evidence(76, false, Some(0), Some(0)), None, None),
        Outcome::Deadline
    );
}
#[test]
fn descendant_oom_survives_a_false_or_missing_docker_flag_and_cancellation() {
    // Shape captured by the native leaf-pressure regression: the leaf
    // disappears, local counters stay zero, and Docker reports false.
    let mut value = evidence(137, false, Some(0), Some(0));
    value.attempt_after = Some(Events {
        oom: 1,
        oom_kill: 3,
        oom_group_kill: 1,
        ..Events::default()
    });
    for flag in [Some(false), None, Some(true)] {
        value.docker_oom_killed = flag;
        for requested in [None, Some(Outcome::Cancelled), Some(Outcome::Interrupted)] {
            assert_eq!(classify(&value, None, requested), Outcome::Oom);
        }
    }
}

#[test]
fn fallback_oom_requires_terminal_kill_and_owned_limit_pressure() {
    let mut value = evidence(137, false, Some(0), Some(0));
    value.attempt_after.as_mut().unwrap().oom_kill = 3;
    // A host/global kill is insufficient even if an unrelated child of
    // the parent raised its hierarchical pressure counter.
    value.parent_before = Some(Events::default());
    value.parent_after = Some(events(3));
    assert_eq!(classify(&value, None, None), Outcome::EngineFailed);
    value.parent_after.as_mut().unwrap().local_oom = 1;
    assert_eq!(classify(&value, None, None), Outcome::Oom);

    for missing_before in [true, false] {
        let mut missing = value.clone();
        if missing_before {
            missing.parent_before = None;
        } else {
            missing.parent_after = None;
        }
        assert_eq!(classify(&missing, None, None), Outcome::EngineFailed);
    }
    let mut regressed = value.clone();
    regressed.parent_before.as_mut().unwrap().local_oom = 2;
    assert_eq!(classify(&regressed, None, None), Outcome::EngineFailed);
    for before in [None, Some(events(3)), Some(events(4))] {
        let mut missing_kill = value.clone();
        missing_kill.attempt_before = before;
        assert_eq!(classify(&missing_kill, None, None), Outcome::EngineFailed);
    }
    let mut missing_kill = value.clone();
    missing_kill.attempt_after = None;
    assert_eq!(classify(&missing_kill, None, None), Outcome::EngineFailed);

    value.parent_before = None;
    value.parent_after = None;
    value.attempt_before.as_mut().unwrap().oom = 2;
    value.attempt_after.as_mut().unwrap().oom = 1;
    assert_eq!(classify(&value, None, None), Outcome::EngineFailed);
    value.attempt_after.as_mut().unwrap().oom = 3;
    assert_eq!(classify(&value, None, None), Outcome::Oom);
    for (code, worker, expected) in [
        (Some(0), Some(Outcome::Completed), Outcome::Completed),
        (
            Some(20),
            Some(Outcome::AllocationFailed),
            Outcome::AllocationFailed,
        ),
        (Some(21), Some(Outcome::StorageFull), Outcome::StorageFull),
        (Some(76), None, Outcome::Deadline),
        (Some(75), None, Outcome::EngineFailed),
        (None, None, Outcome::Unknown),
    ] {
        value.exit_code = code;
        assert_eq!(classify(&value, worker, None), expected);
    }
}
#[test]
fn completed_result_survives_restart_and_late_cancellation() {
    let value = evidence(0, false, Some(0), Some(0));
    for requested in [None, Some(Outcome::Interrupted), Some(Outcome::Cancelled)] {
        assert_eq!(
            classify(&value, Some(Outcome::Completed), requested),
            Outcome::Completed
        );
    }
    assert_eq!(classify(&value, None, None), Outcome::EngineFailed);
    assert_eq!(
        classify(
            &evidence(137, false, Some(0), Some(0)),
            None,
            Some(Outcome::Cancelled)
        ),
        Outcome::Cancelled
    );
    assert_eq!(
        classify(
            &evidence(137, true, Some(0), Some(1)),
            None,
            Some(Outcome::Cancelled)
        ),
        Outcome::Oom
    );
}
#[test]
fn restart_preserves_all_proven_terminal_outcomes() {
    for (code, worker, expected) in [
        (
            20,
            Some(Outcome::AllocationFailed),
            Outcome::AllocationFailed,
        ),
        (21, Some(Outcome::StorageFull), Outcome::StorageFull),
        (76, None, Outcome::Deadline),
    ] {
        assert_eq!(
            classify(
                &evidence(code, false, Some(0), Some(0)),
                worker,
                Some(Outcome::Interrupted)
            ),
            expected
        );
    }
}

fn record(sequence: u64, state: State) -> Record {
    let launch_id = format!("{sequence:032x}");
    Record {
        receipt: Receipt {
            incarnation: "1".repeat(32),
            sequence,
            workload: Workload::ProbeSuccess,
            policy: "2".repeat(64),
            bundle: "3".repeat(64),
            state,
            cancellation_requested: false,
            accepted_at_unix_ms: 0,
            deadline_unix_ms: 30000,
            outcome: Some(Outcome::Completed),
            runtime: Some(Runtime {
                launch_id: launch_id.clone(),
                container_id: None,
                attempt_unit: format!("slipstreamprocessing{}-{launch_id}.slice", "0".repeat(32)),
            }),
            limits: Limits::new(64 * 1024 * 1024),
            evidence: Some(evidence(0, false, Some(0), Some(0))),
            cleanup: Cleanup::Complete,
        },
        launch_id,
        image_id: format!("sha256:{}", "4".repeat(64)),
        unit_invocation: None,
        cgroup_inode: None,
        mount_id: None,
        released: true,
        termination_reason: None,
        manager_pending: None,
        stop_confirmed: false,
        settled_at_unix_ms: Some(1000),
        film: None,
    }
}
fn registry() -> Registry {
    Registry {
        version: 1,
        instance: "0".repeat(32),
        incarnation: "1".repeat(32),
        watermark: 256,
        parent_pending: false,
        parent_identity: Some(ParentIdentity {
            invocation: "a".repeat(32),
            inode: 1,
        }),
        active: None,
        records: (1..=256).map(|i| (i, record(i, State::Settled))).collect(),
        invalidations: None,
    }
}

#[test]
fn legacy_records_do_not_invent_confirmed_stops() {
    let mut value = serde_json::to_value(record(1, State::Settled)).unwrap();
    value.as_object_mut().unwrap().remove("stop_confirmed");
    let restored: Record = serde_json::from_value(value).unwrap();
    assert!(!restored.stop_confirmed);
    let mut confirmed = serde_json::to_value(restored).unwrap();
    confirmed["stop_confirmed"] = serde_json::json!(true);
    let restored: Record = serde_json::from_value(confirmed).unwrap();
    assert!(restored.stop_confirmed);
}

pub(crate) fn qualified_record(sequence: u64) -> Record {
    use crate::{film, qualified};
    let mut record = record(sequence, State::Settled);
    let mut grant = film::test_grant();
    let plan = qualified::Plan::calculate(
        &grant.fixture,
        &qualified::Case::Qualified {
            fixture_id: grant.fixture.id.clone(),
            empirical_ceiling_bytes: 1 << 30,
            safety_reserve_bytes: 1 << 20,
            evidence_sha256: "a".repeat(64),
        },
        &"b".repeat(64),
        &"c".repeat(64),
        8 << 30,
    )
    .unwrap();
    grant.version = 3;
    grant.kind = "film-qualified-grant".into();
    grant.launch_id = record.launch_id.clone();
    grant.bundle = record.receipt.bundle.clone();
    record.receipt.workload = Workload::Film(film::Workload {
        kind: "film-fixture".into(),
        fixture_id: grant.fixture.id.clone(),
    });
    record.receipt.limits = film::limits(8 << 30);
    record.unit_invocation = Some("d".repeat(32));
    record.cgroup_inode = Some(1);
    record.film = Some(film::Captured {
        catalogue: "e".repeat(64),
        resource_model: plan.envelope_sha256.clone(),
        grant: grant.map_plan(|_| film::ExecutionPlan::Qualified(plan)),
        phase: film::Phase::ExecutionFinished,
        stage_release_intent: true,
        engine_release_intent: true,
        grant_file: None,
        snapshot: None,
        result_file: None,
        result: None,
        detail: None,
        qualification_failure: None,
        qualification_observation_valid: Some(true),
    });
    record
}

fn qualified_registry(record: Record) -> Registry {
    Registry {
        version: 3,
        invalidations: Some(Vec::new()),
        watermark: record.receipt.sequence,
        records: BTreeMap::from([(record.receipt.sequence, record)]),
        ..registry()
    }
}

#[test]
fn full_invalidation_capacity_refuses_new_plans_without_redefining_capability() {
    use crate::{film, qualified};
    let record = qualified_record(1);
    let captured = record.film.as_ref().unwrap();
    let grant = &captured.grant;
    let documents = qualified::Documents {
        catalogue: film::Catalogue {
            version: 1,
            numerical_bundle: grant.numerical_bundle.clone(),
            recipe: grant.recipe.clone(),
            procedure: grant.procedure.clone(),
            reference_image: record.image_id.clone(),
            input_icc_sha256: grant.input_icc_sha256.clone(),
            output_icc_sha256: grant.output_icc_sha256.clone(),
            fixtures: vec![grant.fixture.clone()],
        },
        envelope: qualified::Envelope {
            version: 1,
            formula: qualified::FORMULA.into(),
            inventory: qualified::INVENTORY.into(),
            catalogue_sha256: captured.catalogue.clone(),
            image: record.image_id.clone(),
            launcher_sha256: "c".repeat(64),
            environment: qualified::Environment {
                machine: "x86_64".into(),
                kernel_release: "test-kernel".into(),
                page_bytes: 4096,
                cpu_sha256: "d".repeat(64),
                manager_sha256: "e".repeat(64),
            },
            cases: vec![qualified::Case::Qualified {
                fixture_id: grant.fixture.id.clone(),
                empirical_ceiling_bytes: 1 << 30,
                safety_reserve_bytes: 1 << 20,
                evidence_sha256: "f".repeat(64),
            }],
        },
    };
    let mut failures: Vec<_> = (1..=256)
        .map(|sequence| qualified::Invalidation {
            envelope: format!("{sequence:064x}"),
            fixture_id: grant.fixture.id.clone(),
            reason: qualified::QualificationFailure::PeakExceeded,
            incarnation: record.receipt.incarnation.clone(),
            sequence,
        })
        .collect();
    let envelope = &captured.resource_model;
    assert_eq!(
        qualified_availability(true, &documents.envelope.cases, envelope, &failures),
        qualified::Availability::Available
    );
    assert_eq!(
        plan_qualified_start(&documents, &failures, &grant.fixture.id, envelope, 8 << 30),
        Err(ErrorCode::Capacity)
    );
    failures[0].envelope = envelope.clone();
    assert_eq!(
        qualified_availability(true, &documents.envelope.cases, envelope, &failures),
        qualified::Availability::Unqualified
    );
    assert_eq!(
        plan_qualified_start(&documents, &failures, &grant.fixture.id, envelope, 8 << 30),
        Err(ErrorCode::Capacity)
    );
    assert_eq!(
        qualified_availability(false, &documents.envelope.cases, envelope, &failures),
        qualified::Availability::Blocked
    );
    failures.pop();
    assert_eq!(
        plan_qualified_start(&documents, &failures, &grant.fixture.id, envelope, 8 << 30),
        Err(ErrorCode::UnqualifiedEnvelope)
    );
    assert!(
        plan_qualified_start(
            &documents,
            &failures,
            &grant.fixture.id,
            &"a".repeat(64),
            8 << 30
        )
        .is_ok()
    );
    assert_eq!(failures.len(), 255);
}

#[test]
fn qualification_contradictions_preserve_outcome_and_require_owned_observations() {
    use crate::qualified::QualificationFailure as Failure;
    let mut record = qualified_record(1);
    let ceiling = record
        .film
        .as_ref()
        .unwrap()
        .grant
        .plan
        .qualified()
        .unwrap()
        .empirical_ceiling_bytes;
    record.receipt.evidence.as_mut().unwrap().peak_bytes = ceiling;
    assert_eq!(qualification_failure(&record), None);
    record.receipt.evidence.as_mut().unwrap().peak_bytes += 1;
    assert_eq!(qualification_failure(&record), Some(Failure::PeakExceeded));
    assert_eq!(record.receipt.outcome, Some(Outcome::Completed));
    record.receipt.outcome = Some(Outcome::Cancelled);
    assert_eq!(qualification_failure(&record), Some(Failure::PeakExceeded));
    record
        .film
        .as_mut()
        .unwrap()
        .qualification_observation_valid = Some(false);
    assert_eq!(qualification_failure(&record), None);
    record
        .film
        .as_mut()
        .unwrap()
        .qualification_observation_valid = Some(true);
    record.cgroup_inode = None;
    assert_eq!(qualification_failure(&record), None);
    record.cgroup_inode = Some(1);
    let evidence = record.receipt.evidence.as_mut().unwrap();
    evidence.peak_bytes = ceiling;
    evidence.attempt_after.as_mut().unwrap().oom_kill = 1;
    evidence.docker_oom_killed = Some(true);
    assert_eq!(
        qualification_failure(&record),
        None,
        "a kill can come from outside the owned boundary"
    );
    // Owned leaf pressure is hierarchical even when local counters and
    // Docker's flag remain unchanged. A partial worker outcome does not
    // erase this independently established qualification contradiction.
    let evidence = record.receipt.evidence.as_mut().unwrap();
    evidence.docker_oom_killed = Some(false);
    evidence.attempt_after.as_mut().unwrap().oom = 1;
    assert_eq!(qualification_failure(&record), Some(Failure::ProcessingOom));
    assert_eq!(record.receipt.outcome, Some(Outcome::Cancelled));
    record.receipt.evidence.as_mut().unwrap().peak_bytes = ceiling + 1;
    assert_eq!(qualification_failure(&record), Some(Failure::PeakExceeded));
    record.receipt.evidence.as_mut().unwrap().peak_bytes = ceiling;
    for populated in [Some(true), None] {
        record.receipt.evidence.as_mut().unwrap().populated = populated;
        assert_eq!(qualification_failure(&record), None);
    }
    record.receipt.evidence.as_mut().unwrap().populated = Some(false);
    record.unit_invocation = None;
    assert_eq!(qualification_failure(&record), None);
    record.unit_invocation = Some("a".repeat(32));
    record.cgroup_inode = None;
    assert_eq!(qualification_failure(&record), None);
    record.cgroup_inode = Some(1);
    record
        .film
        .as_mut()
        .unwrap()
        .qualification_observation_valid = Some(false);
    assert_eq!(qualification_failure(&record), None);
    record
        .film
        .as_mut()
        .unwrap()
        .qualification_observation_valid = Some(true);

    // An exact parent limit can kill this attempt without increasing the
    // attempt's own oom counter. Parent kill or pressure alone cannot
    // prove that this attempt was killed by an owned limit.
    let evidence = record.receipt.evidence.as_mut().unwrap();
    evidence.attempt_after.as_mut().unwrap().oom = 0;
    evidence.parent_before = Some(Events::default());
    evidence.parent_after = Some(Events {
        oom: 1,
        oom_kill: 1,
        local_oom: 1,
        ..Events::default()
    });
    assert_eq!(qualification_failure(&record), Some(Failure::ProcessingOom));
    let proven_parent = record.clone();
    for kind in 0..7 {
        let mut invalid = proven_parent.clone();
        let evidence = invalid.receipt.evidence.as_mut().unwrap();
        match kind {
            0 => evidence.attempt_before = None,
            1 => evidence.attempt_after = None,
            2 => evidence.attempt_after.as_mut().unwrap().oom_kill = 0,
            3 => evidence.attempt_before.as_mut().unwrap().oom_kill = 2,
            4 => evidence.parent_before = None,
            5 => evidence.parent_after.as_mut().unwrap().local_oom = 0,
            6 => evidence.parent_before.as_mut().unwrap().local_oom = 2,
            _ => unreachable!(),
        }
        assert_eq!(qualification_failure(&invalid), None, "case {kind}");
    }
    let evidence = record.receipt.evidence.as_mut().unwrap();
    evidence.attempt_after = Some(Events::default());
    evidence.parent_after = Some(Events::default());
    record.receipt.outcome = Some(Outcome::AllocationFailed);
    assert_eq!(
        qualification_failure(&record),
        Some(Failure::AllocationFailed)
    );
    for outcome in [
        Outcome::Cancelled,
        Outcome::StorageFull,
        Outcome::EngineFailed,
        Outcome::Interrupted,
    ] {
        record.receipt.outcome = Some(outcome);
        assert_eq!(qualification_failure(&record), None);
    }
}

#[test]
fn recovered_descendant_oom_withdraws_the_exact_qualified_case() {
    let mut record = qualified_record(1);
    let evidence = record.receipt.evidence.as_mut().unwrap();
    evidence.exit_code = Some(137);
    evidence.docker_oom_killed = Some(false);
    evidence.attempt_after = Some(Events {
        oom: 1,
        oom_kill: 3,
        oom_group_kill: 1,
        ..Events::default()
    });
    record.receipt.outcome = Some(classify(evidence, None, None));
    let fixture = record.film.as_ref().unwrap().grant.fixture.id.clone();
    let envelope = record.film.as_ref().unwrap().resource_model.clone();
    let mut registry = qualified_registry(record);
    restore_qualifications(&mut registry).unwrap();
    let invalidations = registry.invalidations.as_ref().unwrap();
    assert_eq!(invalidations.len(), 1);
    assert_eq!(invalidations[0].fixture_id, fixture);
    assert_eq!(invalidations[0].envelope, envelope);
    assert_eq!(
        invalidations[0].reason,
        crate::qualified::QualificationFailure::ProcessingOom
    );
    assert_eq!(registry.records[&1].receipt.outcome, Some(Outcome::Oom));
    let bytes = serde_json::to_vec(&registry).unwrap();
    let mut recovered: Registry = serde_json::from_slice(&bytes).unwrap();
    restore_qualifications(&mut recovered).unwrap();
    assert_eq!(serde_json::to_vec(&recovered).unwrap(), bytes);
}

#[test]
fn recovered_contradiction_is_atomic_and_survives_receipt_expiry_and_restart() {
    let mut record = qualified_record(1);
    record.receipt.outcome = Some(Outcome::AllocationFailed);
    let mut registry = qualified_registry(record);
    restore_qualifications(&mut registry).unwrap();
    let original = serde_json::to_vec(&registry).unwrap();
    let mut recovered: Registry = serde_json::from_slice(&original).unwrap();
    restore_qualifications(&mut recovered).unwrap();
    assert_eq!(serde_json::to_vec(&recovered).unwrap(), original);
    let record = recovered.records.get(&1).unwrap();
    assert_eq!(record.receipt.outcome, Some(Outcome::AllocationFailed));
    assert_eq!(
        record.film.as_ref().unwrap().qualification_failure,
        Some(crate::qualified::QualificationFailure::AllocationFailed)
    );
    let tombstone = recovered.invalidations.as_ref().unwrap()[0].clone();
    expire(&mut recovered, 2000, 1).unwrap();
    assert!(recovered.records.is_empty());
    assert_eq!(recovered.invalidations, Some(vec![tombstone]));
    let bytes = serde_json::to_vec(&recovered).unwrap();
    let mut recovered: Registry = serde_json::from_slice(&bytes).unwrap();
    restore_qualifications(&mut recovered).unwrap();
    assert_eq!(serde_json::to_vec(&recovered).unwrap(), bytes);
}

#[test]
fn invalidated_observation_context_survives_restart_without_a_false_model_failure() {
    let mut record = qualified_record(1);
    record.receipt.outcome = Some(Outcome::Interrupted);
    record.receipt.evidence.as_mut().unwrap().peak_bytes = u64::MAX;
    record
        .film
        .as_mut()
        .unwrap()
        .qualification_observation_valid = Some(false);
    let registry = qualified_registry(record);
    let mut recovered: Registry =
        serde_json::from_slice(&serde_json::to_vec(&registry).unwrap()).unwrap();
    restore_qualifications(&mut recovered).unwrap();
    assert!(recovered.invalidations.unwrap().is_empty());
    let captured = recovered.records[&1].film.as_ref().unwrap();
    assert_eq!(captured.qualification_observation_valid, Some(false));
    assert_eq!(captured.qualification_failure, None);
}

#[test]
fn missing_or_full_invalidation_state_cannot_be_reinitialized_during_recovery() {
    let mut record = qualified_record(256);
    record.receipt.outcome = Some(Outcome::AllocationFailed);
    let mut registry = qualified_registry(record);
    registry.invalidations = None;
    assert_eq!(
        restore_qualifications(&mut registry),
        Err(ErrorCode::Uncertain)
    );
    registry.invalidations = Some(
        (1..=256)
            .map(|sequence| crate::qualified::Invalidation {
                envelope: format!("{sequence:064x}"),
                fixture_id: "a".repeat(32),
                reason: crate::qualified::QualificationFailure::PeakExceeded,
                incarnation: registry.incarnation.clone(),
                sequence,
            })
            .collect(),
    );
    let original = serde_json::to_vec(&registry).unwrap();
    assert_eq!(
        restore_qualifications(&mut registry),
        Err(ErrorCode::Capacity)
    );
    assert_eq!(serde_json::to_vec(&registry).unwrap(), original);
    registry.invalidations.as_mut().unwrap().pop();
    restore_qualifications(&mut registry).unwrap();
    assert_eq!(registry.invalidations.as_ref().unwrap().len(), 256);
    restore_qualifications(&mut registry).unwrap();
    assert_eq!(registry.invalidations.as_ref().unwrap().len(), 256);
}
#[test]
fn expiry_preserves_watermark_and_active_identity_at_capacity() {
    let mut value = registry();
    expire(&mut value, 1999, 1).unwrap();
    assert_eq!(value.records.len(), 256);
    value.active = Some(257);
    value.watermark = 257;
    value.records.insert(257, record(257, State::Running));
    expire(&mut value, 2000, 1).unwrap();
    assert_eq!(value.records.len(), 1);
    assert_eq!(value.watermark, 257);
    assert_eq!(value.active, Some(257));
    assert!(expire(&mut value, 2000, u64::MAX).is_err());
}
#[test]
fn registry_rejects_duplicate_active_or_rebound_runtime() {
    let config = Config {
        version: 1,
        mode: "qualification".into(),
        instance: "0".repeat(32),
        root: "/test".into(),
        socket: "/test.sock".into(),
        peer_uid: 0,
        image: format!("sha256:{}", "4".repeat(64)),
        memory_bytes: 64 * 1024 * 1024,
        receipt_retention_seconds: 1,
    };
    let original = registry();
    validate_registry(&original, &config).unwrap();
    let mut changed = original.clone();
    changed
        .records
        .get_mut(&1)
        .unwrap()
        .receipt
        .runtime
        .as_mut()
        .unwrap()
        .launch_id = "9".repeat(32);
    assert!(validate_registry(&changed, &config).is_err());
    let mut changed = original.clone();
    changed.records.get_mut(&1).unwrap().receipt.state = State::Running;
    assert!(validate_registry(&changed, &config).is_err());
    changed.active = Some(1);
    validate_registry(&changed, &config).unwrap();
    changed.records.get_mut(&2).unwrap().receipt.state = State::Running;
    assert!(validate_registry(&changed, &config).is_err());
    let mut changed = original;
    changed.records.get_mut(&1).unwrap().receipt.cleanup = Cleanup::Pending;
    assert!(validate_registry(&changed, &config).is_err());
    for field in ["policy", "bundle", "image", "invocation"] {
        let mut changed = registry();
        let record = changed.records.get_mut(&1).unwrap();
        match field {
            "policy" => record.receipt.policy.push('a'),
            "bundle" => record.receipt.bundle = "z".repeat(64),
            "image" => record.image_id.push('a'),
            "invocation" => record.unit_invocation = Some("f".repeat(33)),
            _ => unreachable!(),
        }
        assert_eq!(
            validate_registry(&changed, &config),
            Err(ErrorCode::Uncertain)
        );
    }
}
fn maximal_film_registry() -> Registry {
    use crate::film;
    let mut grant = film::test_grant();
    grant.fixture.width = 9568;
    grant.fixture.height = 9568;
    grant.fixture.source = film::Source::DevelopmentTiff {
        bytes: 2 * 1024 * 1024 * 1024,
        sha256: "e".repeat(64),
    };
    let case = film::ModelCase {
        fixture_id: grant.fixture.id.clone(),
        stages: film::STAGES
            .iter()
            .map(|stage| film::StageBounds {
                stage: *stage,
                runtime: film::Bound::Unknown,
                native: film::Bound::Unknown,
                allocator_retention: film::Bound::Unknown,
                kernel: film::Bound::Unknown,
            })
            .collect(),
    };
    grant.plan = film::plan(&grant.fixture, &case, 32 * 1024 * 1024 * 1024).unwrap();
    // Schema maxima intentionally over-approximate the compiled planner's current known subtotal.
    grant.plan.prediction = film::Prediction::Unqualified {
        known_required_bytes: u64::MAX,
        known_terms_exceed_limit: true,
        missing: film::STAGES
            .iter()
            .flat_map(|stage| {
                [
                    film::Term::OwnedArrays,
                    film::Term::Runtime,
                    film::Term::Native,
                    film::Term::AllocatorRetention,
                    film::Term::Kernel,
                ]
                .map(|term| film::MissingTerm {
                    stage: *stage,
                    term,
                })
            })
            .collect(),
    };
    let mut registry = registry();
    registry.version = 2;
    registry.watermark = u64::MAX;
    registry.records = registry
        .records
        .into_values()
        .enumerate()
        .map(|(index, mut r)| {
            let seq = u64::MAX - index as u64;
            r.receipt.sequence = seq;
            (seq, r)
        })
        .collect();
    for record in registry.records.values_mut() {
        grant.launch_id = record.launch_id.clone();
        record.receipt.workload = Workload::Film(film::Workload {
            kind: "film-fixture".into(),
            fixture_id: grant.fixture.id.clone(),
        });
        record.receipt.limits = film::limits(32 * 1024 * 1024 * 1024);
        record.receipt.accepted_at_unix_ms = u64::MAX - 900000;
        record.receipt.deadline_unix_ms = u64::MAX;
        record.receipt.runtime.as_mut().unwrap().container_id =
            Some(format!("{:064x}", record.receipt.sequence));
        let r = &grant.fixture.reference;
        record.film = Some(film::Captured {
            catalogue: "b".repeat(64),
            resource_model: "c".repeat(64),
            grant: grant
                .clone()
                .map_plan(crate::film::ExecutionPlan::Measurement),
            phase: film::Phase::ExecutionFinished,
            stage_release_intent: true,
            engine_release_intent: true,
            grant_file: Some(film::FileIdentity {
                device: u64::MAX,
                inode: u64::MAX,
            }),
            snapshot: Some(film::FileIdentity {
                device: u64::MAX,
                inode: u64::MAX,
            }),
            result_file: Some(film::FileIdentity {
                device: u64::MAX,
                inode: u64::MAX,
            }),
            detail: Some(film::Detail::UnsupportedInput),
            qualification_failure: None,
            qualification_observation_valid: None,
            result: Some(film::WorkerResult::Success(film::WorkerSuccess {
                version: 2,
                kind: "film-measurement-result".into(),
                outcome: Outcome::Completed,
                launch_id: record.launch_id.clone(),
                manifest: grant.manifest.clone(),
                plan_sha256: film::hash(&grant.plan).unwrap(),
                artifact: film::Artifact {
                    input_pixels_sha256: r.input_pixels_sha256.clone(),
                    film_pixels_sha256: r.film_pixels_sha256.clone(),
                    jpeg_sha256: r.jpeg_sha256.clone(),
                    jpeg_bytes: r.jpeg_bytes,
                    width: grant.fixture.width,
                    height: grant.fixture.height,
                    icc_sha256: grant.output_icc_sha256.clone(),
                    reference_evidence_sha256: r.evidence_sha256.clone(),
                },
                execution_us: u64::MAX,
                stages: film::STAGES
                    .iter()
                    .map(|stage| film::Timing {
                        stage: *stage,
                        elapsed_us: u64::MAX,
                        reclaim_us: u64::MAX,
                    })
                    .collect(),
            })),
        });
        record.receipt.outcome = Some(Outcome::Completed);
        let events = Events {
            oom: u64::MAX,
            oom_kill: u64::MAX,
            oom_group_kill: u64::MAX,
            local_oom: u64::MAX,
            local_oom_kill: u64::MAX,
            local_oom_group_kill: u64::MAX,
        };
        let memory_peak_raw = format!("{}\n", u64::MAX);
        let memory_max_raw = format!("{}\n", record.receipt.limits.memory_bytes);
        let memory_swap_current_raw = "0\n".to_owned();
        let memory_swap_max_raw = "0\n".to_owned();
        let mut memory_events_raw = format!(
            "oom {}\noom_kill {}\noom_group_kill {}\n",
            u64::MAX,
            u64::MAX,
            u64::MAX
        );
        let mut memory_events_local_raw = memory_events_raw.clone();
        let mut padding_index = 0_u64;
        loop {
            let total = memory_peak_raw.len()
                + memory_max_raw.len()
                + memory_swap_current_raw.len()
                + memory_swap_max_raw.len()
                + memory_events_raw.len()
                + memory_events_local_raw.len();
            let line = format!("future_{padding_index} 0\n");
            if total + line.len() > TERMINAL_SNAPSHOT_BYTES {
                break;
            }
            if padding_index.is_multiple_of(2) {
                memory_events_raw.push_str(&line);
            } else {
                memory_events_local_raw.push_str(&line);
            }
            padding_index += 1;
        }
        record.receipt.evidence = Some(Evidence {
            peak_bytes: u64::MAX,
            exit_code: Some(0),
            docker_oom_killed: Some(false),
            attempt_before: Some(events.clone()),
            attempt_after: Some(events.clone()),
            parent_before: Some(events.clone()),
            parent_after: Some(events),
            populated: Some(false),
            terminal_snapshot: Some(TerminalSnapshot {
                cgroup_path: format!(
                    "/sys/fs/cgroup/slipstreamprocessing0.slice/{}",
                    record.unit()
                ),
                cgroup_inode: u64::MAX,
                unit_invocation: "6".repeat(32),
                launch_id: record.launch_id.clone(),
                container_id: record
                    .receipt
                    .runtime
                    .as_ref()
                    .unwrap()
                    .container_id
                    .clone()
                    .unwrap(),
                attempt_unit: record.unit().to_owned(),
                incarnation: registry.incarnation.clone(),
                sequence: record.receipt.sequence,
                memory_peak_raw,
                memory_max_raw,
                memory_swap_current_raw,
                memory_swap_max_raw,
                memory_events_raw,
                memory_events_local_raw,
                io_stat_raw: None,
            }),
        });
    }
    registry
}

#[test]
fn maximal_film_receipts_fit_all_fixed_protocol_and_journal_bounds() {
    use crate::film;
    let registry = maximal_film_registry();
    let bytes = serde_json::to_vec(&registry).unwrap();
    let record = registry.records.get(&u64::MAX).unwrap();
    let captured = record.film.as_ref().unwrap();
    let grant_bytes = film::canonical(&captured.grant).unwrap();
    let response = serde_json::to_vec(&Response::Result {
        version: 2,
        result: Box::new(record.result_body()),
    })
    .unwrap();
    let worker = serde_json::to_vec(captured.result.as_ref().unwrap()).unwrap();
    let wire: serde_json::Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(wire["result"]["kind"], "receipt");
    assert_eq!(
        wire["result"]["receipt"]["workload"]["kind"],
        "film-fixture"
    );
    assert!(bytes.len() < 4 * 1024 * 1024, "journal {}", bytes.len());
    assert!(grant_bytes.len() <= film::FRAME);
    assert!(response.len() <= RESPONSE_BYTES);
    assert!(worker.len() <= film::FRAME - 4);
    println!(
        "maximal Film bounds: journal256={} grant={} response={} worker={}",
        bytes.len(),
        grant_bytes.len(),
        response.len(),
        worker.len()
    );
}

#[test]
fn maximal_qualified_internal_records_reserve_terminal_capacity_with_all_invalidations() {
    use crate::{film, qualified};
    let mut registry = maximal_film_registry();
    registry.version = 3;
    registry.invalidations = Some(Vec::new());
    for record in registry.records.values_mut() {
        let captured = record.film.as_mut().unwrap();
        let grant = &mut captured.grant;
        grant.version = 3;
        grant.kind = "film-qualified-grant".into();
        grant.bundle = record.receipt.bundle.clone();
        captured.resource_model = format!("{:064x}", record.receipt.sequence);
        let plan = qualified::Plan::calculate(
            &grant.fixture,
            &qualified::Case::Qualified {
                fixture_id: grant.fixture.id.clone(),
                empirical_ceiling_bytes: 13 << 30,
                safety_reserve_bytes: (32 << 30)
                    - film::STORAGE
                    - grant.fixture.source_bytes()
                    - (13 << 30),
                evidence_sha256: "f".repeat(64),
            },
            &captured.resource_model,
            &"a".repeat(64),
            32 << 30,
        )
        .unwrap();
        assert_eq!(plan.required_bytes, 32 << 30);
        assert_eq!(plan.missing.len(), 58);
        grant.plan = film::ExecutionPlan::Qualified(plan);
        grant.validate().unwrap();
        if let Some(film::WorkerResult::Success(result)) = &mut captured.result {
            result.plan_sha256 = film::hash(&grant.plan).unwrap();
            result.artifact.width = grant.fixture.width;
            result.artifact.height = grant.fixture.height;
        }
        captured.result.as_ref().unwrap().validate(grant).unwrap();
        captured.qualification_observation_valid = Some(true);
        record.unit_invocation = Some("f".repeat(32));
        record.cgroup_inode = Some(u64::MAX);
        record.mount_id = Some(u64::MAX);
        record.termination_reason = Some(Outcome::Interrupted);
        record.settled_at_unix_ms = Some(u64::MAX);
    }
    restore_qualifications(&mut registry).unwrap();
    assert_eq!(registry.invalidations.as_ref().unwrap().len(), 256);
    let config = Config {
        version: 3,
        mode: "film-qualified-fixtures".into(),
        instance: registry.instance.clone(),
        root: "/test".into(),
        socket: "/test.sock".into(),
        peer_uid: 0,
        image: format!("sha256:{}", "4".repeat(64)),
        memory_bytes: 32 << 30,
        receipt_retention_seconds: 604800,
    };
    validate_registry(&registry, &config).unwrap();
    let bytes = serde_json::to_vec(&registry).unwrap();
    let mut recovered: Registry = serde_json::from_slice(&bytes).unwrap();
    validate_registry(&recovered, &config).unwrap();
    restore_qualifications(&mut recovered).unwrap();
    assert_eq!(serde_json::to_vec(&recovered).unwrap(), bytes);
    let record = &registry.records[&u64::MAX];
    let grant = film::canonical(&record.film.as_ref().unwrap().grant).unwrap();
    let response = serde_json::to_vec(&Response::Result {
        version: 3,
        result: Box::new(record.result_body()),
    })
    .unwrap();
    let _: Response = serde_json::from_slice(&response).unwrap();
    let wire: serde_json::Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(
        wire["result"]["receipt"]["qualification_failure"],
        "peak-exceeded"
    );
    assert!(wire["result"]["receipt"].get("resource_model").is_none());
    assert!(
        bytes.len() < 4 * 1024 * 1024,
        "registry bytes={}",
        bytes.len()
    );
    assert!(grant.len() <= film::FRAME);
    assert!(response.len() <= RESPONSE_BYTES);
    // Each admitted record is structurally bounded by this fully populated
    // capture, including its future terminal result and tombstone. There is
    // no reserve that relies on the much smaller accepted-state record.
    println!(
        "maximal qualified bounds: registry256+invalidations256={} grant={} response={}",
        bytes.len(),
        grant.len(),
        response.len()
    );
}