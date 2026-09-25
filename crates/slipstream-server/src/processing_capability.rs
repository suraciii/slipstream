//! The `GET /api/processing/capability` report. The response is the closed
//! capability shape of the merged Photo Development Surface wire contract:
//! the state the deployment can distinguish, the launcher identities it
//! observed, the approved per-class profile report, and one state per stage.

use crate::{
    ProcessingConfig,
    edit_recipe::{ExposureRangeWire, approved_exposure_range},
    http::HttpState,
};
use axum::{extract::State, response::Json};
use serde::Serialize;
use slipstream_processing::{
    photo::{self, Response, ResultBody},
    photo_profile::{APPROVED_WHITE_BALANCE_MODE, approved_profile_ids},
    protocol::{Availability, PHOTO_CAPABILITY},
};

/// The capability report of the merged Photo Development Surface wire
/// contract: one closed `state`, the launcher identities it observed, the
/// approved per-class profile report, and one state per stage.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessingCapabilityResponse {
    state: &'static str,
    bundle_id: Option<String>,
    incarnation: Option<String>,
    exposure: ExposureRangeWire,
    profiles: Vec<ProfileWire>,
    stages: StageStatesWire,
}

/// One approved source class. `whiteBalanceRanges` is `null` while the class
/// admits no adjustable white-balance mode; the only admitted mode of this
/// workload is `as-shot`, which is not adjustable, so no range set exists.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProfileWire {
    profile_id: &'static str,
    white_balance_modes: [&'static str; 1],
    white_balance_ranges: Option<()>,
}

/// One closed state per pipeline stage. `develop` follows the RAW
/// qualification of the configured bundle; `film` stays unavailable until
/// that stage is qualified.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StageStatesWire {
    develop: &'static str,
    film: &'static str,
}

impl StageStatesWire {
    fn new(develop: &'static str) -> Self {
        Self {
            develop,
            film: "unavailable",
        }
    }
}

fn approved_profiles() -> Vec<ProfileWire> {
    approved_profile_ids()
        .map(|profile_id| ProfileWire {
            profile_id,
            white_balance_modes: [APPROVED_WHITE_BALANCE_MODE],
            // No adjustable mode is admitted for this workload, so every
            // report carries the contract's `null`.
            white_balance_ranges: None,
        })
        .collect()
}

impl ProcessingCapabilityResponse {
    fn disabled() -> Self {
        Self {
            state: "disabled",
            bundle_id: None,
            incarnation: None,
            exposure: approved_exposure_range(),
            profiles: approved_profiles(),
            stages: StageStatesWire::new("unavailable"),
        }
    }

    /// A deployment defect observed without a launcher answer that names its
    /// identities: no bundle identity or incarnation was observed.
    fn launcher_unavailable_unobserved() -> Self {
        Self {
            state: "launcher-unavailable",
            bundle_id: None,
            incarnation: None,
            exposure: approved_exposure_range(),
            profiles: approved_profiles(),
            stages: StageStatesWire::new("unavailable"),
        }
    }

    /// A launcher answered and named its identities. The state and the
    /// develop-stage state are fixed by the closed condition the answer
    /// maps onto; `profiles` stays empty only for `source-unsupported`.
    fn observed(state: &'static str, bundle_id: String, incarnation: String) -> Self {
        let source_unsupported = state == "source-unsupported";
        Self {
            state,
            bundle_id: Some(bundle_id),
            incarnation: Some(incarnation),
            exposure: approved_exposure_range(),
            profiles: if source_unsupported {
                Vec::new()
            } else {
                approved_profiles()
            },
            stages: StageStatesWire::new(if source_unsupported {
                "unsupported"
            } else if state == "ready" {
                "ready"
            } else {
                "unavailable"
            }),
        }
    }
}

pub(crate) async fn get_processing_capability(
    State(state): State<HttpState>,
) -> Json<ProcessingCapabilityResponse> {
    let Some(config) = state.processing else {
        return Json(ProcessingCapabilityResponse::disabled());
    };

    let socket = config.socket_path();
    let instance = config.instance.clone();
    let response = tokio::task::spawn_blocking(move || photo::reconcile(socket, instance))
        .await
        .ok()
        .and_then(Result::ok);
    Json(match response {
        Some(response) => map_reconcile_response(&config, response),
        None => ProcessingCapabilityResponse::launcher_unavailable_unobserved(),
    })
}

