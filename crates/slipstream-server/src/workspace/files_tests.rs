use super::*;
use sha2::Digest;
#[test]
fn known_truncated_images_cannot_be_generic_attachment() {
    assert!(known_image("output.JPG", b"bad"));
    assert!(known_image("file", b"\x89PNG\r\n\x1a\n"));
    assert!(!known_image("notes.txt", b"plain"));
    assert!(known_image("external.arw", b""));
}
#[test]
fn replacement_and_truncation_invalidate_checked_descriptor() {
    let root = std::env::temp_dir().join(identity("workspace-test").unwrap());
    let directory = photo_files::Directory::create(&root).unwrap();
    let mut writer = directory.create_file("result", 0o600).unwrap();
    std::io::Write::write_all(&mut writer, b"complete original").unwrap();
    let mut held = directory.open_file("result").unwrap();
    let checked = fingerprint(&mut held, 100).unwrap();
    directory.remove_file("result").unwrap();
    let mut replacement = directory.create_file("result", 0o600).unwrap();
    std::io::Write::write_all(&mut replacement, b"complete original").unwrap();
    assert!(
        checked
            .observation
            .verify(&directory.open_file("result").unwrap())
            .is_err()
    );
    assert_eq!(fingerprint(&mut held, 100).unwrap().sha256, checked.sha256);
    let current = fingerprint(&mut directory.open_file("result").unwrap(), 100).unwrap();
    replacement.set_len(3).unwrap();
    assert!(current.observation.verify(&replacement).is_err());
    directory.remove_file("result").unwrap();
    std::fs::remove_dir(root).unwrap();
}
struct Fixture {
    root: std::path::PathBuf,
    manager: WorkspaceManager,
    workspace: Workspace,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(identity("workspace-manager-test").unwrap());
        Directory::create(&root).unwrap();
        let manager = WorkspaceManager::open(
            &root,
            root.clone(),
            std::sync::Arc::new(|_| None),
            1024 * 1024 * 1024,
        )
        .unwrap();
        let workspace = manager.create_workspace("photo-test").unwrap();
        Self {
            root,
            manager,
            workspace,
        }
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        let directory = self.manager.directory(&self.workspace.id).unwrap();
        let mut file = directory.create_file(path, 0o600).unwrap();
        std::io::Write::write_all(&mut file, bytes).unwrap();
    }
    fn items(&self) -> Vec<Value> {
        let mut session = self
            .manager
            .locked(&self.workspace.id, LeaseMode::Shared)
            .unwrap();
        reconcile(&self.manager, &mut session).unwrap();
        all_files(&session).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
#[test]
fn independent_files_missing_and_replacement_keep_exact_identity_and_revoke_provenance() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", b"initial");
    fixture.write("bad.png", b"\x89PNG");
    fixture.write(".private", b"hidden");
    let items = fixture.items();
    assert_eq!(items.len(), 2);
    assert_eq!(
        items
            .iter()
            .find(|v| v["relativePath"] == "bad.png")
            .unwrap()["state"],
        "unavailable"
    );
    let initial = items
        .iter()
        .find(|v| v["relativePath"] == "notes.txt")
        .unwrap();
    let id = initial["artifactId"].as_str().unwrap().to_owned();
    let revision = initial["revision"].clone();
    {
        let session = fixture
            .manager
            .locked(&fixture.workspace.id, LeaseMode::Shared)
            .unwrap();
        session
            .db
            .execute(
                "UPDATE files SET provenance='{}',managed=1 WHERE id=?",
                [&id],
            )
            .unwrap();
        session.directory.remove_file("notes.txt").unwrap();
    }
    assert_eq!(
        fixture
            .items()
            .iter()
            .find(|v| v["artifactId"] == id)
            .unwrap()["state"],
        "missing"
    );
    fixture.write("notes.txt", b"initial");
    let replaced = fixture
        .items()
        .into_iter()
        .find(|v| v["artifactId"] == id)
        .unwrap();
    assert_ne!(replaced["revision"], revision);
    assert_eq!(replaced["managed"], false);
    assert!(replaced["provenance"].is_null());
}
#[test]
fn removal_replay_preserves_new_same_path_identity_and_default_stays_empty() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", b"first");
    let first = fixture.items().pop().unwrap();
    let id = first["artifactId"].as_str().unwrap();
    let revision = first["revision"].as_str().unwrap();
    {
        let session = fixture
            .manager
            .locked(&fixture.workspace.id, LeaseMode::Shared)
            .unwrap();
        store::set_setting(&session, "defaultInput", &json!(id)).unwrap();
    }
    let removed = fixture
        .manager
        .remove(
            &fixture.workspace.id,
            id,
            Some(revision),
            Some("remove-first"),
        )
        .unwrap();
    assert_eq!(removed["outcome"], "removed");
    assert!(removed["inputArtifactId"].is_null());
    assert!(fixture.items().is_empty());
    fixture.write("notes.txt", b"second");
    let current = fixture.items().pop().unwrap();
    assert_ne!(current["artifactId"], id);
    let replayed = fixture
        .manager
        .remove(
            &fixture.workspace.id,
            id,
            Some(revision),
            Some("remove-first"),
        )
        .unwrap();
    assert_eq!(replayed["outcome"], "replayed");
    assert_eq!(replayed["currentAvailability"], "removed");
    assert_eq!(fixture.items()[0]["artifactId"], current["artifactId"]);
    assert_eq!(
        fixture
            .manager
            .remove(&fixture.workspace.id, id, None, Some("remove-first"))
            .unwrap_err()
            .code,
        "request_conflict"
    );
}
#[test]
fn kept_job_blocks_removal_and_new_file_invalidates_reviewed_delete() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", b"retained");
    let item = fixture.items().pop().unwrap();
    let id = item["artifactId"].as_str().unwrap();
    let revision = item["revision"].as_str().unwrap();
    fixture
        .manager
        .register_dependency(&fixture.workspace.id, id, revision, "job-1", "job")
        .unwrap();
    let error = fixture
        .manager
        .remove(
            &fixture.workspace.id,
            id,
            Some(revision),
            Some("remove-job"),
        )
        .unwrap_err();
    assert_eq!(error.code, "artifact_referenced");
    assert_eq!(error.details["blockers"][0]["name"], "job-1");
    fixture.manager.release_dependency("job-1").unwrap();
    let reviewed = {
        let session = fixture
            .manager
            .locked(&fixture.workspace.id, LeaseMode::Shared)
            .unwrap();
        let inventory = dependencies::inventory(&fixture.manager, &session).unwrap();
        token(&session, &inventory).unwrap()
    };
    fixture.write("new.txt", b"new loss");
    assert_eq!(
        fixture
            .manager
            .delete(&fixture.workspace.id, &reviewed, "delete-1", true)
            .unwrap_err()
            .code,
        "workspace_changed"
    );
    let reviewed = {
        let mut session = fixture
            .manager
            .locked(&fixture.workspace.id, LeaseMode::Shared)
            .unwrap();
        reconcile(&fixture.manager, &mut session).unwrap();
        let inventory = dependencies::inventory(&fixture.manager, &session).unwrap();
        token(&session, &inventory).unwrap()
    };
    let deleted = fixture
        .manager
        .delete(&fixture.workspace.id, &reviewed, "delete-2", true)
        .unwrap();
    assert_eq!(deleted["reclamation"], "complete");
    assert!(fixture.manager.directory(&fixture.workspace.id).is_err());
    let replayed = fixture
        .manager
        .delete(&fixture.workspace.id, &reviewed, "delete-2", true)
        .unwrap();
    assert_eq!(replayed["outcome"], "replayed");
}
#[test]
fn whole_deletion_refuses_standalone_input_lease_before_withdrawal() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", b"independent input");
    fixture.items();
    let reviewed = {
        let session = fixture
            .manager
            .locked(&fixture.workspace.id, LeaseMode::Shared)
            .unwrap();
        let inventory = dependencies::inventory(&fixture.manager, &session).unwrap();
        token(&session, &inventory).unwrap()
    };
    let directory = fixture.manager.directory(&fixture.workspace.id).unwrap();
    // No outside state directory or dependency row: the supported input file
    // lease alone must prevent registry withdrawal and unlink.
    let input = photo_files::FileLease::acquire(
        directory.open_file("notes.txt").unwrap(),
        LeaseMode::Shared,
        std::time::Duration::ZERO,
    )
    .unwrap();
    let refused = fixture
        .manager
        .delete(
            &fixture.workspace.id,
            &reviewed,
            "delete-leased-input",
            true,
        )
        .unwrap_err();
    assert_eq!(refused.code, "workspace_referenced");
    assert_eq!(refused.details["blockers"][0]["kind"], "active-use");
    assert!(fixture.manager.lookup(&fixture.workspace.id).is_ok());
    assert!(directory.open_file("notes.txt").is_ok());
    drop(input);
    let deleted = fixture
        .manager
        .delete(
            &fixture.workspace.id,
            &reviewed,
            "delete-leased-input",
            true,
        )
        .unwrap();
    assert_eq!(deleted["reclamation"], "complete");
    assert!(fixture.manager.lookup(&fixture.workspace.id).is_err());
}

