use super::*;
/// Longest admitted byte length of one composable Export request identity.
pub const MAXIMUM_EXPORT_REQUEST_ID_BYTES: usize = 128;

/// Why one [`SubmitProcessingExport`] request is not admissible. Every
/// refusal is strict and terminal for the request: nothing was read or
/// written for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessingExportRequestError {
    /// The request identity, the Photo identity, the selected step, an
    /// expected revision, or the adapter decision is not an admissible
    /// composable-processing value.
    Contract(ProcessingContractError),
    /// The deployment's adapter decision does not carry a bounded refusal
    /// reason code.
    InvalidAdapterDecision,
}

impl fmt::Display for ProcessingExportRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => write!(formatter, "request is not admissible: {error}"),
            Self::InvalidAdapterDecision => {
                formatter.write_str("adapter decision carries no bounded refusal reason code")
            }
        }
    }
}

impl std::error::Error for ProcessingExportRequestError {}

/// The deployment's adapter qualification for one explicit composable
/// Export, decided from the selected module before admission.
///
/// The host vocabulary cannot decide engine qualification: the deployment
/// that owns the processing bundle does, and admission records exactly the
/// decision it carried. No variant licenses artifact publication without a
/// qualified adapter; when a qualified adapter exists, the identity it
/// pinned at acceptance will be carried here and validated at the executor
/// boundary before any artifact is published.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessingExportAdapterDecision {
    /// This deployment has no qualified adapter for the selected module's
    /// complete parameter tree. Admission captures the selected current
    /// step's exact identity, records one explicit refusal, and publishes
    /// no artifact. The carried code is the closed wire reason code of the
    /// refusal.
    NoQualifiedAdapter { reason_code: String },
    /// The deployment's qualified adapter identity for the selected
    /// module's complete parameter tree: the pinned adapter version and
    /// the parameter-schema version it executes. Admission captures the
    /// selected step against this identity, and the executor must validate
    /// the same pinned identity before any artifact is published.
    Qualified {
        adapter_version: String,
        parameter_schema_version: String,
    },
}

impl ProcessingExportAdapterDecision {
    /// Validates the decision's bounded identities.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        match self {
            Self::NoQualifiedAdapter { reason_code } => {
                validate_bounded_name(reason_code, MAXIMUM_CONTRACT_NAME_BYTES)
            }
            Self::Qualified {
                adapter_version,
                parameter_schema_version,
            } => {
                validate_bounded_name(adapter_version, MAXIMUM_CONTRACT_NAME_BYTES)?;
                validate_bounded_name(parameter_schema_version, MAXIMUM_CONTRACT_NAME_BYTES)
            }
        }
    }
}

/// One admitted explicit composable Export awaiting execution: everything
/// admission captured from the same serialized read — the exact selected
/// step with its complete parameter snapshot and input binding, the guards
/// it passed, the caller's request identity and payload digest, and the
/// qualified adapter identity the executor must revalidate before any
/// artifact is published. Nothing here licenses execution by an adapter
/// other than the one named by `adapter_schema_version`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessingExportAdmission {
    pub photo_id: String,
    pub request_id: String,
    /// The canonical digest of the caller's request payload, binding one
    /// request identity to exactly one admitted intent.
    pub payload_digest: String,
    pub step_id: ProcessingStepId,
    pub recipe_revision: String,
    pub source_revision: String,
    pub module: ProcessingModuleId,
    /// The pinned adapter and parameter-schema identity of the qualified
    /// decision that admitted this execution.
    pub adapter_schema_version: String,
    /// The complete captured parameter snapshot, preserved verbatim.
    pub parameters: ProcessingParameterSnapshot,
    /// The input binding exactly as saved in the selected step.
    pub input: ProcessingInput,
    pub bundle_id: String,
}

