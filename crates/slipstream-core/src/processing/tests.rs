use super::*;

fn module() -> ProcessingModuleId {
    ProcessingModuleId::new("darktable").unwrap()
}

fn parameters() -> ProcessingParameterSnapshot {
    ProcessingParameterSnapshot::new(
        "filmcurves-v3",
        serde_json::json!({"tone": {"ev": 0.5}, "order": ["tone"]}),
    )
    .unwrap()
}

fn full_geometry() -> ProcessingGeometry {
    ProcessingGeometry::new(6048, 4024).unwrap()
}

fn contract() -> ProcessingImageContract {
    ProcessingImageContract {
        format: "image/tiff".into(),
        precision: "uint16".into(),
        color_space: "prophoto-rgb".into(),
        transfer: "linear".into(),
        geometry: full_geometry(),
        encoding: "deflate".into(),
    }
}

fn original_input() -> ProcessingInput {
    ProcessingInput::Original {
        photo_id: "photo-1".into(),
        source_revision: "source-9".into(),
    }
}

fn artifact_input(artifact_id: &str) -> ProcessingInput {
    ProcessingInput::Artifact {
        artifact_id: ProcessingArtifactId::new(artifact_id).unwrap(),
        contract: contract(),
    }
}

fn step(step_id: &str, input: ProcessingInput) -> ProcessingStep {
    ProcessingStep {
        step_id: ProcessingStepId::new(step_id).unwrap(),
        module: module(),
        input,
        parameters: parameters(),
    }
}

fn recipe(steps: Vec<ProcessingStep>, current: Option<&str>) -> ComposableEditRecipe {
    ComposableEditRecipe {
        photo_id: "photo-1".into(),
        revision: "recipe-4".into(),
        source_revision: "source-9".into(),
        current_step_id: current.map(|step_id| ProcessingStepId::new(step_id).unwrap()),
        steps,
    }
}

fn evidence(input: ProcessingInput) -> ProcessingInputEvidence {
    ProcessingInputEvidence::new(input, &"a1".repeat(32), 118_640_128).unwrap()
}

fn preview_identity(input: ProcessingInputEvidence) -> ProcessingPreviewIdentity {
    ProcessingPreviewIdentity {
        input,
        module: module(),
        adapter_schema_version: "adapter-v2".into(),
        parameter_digest: parameters().canonical_digest(),
        output_contract: contract(),
        bundle_id: "bundle-7".into(),
        geometry: ProcessingGeometry::new(1224, 816).unwrap(),
        display_conversion: Some("display-transform-v1".into()),
        invocation_digest: None,
    }
}

fn export_identity(input: ProcessingInputEvidence) -> ProcessingExportIdentity {
    ProcessingExportIdentity {
        input,
        module: module(),
        adapter_schema_version: "adapter-v2".into(),
        parameter_digest: parameters().canonical_digest(),
        output_contract: contract(),
        bundle_id: "bundle-7".into(),
    }
}

#[test]
fn long_opaque_source_revision_remains_distinct_from_recipe_identity() {
    let source = format!("{}\0size\0timestamp", "nested/".repeat(256));
    let mut saved = recipe(
        vec![step(
            "develop",
            ProcessingInput::Original {
                photo_id: "photo-1".into(),
                source_revision: source.clone(),
            },
        )],
        Some("develop"),
    );
    saved.source_revision = source.clone();
    saved.validate().unwrap();
    assert!(validate_revision(&source).is_err());
    assert!(validate_source_revision(&"x".repeat(MAXIMUM_SOURCE_REVISION_BYTES)).is_ok());
    assert!(validate_source_revision(&"x".repeat(MAXIMUM_SOURCE_REVISION_BYTES + 1)).is_err());
    saved.revision = "x".repeat(MAXIMUM_REVISION_BYTES + 1);
    assert!(saved.validate().is_err());
}

