import type { JsonObject } from "../api/workspace.js";

/// One message from the Workspace surface. Every message names the Photo it
/// belongs to, so a response for a Photo the page has left can never take over
/// the view.
export type WorkspaceIntent =
  | Readonly<{ kind: "workspace-open"; photoId: string }>
  | Readonly<{
      kind: "workspace-action";
      photoId: string;
      action: string;
      arguments?: JsonObject;
    }>;
