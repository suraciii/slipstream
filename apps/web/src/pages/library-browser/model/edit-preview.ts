//! The reading of one Edit Preview rendition: the request the workspace makes
//! for the current settings or for the as-shot/baseline development a
//! comparison presents, and the checks a served comparison must pass before it
//! is presented.
//!
//! [Preview Behavior](../../../../docs/photo-development.md#preview-behavior)
//! requires a comparison to keep the chosen stage and display conversion
//! constant while it presents the as-shot/baseline development settings, and
//! [Photo Development](../../../../design/photo-development.md#service-surface)
//! answers it as the same route under a closed `settings` selector. That
//! reading lives here, away from the page's DOM and network work, so it can be
//! checked without a browser.

import type { EditSourceKind } from "./photo-editor.js";

/// The closed settings selectors of the route. `current` is the saved Edit
/// Recipe's settings and the default; `baseline` is the as-shot/baseline
/// development settings the comparison presents, derived from the Photo rather
/// than from the saved recipe.
export const CURRENT_SETTINGS = "current";
export const BASELINE_SETTINGS = "baseline";
export type EditPreviewSettings =
  | typeof CURRENT_SETTINGS
  | typeof BASELINE_SETTINGS;

/// One Edit Preview request. A comparison asks for the chosen stage under
/// `baseline`, so both images share the stage and its display conversion.
export const editPreviewUri = (
  photoId: string,
  stage: string,
  settings: EditPreviewSettings,
): string =>
  `/api/photos/${encodeURIComponent(photoId)}/edit-preview/${encodeURIComponent(stage)}?settings=${settings}`;

/// The rendition a comparison asks for, and the development it is defined by.
/// A baseline rendition names no saved recipe, so a comparison stays current
/// while the saved settings move: it is the Photo, the stage, and the source
/// revision that make it the comparison of one development.
export type Comparison = Readonly<{
  photoId: string;
  stage: string;
  sourceRevision: string | null;
}>;

/// True while a retained comparison still describes the development the
/// workspace presents.
export const comparisonIsCurrent = (
  comparison: Comparison | undefined,
  current: Readonly<{
    photoId: string;
    stage: string;
    sourceRevision: string | null;
  }>,
): boolean =>
  comparison !== undefined &&
  comparison.photoId === current.photoId &&
  comparison.stage === current.stage &&
  comparison.sourceRevision === current.sourceRevision;

/// The wire encoding of the opaque source revision: the route hex-encodes it
/// for its `sourceRevision` header
/// (`crates/slipstream-server/src/edit_preview.rs`).
export const encodedSourceRevision = (revision: string): string =>
  Array.from(new TextEncoder().encode(revision), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");

/// Why a served rendition is not the requested comparison, or `""` when it is.
/// A rendition of another Photo, stage, settings selector, or source revision
/// is never the requested image, so the workspace says so instead of comparing
/// unrelated images.
export const comparisonRefusal = (
  headers: Readonly<{ get: (name: string) => string | null }>,
  expected: Comparison,
): string => {
  if (headers.get("slipstream-edit-preview-settings") !== BASELINE_SETTINGS)
    return "A rendition of the current settings arrived instead of the comparison, so it was discarded.";
  const photoId = headers.get("slipstream-edit-preview-photo-id");
  const stage = headers.get("slipstream-edit-preview-stage");
  // A Photo whose source revision is unknown travels as the empty header, the
  // same reading the current rendition's validation uses.
  const sourceRevision =
    headers.get("slipstream-edit-preview-source-revision") ?? "";
  const expectedSource =
    expected.sourceRevision === null
      ? ""
      : encodedSourceRevision(expected.sourceRevision);
  if (
    photoId !== expected.photoId ||
    stage !== expected.stage ||
    sourceRevision !== expectedSource
  )
    return "A comparison for another Photo, result, or file arrived and was discarded.";
  return "";
};

/// The closed metadata of a current rendition. A rendition whose identity is
/// not exactly the requested one — another Photo, stage, source revision,
/// recipe snapshot, or edit source — is never the requested preview.
export const EDIT_PREVIEW_CONTENT_TYPE = "image/jpeg";
/// The qualified display transform of the Development display derivative. A
/// rendition under another transform is not the current preview.
export const EDIT_PREVIEW_DISPLAY_TRANSFORM = "display-transform-v1";

export type CurrentRendition = Readonly<{
  photoId: string;
  stage: string;
  sourceRevision: string | null;
  recipeVersion: string;
  /// The edit source the current recipe read resolved against. A rendition
  /// served from another edit source — a Development Proxy image while the
  /// Original File is the current source, or the reverse — is a successful
  /// image for another source, not the requested preview, even when the
  /// revision token itself is unchanged.
  editSource: EditSourceKind;
  /// The proxy identity digest the current recipe read bound the edit source
  /// to, or `null` for the Original File. A replaced proxy carries a new
  /// digest, so a late rendition of the earlier proxy fails this guard even
  /// when the source revision token did not move.
  editSourceProxyId: string | null;
}>;

/// Why a served rendition is not the requested current preview, or `""` when
/// it is. The wire encoding of the opaque source revision is the route's
/// hex-encoded header, an absent edit-source header reads as the Original
/// File an older service always rendered from, and an absent proxy identity
/// reads as none.
export const currentRenditionRefusal = (
  headers: Readonly<{ get: (name: string) => string | null }>,
  byteLength: number,
  expected: CurrentRendition,
): string => {
  const contentType = (headers.get("content-type") ?? "").split(";")[0]?.trim();
  const width = Number(headers.get("slipstream-edit-preview-width"));
  const height = Number(headers.get("slipstream-edit-preview-height"));
  const declaredLength = Number(headers.get("content-length"));
  const sha256 = headers.get("slipstream-edit-preview-sha256") ?? "";
  const displayTransform =
    headers.get("slipstream-edit-preview-display-transform") ?? "";
  const wellFormed =
    contentType === EDIT_PREVIEW_CONTENT_TYPE &&
    Number.isInteger(width) &&
    width > 0 &&
    Number.isInteger(height) &&
    height > 0 &&
    Number.isInteger(declaredLength) &&
    declaredLength === byteLength &&
    /^[0-9a-f]{64}$/.test(sha256) &&
    displayTransform === EDIT_PREVIEW_DISPLAY_TRANSFORM;
  if (!wellFormed)
    return "An Edit Preview without its complete metadata arrived and was discarded.";
  const renderedPhoto = headers.get("slipstream-edit-preview-photo-id") ?? "";
  const renderedStage = headers.get("slipstream-edit-preview-stage") ?? "";
  const sourceRevision =
    headers.get("slipstream-edit-preview-source-revision") ?? "";
  const recipeVersion =
    headers.get("slipstream-edit-preview-recipe-version") ?? "";
  const renderedEditSource =
    headers.get("slipstream-edit-preview-source") ?? "original";
  const renderedProxyId = headers.get("slipstream-edit-preview-proxy-id") ?? "";
  const expectedEncodedSource =
    expected.sourceRevision === null
      ? ""
      : encodedSourceRevision(expected.sourceRevision);
  if (
    renderedPhoto !== expected.photoId ||
    renderedStage !== expected.stage ||
    sourceRevision !== expectedEncodedSource ||
    recipeVersion !== expected.recipeVersion
  )
    // A successful image for a different source, stage, or settings
    // snapshot is not the requested preview.
    return "A preview for other settings or an earlier source arrived and was discarded.";
  if (
    renderedEditSource !== expected.editSource ||
    renderedProxyId !== (expected.editSourceProxyId ?? "")
  )
    return "A rendition from another edit source arrived and was discarded.";
  return "";
};
