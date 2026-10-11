//! Canonical retained Film state and exact local request replay.
use photo_files::{Directory, FileLease, LeaseMode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use slipstream_core::{
    ComposableEditRecipe, ComposableEditRecipeWriteOutcome, ProcessingInput,
    SaveComposableEditRecipe,
};
use std::{collections::BTreeMap, io::Read, path::Path, time::Duration};

const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;
const LOCK_WAIT: Duration = Duration::from_secs(2);
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    intent: Value,
    recipe: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StateDocument {
    format_version: u32,
    recipe: Value,
    receipts: BTreeMap<String, Receipt>,
}

pub(super) struct FilmState {
    workspace: Directory,
    directory: Option<Directory>,
    _leases: Vec<FileLease>,
    document: Option<StateDocument>,
}
fn load(directory: &Directory) -> Result<Option<StateDocument>, String> {
    let file = match directory.open_file("state.json") {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_STATE_BYTES {
        return Err("Film state exceeds its bounded read limit".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err("Film state exceeds its bounded read limit".into());
    }
    let document: StateDocument = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if document.format_version != 1 {
        return Err("Unsupported canonical Film state version".into());
    }
    slipstream_core::processing_recipe_decode(document.recipe.clone())
        .map_err(|e| e.to_string())?;
    Ok(Some(document))
}
fn projected(value: &Value) -> Result<ComposableEditRecipe, String> {
    let mut recipe =
        slipstream_core::processing_recipe_decode(value.clone()).map_err(|e| e.to_string())?;
    recipe.revision = format!(
        "film-{:x}",
        Sha256::digest(photo_files::canonical_json(value).map_err(|e| e.to_string())?)
    );
    Ok(recipe)
}
impl FilmState {
    pub(super) fn open(workspace: &Directory) -> Result<Self, String> {
        let mut leases =
            photo_files::ancestor_leases(workspace.path(), LOCK_WAIT).map_err(|e| e.to_string())?;
        leases.push(
            workspace
                .lease(LeaseMode::Shared, LOCK_WAIT)
                .map_err(|e| e.to_string())?,
        );
        let directory = match workspace.child(".edits") {
            Ok(edits) => match edits.child("spektrafilm") {
                Ok(film) => match film.child("adapter") {
                    Ok(directory) => Some(directory),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e.to_string()),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.to_string()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
        if let Some(directory) = directory.as_ref() {
            let lock = match directory.open_file(".state-lock") {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    match directory.create_file(".state-lock", 0o600) {
                        Ok(file) => file,
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => directory
                            .open_file(".state-lock")
                            .map_err(|e| e.to_string())?,
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Err(e) => return Err(e.to_string()),
            };
            leases.push(
                FileLease::acquire(lock, LeaseMode::Exclusive, LOCK_WAIT)
                    .map_err(|e| e.to_string())?,
            );
        }
        let document = directory.as_ref().map(load).transpose()?.flatten();
        Ok(Self {
            workspace: workspace.try_clone().map_err(|e| e.to_string())?,
            directory,
            _leases: leases,
            document,
        })
    }
    fn publish_directory(&mut self) -> Result<&Directory, String> {
        if self.directory.is_none() {
            let directory = self
                .workspace
                .create_directory(".edits")
                .and_then(|d| d.create_directory("spektrafilm"))
                .and_then(|d| d.create_directory("adapter"))
                .map_err(|e| e.to_string())?;
            let lock = directory
                .create_file(".state-lock", 0o600)
                .or_else(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        directory.open_file(".state-lock")
                    } else {
                        Err(e)
                    }
                })
                .map_err(|e| e.to_string())?;
            self._leases.push(
                FileLease::acquire(lock, LeaseMode::Exclusive, LOCK_WAIT)
                    .map_err(|e| e.to_string())?,
            );
            self.directory = Some(directory);
        }
        Ok(self.directory.as_ref().expect("published directory"))
    }

    pub(super) fn recipe(&self) -> Result<Option<ComposableEditRecipe>, String> {
        self.document
            .as_ref()
            .map(|d| projected(&d.recipe))
            .transpose()
    }
    pub(super) fn replay(
        &self,
        request: &str,
        intent: &Value,
    ) -> Result<Option<ComposableEditRecipeWriteOutcome>, String> {
        let Some(receipt) = self.document.as_ref().and_then(|d| d.receipts.get(request)) else {
            return Ok(None);
        };
        if &receipt.intent != intent {
            return Ok(Some(ComposableEditRecipeWriteOutcome::RequestConflict));
        }
        let recipe = slipstream_core::processing_recipe_decode(receipt.recipe.clone())
            .map_err(|e| e.to_string())?;
        Ok(Some(ComposableEditRecipeWriteOutcome::Replayed(recipe)))
    }
    pub(super) fn settle(
        &mut self,
        mutation: &SaveComposableEditRecipe,
        current: Option<ComposableEditRecipe>,
        intent: Value,
    ) -> Result<ComposableEditRecipeWriteOutcome, String> {
        if mutation.expected_recipe_revision.as_deref()
            != current.as_ref().map(|r| r.revision.as_str())
        {
            return Ok(ComposableEditRecipeWriteOutcome::Conflict(current));
        }
        preserve_retained(current.as_ref(), &mutation.recipe)?;
        let unchanged = self.document.is_some()
            && current.as_ref().is_some_and(|r| {
                r.steps == mutation.recipe.steps
                    && r.current_step_id == mutation.recipe.current_step_id
            });
        let mut recipe = mutation.recipe.clone();
        if unchanged {
            recipe = current.expect("unchanged requires current state")
        } else {
            recipe.revision = photo_files::identity("film").map_err(|e| e.to_string())?;
        }
        let encoded =
            slipstream_core::processing_recipe_encode(&recipe).map_err(|e| e.to_string())?;
        let committed = if unchanged {
            recipe.clone()
        } else {
            projected(&encoded)?
        };
        let initially_unpublished = self.directory.is_none();
        let mut document = self.document.take().unwrap_or(StateDocument {
            format_version: 1,
            recipe: encoded.clone(),
            receipts: BTreeMap::new(),
        });
        if !unchanged {
            document.recipe = encoded;
        }
        document.receipts.insert(
            mutation.request_id.clone(),
            Receipt {
                intent: intent.clone(),
                recipe: slipstream_core::processing_recipe_encode(&committed)
                    .map_err(|e| e.to_string())?,
            },
        );
        let bytes = photo_files::canonical_json(&document).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err("Film state retention allowance is exhausted; no state was changed".into());
        }
        self.publish_directory()?;
        if initially_unpublished
            && let Some(directory) = self.directory.as_ref()
            && let Some(latest) = load(directory)?
        {
            self.document = Some(latest);
            if let Some(receipt) = self
                .document
                .as_ref()
                .and_then(|document| document.receipts.get(&mutation.request_id))
            {
                if receipt.intent == intent {
                    let replayed =
                        slipstream_core::processing_recipe_decode(receipt.recipe.clone())
                            .map_err(|e| e.to_string())?;
                    return Ok(ComposableEditRecipeWriteOutcome::Replayed(replayed));
                }
                return Ok(ComposableEditRecipeWriteOutcome::RequestConflict);
            }
            let current = self.recipe()?;
            return Ok(ComposableEditRecipeWriteOutcome::Conflict(current));
        }
        photo_files::atomic_json(
            self.directory.as_ref().expect("published directory"),
            "state.json",
            &document,
        )
        .map_err(|e| e.to_string())?;
        self.document = Some(document);
        Ok(if unchanged {
            ComposableEditRecipeWriteOutcome::Unchanged(committed)
        } else {
            ComposableEditRecipeWriteOutcome::Saved(committed)
        })
    }
}
pub(super) fn preserve_retained(
    current: Option<&ComposableEditRecipe>,
    proposed: &ComposableEditRecipe,
) -> Result<(), String> {
    let supported = |step: &slipstream_core::ProcessingStep| {
        step.module.as_str() == slipstream_processing::modules::SPEKTRAFILM_MODULE
            && slipstream_processing::modules::ModuleRegistry::new(
                slipstream_processing::modules::ModuleAvailability::ready(),
                slipstream_processing::modules::ModuleAvailability::ready(),
            )
            .validate_saved_parameters(&slipstream_processing::modules::Parameters {
                module: step.module.as_str().into(),
                version: step.parameters.schema_version.clone(),
                tree: step.parameters.tree.clone(),
            })
            .is_ok()
    };
    for old in current
        .into_iter()
        .flat_map(|r| r.steps.iter())
        .filter(|s| !supported(s))
    {
        if proposed.steps.iter().find(|s| s.step_id == old.step_id) != Some(old) {
            return Err("Retained darktable or unsupported intent must be echoed unchanged; use the full darktable namespace for darktable edits".into());
        }
    }
    for step in proposed.steps.iter().filter(|s| !supported(s)) {
        if current.and_then(|r| r.steps.iter().find(|s| s.step_id == step.step_id)) != Some(step) {
            return Err("Generic processing writes cannot mutate darktable or unsupported state; use the full darktable namespace".into());
        }
    }
    Ok(())
}
pub(super) fn inspect(path: &Path) -> Result<Value, String> {
    let directory = Directory::open(path).map_err(|e| e.to_string())?;
    let lease = directory
        .lease(LeaseMode::Shared, LOCK_WAIT)
        .map_err(|e| e.to_string())?;
    let document = load(&directory)?.ok_or_else(|| "Canonical Film state is missing".to_owned())?;
    let recipe = projected(&document.recipe)?;
    let mut retained = vec![recipe.clone()];
    for receipt in document.receipts.values() {
        retained.push(
            slipstream_core::processing_recipe_decode(receipt.recipe.clone())
                .map_err(|e| e.to_string())?,
        );
    }
    let dependencies=retained.iter().flat_map(|r|r.steps.iter()).map(|step|match &step.input {
        ProcessingInput::WorkspaceArtifact {workspace_id,artifact_id,revision,..}=>json!({"kind":"input","workspaceId":workspace_id,"artifactId":artifact_id,"revision":revision,"id":step.step_id.as_str(),"name":"Retained Film input","action":"Rebind or remove the Film state explicitly"}),
        ProcessingInput::Artifact {artifact_id,..}=>json!({"kind":"processing-artifact","artifactId":artifact_id.as_str(),"id":step.step_id.as_str(),"name":"Retained processing input","action":"Rebind or remove the Film state explicitly"}),
        ProcessingInput::Original {photo_id,source_revision}=>json!({"kind":"original","photoId":photo_id,"sourceRevision":source_revision,"id":step.step_id.as_str(),"name":"Retained Original input","action":"Resolve its exact protected capture before removal"}),
    }).collect::<Vec<_>>();
    drop(lease);
    Ok(
        json!({"tool":"spektrafilm","revision":recipe.revision,"dependencies":dependencies,"recipe":super::recipe_json(&recipe)}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use slipstream_core::{
        ProcessingGeometry, ProcessingImageContract, ProcessingModuleId,
        ProcessingParameterSnapshot, ProcessingStep, ProcessingStepId,
    };
    fn recipe() -> ComposableEditRecipe {
        ComposableEditRecipe {
            photo_id: "photo-1".into(),
            revision: "draft".into(),
            source_revision: "".into(),
            current_step_id: Some(ProcessingStepId::new("film").unwrap()),
            steps: vec![ProcessingStep {
                step_id: ProcessingStepId::new("film").unwrap(),
                module: ProcessingModuleId::new("spektrafilm").unwrap(),
                input: ProcessingInput::WorkspaceArtifact {
                    workspace_id: "workspace-1".into(),
                    artifact_id: "image-1".into(),
                    revision: "revision-1".into(),
                    contract: ProcessingImageContract {
                        format: "image/tiff".into(),
                        precision: "float32".into(),
                        color_space: "prophoto-rgb".into(),
                        transfer: "linear".into(),
                        geometry: ProcessingGeometry::new(320, 480).unwrap(),
                        encoding: "deflate".into(),
                    },
                },
                parameters: ProcessingParameterSnapshot::new(
                    slipstream_processing::modules::SPEKTRAFILM_PARAMETER_VERSION,
                    slipstream_processing::modules::spektrafilm_default_tree(),
                )
                .unwrap(),
            }],
        }
    }
    fn mutation(
        recipe: ComposableEditRecipe,
        expected: Option<String>,
        request: &str,
    ) -> SaveComposableEditRecipe {
        SaveComposableEditRecipe {
            photo_id: recipe.photo_id.clone(),
            request_id: request.into(),
            expected_recipe_revision: expected,
            expected_source_revision: "".into(),
            request_intent: None,
            recipe,
            automatic_adjustment: None,
        }
    }
    #[test]
    fn workspace_only_state_replay_and_external_revision_conflict() {
        let path = std::env::temp_dir().join(photo_files::identity("film-test").unwrap());
        let directory = Directory::create(&path).unwrap();
        let mut owner = FilmState::open(&directory).unwrap();
        let first = mutation(recipe(), None, "save-1");
        let intent = json!({"test":"first"});
        let saved = match owner.settle(&first, None, intent.clone()).unwrap() {
            ComposableEditRecipeWriteOutcome::Saved(r) => r,
            _ => panic!("not saved"),
        };
        assert_eq!(saved.source_revision, "");
        assert!(matches!(
            owner.replay("save-1", &json!({"test":"other"})).unwrap(),
            Some(ComposableEditRecipeWriteOutcome::RequestConflict)
        ));
        let mut decoded = slipstream_core::processing_recipe_decode(
            owner.document.as_ref().unwrap().recipe.clone(),
        )
        .unwrap();
        let ProcessingInput::WorkspaceArtifact {
            artifact_id,
            revision,
            ..
        } = &mut decoded.steps[0].input
        else {
            panic!("expected workspace input")
        };
        *artifact_id = "image-2".into();
        *revision = "revision-2".into();
        owner.document.as_mut().unwrap().recipe =
            slipstream_core::processing_recipe_encode(&decoded).unwrap();
        photo_files::atomic_json(
            owner.directory.as_ref().unwrap(),
            "state.json",
            owner.document.as_ref().unwrap(),
        )
        .unwrap();
        drop(owner);
        let mut reopened = FilmState::open(&directory).unwrap();
        let current = reopened.recipe().unwrap().unwrap();
        assert_ne!(saved.revision, current.revision);
        assert!(matches!(
            reopened
                .settle(
                    &mutation(recipe(), Some(saved.revision.clone()), "stale"),
                    Some(current),
                    json!({"test":"stale"})
                )
                .unwrap(),
            ComposableEditRecipeWriteOutcome::Conflict(_)
        ));
        match reopened.replay("save-1", &intent).unwrap().unwrap() {
            ComposableEditRecipeWriteOutcome::Replayed(r) => assert_eq!(r, saved),
            _ => panic!("not replayed"),
        };
        let facts = inspect(reopened.directory.as_ref().unwrap().path()).unwrap();
        assert!(
            facts["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["artifactId"] == "image-1")
        );
        drop(reopened);
        drop(directory);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn retained_darktable_omission_and_parameter_mutation_are_refused() {
        let mut old = recipe();
        let mut retained = old.steps[0].clone();
        retained.step_id = ProcessingStepId::new("darktable-old").unwrap();
        retained.module = ProcessingModuleId::new("darktable").unwrap();
        retained.parameters = ProcessingParameterSnapshot::new(
            "unknown-native-version",
            json!({"unknown":{"retain":[1,2,3]}}),
        )
        .unwrap();
        old.steps.push(retained);
        assert!(preserve_retained(Some(&old), &recipe()).is_err());
        let mut changed = old.clone();
        changed.steps[1].parameters.tree = json!({"unknown":false});
        assert!(preserve_retained(Some(&old), &changed).is_err());
        assert!(preserve_retained(Some(&old), &old).is_ok());
    }

    #[test]
    fn rejected_initial_save_does_not_publish_empty_state_directory() {
        let path = std::env::temp_dir().join(photo_files::identity("film-refused").unwrap());
        let directory = Directory::create(&path).unwrap();
        let mut owner = FilmState::open(&directory).unwrap();
        let mut proposed = recipe();
        proposed.steps[0].parameters.schema_version = "foreign-version".into();
        let result = owner.settle(
            &mutation(proposed, None, "rejected"),
            None,
            json!({"test":"rejected"}),
        );
        assert!(result.is_err());
        assert!(!path.join(".edits").exists());
        drop(owner);
        drop(directory);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn malformed_published_state_remains_inspection_blocker() {
        let path = std::env::temp_dir().join(photo_files::identity("film-malformed").unwrap());
        let adapter = Directory::create(&path)
            .unwrap()
            .create_directory(".edits")
            .unwrap()
            .create_directory("spektrafilm")
            .unwrap()
            .create_directory("adapter")
            .unwrap();
        std::fs::write(adapter.path().join("state.json"), b"{}").unwrap();
        assert!(inspect(adapter.path()).is_err());
        drop(adapter);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn raced_initial_save_conflicts_without_overwriting_receipt() {
        let path = std::env::temp_dir().join(photo_files::identity("film-race-conflict").unwrap());
        let directory = Directory::create(&path).unwrap();
        let mut first = FilmState::open(&directory).unwrap();
        let mut second = FilmState::open(&directory).unwrap();
        let saved = match first
            .settle(
                &mutation(recipe(), None, "first"),
                None,
                json!({"request":"first"}),
            )
            .unwrap()
        {
            ComposableEditRecipeWriteOutcome::Saved(recipe) => recipe,
            other => panic!("unexpected outcome: {other:?}"),
        };
        drop(first);
        let result = second
            .settle(
                &mutation(recipe(), None, "second"),
                None,
                json!({"request":"second"}),
            )
            .unwrap();
        assert!(matches!(
            result,
            ComposableEditRecipeWriteOutcome::Conflict(_)
        ));
        assert!(
            matches!(second.replay("first", &json!({"request":"first"})).unwrap(), Some(ComposableEditRecipeWriteOutcome::Replayed(replayed)) if replayed == saved)
        );
        drop(second);
        drop(directory);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn raced_initial_same_request_replays_committed_receipt() {
        let path = std::env::temp_dir().join(photo_files::identity("film-race-replay").unwrap());
        let directory = Directory::create(&path).unwrap();
        let mut first = FilmState::open(&directory).unwrap();
        let mut second = FilmState::open(&directory).unwrap();
        let intent = json!({"request":"same"});
        let saved = match first
            .settle(&mutation(recipe(), None, "same"), None, intent.clone())
            .unwrap()
        {
            ComposableEditRecipeWriteOutcome::Saved(recipe) => recipe,
            other => panic!("unexpected outcome: {other:?}"),
        };
        drop(first);
        let result = second
            .settle(&mutation(recipe(), None, "same"), None, intent)
            .unwrap();
        assert!(
            matches!(result, ComposableEditRecipeWriteOutcome::Replayed(replayed) if replayed == saved)
        );
        drop(second);
        drop(directory);
        std::fs::remove_dir_all(path).unwrap();
    }
}
