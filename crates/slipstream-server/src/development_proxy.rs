//! Durable Development Proxy lifecycle and its CLI-shaped HTTP surface.
//!
//! A proxy build reuses the closed baseline `development-tiff` preview-class
//! launcher path, then decodes/downscales that scene-linear TIFF into an
//! immutable service-owned float32 ProPhoto TIFF. The database row is written
//! only after the artifact has been atomically installed.

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{Request, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use slipstream_core::{
    DEVELOPMENT_PROXY_LONG_EDGE, DEVELOPMENT_PROXY_PIPELINE_VERSION, DevelopmentProxyExpectation,
    DevelopmentProxyRecord, Library, decode_development_frame, encode_development_frame,
};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    ProcessingConfig,
    export_manager::ExportManager,
    http::{
        self, CLI_CONTRACT_HEADER, HttpState, cli_error, invalid_cli, require_cli_contract,
        require_published, valid_id,
    },
    preview_render::{PreviewCancellation, PreviewRender, RenderedPreview},
};

const QUALITY_LIMIT: &str = "2560-long-edge";
const MAX_FAILURE_REASON: usize = 120;
const MAX_FAILURES: usize = 1024;
const MAX_PENDING_BUILDS: usize = 64;

#[derive(Clone)]
struct BuildClaim {
    identity: String,
    generation: u64,
    cancellation: PreviewCancellation,
}

impl PartialEq for BuildClaim {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity && self.generation == other.generation
    }
}

impl Eq for BuildClaim {}

fn preview_build_failure(error: String) -> BuildFailure {
    if error == "source class has no approved profile"
        || error == "source has no RAW container"
        || error == "source camera identity could not be inspected"
        || error == "the Original is unavailable"
        || error == "Photo disappeared before preview admission"
        || error == "Edit recipe facts disappeared before preview admission"
    {
        BuildFailure::terminal(error)
    } else {
        BuildFailure::transient(error)
    }
}

#[derive(Debug)]
enum SourceHashFailure {
    Missing,
    Changed,
    Transient,
}

#[derive(Debug)]
enum FactsFailure {
    Missing,
    Storage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuildFailureKind {
    /// Capacity, contention, or storage pressure: the client may retry.
    Transient,
    /// A changed or absent source, or an invalid artifact: retrying the same
    /// request cannot succeed.
    Terminal,
}

#[derive(Clone, Debug)]
struct BuildFailure {
    kind: BuildFailureKind,
    message: String,
}

impl BuildFailure {
    fn transient(message: impl Into<String>) -> Self {
        Self {
            kind: BuildFailureKind::Transient,
            message: message.into(),
        }
    }

