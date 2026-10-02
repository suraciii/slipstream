use super::*;

fn save_document(guard: Value, exposure: Value, white_balance: Value) -> Vec<u8> {
    json!({
        "requestId": "edit-001",
        "expectedRecipeVersion": guard,
        "expectedSourceRevision": "observed-source-revision",
        "settings": {
            "exposureEv": exposure,
            "whiteBalance": white_balance,
        },
    })
    .to_string()
    .into_bytes()
}

fn failure_argument(failure: &CommandFailure) -> String {
    failure.payload.details["argument"]
        .as_str()
        .expect("input failures name an argument")
        .to_owned()
}

fn refuses(failure: CommandFailure, argument: &str) {
    assert_eq!(failure.exit_code, 2, "for {failure:?}");
    assert_eq!(failure.payload.code, "invalid_input");
    assert_eq!(failure_argument(&failure), argument);
}

fn origin() -> Url {
    Url::parse("https://slipstream.example").expect("origin parses")
}

fn read_wire(document: Value) -> RecipeReadWire {
    serde_json::from_value(document).expect("fixture decodes")
}

fn identity(operation: Operation) -> MutationIdentity {
    MutationIdentity {
        operation,
        photo_ids: vec!["p1".to_owned()],
        album_id: None,
        album_name: None,
        mappings: Vec::new(),
    }
}

// ------------------------------------------------------------ save input

#[test]
fn save_input_accepts_the_documented_shape_with_an_explicit_null_guard() {
    let body = parse_save(save_document(
        Value::Null,
        json!(1.0),
        json!({ "mode": "as-shot" }),
    ))
    .expect("documented save input");
    assert_eq!(
        body,
        json!({
            "requestId": "edit-001",
            "expectedRecipeVersion": Value::Null,
            "expectedSourceRevision": "observed-source-revision",
            "settings": {
                "exposureEv": 1.0,
                "whiteBalance": { "mode": "as-shot" },
            },
        })
    );
}

#[test]
fn save_input_accepts_an_explicit_string_guard_and_payload_bounds() {
    let body = parse_save(
        json!({
            "requestId": "edit-002",
            "expectedRecipeVersion": "recipe-7",
            "expectedSourceRevision": "source-3",
            "settings": {
                "exposureEv": 0.25,
                "whiteBalance": {
                    "mode": "temperature-tint",
                    "temperatureKelvin": 40_000,
                    "tintMilli": -150_000,
                },
            },
        })
        .to_string()
        .into_bytes(),
    )
    .expect("explicit guard save input");
    assert_eq!(body["expectedRecipeVersion"], "recipe-7");
    assert_eq!(
        body["settings"]["whiteBalance"],
        json!({
            "mode": "temperature-tint",
            "temperatureKelvin": 40_000,
            "tintMilli": -150_000,
        })
    );
    assert_eq!(body.as_object().expect("object").len(), 4);
}

#[test]
fn save_input_refuses_an_omitted_nullable_guard_but_not_the_explicit_null() {
    let omitted = br#"{
        "requestId": "edit-001",
        "expectedSourceRevision": "observed-source-revision",
        "settings": {"exposureEv": 0.0, "whiteBalance": {"mode": "as-shot"}}
    }"#;
    refuses(parse_save(omitted.to_vec()).unwrap_err(), "input");
    parse_save(save_document(
        Value::Null,
        json!(0.0),
        json!({ "mode": "as-shot" }),
    ))
    .expect("the explicit null guard is valid");
}

#[test]
fn save_input_refuses_malformed_documents() {
    for invalid in [
        Vec::new(),
        b"{".to_vec(),
        b"[]".to_vec(),
        br#""edit-001""#.to_vec(),
        br#"{"requestId":"a","expectedRecipeVersion":null,"expectedSourceRevision":"s"}"#.to_vec(),
        br#"{"requestId":"a","expectedRecipeVersion":null,"expectedSourceRevision":"s",
             "settings":{"exposureEv":1.0,"whiteBalance":{"mode":"as-shot"}}} trailing"#
            .to_vec(),
        br#"{"requestId":"a","requestId":"b","expectedRecipeVersion":null,
             "expectedSourceRevision":"s","settings":{"exposureEv":1.0,
             "whiteBalance":{"mode":"as-shot"}}}"#
            .to_vec(),
        br#"{"requestId":"a","expectedRecipeVersion":null,"expectedSourceRevision":"s",
             "extra":1,"settings":{"exposureEv":1.0,"whiteBalance":{"mode":"as-shot"}}}"#
            .to_vec(),
        br#"{"requestId":"a","expectedRecipeVersion":null,"expectedSourceRevision":"s",
             "settings":{"exposureEv":1.0,"whiteBalance":{"mode":"as-shot"},"extra":1}}"#
            .to_vec(),
        br#"{"requestId":"a","expectedRecipeVersion":null,"expectedSourceRevision":5,
             "settings":{"exposureEv":1.0,"whiteBalance":{"mode":"as-shot"}}}"#
            .to_vec(),
    ] {
        refuses(parse_save(invalid).unwrap_err(), "input");
    }
}

