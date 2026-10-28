import type { ModalSurfaces } from "./modal-surface.js";

export interface SourceSurfaceController {
  syncLayout(): void;
  open(): void;
  close(restoreFocus?: boolean): void;
  dispose(): void;
}

/// The one width owner of the Sources navigation. A wide Grid resizes it
/// through the separator within these bounds; a narrow Grid has no separator
/// and keeps Sources behind its disclosure.
export const SOURCE_WIDTH_MINIMUM = 176;
export const SOURCE_WIDTH_MAXIMUM = 360;
export const SOURCE_WIDTH_DEFAULT = 224;
/// One pointer or keyboard step of the resize, in CSS pixels.
export const SOURCE_WIDTH_STEP = 16;

export function clampSourceWidth(width: number): number {
  return Math.round(
    Math.max(SOURCE_WIDTH_MINIMUM, Math.min(SOURCE_WIDTH_MAXIMUM, width)),
  );
}

export function sourceWidthAnnouncement(width: number): string {
  const applied = clampSourceWidth(width);
  return `Sources width ${applied} pixels, from ${SOURCE_WIDTH_MINIMUM} to ${SOURCE_WIDTH_MAXIMUM}`;
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
    sourceWidthStatus: HTMLElement;
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
    sourceWidthStatus,
  } = elements;
  let alive = true;
  let sourceWidth = SOURCE_WIDTH_MINIMUM;
  let resizing = false;
  const listeners = new AbortController();

  /// Resize feedback is a transient fact: it fills the region on each step and
  /// leaves no history entry of its own, so a later fact replaces it instead of
  /// appending.
  const announce = (message: string): void => {
    sourceWidthStatus.hidden = false;
    sourceWidthStatus.textContent = message;
  };

  const setSourceWidth = (width: number): void => {
    sourceWidth = clampSourceWidth(width);
    browser.style.setProperty("--source-width", `${sourceWidth}px`);
    // The separator is the one control that reads the width owner's value, so
    // the announced bounds and the applied width can never hold two numbers.
    sourceResizer.setAttribute("aria-valuenow", String(sourceWidth));
    sourceResizer.setAttribute("aria-valuemin", String(SOURCE_WIDTH_MINIMUM));
    sourceResizer.setAttribute("aria-valuemax", String(SOURCE_WIDTH_MAXIMUM));
    sourceResizer.setAttribute(
      "aria-valuetext",
      `Sources width ${sourceWidth} pixels, from ${SOURCE_WIDTH_MINIMUM} to ${SOURCE_WIDTH_MAXIMUM}`,
    );
  };

  const syncExpanded = (): void => {
    if (isModal())
      sourceToggle.setAttribute("aria-expanded", String(sourceDialog.open));
    else sourceToggle.removeAttribute("aria-expanded");
    photoSourceToggle.setAttribute("aria-expanded", String(sourceDialog.open));
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
    sourceToggle.hidden = false;
    photoSourceToggle.hidden = !modal;
    if (!modal) close(false);
    syncExpanded();
  };

  const open = (): void => {
    if (!alive) return;
    if (!isModal()) {
      sourceDialog.querySelector<HTMLElement>("[data-source-list] a")?.focus();
      return;
    }
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
      // A keyboard resize must announce its resulting width. The separator
      // itself exposes that value, so the announcement reads the same owner
      // value that just moved the Sources width.
      announce(sourceWidthAnnouncement(sourceWidth));
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
  setSourceWidth(SOURCE_WIDTH_DEFAULT);
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
