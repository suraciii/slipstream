//! Composable module execution and leased artifact input staging.

use super::*;
use sha2::Digest as _;
use slipstream_processing::modules::{
    ConfinedInput, ImageContract, ModuleAvailability, ModuleRegistry, Parameters,
};
use std::time::{SystemTime, UNIX_EPOCH};

/// One completed bounded Preview of a selected composable step. The
/// rendition is ephemeral; the input evidence is retained only long enough
/// for the HTTP response to construct the complete Preview identity.
pub(crate) struct ProcessingPreviewExecution {
    pub(crate) bytes: Vec<u8>,
    pub(crate) rendition: slipstream_processing::local_preview::PreviewIdentity,
    pub(crate) input: slipstream_core::ProcessingInputEvidence,
}

/// One executed qualified composable Export: the retained, validated
/// output under its minted artifact identity, the concrete output contract
/// the next step's input binding names, plus the verified byte evidence of
/// the staged input the artifact record must carry.
pub(crate) struct ProcessingExportExecution {
    pub(crate) artifact_id: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) input_sha256: String,
    pub(crate) input_size: u64,
    /// The concrete output contract of the executed module: the darktable
    /// peer's linear ProPhoto TIFF handoff or the standalone SpektraFilm
    /// peer's quality-85 sRGB Finished JPEG.
    pub(crate) output: ExportedOutputContract,
    pub(crate) publication: ProcessingPublication,
}

/// The concrete output contract one executed composable Export produced,
/// named with the persistence boundary's own vocabulary so an artifact
/// record can be formed without re-deriving the module's shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExportedOutputContract {
    pub(crate) format: &'static str,
    pub(crate) precision: &'static str,
    pub(crate) color_space: &'static str,
    pub(crate) transfer: &'static str,
    pub(crate) encoding: &'static str,
}

impl ExportedOutputContract {
    /// The darktable peer's pinned Development TIFF handoff.
    pub(crate) fn development_tiff() -> Self {
        Self {
            format: "tiff",
            precision: "float32",
            color_space: "prophoto-rgb",
            transfer: "linear",
            encoding: "deflate",
        }
    }

    /// The standalone SpektraFilm peer's pinned Finished JPEG.
    pub(crate) fn finished_jpeg() -> Self {
        Self {
            format: "jpeg",
            precision: "uint8",
            color_space: "srgb",
            transfer: "srgb",
            encoding: "quality-85-baseline",
        }
    }
}

/// The concrete input handoff one admitted composable Export executes
/// against. `Original` names the published Library Location and the exact
/// source revision admission captured; `Artifact` names the immutable
/// Processing Artifact the step explicitly selected together with its
/// declared image contract, staged under a download lease so the retention
/// sweep cannot delete the input mid-attempt.
pub(crate) enum ProcessingExportInput {
    Original {
        photo_id: String,
        source_revision: String,
    },
    Artifact {
        artifact_id: String,
        contract: slipstream_core::ProcessingImageContract,
    },
}

fn darktable_original_input(
    photo_id: &str,
    source_revision: &str,
    digest: &str,
    byte_size: u64,
) -> ConfinedInput {
    ConfinedInput {
        source_id: format!("original:{photo_id}:{source_revision}"),
        digest: digest.to_owned(),
        byte_size,
        contract: ImageContract {
            format: "arw".to_owned(),
            color_space: "camera-native".to_owned(),
            transfer_function: "linear".to_owned(),
            precision_bits: 14,
            width: 9504,
            height: 6336,
        },
    }
}

