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
    return "A comparison for another Photo, stage, or source arrived and was discarded.";
  return "";
};
