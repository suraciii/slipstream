import type { BrowserFetch } from "../api/browser-fetch.js";
import { isRecord } from "../api/guards.js";
import {
  parseWindow,
  workspaceRequest,
  WorkspaceApiError,
} from "../api/workspace-edit.js";
import type { WorkspaceEditModel } from "./workspace-edit-model.js";

/** Read the supervised workspace tool independently of the processing registry. */
export function createWorkspaceToolDiscovery(
  fetcher: BrowserFetch,
  model: WorkspaceEditModel,
  publish: () => void,
) {
  let alive = true;
  let generation = 0;
  const discover = async (force = false) => {
    const file = model.files.find(
      (item) =>
        item.artifactId === model.selectedInput && item.state === "ready",
    );
    if (!file || !file.revision) {
      generation++;
      delete model.toolDiscovery;
      publish();
      return;
    }
    if (
      !force &&
      model.toolDiscovery?.input === file.artifactId &&
      model.toolDiscovery.revision === file.revision
    )
      return;
    const captured = ++generation;
    const input = file.artifactId;
    const revision = file.revision;
    model.toolDiscovery = { input, revision, state: "loading", reasons: [] };
    publish();
    let state: "ready" | "unknown" | "unavailable" = "unknown";
    let reasons: readonly string[] = [];
    try {
      const result = await workspaceRequest(
        fetcher,
        "/api/darktable/commands",
        {
          operation: "modules list",
          editId: null,
          arguments: {},
        },
      );
      parseWindow(result, (value) => {
        if (!isRecord(value)) throw new Error("Invalid tool module");
        return value;
      });
      if (
        typeof result.bundleId !== "string" ||
        typeof result.schemaVersion !== "string"
      )
        throw new Error("Invalid tool discovery identity");
      state = "ready";
    } catch (error) {
      if (
        error instanceof WorkspaceApiError &&
        error.code === "engine_unavailable"
      ) {
        state = "unavailable";
        reasons = [error.message];
      }
    }
    if (
      !alive ||
      captured !== generation ||
      model.selectedInput !== input ||
      !model.files.some(
        (item) =>
          item.artifactId === input &&
          item.revision === revision &&
          item.state === "ready",
      )
    )
      return;
    model.toolDiscovery = { input, revision, state, reasons };
    publish();
  };
  return {
    discover,
    dispose() {
      alive = false;
    },
  };
}
