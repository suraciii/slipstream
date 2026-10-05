//! Composable processing-module boundary contracts (Issue #496).
//!
//! This module owns only the single-module boundary: module identity with
//! pinned adapter versions, concrete input and output image contracts,
//! finite limits, per-module availability with refusal reasons, module-owned
//! versioned parameter trees with deterministic shape validation, confined
//! input identities, private result identities with actual output facts, and
//! structured refusals. darktable and standalone SpektraFilm are peer modules
//! with independent availability. There is no workflow planner, no implicit
//! conversion registry, and no shared output-argument schema here: output
//! format, precision, color space, transfer function, geometry, and encoding
//! are ordinary fields of each module's own parameter tree.
//!
//! This crate has no confined engine path, so it never claims execution by
//! itself: [`ModuleRegistry::run`] validates and freezes one invocation and
//! then hands it to an explicit [`ModuleRunner`] adapter seam, and a
//! [`ModuleResult`] exists only when that runner produced the actual output
//! facts of a completed private output. Bounded preview admission and
//! artifact publication stay with their existing owners outside this crate.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use sha2::{Digest, Sha256};
mod engine;
pub use engine::*;
mod parameters;
use parameters::{validate_parameter_tree, validate_saved_tree, validate_spektrafilm_tree};
#[cfg(test)]
#[path = "modules/tests.rs"]
mod tests;

const MODULE_PARAMETER_BYTES: usize = 8 * 1024;
const MODULE_NAME_BYTES: usize = 64;
const FILM_PARAMETER_BYTES: usize = 16 * 1024;

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn identifier(value: &str, maximum: usize) -> bool {
    (1..=maximum).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

/// Identity of this boundary contract itself. Discovery and execution pin
/// the same contract, module, and adapter version.
pub const MODULE_CONTRACT_VERSION: &str = "slipstream-module-contract-1";

/// The first peer module: darktable with its ordered image-operation stack.
pub const DARKTABLE_MODULE: &str = "darktable";
/// The pinned darktable adapter identity of this contract revision.
pub const DARKTABLE_ADAPTER_VERSION: &str = "darktable-adapter-1";
/// The darktable-owned parameter schema version this boundary admits.
pub const DARKTABLE_PARAMETER_VERSION: &str = "darktable-params-1";

/// The second peer module: standalone SpektraFilm, not a darktable
/// image-operation module.
pub const SPEKTRAFILM_MODULE: &str = "spektrafilm";
/// The standalone SpektraFilm implementation identity for this boundary.
pub const SPEKTRAFILM_IMPLEMENTATION: &str = "spektrafilm-rs";
/// The checked-in fork revision accepted by bundle verification.
pub const SPEKTRAFILM_FORK_COMMIT: &str = "bb3d5cc8163823bee950b16fde8d2a58d63e196f";
/// The pinned standalone SpektraFilm adapter identity of this contract
/// revision.
pub const SPEKTRAFILM_ADAPTER_VERSION: &str = "spektrafilm-rs-adapter-1";
/// The SpektraFilm-owned parameter schema version this boundary admits.
pub const SPEKTRAFILM_PARAMETER_VERSION: &str = "spektrafilm-rs-params-1";
/// The previous saved-tree version remains readable for historical Recipes
/// and Artifacts, but is not emitted by discovery or new executions.
pub const SPEKTRAFILM_LEGACY_PARAMETER_VERSION: &str = "spektrafilm-params-1";
/// The pinned fixed-recipe identity of the standalone SpektraFilm adapter,
/// the shared `FILM_RECIPE_SHA256` of `tools/development/film_identity.py`.
/// The pinned runtime re-verifies the forwarded tree against this recipe;
/// exact bundle verification separately pins source, packages, and runtime bytes.
pub const SPEKTRAFILM_RECIPE_SHA256: &str =
    "8efdd28d3a49fea7e71835dea82dbc416d9f95e4ae4216ae5fb7535087ec5cf8";
/// The pinned handoff profile of the standalone SpektraFilm input — the
/// shared `INPUT_ICC_SHA256` of `film_identity.py`, which is the byte
/// identity of the darktable peer's Development TIFF output profile.
pub const SPEKTRAFILM_INPUT_ICC_SHA256: &str =
    "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe";
/// The pinned sRGB profile bytes of the standalone SpektraFilm Finished
/// JPEG — the shared `OUTPUT_ICC_SHA256` of `film_identity.py`.
pub const SPEKTRAFILM_OUTPUT_ICC_SHA256: &str =
    "b44e86e44d44993a3a9a880626f9832e9c37e2234caba501548f9114bada6d21";
/// The complete default parameter tree of the pinned fixed recipe: the
/// exact runtime groups and Finished JPEG output the pinned runtime's own
/// `--emit-default-parameters` writes into its bundle
/// (`parameters-default.json`, digest-pinned by the bundle manifest). The
/// fixed runtime executes only this recipe, so the host pins the tree
/// beside the recipe identity above: startup verification refuses a
/// bundle whose emitted defaults differ, and admission compares every
/// forwarded group against these values.
pub const SPEKTRAFILM_RECIPE_TREE: &str = include_str!("spektrafilm-recipe-tree.json");

/// The complete default parameter tree of the pinned fixed recipe, parsed
/// once from [`SPEKTRAFILM_RECIPE_TREE`]. The standalone runtime executes
/// only this tree: discovery publishes it as the module's own defaults,
/// and admission compares every forwarded group against it.
pub fn spektrafilm_default_tree() -> Value {
    static PINNED: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
        serde_json::from_str(SPEKTRAFILM_RECIPE_TREE).expect("the pinned recipe tree parses")
    });
    PINNED.clone()
}

