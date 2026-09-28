import type {
  EditorWhiteBalance,
  EditorWhiteBalancePresentation,
} from "../model/photo-editor.js";
import type {
  EditorStage,
  EditorViewModel,
  LibraryBrowserIntent,
} from "./library-browser-view.js";

type EditorIntent = Extract<LibraryBrowserIntent, { kind: `editor-${string}` }>;

export interface PhotoEditorSurfaceController {
  open(photoId: string): void;
  visible(): boolean;
  render(model: EditorViewModel): void;
  presentPreview(url: string): void;
  clearPreview(): void;
  dispose(): void;
}

const describeWhiteBalance = (intent: EditorWhiteBalance): string =>
  intent.mode === "as-shot"
    ? "As shot"
    : `Temperature ${intent.temperatureKelvin} K, tint ${intent.tintMilli}`;

const loadingWhiteBalance = (): EditorWhiteBalancePresentation =>
  Object.freeze({
    intent: Object.freeze({ mode: "as-shot" }),
    modes: Object.freeze(["as-shot"]),
    adjustable: false,
    note: "",
    temperatureKelvin: null,
    tintMilli: null,
    resettable: false,
  });

export function createPhotoEditorSurfaceController({
  root,
  send,
  isPhotoVisible,
  isEditorSurfaceVisible,
  openEditorSurface,
}: Readonly<{
  root: ParentNode;
  send: (intent: EditorIntent) => void;
  isPhotoVisible: () => boolean;
  isEditorSurfaceVisible: () => boolean;
  openEditorSurface: () => void;
}>): PhotoEditorSurfaceController {
  const editorExposure = required<HTMLInputElement>(
    root,
    "[data-photo-editor-exposure]",
  );
  const editorExposureValue = required<HTMLOutputElement>(
    root,
    "[data-photo-editor-exposure-value]",
  );
  const editorWhiteBalance = required<HTMLElement>(
    root,
    "[data-photo-editor-white-balance]",
  );
  const editorWhiteBalanceMode = required<HTMLSelectElement>(
    root,
    "[data-photo-editor-white-balance-mode]",
  );
  const editorWhiteBalanceNote = required<HTMLElement>(
    root,
    "[data-photo-editor-white-balance-note]",
  );
  const editorTemperature = required<HTMLInputElement>(
    root,
    "[data-photo-editor-temperature]",
  );
  const editorTemperatureValue = required<HTMLOutputElement>(
    root,
    "[data-photo-editor-temperature-value]",
  );
  const editorTint = required<HTMLInputElement>(
    root,
    "[data-photo-editor-tint]",
  );
  const editorTintValue = required<HTMLOutputElement>(
    root,
    "[data-photo-editor-tint-value]",
  );
  const editorResetExposure = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-reset-exposure]",
  );
  const editorResetWhiteBalance = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-reset-white-balance]",
  );
  const editorStageNote = required<HTMLElement>(
    root,
    "[data-photo-editor-stage-note]",
  );
  const editorSupport = required<HTMLElement>(
    root,
    "[data-photo-editor-support]",
  );
  const editorProcessing = required<HTMLElement>(
    root,
    "[data-photo-editor-processing]",
  );
  const editorPreviewState = required<HTMLElement>(
    root,
    "[data-photo-editor-preview-state]",
  );
  const editorCapabilityNote = required<HTMLElement>(
    root,
    "[data-photo-editor-capability]",
  );
  const editorStages = Array.from(
    root.querySelectorAll<HTMLButtonElement>("[data-photo-editor-stage]"),
  );
  const editorProvenance = required<HTMLElement>(
    root,
    "[data-photo-editor-provenance]",
  );
  const editorProxyState = required<HTMLElement>(
    root,
    "[data-photo-editor-proxy-state]",
  );
  const editorProxyCreate = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-proxy-create]",
  );
  const editorProxyRemove = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-proxy-remove]",
  );
  const editorPreview = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-preview]",
  );
  const editorReset = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-reset]",
  );
  const editorUndo = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-undo]",
  );
  const editorRedo = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-redo]",
  );
  const editorCompare = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-compare]",
  );
  const editorPreviewImage = required<HTMLImageElement>(
    root,
    "[data-photo-editor-preview-image]",
  );
  const editorPreviewNote = required<HTMLElement>(
    root,
    "[data-photo-editor-preview-note]",
  );
  const editorDraftNote = required<HTMLElement>(
    root,
    "[data-photo-editor-draft]",
  );
  const editorConflict = required<HTMLElement>(
    root,
    "[data-photo-editor-conflict]",
  );
  const editorConflictMessage = required<HTMLElement>(
    root,
    "[data-photo-editor-conflict-message]",
  );
  const editorUseSaved = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-use-saved]",
  );
  const editorReapply = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-reapply]",
  );
  const editorDiscardDraft = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-discard-draft]",
  );
  const editorExportState = required<HTMLElement>(
    root,
    "[data-photo-editor-export-state]",
  );
  const editorExportSubmit = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-export-submit]",
  );
  const editorExportCancel = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-export-cancel]",
  );
  const editorExportRetry = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-export-retry]",
  );
  const editorExportDownload = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-export-download]",
  );
  const editorExportTarget = required<HTMLElement>(
    root,
    "[data-photo-editor-export-target]",
  );
  const editorStatus = required<HTMLElement>(
    root,
    "[data-photo-editor-status]",
  );
  const editorRebind = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-rebind]",
  );
  const editorRefresh = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-refresh]",
  );
  let alive = true;
  let editorPhotoId: string | undefined;
  let editorModel: EditorViewModel | undefined;
  /// The exposure a live gesture is showing, and the model value it is drawn
  /// over. The draft covers the window between a pointer drag and the write
  /// that follows it, and it is dropped as soon as the model reaches it or
  /// moves anywhere else: an undo, reset, or conflict adoption renders the
  /// settings it actually restored.
  let editorDraft: number | undefined;
  let editorDraftBase: number | undefined;
  const listeners = new AbortController();

  const open = (photoId: string): void => {
    if (!alive || !photoId || !isPhotoVisible()) return;
    editorPhotoId = photoId;
    editorDraft = undefined;
    editorDraftBase = undefined;
    editorModel = undefined;
    render({
      photoId,
      loading: true,
      stage: "develop",
      stageNote: "Develop: loading this Photo's edit facts.",
      filmReason: "",
      editSourceReadiness: "checking",
      editSourceKind: "original",
      sourceFactNote: "Checking source…",
      processingReadiness: "checking",
      previewState: null,
      processingAvailable: false,
      capabilityNote: "",
      exposureEv: 0,
      savedExposureEv: 0,
      baselineExposureEv: 0,
      exposureMinimumEv: 0,
      exposureMaximumEv: 1,
      exposureStepEv: 0.001,
      whiteBalance: loadingWhiteBalance(),
      canEdit: false,
      canPreview: false,
      previewing: false,
      previewNote: "",
      previewStale: false,
      saving: false,
      dirty: false,
      canUndo: false,
      canRedo: false,
      comparing: false,
      conflict: null,
      draftNote: "",
      export: {
        state: "idle",
        note: "",
        artifact: null,
        canSubmit: false,
        canCancel: false,
        canRetry: false,
        canDownload: false,
      },
      status: "Loading edit recipe…",
    });
    send({ kind: "editor-open", photoId });
    openEditorSurface();
  };

  const visible = (): boolean =>
    alive && isPhotoVisible() && isEditorSurfaceVisible();

  const render = (model: EditorViewModel): void => {
    if (
      !alive ||
      (editorPhotoId !== undefined && editorPhotoId !== model.photoId)
    )
      return;
    editorPhotoId = model.photoId;
    editorModel = model;
    const minimum = Number.isFinite(model.exposureMinimumEv)
      ? model.exposureMinimumEv
      : 0;
    const maximum = Number.isFinite(model.exposureMaximumEv)
      ? model.exposureMaximumEv
      : 1;
    const step =
      Number.isFinite(model.exposureStepEv) && model.exposureStepEv > 0
        ? model.exposureStepEv
        : 0.001;
    if (
      editorDraft !== undefined &&
      (model.exposureEv === editorDraft || model.exposureEv !== editorDraftBase)
    ) {
      editorDraft = undefined;
      editorDraftBase = undefined;
    }
    const exposure = editorDraft ?? model.exposureEv;
    editorExposure.min = String(minimum);
    editorExposure.max = String(maximum);
    editorExposure.step = String(step);
    editorExposure.value = String(exposure);
    const label = `${exposure.toFixed(3)} EV`;
    editorExposureValue.value = label;
    editorExposureValue.textContent = label;
    editorExposure.disabled = model.loading || !model.canEdit;
    editorWhiteBalance.textContent = describeWhiteBalance(
      model.whiteBalance.intent,
    );
    const whiteBalance = model.whiteBalance;
    const adjustableOption =
      editorWhiteBalanceMode.querySelector<HTMLOptionElement>(
        'option[value="temperature-tint"]',
      );
    if (adjustableOption) adjustableOption.disabled = !whiteBalance.adjustable;
    editorWhiteBalanceMode.value = whiteBalance.intent.mode;
    editorWhiteBalanceMode.disabled =
      model.loading || !model.canEdit || !whiteBalance.adjustable;
    editorWhiteBalanceNote.textContent = whiteBalance.note;
    editorWhiteBalanceNote.hidden = !whiteBalance.note;
    const temperature = whiteBalance.temperatureKelvin;
    if (temperature) {
      editorTemperature.min = String(temperature.minimum);
      editorTemperature.max = String(temperature.maximum);
      editorTemperature.value = String(temperature.value);
      editorTemperature.disabled = model.loading || !temperature.enabled;
      const temperatureLabel = `${temperature.value} K`;
      editorTemperatureValue.value = temperatureLabel;
      editorTemperatureValue.textContent = temperatureLabel;
    } else {
      editorTemperature.disabled = true;
      editorTemperatureValue.value = "—";
      editorTemperatureValue.textContent = "—";
    }
    const tint = whiteBalance.tintMilli;
    if (tint) {
      editorTint.min = String(tint.minimum);
      editorTint.max = String(tint.maximum);
      editorTint.value = String(tint.value);
      editorTint.disabled = model.loading || !tint.enabled;
      const tintLabel = `${tint.value}`;
      editorTintValue.value = tintLabel;
      editorTintValue.textContent = tintLabel;
    } else {
      editorTint.disabled = true;
      editorTintValue.value = "—";
      editorTintValue.textContent = "—";
    }
    editorResetExposure.disabled =
      model.loading ||
      !model.canEdit ||
      Math.abs(exposure - model.baselineExposureEv) < step / 2;
    editorResetWhiteBalance.disabled =
      model.loading || !model.canEdit || !whiteBalance.resettable;
    // The three readiness axes are presented as the independent facts they
    // are: the Edit source line carries its own wait or outcome, the
    // Processing line names the deployment's engines, and the Edit Preview
    // line names what the presented rendition is on this stage.
    editorSupport.textContent = model.loading
      ? "Checking…"
      : model.sourceFactNote;
    editorProxyState.textContent = model.proxy?.note ?? "No Development Proxy.";
    editorProxyCreate.disabled = model.loading || !model.proxy?.canCreate;
    editorProxyRemove.disabled = model.loading || !model.proxy?.canRemove;
    editorProxyRemove.hidden = !model.proxy?.canRemove;
    editorProcessing.textContent =
      model.processingReadiness === "checking"
        ? "Checking…"
        : model.processingReadiness === "ready"
          ? "Ready"
          : model.processingReadiness === "waiting"
            ? "Waiting for capacity"
            : "Unavailable";
    editorPreviewState.textContent =
      model.previewState === null
        ? "—"
        : model.previewState === "pending"
          ? "Pending"
          : model.previewState === "ready"
            ? "Ready"
            : model.previewState === "stale"
              ? "Stale"
              : "Failed";
    editorCapabilityNote.textContent = model.capabilityNote;
    editorCapabilityNote.hidden = !model.capabilityNote;
    for (const button of editorStages) {
      const stage = button.dataset.photoEditorStage as EditorStage | undefined;
      button.setAttribute("aria-pressed", String(stage === model.stage));
      button.disabled = stage === "film" && Boolean(model.filmReason);
      if (stage === "film") {
        button.title = model.filmReason;
        if (model.filmReason)
          button.setAttribute("aria-describedby", editorStageNote.id);
        else button.removeAttribute("aria-describedby");
      }
    }
    editorStageNote.textContent = model.filmReason;
    editorStageNote.hidden = !model.filmReason;
    editorProvenance.textContent = model.stageNote;
    // Reset all restores the processing baseline of both controls, so it is
    // enabled exactly while the settings are away from that baseline. Local
    // changes are discarded by Undo, never by a disabled Reset all.
    const atBaseline =
      Math.abs(exposure - model.baselineExposureEv) < step / 2 &&
      !model.whiteBalance.resettable;
    editorUndo.disabled = model.loading || !model.canUndo;
    editorRedo.disabled = model.loading || !model.canRedo;
    editorReset.disabled = model.loading || model.saving || atBaseline;
    // The comparison compares a stage's baseline development with the current
    // settings; the Camera stage presents the camera Preview itself, so it
    // offers no comparison of its own.
    editorCompare.disabled =
      model.loading || !model.canPreview || model.stage === "camera";
    editorCompare.setAttribute("aria-pressed", String(model.comparing));
    editorPreview.disabled =
      model.loading || model.previewing || !model.canPreview;
    editorRebind.hidden = !model.conflict;
    editorRebind.disabled = model.loading || model.saving || !model.conflict;
    editorRefresh.disabled = model.loading || model.saving;
    editorDraftNote.textContent = model.draftNote;
    editorDraftNote.hidden = !model.draftNote;
    editorConflict.hidden = !model.conflict;
    editorConflictMessage.textContent = model.conflict?.message ?? "";
    editorUseSaved.disabled = model.saving;
    editorReapply.disabled = model.saving;
    editorDiscardDraft.disabled = model.saving;
    const exported = model.export;
    const exportLabel =
      model.stage === "film" ? "Finished JPEG" : "Development TIFF";
    editorExportTarget.textContent = exportLabel;
    editorExportState.textContent = exported.note;
    editorExportSubmit.textContent = `Export ${exportLabel}`;
    editorExportSubmit.disabled =
      model.loading || !model.canEdit || !exported.canSubmit;
    editorExportCancel.hidden = !exported.canCancel;
    editorExportRetry.hidden = !exported.canRetry;
    editorExportDownload.hidden = !exported.canDownload;
    // The Edit Preview note describes the Develop or Film rendition. The
    // Camera stage presents the camera Preview, which is not an Edit Preview.
    const editPreviewStage = model.stage !== "camera";
    editorPreviewNote.textContent = editPreviewStage ? model.previewNote : "";
    editorPreviewNote.hidden = !editPreviewStage || !model.previewNote;
    editorPreviewNote.dataset.tone = model.previewStale ? "stale" : "";
    editorStatus.textContent = model.status;
    editorStatus.dataset.tone = model.status ? "notice" : "";
  };

  const presentPreview = (url: string): void => {
    if (!alive || !visible() || !editorPhotoId) return;
    editorPreviewImage.src = new URL(url, window.location.href).href;
    editorPreviewImage.hidden = false;
  };

  const clearPreview = (): void => {
    if (!alive) return;
    editorPreviewImage.hidden = true;
    editorPreviewImage.removeAttribute("src");
  };

  editorExposure.addEventListener(
    "input",
    () => {
      if (!editorPhotoId || !editorModel || editorExposure.disabled) return;
      editorDraftBase ??= editorModel.exposureEv;
      editorDraft = Number(editorExposure.value);
      render(editorModel);
    },
    { signal: listeners.signal },
  );
  editorExposure.addEventListener(
    "change",
    () => {
      if (!editorPhotoId || !editorModel || editorExposure.disabled) return;
      const value = Number(editorExposure.value);
      editorDraftBase ??= editorModel.exposureEv;
      editorDraft = value;
      render(editorModel);
      send({
        kind: "editor-exposure",
        photoId: editorPhotoId,
        exposureEv: value,
      });
    },
    { signal: listeners.signal },
  );
  editorWhiteBalanceMode.addEventListener(
    "change",
    () => {
      if (!editorPhotoId || !editorModel || editorWhiteBalanceMode.disabled)
        return;
      send({
        kind: "editor-white-balance-mode",
        photoId: editorPhotoId,
        mode: editorWhiteBalanceMode.value,
      });
    },
    { signal: listeners.signal },
  );
  editorTemperature.addEventListener(
    "change",
    () => {
      if (!editorPhotoId || !editorModel || editorTemperature.disabled) return;
      send({
        kind: "editor-temperature",
        photoId: editorPhotoId,
        temperatureKelvin: Number(editorTemperature.value),
      });
    },
    { signal: listeners.signal },
  );
  editorTint.addEventListener(
    "change",
    () => {
      if (!editorPhotoId || !editorModel || editorTint.disabled) return;
      send({
        kind: "editor-tint",
        photoId: editorPhotoId,
        tintMilli: Number(editorTint.value),
      });
    },
    { signal: listeners.signal },
  );
  editorResetExposure.addEventListener(
    "click",
    () => {
      if (!editorPhotoId || !editorModel) return;
      send({ kind: "editor-reset-exposure", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorResetWhiteBalance.addEventListener(
    "click",
    () => {
      if (!editorPhotoId || !editorModel) return;
      send({ kind: "editor-reset-white-balance", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorPreview.addEventListener(
    "click",
    () => {
      if (editorPhotoId)
        send({ kind: "editor-preview", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorUndo.addEventListener(
    "click",
    () => {
      if (editorPhotoId) send({ kind: "editor-undo", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorRedo.addEventListener(
    "click",
    () => {
      if (editorPhotoId) send({ kind: "editor-redo", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorCompare.addEventListener(
    "click",
    () => {
      if (!editorPhotoId || !editorModel) return;
      send({
        kind: "editor-compare",
        photoId: editorPhotoId,
        pressed: !editorModel.comparing,
      });
    },
    { signal: listeners.signal },
  );
  editorReset.addEventListener(
    "click",
    () => {
      if (!editorPhotoId || !editorModel) return;
      send({ kind: "editor-reset", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorRefresh.addEventListener(
    "click",
    () => {
      if (editorPhotoId)
        send({ kind: "editor-refresh", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorUseSaved.addEventListener(
    "click",
    () => {
      if (editorPhotoId)
        send({ kind: "editor-use-saved", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorReapply.addEventListener(
    "click",
    () => {
      if (editorPhotoId)
        send({ kind: "editor-reapply", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorDiscardDraft.addEventListener(
    "click",
    () => {
      if (editorPhotoId)
        send({ kind: "editor-discard-draft", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorProxyCreate.addEventListener(
    "click",
    () => {
      if (editorPhotoId)
        send({ kind: "editor-proxy-create", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorProxyRemove.addEventListener(
    "click",
    () => {
      if (editorPhotoId)
        send({ kind: "editor-proxy-remove", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorExportSubmit.addEventListener(
    "click",
    () => {
      if (editorPhotoId)
        send({ kind: "editor-export-submit", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorExportCancel.addEventListener(
    "click",
    () => {
      if (editorPhotoId)
        send({ kind: "editor-export-cancel", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorExportRetry.addEventListener(
    "click",
    () => {
      if (editorPhotoId)
        send({ kind: "editor-export-retry", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  editorExportDownload.addEventListener(
    "click",
    () => {
      if (editorPhotoId)
        send({ kind: "editor-export-download", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );
  for (const button of editorStages)
    button.addEventListener(
      "click",
      () => {
        if (!editorPhotoId || button.disabled) return;
        send({
          kind: "editor-stage",
          photoId: editorPhotoId,
          stage: button.dataset.photoEditorStage as EditorStage,
        });
      },
      { signal: listeners.signal },
    );
  editorRebind.addEventListener(
    "click",
    () => {
      if (!editorPhotoId || editorRebind.disabled) return;
      send({ kind: "editor-rebind", photoId: editorPhotoId });
    },
    { signal: listeners.signal },
  );

  return {
    open,
    visible,
    render,
    presentPreview,
    clearPreview,
    dispose() {
      alive = false;
      listeners.abort();
    },
  };
}
function required<T extends Element>(root: ParentNode, selector: string): T {
  const value = root.querySelector<T>(selector);
  if (!value) throw new Error(`Missing ${selector}`);
  return value;
}