    fn terminal(message: impl Into<String>) -> Self {
        Self {
            kind: BuildFailureKind::Terminal,
            message: message.into(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct DevelopmentProxyManager {
    library: Arc<Library>,
    exports: Arc<ExportManager>,
    render: PreviewRender,
    processing: ProcessingConfig,
    root: PathBuf,
    builds: Arc<AsyncMutex<HashMap<String, BuildClaim>>>,
    next_generation: Arc<AtomicU64>,
    lifecycle: Arc<AsyncMutex<()>>,
    failures: Arc<Mutex<HashMap<String, ProxyFailure>>>,
}

#[derive(Clone, Debug)]
struct ProxyFailure {
    reason: String,
    at: u64,
    retryable: bool,
    category: &'static str,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyFacts {
    proxy_id: String,
    source_revision: String,
    source_size: u64,
    source_sha256: String,
    source_profile_id: String,
    pipeline_version: String,
    long_edge: u32,
    width: u32,
    height: u32,
    quality_limit: &'static str,
    byte_length: u64,
    sha256: String,
    created_at: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FailureWire {
    reason: String,
    at: u64,
    retryable: bool,
    category: &'static str,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyStateWire {
    photo_id: String,
    state: &'static str,
    proxy: Option<ProxyFacts>,
    failure: Option<FailureWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildingWire {
    photo_id: String,
    state: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemovalWire {
    photo_id: String,
    state: &'static str,
    removed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateBody {
    expected_source_revision: String,
}

impl DevelopmentProxyManager {
    pub(crate) fn open(
        library: Arc<Library>,
        exports: Arc<ExportManager>,
        processing: ProcessingConfig,
        state_directory: &Path,
    ) -> Result<Self, String> {
        let root = state_directory.join("development-proxies");
        fs::create_dir_all(&root)
            .map_err(|error| format!("Development Proxy storage is unavailable: {error}"))?;
        Ok(Self {
            render: PreviewRender::new(Arc::clone(&library), Arc::clone(&exports)),
            library,
            exports,
            processing,
            root,
            builds: Arc::new(AsyncMutex::new(HashMap::new())),
            next_generation: Arc::new(AtomicU64::new(0)),
            lifecycle: Arc::new(AsyncMutex::new(())),
            failures: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn artifact_path(&self, record: &DevelopmentProxyRecord) -> PathBuf {
        self.root.join(format!("{}.tiff", record.identity_digest()))
    }

    fn expectation<'a>(
        &'a self,
        revision: &'a str,
        profile_id: &'a str,
    ) -> DevelopmentProxyExpectation<'a> {
        DevelopmentProxyExpectation {
            source_revision: revision,
            profile_id,
            pipeline_version: DEVELOPMENT_PROXY_PIPELINE_VERSION,
            bundle_sha256: &self.processing.bundle_sha256,
        }
    }

    async fn source_hash(
        &self,
        photo: &slipstream_core::PhotoRead,
    ) -> Result<(u64, String), SourceHashFailure> {
        if !photo.original_available {
            return Err(SourceHashFailure::Missing);
        }
        let relative =
            slipstream_core::RelativeOriginalPath::parse(photo.original_location.clone())
                .map_err(|_| SourceHashFailure::Missing)?;
        let capability = self
            .library
            .original(relative)
            .map_err(|_| SourceHashFailure::Transient)?;
        tokio::task::spawn_blocking(move || {
            let digest = capability
                .digest_file_if_present()
                .map_err(|error| match error {
                    slipstream_core::confinement::ConfinementError::Changed => {
                        SourceHashFailure::Changed
                    }
                    _ => SourceHashFailure::Transient,
                })?
                .ok_or(SourceHashFailure::Missing)?;
            Ok((digest.facts.size, digest.digest))
        })
        .await
        .map_err(|_| SourceHashFailure::Transient)?
    }

    async fn read_facts(
        &self,
        photo_id: &str,
    ) -> Result<(slipstream_core::PhotoRead, slipstream_core::EditRecipeRead), FactsFailure> {
        let photo = self
            .library
            .photo(photo_id)
            .await
            .map_err(|_| FactsFailure::Storage)?
            .ok_or(FactsFailure::Missing)?;
        let read = self
            .library
            .edit_recipe(photo_id)
            .await
            .map_err(|_| FactsFailure::Storage)?
            .ok_or(FactsFailure::Missing)?;
        Ok((photo, read))
    }

    pub(crate) async fn current_record(&self, photo_id: &str) -> Option<DevelopmentProxyRecord> {
        let (photo, read) = self.read_facts(photo_id).await.ok()?;
        let record = self.library.development_proxy(photo_id).await.ok()??;
        // Without a published Capture fact bound to the observed source the
        // record cannot be revalidated; fail closed instead of accepting a
        // proxy built from an unpublished identity.
        let expected =
            self.expectation(read.current_source_revision.as_deref()?, &record.profile_id);
        if !record.current_against(&expected) {
            return None;
        }
        if photo.original_available {
            let (size, digest) = self.source_hash(&photo).await.ok()?;
            if size != record.source_size || digest != record.source_sha256 {
                return None;
            }
        }
        self.artifact_valid(&record).await.then_some(record)
    }

    pub(crate) async fn current_artifact(
        &self,
        photo_id: &str,
    ) -> Option<(DevelopmentProxyRecord, PathBuf)> {
        let record = self.current_record(photo_id).await?;
        let path = self.artifact_path(&record);
        Some((record, path))
    }

    async fn state(&self, photo_id: &str) -> Result<ProxyStateWire, FactsFailure> {
        let (photo, read) = self.read_facts(photo_id).await?;
        let record = self
            .library
            .development_proxy(photo_id)
            .await
            .map_err(|_| FactsFailure::Storage)?;
        let facts = record.as_ref().map(|record| self.wire_facts(record));
        let building = self.builds.lock().await.contains_key(photo_id);
        let state = if building {
            "building"
        } else if let Some(record) = record.as_ref() {
            // A pending publication gives no bound revision to revalidate
            // against: the record is stale until publication settles.
            let identity_current =
                read.current_source_revision
                    .as_deref()
                    .is_some_and(|revision| {
                        record.current_against(&self.expectation(revision, &record.profile_id))
                    });
            let source_current = if photo.original_available {
                self.source_hash(&photo)
                    .await
                    .ok()
                    .is_some_and(|(size, digest)| {
                        size == record.source_size && digest == record.source_sha256
                    })
            } else {
                true
            };
            if identity_current && source_current && self.artifact_valid(record).await {
                "current"
            } else {
                "stale"
            }
        } else {
            "absent"
        };
        let failure = self
            .failures
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(photo_id)
            .cloned()
            .map(|failure| FailureWire {
                reason: failure.reason,
                at: failure.at,
                retryable: failure.retryable,
                category: failure.category,
            });
        Ok(ProxyStateWire {
            photo_id: photo_id.to_owned(),
            state,
            proxy: facts,
            failure,
        })
    }

    async fn artifact_valid(&self, record: &DevelopmentProxyRecord) -> bool {
        let root = self.root.clone();
        let record = record.clone();
        tokio::task::spawn_blocking(move || Self::artifact_hash_valid_at(&root, &record))
            .await
            .unwrap_or(false)
    }

    fn artifact_hash_valid(&self, record: &DevelopmentProxyRecord) -> bool {
        Self::artifact_hash_valid_at(&self.root, record)
    }

    fn artifact_hash_valid_at(root: &Path, record: &DevelopmentProxyRecord) -> bool {
        let path = root.join(format!("{}.tiff", record.identity_digest()));
        let Ok(before) = fs::metadata(&path) else {
            return false;
        };
        if !before.is_file() || before.len() != record.artifact_bytes {
            return false;
        }
        let Ok(mut file) = OpenOptions::new()
            .read(true)
            .custom_flags(0x20000)
            .open(&path)
        else {
            return false;
        };
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 1024 * 1024];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => hasher.update(&buffer[..size]),
                Err(_) => return false,
            }
        }
        let Ok(after) = file.metadata() else {
            return false;
        };
        after.len() == before.len() && format!("{:x}", hasher.finalize()) == record.artifact_sha256
    }

    fn wire_facts(&self, record: &DevelopmentProxyRecord) -> ProxyFacts {
        ProxyFacts {
            proxy_id: record.identity_digest(),
            source_revision: record.source_revision.clone(),
            source_size: record.source_size,
            source_sha256: record.source_sha256.clone(),
            source_profile_id: record.profile_id.clone(),
            pipeline_version: record.pipeline_version.clone(),
            long_edge: record.long_edge,
            width: record.width,
            height: record.height,
            quality_limit: QUALITY_LIMIT,
            byte_length: record.artifact_bytes,
            sha256: record.artifact_sha256.clone(),
            created_at: record.created_at,
        }
    }

    async fn enqueue(
        self: &Arc<Self>,
        photo_id: String,
        expected_revision: String,
    ) -> Result<bool, ProxyError> {
        let (photo, read) = self
            .read_facts(&photo_id)
            .await
            .map_err(|_| ProxyError::Storage)?;
        if photo.original_kind != slipstream_core::OriginalKind::Raw {
            return Err(ProxyError::Unsupported);
        }
        if !photo.original_available || !read.source_available {
            return Err(ProxyError::Unavailable);
        }
        let current_revision = read.current_source_revision.as_deref();
        if current_revision != Some(expected_revision.as_str()) {
            return Err(match current_revision {
                Some(actual) => ProxyError::SourceChanged(actual.to_owned()),
                // No published Capture fact is bound to the observed source:
                // a retryable publication gap, not a confirmed change.
                None => ProxyError::Pending,
            });
        }
        // The approved profile is classified from the staged Original during
        // the build; an existing record's profile is the identity the prior
        // build was classified under, so it is the correct comparison key.
        if let Some(record) = self
            .library
            .development_proxy(&photo_id)
            .await
            .map_err(|_| ProxyError::Storage)?
        {
            let expected = self.expectation(&expected_revision, &record.profile_id);
            let source_current =
                self.source_hash(&photo)
                    .await
                    .ok()
                    .is_some_and(|(size, digest)| {
                        size == record.source_size && digest == record.source_sha256
                    });
            if record.current_against(&expected)
                && source_current
                && self.artifact_valid(&record).await
            {
                self.failures
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&photo_id);
                return Ok(false);
            }
        }
        let identity = format!("{}:{}", expected_revision, self.processing.bundle_sha256);
        let claim = {
            let mut builds = self.builds.lock().await;
            if let Some(current) = builds.get(&photo_id) {
                if current.identity == identity {
                    return Ok(true);
                }
                return Err(ProxyError::Conflict);
            }
            if builds.len() >= MAX_PENDING_BUILDS {
                return Err(ProxyError::Capacity);
            }
            let claim = BuildClaim {
                identity,
                generation: self.next_generation.fetch_add(1, Ordering::Relaxed),
                cancellation: PreviewCancellation::new(),
            };
            builds.insert(photo_id.clone(), claim.clone());
            claim
        };
        self.failures
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&photo_id);
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            manager.build(photo_id, expected_revision, claim).await;
        });
        Ok(true)
    }

    async fn build(
        self: Arc<Self>,
        photo_id: String,
        expected_revision: String,
        claim: BuildClaim,
    ) {
        let result = self
            .build_inner(&photo_id, &expected_revision, &claim)
            .await;
        let still_current = {
            let mut builds = self.builds.lock().await;
            if builds.get(&photo_id) == Some(&claim) {
                builds.remove(&photo_id);
                true
            } else {
                false
            }
        };
        if let Err(failure) = result
            && still_current
        {
            let reason: String = failure.message.chars().take(MAX_FAILURE_REASON).collect();
            let retryable = failure.kind == BuildFailureKind::Transient;
            let category = if retryable { "capacity" } else { "terminal" };
            let mut failures = self
                .failures
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if failures.len() >= MAX_FAILURES
                && let Some(key) = failures.keys().next().cloned()
            {
                failures.remove(&key);
            }
            failures.insert(
                photo_id,
                ProxyFailure {
                    reason,
                    at: unix_seconds(),
                    retryable,
                    category,
                },
            );
        }
    }

    async fn build_inner(
        &self,
        photo_id: &str,
        expected_revision: &str,
        claim: &BuildClaim,
    ) -> Result<(), BuildFailure> {
        let rendered = self
            .render
            .render(photo_id, "develop", "baseline", claim.cancellation.clone())
            .await
            .map_err(preview_build_failure)?;
        let result = self
            .install_render(photo_id, expected_revision, claim, &rendered)
            .await;
        // Every post-render outcome releases the staged preview artifact.
        self.exports.delete_preview_output(&rendered.attempt_key);
        result
    }

    async fn install_render(
        &self,
        photo_id: &str,
        expected_revision: &str,
        claim: &BuildClaim,
        rendered: &RenderedPreview,
    ) -> Result<(), BuildFailure> {
        let input_path = rendered.path.clone();
        let frame = tokio::task::spawn_blocking(move || {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(&input_path)
                .map_err(|error| error.to_string())?;
            decode_development_frame(
                std::os::fd::AsRawFd::as_raw_fd(&file),
                DEVELOPMENT_PROXY_LONG_EDGE,
            )
            .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| BuildFailure::terminal(error.to_string()))?
        .map_err(BuildFailure::terminal)?;
        let bytes = encode_development_frame(&frame)
            .map_err(|error| BuildFailure::terminal(error.to_string()))?;
        let record = DevelopmentProxyRecord {
            photo_id: photo_id.to_owned(),
            source_revision: expected_revision.to_owned(),
            source_relative_path: rendered.source_relative_path.clone(),
            source_sha256: rendered.source_sha256.clone(),
            source_size: rendered.source_size,
            profile_id: rendered.source_profile_id.clone(),
            pipeline_version: DEVELOPMENT_PROXY_PIPELINE_VERSION.to_owned(),
            bundle_sha256: self.processing.bundle_sha256.clone(),
            long_edge: DEVELOPMENT_PROXY_LONG_EDGE,
            width: frame.width,
            height: frame.height,
            artifact_sha256: format!("{:x}", Sha256::digest(&bytes)),
            artifact_bytes: bytes.len() as u64,
            created_at: unix_seconds(),
        };
        let temporary = self.root.join(format!(
            ".{}.{}.{}.tmp",
            record.identity_digest(),
            claim.generation,
            std::process::id()
        ));
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| BuildFailure::transient(error.to_string()))?;
            file.write_all(&bytes).map_err(|error| {
                let _ = fs::remove_file(&temporary);
                BuildFailure::transient(error.to_string())
            })?;
            file.sync_all().map_err(|error| {
                let _ = fs::remove_file(&temporary);
                BuildFailure::transient(error.to_string())
            })?;
        }
        let result = self.publish(claim, rendered, record, &temporary).await;
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    async fn publish(
        &self,
        claim: &BuildClaim,
        rendered: &RenderedPreview,
        record: DevelopmentProxyRecord,
        temporary: &Path,
    ) -> Result<(), BuildFailure> {
        let photo_id = &record.photo_id;
        let expected_revision = &record.source_revision;
        let path = self.artifact_path(&record);
        // The preview staged and hashed the Original before the native work.
        // Re-read the published source revision and the live Original bytes
        // immediately before installing the replacement, so a concurrent scan
        // cannot publish a proxy bound to an obsolete source as current.
        let current = self
            .library
            .edit_recipe(photo_id)
            .await
            .map_err(|_| BuildFailure::transient("source revision could not be re-read"))?
            .ok_or_else(|| BuildFailure::terminal("source revision is absent"))?;
        match current.current_source_revision.as_deref() {
            Some(revision) if revision == expected_revision.as_str() => {}
            Some(_) => {
                return Err(BuildFailure::terminal("source changed during proxy build"));
            }
            // No published Capture fact is bound to the observed source: a
            // retryable publication gap, not a confirmed change.
            None => {
                return Err(BuildFailure::transient(
                    "source revision publication is pending",
                ));
            }
        }
        let photo = self
            .library
            .photo(photo_id)
            .await
            .map_err(|_| BuildFailure::transient("source could not be re-read"))?
            .ok_or_else(|| BuildFailure::terminal("photo is absent"))?;
        let (size, digest) = self
            .source_hash(&photo)
            .await
            .map_err(|error| match error {
                SourceHashFailure::Missing => BuildFailure::terminal("source is absent"),
                SourceHashFailure::Changed => {
                    BuildFailure::terminal("source changed during proxy build")
                }
                SourceHashFailure::Transient => {
                    BuildFailure::transient("source could not be hashed")
                }
            })?;
        if size != rendered.source_size || digest != rendered.source_sha256 {
            return Err(BuildFailure::terminal(
                "source bytes changed during proxy build",
            ));
        }
        let _lifecycle = self.lifecycle.lock().await;
        if self.builds.lock().await.get(photo_id) != Some(claim) {
            return Err(BuildFailure::terminal("proxy build was removed"));
        }
        fs::rename(temporary, &path).map_err(|error| BuildFailure::transient(error.to_string()))?;
        self.publish_record(record, &path).await
    }

    async fn publish_record(
        &self,
        record: DevelopmentProxyRecord,
        path: &Path,
    ) -> Result<(), BuildFailure> {
        let previous = self
            .library
            .development_proxy(&record.photo_id)
            .await
            .map_err(|_| BuildFailure::transient("proxy record read failed"))?;
        // The new artifact was renamed over `path`. When a prior valid record
        // names the same identity, that file is still its artifact; failure
        // cleanup must never delete it.
        let preserves = previous
            .as_ref()
            .is_some_and(|previous| self.artifact_path(previous) == *path);
        let directory =
            File::open(&self.root).map_err(|error| BuildFailure::transient(error.to_string()))?;
        if let Err(error) = directory.sync_all() {
            if !preserves {
                let _ = fs::remove_file(path);
            }
            return Err(BuildFailure::transient(error.to_string()));
        }
        match self.library.record_development_proxy(record).await {
            Ok(true) => {}
            Ok(false) => {
                if !preserves {
                    let _ = fs::remove_file(path);
                }
                return Err(BuildFailure::terminal("proxy record was invalid"));
            }
            Err(_) => {
                if !preserves {
                    let _ = fs::remove_file(path);
                }
                return Err(BuildFailure::transient("proxy record publication failed"));
            }
        }
        if let Some(previous) = previous {
            let old = self.artifact_path(&previous);
            if old != *path {
                let _ = fs::remove_file(old);
            }
        }
        Ok(())
    }

    pub(crate) fn reconcile_after_restart(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let _lifecycle = manager.lifecycle.lock().await;
            let Ok(records) = manager.library.all_development_proxies().await else {
                return;
            };
            let mut claimed = std::collections::HashSet::new();
            for record in records {
                let path = manager.artifact_path(&record);
                let photo_id = record.photo_id.clone();
                let checker = Arc::clone(&manager);
                let valid =
                    tokio::task::spawn_blocking(move || checker.artifact_hash_valid(&record))
                        .await
                        .unwrap_or(false);
                if valid {
                    claimed.insert(path);
                } else {
                    if matches!(
                        manager.library.remove_development_proxy(&photo_id).await,
                        Ok(true)
                    ) {
                        let _ = fs::remove_file(path);
                    } else {
                        // The row still claims this artifact; orphan sweeping must not erase it.
                        claimed.insert(path);
                    }
                }
            }
            if let Ok(entries) = fs::read_dir(&manager.root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let live_temp = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .and_then(|name| name.strip_prefix('.')?.strip_suffix(".tmp"))
                        .and_then(|name| name.rsplit_once('.')?.1.parse::<u32>().ok())
                        == Some(std::process::id());
                    if live_temp || claimed.contains(&path) {
                        continue;
                    }
                    let _ = fs::remove_file(path);
                }
            }
        });
    }