/// The ordered-operation bound of one darktable stack. The serialized
/// envelope bound also limits the bytes; this keeps the forwarded array
/// explicitly finite per operation.
const DARKTABLE_STACK_OPERATIONS_MAX: usize = 128;

/// The pinned development handoff of the darktable adapter: one linear
/// 32-bit ProPhoto TIFF that preserves the source geometry.
const DARKTABLE_OUTPUT_FORMAT: &str = "tiff";
const DARKTABLE_OUTPUT_PRECISION_BITS: u64 = 32;
const DARKTABLE_OUTPUT_COLOR_SPACE: &str = "prophoto-rgb";
const DARKTABLE_OUTPUT_TRANSFER: &str = "linear";
const DARKTABLE_OUTPUT_GEOMETRY: &str = "source-preserving";

/// The bounded frame the standalone Film workspace plans are computed for,
/// mirroring the pinned film adapter (`MAX_EDGE`, `MAX_DECODED_BYTES`, and
/// `MAX_COORDINATE_SUM` of `tools/processing/photo/film_adapter.py`).
const FILM_MAX_EDGE: u64 = 9568;
const FILM_MAX_DECODED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const FILM_SAMPLE_BYTES: u64 = 12;
const FILM_MAX_COORDINATE_SUM: u64 = 175_000;

/// The enforced execution deadline of both standalone SpektraFilm
/// execution paths. The server's photo executor bounds every Film Preview
/// and Export engine attempt at fifteen minutes — long enough for the
/// qualified full-size workload — and discovery advertises this same
/// deadline so a caller waits on the bound actually enforced.
/// Caller-driven cancellation still applies.
pub const SPEKTRAFILM_DEADLINE_MILLIS: u64 = 900_000;

/// The largest frame the film workspace geometry bounds admit: every edge
/// at the workspace edge bound, whose decoded bytes stay inside the
/// decoded-input bound. The declared output pixel bound covers exactly
/// that qualified handoff class — including the real portrait development
/// handoff the darktable peer exports — without admitting any frame the
/// pinned runtime refuses.
const SPEKTRAFILM_MAX_OUTPUT_PIXELS: u64 = FILM_MAX_EDGE * FILM_MAX_EDGE;

/// The pinned finished-JPEG encoding of the standalone SpektraFilm adapter:
/// a baseline JPEG at the fixed quality 85 of the shared film identity
/// (`FINISHED_JPEG_QUALITY`), never an arbitrary engine control.
const SPEKTRAFILM_OUTPUT_FORMAT: &str = "jpeg";
const SPEKTRAFILM_OUTPUT_PRECISION_BITS: u64 = 8;
const SPEKTRAFILM_OUTPUT_COLOR_SPACE: &str = "srgb";
const SPEKTRAFILM_OUTPUT_TRANSFER: &str = "srgb";
const SPEKTRAFILM_OUTPUT_GEOMETRY: &str = "input-preserving";
const SPEKTRAFILM_OUTPUT_ENCODING: &str = "quality-85-baseline";

/// The runtime-owned parameter groups of standalone SpektraFilm, exactly the
/// manifest groups of its fixed recipe (`tools/development/film.py`): the
/// envelope's camelCase names of `camera`, `enlarger`, `scanner`, `io`,
/// `settings`, `debug`, `film_render`, `print_render`, and `taps`. The group
/// values stay verbatim module-owned objects; the runtime re-verifies them
/// against its pinned recipe identity.
const SPEKTRAFILM_GROUPS: [&str; 9] = [
    "camera",
    "enlarger",
    "scanner",
    "io",
    "settings",
    "debug",
    "filmRender",
    "printRender",
    "taps",
];

/// The output byte bound shared with local execution and Export artifact limits.
const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Identity of one processing module and its pinned adapter version.
/// Discovery and execution use the same pinned identity; a changed bundle
/// requires a fresh availability and admission check.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleId {
    pub name: String,
    pub adapter_version: String,
}

