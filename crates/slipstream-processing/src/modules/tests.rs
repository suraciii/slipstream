use super::parameters::validate_darktable_tree;
use super::*;

#[path = "saved_intent_tests.rs"]
mod saved_intent_tests;

fn both_ready() -> ModuleRegistry {
    ModuleRegistry::new(ModuleAvailability::ready(), ModuleAvailability::ready())
}

fn arw_original_input() -> ConfinedInput {
    ConfinedInput {
        source_id: "photo:1a".repeat(2),
        digest: "1".repeat(64),
        byte_size: 128 * 1024 * 1024,
        contract: sony_arw_contract(),
    }
}

fn linear_tiff_input() -> ConfinedInput {
    ConfinedInput {
        source_id: "artifact:9b".repeat(2),
        digest: "2".repeat(64),
        byte_size: 9504 * 6336 * 6,
        contract: linear_prophoto_tiff_contract(),
    }
}

/// A linear ProPhoto handoff frame of another qualified geometry.
fn handoff_input(width: u64, height: u64) -> ConfinedInput {
    ConfinedInput {
        source_id: "artifact:c3".repeat(2),
        digest: "4".repeat(64),
        byte_size: width * height * 12,
        contract: ImageContract {
            format: "tiff".into(),
            color_space: "prophoto-rgb".into(),
            transfer_function: "linear".into(),
            precision_bits: 32,
            width,
            height,
        },
    }
}

fn darktable_parameters(tree: Value) -> Parameters {
    Parameters {
        module: DARKTABLE_MODULE.into(),
        version: DARKTABLE_PARAMETER_VERSION.into(),
        tree,
    }
}

fn spektrafilm_parameters(tree: Value) -> Parameters {
    Parameters {
        module: SPEKTRAFILM_MODULE.into(),
        version: SPEKTRAFILM_PARAMETER_VERSION.into(),
        tree,
    }
}

/// The only executable SpektraFilm tree: the pinned fixed recipe the
/// runtime itself emitted into its bundle, exactly as a caller
/// composes it from discovery's published group consts.
fn pinned_film_tree() -> Value {
    spektrafilm_default_tree()
}

/// The pinned module tree of the native photo worker's own tests: a
/// stack with every declared field plus the pinned development handoff.
fn worker_pinned_tree() -> Value {
    json!({
        "stack": [
            {
                "operation": "exposure",
                "multiPriority": 0,
                "enabled": true,
                "params": {"exposure": 0.25, "black": 0.0005},
                "before": "rawprepare",
                "after": "colorbalance"
            },
            {
                "operation": "colisa",
                "multiPriority": 0,
                "enabled": false,
                "params": {}
            }
        ],
        "output": {
            "format": "tiff",
            "precisionBits": 32,
            "colorSpace": "prophoto-rgb",
            "transferFunction": "linear"
        }
    })
}

/// A runner that records every invocation it executes and replays one
/// fixed outcome.
struct RecordingRunner {
    outcome: Result<ModuleOutput, ModuleError>,
    invocations: Vec<ModuleInvocation>,
}

impl RecordingRunner {
    fn producing(output: ModuleOutput) -> Self {
        Self {
            outcome: Ok(output),
            invocations: Vec::new(),
        }
    }

    fn failing(message: &str) -> Self {
        Self {
            outcome: Err(refusal(ModuleErrorCode::ExecutionFailed, message.into())),
            invocations: Vec::new(),
        }
    }

    fn executed(&self) -> usize {
        self.invocations.len()
    }
}

impl ModuleRunner for RecordingRunner {
    fn execute(&mut self, invocation: &ModuleInvocation) -> Result<ModuleOutput, ModuleError> {
        self.invocations.push(invocation.clone());
        self.outcome.clone()
    }
}

fn development_output() -> ModuleOutput {
    ModuleOutput {
        digest: "a".repeat(64),
        byte_size: 722_420_992,
        contract: linear_prophoto_tiff_contract(),
    }
}

fn finished_output(width: u64, height: u64) -> ModuleOutput {
    ModuleOutput {
        digest: "b".repeat(64),
        byte_size: 8 * 1024 * 1024,
        contract: ImageContract {
            format: "jpeg".into(),
            color_space: "srgb".into(),
            transfer_function: "srgb".into(),
            precision_bits: 8,
            width,
            height,
        },
    }
}

