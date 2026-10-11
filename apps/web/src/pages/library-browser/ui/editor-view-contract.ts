import type { WorkspaceEditModel } from "../model/workspace-edit-owner.js";
import type { JsonObject } from "../api/workspace-edit.js";
export type EditorViewModel = WorkspaceEditModel;
export type EditorIntent =
  | Readonly<{ kind: "editor-open" | "editor-refresh"; photoId: string }>
  | Readonly<{
      kind: "editor-workspace-action";
      photoId: string;
      action: string;
      arguments: JsonObject;
    }>;