/// One concrete image contract: container format, color space, transfer
/// function, sample precision, and geometry. Compatibility is decided per
/// invocation against the actual contract; a shared format or extension name
/// alone never admits an input.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageContract {
    pub format: String,
    pub color_space: String,
    pub transfer_function: String,
    pub precision_bits: u8,
    pub width: u64,
    pub height: u64,
}

/// Finite per-invocation bounds of one module. Every value is explicit and
/// positive; nothing is unbounded.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleLimits {
    pub max_input_bytes: u64,
    pub max_parameter_bytes: usize,
    pub max_output_pixels: u64,
    pub deadline_millis: u64,
}

/// Current availability state of one module.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AvailabilityState {
    Ready,
    Unavailable,
}

/// Availability of one module with its refusal reasons. Availability belongs
/// to exactly one module: a ready darktable never implies a ready
/// SpektraFilm, and input support or qualified parameter combinations are
/// separate questions from engine availability.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleAvailability {
    pub state: AvailabilityState,
    pub refusal_reasons: Vec<String>,
}

impl ModuleAvailability {
    pub fn ready() -> Self {
        Self {
            state: AvailabilityState::Ready,
            refusal_reasons: Vec::new(),
        }
    }

    pub fn unavailable(reason: &str) -> Self {
        Self {
            state: AvailabilityState::Unavailable,
            refusal_reasons: vec![reason.into()],
        }
    }
}

/// One complete, bounded, versioned parameter tree. The envelope fields are
/// strict; `tree` is preserved verbatim for the owning module, whose
/// deterministic shape validation ([`ModuleRegistry::admit`]) admits or
/// refuses the whole tree before any engine work, so module-owned shapes are
/// never flattened into a shared map, merged across modules, or stripped of
/// unknown fields to obtain a runnable request.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Parameters {
    pub module: String,
    pub version: String,
    pub tree: Value,
}

/// The confined read-only input of one invocation: an opaque guarded
/// identity (an Original or an immutable Processing Artifact), the verified
/// byte identity and size of the staged bytes, and their concrete image
/// contract. It is never a client pathname or a mutable "latest result" of an
/// upstream step.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfinedInput {
    pub source_id: String,
    pub digest: String,
    pub byte_size: u64,
    pub contract: ImageContract,
}

/// The actual facts of one completed private output. They are produced only
/// by an executing [`ModuleRunner`] and are re-validated against the owning
/// module's pinned output contract before a result is formed; this boundary
/// never invents or defaults them.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleOutput {
    /// The verified sha256 of the output bytes, as 64 lowercase hex digits.
    pub digest: String,
    /// The positive byte length of the completed output.
    pub byte_size: u64,
    /// The concrete image contract of the completed output.
    pub contract: ImageContract,
}

/// One admitted, frozen module invocation handed to the executing adapter.
/// `result_id` is derived from the exact module and adapter version, the
/// frozen parameter snapshot, and the confined input binding, so equal
/// invocations yield equal identities. An invocation is admission evidence
/// only: it carries no output facts and by itself claims no completed work.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleInvocation {
    pub result_id: String,
    pub module: ModuleId,
    pub parameter_version: String,
    pub parameter_digest: String,
    pub parameters: Parameters,
    pub input: ConfinedInput,
    pub limits: ModuleLimits,
}

/// The explicit execution seam of the module boundary. This crate has no
/// confined engine path, so a module never runs unless the caller binds a
/// runner: local darktable execution and the standalone Film attempt
/// implement this trait and
/// confine their engine work within the invocation's `limits`, including
/// `deadline_millis`. A runner reports either the actual facts of a
/// completed private output or a structured failure; there is no identity
/// validation that silently counts as execution.
pub trait ModuleRunner {
    fn execute(&mut self, invocation: &ModuleInvocation) -> Result<ModuleOutput, ModuleError>;
}

/// Private descriptor of one completed module invocation. `result_id` is
/// derived from the exact module and adapter version, the frozen parameter
/// snapshot, and the confined input binding, so equal invocations yield
/// equal identities. `output` carries the actual facts of the private
/// output the bound runner completed and this boundary re-validated; it
/// names a private output of a single invocation, not a published artifact,
/// and publication stays outside this crate.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleResult {
    pub result_id: String,
    pub module: ModuleId,
    pub parameter_version: String,
    pub parameter_digest: String,
    pub input: ConfinedInput,
    pub output: ModuleOutput,
}

/// Structured refusal code of one module invocation. Every refusal is
/// explicit; unknown or unsupported modules, versions, combinations,
/// controls, and input contracts never fall back to another module or an
/// implicit conversion, and nothing runs after a refusal.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleErrorCode {
    UnknownModule,
    ModuleUnavailable,
    WrongModuleParameters,
    UnsupportedParameterVersion,
    ParameterTreeTooLarge,
    InputTooLarge,
    OutputTooLarge,
    IncompatibleInput,
    MalformedParameterTree,
    UnsupportedControl,
    ExecutionFailed,
}

