use super::*;
use rusqlite::{Connection, params};

#[path = "recovery_safety.rs"]
mod recovery_safety;
/// Deterministic manual-recovery fixture: one unavailable Photo with
/// retained decisions and Album membership, its file relocated on disk,
/// and optionally an occupying destination record.
fn manual_recovery_fixture(
    occupant_state: Option<(&'static str, u8)>,
    fingerprint: Option<bool>,
) -> (TempTree, LibraryConfig) {
    manual_recovery_fixture_with_candidate(occupant_state, fingerprint, CandidateFile::Readable)
}

/// What the proposed destination Location holds on disk.
#[derive(Clone, Copy)]
enum CandidateFile {
    /// One generated readable JPEG, the ordinary recovered candidate.
    Readable,
    /// No entry at all, as for a destination that was never written.
    Absent,
    /// A generated directory carrying the candidate filename, which no
    /// Original read can use.
    NotRegular,
}

/// [`manual_recovery_fixture`] with explicit control over the destination
/// entry, for candidate states the scanner would not have discovered.
fn manual_recovery_fixture_with_candidate(
    occupant_state: Option<(&'static str, u8)>,
    fingerprint: Option<bool>,
    candidate: CandidateFile,
) -> (TempTree, LibraryConfig) {
    let (base, config) = fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("moved")).unwrap();
    match candidate {
        CandidateFile::Readable => {
            fs::write(root.join("moved/a.JPG"), b"jpeg-bytes-a").unwrap();
        }
        CandidateFile::Absent => {}
        CandidateFile::NotRegular => {
            fs::create_dir(root.join("moved/a.JPG")).unwrap();
        }
    }
    fs::create_dir_all(&config.state_directory).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config.state_directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let database = config.state_directory.join("library.sqlite");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!(
            "../../../../../compatibility/sqlite/schema-v8.sql"
        ))
        .unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata VALUES('canonical_root',?)",
            [root.to_str().unwrap()],
        )
        .unwrap();
    let missing_id = original_id("shoot/a.JPG");
    connection
            .execute(
                "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state,capture_source_revision) VALUES(?,'shoot/a.JPG','jpeg',11,1.0,0,'missing','remembered-revision')",
                params![missing_id],
            )
            .unwrap();
    connection
            .execute(
                "INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating) VALUES('missing-photo',?,0,'unavailable','shoot/a.JPG','selected',3)",
                params![missing_id],
            )
            .unwrap();
    connection
        .execute("INSERT INTO albums VALUES('set','Trip',1)", [])
        .unwrap();
    connection
        .execute(
            "INSERT INTO album_members VALUES('set','missing-photo',0)",
            [],
        )
        .unwrap();
    if let Some((occupant_path, rating)) = occupant_state {
        let occupant_original = original_id(occupant_path);
        connection
                .execute(
                    "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state) VALUES(?,?,'jpeg',9,1.0,1,'pending')",
                    params![occupant_original, occupant_path],
                )
                .unwrap();
        connection
                .execute(
                    "INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating) VALUES('occupant-photo',?,1,'inspection-pending',?,'undecided',?)",
                    params![occupant_original, occupant_path, i64::from(rating)],
                )
                .unwrap();
    }
    if fingerprint == Some(true) {
        connection
                .execute(
                    "INSERT INTO original_fingerprints(original_id,digest,size,mtime_ms) VALUES(?,?,11,1.0)",
                    params![missing_id, crate::digest_bytes(b"jpeg-bytes-a")],
                )
                .unwrap();
    }
    drop(connection);
    (base, config)
}

fn requested_relocation(
    proposal: &crate::ManualProposal,
    survey: &crate::RecoverySurvey,
    facts: crate::OriginalFacts,
    retire_photo_id: Option<&str>,
) -> crate::RequestedRelocation {
    crate::RequestedRelocation {
        original_id: proposal.original_id.clone(),
        from_location: proposal.from_location.clone(),
        to_location: proposal.to_location.clone(),
        mapping_id: proposal.mapping_id.clone(),
        fingerprint: survey
            .unavailable
            .iter()
            .find(|record| record.original_id == proposal.original_id)
            .and_then(|record| record.fingerprint.clone()),
        facts,
        retire_photo_id: retire_photo_id.map(str::to_owned),
    }
}