    async fn remove(&self, photo_id: &str) -> Result<bool, ()> {
        // Signal the launcher before waiting for publication's lifecycle lock.
        if let Some(claim) = self.builds.lock().await.get(photo_id) {
            claim.cancellation.cancel();
        }
        let _lifecycle = self.lifecycle.lock().await;
        self.builds.lock().await.remove(photo_id);
        let record = self
            .library
            .development_proxy(photo_id)
            .await
            .map_err(|_| ())?;
        let removed = self
            .library
            .remove_development_proxy(photo_id)
            .await
            .map_err(|_| ())?;
        if let Some(record) = record {
            let _ = fs::remove_file(self.artifact_path(&record));
        }
        self.failures
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(photo_id);
        Ok(removed)
    }
}

#[derive(Debug)]
enum ProxyError {
    Storage,
    Unavailable,
    Pending,
    Unsupported,
    Conflict,
    Capacity,
    SourceChanged(String),
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn json<T: Serialize>(status: StatusCode, value: &T) -> Response<Body> {
    http::json_response(status, value)
}
fn proxy_error(error: ProxyError, photo_id: &str) -> Response<Body> {
    match error {
        ProxyError::Storage => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "resource_unavailable",
            "Development Proxy storage is unavailable",
            serde_json::json!({"operation":"development-proxy"}),
        ),
        ProxyError::Unavailable => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "resource_unavailable",
            "The Original is unavailable or unreadable",
            serde_json::json!({"photoId":photo_id,"reason":"original-missing"}),
        ),
        ProxyError::Pending => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "resource_unavailable",
            "The source facts are pending publication; retry once the read settles",
            serde_json::json!({"photoId":photo_id,"reason":crate::edit_recipe::READ_PENDING}),
        ),
        ProxyError::Unsupported => cli_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_photo",
            "The source class has no approved profile",
            serde_json::json!({"photoId":photo_id}),
        ),
        ProxyError::Conflict => cli_error(
            StatusCode::CONFLICT,
            "request_conflict",
            "A different Development Proxy build is already running",
            serde_json::json!({"photoId":photo_id}),
        ),
        ProxyError::Capacity => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "capacity_exceeded",
            "Development Proxy build capacity is full; retry later",
            serde_json::json!({"photoId":photo_id,"retryable":true}),
        ),
        ProxyError::SourceChanged(revision) => cli_error(
            StatusCode::CONFLICT,
            "source_changed",
            "The published source revision changed before proxy admission",
            serde_json::json!({"currentSourceRevision":revision,"currentRecipeVersion":serde_json::Value::Null}),
        ),
    }
}

