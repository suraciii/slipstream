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
                // The film stage's readiness is independent of the
                // darktable stage: a `darktable-disabled` deployment with
                // a verified film runtime reports film ready.
                film: match config.film.as_ref() {
                    Some(film) if film.ready() && processing_available => "ready",
                    Some(_) => "unavailable",
                    None => "unavailable",
                },
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
            film: None,
            failure,
        }
    }

    fn film_ready() -> Option<crate::config::FilmConfig> {
        Some(crate::config::FilmConfig {
            bundle_sha256: "f".repeat(64),
            bundle_root: PathBuf::from("/opt/slipstream-film"),
            engine: PathBuf::from("/opt/runtime/bin/python"),
            runner: PathBuf::from("/opt/slipstream-film/runner/film_runner.py"),
            source_root: PathBuf::from("/opt/spektrafilm/src"),
            parameter_default: serde_json::json!({
                "camera": {}, "enlarger": {}, "scanner": {}, "io": {},
                "settings": {}, "debug": {}, "filmRender": {},
                "printRender": {}, "taps": {},
            }),
            failure: None,
        })
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
    fn a_verified_film_runtime_reports_the_film_stage_ready_independently() {
        let mut with_film = config(None);
        with_film.film = film_ready();
        let response =
            ProcessingCapabilityResponse::from_config(&with_film, Some("a".repeat(32)), true);
        assert_eq!(response.stages.develop, "ready");
        assert_eq!(response.stages.film, "ready");

        // A configured-but-unverified runtime stays truthfully unavailable
        // even while development is ready.
        let mut broken_film = config(None);
        broken_film.film = Some(crate::config::FilmConfig {
            failure: Some("film-bundle-unavailable"),
            ..film_ready().expect("fixture")
        });
        let response =
            ProcessingCapabilityResponse::from_config(&broken_film, Some("a".repeat(32)), true);
        assert_eq!(response.stages.develop, "ready");
        assert_eq!(response.stages.film, "unavailable");

        // Film readiness never carries a resource-unavailable deployment.
        let response =
            ProcessingCapabilityResponse::from_config(&with_film, Some("a".repeat(32)), false);
        assert_eq!(response.state, "resource-unavailable");
        assert_eq!(response.stages.film, "unavailable");
    }

    #[tokio::test]
    async fn a_darktable_disabled_deployment_reports_film_ready_on_its_own() {
        let mut film_only = config(Some("darktable-disabled"));
        film_only.film = film_ready();
        let response =
            ProcessingCapabilityResponse::from_config(&film_only, Some("a".repeat(32)), true);
        assert_eq!(response.state, "darktable-disabled");
        assert!(response.bundle_id.is_none());
        assert_eq!(response.stages.develop, "unavailable");
        assert_eq!(response.stages.film, "ready");
        assert_eq!(
            capability_condition(&film_only, true).await,
            "darktable-disabled"
        );
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
