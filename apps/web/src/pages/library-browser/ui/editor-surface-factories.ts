import type { BrowserFetch } from "../api/browser-fetch.js";
import type { EditorSurfaceFactories } from "../model/editor-controller-contract.js";
import { createWorkspaceCatalogSurface } from "./workspace-catalog-surface.js";
import { createWorkspaceFilmSurface } from "./workspace-film-surface.js";

/// UI owns construction of the DOM-backed editor surfaces. The model editor
/// receives these factories through its composition boundary and never imports
/// a UI surface implementation.
export function createEditorSurfaceFactories(
  fetcher: BrowserFetch,
): EditorSurfaceFactories {
  return {
    createFilm(photoId, onFilesChanged) {
      return createWorkspaceFilmSurface(fetcher, photoId, onFilesChanged);
    },
    createCatalog(photoId, onMutation, beforeMutation) {
      return createWorkspaceCatalogSurface(
        fetcher,
        photoId,
        onMutation,
        beforeMutation,
      );
    },
  };
}