#[test]
fn discovery_reports_each_peer_independently() {
    let registry = ModuleRegistry::new(
        ModuleAvailability::ready(),
        ModuleAvailability::unavailable("spektrafilm runtime is not installed"),
    );
    let names: Vec<&str> = registry
        .descriptions()
        .iter()
        .map(|description| description.id.name.as_str())
        .collect();
    assert_eq!(names, ["darktable", "spektrafilm"]);

    let darktable = registry.describe(DARKTABLE_MODULE).expect("peer module");
    let spektrafilm = registry.describe(SPEKTRAFILM_MODULE).expect("peer module");
    assert_eq!(darktable.availability.state, AvailabilityState::Ready);
    assert_eq!(
        spektrafilm.availability.state,
        AvailabilityState::Unavailable
    );
    assert_eq!(
        spektrafilm.availability.refusal_reasons,
        ["spektrafilm runtime is not installed".to_string()]
    );

    // Each schema stays module-owned: the darktable stack never appears
    // in the SpektraFilm description, the film groups never appear in
    // darktable's, and the SpektraFilm groups are exactly its runtime's
    // own manifest groups plus its output options.
    let darktable_groups = darktable.parameter_schema["properties"]
        .as_object()
        .unwrap();
    assert_eq!(
        darktable_groups
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["output", "stack"]
    );
    let mut spektrafilm_groups: Vec<&str> = spektrafilm.parameter_schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut expected_groups = SPEKTRAFILM_GROUPS.to_vec();
    expected_groups.push("output");
    spektrafilm_groups.sort_unstable();
    let mut expected_sorted = expected_groups.clone();
    expected_sorted.sort_unstable();
    assert_eq!(spektrafilm_groups, expected_sorted);

    // The pinned finished-JPEG encoding of the standalone adapter is
    // quality 85, and the declared parameter bounds match the adapters'
    // own envelope bounds.
    assert_eq!(
        spektrafilm.parameter_schema["properties"]["output"]["properties"]["encoding"],
        json!({"const": "quality-85-baseline"})
    );
    assert_eq!(darktable.limits.max_parameter_bytes, MODULE_PARAMETER_BYTES);
    assert_eq!(spektrafilm.limits.max_parameter_bytes, FILM_PARAMETER_BYTES);

    // Availability is not derived from the peer: swapping the states
    // flips exactly the two records.
    let swapped = ModuleRegistry::new(
        ModuleAvailability::unavailable("darktable binary is not installed"),
        ModuleAvailability::ready(),
    );
    assert_eq!(
        swapped
            .describe(DARKTABLE_MODULE)
            .unwrap()
            .availability
            .state,
        AvailabilityState::Unavailable
    );
    assert_eq!(
        swapped
            .describe(SPEKTRAFILM_MODULE)
            .unwrap()
            .availability
            .state,
        AvailabilityState::Ready
    );

    // The same split holds for execution: with otherwise valid input and
    // parameters, the unavailable peer refuses before any engine work
    // while the ready peer executes through the runner.
    let mut runner = RecordingRunner::producing(finished_output(9504, 6336));
    let refused = registry
        .run(
            SPEKTRAFILM_MODULE,
            &linear_tiff_input(),
            &spektrafilm_parameters(json!({"camera": {}})),
            &mut runner,
        )
        .unwrap_err();
    assert_eq!(refused.code, ModuleErrorCode::ModuleUnavailable);
    assert!(refused.message.contains("spektrafilm runtime"));
    assert_eq!(runner.executed(), 0);
    let mut developing = RecordingRunner::producing(development_output());
    assert!(
        registry
            .run(
                DARKTABLE_MODULE,
                &arw_original_input(),
                &darktable_parameters(worker_pinned_tree()),
                &mut developing,
            )
            .is_ok()
    );
    assert_eq!(developing.executed(), 1);
}