#[test]
fn returned_download_releases_index_lock_and_pins_workspace() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", b"download");
    let item = fixture.items().pop().unwrap();
    let session = fixture
        .manager
        .locked(&fixture.workspace.id, LeaseMode::Shared)
        .unwrap();
    let held = download(
        &fixture.manager,
        session,
        item["artifactId"].as_str().unwrap(),
        item["revision"].as_str(),
    )
    .unwrap();
    let another = fixture
        .manager
        .locked(&fixture.workspace.id, LeaseMode::Shared)
        .unwrap();
    drop(another);
    let directory = fixture.manager.directory(&fixture.workspace.id).unwrap();
    assert!(
        directory
            .lease(LeaseMode::Exclusive, std::time::Duration::ZERO)
            .is_err()
    );
    drop(held);
    assert!(
        directory
            .lease(LeaseMode::Exclusive, std::time::Duration::ZERO)
            .is_ok()
    );
}

#[test]
fn independent_binding_lease_precedes_destructive_admission() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", b"input");
    let item = fixture.items().pop().unwrap();
    let directory = fixture.manager.directory(&fixture.workspace.id).unwrap();
    let admitted = directory
        .lease(LeaseMode::Shared, std::time::Duration::ZERO)
        .unwrap();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            barrier.wait();
            assert!(
                directory
                    .lease(LeaseMode::Exclusive, std::time::Duration::ZERO)
                    .is_err()
            );
            fixture
                .manager
                .register_dependency(
                    &fixture.workspace.id,
                    item["artifactId"].as_str().unwrap(),
                    item["revision"].as_str().unwrap(),
                    "saved-binding",
                    "edit",
                )
                .unwrap();
        });
        barrier.wait();
    });
    drop(admitted);
    let error = fixture
        .manager
        .remove(
            &fixture.workspace.id,
            item["artifactId"].as_str().unwrap(),
            item["revision"].as_str(),
            Some("remove-bound"),
        )
        .unwrap_err();
    assert_eq!(error.code, "artifact_referenced");
    fixture.manager.release_dependency("saved-binding").unwrap();
    fixture
        .manager
        .remove(
            &fixture.workspace.id,
            item["artifactId"].as_str().unwrap(),
            item["revision"].as_str(),
            Some("remove-released"),
        )
        .unwrap();
    assert_eq!(
        fixture
            .manager
            .register_dependency(
                &fixture.workspace.id,
                item["artifactId"].as_str().unwrap(),
                item["revision"].as_str().unwrap(),
                "late-binding",
                "edit"
            )
            .unwrap_err()
            .code,
        "artifact_changed"
    );
}