impl ProcessingExportAdmission {
    /// Validates every captured component.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        validate_bounded_name(&self.photo_id, MAXIMUM_PHOTO_ID_BYTES)?;
        validate_bounded_name(&self.request_id, MAXIMUM_EXPORT_REQUEST_ID_BYTES)?;
        validate_digest(&self.payload_digest)?;
        validate_bounded_name(&self.recipe_revision, MAXIMUM_REVISION_BYTES)?;
        validate_revision(&self.source_revision)?;
        validate_bounded_name(&self.adapter_schema_version, MAXIMUM_CONTRACT_NAME_BYTES)?;
        self.parameters.validate()?;
        self.input.validate()?;
        validate_bounded_name(&self.bundle_id, MAXIMUM_CONTRACT_NAME_BYTES)?;
        Ok(())
    }
}

/// One guarded request to submit the selected current Processing Step of a
/// Photo for an explicit Export.
///
/// The request never carries settings of its own: admission captures the
/// stored recipe's selected current step exactly as saved, guarded by the
/// recipe revision and the published source revision the caller observed.
/// The deployment's adapter decision travels with the request, so a
/// recorded refusal always names the qualification that produced it.
#[derive(Clone, Debug, PartialEq)]
pub struct SubmitProcessingExport {
    /// The Photo whose selected current step is submitted.
    pub photo_id: String,
    /// Stable caller-owned identity used to resolve a retry after a lost
    /// response. The recorded decision replays under this identity, and a
    /// different payload under it is refused.
    pub request_id: String,
    /// The Processing Step the caller submitted for explicit Export.
    pub step_id: ProcessingStepId,
    /// The composable recipe revision the caller observed.
    pub expected_recipe_revision: String,
    /// The observed source revision the submission guards against.
    pub expected_source_revision: String,
    /// The exact processing bundle identity of the deployment that decided
    /// adapter qualification. It is deployment state, not caller intent: a
    /// recorded refusal names the bundle that produced it, while the
    /// request's payload digest deliberately excludes it.
    pub bundle_id: String,
    /// The deployment's finite retained-output allowance. Qualified
    /// admission reserves the bounded maximum artifact size inside it before
    /// durable work is accepted.
    pub retained_output_bytes_max: u64,
    /// The deployment's adapter qualification decision for the selected
    /// module's complete parameter tree.
    pub adapter: ProcessingExportAdapterDecision,
}

/// The durable record of one explicitly refused composable Export: the
/// exact captured identity of the selected current step at admission, plus
/// the deployment's adapter refusal that stopped execution before any
/// artifact was published.
///
/// The record carries the complete parameter snapshot's schema version and
/// canonical digest, the input binding identity exactly as saved, and the
/// processing bundle of the refusing deployment. It deliberately carries
/// neither input byte evidence nor a derived output contract: both are
/// confirmed only by a qualified adapter, and a refusal proves neither.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingExportRefusal {
    pub photo_id: String,
    /// The request identity whose recorded decision this is.
    pub request_id: String,
    /// The canonical digest of the caller's request payload, binding one
    /// request identity to exactly one admitted intent.
    pub payload_digest: String,
    pub step_id: ProcessingStepId,
    /// The stored recipe revision the refusal was captured against.
    pub recipe_revision: String,
    /// The published source revision the refusal was captured against.
    pub source_revision: String,
    pub module: ProcessingModuleId,
    /// The module-owned parameter-schema version of the captured snapshot.
    pub parameter_schema_version: String,
    /// The canonical digest of the complete captured parameter snapshot.
    pub parameter_digest: String,
    /// The input binding identity exactly as saved in the selected step.
    pub input: ProcessingInput,
    /// The exact processing bundle identity of the refusing deployment.
    pub bundle_id: String,
    /// The closed refusal reason code carried by the adapter decision.
    pub reason_code: String,
}