#[test]
fn zero_one_and_many_step_recipes_validate() {
    let empty = recipe(vec![], None);
    empty.validate().unwrap();
    assert_eq!(empty.current_step(), None);

    let one = recipe(vec![step("develop", original_input())], Some("develop"));
    one.validate().unwrap();
    assert_eq!(
        one.current_step().map(|step| step.step_id.as_str()),
        Some("develop")
    );

    let many = recipe(
        vec![
            step("develop", original_input()),
            step("film", artifact_input("artifact-1")),
            step("develop-again", artifact_input("artifact-2")),
        ],
        Some("film"),
    );
    many.validate().unwrap();
    assert_eq!(
        many.current_step().map(|step| step.step_id.as_str()),
        Some("film")
    );
    assert!(matches!(
        many.current_step().map(|step| &step.input),
        Some(ProcessingInput::Artifact { .. })
    ));
}

#[test]
fn repeated_module_steps_use_distinct_step_ids() {
    // The same module, input, and parameters stay two distinct records
    // through their step ids alone.
    let first = step("develop-1", original_input());
    let second = step("develop-2", original_input());
    assert_ne!(first, second);
    recipe(vec![first.clone(), second.clone()], Some("develop-2"))
        .validate()
        .unwrap();

    // Two steps with different artifacts remain distinct even when their
    // module and parameters match.
    let artifact_first = step("film-1", artifact_input("artifact-1"));
    let artifact_second = step("film-2", artifact_input("artifact-2"));
    assert_ne!(artifact_first, artifact_second);

    // Reusing one step id inside a recipe is refused.
    let duplicate = recipe(
        vec![first, step("develop-1", artifact_input("artifact-1"))],
        Some("develop-1"),
    );
    assert_eq!(
        duplicate.validate(),
        Err(ProcessingContractError::DuplicateStepId {
            step_id: "develop-1".into()
        })
    );
}

#[test]
fn current_step_must_select_one_recipe_member() {
    let members = vec![
        step("develop", original_input()),
        step("film", artifact_input("artifact-1")),
    ];

    let missing = recipe(members.clone(), Some("develop-3"));
    assert_eq!(
        missing.validate(),
        Err(ProcessingContractError::CurrentStepNotInRecipe {
            step_id: "develop-3".into()
        })
    );

    let unset = recipe(members.clone(), None);
    assert_eq!(
        unset.validate(),
        Err(ProcessingContractError::CurrentStepUnset)
    );

    // A selection without any steps cannot name a member.
    let selected_without_steps = recipe(vec![], Some("develop"));
    assert_eq!(
        selected_without_steps.validate(),
        Err(ProcessingContractError::CurrentStepNotInRecipe {
            step_id: "develop".into()
        })
    );

    recipe(members, Some("film")).validate().unwrap();
}

#[test]
fn identifiers_geometry_and_snapshots_are_strictly_bounded() {
    assert_eq!(
        ProcessingModuleId::new(""),
        Err(ProcessingContractError::IdentifierEmpty)
    );
    assert_eq!(
        ProcessingModuleId::new(" darktable"),
        Err(ProcessingContractError::IdentifierUntrimmed)
    );
    assert_eq!(
        ProcessingStepId::new("step\u{0}id"),
        Err(ProcessingContractError::IdentifierControlCharacter)
    );
    assert_eq!(
        ProcessingModuleId::new(&"m".repeat(MAXIMUM_MODULE_ID_BYTES + 1)),
        Err(ProcessingContractError::IdentifierTooLong {
            maximum: MAXIMUM_MODULE_ID_BYTES,
            actual: MAXIMUM_MODULE_ID_BYTES + 1,
        })
    );

    assert_eq!(
        ProcessingGeometry::new(0, 4024),
        Err(ProcessingContractError::GeometryEdgeZero)
    );
    assert_eq!(
        ProcessingGeometry::new(MAXIMUM_GEOMETRY_EDGE + 1, 4024),
        Err(ProcessingContractError::GeometryEdgeTooLarge {
            maximum: MAXIMUM_GEOMETRY_EDGE,
            actual: MAXIMUM_GEOMETRY_EDGE + 1,
        })
    );

    assert_eq!(
        ProcessingParameterSnapshot::new(
            "filmcurves-v3",
            Value::String("a".repeat(MAXIMUM_PARAMETER_SNAPSHOT_BYTES + 1)),
        ),
        Err(ProcessingContractError::ParameterSnapshotTooLarge {
            maximum: MAXIMUM_PARAMETER_SNAPSHOT_BYTES,
            actual: MAXIMUM_PARAMETER_SNAPSHOT_BYTES + 3,
        })
    );
    let mut deep = Value::Null;
    for _ in 0..MAXIMUM_PARAMETER_SNAPSHOT_DEPTH {
        deep = Value::Array(vec![deep]);
    }
    assert_eq!(
        ProcessingParameterSnapshot::new("filmcurves-v3", deep),
        Err(ProcessingContractError::ParameterSnapshotTooDeep {
            maximum: MAXIMUM_PARAMETER_SNAPSHOT_DEPTH,
            actual: MAXIMUM_PARAMETER_SNAPSHOT_DEPTH + 1,
        })
    );

    assert_eq!(
        ProcessingInputEvidence::new(original_input(), "deadbeef", 118_640_128),
        Err(ProcessingContractError::InvalidDigest {
            actual: "deadbeef".into()
        })
    );
    let uppercase = "A".repeat(DIGEST_HEX_BYTES);
    assert_eq!(
        ProcessingInputEvidence::new(original_input(), &uppercase, 118_640_128),
        Err(ProcessingContractError::InvalidDigest { actual: uppercase })
    );
    assert_eq!(
        ProcessingInputEvidence::new(original_input(), &"a1".repeat(32), 0),
        Err(ProcessingContractError::ZeroByteEvidence)
    );

    let too_many_steps = (0..=MAXIMUM_RECIPE_STEPS)
        .map(|index| step(&format!("step-{index}"), original_input()))
        .collect();
    let too_many = recipe(too_many_steps, Some("step-0"));
    assert_eq!(
        too_many.validate(),
        Err(ProcessingContractError::TooManySteps {
            maximum: MAXIMUM_RECIPE_STEPS,
            actual: MAXIMUM_RECIPE_STEPS + 1,
        })
    );
}

