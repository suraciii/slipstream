//! Durable domain vocabulary for composable photo processing.
//!
//! `design/processing-modules.md` separates single-module execution from
//! caller-controlled composition. This module owns the vocabulary of that
//! boundary: module, step, and artifact identities, the Original-or-Artifact
//! input binding, the concrete image contract, the complete versioned
//! parameter snapshot, the composable recipe of zero or more steps with its
//! selected current step, and the canonical identities of a bounded Preview
//! and an explicit Export.
//!
//! The vocabulary is deliberately free of execution, scheduling, and
//! discovery. It also carries the guarded save request and write outcome of
//! the composable recipe surface, and the durable lifecycle vocabulary of
//! an explicit Export's accepted work — its states, begun attempts, work
//! record, terminal outcomes, settlement, and download leases — as
//! vocabulary; the transactions that commit them live in
//! `crate::persistence`. There is no predecessor, planner,
//! ordering field, or hidden upstream pointer, and none may be added here:
//! every downstream input names its artifact explicitly.
//! Historical fixed recipe/export records remain readable for migration and
//! retained output delivery; new editing and execution use this vocabulary.

use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::collections::HashSet;
use std::fmt;

/// Longest admitted byte length of a [`ProcessingModuleId`].
pub const MAXIMUM_MODULE_ID_BYTES: usize = 64;

/// Longest admitted byte length of a [`ProcessingStepId`].
pub const MAXIMUM_STEP_ID_BYTES: usize = 64;

/// Longest admitted byte length of a [`ProcessingArtifactId`].
pub const MAXIMUM_ARTIFACT_ID_BYTES: usize = 128;

/// Longest admitted byte length of a Photo identity inside this vocabulary.
pub const MAXIMUM_PHOTO_ID_BYTES: usize = 128;

/// Longest admitted byte length of a recipe revision.
pub const MAXIMUM_REVISION_BYTES: usize = 128;

/// Longest admitted byte length of an opaque published source revision.
pub const MAXIMUM_SOURCE_REVISION_BYTES: usize = 16_384;

/// Longest admitted byte length of every module-owned contract name: image
/// format, sample precision, color space, transfer function, encoding
/// options, parameter-schema version, adapter/schema version, bundle, and
/// display-conversion identities.
pub const MAXIMUM_CONTRACT_NAME_BYTES: usize = 128;

/// Largest admitted number of Processing Steps in one recipe. A caller may
/// save zero steps, one step, or any finite set of individually admitted
/// steps; this bound keeps the set finite in storage and in protocol bodies.
pub const MAXIMUM_RECIPE_STEPS: usize = 64;

/// Largest admitted pixel edge of any explicit geometry. Every admitted
/// geometry is finite and explicit; a missing bound is never resolved from a
/// source's full size.
pub const MAXIMUM_GEOMETRY_EDGE: u32 = 65_536;

/// Largest admitted serialized byte length of one complete parameter
/// snapshot, including its JSON framing.
pub const MAXIMUM_PARAMETER_SNAPSHOT_BYTES: usize = 262_144;

/// Largest admitted nesting depth of one module-owned parameter tree.
pub const MAXIMUM_PARAMETER_SNAPSHOT_DEPTH: usize = 32;

/// The exact byte length of every canonical digest: lowercase SHA-256 hex.
pub const DIGEST_HEX_BYTES: usize = 64;

/// Why a composable-processing value is not admissible. Every refusal is
/// strict and terminal for the value under construction: no bound is silently
/// clamped, truncated, or defaulted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessingContractError {
    /// An identifier or contract name is empty.
    IdentifierEmpty,
    /// An identifier or contract name carries leading or trailing
    /// whitespace.
    IdentifierUntrimmed,
    /// An identifier or contract name contains a control character.
    IdentifierControlCharacter,
    /// An identifier or contract name exceeds its byte-length bound.
    IdentifierTooLong { maximum: usize, actual: usize },
    /// A geometry edge is zero; admitted geometry is positive.
    GeometryEdgeZero,
    /// A geometry edge exceeds the finite pixel bound.
    GeometryEdgeTooLarge { maximum: u32, actual: u32 },
    /// A recipe carries more steps than the finite bound admits.
    TooManySteps { maximum: usize, actual: usize },
    /// Two steps of one recipe share a `step_id`.
    DuplicateStepId { step_id: String },
    /// A recipe carries steps but no current step is selected.
    CurrentStepUnset,
    /// The selected current step is not one of the recipe's steps.
    CurrentStepNotInRecipe { step_id: String },
    /// A parameter snapshot exceeds its serialized byte-length bound.
    ParameterSnapshotTooLarge { maximum: usize, actual: usize },
    /// A parameter snapshot nests deeper than the admitted depth.
    ParameterSnapshotTooDeep { maximum: usize, actual: usize },
    /// A byte-evidence digest is not lowercase SHA-256 hex.
    InvalidDigest { actual: String },
    /// Input byte evidence claims zero bytes; a confined image input always
    /// has at least one byte.
    ZeroByteEvidence,
}