/// Maps one launcher Reconcile answer onto the closed capability conditions.
/// A launcher that does not expose the photo-processing capability reports
/// `source-unsupported` with an empty profile list; every other refusal is
/// the deployment defect its answer names, and a fully proven answer is
/// `ready`.
fn map_reconcile_response(
    config: &ProcessingConfig,
    response: Response,
) -> ProcessingCapabilityResponse {
    let Response::Result {
        mode,
        version: 1,
        result,
    } = response
    else {
        return ProcessingCapabilityResponse::launcher_unavailable_unobserved();
    };
    let ResultBody::Capability {
        capability,
        instance,
        incarnation,
        next_sequence,
        policy,
        bundle,
        availability,
        ..
    } = *result
    else {
        return ProcessingCapabilityResponse::launcher_unavailable_unobserved();
    };
    // Launcher identity and bounded fields validate before anything they
    // name can be surfaced, including the source-unsupported condition: a
    // wrong-instance or stale launcher must fail closed without values.
    if instance != config.instance || !lower_hex(&incarnation, 32) || next_sequence == 0 {
        return ProcessingCapabilityResponse::launcher_unavailable_unobserved();
    }
    if policy != config.policy_sha256 {
        return ProcessingCapabilityResponse::observed("launcher-unavailable", bundle, incarnation);
    }
    if bundle != config.bundle_sha256 {
        return ProcessingCapabilityResponse::observed("bundle-unavailable", bundle, incarnation);
    }
    if mode != PHOTO_CAPABILITY || capability != PHOTO_CAPABILITY {
        return ProcessingCapabilityResponse::observed("source-unsupported", bundle, incarnation);
    }
    if availability != Availability::Available {
        return ProcessingCapabilityResponse::observed("resource-unavailable", bundle, incarnation);
    }
    ProcessingCapabilityResponse::observed("ready", bundle, incarnation)
}

