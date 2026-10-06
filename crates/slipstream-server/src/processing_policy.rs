//! Shared Processing Module admission facts for Export and retry routes.
//!
//! This module owns only policy that must agree across route boundaries. HTTP
//! status and wire mapping stay with the callers; source staging, execution,
//! persistence, and publication stay with their existing owners.

use crate::config::ProcessingConfig;
use slipstream_core::{ProcessingExportAdapterDecision, ProcessingInput};
use slipstream_processing::modules::{
    DARKTABLE_ADAPTER_VERSION, DARKTABLE_MODULE, DARKTABLE_PARAMETER_VERSION, ModuleAvailability,
    ModuleError, ModuleRegistry, Parameters, SPEKTRAFILM_ADAPTER_VERSION, SPEKTRAFILM_MODULE,
    SPEKTRAFILM_PARAMETER_VERSION,
};

/// The closed refusal reason used when no qualified module adapter exists.
pub(crate) const NO_QUALIFIED_ADAPTER: &str = "module_parameters_unavailable";

/// One module policy view over the immutable deployment processing config.
pub(crate) struct ProcessingModulePolicy<'a> {
    processing: &'a ProcessingConfig,
}

/// The known minimum live-memory requirement and the effective finite limit
/// for one full-resolution Film input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FilmMemoryRequirement {
    pub(crate) minimum_live_bytes: u64,
    pub(crate) memory_limit_bytes: u64,
}

impl<'a> ProcessingModulePolicy<'a> {
    pub(crate) fn new(processing: &'a ProcessingConfig) -> Self {
        Self { processing }
    }

    /// Saved intent and execution parameter validation deliberately use ready
    /// descriptions. Availability is checked separately by the deployment
    /// policy and must not make retained intent unreadable.
    pub(crate) fn validate_parameters(&self, parameters: &Parameters) -> Result<(), ModuleError> {
        ModuleRegistry::new(ModuleAvailability::ready(), ModuleAvailability::ready())
            .validate_parameters(parameters)
    }

    /// Qualify one selected module against its concrete Processing input. A
    /// known module with the wrong input remains a structured refusal; an
    /// unknown module remains distinguishable to the HTTP caller.
    pub(crate) fn adapter_decision(
        &self,
        module: &str,
        input: &ProcessingInput,
    ) -> Option<ProcessingExportAdapterDecision> {
        match (module, input) {
            (DARKTABLE_MODULE, ProcessingInput::Original { .. }) => {
                Some(ProcessingExportAdapterDecision::Qualified {
                    adapter_version: DARKTABLE_ADAPTER_VERSION.to_owned(),
                    parameter_schema_version: DARKTABLE_PARAMETER_VERSION.to_owned(),
                })
            }
            (SPEKTRAFILM_MODULE, ProcessingInput::Artifact { .. }) if self.film_is_ready() => {
                Some(ProcessingExportAdapterDecision::Qualified {
                    adapter_version: SPEKTRAFILM_ADAPTER_VERSION.to_owned(),
                    parameter_schema_version: SPEKTRAFILM_PARAMETER_VERSION.to_owned(),
                })
            }
            (DARKTABLE_MODULE | SPEKTRAFILM_MODULE, _) => {
                Some(ProcessingExportAdapterDecision::NoQualifiedAdapter {
                    reason_code: NO_QUALIFIED_ADAPTER.to_owned(),
                })
            }
            _ => None,
        }
    }

    pub(crate) fn adapter_matches(
        &self,
        adapter: &ProcessingExportAdapterDecision,
        captured: &str,
    ) -> bool {
        match adapter {
            ProcessingExportAdapterDecision::Qualified {
                adapter_version,
                parameter_schema_version,
            } => format!("{adapter_version}:{parameter_schema_version}") == captured,
            ProcessingExportAdapterDecision::NoQualifiedAdapter { .. } => false,
        }
    }

    /// Return the currently qualified bundle identity for a module. The
    /// fallback preserves the existing empty/unavailable behavior for callers
    /// that are already past durable admission.
    pub(crate) fn bundle_id(&self, module: &str) -> String {
        if module == SPEKTRAFILM_MODULE {
            self.processing
                .film
                .as_ref()
                .filter(|film| film.ready())
                .map_or_else(
                    || self.processing.bundle_sha256.clone(),
                    |film| film.bundle_sha256.clone(),
                )
        } else {
            self.processing.bundle_sha256.clone()
        }
    }