#[test]
fn run_refuses_unknown_modules_and_parameter_versions() {
    let registry = both_ready();
    let mut runner = RecordingRunner::producing(development_output());

    let unknown = registry
        .run(
            "capture-one",
            &arw_original_input(),
            &darktable_parameters(json!({"stack": []})),
            &mut runner,
        )
        .unwrap_err();
    assert_eq!(unknown.code, ModuleErrorCode::UnknownModule);
    assert!(unknown.message.contains("capture-one"));
    assert_eq!(
        registry.describe("capture-one").unwrap_err().code,
        ModuleErrorCode::UnknownModule
    );

    let mut future = darktable_parameters(json!({"stack": []}));
    future.version = "darktable-params-2".into();
    assert_eq!(
        registry
            .run(
                DARKTABLE_MODULE,
                &arw_original_input(),
                &future,
                &mut runner
            )
            .unwrap_err()
            .code,
        ModuleErrorCode::UnsupportedParameterVersion
    );

    // Generic unversioned trees are refused too; versions are explicit
    // module-owned strings.
    let mut bare = darktable_parameters(json!({"stack": []}));
    bare.version = "1".into();
    assert_eq!(
        registry
            .run(DARKTABLE_MODULE, &arw_original_input(), &bare, &mut runner)
            .unwrap_err()
            .code,
        ModuleErrorCode::UnsupportedParameterVersion
    );

    // A tree owned by the other module never runs through this module.
    let mut wrong_owner = darktable_parameters(json!({"stack": []}));
    wrong_owner.module = SPEKTRAFILM_MODULE.into();
    assert_eq!(
        registry
            .run(
                DARKTABLE_MODULE,
                &arw_original_input(),
                &wrong_owner,
                &mut runner
            )
            .unwrap_err()
            .code,
        ModuleErrorCode::WrongModuleParameters
    );
    assert_eq!(runner.executed(), 0);
}

#[test]
fn run_refuses_incompatible_input_contracts() {
    let registry = both_ready();
    let mut runner = RecordingRunner::producing(finished_output(6000, 4000));

    // SpektraFilm admits the linear ProPhoto TIFF handoff, not a RAW
    // original.
    assert_eq!(
        registry
            .run(
                SPEKTRAFILM_MODULE,
                &arw_original_input(),
                &spektrafilm_parameters(pinned_film_tree()),
                &mut runner
            )
            .unwrap_err()
            .code,
        ModuleErrorCode::IncompatibleInput
    );

    // A shared container name is insufficient: a display-referred sRGB
    // TIFF is not the linear 32-bit ProPhoto contract, and no implicit
    // conversion is inserted.
    let mut display_tiff = linear_tiff_input();
    display_tiff.contract.color_space = "srgb".into();
    display_tiff.contract.transfer_function = "srgb".into();
    display_tiff.contract.precision_bits = 8;
    assert_eq!(
        registry
            .run(
                SPEKTRAFILM_MODULE,
                &display_tiff,
                &spektrafilm_parameters(pinned_film_tree()),
                &mut runner
            )
            .unwrap_err()
            .code,
        ModuleErrorCode::IncompatibleInput
    );

    // The handoff class is bounded: a frame beyond the film workspace
    // edge bound is refused even with the right container contract.
    assert_eq!(
        registry
            .run(
                SPEKTRAFILM_MODULE,
                &handoff_input(9569, 4000),
                &spektrafilm_parameters(pinned_film_tree()),
                &mut runner
            )
            .unwrap_err()
            .code,
        ModuleErrorCode::IncompatibleInput
    );

    // A smaller qualified frame of the same handoff class — and the
    // real portrait development handoff the darktable peer exports —
    // admit and execute under the pinned recipe.
    let smaller = handoff_input(6000, 4000);
    let result = registry
        .run(
            SPEKTRAFILM_MODULE,
            &smaller,
            &spektrafilm_parameters(pinned_film_tree()),
            &mut runner,
        )
        .expect("qualified handoff frame");
    assert_eq!(result.output.contract.width, 6000);
    let portrait = handoff_input(6376, 9568);
    runner.outcome = Ok(finished_output(6376, 9568));
    let result = registry
        .run(
            SPEKTRAFILM_MODULE,
            &portrait,
            &spektrafilm_parameters(pinned_film_tree()),
            &mut runner,
        )
        .expect("the real portrait development handoff frame");
    assert_eq!(result.output.contract.width, 6376);
    assert_eq!(result.output.contract.height, 9568);

    // darktable in turn refuses the developed TIFF: the peers admit
    // disjoint inputs and neither substitutes for the other.
    assert_eq!(
        registry
            .run(
                DARKTABLE_MODULE,
                &linear_tiff_input(),
                &darktable_parameters(worker_pinned_tree()),
                &mut runner
            )
            .unwrap_err()
            .code,
        ModuleErrorCode::IncompatibleInput
    );
    assert_eq!(runner.executed(), 2);
}

