use crate::persistence::Persistence;
use crate::persistence::test_support::*;
use crate::processing::{
    ComposableEditRecipe, ComposableEditRecipeWriteOutcome, ComposableRecipeRequestError,
    ProcessingArtifactId, ProcessingContractError, ProcessingGeometry, ProcessingImageContract,
    ProcessingInput, ProcessingModuleId, ProcessingParameterSnapshot, ProcessingStep,
    ProcessingStepId, SaveComposableEditRecipe,
};
use crate::source_revision;
use rusqlite::Connection;
use std::path::PathBuf;

#[path = "composable_recipe_tests/durability.rs"]
mod durability;

struct Seeded {
    _tree: TempTree,
    _root: crate::LibraryRoot,
    persistence: Persistence,
    path: PathBuf,
    source: String,
}

fn seeded_persistence() -> Seeded {
    let (tree, library, state, name, path) = fixture();
    seed(
        &path,
        include_str!("../../../../compatibility/sqlite/schema-v6.sql"),
    );
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
            [library.canonical_path().to_str().unwrap()],
        )
        .unwrap();
    add_recipe_test_photo(
        &connection,
        RecipeTestPhoto {
            original_id: "raw-original",
            photo_id: "raw-photo",
            relative_path: "shoot/one.ARW",
            kind: "raw",
            available: true,
            size: 17,
            mtime_ms: 1_000.0,
        },
    );
    let source = source_revision("shoot/one.ARW", 17, 1_000.0).unwrap();
    connection
        .execute(
            "UPDATE original_files SET capture_metadata_state='missing',capture_source_revision=? WHERE id='raw-original'",
            [format!("{}\0fixture-device\0fixture-inode", source)],
        )
        .unwrap();
    drop(connection);
    let persistence = Persistence::open(
        state,
        name,
        library.canonical_path().to_string_lossy().into_owned(),
    )
    .unwrap();
    Seeded {
        _tree: tree,
        _root: library,
        persistence,
        path,
        source,
    }
}

fn original_input(photo_id: &str, source_revision: &str) -> ProcessingInput {
    ProcessingInput::Original {
        photo_id: photo_id.to_owned(),
        source_revision: source_revision.to_owned(),
    }
}

fn artifact_input(artifact_id: &str) -> ProcessingInput {
    ProcessingInput::Artifact {
        artifact_id: ProcessingArtifactId::new(artifact_id).unwrap(),
        contract: ProcessingImageContract {
            format: "image/tiff".to_owned(),
            precision: "uint16".to_owned(),
            color_space: "srgb-linear".to_owned(),
            transfer: "linear".to_owned(),
            geometry: ProcessingGeometry::new(6000, 4000).unwrap(),
            encoding: "deflate".to_owned(),
        },
    }
}

fn step(step_id: &str, module: &str, input: ProcessingInput) -> ProcessingStep {
    ProcessingStep {
        step_id: ProcessingStepId::new(step_id).unwrap(),
        module: ProcessingModuleId::new(module).unwrap(),
        input,
        parameters: ProcessingParameterSnapshot::new(
            "schema-1",
            serde_json::json!({"exposure_ev": 0.25, "tone": {"contrast": 1.5}}),
        )
        .unwrap(),
    }
}

fn recipe(
    photo_id: &str,
    source_revision: &str,
    steps: Vec<ProcessingStep>,
    current_step_id: Option<&str>,
) -> ComposableEditRecipe {
    ComposableEditRecipe {
        photo_id: photo_id.to_owned(),
        revision: "caller-snapshot".to_owned(),
        source_revision: source_revision.to_owned(),
        current_step_id: current_step_id.map(|id| ProcessingStepId::new(id).unwrap()),
        steps,
    }
}

fn save(
    photo_id: &str,
    request_id: &str,
    expected_recipe_revision: Option<&str>,
    source_revision: &str,
    recipe: ComposableEditRecipe,
) -> SaveComposableEditRecipe {
    SaveComposableEditRecipe {
        photo_id: photo_id.to_owned(),
        request_id: request_id.to_owned(),
        expected_recipe_revision: expected_recipe_revision.map(str::to_owned),
        expected_source_revision: source_revision.to_owned(),
        recipe,
    }
}