#[test]
fn save_input_refuses_empty_or_illformed_guards_in_admission_order() {
    let empty_request_id = save_document(Value::Null, json!(1.0), json!({ "mode": "as-shot" }));
    let mut document: serde_json::Value =
        serde_json::from_slice(&empty_request_id).expect("fixture decodes");
    document["requestId"] = json!("edit 001");
    refuses(
        parse_save(document.to_string().into_bytes()).unwrap_err(),
        "requestId",
    );

    for (argument, guard, source) in [
        ("expectedRecipeVersion", json!(""), json!("source-3")),
        ("expectedSourceRevision", json!("recipe-7"), json!("")),
    ] {
        let document = json!({
            "requestId": "edit-001",
            "expectedRecipeVersion": guard,
            "expectedSourceRevision": source,
            "settings": {"exposureEv": 1.0, "whiteBalance": {"mode": "as-shot"}},
        });
        refuses(
            parse_save(document.to_string().into_bytes()).unwrap_err(),
            argument,
        );
    }
}

#[test]
fn save_input_refuses_nonfinite_exposure_without_hardcoding_a_range() {
    // serde_json itself refuses numbers outside the f64 range; the
    // module's finite guard covers any number a decoder admits.
    let overflowing = br#"{
        "requestId": "edit-001",
        "expectedRecipeVersion": null,
        "expectedSourceRevision": "observed-source-revision",
        "settings": {"exposureEv": 1e400, "whiteBalance": {"mode": "as-shot"}}
    }"#;
    refuses(parse_save(overflowing.to_vec()).unwrap_err(), "input");
}

#[test]
fn save_input_refuses_white_balance_payloads_outside_the_closed_shapes() {
    for (argument, white_balance) in [
        // In-range integers outside the published payload bounds.
        (
            "whiteBalance",
            json!({"mode": "temperature-tint", "temperatureKelvin": 999, "tintMilli": 0}),
        ),
        (
            "whiteBalance",
            json!({"mode": "temperature-tint", "temperatureKelvin": 40_001, "tintMilli": 0}),
        ),
        (
            "whiteBalance",
            json!({"mode": "temperature-tint", "temperatureKelvin": 6_500, "tintMilli": 150_001}),
        ),
        (
            "whiteBalance",
            json!({"mode": "temperature-tint", "temperatureKelvin": 6_500, "tintMilli": -150_001}),
        ),
        // A mode that requires fields the document omits or nulls out.
        (
            "whiteBalance",
            json!({"mode": "temperature-tint", "temperatureKelvin": 6_500}),
        ),
        // A mode that requires no fields must not carry them.
        (
            "whiteBalance",
            json!({"mode": "as-shot", "temperatureKelvin": 6_500}),
        ),
        ("whiteBalance", json!({"mode": "as-shot", "tintMilli": 0})),
    ] {
        refuses(
            parse_save(save_document(Value::Null, json!(1.0), white_balance)).unwrap_err(),
            argument,
        );
    }
    // Wrong types and unknown modes fail at the decoder.
    for white_balance in [
        json!({"mode": "temperature-tint", "temperatureKelvin": 6.5, "tintMilli": 0}),
        json!({"mode": "custom"}),
        json!("as-shot"),
    ] {
        refuses(
            parse_save(save_document(Value::Null, json!(1.0), white_balance)).unwrap_err(),
            "input",
        );
    }
    // A duplicated key inside whiteBalance is refused by the decoder;
    // the document must be raw text because the json! macro collapses
    // duplicate keys while building the fixture.
    let duplicated = br#"{
        "requestId": "edit-001",
        "expectedRecipeVersion": null,
        "expectedSourceRevision": "observed-source-revision",
        "settings": {"exposureEv": 1.0, "whiteBalance": {
            "mode": "temperature-tint",
            "temperatureKelvin": 6500,
            "temperatureKelvin": 6501,
            "tintMilli": 0
        }}
    }"#;
    refuses(parse_save(duplicated.to_vec()).unwrap_err(), "input");
}

