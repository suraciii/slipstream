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
  refreshFeedback(): void;
  cancelFitGesture(): void;
  dispose(): void;
}
type GestureAxis = "horizontal" | "vertical";
export type NavigationFeedback = "enabled" | "boundary" | "disabled";
export type NavigationAvailability = Readonly<{
  previous: NavigationFeedback;
  next: NavigationFeedback;
}>;
export type PhotoGestureFeedback = Readonly<{
  pick: HTMLElement;
  reject: HTMLElement;
  next: HTMLElement;
  previous: HTMLElement;
  navigationAvailability: () => NavigationAvailability;
}>;
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
  gestureFeedback,
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
  gestureFeedback: PhotoGestureFeedback;
  zoom: PhotoZoomController;
  rating: RatingControls;
  isAlive: () => boolean;
  isRenderedImage: (event: PointerEvent) => boolean;
  currentPhotoId: () => string | undefined;
  currentSurface: () => object;
  decisionEnabled: () => boolean;
  send: (intent: PhotoGestureIntent) => void;
}>): PhotoGestures {
  const {
    pick: pickFeedback,
    reject: rejectFeedback,
    next: nextFeedback,
    previous: previousFeedback,
    navigationAvailability,
  } = gestureFeedback;
  let pointer: PointerState | undefined;
  let holdTimer: number | undefined;
  let readyExpiry: number | undefined;
  let alive = true;
  const listeners = new AbortController();
  const cancelHold = () => {
    if (holdTimer !== undefined) window.clearTimeout(holdTimer);
    holdTimer = undefined;
  };
  const cancelReadyExpiry = () => {
    if (readyExpiry !== undefined) window.clearTimeout(readyExpiry);
    readyExpiry = undefined;
  };
  const feedback = (
    element: HTMLElement,
    label = "",
    state: Readonly<{ ready?: boolean; boundary?: boolean }> = {},
  ) => {
    element.hidden = !label;
    element.textContent = label;
    element.classList.toggle("pending", !!label);
    element.classList.toggle("ready", !!label && Boolean(state.ready));
    element.classList.toggle("boundary", !!label && Boolean(state.boundary));
  };
  const clearFeedback = () => {
    cancelReadyExpiry();
    feedback(pickFeedback);
    feedback(rejectFeedback);
    feedback(nextFeedback);
    feedback(previousFeedback);
  };
  const commitReady = (distance: number, elapsed: number) =>
    distance >= SWIPE_COMMIT_PIXELS ||
    (distance >= 48 &&
      distance / Math.max(1, elapsed) >= SWIPE_COMMIT_VELOCITY);
  const scheduleReadyExpiry = (
    element: HTMLElement,
    label: string,
    distance: number,
    elapsed: number,
  ) => {
    cancelReadyExpiry();
    if (distance >= SWIPE_COMMIT_PIXELS) return;
    const remaining = distance / SWIPE_COMMIT_VELOCITY - elapsed;
    readyExpiry = window.setTimeout(
      () => {
        readyExpiry = undefined;
        if (!alive || !isAlive()) return;
        feedback(element, label);
      },
      Math.max(1, Math.ceil(remaining) + 1),
    );
  };
  const renderVerticalFeedback = (dy: number, elapsed: number) => {
    if (!pointer) return;
    cancelReadyExpiry();
    const direction = dy < 0 ? "next" : "previous";
    const state = navigationAvailability()[direction];
    const label =
      state === "boundary"
        ? direction === "next"
          ? "Last photo"
          : "First photo"
        : direction === "next"
          ? "Next"
          : "Previous";
    const distance = Math.abs(dy);
    const active = state !== "disabled" && distance >= SWIPE_PENDING_PIXELS;
    const ready = state === "enabled" && commitReady(distance, elapsed);
    const feedbackElement =
      direction === "next" ? nextFeedback : previousFeedback;
    const otherElement = direction === "next" ? previousFeedback : nextFeedback;
    feedback(feedbackElement, active ? label : "", {
      ready,
      boundary: state === "boundary",
    });
    feedback(otherElement);
    if (ready) scheduleReadyExpiry(feedbackElement, label, distance, elapsed);
  };
  const clearPointer = (releaseCapture = true) => {
    cancelHold();
    const id = pointer?.id;
    pointer = undefined;
    if (releaseCapture && id !== undefined && preview.hasPointerCapture(id))
      preview.releasePointerCapture(id);
    stage.style.transform = "";
    clearFeedback();
  };
  const reset = () => {
    cancelHold();
    rating.closeWheel();
    zoom.resetPointers();
    clearPointer();
  };
  const cancelFitGesture = () => {
    if (pointer && !pointer.pan) reset();
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
  const refreshFeedback = () => {
    if (!alive || !isAlive()) return;
    if (!decisionEnabled()) cancelUnavailableDecision();
    if (
      !pointer ||
      pointer.axis !== "vertical" ||
      pointer.pan ||
      pointer.ratingWheel ||
      zoom.isManual()
    )
      return;
    renderVerticalFeedback(
      pointer.lastY - pointer.startY,
      performance.now() - pointer.startedAt,
    );
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
      clearFeedback();
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
      clearFeedback();
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
    const elapsed = event.timeStamp - pointer.startedAt;
    if (pointer.axis === "vertical") {
      stage.style.transform = `translateY(${Math.max(-140, Math.min(140, dy))}px)`;
      renderVerticalFeedback(dy, elapsed);
      return;
    }
    if (pointer.axis === "horizontal") {
      stage.style.transform = `translateX(${Math.max(-140, Math.min(140, dx))}px)`;
      cancelReadyExpiry();
      const distance = Math.abs(dx);
      const active = decisionEnabled() && distance >= SWIPE_PENDING_PIXELS;
      const ready = active && commitReady(distance, elapsed);
      feedback(pickFeedback, active && dx > 0 ? "Pick" : "", {
        ready: ready && dx > 0,
      });
      feedback(rejectFeedback, active && dx < 0 ? "Reject" : "", {
        ready: ready && dx < 0,
      });
      if (ready)
        scheduleReadyExpiry(
          dx > 0 ? pickFeedback : rejectFeedback,
          dx > 0 ? "Pick" : "Reject",
          distance,
          elapsed,
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
    if (!commitReady(distance, elapsed)) return;
    const lastX = active.lastX - active.startX;
    const lastY = active.lastY - active.startY;
    if (active.axis === "vertical") {
      const direction = dy < 0 ? "next" : "previous";
      const lastDirection = lastY < 0 ? "next" : "previous";
      if (lastY !== 0 && direction !== lastDirection) return;
      if (navigationAvailability()[direction] !== "enabled") return;
      send({ kind: direction });
      return;
    }
    if (!decisionEnabled()) return;
    const direction = dx > 0 ? "picked" : "rejected";
    const lastDirection = lastX > 0 ? "picked" : "rejected";
    if (lastX !== 0 && direction !== lastDirection) return;
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: direction,
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
    refreshFeedback,
    cancelFitGesture,
    dispose() {
      if (!alive) return;
      alive = false;
      reset();
      listeners.abort();
    },
  };
}