#[test]
fn film_discovery_defaults_execute_and_portrait_contract_uses_same_admission() {
    let registry = both_ready();
    let description = registry.describe(SPEKTRAFILM_MODULE).unwrap();
    let tree = description.parameter_schema["default"].clone();
    assert_eq!(tree, pinned_film_tree());
    registry
        .validate_saved_parameters(&spektrafilm_parameters(tree.clone()))
        .unwrap();
    let portrait = handoff_input(6376, 9568);
    registry
        .admit_input_contract(SPEKTRAFILM_MODULE, &portrait.contract)
        .unwrap();
    let invocation = registry
        .admit(SPEKTRAFILM_MODULE, &portrait, &spektrafilm_parameters(tree))
        .unwrap();
    assert_eq!(
        invocation.limits.deadline_millis,
        SPEKTRAFILM_DEADLINE_MILLIS
    );
    let mut wrong_color = portrait.contract.clone();
    wrong_color.color_space = "srgb".into();
    assert_eq!(
        registry
            .admit_input_contract(SPEKTRAFILM_MODULE, &wrong_color)
            .unwrap_err()
            .code,
        ModuleErrorCode::IncompatibleInput
    );
}

#[test]
fn run_refuses_malformed_trees_before_execution() {
    let registry = both_ready();
    let mut runner = RecordingRunner::producing(development_output());

    let malformed_darktable_trees = [
        json!("exposure"),
        json!({"funnel": {}}),
        json!({"output": "tiff"}),
        json!({"output": {"precisionBits": 32, "colorSpace": "prophoto-rgb", "transferFunction": "linear"}}),
        json!({"output": {"format": 7, "precisionBits": 32, "colorSpace": "prophoto-rgb", "transferFunction": "linear"}}),
        json!({"stack": "exposure"}),
        json!({"output": {"format": "tiff", "precisionBits": 32, "colorSpace": "prophoto-rgb", "transferFunction": "linear", "encoding": "not an identifier"}}),
        json!({"stack": [{"operation": "exposure", "enabled": true, "params": {}}]}),
        json!({"stack": [{"operation": "EXPOSURE", "multiPriority": 0, "enabled": true, "params": {}}]}),
        json!({"stack": [{"operation": "exposure", "multiPriority": 0.5, "enabled": true, "params": {}}]}),
        json!({"stack": [{"operation": "exposure", "multiPriority": 0, "enabled": "yes", "params": {}}]}),
        json!({"stack": [{"operation": "exposure", "multiPriority": 0, "enabled": true, "params": [], "before": 3}]}),
        json!({"stack": [{"operation": "exposure", "multiPriority": 0, "enabled": true, "params": {}, "blend": "screen"}]}),
    ];
    for tree in malformed_darktable_trees {
        let error = registry
            .run(
                DARKTABLE_MODULE,
                &arw_original_input(),
                &darktable_parameters(tree),
                &mut runner,
            )
            .unwrap_err();
        assert_eq!(
            error.code,
            ModuleErrorCode::MalformedParameterTree,
            "{error:?}"
        );
        assert!(error.message.contains("darktable"), "{error:?}");
    }

    // The ordered stack is bounded per operation like the worker's own
    // bound. A 129-entry stack no longer fits the tighter 8 KiB
    // envelope bound the worker pins, so the entry bound is checked on
    // the module-owned tree validator itself.
    let oversize_stack = json!({"stack": (0..129).map(|_| {
            json!({"operation": "a", "multiPriority": 0, "enabled": true, "params": {}})
        }).collect::<Vec<_>>()});
    let error = validate_darktable_tree(DARKTABLE_MODULE, &oversize_stack).unwrap_err();
    assert_eq!(error.code, ModuleErrorCode::MalformedParameterTree);
    let maximal_stack = json!({"stack": (0..128).map(|_| {
            json!({"operation": "a", "multiPriority": 0, "enabled": true, "params": {}})
        }).collect::<Vec<_>>()});
    assert!(validate_darktable_tree(DARKTABLE_MODULE, &maximal_stack).is_ok());

    // The standalone module refuses trees that are not its runtime's
    // complete grouped shape, including any darktable stack entry
    // smuggled in and any missing pinned group.
    let mut missing_group = pinned_film_tree();
    assert!(
        missing_group
            .as_object_mut()
            .unwrap()
            .remove("enlarger")
            .is_some()
    );
    let malformed_spektrafilm_trees = [
        json!([]),
        json!({"stack": []}),
        json!({"film_render": {}}),
        json!({"camera": 7}),
        json!({"output": "jpeg"}),
        json!({"output": {"format": "jpeg", "colorSpace": "srgb", "transferFunction": "srgb"}}),
        missing_group,
    ];
    for tree in malformed_spektrafilm_trees {
        let error = registry
            .run(
                SPEKTRAFILM_MODULE,
                &linear_tiff_input(),
                &spektrafilm_parameters(tree),
                &mut runner,
            )
            .unwrap_err();
        assert_eq!(
            error.code,
            ModuleErrorCode::MalformedParameterTree,
            "{error:?}"
        );
        assert!(error.message.contains("spektrafilm"), "{error:?}");
    }

    // Nothing above ever reached the engine.
    assert_eq!(runner.executed(), 0);
}

