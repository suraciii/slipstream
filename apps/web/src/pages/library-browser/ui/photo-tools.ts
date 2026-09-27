import type { ModalSurfaces } from "./modal-surface.js";

export type PhotoToolsView =
  | "tools"
  | "edit"
  | "albums"
  | "details"
  | "zoom"
  | "nearby";

export interface PhotoToolsController {
  view(): PhotoToolsView;
  isNearbyOpen(): boolean;
  open(view?: PhotoToolsView): void;
  returnToTools(): void;
  close(restoreFocus?: boolean): void;
  dispose(): void;
}

export function createPhotoToolsController({
  elements,
  surfaces,
  isPhotoVisible,
  currentPhotoId,
  resetGestures,
  syncFilmstripHost,
  syncSecondarySurface,
  openSources,
  openEditor,
}: Readonly<{
  elements: Readonly<{
    photoToolsDialog: HTMLDialogElement;
    photoToolsClose: HTMLButtonElement;
    photoToolsTitle: HTMLElement;
    photoToolsViews: ReadonlyArray<HTMLElement>;
    photoToolsEntries: HTMLElement;
  }>;
  surfaces: ModalSurfaces;
  isPhotoVisible: () => boolean;
  currentPhotoId: () => string | undefined;
  resetGestures: () => void;
  syncFilmstripHost: () => void;
  syncSecondarySurface: () => void;
  openSources: () => void;
  openEditor: (photoId: string) => void;
}>): PhotoToolsController {
  const {
    photoToolsDialog,
    photoToolsClose,
    photoToolsTitle,
    photoToolsViews,
    photoToolsEntries,
  } = elements;
  let alive = true;
  let currentView: PhotoToolsView = "tools";
  const listeners = new AbortController();

  const applyView = (): void => {
    for (const view of photoToolsViews)
      view.hidden = view.dataset.photoToolsView !== currentView;
    photoToolsDialog.dataset.photoToolsSurface = currentView;
    photoToolsTitle.textContent =
      currentView === "tools"
        ? "Photo tools"
        : currentView === "nearby"
          ? "Nearby Photos"
          : currentView === "zoom"
            ? "Preview Zoom"
            : currentView === "albums"
              ? "Albums"
              : currentView === "details"
                ? "Details"
                : "Edit";
  };

  const onClose = (): void => {
    if (!alive) return;
    currentView = "tools";
    applyView();
    syncFilmstripHost();
    syncSecondarySurface();
  };

  surfaces.register("photo-tools", {
    dialog: photoToolsDialog,
    modal: () => true,
    onOpen: syncSecondarySurface,
  });
  photoToolsDialog.addEventListener("close", onClose, {
    signal: listeners.signal,
  });
  photoToolsClose.addEventListener("click", () => close(), {
    signal: listeners.signal,
  });
  photoToolsEntries.addEventListener(
    "click",
    (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
        "[data-photo-tools-entry]",
      );
      if (!button) return;
      const photoId = currentPhotoId();
      if (!photoId) return;
      const entry = button.dataset.photoToolsEntry;
      if (entry === "sources") {
        openSources();
        return;
      }
      if (entry === "edit") {
        openEditor(photoId);
        return;
      }
      if (
        entry === "albums" ||
        entry === "details" ||
        entry === "zoom" ||
        entry === "nearby"
      )
        open(entry);
    },
    { signal: listeners.signal },
  );

  const open = (view: PhotoToolsView = "tools"): void => {
    if (!alive || !isPhotoVisible()) return;
    resetGestures();
    currentView = view;
    applyView();
    if (!surfaces.isActive("photo-tools"))
      surfaces.open(
        "photo-tools",
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : undefined,
      );
    syncFilmstripHost();
    syncSecondarySurface();
    photoToolsClose.focus();
  };

  const returnToTools = (): void => {
    if (!alive || !surfaces.isActive("photo-tools")) return;
    currentView = "tools";
    applyView();
    syncFilmstripHost();
    photoToolsClose.focus();
  };

  const close = (restoreFocus = true): void => {
    if (!alive) return;
    currentView = "tools";
    applyView();
    surfaces.close("photo-tools", restoreFocus);
    syncFilmstripHost();
    syncSecondarySurface();
  };

  applyView();
  return {
    view: () => currentView,
    isNearbyOpen: () =>
      currentView === "nearby" && surfaces.isActive("photo-tools"),
    open,
    returnToTools,
    close,
    dispose() {
      alive = false;
      listeners.abort();
    },
  };
}