/// One structured module refusal or failure.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleError {
    pub code: ModuleErrorCode,
    pub message: String,
}

/// One module's bounded discovery description: identity, schemas, contracts,
/// limits, availability, and qualified Engine Modules.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleDescription {
    pub id: ModuleId,
    pub parameter_versions: Vec<String>,
    /// The module-owned parameter schema document. Output options — format,
    /// precision, color space, transfer function, geometry, encoding — are
    /// ordinary fields of this tree, not shared output arguments.
    pub parameter_schema: Value,
    pub admitted_inputs: Vec<ImageContract>,
    pub admitted_outputs: Vec<ImageContract>,
    pub limits: ModuleLimits,
    pub availability: ModuleAvailability,
    #[serde(default)]
    pub engine_modules: Vec<EngineModuleDescription>,
}

/// The peer-module registry. Each module is described, admitted, and refused
/// on its own record; one module's availability never derives from or is
/// consulted for the other's.
#[derive(Clone, Debug)]
pub struct ModuleRegistry {
    modules: Vec<ModuleDescription>,
}

impl ModuleRegistry {
    /// Build the registry with each peer's current availability supplied
    /// independently.
    pub fn new(darktable: ModuleAvailability, spektrafilm: ModuleAvailability) -> Self {
        Self {
            modules: vec![
                darktable_description(darktable),
                spektrafilm_description(spektrafilm),
            ],
        }
    }

    /// Read-only discovery of one selected module's bounded description.
    pub fn describe(&self, module: &str) -> Result<&ModuleDescription, ModuleError> {
        self.modules
            .iter()
            .find(|description| description.id.name == module)
            .ok_or_else(|| {
                refusal(
                    ModuleErrorCode::UnknownModule,
                    format!("unknown module `{module}`"),
                )
            })
    }

    /// Every peer's description, each carrying only its own availability.
    pub fn descriptions(&self) -> &[ModuleDescription] {
        &self.modules
    }
    /// Validate a retained editing tree independently of engine availability
    /// and execution qualification. Valid unsupported intent stays verbatim.
    pub fn validate_saved_parameters(&self, parameters: &Parameters) -> Result<(), ModuleError> {
        let description = self.describe(&parameters.module)?;
        let request = serde_json::to_vec(parameters).expect("serializable parameters");
        if request.len() > description.limits.max_parameter_bytes {
            return Err(refusal(
                ModuleErrorCode::ParameterTreeTooLarge,
                format!(
                    "parameter request exceeds the {}-byte bound of module `{}`",
                    description.limits.max_parameter_bytes, parameters.module
                ),
            ));
        }
        if !description.parameter_versions.contains(&parameters.version) {
            return Err(refusal(
                ModuleErrorCode::UnsupportedParameterVersion,
                format!(
                    "module `{}` does not admit parameter version `{}`",
                    parameters.module, parameters.version
                ),
            ));
        }
        validate_saved_tree(&parameters.module, &parameters.tree)
    }

    /// Validate one complete module-owned execution parameter envelope without
    /// consulting availability or input admission. Unsupported output contracts
    /// and retained controls without a qualified mapping fail before engine work.
    pub fn validate_parameters(&self, parameters: &Parameters) -> Result<(), ModuleError> {
        let description = self.describe(&parameters.module)?;
        let request = serde_json::to_vec(parameters).expect("serializable parameters");
        if request.len() > description.limits.max_parameter_bytes {
            return Err(refusal(
                ModuleErrorCode::ParameterTreeTooLarge,
                format!(
                    "parameter request is {} bytes, above the {}-byte bound of module `{}`",
                    request.len(),
                    description.limits.max_parameter_bytes,
                    parameters.module
                ),
            ));
        }
        if !description.parameter_versions.contains(&parameters.version) {
            return Err(refusal(
                ModuleErrorCode::UnsupportedParameterVersion,
                format!(
                    "module `{}` does not admit parameter version `{}`",
                    parameters.module, parameters.version
                ),
            ));
        }
        validate_parameter_tree(&parameters.module, &parameters.tree)
    }