impl fmt::Display for ProcessingContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdentifierEmpty => formatter.write_str("identifier is empty"),
            Self::IdentifierUntrimmed => {
                formatter.write_str("identifier has leading or trailing whitespace")
            }
            Self::IdentifierControlCharacter => {
                formatter.write_str("identifier contains a control character")
            }
            Self::IdentifierTooLong { maximum, actual } => write!(
                formatter,
                "identifier is {actual} bytes, exceeding the {maximum}-byte bound"
            ),
            Self::GeometryEdgeZero => formatter.write_str("geometry edge is zero"),
            Self::GeometryEdgeTooLarge { maximum, actual } => write!(
                formatter,
                "geometry edge {actual} exceeds the {maximum}-pixel bound"
            ),
            Self::TooManySteps { maximum, actual } => write!(
                formatter,
                "recipe carries {actual} steps, exceeding the {maximum}-step bound"
            ),
            Self::DuplicateStepId { step_id } => write!(
                formatter,
                "step id `{step_id}` appears more than once in the recipe"
            ),
            Self::CurrentStepUnset => {
                formatter.write_str("recipe carries steps but no current step is selected")
            }
            Self::CurrentStepNotInRecipe { step_id } => write!(
                formatter,
                "current step `{step_id}` is not one of the recipe's steps"
            ),
            Self::ParameterSnapshotTooLarge { maximum, actual } => write!(
                formatter,
                "parameter snapshot is {actual} bytes, exceeding the {maximum}-byte bound"
            ),
            Self::ParameterSnapshotTooDeep { maximum, actual } => write!(
                formatter,
                "parameter snapshot nests {actual} levels, exceeding the {maximum}-level bound"
            ),
            Self::InvalidDigest { actual } => write!(
                formatter,
                "digest `{actual}` is not {DIGEST_HEX_BYTES} lowercase hex characters"
            ),
            Self::ZeroByteEvidence => formatter.write_str("input evidence claims zero bytes"),
        }
    }
}

impl std::error::Error for ProcessingContractError {}

/// Validates one bounded identifier or module-owned contract name: non-empty,
/// free of control characters, identical to its trimmed form, and no longer
/// than `maximum` bytes. Opaque does not mean unbounded: even caller-owned
/// step ids stay strictly bounded.
pub fn validate_bounded_name(value: &str, maximum: usize) -> Result<(), ProcessingContractError> {
    if value.is_empty() {
        return Err(ProcessingContractError::IdentifierEmpty);
    }
    if value.chars().any(char::is_control) {
        return Err(ProcessingContractError::IdentifierControlCharacter);
    }
    if value != value.trim() {
        return Err(ProcessingContractError::IdentifierUntrimmed);
    }
    let actual = value.len();
    if actual > maximum {
        return Err(ProcessingContractError::IdentifierTooLong { maximum, actual });
    }
    Ok(())
}

/// Validates an opaque recipe revision independently of source evidence.
pub fn validate_revision(value: &str) -> Result<(), ProcessingContractError> {
    if value.is_empty() {
        return Err(ProcessingContractError::IdentifierEmpty);
    }
    let actual = value.len();
    if actual > MAXIMUM_REVISION_BYTES {
        return Err(ProcessingContractError::IdentifierTooLong {
            maximum: MAXIMUM_REVISION_BYTES,
            actual,
        });
    }
    Ok(())
}

