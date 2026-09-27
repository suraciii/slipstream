import type { ModalSurfaces } from "./modal-surface.js";

export interface SourceSurfaceController {
  syncLayout(): void;
  open(): void;
  close(restoreFocus?: boolean): void;
  dispose(): void;
}

export function createSourceSurfaceController({
  elements,
  surfaces,
  isModal,
  resetGestures,
  closePhotoTools,
  closeRatingChoices,
}: Readonly<{
  elements: Readonly<{
    browser: HTMLElement;
    sourceDialog: HTMLDialogElement;
    sourceResizer: HTMLElement;
    sourceToggle: HTMLButtonElement;
    photoSourceToggle: HTMLButtonElement;
    sourceClose: HTMLButtonElement;
  }>;
  surfaces: ModalSurfaces;
  isModal: () => boolean;
  resetGestures: () => void;
  closePhotoTools: (restoreFocus?: boolean) => void;
  closeRatingChoices: (restoreFocus?: boolean) => void;
}>): SourceSurfaceController {
  const {
    browser,
    sourceDialog,
    sourceResizer,
    sourceToggle,
    photoSourceToggle,
    sourceClose,
  } = elements;
  let alive = true;
  let sourceWidth = 224;
  let resizing = false;
  const listeners = new AbortController();

  const setSourceWidth = (width: number): void => {
    sourceWidth = Math.max(176, Math.min(360, width));
    browser.style.setProperty("--source-width", `${sourceWidth}px`);
  };

  const syncExpanded = (): void => {
    const expanded = sourceDialog.open;
    sourceToggle.setAttribute("aria-expanded", String(expanded));
    photoSourceToggle.setAttribute("aria-expanded", String(expanded));
  };

  const close = (restoreFocus = true): void => {
    if (!alive) return;
    surfaces.close("sources", restoreFocus);
    syncExpanded();
  };

  const syncLayout = (): void => {
    if (!alive) return;
    const modal = isModal();
    browser.classList.toggle("sources-drawer", modal);
    sourceToggle.hidden = !modal;
    photoSourceToggle.hidden = !modal;
    if (!modal) close(false);
    syncExpanded();
  };

  const open = (): void => {
    if (!alive || !isModal()) return;
    const invoker =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : undefined;
    resetGestures();
    closePhotoTools(false);
    closeRatingChoices(false);
    surfaces.open("sources", invoker);
    syncLayout();
    sourceClose.focus();
  };

  surfaces.register("sources", {
    dialog: sourceDialog,
    modal: isModal,
  });
  sourceResizer.addEventListener(
    "pointerdown",
    (event) => {
      if (isModal()) return;
      resizing = true;
      sourceResizer.setPointerCapture(event.pointerId);
      sourceResizer.classList.add("active");
    },
    { signal: listeners.signal },
  );
  sourceResizer.addEventListener(
    "pointermove",
    (event) => {
      if (resizing)
        setSourceWidth(event.clientX - browser.getBoundingClientRect().left);
    },
    { signal: listeners.signal },
  );
  const stopResize = (): void => {
    resizing = false;
    sourceResizer.classList.remove("active");
  };
  sourceResizer.addEventListener("pointerup", stopResize, {
    signal: listeners.signal,
  });
  sourceResizer.addEventListener("pointercancel", stopResize, {
    signal: listeners.signal,
  });
  sourceResizer.addEventListener(
    "keydown",
    (event) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      setSourceWidth(sourceWidth + (event.key === "ArrowRight" ? 16 : -16));
    },
    { signal: listeners.signal },
  );
  sourceDialog.addEventListener("cancel", syncExpanded, {
    signal: listeners.signal,
  });
  sourceDialog.addEventListener("close", syncExpanded, {
    signal: listeners.signal,
  });
  sourceToggle.addEventListener("click", open, { signal: listeners.signal });
  photoSourceToggle.addEventListener("click", open, {
    signal: listeners.signal,
  });
  sourceClose.addEventListener("click", () => close(), {
    signal: listeners.signal,
  });
  setSourceWidth(sourceWidth);
  syncLayout();

  return {
    syncLayout,
    open,
    close,
    dispose() {
      alive = false;
      listeners.abort();
    },
  };
}