    /// Validate and freeze one single-module invocation without executing
    /// anything. The module name, the parameter tree's ownership, its
    /// version, the bounded parameter request size, the bounded input size,
    /// the concrete input contract, the declared output pixel bound, and the
    /// module-owned parameter tree shape are all checked before any engine
    /// work; every refusal is structured. The returned invocation is
    /// admission evidence only — a result with actual output facts exists
    /// only through [`ModuleRegistry::run`].
    pub fn admit(
        &self,
        module: &str,
        input: &ConfinedInput,
        parameters: &Parameters,
    ) -> Result<ModuleInvocation, ModuleError> {
        let description = self.describe(module)?;
        if description.availability.state != AvailabilityState::Ready {
            let reasons = description.availability.refusal_reasons.join("; ");
            return Err(refusal(
                ModuleErrorCode::ModuleUnavailable,
                format!("module `{module}` is unavailable: {reasons}"),
            ));
        }
        if parameters.module != module {
            return Err(refusal(
                ModuleErrorCode::WrongModuleParameters,
                format!(
                    "parameter tree is owned by module `{}`, not `{module}`",
                    parameters.module
                ),
            ));
        }
        self.validate_parameters(parameters)?;
        if input.byte_size > description.limits.max_input_bytes {
            return Err(refusal(
                ModuleErrorCode::InputTooLarge,
                format!(
                    "input is {} bytes, above the {}-byte bound of module `{module}`",
                    input.byte_size, description.limits.max_input_bytes
                ),
            ));
        }
        validate_input_contract(module, input, description)?;
        let pixels = input.contract.width.saturating_mul(input.contract.height);
        if pixels > description.limits.max_output_pixels {
            return Err(refusal(
                ModuleErrorCode::OutputTooLarge,
                format!(
                    "input frame is {pixels} pixels, above the {}-pixel output bound of module `{module}`",
                    description.limits.max_output_pixels
                ),
            ));
        }
        let identity = (MODULE_CONTRACT_VERSION, &description.id, parameters, input);
        Ok(ModuleInvocation {
            result_id: digest(&serde_json::to_vec(&identity).expect("serializable invocation")),
            module: description.id.clone(),
            parameter_version: parameters.version.clone(),
            parameter_digest: digest(
                &serde_json::to_vec(&parameters.tree).expect("serializable parameter tree"),
            ),
            parameters: parameters.clone(),
            input: input.clone(),
            limits: description.limits,
        })
    }

    /// Validate, freeze, and execute one single-module invocation through the
    /// bound [`ModuleRunner`] seam. Everything checkable is refused before
    /// the runner starts; a successful result exists only when the runner
    /// completed a private output and its reported facts — digest, byte
    /// size, and output contract — re-validate against the owning module's
    /// pinned output. A runner failure is a structured
    /// [`ModuleErrorCode::ExecutionFailed`], never a fallback result, and no
    /// identity-only validation is reported as execution success.
    pub fn run(
        &self,
        module: &str,
        input: &ConfinedInput,
        parameters: &Parameters,
        runner: &mut dyn ModuleRunner,
    ) -> Result<ModuleResult, ModuleError> {
        let invocation = self.admit(module, input, parameters)?;
        let output = runner.execute(&invocation).map_err(|error| {
            refusal(
                ModuleErrorCode::ExecutionFailed,
                format!(
                    "module `{module}` did not complete its private output: {}",
                    error.message
                ),
            )
        })?;
        validate_output_facts(module, input, &output, self.describe(module)?)?;
        Ok(ModuleResult {
            result_id: invocation.result_id,
            module: invocation.module,
            parameter_version: invocation.parameter_version,
            parameter_digest: invocation.parameter_digest,
            input: invocation.input,
            output,
        })
    }

    /// Validate one concrete input contract against the module's own
    /// admission rule — the same class and geometry check
    /// [`ModuleRegistry::admit`] enforces at execution — without byte
    /// evidence. Recipe persistence uses this when binding a step to a
    /// published artifact's contract; the staged bytes' own identity
    /// guards stay with execution.
    pub fn admit_input_contract(
        &self,
        module: &str,
        contract: &ImageContract,
    ) -> Result<(), ModuleError> {
        let description = self.describe(module)?;
        if input_contract_admitted(module, contract, description) {
            Ok(())
        } else {
            Err(refusal(
                ModuleErrorCode::IncompatibleInput,
                format!("module `{module}` does not admit the input contract {contract:?}"),
            ))
        }
    }
}

fn refusal(code: ModuleErrorCode, message: String) -> ModuleError {
    ModuleError { code, message }
}

fn malformed(module: &str, detail: String) -> ModuleError {
    refusal(
        ModuleErrorCode::MalformedParameterTree,
        format!("module `{module}` parameter tree is malformed: {detail}"),
    )
}

fn unsupported(module: &str, detail: String) -> ModuleError {
    refusal(
        ModuleErrorCode::UnsupportedControl,
        format!("module `{module}` does not admit the control {detail}"),
    )
}

fn execution_refusal(module: &str, detail: &str) -> ModuleError {
    refusal(
        ModuleErrorCode::ExecutionFailed,
        format!("module `{module}` did not produce an admitted private output: {detail}"),
    )
}

fn hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The bounded frame the pinned standalone Film workspace plans admit,
/// mirroring the film adapter's geometry bounds.
pub fn film_geometry_bounded(width: u64, height: u64) -> bool {
    (1..=FILM_MAX_EDGE).contains(&width)
        && (1..=FILM_MAX_EDGE).contains(&height)
        && width
            .saturating_mul(height)
            .saturating_mul(FILM_SAMPLE_BYTES)
            <= FILM_MAX_DECODED_BYTES
        && 3 * width.max(height) <= FILM_MAX_COORDINATE_SUM
}