#[test]
fn run_refuses_unsupported_controls_before_execution() {
    let registry = both_ready();
    let mut runner = RecordingRunner::producing(development_output());

    // A well-formed darktable output request that is not the pinned
    // development handoff is an unsupported control, not a conversion.
    let unsupported_darktable_trees = [
        json!({"output": {"format": "jpeg", "precisionBits": 32, "colorSpace": "prophoto-rgb", "transferFunction": "linear"}}),
        json!({"output": {"format": "tiff", "precisionBits": 16, "colorSpace": "prophoto-rgb", "transferFunction": "linear"}}),
        json!({"output": {"format": "tiff", "precisionBits": 32, "colorSpace": "prophoto-rgb", "transferFunction": "linear", "geometry": "free-resize"}}),
    ];
    for tree in unsupported_darktable_trees {
        let error = registry
            .run(
                DARKTABLE_MODULE,
                &arw_original_input(),
                &darktable_parameters(tree),
                &mut runner,
            )
            .unwrap_err();
        assert_eq!(error.code, ModuleErrorCode::UnsupportedControl, "{error:?}");
    }

    // The standalone film stage executes one pinned fixed recipe and
    // pins its finished JPEG: any changed group value, quality,
    // geometry, or encoding is refused, never negotiated.
    let mut changed_control = pinned_film_tree();
    changed_control["camera"]["exposure_compensation_ev"] = json!(0.5);
    let mut empty_group = pinned_film_tree();
    empty_group["settings"] = json!({});
    let unsupported_spektrafilm_trees = [
        changed_control,
        empty_group,
        pinned_film_tree_with_output(
            json!({"format": "tiff", "precisionBits": 8, "colorSpace": "srgb", "transferFunction": "srgb"}),
        ),
        pinned_film_tree_with_output(
            json!({"format": "jpeg", "precisionBits": 16, "colorSpace": "srgb", "transferFunction": "srgb"}),
        ),
        pinned_film_tree_with_output(
            json!({"format": "jpeg", "precisionBits": 8, "colorSpace": "srgb", "transferFunction": "srgb", "encoding": "quality-90-baseline"}),
        ),
        pinned_film_tree_with_output(
            json!({"format": "jpeg", "precisionBits": 8, "colorSpace": "srgb", "transferFunction": "srgb", "geometry": "upscale-2x"}),
        ),
    ];
    for tree in unsupported_spektrafilm_trees {
        let error = registry
            .run(
                SPEKTRAFILM_MODULE,
                &linear_tiff_input(),
                &spektrafilm_parameters(tree),
                &mut runner,
            )
            .unwrap_err();
        assert_eq!(error.code, ModuleErrorCode::UnsupportedControl, "{error:?}");
        assert!(error.message.contains("not the pinned"), "{error:?}");
    }
    assert_eq!(runner.executed(), 0);
}