impl ProcessingExportRefusal {
    /// Validates every component of the recorded refusal.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        validate_bounded_name(&self.photo_id, MAXIMUM_PHOTO_ID_BYTES)?;
        validate_bounded_name(&self.request_id, MAXIMUM_EXPORT_REQUEST_ID_BYTES)?;
        validate_digest(&self.payload_digest)?;
        validate_bounded_name(&self.recipe_revision, MAXIMUM_REVISION_BYTES)?;
        validate_revision(&self.source_revision)?;
        validate_bounded_name(&self.parameter_schema_version, MAXIMUM_CONTRACT_NAME_BYTES)?;
        validate_digest(&self.parameter_digest)?;
        self.input.validate()?;
        validate_bounded_name(&self.bundle_id, MAXIMUM_CONTRACT_NAME_BYTES)?;
        validate_bounded_name(&self.reason_code, MAXIMUM_CONTRACT_NAME_BYTES)?;
        Ok(())
    }
}

/// Why one selected Processing Step's input binding failed the concrete
/// handoff check at Export admission. Each case is a strict refusal: no
/// newer upstream result, no implicit conversion, and no fallback input is
/// ever resolved in its place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessingInputHandoffError {
    /// The bound Original names a different Photo than the recipe's Photo.
    OriginalPhotoMismatch,
    /// The bound Original's source revision is no longer the published
    /// revision of its Photo.
    OriginalSourceStale,
    /// The bound artifact has no published Processing Artifact record: it
    /// is missing or expired, and no newer upstream result resolves in its
    /// place.
    ArtifactMissing,
    /// The bound artifact exists, but its saved concrete image contract no
    /// longer equals the artifact's published output contract.
    ArtifactContractMismatch,
}

impl fmt::Display for ProcessingInputHandoffError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OriginalPhotoMismatch => {
                formatter.write_str("an Original input is bound to a different Photo")
            }
            Self::OriginalSourceStale => {
                formatter.write_str("an Original input is bound to a stale source revision")
            }
            Self::ArtifactMissing => {
                formatter.write_str("a processing artifact input is missing or expired")
            }
            Self::ArtifactContractMismatch => formatter
                .write_str("a processing artifact input no longer matches its published contract"),
        }
    }
}

impl std::error::Error for ProcessingInputHandoffError {}

/// The guarded outcome of one composable Export submission.
#[derive(Clone, Debug, PartialEq)]
pub enum ProcessingExportSubmitOutcome {
    /// Admission captured the selected current step's exact identity and
    /// recorded the deployment's explicit adapter refusal. Nothing was
    /// executed and no artifact was published.
    Refused(ProcessingExportRefusal),
    /// A replay of one recorded refusal under the same request identity
    /// and payload: the committed refusal, unchanged.
    Replayed(ProcessingExportRefusal),
    /// The deployment's qualified adapter admitted the selected step, and
    /// the accepted work record was committed in the same transaction: the
    /// captured admission is the receipt execution must present before any
    /// artifact is published. Nothing has been executed yet.
    Admitted(ProcessingExportAdmission),
    /// A replay of one still-live accepted execution under the same request
    /// identity and payload: the committed admission, whose work record is
    /// still accepted or executing. No second execution was started.
    Pending(ProcessingExportAdmission),
    /// A replay of one terminally failed execution under the same request
    /// identity and payload: the committed work record with its bounded
    /// failure reason, unchanged.
    FailureReplayed(ProcessingExportWork),
    /// A replay of one cancelled execution under the same request identity
    /// and payload: the committed work record, unchanged.
    CancelledReplayed(ProcessingExportWork),
    /// A replay of one settled execution under the same request identity
    /// and payload: the committed Processing Artifact, unchanged.
    ArtifactReplayed(ProcessingArtifact),
    /// The request identity reached a terminal state whose finite retention
    /// has since expired: the settled artifact, or the terminally failed or
    /// cancelled receipt, is gone, the identity can never start new work,
    /// and no newer result resolves in its place.
    Expired,
    /// The Library already holds the maximum number of live composable
    /// Export work records; nothing was read or written for this request.
    ReservationFull,
    /// The deployment's retained-output allowance cannot reserve another
    /// bounded artifact before durable work admission.
    RetainedOutputFull,
    /// The caller's expected recipe revision is stale. Carries the
    /// currently stored recipe, if any, from the same serialized read.
    RecipeConflict(Option<ComposableEditRecipe>),
    /// The Photo's published source revision moved past the caller's
    /// guard. Carries the currently stored recipe, if any, from the same
    /// serialized read.
    SourceChanged(Option<ComposableEditRecipe>),
    /// The Photo does not exist.
    MissingPhoto,
    /// The Photo's source is unavailable, or no published Capture fact is
    /// bound to the observed source facts, so no guarded admission exists.
    Unavailable,
    /// No composable recipe has been saved for the Photo.
    MissingRecipe,
    /// The submitted step is not the recipe's selected current step;
    /// carries the recipe's current selection, if any.
    StepNotCurrent(Option<ProcessingStepId>),
    /// The selected step's input binding failed the concrete handoff
    /// check: a stale Original, or a missing, expired, or
    /// contract-mismatched artifact.
    IncompatibleInput(ProcessingInputHandoffError),
    /// The request is not admissible; nothing was read or written for it.
    Invalid(ProcessingExportRequestError),
    /// The request identity was already used with a different payload.
    RequestConflict,
}

