use super::*;
// ------------------------------------------------ composable processing

fn composable_recipe_fixture() -> Value {
    json!({
        "photoId": "p1",
        "revision": "recipe-9",
        "sourceRevision": "source-3",
        "currentStepId": "develop-1",
        "steps": [{
            "stepId": "develop-1",
            "module": "darktable",
            "input": {
                "kind": "original",
                "photoId": "p1",
                "sourceRevision": "source-3",
            },
            "parameters": {
                "schemaVersion": "darktable-params-1",
                "tree": {"stack": [], "output": {
                    "format": "tiff",
                    "precisionBits": 32,
                    "colorSpace": "prophoto-rgb",
                    "transferFunction": "linear"
                }},
            },
        }],
    })
}

fn composable_save_document(steps: Value, current: Value) -> Vec<u8> {
    json!({
        "requestId": "composable-001",
        "expectedRecipeRevision": Value::Null,
        "expectedSourceRevision": "source-3",
        "currentStepId": current,
        "steps": steps,
    })
    .to_string()
    .into_bytes()
}

fn composable_step(step_id: &str, module: &str) -> Value {
    json!({
        "stepId": step_id,
        "module": module,
        "input": {
            "kind": "original",
            "photoId": "p1",
            "sourceRevision": "source-3",
        },
        "parameters": {
            "schemaVersion": "darktable-params-1",
            "tree": {"stack": []},
        },
    })
}

#[test]
fn composable_save_accepts_the_zero_step_and_selected_step_documents() {
    let zero = parse_processing_save(composable_save_document(json!([]), Value::Null))
        .expect("zero-step document");
    assert_eq!(zero["steps"], json!([]));
    assert_eq!(zero["currentStepId"], Value::Null);

    let step = composable_step("develop-1", "darktable");
    let body = parse_processing_save(composable_save_document(
        json!([step.clone()]),
        json!("develop-1"),
    ))
    .expect("selected-step document");
    assert_eq!(body["steps"][0], step);
    // The module-owned tree round-trips verbatim, unknown fields kept.
    let mut tree = json!({"kept": {"opaque": [1, 2, 3]}});
    for _ in 0..24 {
        tree = json!({ "node": tree });
    }
    let deep = json!({
        "requestId": "composable-002",
        "expectedRecipeRevision": "recipe-9",
        "expectedSourceRevision": "source-3",
        "currentStepId": "film-1",
        "steps": [{
            "stepId": "film-1",
            "module": "spektrafilm",
            "input": {
                "kind": "artifact",
                "artifactId": "art-1",
                "contract": {
                    "format": "image/tiff",
                    "precision": "float32",
                    "colorSpace": "prophoto-rgb",
                    "transfer": "linear",
                    "geometry": {"width": 64, "height": 32},
                    "encoding": "none",
                },
            },
            "parameters": {
                "schemaVersion": "film-params-1",
                "tree": tree,
            },
        }],
    });
    let body =
        parse_processing_save(deep.to_string().into_bytes()).expect("artifact step document");
    assert_eq!(body, deep);
}

