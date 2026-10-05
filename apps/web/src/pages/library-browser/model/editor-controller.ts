import { createWorkspaceOutputController } from "./workspace-output-controller.js";
import { createEditorComposableRecipe } from "./editor-composable-recipe.js";
import { createEditorComposablePreview } from "./editor-composable-preview.js";
import { createEditorProxyController } from "./editor-proxy-controller.js";
import { createEditorProcessingExport } from "./editor-processing-export.js";
import {
  describePreviewRefusal,
  describeEditRefusal,
} from "./editor-refusals.js";
import type { BrowserFetch } from "./access-session.js";
import type { LibraryBrowserView } from "../ui/library-browser-view.js";
import type {
  EditorController,
  EditorControllerDependencies,
} from "./editor-controller-contract.js";
export type { EditorController } from "./editor-controller-contract.js";

export function createEditorController(
  fetcher: BrowserFetch,
  view: LibraryBrowserView,
  dependencies: EditorControllerDependencies,
): EditorController {
  let photoId: string | undefined;
  let cameraReference = false;
  let comparisonRequested = false;
  const baselineReady = (): boolean =>
    Boolean(
      preview.current.url &&
        baseline.current.url &&
        preview.current.outcome === "ready" &&
        baseline.current.outcome === "ready" &&
        !preview.current.stale &&
        !baseline.current.stale &&
        preview.current.comparisonIdentity &&
        preview.current.comparisonIdentity ===
          baseline.current.comparisonIdentity &&
        !composable.dirty(),
    );
  const presentSelected = (): void => {
    if (cameraReference) return;
    const url =
      comparisonRequested && baselineReady()
        ? baseline.current.url
        : preview.current.url;
    if (url) view.presentEditorPreview(url);
  };
  const owns = (id: string): boolean =>
    photoId === id &&
    dependencies.isAlive() &&
    view.editorVisible() &&
    dependencies.isCurrentPhoto(id) &&
    dependencies.currentPhoto()?.id === id;
  const clearPreview = (): void => {
    comparisonRequested = false;
    preview.clear();
    baseline.clear();
    view.clearEditorPreview();
  };
  const requestPreview = async (id: string): Promise<void> => {
    if (!owns(id) || cameraReference || composable.readPending) return;
    await preview.requestCurrent(id);
  };
  const composable = createEditorComposableRecipe(fetcher, {
    ...dependencies,
    editorOwnsPhoto: owns,
    renderEditor: () => render(),
    previewSelection: () => preview.selection,
    clearEditorPreview: clearPreview,
    requestEditorPreview: requestPreview,
    markEditorPreviewStale: () => {
      comparisonRequested = false;
      preview.markStale();
      baseline.markStale();
      presentSelected();
    },
    describeEditRefusal,
  });
  const preview = createEditorComposablePreview(fetcher, {
    cameraReference: () => cameraReference,
    read: () => composable.read,
    isDirty: composable.dirty,
    editorOwnsPhoto: owns,
    renderEditor: () => render(),
    present: () => presentSelected(),
    clearPresented: () => view.clearEditorPreview(),
    describePreviewRefusal,
  });
  const baseline = createEditorComposablePreview(fetcher, {
    comparison: "baseline",
    cameraReference: () => cameraReference,
    read: () => composable.read,
    isDirty: composable.dirty,
    editorOwnsPhoto: owns,
    renderEditor: () => render(),
    present: () => presentSelected(),
    clearPresented: () => presentSelected(),
    describePreviewRefusal,
  });
  const proxy = createEditorProxyController(fetcher, {
    sourceRevision: (id) =>
      owns(id) && composable.read?.sourceAvailable !== false
        ? (composable.read?.currentSourceRevision ??
          composable.read?.sourceRevision)
        : undefined,
    editorOwnsPhoto: owns,
    renderEditor: () => render(),
    refreshSource: (id) => {
      void composable.loadComposableRecipe(id);
    },
  });
  const outputs = createWorkspaceOutputController(fetcher, {
    owns,
    render: () => render(),
    facts: (id) => {
      if (!owns(id)) return undefined;
      const state = composable.state;
      const recipe = composable.read?.recipe;
      return {
        recipeRevision: recipe?.revision ?? null,
        sourceRevision: recipe?.sourceRevision ?? null,
        stepId: recipe?.currentStepId ?? null,
        saving: state?.saving ?? false,
        dirty: composable.dirty() || exports.unresolved(id),
        conflict: state?.conflict ?? false,
      };
    },
  });
  const exports = createEditorProcessingExport(fetcher, composable, {
    editorOwnsPhoto: owns,
    renderEditor: () => render(),
    processingAvailable: () => {
      const target = composable.target();
      return (
        target.kind === "step" &&
        composable.modules.some(
          (module) =>
            module.id.name === target.step.module &&
            module.availability.state === "ready",
        ) &&
        !(photoId && outputs.unresolved(photoId))
      );
    },
    describeEditRefusal,
  });
  const render = (): void => {
    if (!photoId || !owns(photoId)) return;
    const recipeView = composable.view();
    const state = composable.state;
    const target = composable.target();
    const current = preview.current;
    const selected = target.kind === "step" ? target.step : undefined;
    const module = selected
      ? composable.modules.find((entry) => entry.id.name === selected.module)
      : undefined;
    const provenanceNote = cameraReference
      ? "Original reference: the camera Preview of this Photo."
      : target.kind === "unreadable"
        ? "The Edit State could not be read. Reload to check again."
        : !selected
          ? "No Processing Step is selected; no processing result is shown."
        : `Edit Preview from ${selected.module}.${composable.dirty() ? " Local changes are not yet confirmed." : ""}`;
    view.renderEditor({
      photoId,
      loading: composable.readPending,
      cameraReference,
      canCompare: Boolean(
        current.url &&
          current.outcome === "ready" &&
          !current.stale &&
          !composable.dirty() &&
          !cameraReference,
      ),
      comparing: comparisonRequested && baselineReady() && !cameraReference,
      provenanceNote,
      sourceFactNote:
        composable.read?.sourceAvailable === false
          ? "The Original source is unavailable."
          : composable.read?.currentSourceRevision &&
              composable.read.currentSourceRevision !==
                composable.read.recipe?.sourceRevision
            ? "The Original has changed since this Edit State was saved."
            : composable.read
              ? "Original source revision checked."
              : "Checking source…",
      processingReadiness: !selected
        ? "unavailable"
        : !module
          ? "checking"
          : module.availability.state === "ready"
            ? "ready"
            : "unavailable",
      previewState: cameraReference
        ? null
        : current.busy || current.pending
          ? "pending"
          : current.outcome === "failed" || current.refused
            ? "failed"
            : current.url
              ? current.stale
                ? "stale"
                : "ready"
              : null,
      proxy: proxy.view(photoId),
      canPreview: Boolean(selected) && !composable.dirty() && !cameraReference,
      previewing: current.busy,
      previewNote: comparisonRequested
        ? baseline.current.outcome === "ready" && !baselineReady()
          ? "The baseline does not match this current Preview's processing provenance, so the current Preview remains shown."
          : baseline.current.note ||
            "Preparing the selected step's baseline comparison…"
        : current.note,
      previewStale: current.stale,
      saving: state?.saving ?? false,
      dirty: composable.dirty(),
      mutationsBlocked: Boolean(
        state?.uncertain ||
          state?.conflict ||
          exports.unresolved(photoId) ||
          outputs.unresolved(photoId),
      ),
      rebindAvailable: Boolean(
        composable.read?.recipe &&
          composable.read.sourceAvailable !== false &&
          composable.read.currentSourceRevision &&
          composable.read.currentSourceRevision !==
            composable.read.recipe.sourceRevision,
      ),
      canUndo:
        Boolean(state?.undo.length) &&
        !state?.uncertain &&
        !state?.conflict &&
        !exports.unresolved(photoId) &&
        !outputs.unresolved(photoId),
      canRedo:
        Boolean(state?.redo.length) &&
        !state?.uncertain &&
        !state?.conflict &&
        !exports.unresolved(photoId) &&
        !outputs.unresolved(photoId),
      conflict: state?.conflict ? { message: state.note } : null,
      draftNote: state
        ? `${state.recovered ? state.note : ""}${state.recoveryAvailable ? "" : " Local settings remain in this session; browser reload recovery is unavailable."}`
        : "",
      export: exports.view(),
      composable: recipeView,
      outputs: outputs.view(photoId),
      status: recipeView.note,
      statusDetail: state?.uncertain
        ? "The save outcome is unknown. Check the saved edit before another action."
        : "",
    });
  };
  const open = (id: string): void => {
    if (!id || !dependencies.isCurrentPhoto(id)) return;
    clearPreview();
    proxy.leave();
    outputs.leave();
    composable.reset();
    exports.reset();
    photoId = id;
    cameraReference = false;
    render();
    void composable.loadComposableRecipe(id);
    void composable.loadProcessingModules(id);
    void proxy.read(id);
    void exports.load(id);
    outputs.open(id);
  };
  const scoped =
    <A extends unknown[]>(action: (id: string, ...args: A) => unknown) =>
    (id: string, ...args: A): void => {
      if (owns(id)) void action(id, ...args);
    };
  const mutate = <A extends unknown[]>(
    action: (id: string, ...args: A) => unknown,
  ) =>
    scoped((id: string, ...args: A) => {
      const state = composable.state;
      if (
        state?.uncertain ||
        state?.conflict ||
        exports.unresolved(id) ||
        outputs.unresolved(id)
      )
        return;
      return action(id, ...args);
    });
  return {
    open,
    refresh: scoped((id) => {
      if (composable.state?.uncertain) void composable.saveEditorComposable(id);
      else void composable.loadComposableRecipe(id);
      void composable.loadProcessingModules(id);
      void proxy.read(id);
      void exports.load(id);
      void outputs.refresh(id);
    }),
    stepHistory: mutate((id, operation: "undo" | "redo") => {
      comparisonRequested = false;
      preview.markStale();
      baseline.markStale();
      presentSelected();
      if (operation === "undo") void composable.undo(id);
      else void composable.redo(id);
    }),
    requestPreview: scoped(requestPreview),
    setCameraReference: scoped((id, pressed: boolean) => {
      cameraReference = pressed;
      render();
      if (pressed) view.clearEditorPreview();
      else if (preview.current.url) presentSelected();
      else void requestPreview(id);
    }),
    setComparison: scoped((id, pressed: boolean) => {
      if (
        pressed &&
        (cameraReference ||
          composable.dirty() ||
          preview.current.stale ||
          preview.current.outcome !== "ready" ||
          !preview.current.url)
      )
        return;
      comparisonRequested = pressed;
      render();
      presentSelected();
      if (pressed && !baselineReady()) void baseline.requestCurrent(id);
    }),
    useSaved: scoped(composable.useSaved),
    reapplyLocal: scoped(composable.reapplyLocal),
    discardDraft: scoped(composable.discardEditorComposable),
    rebind: scoped(composable.rebind),
    createProxy: scoped(proxy.create),
    removeProxy: scoped(proxy.remove),
    compose: mutate(composable.composeEditorSteps),
    composableAddStep: mutate(composable.addEditorComposableStep),
    composableRemoveStep: mutate(composable.removeEditorComposableStep),
    composableSelectStep: mutate(composable.selectEditorComposableStep),
    composableEditStep: scoped(composable.editEditorComposableStep),
    composableParameters: mutate((id, text: string) => {
      composable.editEditorComposableParameters(id, text);
      composable.commitEditorComposableParameters(id);
    }),
    composableAutomatic: scoped((id, adjustment) => {
      void composable.automaticEditorAdjustment(id, adjustment);
    }),
    composableSchema: mutate(composable.editEditorComposableSchema),
    composableInput: mutate(composable.editEditorComposableInput),
    composableArtifactFetch: scoped(composable.fetchEditorArtifact),
    composableSave: scoped(composable.saveEditorComposable),
    composableDiscard: scoped(composable.discardEditorComposable),
    checkProcessingExport: scoped(exports.check),
    downloadArtifact: scoped(exports.downloadArtifact),
    downloadHistorical: scoped(exports.downloadHistorical),
    useArtifactInput: mutate(composable.useEditorArtifactInput),
    submitXmp: scoped(outputs.submit),
    downloadXmp: scoped(outputs.download),
    submitExport: scoped(exports.submit),
    cancelExport: scoped(exports.cancel),
    retryExport: scoped(exports.retry),
    downloadExport: scoped(exports.download),
    leave: () => {
      photoId = undefined;
      outputs.leave();
      proxy.leave();
      exports.reset();
      composable.reset();
      clearPreview();
      cameraReference = false;
    },
  };
}
