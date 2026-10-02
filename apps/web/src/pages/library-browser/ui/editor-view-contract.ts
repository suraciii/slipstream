import type {
  EditSourceKind,
  EditSourceReadiness,
  EditorWhiteBalancePresentation,
} from "../model/photo-editor.js";
import type {
  OutputTarget,
  WorkspaceOutputsView,
} from "../model/workspace-output-controller.js";
import type { EditorProxyViewModel } from "./editor-proxy-view-model.js";

/// One Edit workspace presents the current edit, Film, or Original reference.
/// These are presentation states, not stage tabs.
export type EditorStage = "camera" | "develop" | "film";

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
  stage: EditorStage;
  /// What the presented image actually is, in plain language.
  stageNote: string;
  /// Why Film is unavailable, when it is.
  filmReason: string;
  editSourceReadiness: EditSourceReadiness;
  editSourceKind: EditSourceKind;
  /// The Source support fact line: the readiness word, the Library's scan
  /// phase while the source is being checked, and proxy provenance.
  sourceFactNote: string;
  /// The Processing axis: the deployment's engine capability, separately
  /// from this Photo's source.
  processingReadiness: "checking" | "ready" | "waiting" | "unavailable";
  /// The Edit Preview axis for the chosen stage, or `null` when no Edit
  /// Preview is described on that stage.
  previewState: "pending" | "ready" | "stale" | "failed" | null;
  processingAvailable: boolean;
  /// Why the deployment cannot execute development work, when it cannot. An
  /// unavailable deployment is explained instead of attempted.
  capabilityNote: string;
  exposureEv: number;
  savedExposureEv: number;
  baselineExposureEv: number;
  exposureMinimumEv: number;
  exposureMaximumEv: number;
  exposureStepEv: number;
  /// The white-balance intent in force, the modes a Photographer may select,
  /// and why an adjustable mode is not offered.
  whiteBalance: EditorWhiteBalancePresentation;
  proxy?: EditorProxyViewModel;
  canEdit: boolean;
  canPreview: boolean;
  previewing: boolean;
  /// What the presented Edit Preview is, or why none is presented.
  previewNote: string;
  /// True while the retained image is older than the current settings.
  previewStale: boolean;
  saving: boolean;
  dirty: boolean;
  canUndo: boolean;
  canRedo: boolean;
  comparing: boolean;
  conflict: Readonly<{ message: string }> | null;
  draftNote: string;
  export: EditorExportViewModel;
  /// The caller-controlled composable Processing Step surface.
  composable: EditorComposableViewModel;
  /// Why the fixed two-control editor is read-only, or "" when it edits.
  /// Composable mode never writes the two-control recipe.
  controlsReadonlyNote: string;
  outputs: WorkspaceOutputsView;
  status: string;
  /// The session's own detailed wording behind the compact status, presented
  /// only under the optional Details affordance.
  statusDetail: string;
}>;

export type EditorIntent =
  | Readonly<{ kind: "editor-open" | "editor-refresh"; photoId: string }>
  | Readonly<{ kind: "editor-exposure"; photoId: string; exposureEv: number }>
  | Readonly<{
      kind: "editor-white-balance-mode";
      photoId: string;
      mode: string;
    }>
  | Readonly<{
      kind: "editor-temperature";
      photoId: string;
      temperatureKelvin: number;
    }>
  | Readonly<{ kind: "editor-tint"; photoId: string; tintMilli: number }>
  | Readonly<{ kind: "editor-stage"; photoId: string; stage: EditorStage }>
  | Readonly<{ kind: "editor-compare"; photoId: string; pressed: boolean }>
  | Readonly<{ kind: "editor-compose"; photoId: string }>
  | Readonly<{
      kind: "editor-composable-add";
      photoId: string;
      module: string;
    }>
  | Readonly<{
      kind: "editor-composable-remove";
      photoId: string;
      stepId: string;
    }>
  | Readonly<{
      kind: "editor-composable-select";
      photoId: string;
      stepId: string;
    }>
  | Readonly<{
      kind: "editor-composable-edit";
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
      kind: "editor-composable-artifact-fetch";
      photoId: string;
      artifactId: string;
    }>
  | Readonly<{ kind: "editor-composable-save"; photoId: string }>
  | Readonly<{ kind: "editor-composable-discard"; photoId: string }>
  | Readonly<{
      kind: "editor-processing-export-check";
      photoId: string;
    }>
  | Readonly<{
      kind: "editor-artifact-download";
      photoId: string;
      artifactId: string;
    }>
  | Readonly<{
      kind: "editor-artifact-use";
      photoId: string;
      artifactId: string;
    }>
  | Readonly<{
      kind:
        | "editor-undo"
        | "editor-redo"
        | "editor-reset"
        | "editor-reset-exposure"
        | "editor-reset-white-balance"
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
        | "editor-export-submit"
        | "editor-export-cancel"
        | "editor-export-retry"
        | "editor-export-download";
      photoId: string;
      target?: OutputTarget;
    }>;
