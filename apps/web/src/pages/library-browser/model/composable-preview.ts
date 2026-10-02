//! The reading of one composable Processing Step Preview: which Processing
//! Step the workspace may request, and how the route's refusals are said in
//! the workspace's words.
//!
//! [Composable Photo Processing Modules](../../../../../../design/processing-modules.md)
//! bounds the Preview to the recipe's caller-selected current Processing
//! Step. Selection and refusal facts live here, independently of DOM and
//! network work; an empty recipe has no processing result.

import { blobSha256Hex } from "./browser-crypto.js";
import type {
  ComposableProcessingStep,
  ComposableRecipe,
} from "../api/composable-recipe.js";

/// One settled composable recipe read: the Photo's current source revision
/// and the saved composable recipe, or `null` when none is saved.
export type ComposableRecipeRead = Readonly<{
  sourceRevision: string;
  recipe: ComposableRecipe | null;
}>;

/// The recipe's selected current Processing Step, or null when none exists.
export const selectedComposableStep = (
  read: ComposableRecipeRead | undefined,
): ComposableProcessingStep | null => {
  const recipe = read?.recipe;
  const current = recipe?.currentStepId;
  if (!recipe || !current) return null;
  return recipe.steps.find((step) => step.stepId === current) ?? null;
};

/// What the workspace's preview request is bound to.
export type ComposablePreviewTarget = Readonly<
  | { kind: "unreadable" }
  | { kind: "none" }
  | { kind: "step"; step: ComposableProcessingStep }
>;

/// An unreadable recipe yields no request; an empty recipe has no selected step.
export const composablePreviewTarget = (
  read: ComposableRecipeRead | undefined,
): ComposablePreviewTarget => {
  if (read === undefined) return { kind: "unreadable" };
  if (read.recipe === null) return { kind: "none" };
  const step = selectedComposableStep(read);
  return step ? { kind: "step", step } : { kind: "none" };
};

/// The composable Preview route's refusal in the workspace's words, or
/// `undefined` for a code the route does not own. A code this client does
/// not know stays the service's own disclosure in the caller instead of
/// being explained away here.
export const composablePreviewRefusalNote = (
  code: string,
): string | undefined => {
  switch (code) {
    case "missing_recipe":
      return "No Processing Recipe is saved for this Photo anymore, so no Preview is shown. Reload to check again.";
    case "source_changed":
      return "The saved Processing Recipe belongs to an earlier version of this Photo, so no Preview is shown. Reload to check again.";
    case "step_not_current":
      return "The selected Processing Step is no longer the recipe's current step, so no Preview is shown. Reload to check again.";
    case "unknown_step":
      return "The selected Processing Step is no longer part of the saved Processing Recipe, so no Preview is shown.";
    case "unknown_module":
      return "The selected Processing Step names a Processing Module this deployment does not know, so no Preview is shown.";
    case "incompatible_input":
      return "The selected Processing Step's input is not admitted by its module, so no Preview is shown.";
    case "module_parameters_unavailable":
      return "The selected Processing Step's parameters have no qualified rendering adapter in this deployment yet, so no Preview is shown.";
    default:
      return undefined;
  }
};

/// The identity facts a served selected-step Preview is validated against
/// when the route publishes them: the Photo, the selected step, the recipe
/// revision the request captured, and the source revision the recipe is
/// bound to.
export type ComposablePreviewIdentity = Readonly<{
  photoId: string;
  stepId: string;
  recipeRevision: string;
  sourceRevision: string;
  comparison?: "current" | "baseline";
}>;

