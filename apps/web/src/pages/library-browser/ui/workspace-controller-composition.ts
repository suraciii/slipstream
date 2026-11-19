import type { BrowserFetch } from "../api/browser-fetch.js";
import type { PhotoOwner } from "../model/photo-owner.js";
import { createWorkspaceController } from "../model/workspace-controller.js";
import type {
  WorkspaceController,
  WorkspaceControllerDependencies,
  WorkspaceViewPort,
} from "../model/workspace-controller-contract.js";

/// The page composition boundary supplies the live Photo and application
/// lifetime to the Workspace file view.
export function createPageWorkspaceController(
  fetcher: BrowserFetch,
  view: WorkspaceViewPort,
  applicationAlive: () => boolean,
  photoOwner: PhotoOwner,
): WorkspaceController {
  const dependencies: WorkspaceControllerDependencies = {
    isAlive: applicationAlive,
    isCurrentPhoto: (photoId) =>
      photoOwner.isCurrent(photoOwner.authority) &&
      photoOwner.current?.id === photoId,
  };
  return createWorkspaceController(fetcher, view, dependencies);
}
