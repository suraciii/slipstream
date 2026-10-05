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
      "Reload the Edit State to check for current source facts.",
      "Reload to check again.",
    )
    .replace(
      "Reload the Edit State to retry once the current Library work settles.",
      "Reload to retry.",
    )
    .replaceAll("the saved recipe", "the saved edit")
    .replaceAll("The saved recipe", "The saved edit");