/// Validates opaque source evidence, including internal NUL separators.
pub fn validate_source_revision(value: &str) -> Result<(), ProcessingContractError> {
    if value.is_empty() {
        return Err(ProcessingContractError::IdentifierEmpty);
    }
    if value.len() > MAXIMUM_SOURCE_REVISION_BYTES {
        return Err(ProcessingContractError::IdentifierTooLong {
            maximum: MAXIMUM_SOURCE_REVISION_BYTES,
            actual: value.len(),
        });
    }
    Ok(())
}

/// Validates one byte-evidence or parameter digest: exactly
/// [`DIGEST_HEX_BYTES`] lowercase hexadecimal characters.
pub fn validate_digest(value: &str) -> Result<(), ProcessingContractError> {
    let lowercase_hex = value.len() == DIGEST_HEX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if lowercase_hex {
        Ok(())
    } else {
        Err(ProcessingContractError::InvalidDigest {
            actual: value.to_string(),
        })
    }
}

/// The canonical SHA-256 of one JSON payload. Object keys serialize in
/// sorted order, so equal values digest equally regardless of insertion
/// order; every payload carries a `kind` tag so no two vocabularies share a
/// digest.
fn digest_json(payload: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(payload)
                .expect("canonical payload serializes")
                .as_slice()
        )
    )
}

/// The identity of one processing module, for example `darktable` or the
/// standalone SpektraFilm runtime. Peer modules have no required position in
/// a pipeline, and the name alone grants no admission: the selected step must
/// still pass the service's source, bundle, qualification, and resource
/// guards.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProcessingModuleId(String);

impl ProcessingModuleId {
    /// Admits one bounded module identifier. The identifier vocabulary is
    /// adapter-owned; availability and refusal are answered per module.
    pub fn new(value: &str) -> Result<Self, ProcessingContractError> {
        validate_bounded_name(value, MAXIMUM_MODULE_ID_BYTES)?;
        Ok(Self(value.to_string()))
    }

    /// The bounded identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProcessingModuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The opaque identity of one Processing Step record, unique within the
/// Photo's current recipe. Opaque means Slipstream never interprets its
/// content or derives order from it: steps are addressed by this id, never by
/// position.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProcessingStepId(String);

impl ProcessingStepId {
    /// Admits one bounded, opaque step id. A repeated module reuses the same
    /// module under a different step id.
    pub fn new(value: &str) -> Result<Self, ProcessingContractError> {
        validate_bounded_name(value, MAXIMUM_STEP_ID_BYTES)?;
        Ok(Self(value.to_string()))
    }

    /// The bounded identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProcessingStepId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The identity of one published immutable Processing Artifact: a validated
/// Export result. Re-exporting an upstream step creates a new artifact
/// identity; selecting it downstream is a separate explicit action, and a
/// mutable "latest result" of another step is never an artifact identity.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProcessingArtifactId(String);

impl ProcessingArtifactId {
    /// Admits one bounded artifact identity.
    pub fn new(value: &str) -> Result<Self, ProcessingContractError> {
        validate_bounded_name(value, MAXIMUM_ARTIFACT_ID_BYTES)?;
        Ok(Self(value.to_string()))
    }

