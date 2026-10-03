import type { WorkspaceOutputsView } from "../model/workspace-output-controller.js";
import type { EditorProxyViewModel } from "./editor-proxy-view-model.js";
export type {
  EditorComposableViewModel,
  EditorExportViewModel,
} from "./composable-editor-surface.js";
import type {
  EditorComposableViewModel,
  EditorExportViewModel,
} from "./composable-editor-surface.js";

export type EditorViewModel = Readonly<{
  photoId: string;
  loading: boolean;
  cameraReference: boolean;
  canCompare: boolean;
  comparing: boolean;
  provenanceNote: string;
  sourceFactNote: string;
  processingReadiness: "checking" | "ready" | "waiting" | "unavailable";
  previewState: "pending" | "ready" | "stale" | "failed" | null;
  proxy?: EditorProxyViewModel;
  canPreview: boolean;
  previewing: boolean;
  previewNote: string;
  previewStale: boolean;
  saving: boolean;
  dirty: boolean;
  canUndo: boolean;
  canRedo: boolean;
  conflict: Readonly<{ message: string }> | null;
  mutationsBlocked: boolean;
  rebindAvailable: boolean;
  draftNote: string;
  export: EditorExportViewModel;
  composable: EditorComposableViewModel;
  outputs: WorkspaceOutputsView;
  status: string;
  statusDetail: string;
}>;

export type EditorIntent =
  | Readonly<{ kind: "editor-open" | "editor-refresh"; photoId: string }>
  | Readonly<{
      kind: "editor-camera-reference";
      photoId: string;
      pressed: boolean;
    }>
  | Readonly<{ kind: "editor-compare"; photoId: string; pressed: boolean }>
  | Readonly<{ kind: "editor-compose"; photoId: string }>
  | Readonly<{
      kind: "editor-composable-add";
      photoId: string;
      module: string;
      artifactId?: string;
    }>
  | Readonly<{
      kind:
        | "editor-composable-remove"
        | "editor-composable-select"
        | "editor-composable-edit";
      photoId: string;
      stepId: string;
    }>
  | Readonly<{
      kind: "editor-composable-parameters";
      photoId: string;
      text: string;
    }>
  | Readonly<{
      kind: "editor-composable-schema";
      photoId: string;
      schemaVersion: string;
    }>
  | Readonly<{
      kind: "editor-composable-input";
      photoId: string;
      choice: "original" | "artifact";
      artifactId?: string;
    }>
  | Readonly<{
      kind:
        | "editor-composable-artifact-fetch"
        | "editor-artifact-download"
        | "editor-artifact-use";
      photoId: string;
      artifactId: string;
    }>
  | Readonly<{
      kind: "editor-historical-download";
      photoId: string;
      exportId: string;
    }>
  | Readonly<{
      kind:
        | "editor-composable-save"
        | "editor-composable-discard"
        | "editor-undo"
        | "editor-redo"
        | "editor-preview"
        | "editor-rebind"
        | "editor-use-saved"
        | "editor-reapply"
        | "editor-discard-draft"
        | "editor-proxy-create"
        | "editor-proxy-remove"
        | "editor-xmp-submit"
        | "editor-xmp-download";
      photoId: string;
    }>
  | Readonly<{
      kind:
        | "editor-processing-export-check"
        | "editor-export-submit"
        | "editor-export-cancel"
        | "editor-export-retry"
        | "editor-export-download";
      photoId: string;
      requestId?: string;
    }>;