#[test]
fn manifest_restore_preserves_revision_and_live_equal_replacement_changes_it() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", b"retained");
    let original = fixture.items().pop().unwrap();
    let id = original["artifactId"].as_str().unwrap();
    let revision = original["revision"].as_str().unwrap();
    {
        let session = fixture
            .manager
            .locked(&fixture.workspace.id, LeaseMode::Exclusive)
            .unwrap();
        session.directory.remove_file("notes.txt").unwrap();
        let mut restored = session.directory.create_file("notes.txt", 0o600).unwrap();
        std::io::Write::write_all(&mut restored, b"retained").unwrap();
        let manifest = json!([{"relativePath":"notes.txt","sha256":original["sha256"],"byteLength":original["byteLength"]}]);
        rebase_restored_observations(&session, &manifest).unwrap();
        assert_eq!(file(&session, id).unwrap()["revision"], revision);
    }
    {
        let session = fixture
            .manager
            .locked(&fixture.workspace.id, LeaseMode::Shared)
            .unwrap();
        let downloaded = download(&fixture.manager, session, id, Some(revision)).unwrap();
        assert_eq!(downloaded.revision, revision);
    }
    {
        let directory = fixture.manager.directory(&fixture.workspace.id).unwrap();
        directory.remove_file("notes.txt").unwrap();
    }
    fixture.write("notes.txt", b"retained");
    assert_ne!(fixture.items()[0]["revision"], revision);
}

