import type { PhotoSummary } from "../api/contracts.js";
import type {
  EditInstance,
  EditView,
  JsonObject,
} from "../api/workspace-edit.js";
import type { WorkspaceFilmFile } from "../api/workspace-film.js";
import type { WorkspaceEditModel } from "./workspace-edit-model.js";
export type EditorViewModel = WorkspaceEditModel;
export type EditorViewPort = Readonly<{
  editorVisible: () => boolean;
  renderEditor: (model: EditorViewModel) => void;
  presentEditorPreview: (url: string) => void;
  clearEditorPreview: () => void;
}>;
export type EditorViewDependencies = Readonly<{
  libraryPhase: () => string;
}>;
export type EditorFilmSurface = Readonly<{
  readonly element: HTMLElement;
  open(file: WorkspaceFilmFile): Promise<void>;
  dispose(): void;
}>;
export type EditorCatalogSurface = Readonly<{
  readonly element: HTMLElement;
  bind(
    edit: EditView,
    instances: EditInstance[],
    details: Record<string, JsonObject>,
    resourceKinds: JsonObject[],
  ): void;
  dispose(): void;
}>;
export type EditorSurfaceFactories = Readonly<{
  createFilm(photoId: string, onFilesChanged: () => void): EditorFilmSurface;
  createCatalog(
    photoId: string,
    onMutation: () => Promise<void>,
    beforeMutation: () => Promise<boolean>,
  ): EditorCatalogSurface;
}>;
export type EditorControllerDependencies = Readonly<{
  isAlive: () => boolean;
  isCurrentPhoto: (photoId: string) => boolean;
  currentPhoto: () => PhotoSummary | undefined;
  /** The Library's own progress, an independent fact of this Photo. */
  libraryPhase: () => string;
}>;
export type EditorController = Readonly<{
  open(photoId: string): void;
  refresh(photoId: string): void;
  referenceChanged(photoId: string): void;
  act(photoId: string, action: string, args: JsonObject): void;
  leave(): void;
}>;
