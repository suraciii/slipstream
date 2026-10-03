import { createComposableEditorSurface } from "./composable-editor-surface.js";
import { createWorkspaceOutputSurface } from "./workspace-output-surface.js";
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
}: Readonly<{
  root: ParentNode;
  send: (intent: EditorIntent) => void;
  isPhotoVisible: () => boolean;
  isEditorSurfaceVisible: () => boolean;
  openEditorSurface: () => void;
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
  };
  return {
    open(id) {
      if (!alive || !id || !isPhotoVisible()) return;
      photoId = id;
      model = undefined;
      clearPreview();
      text("status", "Loading Processing Recipe…");
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
      text("provenance", next.provenanceNote);
      text("support", next.sourceFactNote);
      text(
        "capability",
        next.processingReadiness === "unavailable"
          ? "The selected module is unavailable for processing."
          : "",
        true,
      );
      text("detail", next.statusDetail, true);
      text("status", next.status);
      text(
        "render-status",
        next.cameraReference
          ? "Camera Preview · Original reference"
          : next.comparing
            ? "Selected step baseline comparison"
            : next.previewState === "ready"
              ? "Current step Preview"
              : next.previewState === "stale"
                ? "Previous step Preview · updating"
                : next.previewState === "failed"
                  ? "Step Preview failed"
                  : "Step Preview pending",
      );
      text("preview-note", next.cameraReference ? "" : next.previewNote, true);
      element("preview-note").dataset.tone = next.previewStale ? "stale" : "";
      text("draft", next.draftNote, true);
      element("conflict").hidden = !next.conflict;
      text("conflict-message", next.conflict?.message ?? "");
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
      button("use-saved", !busy);
      button("reapply", !busy);
      button("discard-draft", !busy);
      text("proxy-state", next.proxy?.note ?? "No Development Proxy.");
      button("proxy-create", !busy && Boolean(next.proxy?.canCreate));
      button(
        "proxy-remove",
        !busy && Boolean(next.proxy?.canRemove),
        !next.proxy?.canRemove,
      );
      text("export-target", "selected Processing Step");
      text("export-state", next.export.note);
      button("export-submit", !next.loading && next.export.canSubmit);
      button("export-cancel", next.export.canCancel, !next.export.canCancel);
      button("export-retry", next.export.canRetry, !next.export.canRetry);
      button(
        "export-download",
        next.export.canDownload,
        !next.export.canDownload,
      );
      button(
        "export-check",
        !next.loading,
        !next.export.processingRequestId ||
          !["queued", "running", "outcome-unknown"].includes(next.export.state),
      );
      element("export-retry").textContent =
        next.export.state === "outcome-unknown" ? "Check result" : "Retry";
      if (next.cameraReference) clearPreview();
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
      image.hidden = false;
    },
    clearPreview,
    dispose() {
      alive = false;
      listeners.abort();
      clearPreview();
    },
  };
}
