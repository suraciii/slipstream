//! `GET /api/processing/modules`: bounded per-module discovery for Issue #496.
//!
//! Discovery is descriptive, not product admission. Each peer has an independent
//! availability record. A configured processing identity is runnable only when
//! the server also opened the bounded Export owner; standalone SpektraFilm is
//! unavailable until its own qualified runtime is installed.

use axum::{extract::State, response::Json};
use serde::Serialize;
use slipstream_processing::modules::{ModuleAvailability, ModuleDescription, ModuleRegistry};

use crate::http::HttpState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessingModulesResponse {
    pub contract_version: &'static str,
    pub modules: Vec<ModuleDescription>,
}

pub(crate) fn registry(state: &HttpState) -> ModuleRegistry {
    let processing = state.processing.as_ref();
    let exports_open = processing.is_some() && state.application.exports.is_some();
    let darktable = match processing {
        Some(config) if config.failure.is_none() && exports_open => ModuleAvailability::ready(),
        Some(config) if config.failure == Some("darktable-disabled") => {
            ModuleAvailability::unavailable("darktable processing is disabled for this deployment")
        }
        _ => ModuleAvailability::unavailable("darktable processing runtime is not configured"),
    };
    let spektrafilm = match processing
        .filter(|_| exports_open)
        .and_then(|processing| processing.film.as_ref())
    {
        Some(film) if film.ready() => ModuleAvailability::ready(),
        Some(_) => ModuleAvailability::unavailable(
            "standalone SpektraFilm runtime is configured but failed verification",
        ),
        None => ModuleAvailability::unavailable("standalone SpektraFilm runtime is not configured"),
    };
    ModuleRegistry::new(darktable, spektrafilm)
}

pub(crate) async fn get_processing_modules(
    State(state): State<HttpState>,
) -> Json<ProcessingModulesResponse> {
    let registry = registry(&state);
    Json(ProcessingModulesResponse {
        contract_version: slipstream_processing::modules::MODULE_CONTRACT_VERSION,
        modules: registry.descriptions().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FilmConfig, ProcessingConfig};
    use std::path::PathBuf;

    fn film(failure: Option<&'static str>) -> FilmConfig {
        FilmConfig {
            bundle_sha256: "f".repeat(64),
            bundle_root: PathBuf::from("/opt/slipstream-film"),
            binary: PathBuf::from("/opt/slipstream-film/spektrafilm"),
            data_root: PathBuf::from("/opt/slipstream-film/data"),
            parameter_default: serde_json::json!({
                "camera": {}, "enlarger": {}, "scanner": {}, "io": {},
                "settings": {}, "debug": {}, "filmRender": {},
                "printRender": {}, "taps": {},
            }),
            film_profile: "kodak_portra_400".to_owned(),
            print_profile: "kodak_portra_endura".to_owned(),
            failure,
        }
    }

    /// Discovery keeps the two peers' availability records independent: a
    /// ready darktable never makes SpektraFilm ready, and a verified film
    /// runtime reports ready only on its own evidence.
    #[test]
    fn module_availability_stays_independent_per_peer() {
        let mut processing = ProcessingConfig {
            policy_sha256: "b".repeat(64),
            bundle_sha256: "c".repeat(64),
            bundle_root: PathBuf::from("/opt/slipstream-photo"),
            film: None,
            failure: None,
        };
        let spektrafilm = |processing: &ProcessingConfig| {
            let darktable = ModuleAvailability::ready();
            let availability = match processing.film.as_ref() {
                Some(film) if film.ready() => ModuleAvailability::ready(),
                Some(_) => ModuleAvailability::unavailable("unverified"),
                None => ModuleAvailability::unavailable("not configured"),
            };
            let registry = ModuleRegistry::new(darktable, availability);
            registry
                .descriptions()
                .iter()
                .find(|description| description.id.name == "spektrafilm")
                .unwrap()
                .availability
                .state
        };
        assert_ne!(
            spektrafilm(&processing),
            slipstream_processing::modules::AvailabilityState::Ready
        );
        processing.film = Some(film(None));
        assert_eq!(
            spektrafilm(&processing),
            slipstream_processing::modules::AvailabilityState::Ready
        );
        processing.film = Some(film(Some("film-bundle-unavailable")));
        assert_ne!(
            spektrafilm(&processing),
            slipstream_processing::modules::AvailabilityState::Ready
        );
    }
}
