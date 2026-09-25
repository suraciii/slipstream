//! Private source staging and Development TIFF publication/preview staging.
//!
//! This module owns the filesystem boundary for Export and preview callers.
//! It does not start an image engine, persist Export state, or enable a
//! processing capability. Callers resolve an [`OriginalCapability`] through
//! the Library first, then use this workspace for one bounded attempt.

use crate::{OriginalCapability, OriginalFacts, confinement::ConfinementError};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::unix::{
        ffi::OsStrExt,
        fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

/// Largest source or output accepted by this bounded filesystem seam.
/// Processing admission may impose a smaller deployment-specific limit.
pub const MAXIMUM_EXPORT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub enum ExportError {
    InvalidWorkspace,
    InvalidExportId,
    InvalidArtifact,
    Source(ConfinementError),
    ResourceLimit,
    Io(io::Error),
    Validation(&'static str),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidWorkspace => "Export workspace is invalid",
            Self::InvalidExportId => "Export identity is invalid",
            Self::InvalidArtifact => "Export artifact is invalid",
            Self::Source(error) => return error.fmt(formatter),
            Self::ResourceLimit => "Export exceeds the bounded output limit",
            Self::Io(_) => "Export filesystem operation failed",
            Self::Validation(message) => message,
        })
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ConfinementError> for ExportError {
    fn from(error: ConfinementError) -> Self {
        Self::Source(error)
    }
}

impl From<io::Error> for ExportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone)]
pub struct ExportWorkspace {
    inner: Arc<WorkspaceInner>,
}

struct WorkspaceInner {
    root: PathBuf,
    staging: PathBuf,
    artifacts: PathBuf,
    previews: PathBuf,
    work: PathBuf,
}

/// A source copy owned by one processing attempt. Dropping it removes the
/// private copy and its empty attempt directory.
pub struct StagedOriginal {
    path: PathBuf,
    attempt_directory: PathBuf,
    digest: String,
    facts: OriginalFacts,
}

struct AttemptDirectory {
    path: PathBuf,
    retained: bool,
}

impl AttemptDirectory {
    fn create(path: PathBuf) -> Result<Self, ExportError> {
        fs::create_dir(&path)?;
        let guard = Self {
            path,
            retained: false,
        };
        if let Err(error) = set_private_directory(&guard.path) {
            drop(guard);
            return Err(error);
        }
        Ok(guard)
    }

    fn retain(mut self) -> PathBuf {
        self.retained = true;
        self.path.clone()
    }
}