/// The selected-step route's rendition identity headers, in the route's own
/// naming. The source revision travels hex encoded, because header values
/// are text. The names live here so a service-side change is one change.
const IDENTITY_HEADER_PREFIX = "slipstream-processing-preview-";
const PHOTO_ID_HEADER = `${IDENTITY_HEADER_PREFIX}photo-id`;
const STEP_ID_HEADER = `${IDENTITY_HEADER_PREFIX}step-id`;
const SOURCE_REVISION_HEADER = `${IDENTITY_HEADER_PREFIX}source-revision`;
const RECIPE_REVISION_HEADER = `${IDENTITY_HEADER_PREFIX}recipe-revision`;
const SHA256_HEADER = `${IDENTITY_HEADER_PREFIX}sha256`;
const WIDTH_HEADER = `${IDENTITY_HEADER_PREFIX}width`;
const HEIGHT_HEADER = `${IDENTITY_HEADER_PREFIX}height`;
const GEOMETRY_HEADER = `${IDENTITY_HEADER_PREFIX}geometry`;
const BUNDLE_ID_HEADER = `${IDENTITY_HEADER_PREFIX}bundle-id`;
const MODULE_HEADER = `${IDENTITY_HEADER_PREFIX}module`;
const ADAPTER_SCHEMA_VERSION_HEADER = `${IDENTITY_HEADER_PREFIX}adapter-schema-version`;
const PARAMETER_DIGEST_HEADER = `${IDENTITY_HEADER_PREFIX}parameter-digest`;
const OUTPUT_CONTRACT_HEADER = `${IDENTITY_HEADER_PREFIX}output-contract`;
const DISPLAY_CONVERSION_HEADER = `${IDENTITY_HEADER_PREFIX}display-conversion`;
const IDENTITY_HEADER = `${IDENTITY_HEADER_PREFIX}identity`;
const IDENTITY_HEADERS = [
  PHOTO_ID_HEADER,
  STEP_ID_HEADER,
  SOURCE_REVISION_HEADER,
  RECIPE_REVISION_HEADER,
  SHA256_HEADER,
  WIDTH_HEADER,
  HEIGHT_HEADER,
  GEOMETRY_HEADER,
  BUNDLE_ID_HEADER,
  MODULE_HEADER,
  ADAPTER_SCHEMA_VERSION_HEADER,
  PARAMETER_DIGEST_HEADER,
  OUTPUT_CONTRACT_HEADER,
  DISPLAY_CONVERSION_HEADER,
  IDENTITY_HEADER,
] as const;

/// Decodes the source revision the service hex encodes for the response
/// headers back to its opaque UTF-8 text.
const decodeHexSourceRevision = (value: string | null): string | null => {
  if (
    value === null ||
    value.length === 0 ||
    value.length % 2 !== 0 ||
    value.length > 32_768 ||
    !/^[0-9a-f]+$/i.test(value)
  )
    return null;
  const bytes: number[] = [];
  for (let index = 0; index < value.length; index += 2) {
    const high = Number.parseInt(value[index]!, 16);
    const low = Number.parseInt(value[index + 1]!, 16);
    if (Number.isNaN(high) || Number.isNaN(low)) return null;
    bytes.push(high * 16 + low);
  }
  try {
    const decoded = new TextDecoder("utf-8", { fatal: true }).decode(
      new Uint8Array(bytes),
    );
    return decoded.length > 0 ? decoded : null;
  } catch {
    return null;
  }
};