// ---------------------------------------------------------- rebind input

#[test]
fn rebind_input_accepts_only_the_exact_three_key_document() {
    let body = parse_rebind(
        br#"{"requestId":"rebind-1","expectedRecipeVersion":"recipe-7",
             "newSourceRevision":"source-9"}"#
            .to_vec(),
    )
    .expect("documented rebind input");
    assert_eq!(
        body,
        json!({
            "requestId": "rebind-1",
            "expectedRecipeVersion": "recipe-7",
            "newSourceRevision": "source-9",
        })
    );
}

#[test]
fn rebind_input_refuses_incomplete_or_illformed_guards() {
    for (document, argument) in [
        (Vec::new(), "input"),
        (
            br#"{"requestId":"rebind-1","newSourceRevision":"source-9"}"#.to_vec(),
            "input",
        ),
        (
            br#"{"requestId":"rebind-1","expectedRecipeVersion":"r",
                 "expectedRecipeVersion":"r2","newSourceRevision":"s"}"#
                .to_vec(),
            "input",
        ),
        (
            br#"{"requestId":"rebind-1","expectedRecipeVersion":"",
                 "newSourceRevision":"source-9"}"#
                .to_vec(),
            "expectedRecipeVersion",
        ),
        (
            br#"{"requestId":"rebind-1","expectedRecipeVersion":"recipe-7",
                 "newSourceRevision":""}"#
                .to_vec(),
            "newSourceRevision",
        ),
        (
            br#"{"requestId":"rebind 1","expectedRecipeVersion":"recipe-7",
                 "newSourceRevision":"source-9"}"#
                .to_vec(),
            "requestId",
        ),
        (
            br#"{"requestId":"rebind-1","expectedRecipeVersion":"recipe-7",
                 "newSourceRevision":"source-9","extra":1}"#
                .to_vec(),
            "input",
        ),
    ] {
        refuses(parse_rebind(document).unwrap_err(), argument);
    }
}

// ------------------------------------------------------------ read shape

fn read_fixture() -> Value {
    json!({
        "photoId": "p1",
        "sourceRevision": "source-3",
        "recipe": Value::Null,
        "sourceSupport": "supported",
        "supportReason": Value::Null,
        "processingAvailable": false,
        "controls": {
            "exposure": {"minimumEv": -4.0, "maximumEv": 4.0, "stepEv": 0.001},
            "whiteBalanceModes": ["as-shot"],
        },
    })
}

#[test]
fn recipe_read_renders_the_documented_facts_with_the_photo_destination() {
    let value =
        validated_recipe_read(read_wire(read_fixture()), "p1", &origin()).expect("documented read");
    assert_eq!(value["photoId"], "p1");
    assert_eq!(value["sourceRevision"], "source-3");
    assert_eq!(value["recipe"], Value::Null);
    assert_eq!(value["sourceSupport"], "supported");
    assert_eq!(value["supportReason"], Value::Null);
    assert_eq!(value["processingAvailable"], false);
    assert_eq!(
        value["controls"],
        json!({
            "exposure": {"minimumEv": -4.0, "maximumEv": 4.0, "stepEv": 0.001},
            "whiteBalanceModes": ["as-shot"],
        })
    );
    assert_eq!(value["webUrl"], "https://slipstream.example/?photoId=p1");
}

#[test]
fn recipe_read_validates_edit_source_provenance_and_defaults_legacy_reads() {
    let legacy = validated_recipe_read(read_wire(read_fixture()), "p1", &origin())
        .expect("legacy recipe read");
    assert_eq!(legacy["editSource"], "original");
    assert_eq!(legacy["editSourceProxyId"], Value::Null);

    let mut proxy_read = read_fixture();
    proxy_read["editSource"] = json!("development-proxy");
    proxy_read["editSourceProxyId"] = json!("a".repeat(64));
    let proxy = validated_recipe_read(read_wire(proxy_read), "p1", &origin())
        .expect("proxy-backed recipe read");
    assert_eq!(proxy["editSource"], "development-proxy");
    assert_eq!(proxy["editSourceProxyId"], "a".repeat(64));

    for (source, proxy_id) in [
        (json!("development-proxy"), Value::Null),
        (json!("original"), json!("a".repeat(64))),
        (json!("future"), Value::Null),
        (json!("development-proxy"), json!("not-a-digest")),
    ] {
        let mut invalid = read_fixture();
        invalid["editSource"] = source;
        invalid["editSourceProxyId"] = proxy_id;
        assert!(
            validated_recipe_read(read_wire(invalid), "p1", &origin()).is_err(),
            "invalid provenance must be refused"
        );
    }
}