impl Drop for AttemptDirectory {
    fn drop(&mut self) {
        if !self.retained {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StagedOriginalFacts {
    pub path: PathBuf,
    pub sha256: String,
    pub source_facts: OriginalFacts,
}

/// The only output target enabled by this filesystem seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportTarget {
    DevelopmentTiff,
}

/// An unpublished output. The path is private to the workspace until commit.
pub struct ArtifactWriter {
    temporary_path: PathBuf,
    final_path: PathBuf,
    target: ExportTarget,
    committed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedArtifact {
    pub path: PathBuf,
    pub target: ExportTarget,
    pub size: u64,
    pub sha256: String,
}

impl ExportWorkspace {
    /// Opens an application-owned workspace and rejects storage inside the
    /// read-only Original tree. The caller owns retention and Export state.
    pub fn open(
        workspace: impl AsRef<Path>,
        original_root: impl AsRef<Path>,
    ) -> Result<Self, ExportError> {
        let workspace = workspace.as_ref();
        let original_root = original_root.as_ref();
        if !workspace.is_absolute() || !original_root.is_absolute() {
            return Err(ExportError::InvalidWorkspace);
        }
        let original_root =
            fs::canonicalize(original_root).map_err(|_| ExportError::InvalidWorkspace)?;
        if !original_root.is_dir() {
            return Err(ExportError::InvalidWorkspace);
        }
        let root = fs::canonicalize(workspace).map_err(|_| ExportError::InvalidWorkspace)?;
        if !root.is_dir() || root == original_root || root.starts_with(&original_root) {
            return Err(ExportError::InvalidWorkspace);
        }
        set_private_directory(&root)?;
        let staging = create_private_child(&root, "staging")?;
        let artifacts = create_private_child(&root, "artifacts")?;
        let previews = create_private_child(&root, "previews")?;
        // A freshly opened workspace owns no preview admission, so every
        // preview output it inherits belongs to a service lifetime that ended
        // without its owner: ephemeral preview staging never outlives the
        // admission that produced it.
        clear_private_child(&previews);
        let work = create_private_child(&root, "work")?;
        Ok(Self {
            inner: Arc::new(WorkspaceInner {
                root,
                staging,
                artifacts,
                previews,
                work,
            }),
        })
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Streams one confined Original into a private, read-only path. The
    /// returned digest and facts describe the exact bytes admitted to staging.
    pub fn stage_original(
        &self,
        original: &OriginalCapability,
    ) -> Result<StagedOriginal, ExportError> {
        let token = unique_token();
        let attempt_directory = AttemptDirectory::create(self.inner.staging.join(&token))?;

        let extension = Path::new(original.path().as_str())
            .extension()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .ok_or(ExportError::InvalidArtifact)?;
        let path = attempt_directory.path.join(format!("input.{extension}"));
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600)
            .open(&path)?;
        let copied = original.copy_revision_checked(&mut output)?;
        output.sync_all()?;
        seal_read_only(&output)?;
        drop(output);
        Ok(StagedOriginal {
            path,
            attempt_directory: attempt_directory.retain(),
            digest: copied.digest,
            facts: copied.facts,
        })
    }

    /// Starts a private output. The caller writes through the returned path,
    /// validates the engine output, then calls `publish`.
    pub fn begin_development_tiff(&self, export_id: &str) -> Result<ArtifactWriter, ExportError> {
        if !valid_export_id(export_id) {
            return Err(ExportError::InvalidExportId);
        }
        let token = unique_token();
        let temporary_path = self.inner.work.join(format!("{token}.tiff"));
        let final_path = self.inner.artifacts.join(format!("{export_id}.tiff"));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600)
            .open(&temporary_path)?;
        drop(file);
        Ok(ArtifactWriter {
            temporary_path,
            final_path,
            target: ExportTarget::DevelopmentTiff,
            committed: false,
        })
    }

    /// Starts a private ephemeral preview output. The caller writes through
    /// the returned path, validates the engine output, then calls `publish`.
    /// Preview outputs are kept below their own directory and never enter the
    /// retained Export artifact namespace.
    pub fn begin_preview_tiff(&self, attempt_key: &str) -> Result<ArtifactWriter, ExportError> {
        if !valid_export_id(attempt_key) {
            return Err(ExportError::InvalidExportId);
        }
        let token = unique_token();
        let temporary_path = self.inner.work.join(format!("{token}.tiff"));
        let final_path = self.inner.previews.join(format!("{attempt_key}.tiff"));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600)
            .open(&temporary_path)?;
        drop(file);
        Ok(ArtifactWriter {
            temporary_path,
            final_path,
            target: ExportTarget::DevelopmentTiff,
            committed: false,
        })
    }

    /// Deletes one ephemeral preview output. Expiry and supersession are
    /// idempotent: an already-removed output is no longer retained.
    pub fn delete_preview_tiff(&self, attempt_key: &str) -> Result<(), ExportError> {
        if !valid_export_id(attempt_key) {
            return Err(ExportError::InvalidExportId);
        }
        let path = self.inner.previews.join(format!("{attempt_key}.tiff"));
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl StagedOriginal {
    pub fn facts(&self) -> StagedOriginalFacts {
        StagedOriginalFacts {
            path: self.path.clone(),
            sha256: self.digest.clone(),
            source_facts: self.facts,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedOriginal {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.attempt_directory);
    }
}

impl ArtifactWriter {
    pub fn temporary_path(&self) -> &Path {
        &self.temporary_path
    }

    pub fn target(&self) -> ExportTarget {
        self.target
    }

    /// Validates the completed output, syncs file and parent directory, then
    /// publishes it with one rename. Any error removes the partial output.
    pub fn publish(
        mut self,
        validate: impl FnOnce(&Path) -> Result<(), ExportError>,
    ) -> Result<PublishedArtifact, ExportError> {
        let temporary = open_regular(&self.temporary_path, true)?;
        let metadata = temporary.metadata()?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAXIMUM_EXPORT_BYTES {
            return Err(ExportError::InvalidArtifact);
        }
        validate(&self.temporary_path)?;
        let facts = hash_regular(&self.temporary_path)?;
        if facts.size != metadata.len() {
            return Err(ExportError::Validation(
                "Export output changed during validation",
            ));
        }
        temporary.sync_all()?;
        seal_read_only(&temporary)?;
        drop(temporary);
        rename_noreplace(&self.temporary_path, &self.final_path)?;
        self.committed = true;
        if let Err(error) = sync_directory(self.inner_dir()) {
            let cleanup = fs::remove_file(&self.final_path);
            let _ = sync_directory(self.inner_dir());
            self.committed = cleanup.is_err();
            if cleanup.is_ok() {
                return Err(error);
            }
            return Err(ExportError::Validation(
                "Export publication durability is uncertain",
            ));
        }
        Ok(PublishedArtifact {
            path: self.final_path.clone(),
            target: self.target,
            size: facts.size,
            sha256: facts.sha256,
        })
    }

    fn inner_dir(&self) -> &Path {
        self.final_path.parent().expect("artifact has a parent")
    }
}

impl Drop for ArtifactWriter {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}

struct HashedFacts {
    size: u64,
    sha256: String,
}

fn hash_regular(path: &Path) -> Result<HashedFacts, ExportError> {
    let mut file = open_regular(path, false)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAXIMUM_EXPORT_BYTES {
        return Err(ExportError::InvalidArtifact);
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut size = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(u64::try_from(count).map_err(|_| ExportError::ResourceLimit)?)
            .ok_or(ExportError::ResourceLimit)?;
        if size > MAXIMUM_EXPORT_BYTES {
            return Err(ExportError::ResourceLimit);
        }
        hasher.update(&buffer[..count]);
    }
    Ok(HashedFacts {
        size,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn open_regular(path: &Path, writable: bool) -> Result<File, ExportError> {
    let mut options = OpenOptions::new();
    options.read(true).write(writable);
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(ExportError::InvalidArtifact);
    }
    Ok(file)
}

fn set_private_directory(path: &Path) -> Result<(), ExportError> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn create_private_child(root: &Path, name: &str) -> Result<PathBuf, ExportError> {
    let path = root.join(name);
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(ExportError::InvalidWorkspace);
    }
    fs::create_dir_all(&path)?;
    set_private_directory(&path)?;
    let canonical = fs::canonicalize(path)?;
    if !canonical.starts_with(root) || !canonical.is_dir() {
        return Err(ExportError::InvalidWorkspace);
    }
    Ok(canonical)
}

/// Removes everything a previous lifetime left in a private workspace child.
/// Removal is best effort: a file that cannot be deleted must not stop the
/// owner from opening its workspace, and the next open retries.
fn clear_private_child(path: &Path) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let target = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&target) else {
            continue;
        };
        let _ = if metadata.file_type().is_dir() {
            fs::remove_dir_all(&target)
        } else {
            fs::remove_file(&target)
        };
    }
}

fn seal_read_only(file: &File) -> Result<(), ExportError> {
    let mut permissions = file.metadata()?.permissions();
    permissions.set_mode(0o400);
    file.set_permissions(permissions)?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), ExportError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn rename_noreplace(from: &Path, to: &Path) -> Result<(), ExportError> {
    let from =
        CString::new(from.as_os_str().as_bytes()).map_err(|_| ExportError::InvalidArtifact)?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(|_| ExportError::InvalidArtifact)?;
    // SAFETY: both paths are NUL-terminated and point to files in the
    // application-owned workspace. RENAME_NOREPLACE prevents a concurrent
    // request with the same Export identity from replacing its artifact.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else if io::Error::last_os_error().kind() == io::ErrorKind::AlreadyExists {
        Err(ExportError::Validation("Export identity already exists"))
    } else {
        Err(io::Error::last_os_error().into())
    }
}

fn valid_export_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value != "."
        && value != ".."
}