#[test]
fn composable_save_refuses_documents_outside_the_closed_shapes() {
    let step = composable_step("develop-1", "darktable");
    let with_steps = |steps: Value, current: Value| composable_save_document(steps, current);
    // Duplicated keys survive the json! macro only in raw text.
    let duplicate = r#"{"requestId":"c-1","requestId":"c-2","expectedRecipeRevision":null,"expectedSourceRevision":"source-3","currentStepId":null,"steps":[]}"#;
    let cases = [
        (duplicate.as_bytes().to_vec(), "input"),
        (
            json!({
                "requestId": "c-1",
                "expectedRecipeRevision": null,
                "expectedSourceRevision": "source-3",
                "currentStepId": null,
                "steps": [],
                "extra": 1,
            })
            .to_string()
            .into_bytes(),
            "input",
        ),
        // The nullable guards and currentStepId key must be explicit.
        (
            json!({
                "requestId": "c-1",
                "expectedSourceRevision": "source-3",
                "currentStepId": null,
                "steps": [],
            })
            .to_string()
            .into_bytes(),
            "input",
        ),
        (
            json!({
                "requestId": "c 1",
                "expectedRecipeRevision": null,
                "expectedSourceRevision": "source-3",
                "currentStepId": null,
                "steps": [],
            })
            .to_string()
            .into_bytes(),
            "requestId",
        ),
        (with_steps(json!([]), json!("")), "currentStepId"),
        // Steps without a selected current step, and a current step
        // outside the recipe, are both refused locally.
        (
            with_steps(json!([step.clone()]), Value::Null),
            "currentStepId",
        ),
        (
            with_steps(json!([step.clone()]), json!("develop-2")),
            "currentStepId",
        ),
        // Duplicate step ids, an unknown input kind, a zero geometry
        // edge, and a control character in a step id.
        (
            with_steps(json!([step.clone(), step]), json!("develop-1")),
            "steps",
        ),
        (
            json!({
                "requestId": "c-1",
                "expectedRecipeRevision": null,
                "expectedSourceRevision": "source-3",
                "currentStepId": "develop-1",
                "steps": [{
                    "stepId": "develop-1",
                    "module": "darktable",
                    "input": {"kind": "derivative", "photoId": "p1"},
                    "parameters": {"schemaVersion": "darktable-params-1", "tree": {}},
                }],
            })
            .to_string()
            .into_bytes(),
            "input",
        ),
        (
            json!({
                "requestId": "c-1",
                "expectedRecipeRevision": null,
                "expectedSourceRevision": "source-3",
                "currentStepId": "film-1",
                "steps": [{
                    "stepId": "film-1",
                    "module": "spektrafilm",
                    "input": {
                        "kind": "artifact",
                        "artifactId": "art-1",
                        "contract": {
                            "format": "image/tiff",
                            "precision": "float32",
                            "colorSpace": "prophoto-rgb",
                            "transfer": "linear",
                            "geometry": {"width": 0, "height": 32},
                            "encoding": "none",
                        },
                    },
                    "parameters": {"schemaVersion": "film-params-1", "tree": {}},
                }],
            })
            .to_string()
            .into_bytes(),
            "geometry",
        ),
        (
            with_steps(
                json!([composable_step("develop\u{1}-1", "darktable")]),
                json!("develop\u{1}-1"),
            ),
            "stepId",
        ),
    ];
    for (bytes, argument) in cases {
        refuses(
            parse_processing_save(bytes).expect_err("document must be refused"),
            argument,
        );
    }

    // The published tree depth bound: 32 nesting levels admit, 33 do not.
    let nested = |levels: usize| {
        let mut tree = json!(true);
        for _ in 0..levels {
            tree = json!({ "node": tree });
        }
        json!({
            "requestId": "c-1",
            "expectedRecipeRevision": null,
            "expectedSourceRevision": "source-3",
            "currentStepId": "develop-1",
            "steps": [{
                "stepId": "develop-1",
                "module": "darktable",
                "input": {
                    "kind": "original",
                    "photoId": "p1",
                    "sourceRevision": "source-3",
                },
                "parameters": {"schemaVersion": "darktable-params-1", "tree": tree},
            }],
        })
    };
    assert!(parse_processing_save(nested(31).to_string().into_bytes()).is_ok());
    refuses(
        parse_processing_save(nested(32).to_string().into_bytes())
            .expect_err("a 33-level tree is refused"),
        "tree",
    );

    // The published recipe step bound.
    let many: Vec<Value> = (0..65)
        .map(|index| composable_step(&format!("s-{index}"), "darktable"))
        .collect();
    refuses(
        parse_processing_save(with_steps(json!(many), json!("s-0")))
            .expect_err("65 steps are refused"),
        "steps",
    );
}