    /// Check the Film-only full-resolution lower-bound resource gate. The
    /// route maps the returned I/O failure and limit mismatch to its own wire
    /// wording because submission and retry intentionally differ there.
    pub(crate) fn film_memory_requirement(
        &self,
        module: &str,
        input: &ProcessingInput,
    ) -> std::io::Result<Option<FilmMemoryRequirement>> {
        if module != SPEKTRAFILM_MODULE {
            return Ok(None);
        }
        if !self.film_is_ready() {
            return Ok(None);
        }
        let ProcessingInput::Artifact { contract, .. } = input else {
            return Ok(None);
        };
        let required = crate::film_resources::minimum_live_bytes(
            contract.geometry.width,
            contract.geometry.height,
        );
        let limit = crate::film_resources::effective_memory_limit()?;
        Ok(Some(FilmMemoryRequirement {
            minimum_live_bytes: required,
            memory_limit_bytes: limit,
        }))
    }

    fn film_is_ready(&self) -> bool {
        self.processing
            .film
            .as_ref()
            .is_some_and(crate::config::FilmConfig::ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FilmConfig;
    use std::path::PathBuf;

    fn config(film_failure: Option<&'static str>) -> ProcessingConfig {
        ProcessingConfig {
            policy_sha256: "p".repeat(64),
            bundle_sha256: "d".repeat(64),
            bundle_root: PathBuf::from("/opt/slipstream-photo"),
            film: Some(FilmConfig {
                bundle_sha256: "f".repeat(64),
                bundle_root: PathBuf::from("/opt/slipstream-film"),
                binary: PathBuf::from("/opt/slipstream-film/spektrafilm"),
                data_root: PathBuf::from("/opt/slipstream-film/data"),
                parameter_default: serde_json::json!({}),
                film_profile: "film".to_owned(),
                print_profile: "print".to_owned(),
                failure: film_failure,
            }),
            failure: None,
        }
    }

    fn original() -> ProcessingInput {
        ProcessingInput::Original {
            photo_id: "photo-1".to_owned(),
            source_revision: "source-1".to_owned(),
        }
    }

    fn artifact() -> ProcessingInput {
        ProcessingInput::Artifact {
            artifact_id: slipstream_core::ProcessingArtifactId::new("artifact-1").unwrap(),
            contract: slipstream_core::ProcessingImageContract {
                format: "tiff".to_owned(),
                precision: "float32".to_owned(),
                color_space: "prophoto-rgb".to_owned(),
                transfer: "linear".to_owned(),
                geometry: slipstream_core::ProcessingGeometry::new(10, 10).unwrap(),
                encoding: "deflate".to_owned(),
            },
        }
    }

    #[test]
    fn peer_input_qualification_stays_independent() {
        let config = config(None);
        let policy = ProcessingModulePolicy::new(&config);
        assert!(matches!(
            policy.adapter_decision(DARKTABLE_MODULE, &original()),
            Some(ProcessingExportAdapterDecision::Qualified { .. })
        ));
        assert!(matches!(
            policy.adapter_decision(SPEKTRAFILM_MODULE, &artifact()),
            Some(ProcessingExportAdapterDecision::Qualified { .. })
        ));
        assert!(matches!(
            policy.adapter_decision(DARKTABLE_MODULE, &artifact()),
            Some(ProcessingExportAdapterDecision::NoQualifiedAdapter { .. })
        ));
        assert!(policy.adapter_decision("unknown", &original()).is_none());
    }

    #[test]
    fn film_bundle_is_not_selected_when_unavailable() {
        let ready = config(None);
        assert_eq!(
            ProcessingModulePolicy::new(&ready).bundle_id(SPEKTRAFILM_MODULE),
            "f".repeat(64)
        );
        let unavailable = config(Some("bundle-failed"));
        assert_eq!(
            ProcessingModulePolicy::new(&unavailable).bundle_id(SPEKTRAFILM_MODULE),
            "d".repeat(64)
        );
    }

    #[test]
    fn captured_adapter_identity_is_checked_without_accepting_refusals() {
        let config = config(None);
        let policy = ProcessingModulePolicy::new(&config);
        let adapter = policy
            .adapter_decision(SPEKTRAFILM_MODULE, &artifact())
            .unwrap();
        assert!(
            policy.adapter_matches(&adapter, "spektrafilm-rs-adapter-1:spektrafilm-rs-params-1")
        );
        assert!(!policy.adapter_matches(&adapter, "darktable-adapter-1:darktable-params-1"));
        assert!(!policy.adapter_matches(
            &ProcessingExportAdapterDecision::NoQualifiedAdapter {
                reason_code: "module_parameters_unavailable".to_owned(),
            },
            "spektrafilm-rs-adapter-1:spektrafilm-rs-params-1"
        ));
    }
}