    /// The bounded identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProcessingArtifactId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One explicit finite pixel geometry. Every admitted geometry is positive
/// and bounded; Preview geometry and Export output geometry are each admitted
/// separately, and neither is ever defaulted from a source's full size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessingGeometry {
    /// Declared width in pixels; at least one.
    pub width: u32,
    /// Declared height in pixels; at least one.
    pub height: u32,
}

impl ProcessingGeometry {
    /// Admits one positive, bounded geometry.
    pub fn new(width: u32, height: u32) -> Result<Self, ProcessingContractError> {
        if width == 0 || height == 0 {
            return Err(ProcessingContractError::GeometryEdgeZero);
        }
        if width > MAXIMUM_GEOMETRY_EDGE {
            return Err(ProcessingContractError::GeometryEdgeTooLarge {
                maximum: MAXIMUM_GEOMETRY_EDGE,
                actual: width,
            });
        }
        if height > MAXIMUM_GEOMETRY_EDGE {
            return Err(ProcessingContractError::GeometryEdgeTooLarge {
                maximum: MAXIMUM_GEOMETRY_EDGE,
                actual: height,
            });
        }
        Ok(Self { width, height })
    }
}

/// The concrete image contract one side of the module boundary observes:
/// container format, sample precision, color space, transfer function, pixel
/// geometry, and encoding options. Each name is module-owned vocabulary; a
/// shared extension or format name alone never implies compatibility, and a
/// module that does not admit an actual contract must refuse it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingImageContract {
    /// Container or encoding format identity, for example `image/tiff`.
    pub format: String,
    /// Sample precision identity, for example `uint16`.
    pub precision: String,
    /// Color space identity, for example an ICC profile identity.
    pub color_space: String,
    /// Transfer function identity, for example `linear`.
    pub transfer: String,
    /// The concrete pixel geometry of the image.
    pub geometry: ProcessingGeometry,
    /// Encoding-options identity, for example compression or chroma
    /// subsampling choices.
    pub encoding: String,
}

impl ProcessingImageContract {
    /// Validates every contract name and the geometry. Reconstructed
    /// contracts must be revalidated before use.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        validate_bounded_name(&self.format, MAXIMUM_CONTRACT_NAME_BYTES)?;
        validate_bounded_name(&self.precision, MAXIMUM_CONTRACT_NAME_BYTES)?;
        validate_bounded_name(&self.color_space, MAXIMUM_CONTRACT_NAME_BYTES)?;
        validate_bounded_name(&self.transfer, MAXIMUM_CONTRACT_NAME_BYTES)?;
        validate_bounded_name(&self.encoding, MAXIMUM_CONTRACT_NAME_BYTES)?;
        Ok(())
    }

    /// The canonical digest of the complete contract. Two invocations agree
    /// on a contract only when every component agrees.
    pub fn canonical_digest(&self) -> String {
        let payload = serde_json::json!({
            "kind": "processing-image-contract-v1",
            "format": self.format,
            "precision": self.precision,
            "color_space": self.color_space,
            "transfer": self.transfer,
            "geometry": [self.geometry.width, self.geometry.height],
            "encoding": self.encoding,
        });
        digest_json(&payload)
    }
}

/// One complete, versioned, module-owned parameter snapshot. The tree is
/// preserved verbatim: the host never flattens it into a shared map, merges
/// schemas across modules, or discards unknown fields to obtain a runnable
/// request. Output format, precision, color space, transfer function,
/// geometry, and encoding options are ordinary parameters of this tree.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessingParameterSnapshot {
    /// The module-owned parameter-schema version. Unknown or unsupported
    /// versions fail explicitly at the module boundary.
    pub schema_version: String,
    /// The complete parameter tree.
    pub tree: Value,
}

impl ProcessingParameterSnapshot {
    /// Admits one snapshot after bounding its version and tree.
    pub fn new(schema_version: &str, tree: Value) -> Result<Self, ProcessingContractError> {
        let snapshot = Self {
            schema_version: schema_version.to_string(),
            tree,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// Validates the version name and the bounded tree. Reconstructed
    /// snapshots must be revalidated before use.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        validate_bounded_name(&self.schema_version, MAXIMUM_CONTRACT_NAME_BYTES)?;
        validate_parameter_tree(&self.tree)
    }

    /// The canonical digest of the complete snapshot, version included. It
    /// stands for the snapshot everywhere an identity carries parameters.
    pub fn canonical_digest(&self) -> String {
        let payload = serde_json::json!({
            "kind": "processing-parameters-v1",
            "schema_version": self.schema_version,
            "parameters": self.tree,
        });
        digest_json(&payload)
    }
}

/// Bounds one module-owned parameter tree: its serialized size and nesting
/// depth stay finite, so a saved snapshot can never carry unbounded
/// structure in either breadth-encoded bytes or depth.
pub fn validate_parameter_tree(tree: &Value) -> Result<(), ProcessingContractError> {
    let actual = serde_json::to_vec(tree)
        .expect("parameter tree serializes")
        .len();
    if actual > MAXIMUM_PARAMETER_SNAPSHOT_BYTES {
        return Err(ProcessingContractError::ParameterSnapshotTooLarge {
            maximum: MAXIMUM_PARAMETER_SNAPSHOT_BYTES,
            actual,
        });
    }
    let actual = json_depth(tree);
    if actual > MAXIMUM_PARAMETER_SNAPSHOT_DEPTH {
        return Err(ProcessingContractError::ParameterSnapshotTooDeep {
            maximum: MAXIMUM_PARAMETER_SNAPSHOT_DEPTH,
            actual,
        });
    }
    Ok(())
}

/// The nesting depth of one tree, measured iteratively so no hostile depth
/// can exhaust the stack while being measured.
fn json_depth(tree: &Value) -> usize {
    let mut maximum = 0;
    let mut pending = vec![(tree, 1_usize)];
    while let Some((value, depth)) = pending.pop() {
        maximum = maximum.max(depth);
        match value {
            Value::Object(map) => {
                pending.extend(map.values().map(|child| (child, depth + 1)));
            }
            Value::Array(items) => {
                pending.extend(items.iter().map(|child| (child, depth + 1)));
            }
            _ => {}
        }
    }
    maximum
}

/// The input binding of one Processing Step. The guarded Original identity of
/// a Photo and one published immutable Processing Artifact plus its concrete
/// image contract are the only two bindings, and they never merge: an
/// Original stays bound to its Photo and guarded source revision, and an
/// artifact input stays bound to its immutable published identity. A
/// mutable "latest result" of another step is not a binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessingInput {
    /// The guarded Original of one Photo, pinned to the exact observed
    /// source revision. Reads stay read-only and confined.
    Original {
        photo_id: String,
        source_revision: String,
    },
    /// One published immutable Processing Artifact and the concrete image
    /// contract its bytes carry. An unavailable or expired artifact fails
    /// rather than resolving a newer upstream result.
    Artifact {
        artifact_id: ProcessingArtifactId,
        contract: ProcessingImageContract,
    },
}