#[tokio::test]
async fn manual_recovery_restores_unavailable_photo_without_fingerprint() {
    let (base, config) = manual_recovery_fixture(None, Some(false));
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    assert_eq!(survey.unavailable.len(), 1);
    let record = &survey.unavailable[0];
    assert_eq!(record.relative_path, "shoot/a.JPG");
    assert_eq!(record.rating, 3);
    assert!(record.fingerprint.is_none());
    assert_eq!(record.album_count, 1);

    let root = LibraryRoot::open(config.library_root.clone()).unwrap();
    let budget = NativeWorkBudget::new();
    let snapshot = library.snapshot().await.unwrap();
    let proposals =
        crate::plan_manual_relocations(&root, &budget, &survey, &snapshot, "shoot", "moved")
            .unwrap();
    assert_eq!(proposals.len(), 1);
    assert!(matches!(
        proposals[0].outcome,
        crate::ManualOutcome::Matched
    ));
    assert!(!proposals[0].verified);
    assert_eq!(proposals[0].to_location, "moved/a.JPG");

    let capability = root
        .original(crate::RelativeOriginalPath::parse("moved/a.JPG").unwrap())
        .unwrap();
    let facts = capability.facts().unwrap();
    let applied = library
        .apply_relocations(vec![requested_relocation(
            &proposals[0],
            &survey,
            facts,
            None,
        )])
        .await
        .unwrap();
    assert_eq!(applied.relocated_photos, 1);
    assert_eq!(applied.unavailable_photos, 0);

    let snapshot = library.snapshot().await.unwrap();
    let photo = snapshot
        .photos
        .iter()
        .find(|photo| photo.id == "missing-photo")
        .unwrap();
    assert!(photo.available);
    assert_eq!(photo.rating, 3);
    assert_eq!(photo.selection_state, crate::SelectionState::Picked);
    let original = snapshot
        .originals
        .iter()
        .find(|original| original.relative_path.as_str() == "moved/a.JPG")
        .unwrap();
    assert!(original.available);
    // The remembered Location is gone.
    assert!(
        !snapshot
            .originals
            .iter()
            .any(|original| original.relative_path.as_str() == "shoot/a.JPG")
    );
    library.shutdown().unwrap();
    drop(base);
}

#[tokio::test]
async fn manual_recovery_verifies_fingerprinted_originals() {
    let (base, config) = manual_recovery_fixture(None, Some(true));
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    assert!(survey.unavailable[0].fingerprint.is_some());
    let root = LibraryRoot::open(config.library_root.clone()).unwrap();
    let snapshot = library.snapshot().await.unwrap();
    let proposals = crate::plan_manual_relocations(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        "shoot",
        "moved",
    )
    .unwrap();
    assert_eq!(proposals.len(), 1);
    assert!(matches!(
        proposals[0].outcome,
        crate::ManualOutcome::Matched
    ));
    assert!(proposals[0].verified);

    // Different content at the candidate fails verification instead of
    // silently rebinding identity.
    fs::write(config.library_root.join("moved/a.JPG"), b"other-bytes").unwrap();
    let proposals = crate::plan_manual_relocations(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        "shoot",
        "moved",
    )
    .unwrap();
    assert!(matches!(
        proposals[0].outcome,
        crate::ManualOutcome::ContentMismatch
    ));
    library.shutdown().unwrap();
    drop(base);
}

