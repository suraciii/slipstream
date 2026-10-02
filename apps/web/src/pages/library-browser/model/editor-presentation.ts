import type { EditSourceKind, EditSourceReadiness } from "./photo-editor.js";

export const formatByteCount = (bytes: number): string => {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
};

/// The Source support fact line. The readiness word names the axis's own
/// state, the Library's scan phase rides along while the source is being
/// checked, and a proxy edit source is named as provenance.
export const sourceFactNote = (
  readiness: EditSourceReadiness,
  editSource: EditSourceKind,
  libraryPhase?: () => string,
): string => {
  const word =
    readiness === "checking"
      ? "Checking source…"
      : readiness === "ready"
        ? "Ready"
        : readiness === "missing"
          ? "Original missing"
          : readiness === "unreadable"
            ? "Original unreadable"
            : "Unsupported source class";
  const phase = readiness === "checking" ? (libraryPhase?.() ?? "") : "";
  const provenance =
    editSource === "development-proxy"
      ? " (Development Proxy edit source)"
      : "";
  return phase ? `${word} — ${phase}${provenance}` : `${word}${provenance}`;
};
/// The session's internal wording in the workspace's plain language. The
/// compact status line never names recipes, revisions, or deployments;
/// the original wording stays available under the Details affordance.
export const plainEditorMessage = (message: string): string =>
  message
    .replace(
      "This Photo's source class has no approved profile in this deployment",
      "This Photo is not supported for editing",
    )
    .replace(
      "Reading this Photo's Original File failed for its current source revision",
      "Reading this Photo's Original File failed",
    )
    .replace(
      "Reload the recipe to check for current source facts.",
      "Reload to check again.",
    )
    .replace(
      "Reload the recipe to retry once the current Library work settles.",
      "Reload to retry.",
    )
    .replaceAll("the saved recipe", "the saved edit")
    .replaceAll("The saved recipe", "The saved edit");
/// The conflict resolutions are preserved exactly; only their wording moves
/// from service concepts to the Photographer's edit.
export const plainConflictMessage = (message: string): string => {
  if (
    message ===
    "A local draft from an earlier revision was recovered. Use the saved recipe or reapply the draft."
  )
    return "A local draft from an earlier version of this Photo was recovered. Use the saved edit or reapply the draft.";
  if (
    message ===
    "The saved recipe is bound to a different source. Rebind it or use the saved recipe."
  )
    return "The saved edit belongs to a different file. Keep the edit for the current file, or use the saved edit.";
  if (
    message ===
    "The Original File changed elsewhere. Autosave stopped; use the saved recipe or reapply the local settings."
  )
    return "The Original File changed elsewhere. Autosave stopped; use the saved edit or reapply your changes.";
  if (
    message ===
    "The saved recipe changed elsewhere. Autosave stopped; use the saved recipe or reapply the local settings."
  )
    return "The saved edit changed elsewhere. Autosave stopped; use the saved edit or reapply your changes.";
  return plainEditorMessage(message);
};
export const plainWhiteBalanceNote = (note: string): string =>
  note.startsWith("This deployment admits temperature and tint")
    ? "Temperature and tint are not available for this Photo right now, so the controls stay read-only."
    : "Only as-shot white balance is available for this Photo.";
/// The one compact status line. Uncertain effects read as checking, never
/// as failure; a refused save offers its retry; everything else the session
/// reports is either transient progress or a read-only explanation.
export const compactEditorStatus = (
  presented: Readonly<{
    photoId: string;
    saving: boolean;
    dirty: boolean;
    canEdit: boolean;
    processingAvailable: boolean;
    conflict: unknown;
    status: string;
    recipeVersion: string | null;
  }>,
): string => {
  if (presented.photoId === "") return "Loading edit…";
  if (presented.conflict)
    return "Could not update — choose how to resolve it below.";
  const status = presented.status;
  if (status.startsWith("The save outcome is unknown"))
    return "Checking result…";
  if (
    status.startsWith("The save was refused") ||
    (presented.dirty && !presented.saving)
  )
    return "Could not update — Retry";
  if (presented.saving) return "Saving…";
  if (status.startsWith("This deployment does not admit the white-balance"))
    return "This white-balance mode is not available for this Photo.";
  if (status.startsWith("This deployment does not admit temperature"))
    return "Temperature and tint are not available for this Photo.";
  if (
    status === "" ||
    status === "Saved." ||
    status === "Using the saved recipe."
  )
    return presented.recipeVersion ? "Edit state saved" : "No saved edit yet";
  return plainEditorMessage(status);
};