impl ProcessingInput {
    /// Validates the binding's bounded identities and contract. Reconstructed
    /// bindings must be revalidated before use.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        match self {
            Self::Original {
                photo_id,
                source_revision,
            } => {
                validate_bounded_name(photo_id, MAXIMUM_PHOTO_ID_BYTES)?;
                validate_source_revision(source_revision)?;
            }
            Self::Artifact {
                artifact_id: _,
                contract,
            } => contract.validate()?,
        }
        Ok(())
    }

    /// The canonical digest of the input binding identity alone, domain
    /// separated per variant so an Original and an artifact can never share a
    /// binding digest.
    pub fn binding_digest(&self) -> String {
        let payload = match self {
            Self::Original {
                photo_id,
                source_revision,
            } => serde_json::json!({
                "kind": "processing-input-original-v1",
                "photo_id": photo_id,
                "source_revision": source_revision,
            }),
            Self::Artifact {
                artifact_id,
                contract,
            } => serde_json::json!({
                "kind": "processing-input-artifact-v1",
                "artifact_id": artifact_id.as_str(),
                "contract": contract.canonical_digest(),
            }),
        };
        digest_json(&payload)
    }
}

/// One Processing Step record: an opaque `step_id` unique within the Photo's
/// current recipe, one selected module, one input binding, and one complete
/// parameter snapshot. There is no predecessor, ordering field, or hidden
/// upstream pointer; an artifact input binding is the only composition edge.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessingStep {
    pub step_id: ProcessingStepId,
    pub module: ProcessingModuleId,
    pub input: ProcessingInput,
    pub parameters: ProcessingParameterSnapshot,
}

impl ProcessingStep {
    /// Validates the record's binding and parameter snapshot. The identities
    /// are validated at construction; reconstructed records must still be
    /// revalidated before use.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        self.input.validate()?;
        self.parameters.validate()
    }
}

/// The composable Edit Recipe of one Photo: the recipe revision, the guarded
/// source revision it is bound to, zero or more individually admitted
/// Processing Steps, and the one record the caller selected as the current
/// step. The step order in storage is incidental; steps are addressed by
/// `step_id`, never by position, and no graph, plan, or ordering semantics
/// exist here.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposableEditRecipe {
    pub photo_id: String,
    pub revision: String,
    pub source_revision: String,
    pub steps: Vec<ProcessingStep>,
    /// The caller's selected current step; `None` only for the zero-step
    /// recipe.
    pub current_step_id: Option<ProcessingStepId>,
}