#[test]
fn composable_recipe_read_preserves_module_trees_and_the_absent_recipe() {
    let recipe = composable_recipe_fixture();
    let read: ProcessingRecipeReadWire = serde_json::from_value(json!({
        "photoId": "p1",
        "sourceRevision": "source-3",
        "recipe": recipe,
    }))
    .expect("fixture decodes");
    let value = validated_processing_recipe_read(read, "p1", &origin()).expect("documented read");
    assert_eq!(value["photoId"], "p1");
    assert_eq!(value["sourceRevision"], "source-3");
    assert_eq!(value["recipe"], recipe);
    assert_eq!(value["webUrl"], "https://slipstream.example/?photoId=p1");

    let absent: ProcessingRecipeReadWire = serde_json::from_value(json!({
        "photoId": "p1",
        "sourceRevision": "",
        "recipe": null,
    }))
    .expect("fixture decodes");
    let value = validated_processing_recipe_read(absent, "p1", &origin())
        .expect("an absent recipe is a successful read");
    assert_eq!(value["recipe"], Value::Null);
    assert_eq!(value["sourceRevision"], "");
    let gap: ProcessingRecipeReadWire = serde_json::from_value(json!({
        "photoId": "p1", "sourceRevision": "", "currentSourceRevision": null,
        "sourceAvailable": false, "recipe": recipe.clone(),
    }))
    .unwrap();
    let retained = validated_processing_recipe_read(gap, "p1", &origin()).unwrap();
    assert_eq!(retained["sourceRevision"], "");
    assert_eq!(retained["recipe"], recipe);
}

#[test]
fn composable_recipe_read_refuses_responses_outside_the_closed_contract() {
    let refused = |document: Value, photo_id: &str| {
        let read: ProcessingRecipeReadWire =
            serde_json::from_value(document).expect("fixture decodes");
        let failure = validated_processing_recipe_read(read, photo_id, &origin())
            .expect_err("response outside the contract");
        assert_eq!(failure.exit_code, 6, "for {failure:?}");
        assert_eq!(failure.payload.code, "transport_failed");
        assert_eq!(
            failure.payload.details["operation"],
            "photos-processing-recipe-get"
        );
    };
    // A changed observed source must leave retained intent inspectable.
    let retained = composable_recipe_fixture();
    let changed: ProcessingRecipeReadWire = serde_json::from_value(json!({
        "photoId": "p1",
        "sourceRevision": "source-4",
        "recipe": retained.clone(),
    }))
    .expect("fixture decodes");
    let inspected = validated_processing_recipe_read(changed, "p1", &origin())
        .expect("retained intent stays readable after source changes");
    assert_eq!(inspected["recipe"], retained);
    assert_eq!(inspected["sourceRevision"], "source-4");
    // A current step that is not one of the recipe's steps.
    let mut stray_current = composable_recipe_fixture();
    stray_current["currentStepId"] = json!("develop-2");
    refused(
        json!({
            "photoId": "p1",
            "sourceRevision": "source-3",
            "recipe": stray_current,
        }),
        "p1",
    );
    // A recipe for another Photo, and an uncommitted empty revision.
    let mut other_photo = composable_recipe_fixture();
    other_photo["photoId"] = json!("p2");
    refused(
        json!({
            "photoId": "p1",
            "sourceRevision": "source-3",
            "recipe": other_photo,
        }),
        "p1",
    );
    let mut empty_revision = composable_recipe_fixture();
    empty_revision["revision"] = json!("");
    refused(
        json!({
            "photoId": "p1",
            "sourceRevision": "source-3",
            "recipe": empty_revision,
        }),
        "p1",
    );
}