#[tokio::test]
async fn manual_recovery_retires_only_unreferenced_default_destination() {
    let (base, config) = manual_recovery_fixture(Some(("moved/a.JPG", 0)), Some(false));
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    let root = LibraryRoot::open(config.library_root.clone()).unwrap();
    let snapshot = library.snapshot().await.unwrap();
    let proposals = crate::plan_manual_relocations(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        "shoot",
        "moved",
    )
    .unwrap();
    assert!(matches!(
        proposals[0].outcome,
        crate::ManualOutcome::Occupied { retire: Some(_) }
    ));
    let capability = root
        .original(crate::RelativeOriginalPath::parse("moved/a.JPG").unwrap())
        .unwrap();
    let facts = capability.facts().unwrap();
    let relocation = requested_relocation(&proposals[0], &survey, facts, None);
    assert!(library.apply_relocations(vec![relocation]).await.is_err());

    let applied = library
        .apply_relocations(vec![requested_relocation(
            &proposals[0],
            &survey,
            facts,
            Some("occupant-photo"),
        )])
        .await
        .unwrap();
    assert_eq!(applied.relocated_photos, 1);
    let snapshot = library.snapshot().await.unwrap();
    // The occupier's Photo and Original rows are retired; the relocated
    // Photo keeps its identity and decisions.
    assert!(
        !snapshot
            .photos
            .iter()
            .any(|photo| photo.id == "occupant-photo")
    );
    let photo = snapshot
        .photos
        .iter()
        .find(|photo| photo.id == "missing-photo")
        .unwrap();
    assert!(photo.available);
    assert_eq!(photo.rating, 3);
    library.shutdown().unwrap();
    drop(base);
}

#[tokio::test]
async fn manual_recovery_refuses_retire_with_user_state() {
    let (base, config) = manual_recovery_fixture(Some(("moved/a.JPG", 4)), Some(false));
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    let root = LibraryRoot::open(config.library_root.clone()).unwrap();
    let snapshot = library.snapshot().await.unwrap();
    let proposals = crate::plan_manual_relocations(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        "shoot",
        "moved",
    )
    .unwrap();
    assert!(matches!(
        proposals[0].outcome,
        crate::ManualOutcome::Occupied { retire: None }
    ));
    let capability = root
        .original(crate::RelativeOriginalPath::parse("moved/a.JPG").unwrap())
        .unwrap();
    let facts = capability.facts().unwrap();
    assert!(
        library
            .apply_relocations(vec![requested_relocation(
                &proposals[0],
                &survey,
                facts,
                Some("occupant-photo"),
            )])
            .await
            .is_err()
    );
    // Both records survive the refusal.
    let snapshot = library.snapshot().await.unwrap();
    assert!(
        snapshot
            .photos
            .iter()
            .any(|photo| photo.id == "occupant-photo")
    );
    assert!(
        snapshot
            .photos
            .iter()
            .any(|photo| photo.id == "missing-photo")
    );
    library.shutdown().unwrap();
    drop(base);
}

#[tokio::test]
async fn manual_recovery_rejects_stale_and_colliding_batches() {
    let (base, config) = manual_recovery_fixture(None, Some(false));
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    let root = LibraryRoot::open(config.library_root.clone()).unwrap();
    let capability = root
        .original(crate::RelativeOriginalPath::parse("moved/a.JPG").unwrap())
        .unwrap();
    let facts = capability.facts().unwrap();
    let original_id = survey.unavailable[0].original_id.clone();
    let proposal = crate::plan_single_relocation(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &library.snapshot().await.unwrap(),
        &original_id,
        "moved/a.JPG",
    )
    .unwrap();
    // Colliding destinations reject the whole batch.
    assert!(
        library
            .apply_relocations(vec![
                requested_relocation(&proposal, &survey, facts, None),
                crate::RequestedRelocation {
                    original_id: "unknown-original".to_owned(),
                    from_location: "shoot/a.JPG".to_owned(),
                    to_location: "moved/a.JPG".to_owned(),
                    mapping_id: "unknown-mapping".to_owned(),
                    fingerprint: None,
                    facts,
                    retire_photo_id: None,
                },
            ])
            .await
            .is_err()
    );
    // An empty batch is not a recovery.
    assert!(library.apply_relocations(vec![]).await.is_err());
    library.shutdown().unwrap();
    drop(base);
}