async fn save_recipe(
    seeded: &Seeded,
    mutation: SaveComposableEditRecipe,
) -> ComposableEditRecipeWriteOutcome {
    seeded
        .persistence
        .save_composable_edit_recipe_receiver(mutation)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
}

async fn read_recipe(seeded: &Seeded, photo_id: &str) -> Option<ComposableEditRecipe> {
    seeded
        .persistence
        .composable_edit_recipe_receiver(photo_id)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn zero_step_recipe_saves_and_reads_without_a_current_step() {
    let seeded = seeded_persistence();
    assert_eq!(read_recipe(&seeded, "raw-photo").await, None);

    let empty = recipe("raw-photo", &seeded.source, vec![], None);
    let outcome = save_recipe(
        &seeded,
        save(
            "raw-photo",
            "empty-save",
            None,
            &seeded.source,
            empty.clone(),
        ),
    )
    .await;
    let ComposableEditRecipeWriteOutcome::Saved(committed) = outcome else {
        panic!("zero-step save should commit, got {outcome:?}");
    };
    assert!(committed.steps.is_empty());
    assert_eq!(committed.current_step_id, None);
    assert_eq!(committed.source_revision, seeded.source);
    assert_ne!(committed.revision, empty.revision);

    let stored = read_recipe(&seeded, "raw-photo").await.unwrap();
    assert_eq!(stored, committed);
    assert!(stored.current_step().is_none());
}

#[tokio::test]
async fn one_and_many_step_recipes_save_and_read_back_in_canonical_order() {
    let seeded = seeded_persistence();
    let one = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("tone"),
    );
    let outcome = save_recipe(
        &seeded,
        save("raw-photo", "one-save", None, &seeded.source, one),
    )
    .await;
    let ComposableEditRecipeWriteOutcome::Saved(one_committed) = outcome else {
        panic!("one-step save should commit, got {outcome:?}");
    };
    assert_eq!(
        read_recipe(&seeded, "raw-photo").await.unwrap(),
        one_committed
    );

    // The submitted step order is incidental: storage, digests, and
    // reads are canonical by step_id, and no position survives a save.
    let many = recipe(
        "raw-photo",
        &seeded.source,
        vec![
            step(
                "z-tone",
                "darktable",
                original_input("raw-photo", &seeded.source),
            ),
            step("a-film", "spektrafilm", artifact_input("artifact-7")),
            step("m-grade", "darktable", artifact_input("artifact-9")),
        ],
        Some("m-grade"),
    );
    let outcome = save_recipe(
        &seeded,
        save(
            "raw-photo",
            "many-save",
            Some(&one_committed.revision),
            &seeded.source,
            many,
        ),
    )
    .await;
    let ComposableEditRecipeWriteOutcome::Saved(many_committed) = outcome else {
        panic!("many-step save should commit, got {outcome:?}");
    };
    assert_eq!(
        many_committed
            .steps
            .iter()
            .map(|step| step.step_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a-film", "m-grade", "z-tone"]
    );
    let stored = read_recipe(&seeded, "raw-photo").await.unwrap();
    assert_eq!(stored, many_committed);
    assert_eq!(stored.current_step().unwrap().step_id.as_str(), "m-grade");
}

#[tokio::test]
async fn duplicate_step_ids_and_current_step_misuse_are_refused() {
    let seeded = seeded_persistence();
    let duplicate = recipe(
        "raw-photo",
        &seeded.source,
        vec![
            step(
                "tone",
                "darktable",
                original_input("raw-photo", &seeded.source),
            ),
            step("tone", "spektrafilm", artifact_input("artifact-7")),
        ],
        Some("tone"),
    );
    assert_eq!(
        save_recipe(
            &seeded,
            save(
                "raw-photo",
                "duplicate-save",
                None,
                &seeded.source,
                duplicate
            ),
        )
        .await,
        ComposableEditRecipeWriteOutcome::Invalid(ComposableRecipeRequestError::Contract(
            ProcessingContractError::DuplicateStepId {
                step_id: "tone".to_owned()
            }
        ))
    );
    assert_eq!(read_recipe(&seeded, "raw-photo").await, None);

    let missing_current = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("absent"),
    );
    assert_eq!(
        save_recipe(
            &seeded,
            save(
                "raw-photo",
                "missing-current-save",
                None,
                &seeded.source,
                missing_current,
            ),
        )
        .await,
        ComposableEditRecipeWriteOutcome::Invalid(ComposableRecipeRequestError::Contract(
            ProcessingContractError::CurrentStepNotInRecipe {
                step_id: "absent".to_owned()
            }
        ))
    );

    let unset_current = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        None,
    );
    assert_eq!(
        save_recipe(
            &seeded,
            save(
                "raw-photo",
                "unset-current-save",
                None,
                &seeded.source,
                unset_current,
            ),
        )
        .await,
        ComposableEditRecipeWriteOutcome::Invalid(ComposableRecipeRequestError::Contract(
            ProcessingContractError::CurrentStepUnset
        ))
    );
    assert_eq!(read_recipe(&seeded, "raw-photo").await, None);
}

