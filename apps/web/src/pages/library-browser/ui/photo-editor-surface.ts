import { createComposableEditorSurface } from "./composable-editor-surface.js";
import { createWorkspaceOutputSurface } from "./workspace-output-surface.js";
import { editorSaveRecovery } from "./photo-editor-presentation.js";
import type {
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
export function createPhotoEditorSurfaceController({
  root,
  send,
  isPhotoVisible,
  isEditorSurfaceVisible,
  openEditorSurface,
  getCameraReferenceUrl = () => undefined,
}: Readonly<{
  root: ParentNode;
  send: (intent: EditorIntent) => void;
  isPhotoVisible: () => boolean;
  isEditorSurfaceVisible: () => boolean;
  openEditorSurface: () => void;
  getCameraReferenceUrl?: (photoId: string) => string | undefined;
}>): PhotoEditorSurfaceController {
  let alive = true;
  let photoId: string | undefined;
  let model: EditorViewModel | undefined;
  const listeners = new AbortController();
  const composable = createComposableEditorSurface(
    root,
    send,
    listeners.signal,
  );
  const outputs = createWorkspaceOutputSurface(root, send, listeners.signal);
  const element = <T extends HTMLElement>(name: string): T => {
    const value = root.querySelector<T>(`[data-photo-editor-${name}]`);
    if (!value) throw new Error(`Missing Photo Editor element: ${name}`);
    return value;
  };
  const image = element<HTMLImageElement>("preview-image");
  const referenceImage = element<HTMLImageElement>("reference-image");
  const syncImage = (): void => {
    const reference = model?.cameraReference === true;
    const url =
      alive &&
      photoId &&
      reference &&
      isPhotoVisible() &&
      isEditorSurfaceVisible()
        ? getCameraReferenceUrl(photoId)
        : undefined;
    if (url) referenceImage.src = url;
    else referenceImage.removeAttribute("src");
    referenceImage.hidden = !url;
    image.hidden = reference || !image.getAttribute("src");
    element("image-empty").hidden = !image.hidden || !referenceImage.hidden;
    element("image-empty").textContent = reference
      ? "The camera Preview for this Photo is unavailable. No image has been substituted."
      : model?.primary?.revision || model?.composable.steps.length
        ? "No Edit Preview is available yet. Refresh the preview when processing is ready."
        : "No Edit State yet. Choose an available Processing Engine to start, or inspect the Original reference.";
  };
  const text = (name: string, value: string, hideEmpty = false): void => {
    const target = element(name);
    target.textContent = value;
    if (hideEmpty) target.hidden = !value;
  };
  const button = (name: string, enabled: boolean, hidden = false): void => {
    const target = element<HTMLButtonElement>(name);
    target.disabled = !enabled;
    target.hidden = hidden;
  };
  const action = (name: string, kind: EditorIntent["kind"]): void => {
    element<HTMLButtonElement>(name).addEventListener(
      "click",
      () => {
        if (
          !alive ||
          !photoId ||
          !model ||
          element<HTMLButtonElement>(name).disabled
        )
          return;
        send({ kind, photoId } as EditorIntent);
      },
      { signal: listeners.signal },
    );
  };
  const exposure = element<HTMLInputElement>("exposure");
  exposure.addEventListener(
    "change",
    () => {
      if (
        !alive ||
        !photoId ||
        !model?.primary?.canSet ||
        exposure.value === ""
      )
        return;
      const value = Number(exposure.value);
      if (Number.isFinite(value))
        send({ kind: "editor-exposure-set", photoId, value });
    },
    { signal: listeners.signal },
  );
  for (const [name, kind] of [
    ["preview", "editor-preview"],
    ["undo", "editor-undo"],
    ["redo", "editor-redo"],
    ["refresh", "editor-refresh"],
    ["use-saved", "editor-use-saved"],
    ["reapply", "editor-reapply"],
    ["discard-draft", "editor-discard-draft"],
    ["rebind", "editor-rebind"],
    ["proxy-create", "editor-proxy-create"],
    ["proxy-remove", "editor-proxy-remove"],
    ["export-submit", "editor-export-submit"],
    ["export-cancel", "editor-export-cancel"],
    ["export-retry", "editor-export-retry"],
    ["export-download", "editor-export-download"],
    ["export-check", "editor-processing-export-check"],
    ["exposure-reset", "editor-exposure-reset"],
  ] as const)
    action(name, kind);
  element<HTMLButtonElement>("camera-reference").addEventListener(
    "click",
    () => {
      if (alive && photoId && model)
        send({
          kind: "editor-camera-reference",
          photoId,
          pressed: !model.cameraReference,
        });
    },
    { signal: listeners.signal },
  );
  element<HTMLButtonElement>("current").addEventListener(
    "click",
    () => {
      if (!alive || !photoId || !model) return;
      if (model.cameraReference)
        send({ kind: "editor-camera-reference", photoId, pressed: false });
      else if (model.comparing)
        send({ kind: "editor-compare", photoId, pressed: false });
    },
    { signal: listeners.signal },
  );
  element<HTMLButtonElement>("compare").addEventListener(
    "click",
    () => {
      if (alive && photoId && model && model.canCompare)
        send({ kind: "editor-compare", photoId, pressed: !model.comparing });
    },
    { signal: listeners.signal },
  );
  const clearPreview = (): void => {
    image.hidden = true;
    image.removeAttribute("src");
    syncImage();
  };
  return {
    open(id) {
      if (!alive || !id || !isPhotoVisible()) return;
      photoId = id;
      model = undefined;
      clearPreview();
      referenceImage.hidden = true;
      referenceImage.removeAttribute("src");
      text("status", "Loading Edit State…");
      root
        .querySelectorAll<HTMLButtonElement>(".photo-editor-controls button")
        .forEach((target: HTMLButtonElement) => {
          target.disabled = true;
        });
      openEditorSurface();
      send({ kind: "editor-open", photoId: id });
    },
    visible: () => alive && isPhotoVisible() && isEditorSurfaceVisible(),
    render(next) {
      if (!alive || photoId !== next.photoId) return;
      model = next;
      const busy = next.loading || next.saving;
      const primary = next.primary;
      text("primary-state", primary?.note ?? "Checking current Edit State…");
      text(
        "primary-input",
        primary?.input?.kind === "artifact"
          ? "Input: retained Processing Artifact"
          : "Input: Original of this Photo",
      );
      text(
        "primary-detail",
        primary
          ? `Revision ${primary.revision ?? "unsaved"} · Source ${primary.sourceRevision ?? "unavailable"}${primary.input?.kind === "artifact" ? ` · Artifact ${primary.input.artifactId}` : ""}`
          : "",
      );
      text(
        "engine-state",
        next.loading
          ? "Checking Processing Engine availability…"
          : next.composable.modules
              .map((engine) =>
                engine.ready
                  ? `${engine.name}: available`
                  : `${engine.name}: unavailable${engine.refusalNote ? ` — ${engine.refusalNote}` : ""}`,
              )
              .join(" · ") || "No Processing Engine is available.",
      );
      exposure.disabled = !primary?.canSet || next.mutationsBlocked;
      exposure.value =
        primary?.exposureEv === null || primary?.exposureEv === undefined
          ? ""
          : String(primary.exposureEv);
      button(
        "exposure-reset",
        Boolean(primary?.canReset) && !next.mutationsBlocked,
      );
      text("provenance", next.provenanceNote);
      text("support", next.sourceFactNote);
      text(
        "capability",
        next.processingReadiness === "unavailable"
          ? "The selected Processing Engine is unavailable for processing."
          : "",
        true,
      );
      text("detail", next.statusDetail, true);
      text("status", next.status);
      text(
        "render-status",
        next.cameraReference
          ? "Camera Preview · Original reference"
          : primary?.saveState === "none" ||
              (!primary?.revision && !next.composable.steps.length)
            ? "Start editing · no Edit State yet"
            : next.comparing
              ? "Baseline comparison · same processing input"
              : next.previewState === "ready"
                ? "Current Edit Preview"
                : next.previewState === "stale"
                  ? "Previous Edit Preview · updating"
                  : next.previewState === "failed"
                    ? "Edit Preview failed"
                    : "Edit Preview pending",
      );
      image.alt = next.comparing
        ? "Baseline comparison · same processing input"
        : "Current Edit Preview";
      text(
        "preview-note",
        next.cameraReference || primary?.saveState === "none"
          ? ""
          : next.previewNote,
        true,
      );
      element("preview-note").dataset.tone = next.previewStale ? "stale" : "";
      text("draft", next.draftNote, true);
      const recovery = editorSaveRecovery(next);
      element("conflict").hidden = !recovery.visible;
      text("conflict-message", recovery.message);
      button("current", !next.loading);
      element("current").setAttribute(
        "aria-pressed",
        String(!next.cameraReference && !next.comparing),
      );
      button("camera-reference", !next.loading);
      element("camera-reference").setAttribute(
        "aria-pressed",
        String(next.cameraReference),
      );
      button("compare", !next.loading && next.canCompare);
      element("compare").setAttribute("aria-pressed", String(next.comparing));
      button("preview", !next.loading && !next.previewing && next.canPreview);
      button("undo", !next.loading && next.canUndo);
      button("redo", !next.loading && next.canRedo);
      button("refresh", !next.loading);
      button("rebind", !busy && next.rebindAvailable, !next.rebindAvailable);
      button("use-saved", !busy, !recovery.canUseSaved);
      button("reapply", !busy, !recovery.canReapply);
      button("discard-draft", !busy, !recovery.canDiscard);
      text("proxy-state", next.proxy?.note ?? "No Development Proxy.");
      button("proxy-create", !busy && Boolean(next.proxy?.canCreate));
      button(
        "proxy-remove",
        !busy && Boolean(next.proxy?.canRemove),
        !next.proxy?.canRemove,
      );
      text("export-target", "current Edit State");
      text("export-state", next.export.note);
      const output = next.export.output;
      text(
        "export-output",
        output ? `${output.note} Expires ${output.expiresAt}.` : "",
        true,
      );
      text("export-evidence", output?.evidence ?? "");
      element("export-details").hidden = !output;
      button("export-submit", !next.loading && next.export.canSubmit);
      button("export-cancel", next.export.canCancel, !next.export.canCancel);
      button("export-retry", next.export.canRetry, !next.export.canRetry);
      button(
        "export-download",
        next.export.canDownload && output?.state !== "expired",
        !output && !next.export.canDownload,
      );
      button(
        "export-check",
        !next.loading,
        !next.export.processingRequestId ||
          !["queued", "running", "outcome-unknown"].includes(next.export.state),
      );
      element("export-retry").textContent =
        next.export.state === "outcome-unknown" ? "Check result" : "Retry";
      syncImage();
      composable.render(next);
      button("composable-artifact-fetch", !next.loading);
      root
        .querySelectorAll<
          HTMLInputElement | HTMLSelectElement | HTMLButtonElement
        >("[data-photo-editor-module-controls] input, [data-photo-editor-module-controls] select, [data-photo-editor-composable-input-original], [data-photo-editor-composable-input-artifact], [data-photo-editor-composable-artifact], [data-photo-editor-composable-module], [data-photo-editor-composable-add]")
        .forEach(
          (
            control: HTMLInputElement | HTMLSelectElement | HTMLButtonElement,
          ) => {
            if (next.loading || next.mutationsBlocked) control.disabled = true;
          },
        );
      outputs.render(
        next.photoId,
        next.outputs,
        next.saving || next.dirty,
        next.loading,
      );
    },
    presentPreview(url) {
      if (!alive || !model || model.cameraReference) return;
      image.src = url;
      syncImage();
    },
    clearPreview,
    dispose() {
      alive = false;
      listeners.abort();
      clearPreview();
      referenceImage.hidden = true;
      referenceImage.removeAttribute("src");
    },
  };
}
