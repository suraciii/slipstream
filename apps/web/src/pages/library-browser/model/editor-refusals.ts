import { isRecord } from "../api/guards.js";
import { composablePreviewRefusalNote } from "./composable-preview.js";
import { plainEditorMessage } from "./editor-presentation.js";
type SourceSupportReason =
  | ""
  | "original-missing"
  | "original-unreadable"
  | "read-pending"
  | "resource-unavailable";
const asEditorSupportReason = (value: unknown): SourceSupportReason =>
  value === "original-missing" ||
  value === "original-unreadable" ||
  value === "read-pending" ||
  value === "resource-unavailable"
    ? value
    : "";
const isRetryableSupportReason = (reason: SourceSupportReason): boolean =>
  reason === "read-pending" || reason === "resource-unavailable";
const supportReasonExplanation = (reason: SourceSupportReason): string => {
  switch (reason) {
    case "original-missing":
      return "This Photo's Original File is missing from its remembered Location";
    case "original-unreadable":
      return "Reading this Photo's Original File failed for its current source revision";
    case "read-pending":
      return "This Photo's source read is still pending while the Library recovers";
    case "resource-unavailable":
      return "The Library could not spare the capacity to read this Photo's Original File";
    default:
      return "Current source facts are unavailable";
  }
};
export const describePreviewRefusal = async (
  response: Response,
): Promise<string> => {
  const body: unknown = await response.json().catch(() => undefined);
  const error = isRecord(body) ? body["error"] : undefined;
  const code =
    isRecord(error) && typeof error["code"] === "string" ? error["code"] : "";
  const reason =
    isRecord(error) &&
    isRecord(error["details"]) &&
    typeof error["details"]["reason"] === "string"
      ? error["details"]["reason"]
      : "";
  const composableRefusal = composablePreviewRefusalNote(code);
  if (composableRefusal) return composableRefusal;
  if (code === "processing_unavailable") {
    if (reason === "preview-render-admission-unavailable")
      return "Previews are not available yet in this deployment, so no preview is shown. Editing, Export, and download still work.";
    return "Processing is not available for this Photo right now, so no preview is shown.";
  }
  if (code === "resource_unavailable") {
    // A refusal that follows from the Photo's source state carries the
    // same closed reason the edit read reports, so the preview surface
    // agrees with the read: a retryable wait names its retry, a confirmed
    // outcome explains the permanent read failure.
    const sourceReason = asEditorSupportReason(reason);
    if (sourceReason !== "") {
      const note = `${plainEditorMessage(
        supportReasonExplanation(sourceReason),
      )}, so no preview is shown.`;
      return isRetryableSupportReason(sourceReason)
        ? `${note} Refresh the preview once the current Library work settles.`
        : note;
    }
    return "Could not create the preview right now. Refresh the preview to try again.";
  }
  if (code === "unsupported_photo")
    return "This Photo is not supported for editing, so no preview is shown.";
  if (code === "unknown_photo")
    return "This Photo is no longer in the Library, so no preview is shown.";
  return code
    ? `The service refused the preview: ${code}${reason ? ` (${reason})` : ""}.`
    : `The preview request failed with HTTP ${response.status}.`;
};

export const describeEditRefusal = async (
  response: Response,
  subject: string,
): Promise<string> => {
  const body: unknown = await response.json().catch(() => undefined);
  const error = isRecord(body) ? body["error"] : undefined;
  const code =
    isRecord(error) && typeof error["code"] === "string"
      ? error["code"]
      : `HTTP ${response.status}`;
  const reason =
    isRecord(error) &&
    isRecord(error["details"]) &&
    typeof error["details"]["reason"] === "string"
      ? ` (${error["details"]["reason"]})`
      : "";
  return `${subject} is unavailable: ${code}${reason}.`;
};