fn artifact_module_input(
    artifact_id: &str,
    contract: &slipstream_core::ProcessingImageContract,
    digest: &str,
    byte_size: u64,
) -> Result<ConfinedInput, String> {
    let precision = contract
        .precision
        .strip_prefix("uint")
        .or_else(|| contract.precision.strip_prefix("float"))
        .and_then(|value| value.parse::<u8>().ok())
        .ok_or_else(|| "artifact precision is not admitted".to_owned())?;
    Ok(ConfinedInput {
        source_id: format!("artifact:{artifact_id}"),
        digest: digest.to_owned(),
        byte_size,
        contract: ImageContract {
            format: contract
                .format
                .strip_prefix("image/")
                .unwrap_or(&contract.format)
                .to_owned(),
            color_space: contract.color_space.clone(),
            transfer_function: contract.transfer.clone(),
            precision_bits: precision,
            width: u64::from(contract.geometry.width),
            height: u64::from(contract.geometry.height),
        },
    })
}

fn admit_module(
    module: &str,
    input: &ConfinedInput,
    parameters: &Parameters,
) -> Result<slipstream_processing::modules::ModuleInvocation, String> {
    ModuleRegistry::new(ModuleAvailability::ready(), ModuleAvailability::ready())
        .admit(module, input, parameters)
        .map_err(|error| error.message)
}

/// One staged artifact input: the private verified copy below the
/// attempt's workspace and the lease guard that holds the published
/// artifact against expiry cleanup until the engine consumed the copy.
/// A guard dropped without `release` leaves its lease to the lease's own
/// staleness window, so a lost attempt can never block cleanup forever.
pub(crate) struct StagedArtifactInput {
    path: PathBuf,
    sha256: String,
    size: u64,
    lease: Option<(Arc<Library>, String)>,
}

impl StagedArtifactInput {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// Release the download lease once the attempt no longer needs the
    /// published artifact; the private copy is removed when the guard
    /// drops.
    pub(crate) async fn release(mut self) {
        if let Some((library, lease_id)) = self.lease.take() {
            let _ = library.release_processing_artifact_lease(&lease_id).await;
        }
    }
}

impl Drop for StagedArtifactInput {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// A service-minted composable artifact identity is opaque outside the
/// service and uses only its closed lower-case identifier alphabet.
static PROCESSING_ARTIFACT_COUNTER: AtomicU64 = AtomicU64::new(0);
static PROCESSING_PREVIEW_COUNTER: AtomicU64 = AtomicU64::new(0);

fn processing_preview_id() -> String {
    let sequence = PROCESSING_PREVIEW_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("cp-{sequence}-{}", std::process::id())
}

fn processing_artifact_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = PROCESSING_ARTIFACT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("pa-{}-{nanos}-{sequence}", std::process::id())
}