#[tokio::test]
async fn manual_recovery_rejects_duplicate_source_mappings() {
    let (base, config) = manual_recovery_fixture(None, Some(false));
    fs::write(base.0.join("originals/moved/b.JPG"), b"jpeg-bytes-b").unwrap();
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    let root = LibraryRoot::open(config.library_root.clone()).unwrap();
    let first = root
        .original(crate::RelativeOriginalPath::parse("moved/a.JPG").unwrap())
        .unwrap();
    let second = root
        .original(crate::RelativeOriginalPath::parse("moved/b.JPG").unwrap())
        .unwrap();
    let original_id = survey.unavailable[0].original_id.clone();
    let snapshot = library.snapshot().await.unwrap();
    let first_proposal = crate::plan_single_relocation(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        &original_id,
        "moved/a.JPG",
    )
    .unwrap();
    let second_proposal = crate::plan_single_relocation(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        &original_id,
        "moved/b.JPG",
    )
    .unwrap();
    // Two mappings for one Original File are a colliding batch: only one
    // Location could win, so the whole batch is refused with a reason.
    let error = match library
        .apply_relocations(vec![
            requested_relocation(&first_proposal, &survey, first.facts().unwrap(), None),
            requested_relocation(&second_proposal, &survey, second.facts().unwrap(), None),
        ])
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("duplicate source mappings must be rejected"),
    };
    assert!(matches!(
        error,
        LibraryError::Persistence(PersistenceError::InvalidRecoveryMapping {
            reason: "colliding",
            ..
        })
    ));
    // The refusal leaves the Library untouched.
    let snapshot = library.snapshot().await.unwrap();
    let photo = snapshot
        .photos
        .iter()
        .find(|photo| photo.id == "missing-photo")
        .unwrap();
    assert!(!photo.available);
    library.shutdown().unwrap();
    drop(base);
}

#[tokio::test]
async fn manual_recovery_reports_verified_content_behind_an_occupied_destination() {
    let (base, config) = manual_recovery_fixture(Some(("moved/a.JPG", 4)), Some(true));
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    let snapshot = library.snapshot().await.unwrap();
    let proposals = crate::plan_manual_relocations(
        &LibraryRoot::open(config.library_root.clone()).unwrap(),
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        "shoot",
        "moved",
    )
    .unwrap();
    assert!(matches!(
        proposals[0].outcome,
        crate::ManualOutcome::Occupied { retire: None }
    ));
    // The destination content matched the persisted fingerprint, so the
    // proposal reports verification even without a permitted retire.
    assert!(proposals[0].verified);
    library.shutdown().unwrap();
    drop(base);
}

/// A fingerprint-less proposal cannot fall back on remembered content, so
/// it must establish that the destination really holds a readable
/// Original. An absent unowned Location is not a match.
#[tokio::test]
async fn manual_recovery_reports_missing_destination_without_fingerprint() {
    let (base, config) =
        manual_recovery_fixture_with_candidate(None, Some(false), CandidateFile::Absent);
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    assert!(survey.unavailable[0].fingerprint.is_none());
    let root = LibraryRoot::open(config.library_root.clone()).unwrap();
    let snapshot = library.snapshot().await.unwrap();
    let proposals = crate::plan_manual_relocations(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        "shoot",
        "moved",
    )
    .unwrap();
    assert_eq!(proposals.len(), 1);
    assert!(matches!(
        proposals[0].outcome,
        crate::ManualOutcome::Missing
    ));
    assert!(!proposals[0].verified);

    // The single-mapping path reads the same Location and reports the
    // same truth.
    let single = crate::plan_single_relocation(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        &survey.unavailable[0].original_id,
        "moved/a.JPG",
    )
    .unwrap();
    assert!(matches!(single.outcome, crate::ManualOutcome::Missing));
    assert!(!single.verified);
    library.shutdown().unwrap();
    drop(base);
}

