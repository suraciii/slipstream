import type { WorkspaceController } from "./workspace-controller-contract.js";
import type { WorkspaceIntent } from "./workspace-intent-contract.js";

export function routeWorkspaceIntent(
  workspace: WorkspaceController,
  intent: Readonly<{ kind: string }>,
): boolean {
  const workspaceIntent = intent as WorkspaceIntent;
  if (workspaceIntent.kind === "workspace-open") {
    workspace.open(workspaceIntent.photoId);
    return true;
  }
  if (workspaceIntent.kind === "workspace-action") {
    void workspace.action(
      workspaceIntent.photoId,
      workspaceIntent.action,
      workspaceIntent.arguments ?? {},
    );
    return true;
  }
  return false;
}