/// One published immutable Processing Artifact: the complete provenance and
/// byte identity of one validated Export result.
///
/// Publication is insert-only: an artifact identity resolves to exactly one
/// record forever, an identical re-publication is an idempotent replay, and
/// any different record under the same identity is refused. The record
/// retains everything acceptance captured — the Photo, the selected step
/// and module with its pinned adapter/schema version, the complete captured
/// parameter snapshot preserved verbatim, the input binding with its
/// verified byte evidence, the intended and validated output contract, and
/// the processing bundle — plus the identity of the published bytes
/// themselves. A later step selects this record through an explicit
/// artifact input binding; a mutable "latest result" of a step is never an
/// artifact.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessingArtifact {
    pub artifact_id: ProcessingArtifactId,
    pub photo_id: String,
    pub step_id: ProcessingStepId,
    pub module: ProcessingModuleId,
    /// The pinned adapter and parameter-schema identity the validated
    /// execution used.
    pub adapter_schema_version: String,
    /// The complete captured parameter snapshot, preserved verbatim.
    pub parameters: ProcessingParameterSnapshot,
    /// The confirmed input binding with its verified byte evidence.
    pub input: ProcessingInputEvidence,
    /// The intended and validated output contract of the published bytes.
    pub output_contract: ProcessingImageContract,
    /// The exact processing bundle identity.
    pub bundle_id: String,
    /// SHA-256 of the published artifact bytes, as lowercase hex.
    pub sha256: String,
    /// The published artifact's byte length; at least one.
    pub byte_length: u64,
}

impl ProcessingArtifact {
    /// Validates every component of the published record.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        validate_bounded_name(&self.photo_id, MAXIMUM_PHOTO_ID_BYTES)?;
        validate_bounded_name(&self.adapter_schema_version, MAXIMUM_CONTRACT_NAME_BYTES)?;
        self.parameters.validate()?;
        self.input.validate()?;
        self.output_contract.validate()?;
        validate_bounded_name(&self.bundle_id, MAXIMUM_CONTRACT_NAME_BYTES)?;
        validate_digest(&self.sha256)?;
        if self.byte_length == 0 {
            return Err(ProcessingContractError::ZeroByteEvidence);
        }
        Ok(())
    }

    /// Whether two records are one identical publication: every field,
    /// including the artifact identity, equal. Immutability makes this the
    /// only admissible overlap between two publications of one identity.
    pub fn same_publication(&self, other: &Self) -> bool {
        self == other
    }
}