#[test]
fn composable_write_confirmation_partitions_outcomes_by_admitted_status() {
    let confirmed = |outcome: &str, status: StatusCode| {
        confirmed_processing_recipe_write(
            &identity(PROCESSING_SAVE_OPERATION),
            "p1",
            "c-1",
            "source-3",
            status,
            ProcessingRecipeWriteWire {
                outcome: outcome.to_owned(),
                recipe: serde_json::from_value(composable_recipe_fixture())
                    .expect("fixture decodes"),
                recipe_version: "recipe-9".to_owned(),
                source_revision: "source-3".to_owned(),
            },
            &origin(),
        )
    };
    let value = confirmed("saved", StatusCode::CREATED).expect("created save");
    assert_eq!(value["outcome"], "saved");
    assert_eq!(value["recipeVersion"], "recipe-9");
    assert_eq!(value["sourceRevision"], "source-3");
    assert_eq!(value["recipe"]["steps"][0]["module"], "darktable");
    assert_eq!(value["webUrl"], "https://slipstream.example/?photoId=p1");
    for outcome in ["replayed", "unchanged"] {
        confirmed(outcome, StatusCode::OK).expect("ok replay or unchanged");
    }
    let unknown = |outcome: &str, status: StatusCode| {
        let failure = confirmed(outcome, status).expect_err("unusable confirmation");
        assert_eq!(failure.exit_code, 7, "for {failure:?}");
        assert_eq!(failure.payload.code, "outcome_unknown");
        assert_eq!(
            failure.payload.details["operation"],
            "photos-processing-recipe-save"
        );
    };
    unknown("saved", StatusCode::OK);
    unknown("replayed", StatusCode::CREATED);
    unknown("conflicted", StatusCode::OK);
}

#[test]
fn source_revisions_round_trip_opaque_bytes_at_the_published_bound() {
    for source in [format!("opaque\0\n{}", "é".repeat(200)), "x".repeat(16_384)] {
        let mut step = composable_step("develop-1", "darktable");
        step["input"]["sourceRevision"] = json!(source);
        let mut save: Value =
            serde_json::from_slice(&composable_save_document(json!([step]), json!("develop-1")))
                .unwrap();
        save["expectedSourceRevision"] = json!(source);
        assert_eq!(
            parse_processing_save(serde_json::to_vec(&save).unwrap()).unwrap(),
            save
        );
        let rebind = json!({"requestId": "rebind-1", "expectedRecipeRevision": "recipe-9", "newSourceRevision": source});
        assert_eq!(
            parse_processing_rebind(serde_json::to_vec(&rebind).unwrap()).unwrap(),
            rebind
        );
        let export = json!({"requestId": "export-1", "stepId": "develop-1", "expectedRecipeRevision": "recipe-9", "expectedSourceRevision": source});
        assert_eq!(
            parse_processing_export(serde_json::to_vec(&export).unwrap()).unwrap(),
            export
        );
    }
    for source in [String::new(), "é".repeat(8_193)] {
        let rebind = json!({"requestId": "rebind-1", "expectedRecipeRevision": "recipe-9", "newSourceRevision": source});
        refuses(
            parse_processing_rebind(serde_json::to_vec(&rebind).unwrap()).unwrap_err(),
            "newSourceRevision",
        );
    }
    let rebind = json!({"requestId": "rebind-1", "expectedRecipeRevision": "r".repeat(129), "newSourceRevision": "source-3"});
    refuses(
        parse_processing_rebind(serde_json::to_vec(&rebind).unwrap()).unwrap_err(),
        "expectedRecipeRevision",
    );
}

