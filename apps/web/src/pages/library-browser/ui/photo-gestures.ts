import type { LibraryBrowserIntent } from "./library-browser-view.js";
import type { PhotoZoomController } from "./photo-zoom.js";
import type { RatingControls } from "./rating-controls.js";

type PhotoMutationIntent = Extract<
  LibraryBrowserIntent,
  { kind: "photo-mutation" }
>;
type PhotoGestureIntent =
  | PhotoMutationIntent
  | Readonly<{ kind: "previous" | "next" }>;

const SWIPE_PENDING_PIXELS = 24;
const SWIPE_COMMIT_PIXELS = 72;
const SWIPE_COMMIT_VELOCITY = 0.5;
const RATING_WHEEL_HOLD_MS = 450;
const RATING_WHEEL_MOVE_PIXELS = 12;

export function isPaintedImagePointer(
  event: PointerEvent,
  stage: HTMLElement,
): boolean {
  if (
    !(event.target instanceof HTMLImageElement) ||
    !stage.contains(event.target)
  )
    return false;
  const image = event.target;
  const box = image.getBoundingClientRect();
  const naturalWidth = image.naturalWidth;
  const naturalHeight = image.naturalHeight;
  if (!naturalWidth || !naturalHeight || !box.width || !box.height)
    return false;
  const scale = Math.min(box.width / naturalWidth, box.height / naturalHeight);
  const paintedWidth = naturalWidth * scale;
  const paintedHeight = naturalHeight * scale;
  return (
    event.clientX >= box.left + (box.width - paintedWidth) / 2 &&
    event.clientX <= box.right - (box.width - paintedWidth) / 2 &&
    event.clientY >= box.top + (box.height - paintedHeight) / 2 &&
    event.clientY <= box.bottom - (box.height - paintedHeight) / 2
  );
}

