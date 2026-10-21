import type { BrowserFetch } from "../api/browser-fetch.js";
import type { WorkspaceEditModel } from "./workspace-edit-owner.js";
import {
  parseFile,
  parseWindow,
  workspacePath,
  workspaceRequest,
} from "../api/workspace-edit.js";

export async function refreshWorkspaceFiles(
  fetcher: BrowserFetch,
  model: WorkspaceEditModel,
  continuation: boolean,
): Promise<void> {
  if (!model.workspace) return;
  const result = await workspaceRequest(
    fetcher,
    continuation && model.fileCursor
      ? `/api/workspace-files?cursor=${encodeURIComponent(model.fileCursor)}`
      : `${workspacePath(model.workspace.workspaceId)}/files?limit=60`,
  );
  const window = parseWindow(result, parseFile);
  model.files = continuation ? [...model.files, ...window.items] : window.items;
  model.fileCursor = window.nextCursor;
  if (model.inspectedFile) {
    const reviewed = model.files.find(
      (file) =>
        file.artifactId === model.inspectedFile?.artifactId &&
        file.revision === model.inspectedFile.revision,
    );
    if (reviewed) model.inspectedFile = reviewed;
    else delete model.inspectedFile;
  }
}