/// Why a served step Preview's identity cannot be presented, or `""` when
/// it can. The route publishes the complete identity framing of a bounded
/// Preview — selection, input revision, bytes, geometry, module/schema,
/// parameter and output-contract digests, display conversion, bundle, and
/// complete identity digest — so omitted or malformed facts are refused.
export const composablePreviewIdentityRefusal = (
  headers: Headers,
  expected: ComposablePreviewIdentity,
): string => {
  const refusal =
    "The Processing Step Preview could not be identified. Refresh the preview to try again.";
  const dimension = (name: string): boolean => {
    const value = Number.parseInt(headers.get(name) ?? "", 10);
    return Number.isInteger(value) && value > 0 && value <= 65_535;
  };
  const bounded = (name: string): boolean => {
    const value = headers.get(name) ?? "";
    return value.length > 0 && value.length <= 8_192;
  };
  const digest = (name: string): boolean =>
    /^[0-9a-f]{64}$/.test(headers.get(name) ?? "");
  const identified =
    IDENTITY_HEADERS.every((name) => headers.get(name) !== null) &&
    headers.get(PHOTO_ID_HEADER) === expected.photoId &&
    headers.get(STEP_ID_HEADER) === expected.stepId &&
    headers.get(RECIPE_REVISION_HEADER) === expected.recipeRevision &&
    decodeHexSourceRevision(headers.get(SOURCE_REVISION_HEADER)) ===
      expected.sourceRevision &&
    (expected.comparison === undefined ||
      headers.get(`${IDENTITY_HEADER_PREFIX}comparison`) ===
        expected.comparison) &&
    (expected.comparison === undefined ||
      (digest(`${IDENTITY_HEADER_PREFIX}input-sha256`) &&
        /^[1-9][0-9]*$/.test(
          headers.get(`${IDENTITY_HEADER_PREFIX}input-byte-length`) ?? "",
        ))) &&
    digest(SHA256_HEADER) &&
    dimension(WIDTH_HEADER) &&
    dimension(HEIGHT_HEADER) &&
    dimension(GEOMETRY_HEADER) &&
    bounded(BUNDLE_ID_HEADER) &&
    bounded(MODULE_HEADER) &&
    bounded(ADAPTER_SCHEMA_VERSION_HEADER) &&
    digest(PARAMETER_DIGEST_HEADER) &&
    digest(OUTPUT_CONTRACT_HEADER) &&
    bounded(DISPLAY_CONVERSION_HEADER) &&
    digest(IDENTITY_HEADER);
  return identified ? "" : refusal;
};

/// The PNG signature and the IHDR chunk that carries the served frame's own
/// geometry, so the bytes the route serves are checked against the geometry
/// its headers declare instead of being trusted for their shape.
const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// Why a served step Preview's bytes cannot be presented as the declared
/// rendition, or `""` when they can: the body must be a PNG whose own IHDR
/// frame matches the width and height the identity framing declared.
export const composablePreviewPngRefusal = (
  bytes: Uint8Array,
  width: string | null,
  height: string | null,
): string => {
  const refusal =
    "The Processing Step Preview could not be read. Refresh the preview to try again.";
  if (
    bytes.length < 24 ||
    !PNG_SIGNATURE.every((byte, index) => bytes[index] === byte) ||
    String.fromCharCode(...bytes.slice(12, 16)) !== "IHDR"
  )
    return refusal;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const frameWidth = view.getUint32(16);
  const frameHeight = view.getUint32(20);
  return frameWidth > 0 &&
    frameHeight > 0 &&
    String(frameWidth) === width &&
    String(frameHeight) === height
    ? ""
    : refusal;
};

/// Why a served step Preview's bytes cannot be presented, or `""` when they
/// can: the body must hash to the digest the identity framing published. A
/// digest that cannot be computed or does not match is refused rather than
/// presented as the selected step's result.
export const composablePreviewDigestRefusal = async (
  claimed: string | null,
  bytes: ArrayBuffer,
): Promise<string> => {
  const digest = await blobSha256Hex(new Blob([bytes])).catch(() => undefined);
  return claimed !== null && digest === claimed
    ? ""
    : "The Processing Step Preview could not be verified. Refresh the preview to try again.";
};

/// Why a served step Preview cannot be presented, or `""` when it can. The
/// route's bytes check: a response that is not an image with bytes is
/// refused rather than presented as the selected step's result. The
/// identity check above governs the framing the route publishes.
export const composablePreviewReadRefusal = (
  contentType: string | null,
  byteLength: number,
): string =>
  contentType !== null && contentType.startsWith("image/") && byteLength > 0
    ? ""
    : "The Processing Step Preview could not be read. Refresh the preview to try again.";