/// A remembered destination owner cannot turn an absent Location into a
/// retireable occupant: nothing occupies a Location with no file.
#[tokio::test]
async fn manual_recovery_reports_missing_occupied_destination_without_retirement() {
    let (base, config) = manual_recovery_fixture_with_candidate(
        Some(("moved/a.JPG", 0)),
        Some(false),
        CandidateFile::Absent,
    );
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    let snapshot = library.snapshot().await.unwrap();
    let root = LibraryRoot::open(config.library_root.clone()).unwrap();
    let proposals = crate::plan_manual_relocations(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        "shoot",
        "moved",
    )
    .unwrap();
    assert_eq!(proposals.len(), 1);
    // The occupant is remembered with default decisions, so a readable
    // candidate here would offer retirement. With no file at the
    // Location the proposal refuses instead.
    assert!(matches!(
        proposals[0].outcome,
        crate::ManualOutcome::Missing
    ));
    assert!(!proposals[0].verified);
    library.shutdown().unwrap();
    drop(base);
}

/// An inaccessible candidate cannot be judged, so it is not a match and
/// offers no retirement even when a remembered occupant qualifies.
#[tokio::test]
async fn manual_recovery_reports_inaccessible_destination_without_fingerprint() {
    let (base, config) = manual_recovery_fixture_with_candidate(
        Some(("moved/a.JPG", 0)),
        Some(false),
        CandidateFile::Readable,
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            config.library_root.join("moved/a.JPG"),
            fs::Permissions::from_mode(0o000),
        )
        .unwrap();
    }
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    let snapshot = library.snapshot().await.unwrap();
    let proposals = crate::plan_manual_relocations(
        &LibraryRoot::open(config.library_root.clone()).unwrap(),
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        "shoot",
        "moved",
    )
    .unwrap();
    assert!(matches!(
        proposals[0].outcome,
        crate::ManualOutcome::Unreadable
    ));
    assert!(!proposals[0].verified);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(
            config.library_root.join("moved/a.JPG"),
            fs::Permissions::from_mode(0o644),
        );
    }
    library.shutdown().unwrap();
    drop(base);
}

/// A non-regular entry at the destination is not a usable Original, so
/// the proposal refuses it instead of offering a match.
#[tokio::test]
async fn manual_recovery_reports_non_regular_destination_without_fingerprint() {
    let (base, config) =
        manual_recovery_fixture_with_candidate(None, Some(false), CandidateFile::NotRegular);
    let library = Library::open(config.clone()).unwrap();
    let survey = library.recovery_survey().await.unwrap();
    let snapshot = library.snapshot().await.unwrap();
    let proposals = crate::plan_manual_relocations(
        &LibraryRoot::open(config.library_root.clone()).unwrap(),
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        "shoot",
        "moved",
    )
    .unwrap();
    assert!(matches!(
        proposals[0].outcome,
        crate::ManualOutcome::Unreadable
    ));
    assert!(!proposals[0].verified);
    library.shutdown().unwrap();
    drop(base);
}