#[test]
fn explicit_retry_accepts_only_one_new_request_identity() {
    let retry = json!({"requestId": "retry-1"});
    assert_eq!(
        parse_processing_export_retry(serde_json::to_vec(&retry).unwrap()).unwrap(),
        retry
    );
    for bytes in [
        br#"{"requestId":"retry-1","requestId":"retry-2"}"#.to_vec(),
        br#"{"requestId":"retry-1","stepId":"new-step"}"#.to_vec(),
        br#"{"requestId":"retry-1","expectedSourceRevision":"new-source"}"#.to_vec(),
        b"{}".to_vec(),
    ] {
        refuses(parse_processing_export_retry(bytes).unwrap_err(), "input");
    }
    refuses(
        parse_processing_export_retry(br#"{"requestId":"retry 1"}"#.to_vec()).unwrap_err(),
        "requestId",
    );
}

#[test]
fn source_revision_limits_apply_to_save_guards_original_bindings_and_exports() {
    for source in [String::new(), "x".repeat(16_385)] {
        let mut save: Value =
            serde_json::from_slice(&composable_save_document(json!([]), Value::Null)).unwrap();
        save["expectedSourceRevision"] = json!(source);
        refuses(
            parse_processing_save(serde_json::to_vec(&save).unwrap()).unwrap_err(),
            "expectedSourceRevision",
        );

        let mut step = composable_step("develop-1", "darktable");
        step["input"]["sourceRevision"] = json!(source);
        refuses(
            parse_processing_save(composable_save_document(json!([step]), json!("develop-1")))
                .unwrap_err(),
            "sourceRevision",
        );

        let export = json!({"requestId": "export-1", "stepId": "develop-1", "expectedRecipeRevision": "recipe-9", "expectedSourceRevision": source});
        refuses(
            parse_processing_export(serde_json::to_vec(&export).unwrap()).unwrap_err(),
            "expectedSourceRevision",
        );
    }
    let mut recipe = composable_recipe_fixture();
    let source = format!("opaque\0{}", "é".repeat(300));
    recipe["sourceRevision"] = json!(source);
    recipe["steps"][0]["input"]["sourceRevision"] = json!(source);
    let read: ProcessingRecipeReadWire = serde_json::from_value(
        json!({"photoId": "p1", "sourceRevision": source, "recipe": recipe.clone()}),
    )
    .unwrap();
    let inspected = validated_processing_recipe_read(read, "p1", &origin()).unwrap();
    assert_eq!(inspected["sourceRevision"], source);
    assert_eq!(inspected["recipe"], recipe);
}

#[test]
fn retained_work_and_artifact_validate_opaque_source_bytes_independently_of_recipe_revision() {
    let source = format!("\0{}", "é".repeat(8_191)) + "x";
    assert_eq!(source.len(), 16_384);
    let binding = json!({"kind": "original", "photoId": "p1", "sourceRevision": source});
    let parameters = json!({"schemaVersion": "darktable-params-1", "tree": {"stack": []}});
    let work = json!({
        "photoId": "p1", "requestId": "export-1", "stepId": "develop-1",
        "module": "darktable", "recipeRevision": "r".repeat(128),
        "sourceRevision": source, "adapterSchemaVersion": "darktable-adapter-1:darktable-params-1",
        "parameters": parameters, "input": binding, "bundleId": "bundle-1",
        "state": "accepted", "acceptedAt": 20, "attempt": null, "artifactId": null,
        "failureReason": null, "terminalAt": null, "retainUntil": null,
    });
    assert!(processing_work_valid(&work, "p1", Some("export-1")));
    for field in ["sourceRevision", "recipeRevision"] {
        let mut invalid = work.clone();
        invalid[field] = json!(if field == "sourceRevision" {
            source.clone() + "x"
        } else {
            "r".repeat(129)
        });
        assert!(
            !processing_work_valid(&invalid, "p1", Some("export-1")),
            "{field}"
        );
    }
    let mut invalid_binding = work.clone();
    invalid_binding["input"]["sourceRevision"] = json!(source.clone() + "x");
    assert!(!processing_work_valid(
        &invalid_binding,
        "p1",
        Some("export-1")
    ));

    let artifact = json!({
        "artifactId": "artifact-1", "photoId": "p1", "stepId": "develop-1",
        "module": "darktable", "adapterSchemaVersion": "darktable-adapter-1:darktable-params-1",
        "parameters": parameters,
        "input": {"binding": binding, "sha256": "a".repeat(64), "byteLength": 24},
        "outputContract": {"format": "image/tiff", "precision": "float32", "colorSpace": "prophoto-rgb", "transfer": "linear", "geometry": {"width": 8, "height": 4}, "encoding": "none"},
        "bundleId": "bundle-1", "filename": "artifact-1.tif",
        "publishedAt": "2026-10-02T12:00:00Z", "expiresAt": "2026-10-09T12:00:00Z",
        "sha256": "b".repeat(64), "byteLength": 96,
    });
    assert!(processing_artifact_valid(&artifact, "p1"));
    for invalid_source in [String::new(), source + "x"] {
        let mut invalid = artifact.clone();
        invalid["input"]["binding"]["sourceRevision"] = json!(invalid_source);
        assert!(!processing_artifact_valid(&invalid, "p1"));
    }
}
