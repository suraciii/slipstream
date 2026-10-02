import type { PhotoSummary } from "../api/contracts.js";
import type { EditorStage } from "../ui/library-browser-view.js";
import type { OutputTarget } from "./workspace-output-controller.js";

export type EditorControllerDependencies = Readonly<{
  isAlive: () => boolean;
  isCurrentPhoto: (photoId: string) => boolean;
  currentPhoto: () => PhotoSummary | undefined;
  /// The Library's current scan phase in the summary's words, or "" while
  /// no scan is running. A `Checking source…` wait names it so the
  /// Photographer can see what the Library is doing.
  libraryPhase?: () => string;
}>;

export type EditorController = Readonly<{
  open: (photoId: string) => void;
  refresh: (photoId: string) => void;
  commitExposure: (photoId: string, exposureEv: number) => void;
  commitWhiteBalance: (
    photoId: string,
    action:
      | Readonly<{ kind: "mode"; mode: string }>
      | Readonly<{ kind: "temperature"; temperatureKelvin: number }>
      | Readonly<{ kind: "tint"; tintMilli: number }>,
  ) => void;
  stepHistory: (
    photoId: string,
    operation:
      | "undo"
      | "redo"
      | "reset"
      | "resetExposure"
      | "resetWhiteBalance",
  ) => void;
  requestPreview: (photoId: string) => void;
  applyStage: (photoId: string, stage: EditorStage) => void;
  setComparison: (photoId: string, pressed: boolean) => void;
  useSaved: (photoId: string) => void;
  reapplyLocal: (photoId: string) => void;
  discardDraft: (photoId: string) => void;
  rebind: (photoId: string) => void;
  createProxy: (photoId: string) => void;
  removeProxy: (photoId: string) => void;
  /// Starts composing this Photo's composable Processing Recipe.
  compose: (photoId: string) => void;
  composableAddStep: (photoId: string, module: string) => void;
  composableRemoveStep: (photoId: string, stepId: string) => void;
  composableSelectStep: (photoId: string, stepId: string) => void;
  composableEditStep: (photoId: string, stepId: string) => void;
  composableParameters: (photoId: string, text: string) => void;
  composableSchema: (photoId: string, schemaVersion: string) => void;
  composableInput: (
    photoId: string,
    choice: "original" | "artifact",
    artifactId?: string,
  ) => void;
  composableArtifactFetch: (photoId: string, artifactId: string) => void;
  composableSave: (photoId: string) => void;
  composableDiscard: (photoId: string) => void;
  /// Reconciles the live composable Export through its durable work record.
  checkProcessingExport: (photoId: string) => void;
  /// Downloads one retained Processing Artifact's validated bytes.
  downloadArtifact: (photoId: string, artifactId: string) => void;
  /// Selects one retained artifact as a step's explicit input binding.
  useArtifactInput: (photoId: string, artifactId: string) => void;
  submitXmp: (photoId: string) => void;
  downloadXmp: (photoId: string) => void;
  submitExport: (photoId: string, target?: OutputTarget) => void;
  cancelExport: (photoId: string, target?: OutputTarget) => void;
  retryExport: (photoId: string, target?: OutputTarget) => void;
  downloadExport: (photoId: string, target?: OutputTarget) => void;
  leave: () => void;
}>;
