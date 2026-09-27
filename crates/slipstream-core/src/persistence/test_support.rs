//! Test-only fixtures shared by the persistence module suites.
//!
//! `#[cfg(test)]` only: production code never sees this module.

use crate::persistence::admission::{DatabaseName, StateDirectory};
use crate::{
    CaptureFact, DiscoveredOriginal, LibraryRoot, OriginalFacts, OriginalKind, PhotoQueryCandidate,
    PhotoQueryProjection, PhotoRemovalMutation, PhotoStateField, PhotoStateMutation,
    PhotoStateValue, ScanSnapshot, SelectionState,
};
use rusqlite::Connection;
use rusqlite::params;
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) static NEXT_TEMP_TREE: AtomicU64 = AtomicU64::new(0);

pub(super) struct TempTree(pub(super) PathBuf);

impl TempTree {
    fn new() -> Self {
        loop {
            let nonce = NEXT_TEMP_TREE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("slipstream-owner-{}-{nonce}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("temporary owner fixture could not be created: {error}"),
            }
        }
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn fixture() -> (TempTree, LibraryRoot, StateDirectory, DatabaseName, PathBuf) {
    let base = TempTree::new();
    let originals = base.0.join("originals");
    let state_path = base.0.join("state");
    fs::create_dir(&originals).unwrap();
    fs::create_dir(&state_path).unwrap();
    fs::set_permissions(&state_path, fs::Permissions::from_mode(0o700)).unwrap();
    let library = LibraryRoot::open(&originals).unwrap();
    let state = StateDirectory::open_or_create(&library, &state_path).unwrap();
    let database_path = state_path.join("library.sqlite");
    (
        base,
        library,
        state,
        DatabaseName::parse("library.sqlite").unwrap(),
        database_path,
    )
}

pub(super) fn seed(path: &Path, sql: &str) {
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(sql).unwrap();
}

pub(super) fn discovered(
    path: &str,
    kind: OriginalKind,
    size: u64,
    mtime_ms: f64,
) -> DiscoveredOriginal {
    DiscoveredOriginal {
        path: crate::RelativeOriginalPath::parse(path).unwrap(),
        kind,
        facts: OriginalFacts {
            size,
            mtime_ms,
            device: 1,
            inode: 1,
        },
        error_category: None,
        error_message: None,
        capture: CaptureFact::pending(),
    }
}

pub(super) fn query_projection(snapshot: &ScanSnapshot) -> Arc<PhotoQueryProjection> {
    let originals = snapshot
        .originals
        .iter()
        .map(|original| (original.id.as_str(), original))
        .collect::<HashMap<_, _>>();
    let candidates = snapshot
        .photos
        .iter()
        .map(|photo| {
            let original = originals[photo.original_id.as_str()];
            PhotoQueryCandidate {
                photo_id: photo.id.clone(),
                relative_path: original.relative_path.as_str().to_owned(),
                sort_path: photo.sort_path.clone(),
                original_kind: original.kind,
                original_available: original.available,
                capture: original.capture.clone(),
                preview_state: photo.preview_state,
                preview_source_revision: photo.preview_source_revision.clone(),
                preview_width: photo.preview_width,
                preview_height: photo.preview_height,
            }
        })
        .collect::<Vec<_>>();
    let mut descending = (0..candidates.len()).collect::<Vec<_>>();
    descending.sort_by(|a, b| {
        let a = &candidates[*a];
        let b = &candidates[*b];
        a.capture_order_key()
            .is_none()
            .cmp(&b.capture_order_key().is_none())
            .then_with(|| match (a.capture_order_key(), b.capture_order_key()) {
                (Some(a), Some(b)) => b.cmp(a),
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.sort_path.cmp(&b.sort_path))
            .then_with(|| a.photo_id.cmp(&b.photo_id))
    });
    Arc::new(PhotoQueryProjection::new(candidates, descending).unwrap())
}

pub(super) fn photo_ids(snapshot: &ScanSnapshot) -> Vec<String> {
    snapshot
        .photos
        .iter()
        .map(|photo| photo.id.clone())
        .collect()
}

pub(super) fn sidecar_config(
    root: &LibraryRoot,
    state: &StateDirectory,
    name: &DatabaseName,
) -> crate::LibraryConfig {
    crate::LibraryConfig {
        library_root: root.canonical_path().to_owned(),
        state_directory: state.canonical_path().to_owned(),
        database_basename: name.as_os_str().to_string_lossy().into_owned(),
        ..crate::LibraryConfig::default()
    }
}

pub(super) fn seed_sidecar(path: &Path, photo: &str, sidecar: &str) {
    Connection::open(path)
        .unwrap()
        .execute(
            "INSERT INTO sidecar_associations VALUES(?,?,7,1234.5,?)",
            params![photo, sidecar, "a".repeat(64)],
        )
        .unwrap();
}

pub(super) fn association_generation(path: &Path, photo: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT association_generation FROM photos WHERE id=?",
            [photo],
            |r| r.get(0),
        )
        .unwrap()
}

pub(super) fn assert_retired(
    path: &Path,
    photo: &str,
    original: &str,
    sidecar: &str,
    generation: i64,
) {
    let connection = Connection::open(path).unwrap();
    let value: (String, String, String, i64, i64, f64, String) = connection.query_row(
        "SELECT retired_photo_id,retired_original_path,original_kind,retired_generation,observed_size,observed_mtime_ms,observed_digest FROM retained_sidecar_orphans WHERE sidecar_path=?",
        [sidecar], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
    ).unwrap();
    assert_eq!(
        value,
        (
            photo.to_owned(),
            original.to_owned(),
            "jpeg".to_owned(),
            generation,
            7,
            1234.5,
            "a".repeat(64)
        )
    );
    assert!(
        !connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sidecar_associations WHERE photo_id=?)",
                [photo],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
}

pub(super) async fn reject_and_remove(library: &crate::Library, photo: &str) {
    library
        .mutate_photo_state(PhotoStateMutation {
            photo_id: photo.to_owned(),
            field: PhotoStateField::SelectionState,
            value: PhotoStateValue::Selection(SelectionState::Rejected),
            expected_current: None,
            album_id: None,
        })
        .await
        .unwrap();
    assert_eq!(
        library
            .remove_photos(PhotoRemovalMutation {
                photo_ids: vec![photo.to_owned()],
                operation_id: "remove-sidecar".to_owned(),
            })
            .await
            .unwrap()
            .removed,
        vec![photo.to_owned()]
    );
}

pub(super) struct RecipeTestPhoto<'a> {
    pub(super) original_id: &'a str,
    pub(super) photo_id: &'a str,
    pub(super) relative_path: &'a str,
    pub(super) kind: &'a str,
    pub(super) available: bool,
    pub(super) size: i64,
    pub(super) mtime_ms: f64,
}

pub(super) fn add_recipe_test_photo(connection: &Connection, photo: RecipeTestPhoto<'_>) {
    connection
        .execute(
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state)
             VALUES(?,?,?,?,?,?,'pending')",
            params![
                photo.original_id,
                photo.relative_path,
                photo.kind,
                photo.size,
                photo.mtime_ms,
                i64::from(photo.available)
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating)
             VALUES(?,?,?,'inspection-pending',?,'undecided',0)",
            params![
                photo.photo_id,
                photo.original_id,
                i64::from(photo.available),
                photo.relative_path
            ],
        )
        .unwrap();
}