/// Module-owned input admission. darktable admits exactly the qualified ARW
/// source contracts it declares; standalone SpektraFilm admits the linear
/// 32-bit ProPhoto TIFF handoff class whose geometry stays inside the film
/// workspace bounds. A shared container name alone never admits an input,
/// and no implicit conversion is inserted for a near miss.
fn validate_input_contract(
    module: &str,
    input: &ConfinedInput,
    description: &ModuleDescription,
) -> Result<(), ModuleError> {
    if input_contract_admitted(module, &input.contract, description) {
        Ok(())
    } else {
        Err(refusal(
            ModuleErrorCode::IncompatibleInput,
            format!(
                "module `{module}` does not admit the input contract {:?}",
                input.contract
            ),
        ))
    }
}

/// The module-owned admission rule over one concrete contract, without byte
/// evidence — the single rule execution and recipe persistence share, so a
/// saved binding's admission never duplicates or widens the executing
/// boundary's own class and geometry check.
fn input_contract_admitted(
    module: &str,
    contract: &ImageContract,
    description: &ModuleDescription,
) -> bool {
    match module {
        DARKTABLE_MODULE => description.admitted_inputs.contains(contract),
        SPEKTRAFILM_MODULE => {
            contract.format == "tiff"
                && contract.color_space == DARKTABLE_OUTPUT_COLOR_SPACE
                && contract.transfer_function == DARKTABLE_OUTPUT_TRANSFER
                && contract.precision_bits
                    == u8::try_from(DARKTABLE_OUTPUT_PRECISION_BITS).expect("fits u8")
                && film_geometry_bounded(contract.width, contract.height)
        }
        // The registry only ever holds the two peer constructors, so every
        // reachable name falls back to its own declared contract list.
        _ => description.admitted_inputs.contains(contract),
    }
}

/// Re-validate the runner's reported output facts against the owning
/// module's pinned output before a result is formed. A runner that reports
/// a malformed digest, an out-of-bounds byte length, or a contract the
/// module does not produce has not completed an admitted output.
fn validate_output_facts(
    module: &str,
    input: &ConfinedInput,
    output: &ModuleOutput,
    description: &ModuleDescription,
) -> Result<(), ModuleError> {
    if !hex64(&output.digest) {
        return Err(execution_refusal(
            module,
            "the reported output digest is not 64 lowercase hex digits",
        ));
    }
    if output.byte_size == 0 || output.byte_size > MAX_OUTPUT_BYTES {
        return Err(execution_refusal(
            module,
            &format!(
                "the reported output byte length {} is outside the 1..={MAX_OUTPUT_BYTES} bound",
                output.byte_size
            ),
        ));
    }
    let pixels = output.contract.width.saturating_mul(output.contract.height);
    if pixels > description.limits.max_output_pixels {
        return Err(execution_refusal(
            module,
            &format!(
                "the reported output frame is {pixels} pixels, above the {}-pixel bound",
                description.limits.max_output_pixels
            ),
        ));
    }
    let admitted = match module {
        DARKTABLE_MODULE => description.admitted_outputs.contains(&output.contract),
        SPEKTRAFILM_MODULE => {
            let contract = &output.contract;
            contract.format == SPEKTRAFILM_OUTPUT_FORMAT
                && contract.color_space == SPEKTRAFILM_OUTPUT_COLOR_SPACE
                && contract.transfer_function == SPEKTRAFILM_OUTPUT_TRANSFER
                && contract.precision_bits
                    == u8::try_from(SPEKTRAFILM_OUTPUT_PRECISION_BITS).expect("fits u8")
                && contract.width == input.contract.width
                && contract.height == input.contract.height
                && film_geometry_bounded(contract.width, contract.height)
        }
        _ => description.admitted_outputs.contains(&output.contract),
    };
    if admitted {
        Ok(())
    } else {
        Err(execution_refusal(
            module,
            &format!(
                "the reported output contract {:?} is not one the module produces",
                output.contract
            ),
        ))
    }
}

