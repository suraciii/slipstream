use super::*;

fn capability_wire(document: Value) -> CapabilityReportWire {
    serde_json::from_value(document).expect("fixture decodes")
}

fn ready_capability() -> Value {
    json!({
        "state": "ready",
        "bundleId": "c".repeat(64),
        "incarnation": "a".repeat(32),
        "exposure": {"minimumEv": -4.0, "maximumEv": 4.0, "stepEv": 0.001},
        "profiles": [
            {
                "profileId": "sony-a7iv",
                "whiteBalanceModes": ["as-shot"],
                "whiteBalanceRanges": Value::Null,
            },
        ],
        "stages": {"develop": "ready", "film": "ready"},
    })
}

#[test]
fn capability_preserves_the_reported_profiles_and_ranges() {
    let value =
        validated_capability_report(capability_wire(ready_capability())).expect("ready report");
    assert_eq!(value, ready_capability());
    // Every condition that verified no engine bundle reports null
    // identities and keeps the closed profile list and unavailable stages.
    for state in ["disabled", "bundle-unavailable", "resource-unavailable"] {
        let mut unavailable = ready_capability();
        unavailable["state"] = json!(state);
        unavailable["bundleId"] = Value::Null;
        unavailable["incarnation"] = Value::Null;
        unavailable["stages"] = json!({"develop": "unavailable", "film": "unavailable"});
        let value = validated_capability_report(capability_wire(unavailable))
            .unwrap_or_else(|_| panic!("{state} report"));
        assert_eq!(value["state"], json!(state));
        assert_eq!(value["bundleId"], Value::Null);
        assert_eq!(value["incarnation"], Value::Null);
        assert!(!value["profiles"].as_array().unwrap().is_empty());
    }
    // A source-unsupported deployment reports an empty profile list.
    let mut unsupported = ready_capability();
    unsupported["state"] = json!("source-unsupported");
    unsupported["bundleId"] = Value::Null;
    unsupported["incarnation"] = Value::Null;
    unsupported["profiles"] = json!([]);
    unsupported["stages"] = json!({"develop": "unsupported", "film": "unsupported"});
    let value = validated_capability_report(capability_wire(unsupported))
        .expect("source-unsupported report");
    assert_eq!(value["profiles"], json!([]));
}

#[test]
fn capability_refuses_reports_outside_the_closed_contract() {
    let invalid = |document: Value| {
        let failure = validated_capability_report(capability_wire(document)).unwrap_err();
        assert_eq!(failure.exit_code, 6, "for {failure:?}");
        assert_eq!(failure.payload.code, "transport_failed");
        assert_eq!(
            failure.payload.details["operation"],
            "processing-capability"
        );
    };
    let mut document = ready_capability();
    document["state"] = json!("offline");
    invalid(document);
    let mut document = ready_capability();
    document["stages"]["film"] = json!("queued");
    invalid(document);
    let mut document = ready_capability();
    document["exposure"]["stepEv"] = json!(0.0);
    invalid(document);
    let mut document = ready_capability();
    document["exposure"] = json!({"minimumEv": 4.0, "maximumEv": -4.0, "stepEv": 0.001});
    invalid(document);
    let mut document = ready_capability();
    document["bundleId"] = json!("");
    invalid(document);
    let mut document = ready_capability();
    document["profiles"][0]["profileId"] = json!("");
    invalid(document);
    let mut document = ready_capability();
    document["profiles"][0]["whiteBalanceModes"] = json!([]);
    invalid(document);
    let mut document = ready_capability();
    document["profiles"][0]["whiteBalanceModes"] = json!([""]);
    invalid(document);
    let mut document = ready_capability();
    document["bundleId"] = Value::Null;
    invalid(document);
    let mut document = ready_capability();
    document["stages"]["develop"] = json!("unsupported");
    invalid(document);
    let mut document = ready_capability();
    document["profiles"][0]["whiteBalanceRanges"] = json!(42);
    invalid(document);
    // The unavailable conditions keep the closed profile list; a report
    // that empties it outside source-unsupported is outside the contract.
    for state in ["disabled", "bundle-unavailable", "resource-unavailable"] {
        let mut document = ready_capability();
        document["state"] = json!(state);
        document["bundleId"] = Value::Null;
        document["incarnation"] = Value::Null;
        document["stages"] = json!({"develop": "unavailable", "film": "unavailable"});
        document["profiles"] = json!([]);
        invalid(document);
    }
    // The retired launcher condition is no longer a recognized state, and
    // only ready may carry the observed bundle identities.
    let mut document = ready_capability();
    document["state"] = json!("launcher-unavailable");
    document["bundleId"] = Value::Null;
    document["incarnation"] = Value::Null;
    document["stages"] = json!({"develop": "unavailable", "film": "unavailable"});
    invalid(document);
    let mut document = ready_capability();
    document["state"] = json!("bundle-unavailable");
    document["stages"] = json!({"develop": "unavailable", "film": "unavailable"});
    invalid(document);
    let mut document = ready_capability();
    document["incarnation"] = Value::Null;
    invalid(document);
}