pub(crate) async fn get_development_proxy(
    State(state): State<HttpState>,
    AxumPath(photo_id): AxumPath<String>,
    request: Request<Body>,
) -> Response<Body> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    if !valid_id(&photo_id) {
        return invalid_cli("photoId", "The Photo ID is invalid.");
    }
    let Some(manager) = state.application.proxies.as_ref() else {
        return proxy_error(ProxyError::Unavailable, &photo_id);
    };
    match manager.state(&photo_id).await {
        Ok(value) => json(StatusCode::OK, &value),
        Err(FactsFailure::Missing) => cli_error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the persisted Library",
            serde_json::json!({"photoId":photo_id}),
        ),
        Err(FactsFailure::Storage) => proxy_error(ProxyError::Storage, &photo_id),
    }
}

pub(crate) async fn post_development_proxy(
    State(state): State<HttpState>,
    AxumPath(photo_id): AxumPath<String>,
    request: Request<Body>,
) -> Response<Body> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    if !valid_id(&photo_id) {
        return invalid_cli("photoId", "The Photo ID is invalid.");
    }
    let body: CreateBody = match http::read_cli_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if body.expected_source_revision.is_empty() {
        return invalid_cli("expectedSourceRevision", "The source revision is required.");
    }
    let Some(manager) = state.application.proxies.as_ref() else {
        return proxy_error(ProxyError::Unavailable, &photo_id);
    };
    match manager
        .enqueue(photo_id.clone(), body.expected_source_revision)
        .await
    {
        Ok(false) => match manager.state(&photo_id).await {
            Ok(value) => json(StatusCode::OK, &value),
            Err(_) => proxy_error(ProxyError::Storage, &photo_id),
        },
        Ok(true) => json(
            StatusCode::ACCEPTED,
            &BuildingWire {
                photo_id,
                state: "building",
            },
        ),
        Err(error) => proxy_error(error, &photo_id),
    }
}