/// The pinned recipe groups carrying one caller-supplied output
/// object, so output-pinning refusals are exercised against the
/// complete executable shape.
fn pinned_film_tree_with_output(output: Value) -> Value {
    let mut tree = pinned_film_tree();
    tree["output"] = output;
    tree
}

#[test]
fn run_enforces_finite_input_parameter_and_output_bounds() {
    let registry = both_ready();
    let mut runner = RecordingRunner::producing(development_output());

    let mut huge_input = arw_original_input();
    huge_input.byte_size = 512 * 1024 * 1024 + 1;
    assert_eq!(
        registry
            .run(
                DARKTABLE_MODULE,
                &huge_input,
                &darktable_parameters(worker_pinned_tree()),
                &mut runner
            )
            .unwrap_err()
            .code,
        ModuleErrorCode::InputTooLarge
    );

    // The parameter envelope bound is the native worker's own 8 KiB
    // module-parameter bound.
    let oversized = darktable_parameters(json!({
        "stack": [],
        "padding": "x".repeat(MODULE_PARAMETER_BYTES)
    }));
    let error = registry
        .run(
            DARKTABLE_MODULE,
            &arw_original_input(),
            &oversized,
            &mut runner,
        )
        .unwrap_err();
    assert_eq!(error.code, ModuleErrorCode::ParameterTreeTooLarge);
    assert!(error.message.contains("above the 8192-byte bound"));

    // The film handoff class bound is the film workspace geometry rule
    // itself: the square 9000×9000 frame and the real 6376×9568
    // portrait handoff both stay inside the declared output pixel
    // bound — the largest frame those geometry bounds admit — while a
    // frame beyond the workspace edge is refused by the class rule.
    let square = handoff_input(9000, 9000);
    assert!(
        registry
            .admit(
                SPEKTRAFILM_MODULE,
                &square,
                &spektrafilm_parameters(pinned_film_tree()),
            )
            .is_ok()
    );
    let portrait = handoff_input(6376, 9568);
    assert!(
        registry
            .admit(
                SPEKTRAFILM_MODULE,
                &portrait,
                &spektrafilm_parameters(pinned_film_tree()),
            )
            .is_ok()
    );
    assert!(
        square.contract.width * square.contract.height
            <= registry
                .describe(SPEKTRAFILM_MODULE)
                .unwrap()
                .limits
                .max_output_pixels
    );
    let beyond = handoff_input(9569, 9569);
    assert_eq!(
        registry
            .admit(
                SPEKTRAFILM_MODULE,
                &beyond,
                &spektrafilm_parameters(pinned_film_tree()),
            )
            .unwrap_err()
            .code,
        ModuleErrorCode::IncompatibleInput
    );
    assert_eq!(runner.executed(), 0);
}