/// The closed capability condition of one configured deployment: the state
/// the launcher answer maps onto, or the transport-failure state. Per-Photo
/// support derivation uses this to stay consistent with the capability
/// report.
pub(crate) async fn capability_condition(config: &ProcessingConfig) -> &'static str {
    let socket = config.socket_path();
    let instance = config.instance.clone();
    let response = tokio::task::spawn_blocking(move || photo::reconcile(socket, instance))
        .await
        .ok()
        .and_then(Result::ok);
    match response {
        Some(response) => map_reconcile_response(config, response).state,
        None => "launcher-unavailable",
    }
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTANCE: &str = "0123456789abcdef0123456789abcdef";

    fn config() -> ProcessingConfig {
        ProcessingConfig {
            instance: INSTANCE.to_owned(),
            policy_sha256: "b".repeat(64),
            bundle_sha256: "c".repeat(64),
            socket_override: None,
        }
    }

    fn capability(capability: &str, availability: Availability) -> Response {
        capability_with_identities(capability, availability, "b", "c")
    }

    fn capability_with_identities(
        capability: &str,
        availability: Availability,
        policy: &str,
        bundle: &str,
    ) -> Response {
        Response::result(ResultBody::Capability {
            capability: capability.to_owned(),
            instance: INSTANCE.to_owned(),
            incarnation: "a".repeat(32),
            next_sequence: 1,
            policy: policy.repeat(64),
            bundle: bundle.repeat(64),
            availability,
            active: None,
        })
    }

    #[test]
    fn proven_launcher_readiness_reports_the_ready_condition() {
        let response = map_reconcile_response(
            &config(),
            capability("photo-processing", Availability::Available),
        );
        assert_eq!(response.state, "ready");
        assert_eq!(response.bundle_id.as_deref(), Some("c".repeat(64).as_str()));
        assert_eq!(
            response.incarnation.as_deref(),
            Some("a".repeat(32).as_str())
        );
        // The proven reconcile qualifies the develop stage; the film stage
        // stays unavailable until its own qualification.
        assert_eq!(response.stages.develop, "ready");
        assert_eq!(response.stages.film, "unavailable");
        assert_eq!(
            response
                .profiles
                .iter()
                .map(|profile| profile.profile_id)
                .collect::<Vec<_>>(),
            vec!["sony-ilce-7rm5-arw", "sony-ilce-7cm2-arw"]
        );
        assert_eq!(response.exposure.minimum_ev, 0.0);
        assert_eq!(response.exposure.maximum_ev, 1.0);
        assert_eq!(response.exposure.step_ev, 0.001);
    }

    #[test]
    fn qualification_and_film_capabilities_are_source_unsupported_with_empty_profiles() {
        for name in ["qualification-only", "film-measurement-only"] {
            let response =
                map_reconcile_response(&config(), capability(name, Availability::Available));
            assert_eq!(response.state, "source-unsupported");
            // The closed contract empties the profile list for this state.
            assert!(response.profiles.is_empty());
            assert_eq!(response.stages.develop, "unsupported");
            assert!(response.bundle_id.is_some());
        }
    }

    #[test]
    fn unrecognized_capability_from_unproven_identity_fails_closed_unobserved() {
        // A wrong-instance launcher that does not even expose the
        // photo-processing capability must fail closed as launcher-unavailable
        // and must not surface any of the values it named.
        let wrong_instance = Response::result(ResultBody::Capability {
            capability: "qualification-only".to_owned(),
            instance: "f".repeat(32),
            incarnation: "a".repeat(32),
            next_sequence: 1,
            policy: "b".repeat(64),
            bundle: "c".repeat(64),
            availability: Availability::Available,
            active: None,
        });
        let response = map_reconcile_response(&config(), wrong_instance);
        assert_eq!(response.state, "launcher-unavailable");
        assert!(response.bundle_id.is_none());
        assert!(response.incarnation.is_none());

        // The same for a stale sequence and a malformed incarnation.
        let stale_sequence = Response::result(ResultBody::Capability {
            capability: "qualification-only".to_owned(),
            instance: INSTANCE.to_owned(),
            incarnation: "a".repeat(32),
            next_sequence: 0,
            policy: "b".repeat(64),
            bundle: "c".repeat(64),
            availability: Availability::Available,
            active: None,
        });
        let response = map_reconcile_response(&config(), stale_sequence);
        assert_eq!(response.state, "launcher-unavailable");
        assert!(response.bundle_id.is_none());

        let malformed_incarnation = Response::result(ResultBody::Capability {
            capability: "qualification-only".to_owned(),
            instance: INSTANCE.to_owned(),
            incarnation: "zz-not-hex".to_owned(),
            next_sequence: 1,
            policy: "b".repeat(64),
            bundle: "c".repeat(64),
            availability: Availability::Available,
            active: None,
        });
        let response = map_reconcile_response(&config(), malformed_incarnation);
        assert_eq!(response.state, "launcher-unavailable");
        assert!(response.incarnation.is_none());
    }

    #[test]
    fn blocked_and_mismatched_launcher_identities_fail_closed() {
        let blocked = map_reconcile_response(
            &config(),
            capability("photo-processing", Availability::Blocked),
        );
        assert_eq!(blocked.state, "resource-unavailable");
        assert_eq!(blocked.stages.develop, "unavailable");

        let policy_mismatch = map_reconcile_response(
            &config(),
            capability_with_identities("photo-processing", Availability::Available, "d", "c"),
        );
        assert_eq!(policy_mismatch.state, "launcher-unavailable");

        let bundle_mismatch = map_reconcile_response(
            &config(),
            capability_with_identities("photo-processing", Availability::Available, "b", "d"),
        );
        assert_eq!(bundle_mismatch.state, "bundle-unavailable");
        // The launcher named the bundle identity it actually runs.
        assert_eq!(
            bundle_mismatch.bundle_id.as_deref(),
            Some("d".repeat(64).as_str())
        );
    }
}