pub(crate) async fn delete_development_proxy(
    State(state): State<HttpState>,
    AxumPath(photo_id): AxumPath<String>,
    request: Request<Body>,
) -> Response<Body> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    if !valid_id(&photo_id) {
        return invalid_cli("photoId", "The Photo ID is invalid.");
    }
    let Some(manager) = state.application.proxies.as_ref() else {
        return proxy_error(ProxyError::Unavailable, &photo_id);
    };
    match manager.remove(&photo_id).await {
        Ok(removed) => json(
            StatusCode::OK,
            &RemovalWire {
                photo_id,
                state: "absent",
                removed,
            },
        ),
        Err(_) => proxy_error(ProxyError::Storage, &photo_id),
    }
}

#[cfg(test)]
mod tests {
    use super::{BuildFailureKind, preview_build_failure};

    #[test]
    fn unsupported_profile_is_terminal_but_capacity_pressure_is_retryable() {
        let unsupported = preview_build_failure("source class has no approved profile".into());
        assert_eq!(unsupported.kind, BuildFailureKind::Terminal);
        let capacity = preview_build_failure("preview admission capacity is exhausted".into());
        assert_eq!(capacity.kind, BuildFailureKind::Transient);
    }

    #[test]
    fn missing_source_is_terminal_but_unreadable_source_can_be_retried() {
        assert_eq!(
            preview_build_failure("the Original is unavailable".into()).kind,
            BuildFailureKind::Terminal
        );
        assert_eq!(
            preview_build_failure("source metadata could not be inspected".into()).kind,
            BuildFailureKind::Transient
        );
    }
}