#[test]
fn recipe_read_accepts_an_unavailable_source_with_its_closed_reason() {
    // The confirmed outcomes and the retryable waits are all closed
    // reasons this client believes without downgrading the read.
    for reason in [
        "original-missing",
        "original-unreadable",
        "read-pending",
        "resource-unavailable",
    ] {
        let document = json!({
            "photoId": "p1",
            "sourceRevision": Value::Null,
            "recipe": Value::Null,
            "sourceSupport": "unavailable",
            "supportReason": reason,
            "processingAvailable": false,
            "controls": {
                "exposure": {"minimumEv": -4.0, "maximumEv": 4.0, "stepEv": 0.001},
                "whiteBalanceModes": ["as-shot"],
            },
        });
        let value = validated_recipe_read(read_wire(document), "p1", &origin())
            .unwrap_or_else(|failure| panic!("{reason} is a closed reason: {failure:?}"));
        assert_eq!(value["sourceSupport"], "unavailable");
        assert_eq!(value["supportReason"], json!(reason));
        assert_eq!(value["sourceRevision"], Value::Null);
    }
}

#[test]
fn recipe_read_accepts_retryable_source_evidence() {
    for reason in ["read-pending", "resource-unavailable"] {
        let mut document = read_fixture();
        document["sourceRevision"] = Value::Null;
        document["sourceSupport"] = json!("unavailable");
        document["supportReason"] = json!(reason);
        let value = validated_recipe_read(read_wire(document), "p1", &origin())
            .expect("retryable source evidence is valid");
        assert_eq!(value["supportReason"], reason);
    }
}

#[test]
fn retained_unadmitted_settings_cannot_claim_processing_available() {
    for (exposure, white_balance) in [
        (
            1.0,
            json!({"mode":"temperature-tint", "temperatureKelvin":6500,"tintMilli":0}),
        ),
        (5.0, json!({"mode":"as-shot"})),
        (0.0005, json!({"mode":"as-shot"})),
    ] {
        let mut document = read_fixture();
        document["recipe"] = json!({"recipeVersion":"retained", "exposureEv":exposure, "whiteBalance":white_balance});
        let retained = validated_recipe_read(read_wire(document.clone()), "p1", &origin()).unwrap();
        assert_eq!(retained["recipe"], document["recipe"]);
        assert_eq!(retained["processingAvailable"], false);
        document["processingAvailable"] = json!(true);
        assert_eq!(
            validated_recipe_read(read_wire(document), "p1", &origin())
                .unwrap_err()
                .payload
                .code,
            "transport_failed"
        );
    }
}

#[test]
fn recipe_read_accepts_retained_temperature_tint_inside_the_payload_bounds() {
    let mut document = read_fixture();
    document["recipe"] = json!({
        "recipeVersion": "recipe-7",
        "exposureEv": 1.5,
        "whiteBalance": {
            "mode": "temperature-tint",
            "temperatureKelvin": 40_000,
            "tintMilli": 150_000,
        },
    });
    let value =
        validated_recipe_read(read_wire(document), "p1", &origin()).expect("retained intent");
    assert_eq!(
        value["recipe"]["whiteBalance"],
        json!({
            "mode": "temperature-tint",
            "temperatureKelvin": 40_000,
            "tintMilli": 150_000,
        })
    );
}

