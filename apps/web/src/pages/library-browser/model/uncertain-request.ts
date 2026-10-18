import { WorkspaceApiError, type JsonObject } from "../api/workspace-edit.js";

/**
 * A tool request whose admission outcome is unknown. The carrier retains the
 * entire original payload, including its observed guard and request identity,
 * so the exact request can be replayed instead of submitted again as new
 * intent. Nothing here may be refreshed from a later observation.
 */
export type UncertainRequest = Readonly<{
  operation: string;
  arguments: JsonObject;
  editId: string | null;
  /** Global catalog commands answer to the global authority, not this edit. */
  global?: boolean;
}>;

/** True when a failure leaves a request's admission explicitly undecided. */
export function isUnknownEffect(error: unknown): boolean {
  return error instanceof WorkspaceApiError && error.effect === "unknown";
}
