use super::*;

#[test]
fn source_unsupported_condition_downgrades_only_supported_classes() {
    let supported = SupportClassification {
        state: "supported",
        reason: None,
    };
    let downgraded = apply_capability_condition(supported, "source-unsupported");
    assert_eq!(downgraded.state, "unsupported");
    assert!(downgraded.reason.is_none());

    // Other conditions keep the class fact; an already-unavailable or
    // unsupported class never becomes supported.
    for condition in [
        "disabled",
        "ready",
        "bundle-unavailable",
        "resource-unavailable",
    ] {
        assert_eq!(
            apply_capability_condition(supported, condition).state,
            "supported"
        );
    }
    let unavailable = SupportClassification {
        state: "unavailable",
        reason: Some(ORIGINAL_MISSING),
    };
    assert_eq!(
        apply_capability_condition(unavailable, "source-unsupported").state,
        "unavailable"
    );
    let unsupported = SupportClassification {
        state: "unsupported",
        reason: None,
    };
    assert_eq!(
        apply_capability_condition(unsupported, "source-unsupported").state,
        "unsupported"
    );
}

#[test]
fn temperature_tint_payload_bounds_close_the_shape() {
    assert_eq!(
        parse_white_balance_payload(&serde_json::json!({"mode": "as-shot"})),
        Some(WhiteBalanceIntent::AsShot)
    );
    assert_eq!(
        parse_white_balance_payload(&serde_json::json!({
            "mode": "temperature-tint", "temperatureKelvin": 6500, "tintMilli": -10
        })),
        Some(WhiteBalanceIntent::TemperatureTint {
            temperature_kelvin: 6500,
            tint_milli: -10,
        })
    );
    // Out of bounds, wrong arity, wrong types, and unknown modes refuse.
    assert_eq!(
        parse_white_balance_payload(&serde_json::json!({
            "mode": "temperature-tint", "temperatureKelvin": 999, "tintMilli": 0
        })),
        None
    );
    assert_eq!(
        parse_white_balance_payload(&serde_json::json!({
            "mode": "temperature-tint", "temperatureKelvin": 6500
        })),
        None
    );
    assert_eq!(
        parse_white_balance_payload(&serde_json::json!({
            "mode": "temperature-tint", "temperatureKelvin": 6.5, "tintMilli": 0
        })),
        None
    );
    assert_eq!(
        parse_white_balance_payload(&serde_json::json!({"mode": "custom"})),
        None
    );
    assert_eq!(
        parse_white_balance_payload(&serde_json::json!("as-shot")),
        None
    );
}

#[test]
fn stored_temperature_tint_reads_but_blocks_processing() {
    let recipe = EditRecipe {
        photo_id: "photo".to_owned(),
        revision: "rev-1".to_owned(),
        source_revision: "source-1".to_owned(),
        settings: EditRecipeSettings {
            exposure_ev: 0.25,
            white_balance: WhiteBalanceIntent::TemperatureTint {
                temperature_kelvin: 6500,
                tint_milli: 12,
            },
        },
    };
    let wire = RecipeWire::from(recipe.clone());
    assert_eq!(
        wire.white_balance,
        serde_json::json!({
            "mode": "temperature-tint", "temperatureKelvin": 6500, "tintMilli": 12
        })
    );
    let support = SupportClassification {
        state: "supported",
        reason: None,
    };
    assert!(!processing_available(support, "ready", true, Some(&recipe)));
    // The same exposure as as-shot stays processable only when the
    // reconciled condition is ready.
    let mut as_shot = recipe.clone();
    as_shot.settings.white_balance = WhiteBalanceIntent::AsShot;
    assert!(processing_available(support, "ready", true, Some(&as_shot)));
}

#[test]
fn only_the_ready_condition_admits_processing() {
    let recipe = EditRecipe {
        photo_id: "photo".to_owned(),
        revision: "rev-1".to_owned(),
        source_revision: "source-1".to_owned(),
        settings: EditRecipeSettings {
            exposure_ev: 0.0,
            white_balance: WhiteBalanceIntent::AsShot,
        },
    };
    let support = SupportClassification {
        state: "supported",
        reason: None,
    };
    for condition in [
        "disabled",
        "bundle-unavailable",
        "source-unsupported",
        "resource-unavailable",
    ] {
        assert!(
            !processing_available(support, condition, true, Some(&recipe)),
            "{condition} must not admit processing"
        );
    }
    assert!(processing_available(support, "ready", true, Some(&recipe)));
}
fn support_facts<'a>(
    capture: &'a slipstream_core::CaptureFact,
    kind: OriginalKind,
    filename: &'a str,
) -> SourceFacts<'a> {
    SourceFacts {
        kind,
        filename,
        capture,
    }
}

#[test]
fn source_support_requires_current_published_revision() {
    let revision = "current-revision";
    let approved = slipstream_core::CaptureFact {
        state: slipstream_core::CaptureMetadataState::Missing,
        order_key: None,
        field: None,
        offset_minutes: None,
        source_revision: Some(revision.to_owned()),
        identity: slipstream_core::CameraIdentity::Observed {
            make: Some("SONY".to_owned()),
            model: Some("ILCE-7RM5".to_owned()),
        },
    };
    let facts = support_facts(&approved, OriginalKind::Raw, "photo.ARW");
    assert_eq!(
        derive_support(facts, true, true, Some(revision)),
        SupportClassification {
            state: "supported",
            reason: None,
        }
    );
    assert_eq!(
        derive_support(facts, true, true, None),
        SupportClassification {
            state: "unavailable",
            reason: Some(READ_PENDING),
        }
    );
    let pending = slipstream_core::CaptureFact::pending();
    assert_eq!(
        derive_support(
            support_facts(&pending, OriginalKind::Raw, "photo.ARW"),
            true,
            true,
            None,
        ),
        SupportClassification {
            state: "unavailable",
            reason: Some(READ_PENDING),
        }
    );
}

#[test]
fn only_revision_bound_failures_are_confirmed_unreadable() {
    let failed = slipstream_core::CaptureFact::failed(Some("current-revision".to_owned()));
    assert_eq!(
        derive_support(
            support_facts(&failed, OriginalKind::Raw, "photo.ARW"),
            true,
            true,
            Some("current-revision"),
        ),
        SupportClassification {
            state: "unavailable",
            reason: Some(ORIGINAL_UNREADABLE),
        }
    );
    let transient = slipstream_core::CaptureFact::failed(None);
    assert_eq!(
        derive_support(
            support_facts(&transient, OriginalKind::Raw, "photo.ARW"),
            true,
            true,
            None,
        ),
        SupportClassification {
            state: "unavailable",
            reason: Some(READ_PENDING),
        }
    );
}