#[test]
fn original_and_artifact_inputs_never_share_identity() {
    let original = original_input();
    let artifact = artifact_input("artifact-1");
    assert_ne!(original, artifact);
    assert_ne!(original.binding_digest(), artifact.binding_digest());

    // Equal bindings digest equally: coalescing compares identity, not
    // construction.
    assert_eq!(original.binding_digest(), original_input().binding_digest());
    let artifact_again = artifact_input("artifact-1");
    assert_eq!(artifact.binding_digest(), artifact_again.binding_digest());
    assert_ne!(
        artifact.binding_digest(),
        artifact_input("artifact-2").binding_digest()
    );

    // Even with identical module, parameters, output contract, bundle,
    // geometry, and display conversion, the input binding keeps the two
    // Preview identities distinct.
    let preview_of_original = preview_identity(evidence(original.clone()));
    let preview_of_artifact = preview_identity(evidence(artifact.clone()));
    preview_of_original.validate().unwrap();
    preview_of_artifact.validate().unwrap();
    assert_ne!(preview_of_original, preview_of_artifact);
    assert_ne!(preview_of_original.digest(), preview_of_artifact.digest());
}

#[test]
fn parameter_changes_change_every_digest() {
    let base = parameters();
    let changed_tree = ProcessingParameterSnapshot::new(
        "filmcurves-v3",
        serde_json::json!({"order": ["tone"], "tone": {"ev": 0.5}}),
    )
    .unwrap();
    let changed_value = ProcessingParameterSnapshot::new(
        "filmcurves-v3",
        serde_json::json!({"tone": {"ev": 0.7}, "order": ["tone"]}),
    )
    .unwrap();
    let changed_version = ProcessingParameterSnapshot::new(
        "filmcurves-v4",
        serde_json::json!({"tone": {"ev": 0.5}, "order": ["tone"]}),
    )
    .unwrap();

    // Canonical: key order never changes the digest, any value or
    // version change does.
    assert_eq!(base.canonical_digest(), changed_tree.canonical_digest());
    assert_ne!(base.canonical_digest(), changed_value.canonical_digest());
    assert_ne!(base.canonical_digest(), changed_version.canonical_digest());

    let mut preview_changed = preview_identity(evidence(original_input()));
    preview_changed.parameter_digest = changed_value.canonical_digest();
    assert_ne!(
        preview_identity(evidence(original_input())).digest(),
        preview_changed.digest()
    );

    let mut export_changed = export_identity(evidence(original_input()));
    export_changed.parameter_digest = changed_value.canonical_digest();
    assert_ne!(
        export_identity(evidence(original_input())).digest(),
        export_changed.digest()
    );
}

