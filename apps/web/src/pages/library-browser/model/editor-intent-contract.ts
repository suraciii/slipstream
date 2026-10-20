import type { JsonObject } from "../api/workspace-edit.js";

/// The editor intents the router routes: the open, refresh, and workspace
/// actions the editor controller admits. The page's intent union includes
/// these kinds; the router names only the editor ones, so the model never
/// depends on the whole browser intent union.

export type EditorIntent =
  | Readonly<{ kind: "editor-open" | "editor-refresh"; photoId: string }>
  | Readonly<{
      kind: "editor-workspace-action";
      photoId: string;
      action: string;
      arguments: JsonObject;
    }>;