export interface PhotoGestures {
  reset(): void;
  cancelUnavailableDecision(): void;
  dispose(): void;
}
type GestureAxis = "horizontal" | "vertical";
type PointerState = {
  id: number;
  startX: number;
  startY: number;
  lastX: number;
  lastY: number;
  startedAt: number;
  axis: GestureAxis | undefined;
  axisSettled: boolean;
  ratingPending: boolean;
  ratingWheel: boolean;
  pan: boolean;
  surface: object;
  photoId: string;
};
export function createPhotoGestures({
  preview,
  stage,
  pickFeedback,
  rejectFeedback,
  zoom,
  rating,
  isAlive,
  isRenderedImage,
  currentPhotoId,
  currentSurface,
  decisionEnabled,
  send,
}: Readonly<{
  preview: HTMLElement;
  stage: HTMLElement;
  pickFeedback: HTMLElement;
  rejectFeedback: HTMLElement;
  zoom: PhotoZoomController;
  rating: RatingControls;
  isAlive: () => boolean;
  isRenderedImage: (event: PointerEvent) => boolean;
  currentPhotoId: () => string | undefined;
  currentSurface: () => object;
  decisionEnabled: () => boolean;
  send: (intent: PhotoGestureIntent) => void;
}>): PhotoGestures {
  let pointer: PointerState | undefined;
  let holdTimer: number | undefined;
  let alive = true;
  const listeners = new AbortController();
  const cancelHold = () => {
    if (holdTimer !== undefined) window.clearTimeout(holdTimer);
    holdTimer = undefined;
  };
  const feedback = (element: HTMLElement, label = "") => {
    element.hidden = !label;
    element.textContent = label;
    element.classList.toggle("pending", !!label);
    element.classList.toggle(
      "navigation",
      label === "Next" || label === "Previous",
    );
  };
  const clearPointer = (releaseCapture = true) => {
    cancelHold();
    const id = pointer?.id;
    pointer = undefined;
    if (releaseCapture && id !== undefined && preview.hasPointerCapture(id))
      preview.releasePointerCapture(id);
    stage.style.transform = "";
    feedback(pickFeedback);
    feedback(rejectFeedback);
  };
  const reset = () => {
    cancelHold();
    rating.closeWheel();
    zoom.resetPointers();
    clearPointer();
  };
  const cancelUnavailableDecision = () => {
    if (
      pointer?.axis === "horizontal" ||
      pointer?.ratingPending ||
      pointer?.ratingWheel ||
      rating.isWheelOpen()
    )
      reset();
  };
  const pointerDown = (event: PointerEvent) => {
    const photoId = currentPhotoId();
    if (!alive || !isAlive() || !photoId) return;
    const secondPointer = pointer !== undefined;
    const pointerCount = zoom.trackPointer(
      event.pointerId,
      event.clientX,
      event.clientY,
    );
    if (pointerCount === 2) {
      event.preventDefault();
      cancelHold();
      rating.closeWheel();
      clearPointer(false);
      zoom.beginPinch();
      return;
    }
    if (!secondPointer && !isRenderedImage(event)) return;
    if (
      pointerCount > 2 ||
      zoom.isPinching() ||
      (pointer && !secondPointer) ||
      !event.isPrimary
    )
      return;
    if (!secondPointer && !zoom.isManual() && !zoom.hasMeasurableImage())
      return;
    const ratingPending =
      event.pointerType === "touch" &&
      !zoom.isManual() &&
      zoom.hasMeasurableImage() &&
      decisionEnabled();
    pointer = {
      id: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      startedAt: event.timeStamp,
      axis: undefined,
      axisSettled: false,
      ratingPending,
      ratingWheel: false,
      pan: zoom.isManual(),
      surface: currentSurface(),
      photoId,
    };
    try {
      preview.setPointerCapture(event.pointerId);
    } catch {
      // Synthetic qualification events have no native pointer to capture.
    }
    const id = event.pointerId;
    const active = pointer;
    if (!active) return;
    const surface = active.surface;
    holdTimer = window.setTimeout(() => {
      const active = pointer;
      holdTimer = undefined;
      if (
        !active ||
        active.id !== id ||
        !active.ratingPending ||
        active.surface !== surface ||
        active.photoId !== photoId ||
        active.axisSettled ||
        active.axis !== undefined ||
        active.pan ||
        zoom.isManual() ||
        !currentPhotoId() ||
        currentPhotoId() !== photoId
      )
        return;
      active.ratingPending = false;
      active.ratingWheel = true;
      stage.style.transform = "";
      feedback(pickFeedback);
      feedback(rejectFeedback);
      rating.openWheel(active.lastX, active.lastY);
      rating.updateWheel(active.lastX, active.lastY);
    }, RATING_WHEEL_HOLD_MS);
  };
  const pointerMove = (event: PointerEvent) => {
    if (!alive || !isAlive()) return;
    zoom.updatePointer(event.pointerId, event.clientX, event.clientY);
    if (zoom.isPinching()) {
      zoom.updatePinch();
      return;
    }
    if (!pointer || pointer.id !== event.pointerId) return;
    const dx = event.clientX - pointer.startX;
    const dy = event.clientY - pointer.startY;
    const stepX = event.clientX - pointer.lastX;
    const stepY = event.clientY - pointer.lastY;
    pointer.lastX = event.clientX;
    pointer.lastY = event.clientY;
    if (pointer.ratingWheel) {
      rating.updateWheel(event.clientX, event.clientY);
      return;
    }
    if (pointer.pan || zoom.isManual()) {
      feedback(pickFeedback);
      feedback(rejectFeedback);
      zoom.panBy(stepX, stepY);
      return;
    }
    const distance = Math.hypot(dx, dy);
    if (!pointer.axisSettled && distance > RATING_WHEEL_MOVE_PIXELS) {
      pointer.ratingPending = false;
      cancelHold();
      pointer.axisSettled = true;
      if (Math.abs(dx) !== Math.abs(dy))
        pointer.axis = Math.abs(dx) > Math.abs(dy) ? "horizontal" : "vertical";
    }
    if (pointer.axis === "vertical") {
      stage.style.transform = `translateY(${Math.max(-140, Math.min(140, dy))}px)`;
      feedback(pickFeedback, dy < -SWIPE_PENDING_PIXELS ? "Next" : "");
      feedback(rejectFeedback, dy > SWIPE_PENDING_PIXELS ? "Previous" : "");
      return;
    }
    if (pointer.axis === "horizontal") {
      stage.style.transform = `translateX(${Math.max(-140, Math.min(140, dx))}px)`;
      feedback(
        pickFeedback,
        decisionEnabled() && dx > SWIPE_PENDING_PIXELS ? "Pick" : "",
      );
      feedback(
        rejectFeedback,
        decisionEnabled() && dx < -SWIPE_PENDING_PIXELS ? "Reject" : "",
      );
    }
  };
  const finishPointer = (event: PointerEvent, cancelled = false) => {
    if (!alive || !isAlive()) return;
    const remaining = zoom.removePointer(event.pointerId);
    if (zoom.isPinching()) {
      // The remaining finger cannot become a decision swipe after a pinch.
      if (remaining >= 2) return;
      zoom.endPinch();
      return;
    }
    if (!pointer || pointer.id !== event.pointerId) return;
    const active = pointer;
    const wheelCandidate = active.ratingWheel ? rating.candidate() : undefined;
    if (active.ratingWheel) rating.closeWheel();
    clearPointer();
    if (active.ratingWheel) {
      if (
        cancelled ||
        wheelCandidate === undefined ||
        !decisionEnabled() ||
        active.surface !== currentSurface() ||
        active.photoId !== currentPhotoId()
      )
        return;
      send({
        kind: "photo-mutation",
        field: "rating",
        value: wheelCandidate,
        advance: false,
      });
      return;
    }
    if (
      active.pan ||
      zoom.isManual() ||
      cancelled ||
      active.surface !== currentSurface() ||
      active.photoId !== currentPhotoId() ||
      active.axis === undefined
    )
      return;
    const dx = event.clientX - active.startX;
    const dy = event.clientY - active.startY;
    const distance = active.axis === "horizontal" ? Math.abs(dx) : Math.abs(dy);
    const elapsed = Math.max(1, event.timeStamp - active.startedAt);
    const velocity = distance / elapsed;
    if (
      distance < SWIPE_COMMIT_PIXELS &&
      !(distance >= 48 && velocity >= SWIPE_COMMIT_VELOCITY)
    )
      return;
    if (active.axis === "vertical") {
      send({ kind: dy < 0 ? "next" : "previous" });
      return;
    }
    if (!decisionEnabled()) return;
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: dx > 0 ? "picked" : "rejected",
      advance: true,
    });
  };
  const onContextMenu = (event: MouseEvent) => {
    if (pointer?.ratingPending || rating.isWheelOpen()) event.preventDefault();
  };
  preview.addEventListener("pointerdown", pointerDown, {
    signal: listeners.signal,
  });
  preview.addEventListener("pointermove", pointerMove, {
    signal: listeners.signal,
  });
  preview.addEventListener("pointerup", (event) => finishPointer(event), {
    signal: listeners.signal,
  });
  preview.addEventListener(
    "pointercancel",
    (event) => finishPointer(event, true),
    { signal: listeners.signal },
  );
  preview.addEventListener(
    "lostpointercapture",
    (event) => finishPointer(event, true),
    { signal: listeners.signal },
  );
  preview.addEventListener("contextmenu", onContextMenu, {
    signal: listeners.signal,
  });
  return {
    reset,
    cancelUnavailableDecision,
    dispose() {
      if (!alive) return;
      alive = false;
      reset();
      listeners.abort();
    },
  };
}