#[test]
fn preview_identity_binds_rendition_choices_that_export_omits() {
    let base = preview_identity(evidence(original_input()));

    let mut larger_geometry = base.clone();
    larger_geometry.geometry = ProcessingGeometry::new(2448, 1632).unwrap();
    assert_ne!(base.digest(), larger_geometry.digest());

    let mut without_display = base.clone();
    without_display.display_conversion = None;
    assert_ne!(base.digest(), without_display.digest());

    let mut other_bundle = base.clone();
    other_bundle.bundle_id = "bundle-8".into();
    assert_ne!(base.digest(), other_bundle.digest());

    let mut derived_invocation = base.clone();
    derived_invocation.invocation_digest = Some(base.parameter_digest.clone());
    assert_ne!(base.digest(), derived_invocation.digest());

    // The Export identity binds the captured intent only: the same
    // parameter change moves it, while the Preview-only rendition
    // choices leave it untouched.
    let export = export_identity(evidence(original_input()));
    let mut export_changed_contract = export.clone();
    export_changed_contract.output_contract.encoding = "jpeg-95".into();
    assert_ne!(export.digest(), export_changed_contract.digest());
}

fn artifact_record() -> ProcessingArtifact {
    ProcessingArtifact {
        artifact_id: ProcessingArtifactId::new("artifact-1").unwrap(),
        photo_id: "photo-1".into(),
        step_id: ProcessingStepId::new("develop-1").unwrap(),
        module: module(),
        adapter_schema_version: "darktable-adapter-1".into(),
        parameters: parameters(),
        input: evidence(original_input()),
        output_contract: contract(),
        bundle_id: "bundle-7".into(),
        sha256: "a".repeat(64),
        byte_length: 12_288,
    }
}

#[test]
fn artifact_record_validates_provenance_and_byte_identity() {
    let record = artifact_record();
    record.validate().unwrap();
    assert!(record.same_publication(&record.clone()));

    let mut zero_bytes = record.clone();
    zero_bytes.byte_length = 0;
    assert!(zero_bytes.validate().is_err());

    let mut bad_digest = record.clone();
    bad_digest.sha256 = "not-a-digest".into();
    assert!(bad_digest.validate().is_err());

    let mut unbound_bundle = record.clone();
    unbound_bundle.bundle_id = String::new();
    assert!(unbound_bundle.validate().is_err());

    let mut other = record.clone();
    other.byte_length += 1;
    assert!(!record.same_publication(&other));
}

#[test]
fn adapter_decision_and_refusal_admit_only_bounded_reasons() {
    let decision = ProcessingExportAdapterDecision::NoQualifiedAdapter {
        reason_code: "module_parameters_unavailable".into(),
    };
    decision.validate().unwrap();
    assert_eq!(
        ProcessingExportAdapterDecision::NoQualifiedAdapter {
            reason_code: String::new()
        }
        .validate(),
        Err(ProcessingContractError::IdentifierEmpty)
    );

    let refusal = ProcessingExportRefusal {
        photo_id: "photo-1".into(),
        request_id: "export-request-1".into(),
        payload_digest: "b".repeat(64),
        step_id: ProcessingStepId::new("develop-1").unwrap(),
        recipe_revision: "recipe-9".into(),
        source_revision: "source-9".into(),
        module: module(),
        parameter_schema_version: parameters().schema_version,
        parameter_digest: parameters().canonical_digest(),
        input: original_input(),
        bundle_id: "bundle-7".into(),
        reason_code: "module_parameters_unavailable".into(),
    };
    refusal.validate().unwrap();
    let mut unbounded = refusal.clone();
    unbounded.reason_code = "x".repeat(MAXIMUM_CONTRACT_NAME_BYTES + 1);
    assert_eq!(
        unbounded.validate(),
        Err(ProcessingContractError::IdentifierTooLong {
            maximum: MAXIMUM_CONTRACT_NAME_BYTES,
            actual: MAXIMUM_CONTRACT_NAME_BYTES + 1,
        })
    );
}
