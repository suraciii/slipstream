import type { BrowserFetch } from "../api/browser-fetch.js";
import type { PhotoOwner } from "../model/photo-owner.js";
import {
  createEditorController,
  type EditorController,
} from "../model/editor-controller.js";
import type {
  EditorControllerDependencies,
  EditorViewPort,
} from "../model/editor-controller-contract.js";
import { createEditorSurfaceFactories } from "./editor-surface-factories.js";

/// The page composition boundary supplies the live Photo and application
/// lifetime to the model editor while the UI owns its DOM surface factories.
export function createPageEditorController(
  fetcher: BrowserFetch,
  view: EditorViewPort,
  applicationAlive: () => boolean,
  photoOwner: PhotoOwner,
  libraryPhase: () => string,
): EditorController {
  const dependencies: EditorControllerDependencies = {
    isAlive: applicationAlive,
    isCurrentPhoto: (photoId) =>
      photoOwner.isCurrent(photoOwner.authority) &&
      photoOwner.current?.id === photoId,
    currentPhoto: () => photoOwner.current,
    libraryPhase,
  };
  return createEditorController(
    fetcher,
    view,
    dependencies,
    createEditorSurfaceFactories(fetcher),
  );
}