/// Validate one complete standalone-SpektraFilm parameter envelope before
/// any engine work: the module and version identity, the bounded envelope
/// size, and the module-owned tree — every runtime group present and
/// exactly the pinned fixed recipe, whose only output is the pinned
/// Finished JPEG. Startup verification uses this same gate to prove a
/// bundle's emitted defaults are the pinned tree; the pinned runtime
/// re-verifies the forwarded tree inside the attempt, and this is the
/// host-side admission shared by discovery, recipe persistence, Export,
/// and Preview.
pub fn validate_spektrafilm_parameters(parameters: &Parameters) -> Result<(), ModuleError> {
    if parameters.module != SPEKTRAFILM_MODULE {
        return Err(refusal(
            ModuleErrorCode::WrongModuleParameters,
            format!(
                "parameter tree is owned by module `{}`, not `{SPEKTRAFILM_MODULE}`",
                parameters.module
            ),
        ));
    }
    if parameters.version != SPEKTRAFILM_PARAMETER_VERSION
        && parameters.version != SPEKTRAFILM_LEGACY_PARAMETER_VERSION
    {
        return Err(refusal(
            ModuleErrorCode::UnsupportedParameterVersion,
            format!(
                "module `{SPEKTRAFILM_MODULE}` does not admit parameter version `{}`",
                parameters.version
            ),
        ));
    }
    let encoded = serde_json::to_vec(parameters).expect("serializable parameters");
    if encoded.len() > MODULE_PARAMETER_BYTES {
        return Err(refusal(
            ModuleErrorCode::ParameterTreeTooLarge,
            format!(
                "parameter request is {} bytes, above the {}-byte envelope bound",
                encoded.len(),
                MODULE_PARAMETER_BYTES
            ),
        ));
    }
    validate_spektrafilm_tree(SPEKTRAFILM_MODULE, &parameters.tree)
}

/// The current darktable description, with its bounded module-owned
/// parameter envelope, ordered stack, and pinned Development TIFF handoff.
fn darktable_description(availability: ModuleAvailability) -> ModuleDescription {
    let manual = crate::native_development::manual_exposure_parameters(0.0);
    let output = json!({
        "format": DARKTABLE_OUTPUT_FORMAT,
        "precisionBits": DARKTABLE_OUTPUT_PRECISION_BITS,
        "colorSpace": DARKTABLE_OUTPUT_COLOR_SPACE,
        "transferFunction": DARKTABLE_OUTPUT_TRANSFER,
        "geometry": DARKTABLE_OUTPUT_GEOMETRY
    });
    let entry =
        json!({"operation": "exposure", "multiPriority": 0, "enabled": true, "params": manual});
    let mut controls = Map::new();
    for (key, value) in manual.as_object().expect("manual controls are an object") {
        let kind = if value.is_boolean() {
            "boolean"
        } else if value.is_string() {
            "string"
        } else {
            "number"
        };
        controls.insert(key.clone(), json!({
            "type": kind,
            "default": value,
            "x-qualification": if key == "exposure" { "editable-manual-exposure" } else { "fixed-qualified-default" }
        }));
    }
    ModuleDescription {
        id: ModuleId {
            name: DARKTABLE_MODULE.into(),
            adapter_version: DARKTABLE_ADAPTER_VERSION.into(),
        },
        parameter_versions: vec![DARKTABLE_PARAMETER_VERSION.into()],
        parameter_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "default": {"stack": [entry], "output": output},
            "x-qualification": "Explicit manual baseline controls reuse the qualified native development request. Exposure is editable; mode, black and both compensation controls stay at these defaults. White balance remains as-shot. Native image-dependent defaults are not qualified defaults. Independent pixel-reference qualification covers 0 and 1 EV only; schema discovery does not grant additional control execution.",
            "x-automatic-adjustments": [{"operation": "exposure", "label": "Auto exposure", "multiPriority": 0, "instruction": {"deflicker_percentile": 50.0, "deflicker_target_level": -4.0}}, {"operation": "channelmixerrgb", "label": "Detect illuminant", "multiPriority": 0, "instruction": {"illuminant": "DT_ILLUMINANT_DETECT_EDGES"}}],
            "properties": {
                "stack": {
                    "type": "array",
                    "maxItems": DARKTABLE_STACK_OPERATIONS_MAX,
                    "default": [entry],
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["operation", "multiPriority", "enabled", "params"],
                        "default": entry,
                        "allOf": [{
                            "if": {"properties": {"operation": {"const": "exposure"}}, "required": ["operation"]},
                            "then": {"properties": {"params": {
                                "type": "object",
                                "default": manual,
                                "properties": controls
                            }}}
                        }],
                        "properties": {
                            "operation": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": MODULE_NAME_BYTES,
                                "pattern": "^[a-z0-9._-]+$"
                            },
                            "multiPriority": {"type": "integer"},
                            "enabled": {"type": "boolean"},
                            "params": {"type": "object"},
                            "before": {"type": "string"},
                            "after": {"type": "string"}
                        }
                    }
                },
                "output": {
                    "type": "object",
                    "additionalProperties": false,
                    "default": output,
                    "required": ["format", "precisionBits", "colorSpace", "transferFunction"],
                    "properties": {
                        "format": {"const": DARKTABLE_OUTPUT_FORMAT},
                        "precisionBits": {"const": DARKTABLE_OUTPUT_PRECISION_BITS},
                        "colorSpace": {"const": DARKTABLE_OUTPUT_COLOR_SPACE},
                        "transferFunction": {"const": DARKTABLE_OUTPUT_TRANSFER},
                        "geometry": {"const": DARKTABLE_OUTPUT_GEOMETRY},
                        "encoding": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": MODULE_NAME_BYTES,
                            "pattern": "^[a-z0-9._-]+$"
                        }
                    }
                }
            }
        }),
        admitted_inputs: vec![sony_arw_contract()],
        admitted_outputs: vec![linear_prophoto_tiff_contract()],
        limits: ModuleLimits {
            max_input_bytes: 512 * 1024 * 1024,
            max_parameter_bytes: MODULE_PARAMETER_BYTES,
            max_output_pixels: 9504 * 6336,
            deadline_millis: 180_000,
        },
        engine_modules: engine::darktable_engine_modules(),
        availability,
    }
}

