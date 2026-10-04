use super::*;
#[test]
fn discoverable_manual_defaults_form_an_admitted_baseline() {
    let registry = both_ready();
    let schema = &registry
        .describe(DARKTABLE_MODULE)
        .unwrap()
        .parameter_schema;
    let defaults = &schema["default"];
    assert_eq!(
        defaults["stack"][0]["params"],
        crate::native_development::manual_exposure_parameters(0.0)
    );
    let parameters = darktable_parameters(defaults.clone());
    registry.validate_saved_parameters(&parameters).unwrap();
    assert_eq!(
        schema["x-automatic-adjustments"].as_array().map(Vec::len),
        Some(2)
    );
    let mut runner = RecordingRunner::producing(development_output());
    registry
        .run(
            DARKTABLE_MODULE,
            &arw_original_input(),
            &parameters,
            &mut runner,
        )
        .unwrap();
    assert_eq!(runner.invocations[0].parameters.tree, *defaults);
}

#[test]
fn saved_white_balance_intent_survives_unavailable_engine_but_never_executes() {
    let tree = json!({"stack": [{"operation": "temperature", "multiPriority": 0,
        "enabled": true, "params": {"temperatureKelvin": 6500, "tintMilli": -1200}}]});
    let parameters = darktable_parameters(tree.clone());
    let unavailable = ModuleRegistry::new(
        ModuleAvailability::unavailable("not installed"),
        ModuleAvailability::ready(),
    );
    unavailable.validate_saved_parameters(&parameters).unwrap();
    let mut runner = RecordingRunner::producing(development_output());
    let error = both_ready()
        .run(
            DARKTABLE_MODULE,
            &arw_original_input(),
            &parameters,
            &mut runner,
        )
        .unwrap_err();
    assert_eq!(error.code, ModuleErrorCode::UnsupportedControl);
    assert_eq!(runner.executed(), 0);
    assert_eq!(parameters.tree, tree);
}

#[test]
fn saved_validation_checks_later_fields_before_unsupported_qualification() {
    let registry = both_ready();
    let parameters = darktable_parameters(json!({
        "stack": [
            {"operation": "temperature", "multiPriority": 0, "enabled": true,
             "params": {"temperatureKelvin": 6500, "tintMilli": 0}},
            {"operation": "exposure", "multiPriority": 0, "enabled": true,
             "params": {"exposure": "bad"}}
        ],
        "output": {"format": "jpeg", "precisionBits": 8, "colorSpace": "srgb", "transferFunction": "srgb"}
    }));
    assert_eq!(
        registry
            .validate_saved_parameters(&parameters)
            .unwrap_err()
            .code,
        ModuleErrorCode::MalformedParameterTree
    );
    let mut valid = parameters;
    valid.tree["stack"][1]["params"]["exposure"] = json!(0.5);
    registry.validate_saved_parameters(&valid).unwrap();
    assert_eq!(
        registry.validate_parameters(&valid).unwrap_err().code,
        ModuleErrorCode::UnsupportedControl
    );
}

#[test]
fn retained_white_balance_requires_both_bounded_integer_components() {
    let registry = both_ready();
    for params in [
        json!({"temperatureKelvin": 6500}),
        json!({"temperatureKelvin": 6500, "tintMilli": 0.5}),
        json!({"temperatureKelvin": 999, "tintMilli": 0}),
        json!({"temperatureKelvin": 6500, "tintMilli": 150001}),
    ] {
        let parameters = darktable_parameters(json!({"stack": [{"operation": "temperature",
            "multiPriority": 0, "enabled": true, "params": params}]}));
        assert_eq!(
            registry
                .validate_saved_parameters(&parameters)
                .unwrap_err()
                .code,
            ModuleErrorCode::MalformedParameterTree
        );
    }
}

#[test]
fn automatic_modes_are_saved_but_refused_until_concrete_capture() {
    let registry = both_ready();
    for (operation, params) in [
        (
            "exposure",
            json!({
                "mode": "EXPOSURE_MODE_DEFLICKER",
                "exposure": 0.0,
                "deflicker_percentile": 50.0,
                "deflicker_target_level": -4.0
            }),
        ),
        (
            "channelmixerrgb",
            json!({
                "illuminant": "DT_ILLUMINANT_DETECT_EDGES",
                "adaptation": "DT_ADAPTATION_CAT16",
                "x": 0.333,
                "y": 0.333,
                "temperature": 5003.0
            }),
        ),
    ] {
        let parameters = darktable_parameters(json!({
            "stack": [{
                "operation": operation,
                "multiPriority": 0,
                "enabled": true,
                "params": params
            }]
        }));
        registry.validate_saved_parameters(&parameters).unwrap();
        assert_eq!(
            registry.validate_parameters(&parameters).unwrap_err().code,
            ModuleErrorCode::UnsupportedControl
        );
        let mut runner = RecordingRunner::producing(development_output());
        assert_eq!(
            registry
                .run(
                    DARKTABLE_MODULE,
                    &arw_original_input(),
                    &parameters,
                    &mut runner,
                )
                .unwrap_err()
                .code,
            ModuleErrorCode::UnsupportedControl
        );
        assert_eq!(runner.executed(), 0);
    }
}