#[test]
fn recipe_read_refuses_responses_outside_the_closed_contract() {
    let invalid = |document: Value| {
        let failure = validated_recipe_read(read_wire(document), "p1", &origin()).unwrap_err();
        assert_eq!(failure.exit_code, 6, "for {failure:?}");
        assert_eq!(failure.payload.code, "transport_failed");
        assert_eq!(failure.payload.details["operation"], "photos-recipe-get");
    };
    // A read must answer for exactly the requested Photo.
    let mut document = read_fixture();
    document["photoId"] = json!("p2");
    invalid(document);
    // The support state is closed.
    let mut document = read_fixture();
    document["sourceSupport"] = json!("missing");
    invalid(document);
    // sourceRevision is null exactly when sourceSupport is unavailable.
    let mut document = read_fixture();
    document["sourceRevision"] = Value::Null;
    invalid(document);
    let mut document = read_fixture();
    document["sourceRevision"] = json!("");
    invalid(document);
    let mut document = read_fixture();
    document["sourceSupport"] = json!("unavailable");
    invalid(document);
    // supportReason is non-null only with unavailable and carries a
    // closed reason.
    let mut document = read_fixture();
    document["supportReason"] = json!("original-missing");
    invalid(document);
    let mut document = read_fixture();
    document["supportReason"] = json!("read-pending");
    invalid(document);
    let mut document = read_fixture();
    document["sourceSupport"] = json!("unavailable");
    document["sourceRevision"] = Value::Null;
    document["supportReason"] = json!("original-rotated");
    invalid(document);
    let mut document = read_fixture();
    document["sourceSupport"] = json!("unavailable");
    document["sourceRevision"] = Value::Null;
    invalid(document);
    // A retained recipe keeps the shared field shapes.
    let mut document = read_fixture();
    document["recipe"] = json!({
        "recipeVersion": "",
        "exposureEv": 1.0,
        "whiteBalance": { "mode": "as-shot" },
    });
    invalid(document);
    let mut document = read_fixture();
    document["recipe"] = json!({
        "recipeVersion": "recipe-7",
        "exposureEv": 1.0,
        "whiteBalance": {
            "mode": "temperature-tint",
            "temperatureKelvin": 999,
            "tintMilli": 0,
        },
    });
    invalid(document);
    // The controls carry a sane closed range.
    let mut document = read_fixture();
    document["controls"]["exposure"]["stepEv"] = json!(0.0);
    invalid(document);
    let mut document = read_fixture();
    document["controls"]["exposure"] =
        json!({"minimumEv": 4.0, "maximumEv": -4.0, "stepEv": 0.001});
    invalid(document);
    let mut document = read_fixture();
    document["controls"]["whiteBalanceModes"] = json!([]);
    invalid(document);
}

// --------------------------------------------------- write confirmation

#[test]
fn write_confirmation_renders_the_documented_result_for_matching_outcomes() {
    for outcome in ["saved", "unchanged"] {
        let result = RecipeWriteWire {
            outcome: outcome.to_owned(),
            recipe_version: "recipe-8".to_owned(),
            source_revision: "source-3".to_owned(),
        };
        let value = confirmed_recipe_write(
            &identity(SAVE_OPERATION),
            "p1",
            "edit-001",
            "source-3",
            "https://slipstream.example/?photoId=p1".to_owned(),
            result,
        )
        .expect("matching confirmation");
        assert_eq!(
            value,
            json!({
                "photoId": "p1",
                "requestId": "edit-001",
                "outcome": outcome,
                "recipeVersion": "recipe-8",
                "sourceRevision": "source-3",
                "webUrl": "https://slipstream.example/?photoId=p1",
            })
        );
    }
}

#[test]
fn write_confirmation_treats_an_unusable_response_as_an_unknown_outcome() {
    let unknown = |result: RecipeWriteWire, submitted: &str| {
        let failure = confirmed_recipe_write(
            &identity(SAVE_OPERATION),
            "p1",
            "edit-001",
            submitted,
            "https://slipstream.example/?photoId=p1".to_owned(),
            result,
        )
        .unwrap_err();
        assert_eq!(failure.exit_code, 7, "for {failure:?}");
        assert_eq!(failure.payload.code, "outcome_unknown");
        assert_eq!(failure.payload.effect, "unknown");
        assert_eq!(failure.payload.details["operation"], "photos-recipe-save");
        assert_eq!(failure.payload.details["photoIds"], json!(["p1"]));
    };
    unknown(
        RecipeWriteWire {
            outcome: "conflicted".to_owned(),
            recipe_version: "recipe-8".to_owned(),
            source_revision: "source-3".to_owned(),
        },
        "source-3",
    );
    unknown(
        RecipeWriteWire {
            outcome: "saved".to_owned(),
            recipe_version: "".to_owned(),
            source_revision: "source-3".to_owned(),
        },
        "source-3",
    );
    // The returned source must be exactly the submitted guard.
    unknown(
        RecipeWriteWire {
            outcome: "saved".to_owned(),
            recipe_version: "recipe-8".to_owned(),
            source_revision: "source-4".to_owned(),
        },
        "source-3",
    );
}

mod composable;
// ------------------------------------------------------------ capability

mod capability;

#[test]
fn missing_nullable_facts_and_null_white_balance_fields_are_refused() {
    for key in ["sourceRevision", "recipe", "supportReason"] {
        let mut value = read_fixture();
        value.as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<RecipeReadWire>(value).is_err());
    }
    for key in ["temperatureKelvin", "tintMilli"] {
        let mut white_balance = json!({"mode":"as-shot"});
        white_balance[key] = Value::Null;
        assert!(parse_save(save_document(Value::Null, json!(1), white_balance)).is_err());
    }
}