#[test]
fn restore_skips_historical_unavailable_and_rebases_only_known_provenance_path() {
    let fixture = Fixture::new();
    fixture.write("ready.txt", b"ready");
    fixture.write("missing.txt", b"missing");
    fixture.write("bad.png", b"bad");
    let items = fixture.items();
    let ready = items
        .iter()
        .find(|v| v["relativePath"] == "ready.txt")
        .unwrap();
    let missing = items
        .iter()
        .find(|v| v["relativePath"] == "missing.txt")
        .unwrap();
    let mut session = fixture
        .manager
        .locked(&fixture.workspace.id, LeaseMode::Exclusive)
        .unwrap();
    session.directory.remove_file("missing.txt").unwrap();
    reconcile(&fixture.manager, &mut session).unwrap();
    let provenance = json!({"path":"/previous/workspace/ready.txt","opaque":{"path":"/previous/workspace/parameter"}});
    session
        .db
        .execute(
            "UPDATE files SET provenance=? WHERE id=?",
            params![
                provenance.to_string(),
                ready["artifactId"].as_str().unwrap()
            ],
        )
        .unwrap();
    let manifest = json!([{"relativePath":"ready.txt","sha256":ready["sha256"],"byteLength":ready["byteLength"]}]);
    rebase_restored_observations(&session, &manifest).unwrap();
    let restored = file(&session, ready["artifactId"].as_str().unwrap()).unwrap();
    assert_eq!(
        restored["provenance"]["path"],
        json!(session.directory.path().join("ready.txt"))
    );
    assert_eq!(restored["provenance"]["opaque"], provenance["opaque"]);
    assert_eq!(
        file(&session, missing["artifactId"].as_str().unwrap()).unwrap()["state"],
        "missing"
    );
    session.directory.remove_file("ready.txt").unwrap();
    assert!(rebase_restored_observations(&session, &manifest).is_err());
}

#[test]
fn only_film_preview_receipt_establishes_unprofiled_png_contract() {
    let fixture = Fixture::new();
    let source = fixture.root.join("film-preview.png");
    let bytes = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 1, 8, 2,
        0, 0, 0, 123, 64, 232, 221, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 8, 72, 169, 0, 34,
        0, 7, 235, 2, 89, 147, 249, 235, 236, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    std::fs::write(&source, bytes).unwrap();
    let contract = json!({
        "format":"image/png", "precision":"uint8", "colorSpace":"srgb",
        "transfer":"srgb", "geometry":{"width":2,"height":1}, "encoding":"deflate"
    });
    let provenance = json!({
        "kind":"preview", "module":"spektrafilm", "sha256":format!("{:x}", sha2::Sha256::digest(bytes)),
        "byteLength":bytes.len(), "outputContract":contract
    });
    for (handle, field, value) in [
        ("foreign-preview", "module", json!("unrelated")),
        ("png-export", "kind", json!("output")),
    ] {
        let mut refused = provenance.clone();
        refused[field] = value;
        assert_eq!(
            fixture
                .manager
                .adopt_output(&fixture.workspace.id, handle, &source, &refused)
                .unwrap_err()
                .code,
            "output_invalid"
        );
    }
    let published = fixture
        .manager
        .adopt_output(&fixture.workspace.id, "film-preview", &source, &provenance)
        .unwrap();
    assert_eq!(published["imageContract"], contract);
    assert_eq!(published["provenance"]["outputContract"], contract);
    assert!(published["imageFacts"]["iccSha256"].is_null());
    let retained = fixture
        .items()
        .into_iter()
        .find(|item| item["artifactId"] == published["artifactId"])
        .unwrap();
    assert_eq!(retained["imageContract"], contract);
    assert_eq!(retained["state"], "ready");
}

#[test]
fn film_jpeg_writer_encoding_requires_qualified_srgb_bytes() {
    let generic = json!({
        "format":"image/jpeg", "precision":"uint8", "colorSpace":"srgb",
        "transfer":"srgb", "geometry":{"width":2,"height":1}, "encoding":"jpeg"
    });
    let mut asserted = generic.clone();
    asserted["format"] = json!("jpeg");
    asserted["encoding"] = json!("quality-85-baseline");
    let mut row = Record {
        id: "artifact".into(),
        path: "outputs/film.jpg".into(),
        kind: "output".into(),
        state: "ready".into(),
        evidence: None,
        contract: Some(generic),
        source: None,
        managed: false,
        provenance: None,
        removed: false,
        facts: Some(json!({"channels":3})),
        revision: None,
    };
    let provenance = json!({"module":"spektrafilm"});
    assert!(qualified_film_contract(&row, &provenance, &asserted));
    let mut foreign_color = asserted.clone();
    foreign_color["colorSpace"] = json!("prophoto-rgb");
    assert!(!qualified_film_contract(&row, &provenance, &foreign_color));
    row.contract = None;
    assert!(!qualified_film_contract(&row, &provenance, &asserted));
}
