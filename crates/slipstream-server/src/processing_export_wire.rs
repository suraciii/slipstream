//! The shared Processing Export wire projections: the exact JSON shapes
//! every admitted Export, retained record, and composable Edit Recipe
//! answer repeats unchanged.
//!
//! The input binding has exactly three wire shapes — an Original bound to
//! its guarded Photo, an artifact bound to its published identity, and a
//! workspace artifact bound to its selected revision — and one image
//! contract shape. Admission in both surfaces compares against the same
//! contract projection, so a shared projection is what keeps a changed
//! contract detected as exactly equal or exactly different. The shapes are
//! frozen: no caller may reshape them per surface.

use serde_json::{Value, json};
use slipstream_core::{ProcessingImageContract, ProcessingInput};

/// The exact wire shape of one step input binding.
///
/// `original` carries the guarded Photo and its pinned source revision,
/// `artifact` carries the published artifact identity and its concrete
/// contract, and `workspace-artifact` carries the selected workspace
/// image, its revision, and the same contract shape. The `kind`
/// discriminant is the only tag; no other field discriminates the shapes.
pub(crate) fn input_json(input: &ProcessingInput) -> Value {
    match input {
        ProcessingInput::Original {
            photo_id,
            source_revision,
        } => json!({
            "kind": "original",
            "photoId": photo_id,
            "sourceRevision": source_revision,
        }),
        ProcessingInput::Artifact {
            artifact_id,
            contract,
        } => json!({
            "kind": "artifact",
            "artifactId": artifact_id.as_str(),
            "contract": contract_json(contract),
        }),
        ProcessingInput::WorkspaceArtifact {
            workspace_id,
            artifact_id,
            revision,
            contract,
        } => json!({
            "kind": "workspace-artifact",
            "workspaceId": workspace_id,
            "artifactId": artifact_id,
            "revision": revision,
            "contract": contract_json(contract),
        }),
    }
}

/// The exact wire shape of one image contract, the same shape embedded
/// under an input binding's `contract` field and compared against the
/// workspace's admitted contract. Every field name is frozen.
pub(crate) fn contract_json(contract: &ProcessingImageContract) -> Value {
    json!({
        "format": contract.format,
        "precision": contract.precision,
        "colorSpace": contract.color_space,
        "transfer": contract.transfer,
        "geometry": {
            "width": contract.geometry.width,
            "height": contract.geometry.height,
        },
        "encoding": contract.encoding,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use slipstream_core::{ProcessingArtifactId, ProcessingGeometry};

    fn contract() -> ProcessingImageContract {
        ProcessingImageContract {
            format: "image/tiff".to_owned(),
            precision: "uint16".to_owned(),
            color_space: "prophoto-rgb".to_owned(),
            transfer: "linear".to_owned(),
            geometry: ProcessingGeometry::new(640, 480).unwrap(),
            encoding: "deflate".to_owned(),
        }
    }

    #[test]
    fn original_input_projects_its_exact_binding_shape() {
        assert_eq!(
            input_json(&ProcessingInput::Original {
                photo_id: "photo-1".to_owned(),
                source_revision: "source-9".to_owned(),
            }),
            json!({
                "kind": "original",
                "photoId": "photo-1",
                "sourceRevision": "source-9",
            })
        );
    }

    #[test]
    fn artifact_input_projects_its_identity_and_contract_shape() {
        assert_eq!(
            input_json(&ProcessingInput::Artifact {
                artifact_id: ProcessingArtifactId::new("artifact-1").unwrap(),
                contract: contract(),
            }),
            json!({
                "kind": "artifact",
                "artifactId": "artifact-1",
                "contract": contract_json(&contract()),
            })
        );
    }

    #[test]
    fn workspace_artifact_input_projects_its_revision_and_contract_shape() {
        assert_eq!(
            input_json(&ProcessingInput::WorkspaceArtifact {
                workspace_id: "workspace-1".to_owned(),
                artifact_id: "capture/input.tif".to_owned(),
                revision: "capture-1".to_owned(),
                contract: contract(),
            }),
            json!({
                "kind": "workspace-artifact",
                "workspaceId": "workspace-1",
                "artifactId": "capture/input.tif",
                "revision": "capture-1",
                "contract": contract_json(&contract()),
            })
        );
    }

    #[test]
    fn contract_projection_changes_when_any_contract_field_changes() {
        let baseline = contract_json(&contract());
        let mut changed = contract();
        changed.precision = "uint8".to_owned();
        assert_ne!(contract_json(&changed), baseline);
        let mut changed = contract();
        changed.geometry = ProcessingGeometry::new(641, 480).unwrap();
        assert_ne!(contract_json(&changed), baseline);
        let mut changed = contract();
        changed.encoding = "none".to_owned();
        assert_ne!(contract_json(&changed), baseline);
        assert_eq!(contract_json(&contract()), baseline);
    }

    #[test]
    fn one_contract_projection_serves_both_binding_shapes_that_carry_it() {
        let artifact = input_json(&ProcessingInput::Artifact {
            artifact_id: ProcessingArtifactId::new("artifact-1").unwrap(),
            contract: contract(),
        });
        let workspace = input_json(&ProcessingInput::WorkspaceArtifact {
            workspace_id: "workspace-1".to_owned(),
            artifact_id: "capture/input.tif".to_owned(),
            revision: "capture-1".to_owned(),
            contract: contract(),
        });
        assert_eq!(artifact["contract"], contract_json(&contract()));
        assert_eq!(workspace["contract"], contract_json(&contract()));
    }
}