#[tokio::test]
async fn identity_and_guard_mismatches_are_invalid_requests() {
    let seeded = seeded_persistence();
    let other_photo = recipe(
        "other-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("tone"),
    );
    assert_eq!(
        save_recipe(
            &seeded,
            save(
                "raw-photo",
                "mismatch-save",
                None,
                &seeded.source,
                other_photo
            ),
        )
        .await,
        ComposableEditRecipeWriteOutcome::Invalid(ComposableRecipeRequestError::PhotoMismatch)
    );

    let unbound = recipe(
        "raw-photo",
        "stale-source-revision",
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("tone"),
    );
    assert_eq!(
        save_recipe(
            &seeded,
            save("raw-photo", "unbound-save", None, &seeded.source, unbound),
        )
        .await,
        ComposableEditRecipeWriteOutcome::Invalid(
            ComposableRecipeRequestError::SourceRevisionMismatch,
        )
    );

    let control = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("tone"),
    );
    assert!(matches!(
        save_recipe(
            &seeded,
            save(
                "raw-photo",
                "bad\nrequest",
                None,
                &seeded.source,
                control.clone(),
            ),
        )
        .await,
        ComposableEditRecipeWriteOutcome::Invalid(ComposableRecipeRequestError::InvalidRequestId)
    ));
    assert!(matches!(
        save_recipe(
            &seeded,
            save("raw-photo", &"r".repeat(129), None, &seeded.source, control,),
        )
        .await,
        ComposableEditRecipeWriteOutcome::Invalid(ComposableRecipeRequestError::InvalidRequestId)
    ));
}

#[tokio::test]
async fn replay_returns_the_committed_recipe_and_a_changed_payload_conflicts() {
    let seeded = seeded_persistence();
    let first = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("tone"),
    );
    let committed = match save_recipe(
        &seeded,
        save(
            "raw-photo",
            "first-save",
            None,
            &seeded.source,
            first.clone(),
        ),
    )
    .await
    {
        ComposableEditRecipeWriteOutcome::Saved(committed) => committed,
        outcome => panic!("first save should commit, got {outcome:?}"),
    };

    // Same request identity and same payload: the committed recipe is
    // replayed even though the caller's local snapshot label and the
    // stored revision have both moved on since.
    let replay = save_recipe(
        &seeded,
        save("raw-photo", "first-save", None, &seeded.source, first),
    )
    .await;
    assert_eq!(
        replay,
        ComposableEditRecipeWriteOutcome::Replayed(committed.clone())
    );

    // Same request identity with any other admissible payload: refused,
    // whatever field changed.
    let mut changed_parameters = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("tone"),
    );
    changed_parameters.steps[0].parameters = ProcessingParameterSnapshot::new(
        "schema-1",
        serde_json::json!({"exposure_ev": 1.25, "tone": {"contrast": 1.5}}),
    )
    .unwrap();
    let changed_guard = recipe(
        "raw-photo",
        "another-source-revision",
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", "another-source-revision"),
        )],
        Some("tone"),
    );
    for (expected, source, recipe_changed) in [
        (None, seeded.source.clone(), changed_parameters),
        (None, "another-source-revision".to_owned(), changed_guard),
    ] {
        let outcome = save_recipe(
            &seeded,
            save("raw-photo", "first-save", expected, &source, recipe_changed),
        )
        .await;
        assert_eq!(outcome, ComposableEditRecipeWriteOutcome::RequestConflict);
    }
    assert_eq!(read_recipe(&seeded, "raw-photo").await.unwrap(), committed);
}