/// The outcome of publishing one Processing Artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessingArtifactPublication {
    /// The record was installed under a fresh artifact identity.
    Published,
    /// An identical record was already published under the same identity:
    /// nothing was written.
    Replayed,
    /// A different record already owns this artifact identity; the
    /// submitted publication was refused unchanged.
    IdentityConflict,
}

/// The finite retention window of one terminally failed or cancelled
/// composable Export work record, measured in unix seconds from the
/// terminal decision. Until the window passes, a replay of the request
/// identity returns the committed terminal record; after the sweep removes
/// it past the window, the tombstoned identity answers `Expired` and can
/// never start new work.
pub const PROCESSING_EXPORT_RECEIPT_RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60;

/// The finite retention window of one published Processing Artifact,
/// measured in unix seconds from the settlement that published it. When the
/// window passes and no download lease holds the artifact, the sweep removes
/// the artifact record, its retention, and its terminal work, and tombstones
/// the request identity so it can never start new work.
pub const PROCESSING_ARTIFACT_RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60;

/// Largest admitted number of simultaneously live composable Export work
/// records — accepted or executing — in one Library. A further qualified
/// submission is refused with `ReservationFull` until live work settles, so
/// the durable work store stays finite.
pub const MAXIMUM_LIVE_PROCESSING_EXPORTS: usize = 256;

/// The durable lifecycle state of one admitted composable Export. `Accepted`
/// is the committed receipt that execution may begin; `Executing` records a
/// durably begun attempt; `Succeeded`, `Failed`, and `Cancelled` are
/// terminal, and the first terminal decision wins.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProcessingExportWorkState {
    /// The request was durably admitted for execution; no attempt has
    /// begun.
    Accepted,
    /// One execution attempt durably began and has not settled.
    Executing,
    /// Execution settled a published Processing Artifact.
    Succeeded,
    /// Execution terminally failed with a bounded reason code.
    Failed,
    /// The request was explicitly cancelled.
    Cancelled,
}

impl ProcessingExportWorkState {
    /// The closed stored name of the state.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Executing => "executing",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses one stored state name strictly; any other value is not a
    /// state.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "accepted" => Some(Self::Accepted),
            "executing" => Some(Self::Executing),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Whether the state is terminal: no further lifecycle decision may
    /// change the record, and the first terminal decision wins.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// One durably begun execution attempt of an admitted composable Export.
/// A restart reconciliation that re-begins unfinished work allocates the
/// next sequence, so a crashed attempt is superseded, never reused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessingExportAttempt {
    /// The one-based attempt sequence within the request.
    pub sequence: u64,
    /// Unix seconds when the attempt durably began.
    pub began_at: u64,
}

/// The durable work record of one admitted composable Export: the accepted
/// admission — which is the accepted receipt execution must present before
/// any artifact is published — plus its lifecycle state, its latest begun
/// attempt, and its first terminal decision. The record exists before
/// execution, survives restarts unchanged while live, and keeps its first
/// terminal decision for its finite retention window, until a sweep removes
/// a succeeded record together with its expired artifact or a failed or
/// cancelled receipt past its own deadline.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessingExportWork {
    /// The admission captured when the request was durably accepted.
    pub admission: ProcessingExportAdmission,
    /// The lifecycle state of the work.
    pub state: ProcessingExportWorkState,
    /// Unix seconds when the work was durably accepted.
    pub accepted_at: u64,
    /// The latest durably begun attempt; absent until one begins.
    pub attempt: Option<ProcessingExportAttempt>,
    /// The settled artifact identity; present exactly when the state is
    /// `Succeeded`.
    pub artifact_id: Option<ProcessingArtifactId>,
    /// The bounded failure reason code; present exactly when the state is
    /// `Failed`.
    pub failure_reason: Option<String>,
    /// Unix seconds when the terminal decision was recorded; present
    /// exactly when the state is terminal.
    pub terminal_at: Option<u64>,
    /// The record's retention deadline in unix seconds, present exactly
    /// when the state is terminal: the artifact's finite retention window
    /// when `Succeeded`, and the terminal receipt's finite retention window
    /// when `Failed` or `Cancelled`. Past the deadline the sweep removes the
    /// record and tombstones the request identity.
    pub retain_until: Option<u64>,
}

