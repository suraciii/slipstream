use super::*;
use slipstream_core::{LibraryRoot, RelativeOriginalPath, capture_source_revision};
use std::{fs, path::Path};

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    library: PathBuf,
    source: Arc<Mutex<Option<Source>>>,
    manager: Arc<WorkspaceManager>,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(photo_files::identity("workspace-lifecycle-test").unwrap());
        let state = root.join("state");
        let library = root.join("library");
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&library).unwrap();
        fs::write(library.join("original.ARW"), b"protected original content").unwrap();
        fs::write(library.join("original.ARW.xmp"), b"external sidecar").unwrap();
        let source = Arc::new(Mutex::new(None));
        let observed = Arc::clone(&source);
        let manager = Arc::new(
            WorkspaceManager::open(
                &state,
                library.clone(),
                Arc::new(move |_| Ok(observed.lock().clone())),
                1024 * 1024 * 1024,
            )
            .unwrap(),
        );
        let fixture = Self {
            root,
            state,
            library,
            source,
            manager,
        };
        fixture.observe();
        fixture
    }
    fn observe(&self) -> String {
        let path = RelativeOriginalPath::parse("original.ARW").unwrap();
        let root = LibraryRoot::open(&self.library).unwrap();
        let original = root.original(path.clone()).unwrap();
        let revision = capture_source_revision(path.as_str(), original.facts().unwrap()).unwrap();
        *self.source.lock() = Some(Source {
            relative_path: path,
            revision: Some(revision.clone()),
        });
        revision
    }
    fn open(&self) -> Value {
        self.manager.open_photo("photo-test").unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn prepared_input_survives_missing_original_and_explicit_removal_needs_refresh() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let opened = fixture.open();
    let workspace = opened["workspaceId"].as_str().unwrap();
    let artifact = opened["inputArtifactId"].as_str().unwrap();
    let path = Path::new(opened["inputPath"].as_str().unwrap());
    assert_eq!(fs::read(path).unwrap(), b"protected original content");
    assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o222, 0);
    fs::remove_file(fixture.library.join("original.ARW")).unwrap();
    *fixture.source.lock() = None;
    let reopened = fixture.open();
    assert_eq!(reopened["inputRevision"], opened["inputRevision"]);
    assert_eq!(reopened["inputAvailability"], "ready");
    assert!(reopened["observedSourceRevision"].is_null());
    let removed = fixture
        .manager
        .remove(
            workspace,
            artifact,
            opened["inputRevision"].as_str(),
            Some("remove-input"),
        )
        .unwrap();
    assert_eq!(removed["reclamation"], "complete");
    assert!(fixture.open()["inputPath"].is_null());
    assert!(!path.exists());
    fs::write(
        fixture.library.join("original.ARW"),
        b"protected original content",
    )
    .unwrap();
    let source_revision = fixture.observe();
    assert!(fixture.open()["inputArtifactId"].is_null());
    let refreshed = fixture
        .manager
        .refresh(workspace, &source_revision, Some("refresh-input"))
        .unwrap();
    assert_ne!(refreshed["inputArtifactId"], opened["inputArtifactId"]);
    let replay = fixture
        .manager
        .remove(
            workspace,
            artifact,
            opened["inputRevision"].as_str(),
            Some("remove-input"),
        )
        .unwrap();
    assert_eq!(replay["outcome"], "replayed");
    assert_eq!(
        fixture.open()["inputArtifactId"],
        refreshed["inputArtifactId"]
    );
    assert_eq!(
        fs::read(fixture.library.join("original.ARW")).unwrap(),
        b"protected original content"
    );
    assert_eq!(
        fs::read(fixture.library.join("original.ARW.xmp")).unwrap(),
        b"external sidecar"
    );
}

#[test]
fn refresh_refuses_changed_source_and_preserves_existing_input() {
    let fixture = Fixture::new();
    let opened = fixture.open();
    let workspace = opened["workspaceId"].as_str().unwrap();
    let original_revision = opened["observedSourceRevision"].as_str().unwrap();
    fs::write(
        fixture.library.join("original.ARW"),
        b"new bytes after the observed source",
    )
    .unwrap();
    let error = fixture
        .manager
        .refresh(workspace, original_revision, Some("stale-refresh"))
        .unwrap_err();
    assert_eq!(error.code, "conflict");
    assert_eq!(
        fs::read(opened["inputPath"].as_str().unwrap()).unwrap(),
        b"protected original content"
    );
    let current = fixture.observe();
    let refreshed = fixture
        .manager
        .refresh(workspace, &current, Some("fresh-refresh"))
        .unwrap();
    assert_ne!(refreshed["inputArtifactId"], opened["inputArtifactId"]);
    assert_eq!(fixture.open()["inputArtifactId"], opened["inputArtifactId"]);
    assert_eq!(
        fs::read(refreshed["inputPath"].as_str().unwrap()).unwrap(),
        b"new bytes after the observed source"
    );
    let replay = fixture
        .manager
        .refresh(workspace, &current, Some("fresh-refresh"))
        .unwrap();
    assert_eq!(replay["outcome"], "replayed");
    assert_eq!(replay["inputArtifactId"], refreshed["inputArtifactId"]);
    assert_eq!(
        fixture
            .manager
            .refresh(workspace, original_revision, Some("fresh-refresh"))
            .unwrap_err()
            .code,
        "request_conflict"
    );
}

