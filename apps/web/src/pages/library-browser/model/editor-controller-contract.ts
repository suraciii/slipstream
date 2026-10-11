import type { PhotoSummary } from "../api/contracts.js";
import type { JsonObject } from "../api/workspace-edit.js";
export type EditorControllerDependencies = Readonly<{
  isAlive: () => boolean;
  isCurrentPhoto: (photoId: string) => boolean;
  currentPhoto: () => PhotoSummary | undefined;
  libraryPhase?: () => string;
}>;
export type EditorController = Readonly<{
  open(photoId: string): void;
  refresh(photoId: string): void;
  act(photoId: string, action: string, args: JsonObject): void;
  leave(): void;
}>;