impl ProcessingExportWork {
    /// Validates every admissible component of the record: the complete
    /// admission and, where present, the bounded failure reason code. The
    /// state-dependent shape invariants — which optional fields may accompany
    /// which state — are enforced where the record is reconstructed, and a
    /// record that violates them is a storage error.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        self.admission.validate()?;
        if let Some(reason) = self.failure_reason.as_deref() {
            validate_bounded_name(reason, MAXIMUM_CONTRACT_NAME_BYTES)?;
        }
        Ok(())
    }
}

/// The guarded outcome of durably beginning one execution attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum ProcessingExportAttemptOutcome {
    /// The attempt durably began: the work is executing under a fresh
    /// attempt sequence.
    Began(ProcessingExportWork),
    /// The work already reached a terminal decision; the first terminal
    /// decision wins and the committed record is returned unchanged.
    Terminal(ProcessingExportWork),
    /// No work record exists under the request identity.
    Missing,
}

/// The guarded outcome of recording one terminal execution failure.
#[derive(Clone, Debug, PartialEq)]
pub enum ProcessingExportFailureOutcome {
    /// The terminal failure was recorded with its bounded reason code.
    Failed(ProcessingExportWork),
    /// The work already reached a terminal decision; the first terminal
    /// decision wins and the committed record is returned unchanged.
    Terminal(ProcessingExportWork),
    /// No work record exists under the request identity.
    Missing,
    /// The failure reason code is not an admissible bounded name.
    Invalid(ProcessingContractError),
}

/// The guarded outcome of cancelling one admitted composable Export.
#[derive(Clone, Debug, PartialEq)]
pub enum ProcessingExportCancelOutcome {
    /// The cancellation was recorded; live execution must stop.
    Cancelled(ProcessingExportWork),
    /// The work already reached a terminal decision; the first terminal
    /// decision wins and the committed record is returned unchanged.
    Terminal(ProcessingExportWork),
    /// No work record exists under the request identity.
    Missing,
}

/// The guarded outcome of settling one admitted composable Export.
#[derive(Clone, Debug, PartialEq)]
pub enum ProcessingExportSettlement {
    /// The validated artifact was published — freshly or as an identical
    /// replay — and the work was recorded `Succeeded` with its finite
    /// artifact retention in the same transaction.
    Settled(ProcessingExportWork),
    /// A replay of one already-settled request identity, payload, and
    /// artifact: the committed work record, unchanged.
    Replayed(ProcessingExportWork),
    /// The work already reached a different terminal decision; the first
    /// terminal decision wins and the committed record is returned
    /// unchanged.
    Terminal(ProcessingExportWork),
    /// No accepted work record exists under the request identity, so
    /// nothing was published and nothing may execute.
    Missing,
    /// The request identity, payload digest, or artifact identity conflicts
    /// with the committed record; the settlement was refused unchanged and
    /// the work stays live.
    Conflict,
}

/// The guarded outcome of acquiring one Processing Artifact download lease.
#[derive(Clone, Debug, PartialEq)]
pub enum ProcessingArtifactLeaseOutcome {
    /// The lease was installed with its renewal anchor; the artifact is
    /// held against expiry cleanup while the lease is renewed and released.
    Acquired {
        lease_id: String,
        artifact: Box<ProcessingArtifact>,
    },
    /// No Processing Artifact is published under the identity.
    Unknown,
    /// The artifact's finite retention has expired; no lease is granted.
    Expired,
}