fn unique_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static BASE: OnceLock<u64> = OnceLock::new();
    let base = *BASE.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(1)
    });
    format!(
        "{}-{}-{}",
        std::process::id(),
        base,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LibraryRoot, RelativeOriginalPath};
    use std::{fs, os::unix::fs::MetadataExt};

    fn fixture() -> (std::path::PathBuf, LibraryRoot, ExportWorkspace) {
        let root = std::env::temp_dir().join(format!("slipstream-export-test-{}", unique_token()));
        let library_path = root.join("library");
        fs::create_dir_all(&library_path).unwrap();
        fs::write(library_path.join("one.ARW"), b"raw bytes").unwrap();
        let library = LibraryRoot::open(&library_path).unwrap();
        fs::create_dir_all(root.join("exports")).unwrap();
        let workspace =
            ExportWorkspace::open(root.join("exports"), library.canonical_path()).unwrap();
        (root, library, workspace)
    }

    #[test]
    fn stages_confined_original_and_seals_copy() {
        let (root, library, workspace) = fixture();
        let source = root.join("library/one.ARW");
        let before = fs::read(&source).unwrap();
        let capability = library
            .original(RelativeOriginalPath::parse("one.ARW").unwrap())
            .unwrap();
        let staged = workspace.stage_original(&capability).unwrap();
        assert_eq!(fs::read(staged.path()).unwrap(), before);
        assert_eq!(staged.path().extension().unwrap(), "ARW");
        assert_eq!(staged.path().metadata().unwrap().mode() & 0o777, 0o400);
        assert_eq!(staged.facts().source_facts.size, before.len() as u64);
        drop(staged);
        assert!(
            !workspace
                .root()
                .join("staging")
                .read_dir()
                .unwrap()
                .next()
                .is_some()
        );
        assert_eq!(fs::read(source).unwrap(), before);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_extensionless_original_without_leaking_staging_directory() {
        let (root, library, workspace) = fixture();
        let source = root.join("library/no-extension");
        fs::write(&source, b"raw bytes").unwrap();
        let capability = library
            .original(RelativeOriginalPath::parse("no-extension").unwrap())
            .unwrap();

        assert!(matches!(
            workspace.stage_original(&capability),
            Err(ExportError::InvalidArtifact)
        ));
        assert!(
            workspace
                .root()
                .join("staging")
                .read_dir()
                .unwrap()
                .next()
                .is_none()
        );
        assert_eq!(fs::read(source).unwrap(), b"raw bytes");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publishes_validated_tiff_atomically_and_hashes_bytes() {
        let (root, _library, workspace) = fixture();
        let writer = workspace.begin_development_tiff("request-1").unwrap();
        fs::write(writer.temporary_path(), b"II*\0tiff").unwrap();
        let published = writer
            .publish(|path| {
                assert_eq!(
                    path.extension().and_then(|value| value.to_str()),
                    Some("tiff")
                );
                Ok(())
            })
            .unwrap();
        assert_eq!(published.target, ExportTarget::DevelopmentTiff);
        assert_eq!(published.size, 8);
        assert_eq!(fs::metadata(&published.path).unwrap().mode() & 0o777, 0o400);
        assert_eq!(fs::read(&published.path).unwrap(), b"II*\0tiff");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validation_failure_removes_partial_output() {
        let (root, _library, workspace) = fixture();
        let writer = workspace.begin_development_tiff("request-2").unwrap();
        fs::write(writer.temporary_path(), b"partial").unwrap();
        let temporary = writer.temporary_path().to_owned();
        assert!(
            writer
                .publish(|_| Err(ExportError::Validation("bad tiff")))
                .is_err()
        );
        assert!(!temporary.exists());
        assert!(!workspace.root().join("artifacts/request-2.tiff").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preview_output_is_private_and_deletable() {
        let (root, _library, workspace) = fixture();
        let writer = workspace.begin_preview_tiff("prev-test-1").unwrap();
        fs::write(writer.temporary_path(), b"preview").unwrap();
        let published = writer.publish(|_| Ok(())).unwrap();
        assert!(
            published
                .path
                .starts_with(workspace.root().join("previews"))
        );
        assert!(!workspace.root().join("artifacts/prev-test-1.tiff").exists());
        assert!(published.path.exists());
        workspace.delete_preview_tiff("prev-test-1").unwrap();
        assert!(!published.path.exists());
        let _ = fs::remove_dir_all(root);
    }

    /// Ephemeral preview staging never outlives the service lifetime that
    /// produced it: a reopened workspace inherits no preview output, while a
    /// retained Export artifact of the same lifetime stays.
    #[test]
    fn reopening_the_workspace_removes_inherited_preview_outputs() {
        let (root, library, workspace) = fixture();
        let writer = workspace.begin_preview_tiff("prev-test-2").unwrap();
        fs::write(writer.temporary_path(), b"preview").unwrap();
        let preview = writer.publish(|_| Ok(())).unwrap();
        let retained = workspace.begin_development_tiff("request-3").unwrap();
        fs::write(retained.temporary_path(), b"II*\0tiff").unwrap();
        retained.publish(|_| Ok(())).unwrap();

        let reopened =
            ExportWorkspace::open(root.join("exports"), library.canonical_path()).unwrap();
        assert!(!preview.path.exists());
        assert!(
            reopened
                .root()
                .join("previews")
                .read_dir()
                .unwrap()
                .next()
                .is_none()
        );
        assert!(reopened.root().join("artifacts/request-3.tiff").exists());
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn publication_does_not_replace_an_existing_export_identity() {
        let (root, _library, workspace) = fixture();
        let first = workspace.begin_development_tiff("same-request").unwrap();
        fs::write(first.temporary_path(), b"first").unwrap();
        first.publish(|_| Ok(())).unwrap();

        let second = workspace.begin_development_tiff("same-request").unwrap();
        fs::write(second.temporary_path(), b"second").unwrap();
        assert!(matches!(
            second.publish(|_| Ok(())),
            Err(ExportError::Validation("Export identity already exists"))
        ));
        assert_eq!(
            fs::read(workspace.root().join("artifacts/same-request.tiff")).unwrap(),
            b"first"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_inside_originals_is_rejected() {
        let root = std::env::temp_dir().join(format!("slipstream-export-test-{}", unique_token()));
        fs::create_dir_all(&root).unwrap();
        let workspace = root.join("exports");
        assert!(matches!(
            ExportWorkspace::open(&workspace, &root),
            Err(ExportError::InvalidWorkspace)
        ));
        assert!(!workspace.exists());

        // A missing exports directory is likewise rejected, not created.
        let library = root.join("library");
        let missing = library.join("exports");
        fs::create_dir_all(&library).unwrap();
        assert!(matches!(
            ExportWorkspace::open(&missing, &library),
            Err(ExportError::InvalidWorkspace)
        ));
        assert!(!missing.exists());
        let _ = fs::remove_dir_all(root);
    }
}