impl ComposableEditRecipe {
    /// Validates the whole recipe: bounded identities, a finite step set,
    /// unique step ids, every step individually valid, and a current step
    /// that is one of the recipe's steps whenever steps exist. Reconstructed
    /// recipes must be revalidated before use.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        validate_bounded_name(&self.photo_id, MAXIMUM_PHOTO_ID_BYTES)?;
        validate_bounded_name(&self.revision, MAXIMUM_REVISION_BYTES)?;
        validate_source_revision(&self.source_revision)?;
        let actual = self.steps.len();
        if actual > MAXIMUM_RECIPE_STEPS {
            return Err(ProcessingContractError::TooManySteps {
                maximum: MAXIMUM_RECIPE_STEPS,
                actual,
            });
        }
        let mut seen = HashSet::with_capacity(actual);
        for step in &self.steps {
            if !seen.insert(step.step_id.clone()) {
                return Err(ProcessingContractError::DuplicateStepId {
                    step_id: step.step_id.as_str().to_string(),
                });
            }
            step.validate()?;
        }
        match &self.current_step_id {
            Some(current) => {
                if !seen.contains(current) {
                    return Err(ProcessingContractError::CurrentStepNotInRecipe {
                        step_id: current.as_str().to_string(),
                    });
                }
            }
            None if !self.steps.is_empty() => {
                return Err(ProcessingContractError::CurrentStepUnset);
            }
            None => {}
        }
        Ok(())
    }

    /// The selected current step record, or `None` for the zero-step recipe.
    pub fn current_step(&self) -> Option<&ProcessingStep> {
        let current = self.current_step_id.as_ref()?;
        self.steps.iter().find(|step| &step.step_id == current)
    }
}

/// Why one [`SaveComposableEditRecipe`] request is not admissible. Every
/// refusal is strict and terminal for the request: nothing is written, and
/// no bound is silently clamped, truncated, or defaulted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposableRecipeRequestError {
    /// The request identity, the Photo identity, an expected revision, or
    /// the recipe itself is not an admissible composable-processing value.
    Contract(ProcessingContractError),
    /// The request's Photo identity and its recipe's Photo identity differ.
    PhotoMismatch,
    /// The guarded source revision and the recipe's bound source revision
    /// differ: a save must be bound to exactly the revision it guards
    /// against.
    SourceRevisionMismatch,
    /// The caller identity is outside the closed request alphabet.
    InvalidRequestId,
}

impl fmt::Display for ComposableRecipeRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => write!(formatter, "request is not admissible: {error}"),
            Self::InvalidRequestId => formatter.write_str("request identity is invalid"),
            Self::PhotoMismatch => {
                formatter.write_str("request Photo identity and recipe Photo identity differ")
            }
            Self::SourceRevisionMismatch => {
                formatter.write_str("guarded source revision and recipe source revision differ")
            }
        }
    }
}

impl std::error::Error for ComposableRecipeRequestError {}

/// One guarded request to save a Photo's complete composable Edit Recipe.
///
/// The request carries the complete intended recipe, never a delta, and the
/// commit assigns a fresh recipe revision, so the submitted
/// [`ComposableEditRecipe::revision`] is the caller's local snapshot label:
/// it is admitted as a bounded identity and then replaced. The request is
/// admissible only when its Photo identity equals the recipe's Photo
/// identity and the recipe is bound to exactly the source revision the save
/// guards against.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveComposableEditRecipe {
    /// The Photo whose recipe is saved.
    pub photo_id: String,
    /// Stable caller-owned identity used to resolve a retry after a lost
    /// response.
    pub request_id: String,
    pub expected_recipe_revision: Option<String>,
    pub expected_source_revision: String,
    pub recipe: ComposableEditRecipe,
    /// Optional engine-owned adjustment, identified by the original request.
    pub automatic_adjustment: Option<AutomaticAdjustment>,
}

/// An engine-owned automatic adjustment requested as part of one atomic save.
#[derive(Clone, Debug, PartialEq)]
pub struct AutomaticAdjustment {
    pub step_id: ProcessingStepId,
    pub operation: String,
    pub multi_priority: i64,
    pub instruction: Value,
    /// Original complete recipe used for retry identity; never persisted.
    pub original_recipe: Option<Box<ComposableEditRecipe>>,
}

impl AutomaticAdjustment {
    pub fn new(
        step_id: ProcessingStepId,
        operation: String,
        multi_priority: i64,
        instruction: Value,
    ) -> Self {
        Self {
            step_id,
            operation,
            multi_priority,
            instruction,
            original_recipe: None,
        }
    }
}

