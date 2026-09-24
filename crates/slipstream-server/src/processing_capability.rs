use crate::{ProcessingConfig, http::HttpState};
use axum::{extract::State, response::Json};
use serde::Serialize;
use slipstream_processing::{
    protocol::{Availability, PHOTO_CAPABILITY, Request, Response, ResultBody},
    request as launcher_request,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessingCapabilityResponse {
    state: &'static str,
    launcher: &'static str,
    source: &'static str,
    bundle: &'static str,
    reason: Option<&'static str>,
}

impl ProcessingCapabilityResponse {
    fn disabled() -> Self {
        Self {
            state: "disabled",
            launcher: "disabled",
            source: "disabled",
            bundle: "disabled",
            reason: Some("operator-disabled"),
        }
    }

    fn unavailable(reason: &'static str) -> Self {
        Self {
            state: "unavailable",
            launcher: "unavailable",
            source: "unavailable",
            bundle: "unavailable",
            reason: Some(reason),
        }
    }

    fn source_unavailable() -> Self {
        Self {
            state: "unavailable",
            launcher: "available",
            source: "unavailable",
            bundle: "available",
            reason: Some("source-unavailable"),
        }
    }

    fn launcher_unavailable(reason: &'static str, bundle: &'static str) -> Self {
        Self {
            state: "unavailable",
            launcher: "unavailable",
            source: "unavailable",
            bundle,
            reason: Some(reason),
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
    let reconcile = Request::Reconcile {
        version: 1,
        instance: config.instance.clone(),
    };
    let response = tokio::task::spawn_blocking(move || launcher_request(socket, &reconcile))
        .await
        .ok()
        .and_then(Result::ok);
    Json(match response {
        Some(response) => map_reconcile_response(&config, response),
        None => ProcessingCapabilityResponse::unavailable("launcher-unavailable"),
    })
}

fn map_reconcile_response(
    config: &ProcessingConfig,
    response: Response,
) -> ProcessingCapabilityResponse {
    let Response::Result { version: 1, result } = response else {
        return ProcessingCapabilityResponse::unavailable("launcher-unavailable");
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
        return ProcessingCapabilityResponse::unavailable("unsupported-capability");
    };
    if capability != PHOTO_CAPABILITY {
        return ProcessingCapabilityResponse::unavailable("unsupported-capability");
    }
    if instance != config.instance || !lower_hex(&incarnation, 32) || next_sequence == 0 {
        return ProcessingCapabilityResponse::unavailable("identity-mismatch");
    }
    let bundle = if bundle == config.bundle_sha256 {
        "available"
    } else {
        "unavailable"
    };
    if policy != config.policy_sha256 {
        return ProcessingCapabilityResponse::launcher_unavailable("identity-mismatch", bundle);
    }
    if availability != Availability::Available {
        return ProcessingCapabilityResponse::launcher_unavailable("launcher-blocked", bundle);
    }
    if bundle != "available" {
        return ProcessingCapabilityResponse::launcher_unavailable("identity-mismatch", bundle);
    }

    ProcessingCapabilityResponse::source_unavailable()
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
        Response::Result {
            version: 1,
            result: Box::new(ResultBody::Capability {
                capability: capability.to_owned(),
                instance: INSTANCE.to_owned(),
                incarnation: "a".repeat(32),
                next_sequence: 1,
                policy: policy.repeat(64),
                bundle: bundle.repeat(64),
                availability,
                active: None,
            }),
        }
    }

    #[test]
    fn exact_launcher_readiness_still_reports_unavailable_until_source_is_connected() {
        let response = map_reconcile_response(
            &config(),
            capability("photo-processing", Availability::Available),
        );
        assert_eq!(response.state, "unavailable");
        assert_eq!(response.launcher, "available");
        assert_eq!(response.bundle, "available");
        assert_eq!(response.source, "unavailable");
        assert_eq!(response.reason, Some("source-unavailable"));
    }

    #[test]
    fn qualification_and_film_capabilities_are_not_photo_readiness() {
        for name in ["qualification-only", "film-measurement-only"] {
            let response =
                map_reconcile_response(&config(), capability(name, Availability::Available));
            assert_eq!(response.reason, Some("unsupported-capability"));
        }
    }

    #[test]
    fn blocked_and_mismatched_launcher_identities_fail_closed() {
        let blocked = map_reconcile_response(
            &config(),
            capability("photo-processing", Availability::Blocked),
        );
        assert_eq!(blocked.reason, Some("launcher-blocked"));
        assert_eq!(blocked.launcher, "unavailable");
        assert_eq!(blocked.bundle, "available");

        let policy_mismatch = map_reconcile_response(
            &config(),
            capability_with_identities("photo-processing", Availability::Available, "d", "c"),
        );
        assert_eq!(policy_mismatch.reason, Some("identity-mismatch"));
        assert_eq!(policy_mismatch.launcher, "unavailable");
        assert_eq!(policy_mismatch.bundle, "available");

        let bundle_mismatch = map_reconcile_response(
            &config(),
            capability_with_identities("photo-processing", Availability::Available, "b", "d"),
        );
        assert_eq!(bundle_mismatch.reason, Some("identity-mismatch"));
        assert_eq!(bundle_mismatch.launcher, "unavailable");
        assert_eq!(bundle_mismatch.bundle, "unavailable");
    }
}
