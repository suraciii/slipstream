//! The `GET /api/processing/capability` report for the optional local Photo
//! Development extension.

use crate::{
    ProcessingConfig,
    edit_recipe::{ExposureRangeWire, approved_exposure_range},
    http::HttpState,
};
use axum::{extract::State, response::Json};
use serde::Serialize;
use slipstream_processing::photo_profile::{APPROVED_WHITE_BALANCE_MODE, approved_profile_ids};

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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileWire {
    profile_id: &'static str,
    white_balance_modes: [&'static str; 1],
    white_balance_ranges: Option<()>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StageStatesWire {
    develop: &'static str,
    film: &'static str,
}

fn approved_profiles() -> Vec<ProfileWire> {
    approved_profile_ids()
        .map(|profile_id| ProfileWire {
            profile_id,
            white_balance_modes: [APPROVED_WHITE_BALANCE_MODE],
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
            stages: StageStatesWire {
                develop: "unavailable",
                film: "unavailable",
            },
        }
    }

    fn from_config(
        config: &ProcessingConfig,
        incarnation: Option<String>,
        processing_available: bool,
    ) -> Self {
        let bundle_ready = config.failure.is_none();
        let ready = bundle_ready && processing_available;
        let state = config.failure.unwrap_or(if processing_available {
            "ready"
        } else {
            "resource-unavailable"
        });
        Self {
            state,
            bundle_id: ready.then(|| config.bundle_sha256.clone()),
            incarnation: ready.then_some(incarnation).flatten(),
            exposure: approved_exposure_range(),
            profiles: approved_profiles(),
            stages: StageStatesWire {
                develop: if ready { "ready" } else { "unavailable" },
                film: "unavailable",
            },
        }
    }
}

pub(crate) async fn get_processing_capability(
    State(state): State<HttpState>,
) -> Json<ProcessingCapabilityResponse> {
    let Some(config) = state.processing else {
        return Json(ProcessingCapabilityResponse::disabled());
    };
    Json(ProcessingCapabilityResponse::from_config(
        &config,
        Some(state.application.instance_epoch().to_owned()),
        state.application.processing_available(),
    ))
}

/// The capability condition used by the Edit Recipe boundary. It is derived
/// from startup validation and the opened processing resources.
pub(crate) async fn capability_condition(
    config: &ProcessingConfig,
    processing_available: bool,
) -> &'static str {
    if let Some(failure) = config.failure {
        failure
    } else if processing_available {
        "ready"
    } else {
        "resource-unavailable"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn config(failure: Option<&'static str>) -> ProcessingConfig {
        ProcessingConfig {
            policy_sha256: "b".repeat(64),
            bundle_sha256: "c".repeat(64),
            bundle_root: PathBuf::from("/opt/slipstream-photo"),
            failure,
        }
    }

    #[test]
    fn valid_local_bundle_reports_ready_development_and_unavailable_film() {
        let response =
            ProcessingCapabilityResponse::from_config(&config(None), Some("a".repeat(32)), true);
        assert_eq!(response.state, "ready");
        assert_eq!(response.bundle_id.as_deref(), Some("c".repeat(64).as_str()));
        assert_eq!(
            response.incarnation.as_deref(),
            Some("a".repeat(32).as_str())
        );
        assert_eq!(response.stages.develop, "ready");
        assert_eq!(response.stages.film, "unavailable");
    }

    #[test]
    fn invalid_local_bundle_reports_bundle_unavailable_without_identity() {
        let response = ProcessingCapabilityResponse::from_config(
            &config(Some("bundle-unavailable")),
            Some("a".repeat(32)),
            true,
        );
        assert_eq!(response.state, "bundle-unavailable");
        assert!(response.bundle_id.is_none());
        assert!(response.incarnation.is_none());
        assert_eq!(response.stages.develop, "unavailable");
    }

    #[tokio::test]
    async fn missing_processing_resource_is_not_ready() {
        let response =
            ProcessingCapabilityResponse::from_config(&config(None), Some("a".repeat(32)), false);
        assert_eq!(response.state, "resource-unavailable");
        assert!(response.bundle_id.is_none());
        assert!(response.incarnation.is_none());
        assert_eq!(response.stages.develop, "unavailable");
        assert_eq!(
            capability_condition(&config(None), false).await,
            "resource-unavailable"
        );
    }
}