fn seed_fingerprint(base: &TempTree, relative_path: &str, bytes: &[u8]) {
    let connection = Connection::open(base.0.join("state").join("library.sqlite")).unwrap();
    let original_id: String = connection
        .query_row(
            "SELECT id FROM original_files WHERE relative_path=?",
            [relative_path],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute(
            "INSERT OR REPLACE INTO original_fingerprints(original_id,digest,size,mtime_ms) \
                 VALUES(?,?,?,1.0)",
            params![
                original_id,
                crate::digest_bytes(bytes),
                i64::try_from(bytes.len()).unwrap()
            ],
        )
        .unwrap();
}

fn seed_legacy_edit_recipe(
    base: &TempTree,
    photo_id: &str,
    revision: &str,
    source_revision: &str,
    exposure_ev: f64,
) {
    let connection = Connection::open(base.0.join("state").join("library.sqlite")).unwrap();
    connection
        .execute(
            "INSERT OR REPLACE INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode) VALUES(?,?,?,?,?)",
            params![photo_id, revision, source_revision, exposure_ev, "as-shot"],
        )
        .unwrap();
}

#[tokio::test]
async fn scan_recovery_follows_a_unique_exact_candidate() {
    let (base, initial_config) = fixture();
    let raw_bytes = raw_capture_fixture("2026:02:03 04:05:06");
    fs::write(base.0.join("originals/a.ARW"), &raw_bytes).unwrap();
    let library = Library::open(initial_config).unwrap();
    let first = library.scan().await.unwrap();
    let photo_id = first.photos[0].id.clone();
    let current = library.edit_recipe(&photo_id).await.unwrap().unwrap();
    let original_source_revision = current.current_source_revision.expect("published source");
    library.shutdown().unwrap();
    seed_legacy_edit_recipe(
        &base,
        &photo_id,
        "library-first-save",
        &original_source_revision,
        0.0,
    );
    let saved = crate::EditRecipe {
        photo_id: photo_id.clone(),
        revision: "library-first-save".to_owned(),
        source_revision: original_source_revision,
        settings: crate::EditRecipeSettings {
            exposure_ev: 0.0,
            white_balance: crate::WhiteBalanceIntent::AsShot,
        },
    };
    seed_fingerprint(&base, "a.ARW", &raw_bytes);
    fs::create_dir(base.0.join("originals/moved")).unwrap();
    fs::rename(
        base.0.join("originals/a.ARW"),
        base.0.join("originals/moved/a.ARW"),
    )
    .unwrap();
    let library = Library::open(config(&base)).unwrap();
    let second = library.scan().await.unwrap();
    assert_eq!(second.photos.len(), 1);
    assert_eq!(second.photos[0].id, photo_id);
    assert!(second.photos[0].available);
    assert!(second.photos[0].has_saved_edits);
    assert!(
        library
            .photo(&photo_id)
            .await
            .unwrap()
            .unwrap()
            .has_saved_edits
    );
    assert!(
        second
            .originals
            .iter()
            .any(|original| original.relative_path.as_str() == "moved/a.ARW")
    );
    let recovered = library.edit_recipe(&photo_id).await.unwrap().unwrap();
    let recovered_recipe = recovered.recipe.unwrap();
    assert_eq!(recovered_recipe.revision, saved.revision);
    assert_eq!(recovered_recipe.settings, saved.settings);
    assert_ne!(
        recovered.current_source_revision.as_deref(),
        Some(recovered_recipe.source_revision.as_str())
    );
    library.shutdown().unwrap();
    drop(base);
}

#[cfg(unix)]
#[tokio::test]
async fn scan_recovery_treats_unreadable_same_kind_candidates_as_ambiguous() {
    use std::os::unix::fs::PermissionsExt;
    let (base, initial_config) = fixture();
    fs::write(base.0.join("originals/a.JPG"), b"jpeg-bytes-a").unwrap();
    let library = Library::open(initial_config).unwrap();
    let first = library.scan().await.unwrap();
    let photo_id = first.photos[0].id.clone();
    library.shutdown().unwrap();
    seed_fingerprint(&base, "a.JPG", b"jpeg-bytes-a");
    fs::create_dir_all(base.0.join("originals/moved")).unwrap();
    fs::rename(
        base.0.join("originals/a.JPG"),
        base.0.join("originals/moved/a.JPG"),
    )
    .unwrap();
    let locked = base.0.join("originals/locked/b.JPG");
    fs::create_dir_all(base.0.join("originals/locked")).unwrap();
    fs::write(&locked, b"jpeg-bytes-a").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let library = Library::open(config(&base)).unwrap();
    let second = library.scan().await.unwrap();
    let photo = second
        .photos
        .iter()
        .find(|photo| photo.id == photo_id)
        .unwrap();
    // The unreadable same-kind candidate could hold the same content,
    // so the scan must not treat uniqueness as proven.
    assert!(!photo.available);
    assert!(
        second
            .originals
            .iter()
            .any(|original| original.relative_path.as_str() == "a.JPG" && !original.available)
    );
    library.shutdown().unwrap();
    let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o644));
    drop(base);
}