#[tokio::test]
async fn unchanged_content_does_not_mint_a_new_revision() {
    let seeded = seeded_persistence();
    let first = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("tone"),
    );
    let committed = match save_recipe(
        &seeded,
        save("raw-photo", "first-save", None, &seeded.source, first),
    )
    .await
    {
        ComposableEditRecipeWriteOutcome::Saved(committed) => committed,
        outcome => panic!("first save should commit, got {outcome:?}"),
    };

    // The same content under a new request identity and a different
    // caller snapshot label stays one committed recipe.
    let same = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("tone"),
    );
    let outcome = save_recipe(
        &seeded,
        save(
            "raw-photo",
            "same-save",
            Some(&committed.revision),
            &seeded.source,
            same,
        ),
    )
    .await;
    assert_eq!(
        outcome,
        ComposableEditRecipeWriteOutcome::Unchanged(committed.clone())
    );
    assert_eq!(read_recipe(&seeded, "raw-photo").await.unwrap(), committed);
}

#[tokio::test]
async fn stale_recipe_and_source_guards_refuse_with_the_stored_recipe() {
    let seeded = seeded_persistence();
    let first = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("tone"),
    );
    let committed = match save_recipe(
        &seeded,
        save("raw-photo", "first-save", None, &seeded.source, first),
    )
    .await
    {
        ComposableEditRecipeWriteOutcome::Saved(committed) => committed,
        outcome => panic!("first save should commit, got {outcome:?}"),
    };

    // A save that never observed the committed revision conflicts.
    let stale_revision = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "a",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("a"),
    );
    assert_eq!(
        save_recipe(
            &seeded,
            save(
                "raw-photo",
                "stale-revision-save",
                None,
                &seeded.source,
                stale_revision,
            ),
        )
        .await,
        ComposableEditRecipeWriteOutcome::Conflict(Some(committed.clone()))
    );

    // A moved source revision refuses even a matching recipe revision:
    // the caller still guards the revision it observed before the move.
    let connection = Connection::open(&seeded.path).unwrap();
    let changed_source = source_revision("shoot/one.ARW", 18, 2_000.0).unwrap();
    connection
        .execute(
            "UPDATE original_files SET size=18,mtime_ms=2_000.0,capture_source_revision=? WHERE id='raw-original'",
            [format!("{}\0fixture-device\0fixture-inode", changed_source)],
        )
        .unwrap();
    drop(connection);
    let stale_source = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("tone"),
    );
    assert_eq!(
        save_recipe(
            &seeded,
            save(
                "raw-photo",
                "stale-source-save",
                Some(&committed.revision),
                &seeded.source,
                stale_source,
            ),
        )
        .await,
        ComposableEditRecipeWriteOutcome::SourceChanged(Some(committed.clone()))
    );
    assert_eq!(read_recipe(&seeded, "raw-photo").await.unwrap(), committed);
}

#[tokio::test]
async fn missing_photo_and_unavailable_sources_refuse() {
    let seeded = seeded_persistence();
    let orphan = recipe("absent-photo", &seeded.source, vec![], None);
    assert_eq!(
        save_recipe(
            &seeded,
            save("absent-photo", "orphan-save", None, &seeded.source, orphan),
        )
        .await,
        ComposableEditRecipeWriteOutcome::MissingPhoto
    );
    assert_eq!(read_recipe(&seeded, "absent-photo").await, None);

    let unavailable = recipe(
        "raw-photo",
        &seeded.source,
        vec![step(
            "tone",
            "darktable",
            original_input("raw-photo", &seeded.source),
        )],
        Some("tone"),
    );
    let connection = Connection::open(&seeded.path).unwrap();
    connection
        .execute(
            "UPDATE original_files SET available=0 WHERE id='raw-original'",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        save_recipe(
            &seeded,
            save(
                "raw-photo",
                "unavailable-save",
                None,
                &seeded.source,
                unavailable,
            ),
        )
        .await,
        ComposableEditRecipeWriteOutcome::Unavailable
    );
}