/// The current standalone SpektraFilm description. Its grouped parameters
/// are its runtime's own manifest groups, never darktable stack entries, and
/// no darktable operation is inserted to emulate them. The schema publishes
/// the complete pinned defaults itself — every group is `required` and
/// carries its executable pinned value as a `const`, so a caller composes
/// the exact executable tree from discovery instead of an empty
/// underdescribed shape. The parameter bound is the film runtime's own
/// 16 KiB parameter bound, the input class the linear ProPhoto
/// handoff inside the film workspace geometry bounds, the output the
/// pinned quality-85 baseline finished JPEG, and the deadline the bound
/// the server's executor actually enforces for both Film execution paths.
fn spektrafilm_description(availability: ModuleAvailability) -> ModuleDescription {
    let pinned = spektrafilm_default_tree();
    let mut properties = Map::new();
    for group in SPEKTRAFILM_GROUPS {
        properties.insert(
            group.into(),
            json!({"const": pinned.get(group).expect("the pinned recipe tree carries every runtime group")}),
        );
    }
    properties.insert(
        "output".into(),
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["format", "precisionBits", "colorSpace", "transferFunction"],
            "properties": {
                "format": {"const": SPEKTRAFILM_OUTPUT_FORMAT},
                "precisionBits": {"const": SPEKTRAFILM_OUTPUT_PRECISION_BITS},
                "colorSpace": {"const": SPEKTRAFILM_OUTPUT_COLOR_SPACE},
                "transferFunction": {"const": SPEKTRAFILM_OUTPUT_TRANSFER},
                "geometry": {"const": SPEKTRAFILM_OUTPUT_GEOMETRY},
                "encoding": {"const": SPEKTRAFILM_OUTPUT_ENCODING}
            }
        }),
    );
    ModuleDescription {
        id: ModuleId {
            name: SPEKTRAFILM_MODULE.into(),
            adapter_version: SPEKTRAFILM_ADAPTER_VERSION.into(),
        },
        parameter_versions: vec![
            SPEKTRAFILM_PARAMETER_VERSION.into(),
            SPEKTRAFILM_LEGACY_PARAMETER_VERSION.into(),
        ],
        parameter_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "x-implementation": SPEKTRAFILM_IMPLEMENTATION,
            "x-fork-reference": "suraciii/spektrafilm-rs",
            "x-legacy-parameter-version": SPEKTRAFILM_LEGACY_PARAMETER_VERSION,
            "default": pinned,
            "required": SPEKTRAFILM_GROUPS.to_vec(),
            "properties": properties
        }),
        admitted_inputs: vec![linear_prophoto_tiff_contract()],
        admitted_outputs: vec![srgb_jpeg_contract()],
        limits: ModuleLimits {
            max_input_bytes: FILM_MAX_DECODED_BYTES,
            max_parameter_bytes: FILM_PARAMETER_BYTES,
            max_output_pixels: SPEKTRAFILM_MAX_OUTPUT_PIXELS,
            deadline_millis: SPEKTRAFILM_DEADLINE_MILLIS,
        },
        engine_modules: engine::spektrafilm_engine_modules(),
        availability,
    }
}

/// The qualified Sony ARW source class of the first development workload,
/// within the crate's existing geometry envelope.
fn sony_arw_contract() -> ImageContract {
    ImageContract {
        format: "arw".into(),
        color_space: "camera-native".into(),
        transfer_function: "linear".into(),
        precision_bits: 14,
        width: 9504,
        height: 6336,
    }
}

/// The linear ProPhoto TIFF development handoff one peer exports and the
/// other admits.
fn linear_prophoto_tiff_contract() -> ImageContract {
    ImageContract {
        format: "tiff".into(),
        color_space: "prophoto-rgb".into(),
        transfer_function: "linear".into(),
        precision_bits: 32,
        width: 9504,
        height: 6336,
    }
}

/// The finished sRGB JPEG the film stage renders.
fn srgb_jpeg_contract() -> ImageContract {
    ImageContract {
        format: "jpeg".into(),
        color_space: "srgb".into(),
        transfer_function: "srgb".into(),
        precision_bits: 8,
        width: 9504,
        height: 6336,
    }
}
