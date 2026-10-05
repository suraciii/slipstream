import type { PhotoSummary } from "../api/contracts.js";

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
  setExposure: (photoId: string, value: number) => void;
  resetExposure: (photoId: string) => void;
  stepHistory: (photoId: string, operation: "undo" | "redo") => void;
  requestPreview: (photoId: string) => void;
  setCameraReference: (photoId: string, pressed: boolean) => void;
  setComparison: (photoId: string, pressed: boolean) => void;
  useSaved: (photoId: string) => void;
  reapplyLocal: (photoId: string) => void;
  discardDraft: (photoId: string) => void;
  rebind: (photoId: string) => void;
  createProxy: (photoId: string) => void;
  removeProxy: (photoId: string) => void;
  /// Starts composing this Photo's composable Processing Recipe.
  compose: (photoId: string) => void;
  composableAddStep: (
    photoId: string,
    module: string,
    artifactId?: string,
  ) => void;
  composableRemoveStep: (photoId: string, stepId: string) => void;
  composableSelectStep: (photoId: string, stepId: string) => void;
  composableEditStep: (photoId: string, stepId: string) => void;
  composableParameters: (photoId: string, text: string) => void;
  composableAutomatic: (
    photoId: string,
    adjustment: {
      stepId: string;
      operation: string;
      multiPriority: number;
      instruction: Record<string, unknown>;
    },
  ) => void;
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
  checkProcessingExport: (photoId: string, requestId?: string) => void;
  /// Downloads one retained Processing Artifact's validated bytes.
  downloadArtifact: (photoId: string, artifactId: string) => void;
  downloadHistorical: (photoId: string, exportId: string) => void;
  /// Selects one retained artifact as a step's explicit input binding.
  useArtifactInput: (photoId: string, artifactId: string) => void;
  submitXmp: (photoId: string) => void;
  downloadXmp: (photoId: string) => void;
  submitExport: (photoId: string) => void;
  cancelExport: (photoId: string, requestId?: string) => void;
  retryExport: (photoId: string, requestId?: string) => void;
  downloadExport: (photoId: string) => void;
  leave: () => void;
}>;
