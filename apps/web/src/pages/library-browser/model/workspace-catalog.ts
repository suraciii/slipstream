import type { BrowserFetch } from "./access-session.js";
import {
  toolCommand,
  workspaceRequest,
  WorkspaceApiError,
  requestIdentity,
  type JsonObject,
} from "../api/workspace-edit.js";

export type CatalogIntent = {
  editId: string | null;
  operation: string;
  arguments: JsonObject;
  global?: boolean;
};
/** An uncertain operation retains the entire original payload, including its observed guard. */
export function createCatalogMutation(
  fetcher: BrowserFetch,
  photoId: string,
  confirmed: () => Promise<void>,
) {
  let pending: CatalogIntent | undefined;
  let busy = false;
  let error: WorkspaceApiError | undefined;
  async function replay(): Promise<JsonObject | undefined> {
    if (busy || !pending) return;
    busy = true;
    error = undefined;
    try {
      const result = pending.global
        ? await workspaceRequest(fetcher, "/api/darktable/commands", {
            operation: pending.operation,
            editId: null,
            arguments: pending.arguments,
          })
        : await toolCommand(
            fetcher,
            photoId,
            pending.editId,
            pending.operation,
            pending.arguments,
          );
      if (
        !["saved", "unchanged", "replayed", "created", "removed"].includes(
          String(result.outcome),
        )
      )
        throw new WorkspaceApiError(
          "outcome_unknown",
          "No confirmed mutation outcome was returned. Replay this exact request.",
          "unknown",
          result,
        );
      pending = undefined;
      try {
        await confirmed();
      } catch (failure) {
        throw new WorkspaceApiError(
          "refresh_failed",
          `Mutation confirmed; refreshing failed: ${String(failure)}`,
          "none",
          result,
        );
      }
      return result;
    } catch (failure) {
      error =
        failure instanceof WorkspaceApiError
          ? failure
          : new WorkspaceApiError(
              "outcome_unknown",
              String(failure),
              "unknown",
              {},
            );
      throw error;
    } finally {
      busy = false;
    }
  }
  return {
    state: () => ({ pending, busy, error }),
    async submit(intent: CatalogIntent) {
      if (busy || pending)
        throw new Error(
          "Resolve the retained catalog request before another mutation.",
        );
      pending = {
        ...intent,
        arguments: structuredClone({
          ...intent.arguments,
          requestId: requestIdentity(),
        }),
      };
      return replay();
    },
    replay,
    discard() {
      if (busy || error?.effect === "unknown")
        throw new Error("Replay the unknown request before discarding it.");
      pending = undefined;
      error = undefined;
    },
  };
}

export function catalogMapping(
  entries: JsonObject[],
  selected: Record<string, string>,
): JsonObject[] {
  const used = new Set<string>();
  return entries.map((entry) => {
    const entryId = String(entry.entryId);
    const target = selected[entryId];
    if (!target)
      throw new Error(`Choose a target or New instance for entry ${entryId}.`);
    if (target !== "@new" && used.has(target))
      throw new Error(
        "An existing target cannot receive more than one style entry.",
      );
    if (target !== "@new") used.add(target);
    return { entryId, instanceId: target === "@new" ? null : target };
  });
}