#[tokio::test]
async fn original_and_artifact_input_identities_survive_the_round_trip() {
    let seeded = seeded_persistence();
    let original_bound = original_input("raw-photo", &seeded.source);
    let artifact_bound = artifact_input("artifact-7");
    let composed = recipe(
        "raw-photo",
        &seeded.source,
        vec![
            step("film", "spektrafilm", artifact_bound.clone()),
            step("tone", "darktable", original_bound.clone()),
        ],
        Some("film"),
    );
    let committed = match save_recipe(
        &seeded,
        save("raw-photo", "mixed-save", None, &seeded.source, composed),
    )
    .await
    {
        ComposableEditRecipeWriteOutcome::Saved(committed) => committed,
        outcome => panic!("mixed save should commit, got {outcome:?}"),
    };
    let stored = read_recipe(&seeded, "raw-photo").await.unwrap();
    // Canonical storage order is by step_id: "film" sorts before "tone"
    // whatever list order the caller composed them in.
    assert_eq!(stored.steps[0].step_id.as_str(), "film");
    assert_eq!(stored.steps[0].input, artifact_bound);
    assert_eq!(stored.steps[1].step_id.as_str(), "tone");
    assert_eq!(stored.steps[1].input, original_bound);
    assert_eq!(stored, committed);

    // The two bindings never merge or convert: an Original stays bound
    // to its Photo and source revision, and an artifact stays bound to
    // its immutable artifact identity and full contract.
    assert_eq!(
        committed.steps[0].input.binding_digest(),
        artifact_bound.binding_digest()
    );
    assert_eq!(
        committed.steps[1].input.binding_digest(),
        original_bound.binding_digest()
    );
    let other_artifact = artifact_input("artifact-8");
    assert_ne!(
        other_artifact.binding_digest(),
        artifact_bound.binding_digest()
    );
}

#[tokio::test]
async fn malformed_stored_records_are_storage_errors() {
    let seeded = seeded_persistence();
    let connection = Connection::open(&seeded.path).unwrap();
    for (key, value) in [
        (
            "composable_edit_recipe:truncated",
            "{\"photo_id\":\"raw-photo\",\"revision\":\"r\",",
        ),
        ("composable_edit_recipe:broken-json", "not json"),
        (
            "composable_edit_recipe:raw-photo",
            "{\"photo_id\":\"raw-photo\",\"revision\":\"r\",\"source_revision\":\"s\",\"steps\":[],\"current_step_id\":null,\"extra\":1}",
        ),
    ] {
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES(?,?)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![key, value],
            )
            .unwrap();
    }
    // A record that parses but fails the contract vocabulary: two steps
    // sharing one step_id.
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES('composable_edit_recipe:contract-failure',?)",
            rusqlite::params![
                "{\"photo_id\":\"p\",\"revision\":\"r\",\"source_revision\":\"s\",\"steps\":[{\"step_id\":\"a\",\"module\":\"darktable\",\"input\":{\"Original\":{\"photo_id\":\"p\",\"source_revision\":\"s\"}},\"parameters\":{\"schema_version\":\"1\",\"tree\":{}}},{\"step_id\":\"a\",\"module\":\"darktable\",\"input\":{\"Original\":{\"photo_id\":\"p\",\"source_revision\":\"s\"}},\"parameters\":{\"schema_version\":\"1\",\"tree\":{}}}],\"current_step_id\":\"a\"}"
            ],
        )
        .unwrap();
    drop(connection);

    for photo_id in ["raw-photo", "truncated", "broken-json", "contract-failure"] {
        let outcome = seeded
            .persistence
            .composable_edit_recipe_receiver(photo_id)
            .unwrap()
            .await
            .unwrap();
        assert!(
            matches!(outcome, Err(crate::persistence::PersistenceError::Storage)),
            "malformed record for {photo_id} should be a storage error, got {outcome:?}"
        );
    }
}
