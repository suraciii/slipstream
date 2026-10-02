import type { EditorController } from "./editor-controller-contract.js";
import type { LibraryBrowserIntent } from "../ui/library-browser-view.js";

export function routeEditorIntent(
  editor: EditorController,
  intent: LibraryBrowserIntent,
): boolean {
  switch (intent.kind) {
    // The Edit workspace owns one Photo at a time. Every editor intent
    // re-checks the owner, so a gesture or response for a Photo the
    // Photographer already left can never write or publish for it.
    case "editor-open":
      editor.open(intent.photoId);
      return true;
    case "editor-refresh":
      editor.refresh(intent.photoId);
      return true;
    case "editor-exposure":
      editor.commitExposure(intent.photoId, intent.exposureEv);
      return true;
    case "editor-white-balance-mode":
      editor.commitWhiteBalance(intent.photoId, {
        kind: "mode",
        mode: intent.mode,
      });
      return true;
    case "editor-temperature":
      editor.commitWhiteBalance(intent.photoId, {
        kind: "temperature",
        temperatureKelvin: intent.temperatureKelvin,
      });
      return true;
    case "editor-tint":
      editor.commitWhiteBalance(intent.photoId, {
        kind: "tint",
        tintMilli: intent.tintMilli,
      });
      return true;
    case "editor-undo":
      editor.stepHistory(intent.photoId, "undo");
      return true;
    case "editor-redo":
      editor.stepHistory(intent.photoId, "redo");
      return true;
    case "editor-reset":
      editor.stepHistory(intent.photoId, "reset");
      return true;
    case "editor-reset-exposure":
      editor.stepHistory(intent.photoId, "resetExposure");
      return true;
    case "editor-reset-white-balance":
      editor.stepHistory(intent.photoId, "resetWhiteBalance");
      return true;
    case "editor-preview":
      void editor.requestPreview(intent.photoId);
      return true;
    case "editor-stage":
      editor.applyStage(intent.photoId, intent.stage);
      return true;
    case "editor-compare":
      editor.setComparison(intent.photoId, intent.pressed);
      return true;
    case "editor-use-saved":
      void editor.useSaved(intent.photoId);
      return true;
    case "editor-reapply":
      editor.reapplyLocal(intent.photoId);
      return true;
    case "editor-discard-draft":
      editor.discardDraft(intent.photoId);
      return true;
    case "editor-proxy-create":
      editor.createProxy(intent.photoId);
      return true;
    case "editor-proxy-remove":
      editor.removeProxy(intent.photoId);
      return true;
    case "editor-rebind":
      void editor.rebind(intent.photoId);
      return true;
    case "editor-compose":
      editor.compose(intent.photoId);
      return true;
    case "editor-composable-add":
      editor.composableAddStep(intent.photoId, intent.module);
      return true;
    case "editor-composable-remove":
      editor.composableRemoveStep(intent.photoId, intent.stepId);
      return true;
    case "editor-composable-select":
      editor.composableSelectStep(intent.photoId, intent.stepId);
      return true;
    case "editor-composable-edit":
      editor.composableEditStep(intent.photoId, intent.stepId);
      return true;
    case "editor-composable-parameters":
      editor.composableParameters(intent.photoId, intent.text);
      return true;
    case "editor-composable-schema":
      editor.composableSchema(intent.photoId, intent.schemaVersion);
      return true;
    case "editor-composable-input":
      editor.composableInput(
        intent.photoId,
        intent.choice,
        intent.artifactId ?? "",
      );
      return true;
    case "editor-composable-artifact-fetch":
      editor.composableArtifactFetch(intent.photoId, intent.artifactId);
      return true;
    case "editor-composable-save":
      editor.composableSave(intent.photoId);
      return true;
    case "editor-composable-discard":
      editor.composableDiscard(intent.photoId);
      return true;
    case "editor-processing-export-check":
      editor.checkProcessingExport(intent.photoId);
      return true;
    case "editor-artifact-download":
      editor.downloadArtifact(intent.photoId, intent.artifactId);
      return true;
    case "editor-artifact-use":
      editor.useArtifactInput(intent.photoId, intent.artifactId);
      return true;
    case "editor-export-submit":
      void editor.submitExport(intent.photoId, intent.target);
      return true;
    case "editor-export-cancel":
      void editor.cancelExport(intent.photoId, intent.target);
      return true;
    case "editor-export-retry":
      void editor.retryExport(intent.photoId, intent.target);
      return true;
    case "editor-export-download":
      void editor.downloadExport(intent.photoId, intent.target);
      return true;
    case "editor-xmp-submit":
      editor.submitXmp(intent.photoId);
      return true;
    case "editor-xmp-download":
      editor.downloadXmp(intent.photoId);
      return true;
    default:
      return false;
  }
}