#[test]
fn run_executes_through_the_runner_seam_with_actual_output_facts() {
    let registry = both_ready();
    let input = arw_original_input();
    let parameters = darktable_parameters(worker_pinned_tree());
    let mut runner = RecordingRunner::producing(development_output());

    let first = registry
        .run(DARKTABLE_MODULE, &input, &parameters, &mut runner)
        .expect("admitted invocation");
    let repeated = registry
        .run(DARKTABLE_MODULE, &input, &parameters, &mut runner)
        .expect("admitted invocation");
    assert_eq!(first.result_id, repeated.result_id);
    assert_eq!(first.module.name, DARKTABLE_MODULE);
    assert_eq!(first.module.adapter_version, DARKTABLE_ADAPTER_VERSION);
    assert_eq!(first.parameter_version, DARKTABLE_PARAMETER_VERSION);
    assert_eq!(first.input, input);

    // The result reports the actual facts of the completed private
    // output, not an identity claim.
    assert_eq!(first.output, development_output());
    assert_eq!(first.output, repeated.output);

    // The runner executed the frozen invocation verbatim: the same
    // private identity, the frozen parameter snapshot, the confined
    // input, and the module's finite limits.
    assert_eq!(runner.executed(), 2);
    for invocation in &runner.invocations {
        assert_eq!(invocation.result_id, first.result_id);
        assert_eq!(invocation.module.name, DARKTABLE_MODULE);
        assert_eq!(invocation.parameter_version, DARKTABLE_PARAMETER_VERSION);
        assert_eq!(invocation.parameter_digest, first.parameter_digest);
        assert_eq!(invocation.parameters, parameters);
        assert_eq!(invocation.input, input);
        assert_eq!(
            invocation.limits.max_parameter_bytes,
            MODULE_PARAMETER_BYTES
        );
        assert_eq!(invocation.limits.deadline_millis, 180_000);
    }

    // A changed parameter snapshot is a different private identity.
    let mut changed = worker_pinned_tree();
    changed["stack"][0]["params"]["black"] = json!(0.001);
    let other = registry
        .run(
            DARKTABLE_MODULE,
            &input,
            &darktable_parameters(changed),
            &mut runner,
        )
        .expect("admitted invocation");
    assert_ne!(other.result_id, first.result_id);

    // A different input binding is a different private identity too.
    let mut rebound = input.clone();
    rebound.digest = "3".repeat(64);
    let rebound = registry
        .run(DARKTABLE_MODULE, &rebound, &parameters, &mut runner)
        .expect("admitted invocation");
    assert_ne!(rebound.result_id, first.result_id);

    // Admission alone freezes the same identity without executing or
    // claiming any completed output.
    let admitted = registry
        .admit(DARKTABLE_MODULE, &input, &parameters)
        .expect("admitted invocation");
    assert_eq!(admitted.result_id, first.result_id);
    assert_eq!(
        admitted.parameter_digest,
        digest(&serde_json::to_vec(&parameters.tree).unwrap())
    );
    assert_eq!(runner.executed(), 4);
}

#[test]
fn film_defaults_survive_integral_json_number_roundtrip() {
    fn integers(value: &mut Value) {
        match value {
            Value::Number(number) => {
                if let Some(float) = number.as_f64()
                    && float.fract() == 0.0
                {
                    *value = json!(float as i64);
                }
            }
            Value::Array(values) => values.iter_mut().for_each(integers),
            Value::Object(values) => values.values_mut().for_each(integers),
            _ => {}
        }
    }
    let mut tree = pinned_film_tree();
    integers(&mut tree);
    assert!(validate_spektrafilm_parameters(&spektrafilm_parameters(tree.clone())).is_ok());
    tree["camera"]["exposure_compensation_ev"] = json!(1);
    assert!(validate_spektrafilm_parameters(&spektrafilm_parameters(tree)).is_err());
}

#[test]
fn run_reports_runner_failures_and_unadmitted_output_facts() {
    let registry = both_ready();
    let parameters = spektrafilm_parameters(pinned_film_tree());

    // A runner failure is a structured execution failure; no fallback
    // result is ever formed.
    let mut failing = RecordingRunner::failing("the film engine could not allocate its workspace");
    let error = registry
        .run(
            SPEKTRAFILM_MODULE,
            &linear_tiff_input(),
            &parameters,
            &mut failing,
        )
        .unwrap_err();
    assert_eq!(error.code, ModuleErrorCode::ExecutionFailed);
    assert!(error.message.contains("workspace"), "{error:?}");
    assert_eq!(failing.executed(), 1);

    // Reported output facts are re-validated against the module's
    // pinned output before a result exists.
    let mut wrong_contract = RecordingRunner::producing(development_output());
    let error = registry
        .run(
            SPEKTRAFILM_MODULE,
            &linear_tiff_input(),
            &parameters,
            &mut wrong_contract,
        )
        .unwrap_err();
    assert_eq!(error.code, ModuleErrorCode::ExecutionFailed);
    assert!(error.message.contains("output contract"), "{error:?}");

    let mut resized = RecordingRunner::producing(finished_output(4000, 3000));
    let error = registry
        .run(
            SPEKTRAFILM_MODULE,
            &linear_tiff_input(),
            &parameters,
            &mut resized,
        )
        .unwrap_err();
    assert_eq!(error.code, ModuleErrorCode::ExecutionFailed);
    assert!(error.message.contains("output contract"), "{error:?}");

    let mut bad_digest = RecordingRunner::producing(ModuleOutput {
        digest: "NOT-A-DIGEST".into(),
        byte_size: 1024,
        contract: srgb_jpeg_contract(),
    });
    let error = registry
        .run(
            SPEKTRAFILM_MODULE,
            &linear_tiff_input(),
            &parameters,
            &mut bad_digest,
        )
        .unwrap_err();
    assert_eq!(error.code, ModuleErrorCode::ExecutionFailed);
    assert!(error.message.contains("digest"), "{error:?}");

    let mut empty_output = RecordingRunner::producing(ModuleOutput {
        digest: "c".repeat(64),
        byte_size: 0,
        contract: srgb_jpeg_contract(),
    });
    let error = registry
        .run(
            SPEKTRAFILM_MODULE,
            &linear_tiff_input(),
            &parameters,
            &mut empty_output,
        )
        .unwrap_err();
    assert_eq!(error.code, ModuleErrorCode::ExecutionFailed);
    assert!(error.message.contains("byte length"), "{error:?}");
}