/// Rebinds only Original inputs under the observed recipe and source guards.
#[derive(Clone, Debug, PartialEq)]
pub struct RebindComposableEditRecipe {
    pub photo_id: String,
    pub request_id: String,
    pub expected_recipe_revision: String,
    pub new_source_revision: String,
}

/// Saved intent and independently observed Original availability.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposableEditRecipeRead {
    pub recipe: Option<ComposableEditRecipe>,
    pub current_source_revision: Option<String>,
    pub source_available: bool,
}

/// The guarded outcome of one composable recipe save.
#[derive(Clone, Debug, PartialEq)]
pub enum ComposableEditRecipeWriteOutcome {
    /// A fresh commit installed a new recipe revision inside the write
    /// transaction.
    Saved(ComposableEditRecipe),
    /// A receipt replay of a committed write: nothing was written, and the
    /// carried recipe is the committed receipt, whatever advanced since.
    Replayed(ComposableEditRecipe),
    /// The submitted recipe carries the same steps, current step, and source
    /// binding as the committed recipe, so no new revision was installed.
    Unchanged(ComposableEditRecipe),
    /// The caller's expected recipe revision is stale. Carries the currently
    /// stored recipe, if any, from the same serialized read.
    Conflict(Option<ComposableEditRecipe>),
    /// The Photo's source revision moved past the caller's guard. Carries
    /// the currently stored recipe, if any, from the same serialized read.
    SourceChanged(Option<ComposableEditRecipe>),
    /// The Photo does not exist.
    MissingPhoto,
    /// The Photo's source is unavailable, or no published Capture fact is
    /// bound to the observed source facts, so no guarded save is possible.
    Unavailable,
    /// The request is not admissible; nothing was read or written for it.
    Invalid(ComposableRecipeRequestError),
    /// The request identity was already used with a different payload.
    RequestConflict,
    /// A settled identity has passed its seven-day reconciliation period.
    ReceiptExpired,
}

/// The verified byte evidence of one invocation's input: the input binding
/// plus the content digest and byte length of the confined bytes actually
/// read. Identity compares evidence, never a mutable pointer, so a
/// republished upstream artifact can never silently satisfy an older
/// binding's identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingInputEvidence {
    pub input: ProcessingInput,
    /// SHA-256 of the confined input bytes, as lowercase hex.
    pub sha256: String,
    pub byte_length: u64,
}

impl ProcessingInputEvidence {
    /// Admits one evidence record after validating its binding and digest.
    pub fn new(
        input: ProcessingInput,
        sha256: &str,
        byte_length: u64,
    ) -> Result<Self, ProcessingContractError> {
        let evidence = Self {
            input,
            sha256: sha256.to_string(),
            byte_length,
        };
        evidence.validate()?;
        Ok(evidence)
    }

    /// Validates the binding, digest shape, and positive byte length.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        self.input.validate()?;
        validate_digest(&self.sha256)?;
        if self.byte_length == 0 {
            return Err(ProcessingContractError::ZeroByteEvidence);
        }
        Ok(())
    }

    /// The canonical digest of the binding identity plus its byte evidence.
    pub fn digest(&self) -> String {
        let payload = serde_json::json!({
            "kind": "processing-input-evidence-v1",
            "input": self.input.binding_digest(),
            "sha256": self.sha256,
            "byte_length": self.byte_length,
        });
        digest_json(&payload)
    }
}

