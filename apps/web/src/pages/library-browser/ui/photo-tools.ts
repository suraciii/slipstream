import type { ModalSurfaces } from "./modal-surface.js";

export type PhotoToolsView =
  | "tools"
  | "rating"
  | "edit"
  | "albums"
  | "details"
  | "zoom"
  | "nearby";
export interface PhotoToolsController {
  view(): PhotoToolsView;
  isNearbyOpen(): boolean;
  open(view?: PhotoToolsView, invoker?: HTMLElement): void;
  returnToTools(): void;
  close(restoreFocus?: boolean): void;
  dispose(): void;
}

/// The Photo review dock's secondary-action disclosure: one `⋯` toggle that
/// opens the Undo and More pair, Escape that closes it and returns focus to
/// the toggle, and a pointer press outside the dock that closes it. The
/// disclosure is the dock's own state; a wide layout shows the same two
/// buttons inline through CSS, so this controller never hides or duplicates
/// them.
export interface DockDisclosure {
  isOpen(): boolean;
  close(restoreFocus?: boolean): void;
  dispose(): void;
}

export function createDockDisclosure(dock: HTMLElement): DockDisclosure {
  const toggle = dock.querySelector<HTMLButtonElement>("[data-dock-overflow]");
  const secondary = dock.querySelector<HTMLElement>("[data-dock-secondary]");
  if (!toggle || !secondary)
    throw new Error("Photo dock disclosure controls are missing.");
  let open = false;
  const listeners = new AbortController();

  const apply = (): void => {
    if (open) secondary.setAttribute("data-open", "");
    else secondary.removeAttribute("data-open");
    toggle.setAttribute("aria-expanded", String(open));
  };

  const close = (restoreFocus = true): void => {
    if (!open) return;
    open = false;
    apply();
    if (restoreFocus) toggle.focus();
  };

  toggle.addEventListener(
    "click",
    () => {
      open = !open;
      apply();
      if (open) toggle.focus();
    },
    { signal: listeners.signal },
  );
  dock.addEventListener(
    "keydown",
    (event) => {
      if (event.key !== "Escape" || !open) return;
      event.stopPropagation();
      close();
    },
    { signal: listeners.signal },
  );
  document.addEventListener(
    "pointerdown",
    (event) => {
      if (!open) return;
      // A press inside the dock belongs to the disclosure itself, so only a
      // press elsewhere dismisses it — without restoring focus, because the
      // press already moved the Photographer somewhere else.
      const target = event.target;
      if (target === dock || target === toggle) return;
      if (
        target !== null &&
        typeof target === "object" &&
        dock.contains(target as Node)
      )
        return;
      // A press inside an open modal belongs to a surface the dock entry
      // opened; the disclosure stays open so closing that surface can return
      // focus to the entry that started it.
      const owner = target as {
        closest?: (selector: string) => unknown;
      } | null;
      if (owner?.closest?.("dialog[open]")) return;
      close(false);
    },
    { signal: listeners.signal },
  );

  apply();
  return {
    isOpen: () => open,
    close,
    dispose() {
      listeners.abort();
    },
  };
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
  openDetails,
  openViewOptions,
  openRecovery,
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
  openDetails?: (photoId: string) => void;
  openViewOptions: () => void;
  openRecovery: () => void;
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
        : currentView === "rating"
          ? "Rating"
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
      const entry = button.dataset.photoToolsEntry;
      if (!entry) return;
      if (entry === "view-options") {
        openViewOptions();
        return;
      }
      if (entry === "recovery") {
        openRecovery();
        return;
      }
      const photoId = currentPhotoId();
      if (!photoId) return;
      if (entry === "sources") {
        openSources();
        return;
      }
      if (entry === "edit") {
        openEditor(photoId);
        return;
      }
      if (
        entry === "rating" ||
        entry === "albums" ||
        entry === "details" ||
        entry === "zoom" ||
        entry === "nearby"
      ) {
        open(entry);
      }
    },
    { signal: listeners.signal },
  );

  const open = (
    view: PhotoToolsView = "tools",
    invoker?: HTMLElement,
  ): void => {
    if (!alive || !isPhotoVisible()) return;
    resetGestures();
    currentView = view;
    applyView();
    if (view === "details") {
      const photoId = currentPhotoId();
      if (photoId) openDetails?.(photoId);
    }
    if (!surfaces.isActive("photo-tools"))
      surfaces.open(
        "photo-tools",
        invoker ??
          (document.activeElement instanceof HTMLElement
            ? document.activeElement
            : undefined),
      );
    syncFilmstripHost();
    syncSecondarySurface();
    photoToolsClose.focus();
  };

  const returnToTools = (): void => {
    if (!alive || !surfaces.isActive("photo-tools")) return;
    currentView = "tools";
    applyView();
    syncSecondarySurface();
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
