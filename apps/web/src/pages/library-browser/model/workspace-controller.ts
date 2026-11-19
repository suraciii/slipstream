import { randomUuid } from "./browser-crypto.js";
import type { BrowserFetch } from "../api/browser-fetch.js";
import {
  checkWorkspaceDownload,
  fileBytesUrl,
  parseWindow,
  parseWorkspace,
  parseWorkspaceFile,
  workspaceFilePath,
  workspaceFilesPath,
  workspacePath,
  workspaceRequest,
  WorkspaceApiError,
  type JsonObject,
  type Workspace,
} from "../api/workspace.js";
import type {
  WorkspaceController,
  WorkspaceControllerDependencies,
  WorkspaceViewModel,
  WorkspaceViewPort,
} from "./workspace-controller-contract.js";

const noDefaultInput = "No default input selected.";

const argument = (args: JsonObject, key: string): string | undefined =>
  typeof args[key] === "string" ? args[key] : undefined;

const statusFor = (state: WorkspaceViewModel): string => {
  if (!state.workspace) return "Workspace unavailable.";
  if (!state.workspace.inputArtifactId) return noDefaultInput;
  return state.files.length ? "Ready." : "No completed files yet.";
};

/// Presents one Photo's Workspace as a file view. It owns reads of the
/// workspace catalogue, bounded file pages, inspection, download admission, and
/// removal admission; it never sends a tool command or requests a Preview.
export function createWorkspaceController(
  fetcher: BrowserFetch,
  view: WorkspaceViewPort,
  dependencies: WorkspaceControllerDependencies,
): WorkspaceController {
  let alive = true;
  let selected: string | undefined;
  let generation = 0;
  let inspectionVersion = 0;
  let removalRequest:
    | Readonly<{ artifactId: string; revision: string; requestId: string }>
    | undefined;
  let state: WorkspaceViewModel | undefined;

  const owns = (photoId: string, run: number): boolean =>
    alive &&
    selected === photoId &&
    generation === run &&
    dependencies.isAlive() &&
    dependencies.isCurrentPhoto(photoId);

  const publish = (): void => {
    if (
      !alive ||
      !state ||
      !dependencies.isAlive() ||
      !dependencies.isCurrentPhoto(state.photoId) ||
      !view.workspaceVisible()
    )
      return;
    view.renderWorkspace(state);
  };

  const failure = (error: unknown, scope: string): void => {
    if (!state) return;
    state.error =
      error instanceof WorkspaceApiError
        ? error
        : new WorkspaceApiError(
            "operation_failed",
            error instanceof Error ? error.message : String(error),
            "none",
            {},
          );
    state.status = `${scope} failed.`;
    publish();
  };

  const applyWorkspace = (workspace: Workspace): void => {
    if (!state) return;
    state.workspace = workspace;
    if (workspace.files) {
      state.files = workspace.files.items;
      state.fileTotal = workspace.files.total;
      state.fileCursor = workspace.files.nextCursor;
      return;
    }
    state.files = [];
    state.fileTotal = 0;
    state.fileCursor = null;
  };

  const readFiles = async (
    run: number,
    continuation: boolean,
  ): Promise<void> => {
    if (!state?.workspace) return;
    const workspaceId = state.workspace.workspaceId;
    if (continuation && state.fileCursor === null) return;
    const value = await workspaceRequest(
      fetcher,
      workspaceFilesPath(workspaceId, continuation ? state.fileCursor : null),
    );
    if (
      !state ||
      !owns(state.photoId, run) ||
      state.workspace?.workspaceId !== workspaceId
    )
      return;
    const listing = parseWindow(value, parseWorkspaceFile);
    if (continuation) {
      const revisions = new Map(
        state.files.map((file) => [file.artifactId, file.revision]),
      );
      for (const file of listing.items) {
        if (
          revisions.has(file.artifactId) &&
          revisions.get(file.artifactId) === file.revision
        )
          continue;
        const index = state.files.findIndex(
          (existing) => existing.artifactId === file.artifactId,
        );
        if (index === -1) state.files.push(file);
        else state.files[index] = file;
      }
    } else {
      state.files = listing.items;
    }
    state.fileCursor = listing.nextCursor;
    state.fileTotal = listing.total;
    publish();
  };

  const openWorkspace = async (photoId: string, run: number): Promise<void> => {
    try {
      const value = await workspaceRequest(
        fetcher,
        `/api/photos/${encodeURIComponent(photoId)}/workspace`,
        {},
      );
      if (!owns(photoId, run) || !state) return;
      const workspace = parseWorkspace(value);
      if (workspace.photoId !== photoId)
        throw new WorkspaceApiError(
          "schema_error",
          "The service returned a Workspace for another Photo.",
          "none",
          {},
        );
      applyWorkspace(workspace);
      if (!workspace.files) await readFiles(run, false);
      if (!owns(photoId, run) || !state) return;
      state.loading = false;
      state.status = statusFor(state);
      publish();
    } catch (error) {
      if (!owns(photoId, run) || !state) return;
      state.loading = false;
      failure(error, "Opening workspace");
    }
  };

  const refreshWorkspace = async (
    photoId: string,
    run: number,
  ): Promise<void> => {
    if (!state?.workspace) return;
    const workspaceId = state.workspace.workspaceId;
    try {
      const value = await workspaceRequest(fetcher, workspacePath(workspaceId));
      if (!owns(photoId, run) || !state) return;
      const workspace = parseWorkspace(value);
      if (
        workspace.workspaceId !== workspaceId ||
        workspace.photoId !== photoId
      )
        throw new WorkspaceApiError(
          "schema_error",
          "The service returned a Workspace for another Photo.",
          "none",
          {},
        );
      applyWorkspace(workspace);
      if (!workspace.files) await readFiles(run, false);
      if (!owns(photoId, run) || !state) return;
      state.loading = false;
      state.status = statusFor(state);
      publish();
    } catch (error) {
      if (!owns(photoId, run) || !state) return;
      state.loading = false;
      failure(error, "Refreshing workspace");
    }
  };

  const inspect = async (args: JsonObject): Promise<void> => {
    if (!state?.workspace) return;
    const artifactId = argument(args, "artifactId");
    const file = artifactId
      ? state.files.find((item) => item.artifactId === artifactId)
      : undefined;
    if (!file) {
      failure(
        new WorkspaceApiError(
          "file_not_found",
          "The selected file is no longer in the Workspace list.",
          "none",
          {},
        ),
        "Selecting file",
      );
      return;
    }
    const run = generation;
    const version = ++inspectionVersion;
    state.inspectingArtifactId = file.artifactId;
    delete state.error;
    delete state.notice;
    state.status = `Reading ${file.relativePath}…`;
    publish();
    try {
      const value = await workspaceRequest(
        fetcher,
        workspaceFilePath(state.workspace.workspaceId, file.artifactId),
      );
      if (!state || !owns(state.photoId, run) || version !== inspectionVersion)
        return;
      state.inspectedFile = parseWorkspaceFile({ ...file, ...value });
      state.status = statusFor(state);
    } catch (error) {
      if (!state || !owns(state.photoId, run) || version !== inspectionVersion)
        return;
      failure(error, "Reading file");
    } finally {
      if (state && owns(state.photoId, run) && version === inspectionVersion) {
        delete state.inspectingArtifactId;
        publish();
      }
    }
  };

  const admitDownload = async (
    photoId: string,
    args: JsonObject,
  ): Promise<void> => {
    if (!state?.workspace || state.downloadingArtifactId) return;
    const artifactId = argument(args, "artifactId");
    const file =
      artifactId && state.inspectedFile?.artifactId === artifactId
        ? state.inspectedFile
        : artifactId
          ? state.files.find((item) => item.artifactId === artifactId)
          : undefined;
    if (!file) {
      failure(
        new WorkspaceApiError(
          "file_not_found",
          "The selected file is no longer in the Workspace list.",
          "none",
          {},
        ),
        "Download",
      );
      return;
    }
    if (file.state !== "ready" || !file.revision) {
      failure(
        new WorkspaceApiError(
          "file_unavailable",
          "Only a ready checked file revision can be downloaded.",
          "none",
          {},
        ),
        "Download",
      );
      return;
    }
    const run = generation;
    state.downloadingArtifactId = file.artifactId;
    delete state.error;
    delete state.notice;
    state.status = `Checking ${file.relativePath}…`;
    publish();
    try {
      const url = fileBytesUrl(state.workspace.workspaceId, file);
      await checkWorkspaceDownload(fetcher, url);
      if (!owns(photoId, run) || !state) return;
      state.download = {
        artifactId: file.artifactId,
        url,
      };
      state.status = statusFor(state);
      publish();
    } catch (error) {
      if (owns(photoId, run)) failure(error, "Download");
    } finally {
      if (state && owns(photoId, run)) {
        delete state.downloadingArtifactId;
        publish();
      }
    }
  };

  const remove = async (photoId: string, args: JsonObject): Promise<void> => {
    if (!state?.workspace) return;
    if (state.removingArtifactId) return;
    const file = state.inspectedFile;
    if (
      !file ||
      argument(args, "artifactId") !== file.artifactId ||
      argument(args, "revision") !== file.revision
    ) {
      failure(
        new WorkspaceApiError(
          "review_required",
          "Inspect the selected file revision before removing it.",
          "none",
          {},
        ),
        "Remove",
      );
      return;
    }
    if (file.state !== "ready" || !file.revision) {
      failure(
        new WorkspaceApiError(
          "file_unavailable",
          "Only a ready checked file revision can be removed.",
          "none",
          {},
        ),
        "Remove",
      );
      return;
    }
    const run = generation;
    const request =
      removalRequest?.artifactId === file.artifactId &&
      removalRequest.revision === file.revision
        ? removalRequest
        : {
            artifactId: file.artifactId,
            revision: file.revision,
            requestId: randomUuid(),
          };
    removalRequest = request;
    let settled = false;
    state.removingArtifactId = file.artifactId;
    delete state.error;
    delete state.notice;
    state.status = `Removing ${file.relativePath}…`;
    publish();
    try {
      const result = await workspaceRequest(
        fetcher,
        workspaceFilePath(state.workspace.workspaceId, file.artifactId),
        { revision: file.revision, requestId: request.requestId },
        "DELETE",
      );
      if (!owns(photoId, run) || !state) return;
      settled = true;
      removalRequest = undefined;
      if (
        result.inputArtifactId === null ||
        typeof result.inputArtifactId === "string"
      ) {
        state.workspace.inputArtifactId = result.inputArtifactId;
        if (result.inputArtifactId === null) state.workspace.inputPath = null;
      }
      if (state.inspectedFile?.artifactId === file.artifactId)
        delete state.inspectedFile;
      const pendingReclamation = result.reclamation === "pending";
      state.notice = pendingReclamation
        ? `Removed ${file.relativePath}; reclamation is pending.`
        : `Removed ${file.relativePath}.`;
      state.status = pendingReclamation
        ? "File removed; reclamation pending."
        : "File removed.";
      publish();
      await readFiles(run, false);
      if (!owns(photoId, run) || !state) return;
      state.status = pendingReclamation
        ? "File removed; reclamation pending."
        : "File removed.";
      publish();
    } catch (error) {
      if (!owns(photoId, run) || !state) return;
      if (settled) {
        failure(error, "Refreshing files");
        state.status = "File removed; file list refresh failed.";
        publish();
        return;
      }
      failure(error, "Remove");
      if (error instanceof WorkspaceApiError && error.effect === "unknown") {
        if (state.inspectedFile?.artifactId === file.artifactId)
          delete state.inspectedFile;
        state.status = "Removal outcome unknown.";
        state.notice =
          "Check the current Workspace file before removing again.";
        try {
          await readFiles(run, false);
          if (state && owns(photoId, run)) {
            state.notice =
              "Files reloaded. Check the current file before removing again.";
            if (
              !state.files.some(
                (item) =>
                  item.artifactId === request.artifactId &&
                  item.revision === request.revision,
              )
            )
              removalRequest = undefined;
          }
        } catch {
          // Keep the original unknown outcome; the list may be stale.
        }
        publish();
      }
    } finally {
      if (state && owns(photoId, run)) {
        delete state.removingArtifactId;
        publish();
      }
    }
  };

  const refresh = (photoId: string): void => {
    if (
      !alive ||
      !dependencies.isAlive() ||
      selected !== photoId ||
      !dependencies.isCurrentPhoto(photoId)
    )
      return;
    generation += 1;
    inspectionVersion += 1;
    const run = generation;
    if (!state) return;
    delete state.inspectedFile;
    delete state.inspectingArtifactId;
    delete state.removingArtifactId;
    delete state.downloadingArtifactId;
    delete state.download;
    delete state.error;
    delete state.notice;
    state.loading = true;
    state.status = "Refreshing files…";
    publish();
    if (state.workspace) void refreshWorkspace(photoId, run);
    else void openWorkspace(photoId, run);
  };

  return {
    open(photoId) {
      if (
        !alive ||
        !dependencies.isAlive() ||
        !dependencies.isCurrentPhoto(photoId)
      )
        return;
      selected = photoId;
      generation += 1;
      inspectionVersion += 1;
      removalRequest = undefined;
      const run = generation;
      state = {
        photoId,
        files: [],
        fileTotal: 0,
        fileCursor: null,
        loading: true,
        status: "Preparing workspace…",
      };
      publish();
      void openWorkspace(photoId, run);
    },
    refresh,
    async action(photoId, action, args = {}) {
      if (
        !alive ||
        !dependencies.isAlive() ||
        selected !== photoId ||
        !dependencies.isCurrentPhoto(photoId)
      )
        return;
      if (action === "refresh") {
        refresh(photoId);
        return;
      }
      if (action === "download") {
        await admitDownload(photoId, args);
        return;
      }
      if (action === "files-more") {
        if (!state || state.loading || state.fileCursor === null) return;
        const run = generation;
        state.loading = true;
        publish();
        try {
          await readFiles(run, true);
        } catch (error) {
          if (owns(photoId, run)) failure(error, "Loading more files");
        } finally {
          if (state && owns(photoId, run)) {
            state.loading = false;
            publish();
          }
        }
        return;
      }
      if (action === "inspect") {
        await inspect(args);
        return;
      }
      if (action === "remove") {
        await remove(photoId, args);
      }
    },
    leave() {
      selected = undefined;
      generation += 1;
      inspectionVersion += 1;
      removalRequest = undefined;
      state = undefined;
    },
    dispose() {
      alive = false;
      generation += 1;
      inspectionVersion += 1;
      removalRequest = undefined;
      state = undefined;
    },
  };
}