/// The complete identity of one bounded current-step Preview: the exact input
/// binding and its verified byte evidence, the selected module with its
/// pinned adapter/schema version, the canonical digest of the captured
/// complete parameter snapshot, the intended output contract captured with
/// that snapshot, the processing bundle, the disclosed Preview geometry, the
/// separately identified display conversion, and the digest of the exact
/// derived invocation parameters when the adapter expresses Preview geometry
/// through its parameter tree.
///
/// The Preview geometry and display conversion are disclosed rendition
/// choices; they are never exported as the intended full-size output, and no
/// field here licenses a full-resolution handoff. Equal identities may
/// coalesce; any changed component is a new identity, and late, superseded,
/// or wrong-module results never satisfy it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingPreviewIdentity {
    pub input: ProcessingInputEvidence,
    pub module: ProcessingModuleId,
    /// The pinned adapter and parameter-schema identity used by discovery
    /// and execution; a changed bundle requires a fresh check.
    pub adapter_schema_version: String,
    /// The canonical digest of the complete captured parameter snapshot.
    pub parameter_digest: String,
    /// The intended Export output contract captured inside the step
    /// parameters.
    pub output_contract: ProcessingImageContract,
    /// The exact processing bundle identity.
    pub bundle_id: String,
    /// The explicit finite Preview geometry.
    pub geometry: ProcessingGeometry,
    /// The display-conversion identity applied for delivery, if any; `None`
    /// means scene-referred delivery.
    pub display_conversion: Option<String>,
    /// The canonical digest of the exact bounded invocation parameters
    /// derived for this Preview, when geometry was derived into the
    /// invocation; `None` when the captured snapshot is invoked verbatim.
    pub invocation_digest: Option<String>,
}

impl ProcessingPreviewIdentity {
    /// Validates every component of the identity.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        self.input.validate()?;
        validate_bounded_name(&self.adapter_schema_version, MAXIMUM_CONTRACT_NAME_BYTES)?;
        validate_digest(&self.parameter_digest)?;
        self.output_contract.validate()?;
        validate_bounded_name(&self.bundle_id, MAXIMUM_CONTRACT_NAME_BYTES)?;
        if let Some(display) = &self.display_conversion {
            validate_bounded_name(display, MAXIMUM_CONTRACT_NAME_BYTES)?;
        }
        if let Some(invocation) = &self.invocation_digest {
            validate_digest(invocation)?;
        }
        Ok(())
    }

    /// The canonical digest of the complete Preview identity.
    pub fn digest(&self) -> String {
        let payload = serde_json::json!({
            "kind": "processing-preview-identity-v1",
            "input": self.input.digest(),
            "module": self.module.as_str(),
            "adapter_schema_version": self.adapter_schema_version,
            "parameter_digest": self.parameter_digest,
            "output_contract": self.output_contract.canonical_digest(),
            "bundle_id": self.bundle_id,
            "geometry": [self.geometry.width, self.geometry.height],
            "display_conversion": self.display_conversion,
            "invocation_digest": self.invocation_digest,
        });
        digest_json(&payload)
    }
}

/// The confirmed acceptance identity of one explicit Export: the confirmed
/// input binding with its verified byte evidence, the selected module with
/// its pinned adapter/schema version, the canonical digest of the complete
/// captured step parameters, the intended output contract, and the processing
/// bundle. It is captured at acceptance, never inherits whichever settings
/// happen to be current when queued work starts, and deliberately carries no
/// Preview geometry or display derivation: the Export output geometry is the
/// one inside the intended output contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingExportIdentity {
    pub input: ProcessingInputEvidence,
    pub module: ProcessingModuleId,
    /// The pinned adapter and parameter-schema identity used at acceptance.
    pub adapter_schema_version: String,
    /// The canonical digest of the complete captured parameter snapshot.
    pub parameter_digest: String,
    /// The intended output contract, including the separately admitted output
    /// geometry.
    pub output_contract: ProcessingImageContract,
    /// The exact processing bundle identity.
    pub bundle_id: String,
}

impl ProcessingExportIdentity {
    /// Validates every component of the identity.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        self.input.validate()?;
        validate_bounded_name(&self.adapter_schema_version, MAXIMUM_CONTRACT_NAME_BYTES)?;
        validate_digest(&self.parameter_digest)?;
        self.output_contract.validate()?;
        validate_bounded_name(&self.bundle_id, MAXIMUM_CONTRACT_NAME_BYTES)?;
        Ok(())
    }

    /// The canonical digest of the complete Export acceptance identity.
    pub fn digest(&self) -> String {
        let payload = serde_json::json!({
            "kind": "processing-export-identity-v1",
            "input": self.input.digest(),
            "module": self.module.as_str(),
            "adapter_schema_version": self.adapter_schema_version,
            "parameter_digest": self.parameter_digest,
            "output_contract": self.output_contract.canonical_digest(),
            "bundle_id": self.bundle_id,
        });
        digest_json(&payload)
    }
}

mod export;
pub use export::*;
#[cfg(test)]
#[path = "processing/tests.rs"]
mod tests;