#[test]
fn contract_envelopes_reject_unknown_fields() {
    let input = serde_json::to_string(&arw_original_input()).unwrap();
    let extended = input.replace("\"byteSize\"", "\"unexpected\":1,\"byteSize\"");
    assert!(serde_json::from_str::<ConfinedInput>(&extended).is_err());

    let parameters = serde_json::to_string(&darktable_parameters(json!({"stack": []}))).unwrap();
    let extended = parameters.replace("\"tree\"", "\"unexpected\":1,\"tree\"");
    assert!(serde_json::from_str::<Parameters>(&extended).is_err());

    // The module-owned tree itself keeps every field the adapter wrote;
    // only the strict envelopes above reject unknown fields.
    assert!(serde_json::from_str::<Parameters>(&parameters).is_ok());

    let output = serde_json::to_string(&development_output()).unwrap();
    let extended = output.replace("\"byteSize\"", "\"unexpected\":1,\"byteSize\"");
    assert!(serde_json::from_str::<ModuleOutput>(&extended).is_err());
}

#[test]
fn engine_catalog_is_bounded_and_marks_unqualified_modules() {
    let registry = both_ready();
    let darktable = registry.describe(DARKTABLE_MODULE).unwrap();
    let exposure = darktable
        .engine_modules
        .iter()
        .find(|module| module.name == "exposure")
        .unwrap();
    let ev = exposure
        .controls
        .iter()
        .find(|control| control.name == "ev")
        .unwrap();
    assert!(ev.readable && ev.editable && ev.executable);
    assert_eq!(ev.default, json!(0.0));
    assert_eq!(ev.reset, json!(0.0));
    assert_eq!(ev.schema["minimum"], json!(0.0));
    assert_eq!(ev.schema["maximum"], json!(1.0));
    let white_balance = darktable
        .engine_modules
        .iter()
        .find(|module| module.name == "white-balance")
        .unwrap();
    assert!(!white_balance.executable);
    assert!(white_balance.controls.is_empty());
    assert!(white_balance.refusal_reason.is_some());

    let film = registry.describe(SPEKTRAFILM_MODULE).unwrap();
    assert!(film.engine_modules.iter().all(|module| !module.executable));
    assert!(
        film.engine_modules
            .iter()
            .all(|module| module.controls.is_empty())
    );
}

#[test]
fn darktable_canonical_exposure_can_set_and_reset() {
    let tree = apply_darktable_control(None, "darktable.exposure.ev", &json!(0.5)).unwrap();
    assert_eq!(tree["stack"][0]["params"]["exposure"], json!(0.5));
    let reset = reset_darktable_control(Some(&tree), "darktable.exposure.ev").unwrap();
    assert_eq!(reset["stack"][0]["params"]["exposure"], json!(0.0));
    assert!(validate_darktable_tree(DARKTABLE_MODULE, &reset).is_ok());
}

#[test]
fn darktable_canonical_exposure_rejects_bounds_and_unknown_controls() {
    for value in [json!(-0.01), json!(1.01)] {
        let error = apply_darktable_control(None, "darktable.exposure.ev", &value).unwrap_err();
        assert_eq!(error.code, ModuleErrorCode::UnsupportedControl);
    }
    let error =
        apply_darktable_control(None, "darktable.exposure.unknown", &json!(0.5)).unwrap_err();
    assert_eq!(error.code, ModuleErrorCode::UnsupportedControl);
}
