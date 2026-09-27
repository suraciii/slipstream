export interface PhotoZoomController {
  isManual(): boolean;
  hasMeasurableImage(): boolean;
  applyZoom(): void;
  applyFit(): void;
  applyManualZoom(percent: number): void;
  zoomIn(): void;
  zoomOut(): void;
  toggleDetail(): void;
  resetForImage(): void;
  clampPan(): void;
  panBy(x: number, y: number): void;
  trackPointer(id: number, x: number, y: number): number;
  updatePointer(id: number, x: number, y: number): void;
  removePointer(id: number): number;
  beginPinch(): void;
  updatePinch(): void;
  isPinching(): boolean;
  endPinch(): void;
  resetPointers(): void;
  dispose(): void;
}

const MIN_ZOOM_PERCENT = 10;
const MAX_ZOOM_PERCENT = 800;
const ZOOM_STEP = 1.25;
const DETAIL_ZOOM_PERCENT = 200;
const ZOOM_LEVEL_UNMEASURED = "—";

export function createPhotoZoomController({
  preview,
  stage,
  controls,
  fit,
  out,
  inButton,
  hundred,
  slider,
  level,
  isAlive,
}: Readonly<{
  preview: HTMLElement;
  stage: HTMLElement;
  controls: HTMLElement;
  fit: HTMLButtonElement;
  out: HTMLButtonElement;
  inButton: HTMLButtonElement;
  hundred: HTMLButtonElement;
  slider: HTMLInputElement;
  level: HTMLElement;
  isAlive: () => boolean;
}>): PhotoZoomController {
  let zoomManual = false;
  let zoomPercent = 100;
  let panX = 0;
  let panY = 0;
  let imageNaturalWidth = 0;
  let imageNaturalHeight = 0;
  const activePointers = new Map<number, { x: number; y: number }>();
  type PinchGesture = Readonly<{
    startPercent: number;
    startScale: number;
    startDistance: number;
    offsetX: number;
    offsetY: number;
  }>;
  let pinch: PinchGesture | undefined;
  let alive = true;
  const listeners = new AbortController();

  const clamp = (value: number, low: number, high: number): number =>
    Math.max(low, Math.min(high, value));
  const stageCenter = () => {
    const box = stage.getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
  };
  const fitScale = () => {
    if (!imageNaturalWidth || !imageNaturalHeight) return 0;
    const width = stage.clientWidth;
    const height = stage.clientHeight;
    if (!width || !height) return 0;
    return Math.min(width / imageNaturalWidth, height / imageNaturalHeight);
  };
  const currentScale = () => (zoomManual ? zoomPercent / 100 : fitScale());
  const currentPercent = () => {
    const scale = currentScale();
    return scale > 0 ? scale * 100 : zoomPercent;
  };
  const panLimitX = () =>
    Math.max(0, (imageNaturalWidth * currentScale() - stage.clientWidth) / 2);
  const panLimitY = () =>
    Math.max(0, (imageNaturalHeight * currentScale() - stage.clientHeight) / 2);
  const clampPan = (): void => {
    panX = clamp(panX, -panLimitX(), panLimitX());
    panY = clamp(panY, -panLimitY(), panLimitY());
  };
  // Zoom acts on the Preview's own pixels. A retained Preview can be shown
  // again without a new load event, so the size is measured from the image
  // element whenever the cached measurement is missing, and a Preview whose
  // bytes never arrived has no pixels to measure at all.
  const loadedPreviewImage = (): HTMLImageElement | undefined => {
    const image = stage.querySelector<HTMLImageElement>("img");
    if (!image || !image.complete || image.naturalWidth === 0) return undefined;
    if (!imageNaturalWidth || !imageNaturalHeight) {
      imageNaturalWidth = image.naturalWidth;
      imageNaturalHeight = image.naturalHeight;
    }
    return image;
  };
  const hasMeasurableImage = (): boolean => Boolean(loadedPreviewImage());
  const syncZoomControls = (): void => {
    const enabled = alive && isAlive() && hasMeasurableImage();
    for (const button of [fit, out, inButton, hundred])
      button.disabled = !enabled;
    slider.disabled = !enabled;
  };
  const applyZoom = (): void => {
    if (!alive || !isAlive()) return;
    preview.dataset.zoomState = zoomManual ? "manual" : "fit";
    fit.setAttribute("aria-pressed", String(!zoomManual));
    const image = loadedPreviewImage();
    const scale = currentScale();
    if (!image || !imageNaturalWidth || !imageNaturalHeight || scale <= 0) {
      // Without measurable pixels there is no percentage to report: the
      // presentation returns to Fit instead of keeping a stale manual value.
      level.textContent = ZOOM_LEVEL_UNMEASURED;
      slider.value = String(MIN_ZOOM_PERCENT);
      slider.setAttribute("aria-valuetext", "Fit");
      syncZoomControls();
      return;
    }
    image.style.width = `${imageNaturalWidth * scale}px`;
    image.style.height = `${imageNaturalHeight * scale}px`;
    // The Preview is centered on the stage center, so the stage center is the
    // origin that pan and pointer-anchored zoom are measured from.
    image.style.transform = `translate(${panX}px, ${panY}px)`;
    const percent = Math.round(scale * 100);
    // A Fit can land below the manual floor on a large derivative: the label
    // reports it truthfully while the slider keeps the value it can hold.
    const sliderPercent = clamp(percent, MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT);
    level.textContent = `${percent}%`;
    slider.value = String(sliderPercent);
    slider.setAttribute("aria-valuetext", `${sliderPercent}%`);
    syncZoomControls();
  };
  const applyFit = (): void => {
    if (!alive || !isAlive()) return;
    zoomManual = false;
    panX = 0;
    panY = 0;
    applyZoom();
  };
  const applyManualZoom = (percent: number): void => {
    if (!alive || !isAlive()) return;
    zoomManual = true;
    zoomPercent = clamp(percent, MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT);
    clampPan();
    applyZoom();
  };
  const zoomBy = (factor: number): void => {
    const current = currentPercent();
    const base = zoomManual ? zoomPercent : Math.max(MIN_ZOOM_PERCENT, current);
    const target = clamp(base * factor, MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT);
    // Stepping stays monotonic: the manual floor cannot magnify a Fit that
    // sits below it by zooming out, and it cannot shrink a 800% manual zoom
    // by zooming in.
    if (factor < 1 ? target >= current : target <= current) return;
    applyManualZoom(target);
  };
  const zoomOut = (): void => zoomBy(1 / ZOOM_STEP);
  const zoomIn = (): void => zoomBy(ZOOM_STEP);
  /// Changes zoom while keeping the image point under `anchor` (client
  /// coordinates) stationary, then re-clamps the bounded pan.
  const zoomAt = (
    percent: number,
    anchor: Readonly<{ x: number; y: number }>,
  ): void => {
    if (!alive || !isAlive()) return;
    const nextPercent = clamp(percent, MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT);
    const previousScale = currentScale();
    const center = stageCenter();
    const offsetX = anchor.x - (center.x + panX);
    const offsetY = anchor.y - (center.y + panY);
    if (previousScale > 0 && imageNaturalWidth > 0) {
      const ratio = nextPercent / 100 / previousScale;
      panX = anchor.x - center.x - offsetX * ratio;
      panY = anchor.y - center.y - offsetY * ratio;
    }
    zoomManual = true;
    zoomPercent = nextPercent;
    clampPan();
    applyZoom();
  };
  const resetForImage = (): void => {
    if (!alive || !isAlive()) return;
    zoomManual = false;
    zoomPercent = 100;
    panX = 0;
    panY = 0;
    imageNaturalWidth = 0;
    imageNaturalHeight = 0;
    applyZoom();
  };
  const toggleDetail = (): void => {
    if (!alive || !isAlive() || !hasMeasurableImage()) return;
    if (zoomManual && Math.abs(zoomPercent - DETAIL_ZOOM_PERCENT) < 0.5)
      applyFit();
    else applyManualZoom(DETAIL_ZOOM_PERCENT);
  };
  const beginPinch = (): void => {
    const [first, second] = Array.from(activePointers.values());
    const scale = currentScale();
    if (!first || !second || scale <= 0 || !imageNaturalWidth) {
      pinch = undefined;
      return;
    }
    const midpoint = {
      x: (first.x + second.x) / 2,
      y: (first.y + second.y) / 2,
    };
    const center = stageCenter();
    pinch = {
      startPercent: currentPercent(),
      startScale: scale,
      startDistance: Math.max(
        1,
        Math.hypot(first.x - second.x, first.y - second.y),
      ),
      offsetX: midpoint.x - (center.x + panX),
      offsetY: midpoint.y - (center.y + panY),
    };
    for (const id of activePointers.keys()) {
      try {
        preview.setPointerCapture(id);
      } catch {
        // Synthetic PointerEvents have no native active pointer to capture.
      }
    }
    preview.style.touchAction = "none";
  };
  const updatePinch = (): void => {
    const active = pinch;
    const [first, second] = Array.from(activePointers.values());
    if (!active || !first || !second) return;
    const midpoint = {
      x: (first.x + second.x) / 2,
      y: (first.y + second.y) / 2,
    };
    const distance = Math.max(
      1,
      Math.hypot(first.x - second.x, first.y - second.y),
    );
    const nextPercent = clamp(
      active.startPercent * (distance / active.startDistance),
      MIN_ZOOM_PERCENT,
      MAX_ZOOM_PERCENT,
    );
    const ratio = nextPercent / 100 / active.startScale;
    const center = stageCenter();
    zoomManual = true;
    zoomPercent = nextPercent;
    panX = midpoint.x - center.x - active.offsetX * ratio;
    panY = midpoint.y - center.y - active.offsetY * ratio;
    clampPan();
    applyZoom();
  };
  const onWheel = (event: WheelEvent): void => {
    if (!alive || !isAlive() || !hasMeasurableImage()) return;
    event.preventDefault();
    const factor = event.deltaY < 0 ? ZOOM_STEP : 1 / ZOOM_STEP;
    const base = zoomManual
      ? zoomPercent
      : Math.max(MIN_ZOOM_PERCENT, currentPercent());
    zoomAt(base * factor, { x: event.clientX, y: event.clientY });
  };

  controls.addEventListener("pointerdown", (event) => event.stopPropagation(), {
    signal: listeners.signal,
  });
  controls.addEventListener("pointermove", (event) => event.stopPropagation(), {
    signal: listeners.signal,
  });
  controls.addEventListener("pointerup", (event) => event.stopPropagation(), {
    signal: listeners.signal,
  });
  fit.addEventListener("click", applyFit, { signal: listeners.signal });
  out.addEventListener("click", () => zoomOut(), {
    signal: listeners.signal,
  });
  inButton.addEventListener("click", () => zoomIn(), {
    signal: listeners.signal,
  });
  hundred.addEventListener("click", () => applyManualZoom(100), {
    signal: listeners.signal,
  });
  slider.addEventListener(
    "input",
    () => applyManualZoom(Number(slider.value)),
    { signal: listeners.signal },
  );
  stage.addEventListener("dblclick", toggleDetail, {
    signal: listeners.signal,
  });
  preview.addEventListener("wheel", onWheel, {
    passive: false,
    signal: listeners.signal,
  });

  return {
    isManual: () => zoomManual,
    hasMeasurableImage,
    applyZoom,
    applyFit,
    applyManualZoom,
    zoomIn,
    zoomOut,
    toggleDetail,
    resetForImage,
    clampPan,
    panBy(x, y) {
      if (!alive || !isAlive()) return;
      panX = clamp(panX + x, -panLimitX(), panLimitX());
      panY = clamp(panY + y, -panLimitY(), panLimitY());
      applyZoom();
    },
    trackPointer(id, x, y) {
      activePointers.set(id, { x, y });
      return activePointers.size;
    },
    updatePointer(id, x, y) {
      const tracked = activePointers.get(id);
      if (tracked) {
        tracked.x = x;
        tracked.y = y;
      }
    },
    removePointer(id) {
      activePointers.delete(id);
      return activePointers.size;
    },
    beginPinch,
    updatePinch,
    isPinching: () => pinch !== undefined,
    endPinch() {
      pinch = undefined;
      preview.style.removeProperty("touch-action");
    },
    resetPointers() {
      for (const id of activePointers.keys())
        if (preview.hasPointerCapture(id)) preview.releasePointerCapture(id);
      activePointers.clear();
      pinch = undefined;
      preview.style.removeProperty("touch-action");
    },
    dispose() {
      alive = false;
      listeners.abort();
      activePointers.clear();
      pinch = undefined;
    },
  };
}