#[test]
fn concurrent_open_returns_one_workspace_and_one_input() {
    let fixture = Fixture::new();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let opened = std::thread::scope(|scope| {
        let first = Arc::clone(&fixture.manager);
        let ready = Arc::clone(&barrier);
        let thread = scope.spawn(move || {
            ready.wait();
            first.open_photo("photo-test").unwrap()
        });
        barrier.wait();
        let second = fixture.open();
        (thread.join().unwrap(), second)
    });
    assert_eq!(opened.0["workspaceId"], opened.1["workspaceId"]);
    assert_eq!(opened.0["inputArtifactId"], opened.1["inputArtifactId"]);
    assert_eq!(opened.1["files"]["items"].as_array().unwrap().len(), 1);
}

#[test]
fn an_existing_workspace_reopens_when_the_authoritative_source_is_unavailable() {
    let fixture = Fixture::new();
    let opened = fixture.open();
    let workspace_id = opened["workspaceId"].as_str().unwrap().to_owned();
    *fixture.source.lock() = None;

    let reopened = fixture.open();
    assert_eq!(reopened["workspaceId"], workspace_id);
    assert_eq!(reopened["inputArtifactId"], opened["inputArtifactId"]);
    assert_eq!(reopened["inputRevision"], opened["inputRevision"]);
    assert_eq!(reopened["inputAvailability"], "ready");
    assert!(reopened["observedSourceRevision"].is_null());
}

fn snapshot(source: &Path, destination: &Path, relative: &Path, files: &mut Vec<Value>) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let next = relative.join(&name);
        if entry.file_type().unwrap().is_dir() {
            snapshot(&entry.path(), &destination.join(&name), &next, files);
        } else {
            use sha2::{Digest, Sha256};
            let bytes = fs::read(entry.path()).unwrap();
            fs::copy(entry.path(), destination.join(&name)).unwrap();
            files.push(json!({"relativePath":next,"sha256":format!("{:x}",Sha256::digest(&bytes)),"byteLength":bytes.len()}));
        }
    }
}

#[test]
fn isolated_restore_preserves_file_revision_and_refuses_altered_bytes() {
    let fixture = Fixture::new();
    let opened = fixture.open();
    let restored = fixture.root.join("restored");
    let mut files = Vec::new();
    snapshot(&fixture.state, &restored, Path::new("root-0"), &mut files);
    let manifest_path = fixture.root.join("manifest.json");
    let manifest = json!({"format":1,"roots":{"root-0":{"previousRoot":fixture.state,"restoredRoot":restored}},"files":files});
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let outcome = restore_verified(&restored, &manifest_path).unwrap();
    assert_eq!(outcome["workspaces"], 1);
    let manager = WorkspaceManager::open(
        &restored,
        fixture.root.join("missing-originals"),
        Arc::new(|_| Ok(None)),
        1024 * 1024 * 1024,
    )
    .unwrap();
    let resumed = manager.open_photo("photo-test").unwrap();
    assert_eq!(resumed["workspaceId"], opened["workspaceId"]);
    assert_eq!(resumed["inputArtifactId"], opened["inputArtifactId"]);
    assert_eq!(resumed["inputRevision"], opened["inputRevision"]);
    assert_eq!(resumed["inputAvailability"], "ready");
    assert_eq!(
        fs::read(resumed["inputPath"].as_str().unwrap()).unwrap(),
        b"protected original content"
    );
    let damaged = fixture.root.join("damaged");
    let mut damaged_files = Vec::new();
    snapshot(
        &fixture.state,
        &damaged,
        Path::new("root-0"),
        &mut damaged_files,
    );
    let relative = Path::new(opened["inputPath"].as_str().unwrap())
        .strip_prefix(&fixture.state)
        .unwrap();
    fs::remove_file(damaged.join(relative)).unwrap();
    fs::write(damaged.join(relative), b"changed backup bytes").unwrap();
    let damaged_manifest = json!({"format":1,"roots":{"root-0":{"previousRoot":fixture.state,"restoredRoot":damaged}},"files":damaged_files});
    fs::write(
        &manifest_path,
        serde_json::to_vec(&damaged_manifest).unwrap(),
    )
    .unwrap();
    assert_eq!(
        restore_verified(&damaged, &manifest_path).unwrap_err().code,
        "restore_mismatch"
    );
}