impl ExportManager {
    /// Called under heavy admission before the atomic rename. Sweep takes
    /// the same admission slot before it observes claims or deletes bytes.
    fn claim_processing_publication(
        &self,
        artifact_id: &str,
        request_id: &str,
    ) -> ProcessingPublication {
        self.publications
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                artifact_id.to_owned(),
                ProcessingPublicationOwner {
                    request_id: request_id.to_owned(),
                    active: true,
                },
            );
        ProcessingPublication {
            artifact_id: artifact_id.to_owned(),
            publications: Arc::clone(&self.publications),
            admission: Arc::clone(&self.admission),
        }
    }

    /// Renders one selected Original-bound Processing Step directly at the
    /// bounded Preview geometry. The PNG remains private and is discarded
    /// when this call returns; no full-resolution handoff or artifact row is
    /// created.
    pub(crate) async fn render_selected_preview(
        &self,
        photo_id: &str,
        source_revision: &str,
        kind: OriginalKind,
        filename: &str,
        parameters: slipstream_processing::modules::Parameters,
        cancellation: Arc<AtomicBool>,
    ) -> Result<ProcessingPreviewExecution, String> {
        let _slot = self.acquire_heavy_slot().await;
        self.ensure_admissible().await?;
        if cancellation.load(Ordering::Acquire) {
            return Err("preview render cancelled".to_owned());
        }
        let (staged, _) = self
            .stage_preview_original(photo_id, source_revision, kind, filename)
            .await?;
        let staged_facts = staged.facts();
        let module_input = darktable_original_input(
            photo_id,
            source_revision,
            &staged_facts.sha256,
            staged_facts.source_facts.size,
        );
        let invocation = admit_module(
            slipstream_processing::modules::DARKTABLE_MODULE,
            &module_input,
            &parameters,
        )?;
        let attempt_key = processing_preview_id();
        let writer = self
            .begin_preview_output(&attempt_key, ExportTarget::DevelopmentTiff)
            .map_err(|error| format!("preview output staging failed: {error}"))?;
        let output = writer.temporary_path().to_path_buf();
        let rendered = self
            .executor
            .render_selected_step(
                staged.path().to_path_buf(),
                output.clone(),
                invocation.parameters.clone(),
                cancellation.clone(),
            )
            .await;
        let staged_facts = staged.facts();
        let input = slipstream_core::ProcessingInputEvidence::new(
            slipstream_core::ProcessingInput::Original {
                photo_id: photo_id.to_owned(),
                source_revision: source_revision.to_owned(),
            },
            &staged_facts.sha256,
            staged_facts.source_facts.size,
        )
        .map_err(|error| error.to_string())?;
        drop(staged);
        let identity = rendered?;
        if cancellation.load(Ordering::Acquire) {
            return Err("preview render cancelled".to_owned());
        }
        let expected_size = identity.size;
        let expected_sha256 = identity.sha256.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            let bytes =
                fs::read(&output).map_err(|error| format!("preview could not be read: {error}"))?;
            if bytes.len() as u64 != expected_size || bytes.len() > 16 * 1024 * 1024 {
                return Err("preview size changed after rendering".to_owned());
            }
            let mut hasher = sha2::Sha256::new();
            use std::io::Write;
            hasher
                .write_all(&bytes)
                .map_err(|error| error.to_string())?;
            if format!("{:x}", hasher.finalize()) != expected_sha256 {
                return Err("preview digest changed after rendering".to_owned());
            }
            Ok(bytes)
        })
        .await
        .map_err(|error| format!("preview read task failed: {error}"))??;
        Ok(ProcessingPreviewExecution {
            bytes,
            rendition: identity,
            input,
        })
    }
    /// Executes one admitted qualified composable Export of an
    /// Original-bound darktable step through the confined local development
    /// workload with its frozen parameter tree. The validated Development
    /// TIFF is published into the retained artifact namespace under the
    /// minted artifact identity, and the staged source's verified byte
    /// evidence travels with the result for the artifact record. Nothing is
    /// recorded in the Library here; settlement is the caller's atomic step.
    pub(crate) async fn run_processing_export(
        &self,
        request_id: &str,
        photo_id: &str,
        source_revision: &str,
        parameters: Parameters,
    ) -> Result<ProcessingExportExecution, String> {
        self.execute_processing_export(
            request_id,
            ProcessingExportInput::Original {
                photo_id: photo_id.to_owned(),
                source_revision: source_revision.to_owned(),
            },
            parameters,
        )
        .await
    }

    /// Executes one admitted qualified composable Export of an
    /// artifact-bound standalone SpektraFilm step through the confined
    /// local film workload with its frozen complete parameter tree. The
    /// validated Finished JPEG is published into the retained artifact
    /// namespace under the minted artifact identity, and the staged
    /// artifact input's verified byte evidence travels with the result for
    /// the artifact record. Nothing is recorded in the Library here;
    /// settlement is the caller's atomic step.
    pub(crate) async fn run_film_export(
        &self,
        request_id: &str,
        artifact_id: &str,
        contract: &slipstream_core::ProcessingImageContract,
        parameters: Parameters,
    ) -> Result<ProcessingExportExecution, String> {
        self.execute_processing_export(
            request_id,
            ProcessingExportInput::Artifact {
                artifact_id: artifact_id.to_owned(),
                contract: contract.clone(),
            },
            parameters,
        )
        .await
    }

    /// Renders one bounded selected-step standalone SpektraFilm Preview
    /// over an explicitly selected artifact input. The PNG rendition is
    /// private and discarded when this call returns; the input is staged
    /// under the same lease protection as an Export, and no
    /// full-resolution handoff or artifact row is created.
    pub(crate) async fn render_film_preview(
        &self,
        artifact_id: &str,
        contract: &slipstream_core::ProcessingImageContract,
        parameters: Parameters,
        cancellation: Arc<AtomicBool>,
    ) -> Result<ProcessingPreviewExecution, String> {
        let _slot = self.acquire_heavy_slot().await;
        self.ensure_film_admissible().await?;
        if cancellation.load(Ordering::Acquire) {
            return Err("preview render cancelled".to_owned());
        }
        let staged = self.stage_artifact_input(artifact_id, contract).await?;
        let module_input =
            artifact_module_input(artifact_id, contract, staged.sha256(), staged.size())?;
        let invocation = admit_module(
            slipstream_processing::modules::SPEKTRAFILM_MODULE,
            &module_input,
            &parameters,
        )?;
        let attempt_key = processing_preview_id();
        let writer = self
            .begin_preview_output(&attempt_key, ExportTarget::DevelopmentTiff)
            .map_err(|error| format!("preview output staging failed: {error}"))?;
        let output = writer.temporary_path().to_path_buf();
        let rendered = self
            .executor
            .render_film_selected_step(
                staged.path().to_path_buf(),
                output.clone(),
                invocation.parameters.clone(),
                cancellation.clone(),
            )
            .await;
        let input = slipstream_core::ProcessingInputEvidence::new(
            slipstream_core::ProcessingInput::Artifact {
                artifact_id: slipstream_core::ProcessingArtifactId::new(artifact_id)
                    .map_err(|error| error.to_string())?,
                contract: contract.clone(),
            },
            staged.sha256(),
            staged.size(),
        )
        .map_err(|error| error.to_string())?;
        staged.release().await;
        let identity = rendered?;
        if cancellation.load(Ordering::Acquire) {
            return Err("preview render cancelled".to_owned());
        }
        let expected_size = identity.size;
        let expected_sha256 = identity.sha256.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            let bytes =
                fs::read(&output).map_err(|error| format!("preview could not be read: {error}"))?;
            if bytes.len() as u64 != expected_size || bytes.len() > 16 * 1024 * 1024 {
                return Err("preview size changed after rendering".to_owned());
            }
            let mut hasher = sha2::Sha256::new();
            use std::io::Write;
            hasher
                .write_all(&bytes)
                .map_err(|error| error.to_string())?;
            if format!("{:x}", hasher.finalize()) != expected_sha256 {
                return Err("preview digest changed after rendering".to_owned());
            }
            Ok(bytes)
        })
        .await
        .map_err(|error| format!("preview read task failed: {error}"))??;
        Ok(ProcessingPreviewExecution {
            bytes,
            rendition: identity,
            input,
        })
    }

    /// The retained artifact file of one composable artifact identity for
    /// the module that produced it: the darktable peer's TIFF handoff or
    /// the standalone SpektraFilm peer's Finished JPEG. The path stays
    /// private to the service; responses carry identity facts only.
    pub(crate) fn artifact_path_for_module(
        &self,
        artifact_id: &str,
        module: &str,
    ) -> Option<PathBuf> {
        let workload = match module {
            slipstream_processing::modules::SPEKTRAFILM_MODULE => "film-jpeg",
            _ => "development-tiff",
        };
        self.artifact_path_for_workload(artifact_id, workload)
    }

    /// The generalized composable execution seam. Admission inputs are not
    /// Original-bound forever: the enum names the concrete input handoff the
    /// deployment qualified, and only the staging step below depends on the
    /// variant. Execution, validation, and publication are input-agnostic.
    pub(crate) async fn execute_processing_export(
        &self,
        request_id: &str,
        input: ProcessingExportInput,
        parameters: Parameters,
    ) -> Result<ProcessingExportExecution, String> {
        match parameters.module.as_str() {
            slipstream_processing::modules::SPEKTRAFILM_MODULE => {
                self.ensure_film_admissible().await?;
                self.execute_film_export(request_id, input, parameters)
                    .await
            }
            _ => {
                self.ensure_admissible().await?;
                self.execute_darktable_export(request_id, input, parameters)
                    .await
            }
        }
    }

    /// Pre-acceptance admission of one standalone SpektraFilm attempt: the
    /// peer's own runtime must be verified and the shared slot openable.
    /// Never derived from the darktable stage's availability.
    pub(crate) async fn ensure_film_admissible(&self) -> Result<(), String> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("Photo Development is shutting down".to_owned());
        }
        self.executor.film_available()
    }

    /// The Original-bound darktable execution the first qualified workload
    /// runs: stage the guarded source, develop the frozen stack, validate,
    /// and publish the Development TIFF handoff.
    async fn execute_darktable_export(
        &self,
        request_id: &str,
        input: ProcessingExportInput,
        parameters: Parameters,
    ) -> Result<ProcessingExportExecution, String> {
        let _slot = self.admission.lock().await;
        let artifact_id = processing_artifact_id();
        let staged = match &input {
            ProcessingExportInput::Original {
                photo_id,
                source_revision,
            } => self.stage_original_for(photo_id, source_revision).await?,
            ProcessingExportInput::Artifact { .. } => {
                return Err(
                    "the darktable module admits only its explicitly bound Original input"
                        .to_owned(),
                );
            }
        };
        let staged_facts = staged.facts();
        let input_sha256 = staged_facts.sha256.clone();
        let input_size = staged_facts.source_facts.size;
        let module_input = match &input {
            ProcessingExportInput::Original {
                photo_id,
                source_revision,
            } => darktable_original_input(photo_id, source_revision, &input_sha256, input_size),
            ProcessingExportInput::Artifact { .. } => {
                return Err("the darktable module admits only its Original input".to_owned());
            }
        };
        let invocation = admit_module(
            slipstream_processing::modules::DARKTABLE_MODULE,
            &module_input,
            &parameters,
        )?;
        let target = ExportTarget::DevelopmentTiff;

        // The private output the engine writes; validation gates any
        // publication, and dropping the writer discards a partial output.
        let writer = self
            .workspace
            .begin_artifact(&artifact_id, target)
            .map_err(|error| format!("output staging failed: {error}"))?;
        let output_path = writer.temporary_path().to_path_buf();

        // The running token is registered by request identity so the durable
        // cancel route can terminate this exact engine attempt.
        let cancellation = self.begin_running(request_id);
        let developed = self
            .executor
            .develop_selected_step(
                staged.path().to_path_buf(),
                output_path.clone(),
                invocation.parameters.clone(),
                Arc::clone(&cancellation),
            )
            .await;
        self.end_running(request_id, &cancellation);
        drop(staged);
        let identity = developed?;

        // Verify the developed bytes against the executor's report and the
        // closed Development TIFF contract before anything is published.
        let validation_path = output_path.clone();
        let facts = tokio::task::spawn_blocking(move || {
            verify_developed_output(&validation_path, &identity, target)
        })
        .await
        .map_err(|error| format!("validation task failed: {error}"))?
        .map_err(|_| OUTPUT_VALIDATION_FAILED.to_owned())?;

        let publication = self.claim_processing_publication(&artifact_id, request_id);
        let published = match writer.publish(|path| validate_output(path, target).map(|_| ())) {
            Ok(published) => published,
            Err(error) => {
                publication.clear();
                return Err(format!("artifact publication failed: {error}"));
            }
        };
        Ok(ProcessingExportExecution {
            artifact_id,
            size: published.size,
            sha256: published.sha256,
            width: facts.width,
            height: facts.height,
            input_sha256,
            input_size,
            output: ExportedOutputContract::development_tiff(),
            publication,
        })
    }

    /// The artifact-bound standalone SpektraFilm execution (Issue #496):
    /// stage the explicitly selected immutable artifact under a download
    /// lease, verify its byte identity and concrete contract, run the
    /// pinned runtime once over the frozen complete parameter tree, and
    /// publish the validated Finished JPEG. No darktable invocation, no
    /// implicit chain, and no full-resolution Preview reuse ever happens
    /// here.
    async fn execute_film_export(
        &self,
        request_id: &str,
        input: ProcessingExportInput,
        parameters: Parameters,
    ) -> Result<ProcessingExportExecution, String> {
        let ProcessingExportInput::Artifact {
            artifact_id: input_artifact_id,
            contract,
        } = &input
        else {
            return Err(
                "the standalone SpektraFilm module admits only its explicitly selected artifact input"
                    .to_owned(),
            );
        };
        let _slot = self.admission.lock().await;
        let artifact_id = processing_artifact_id();
        let staged = self
            .stage_artifact_input(input_artifact_id, contract)
            .await?;
        let input_sha256 = staged.sha256().to_owned();
        let input_size = staged.size();
        let module_input =
            artifact_module_input(input_artifact_id, contract, &input_sha256, input_size)?;
        let invocation = admit_module(
            slipstream_processing::modules::SPEKTRAFILM_MODULE,
            &module_input,
            &parameters,
        )?;
        let target = ExportTarget::FilmJpeg;

        let writer = self
            .workspace
            .begin_artifact(&artifact_id, target)
            .map_err(|error| format!("output staging failed: {error}"))?;
        let output_path = writer.temporary_path().to_path_buf();

        let cancellation = self.begin_running(request_id);
        let developed = self
            .executor
            .develop_film_selected_step(
                staged.path().to_path_buf(),
                output_path.clone(),
                invocation.parameters.clone(),
                Arc::clone(&cancellation),
            )
            .await;
        self.end_running(request_id, &cancellation);
        staged.release().await;
        let identity = developed?;

        let validation_path = output_path.clone();
        let facts = tokio::task::spawn_blocking(move || {
            verify_developed_output(&validation_path, &identity, target)
        })
        .await
        .map_err(|error| format!("validation task failed: {error}"))?
        .map_err(|_| OUTPUT_VALIDATION_FAILED.to_owned())?;

        let publication = self.claim_processing_publication(&artifact_id, request_id);
        let published = match writer.publish(|path| validate_output(path, target).map(|_| ())) {
            Ok(published) => published,
            Err(error) => {
                publication.clear();
                return Err(format!("artifact publication failed: {error}"));
            }
        };
        Ok(ProcessingExportExecution {
            artifact_id,
            size: published.size,
            sha256: published.sha256,
            width: facts.width,
            height: facts.height,
            input_sha256,
            input_size,
            output: ExportedOutputContract::finished_jpeg(),
            publication,
        })
    }

    /// Stage one explicitly selected immutable Processing Artifact input:
    /// acquire its download lease so the retention sweep cannot delete the
    /// input mid-attempt, copy the published bytes into the attempt's
    /// private workspace, and verify the digest, byte length, and the
    /// closed Development TIFF contract against the binding the recipe
    /// captured. A missing, expired, or mismatched artifact fails; it
    /// never resolves a newer upstream result.
    async fn stage_artifact_input(
        &self,
        artifact_id: &str,
        contract: &slipstream_core::ProcessingImageContract,
    ) -> Result<StagedArtifactInput, String> {
        if slipstream_core::ProcessingArtifactId::new(artifact_id).is_err() {
            return Err("the selected artifact identity is invalid".to_owned());
        }
        // The lease carries the published record: its byte identity is the
        // digest the staged copy must reproduce, and its retention is held
        // for the whole attempt so the sweep cannot delete the input.
        let (mut lease, record) = match self
            .library
            .acquire_processing_artifact_lease(artifact_id, unix_seconds())
            .await
        {
            Ok(slipstream_core::ProcessingArtifactLeaseOutcome::Acquired {
                lease_id,
                artifact,
            }) => (Some((Arc::clone(&self.library), lease_id)), *artifact),
            Ok(slipstream_core::ProcessingArtifactLeaseOutcome::Expired) => {
                return Err("the selected artifact input has expired".to_owned());
            }
            Ok(_) => return Err("the selected artifact input is unknown".to_owned()),
            Err(_) => return Err("the artifact input lease could not be persisted".to_owned()),
        };
        // The binding's contract must be the record's own published
        // contract: a step bound to an older contract view never runs
        // against bytes it did not name.
        if record.artifact_id.as_str() != artifact_id {
            let _ = self.release_quietly(lease.take()).await;
            return Err("the selected artifact input is unknown".to_owned());
        }
        let published_contract = &record.output_contract;
        if published_contract.format != contract.format
            || published_contract.precision != contract.precision
            || published_contract.color_space != contract.color_space
            || published_contract.transfer != contract.transfer
            || published_contract.encoding != contract.encoding
            || published_contract.geometry.width != contract.geometry.width
            || published_contract.geometry.height != contract.geometry.height
        {
            let _ = self.release_quietly(lease.take()).await;
            return Err("the artifact input no longer matches its captured contract".to_owned());
        }
        let Some(published) = self.artifact_path_for_workload(artifact_id, "development-tiff")
        else {
            let _ = self.release_quietly(lease.take()).await;
            return Err("the selected artifact input has no retained bytes".to_owned());
        };
        let expected_sha256 = record.sha256.clone();
        let expected_size = record.byte_length;
        let staged_root = self.workspace.root().join("staged-inputs");
        let attempt = format!("si-{artifact_id}-{}", processing_preview_id());
        let mut staged = match tokio::task::spawn_blocking(move || {
            stage_artifact_bytes(
                &staged_root,
                &attempt,
                &published,
                &expected_sha256,
                expected_size,
            )
        })
        .await
        .map_err(|error| format!("staging worker failed: {error}"))?
        {
            Ok(staged) => staged,
            Err(error) => {
                let _ = self.release_quietly(lease.take()).await;
                return Err(error);
            }
        };
        staged.lease = lease;
        Ok(staged)
    }

    async fn release_quietly(&self, lease: Option<(Arc<Library>, String)>) -> Result<(), String> {
        if let Some((library, lease_id)) = lease {
            let _ = library.release_processing_artifact_lease(&lease_id).await;
        }
        Ok(())
    }
}

/// Copy one leased artifact input into its private staging path and verify
/// the copy against the published record's byte identity and the film
/// module boundary's own closed Development TIFF admission. The copy is
/// removed by the returned guard's drop.
fn stage_artifact_bytes(
    staged_root: &Path,
    attempt: &str,
    published: &Path,
    expected_sha256: &str,
    expected_size: u64,
) -> Result<StagedArtifactInput, String> {
    std::fs::create_dir_all(staged_root).map_err(|error| error.to_string())?;
    let path = staged_root.join(attempt);
    std::fs::copy(published, &path).map_err(|error| error.to_string())?;
    let identity = slipstream_processing::local_film::validate_input(&path)
        .map_err(|error| format!("the artifact input contract was refused: {error}"))?;
    if identity.size != expected_size || identity.sha256 != expected_sha256 {
        let _ = fs::remove_file(&path);
        return Err("the artifact input bytes no longer match its record".to_owned());
    }
    Ok(StagedArtifactInput {
        path,
        sha256: identity.sha256,
        size: identity.size,
        lease: None,
    })
}
