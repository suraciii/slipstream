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

export interface PhotoGestures {
  reset(): void;
  cancelUnavailableDecision(): void;
  dispose(): void;
}
type PointerState = {
  id: number;
  startX: number;
  startY: number;
  lastX: number;
  lastY: number;
  startedAt: number;
  vertical: boolean;
  ratingPending: boolean;
  ratingWheel: boolean;
  pan: boolean;
  surface: object;
  photoId: string;
};

export function createPhotoGestures({
  preview,
  stage,
  selectFeedback,
  rejectFeedback,
  zoom,
  rating,
  isAlive,
  currentPhotoId,
  currentSelectionState,
  currentSurface,
  decisionEnabled,
  send,
}: Readonly<{
  preview: HTMLElement;
  stage: HTMLElement;
  selectFeedback: HTMLElement;
  rejectFeedback: HTMLElement;
  zoom: PhotoZoomController;
  rating: RatingControls;
  isAlive: () => boolean;
  currentPhotoId: () => string | undefined;
  currentSelectionState?: () => "unflagged" | "picked" | "rejected";
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
  const clearPointer = () => {
    cancelHold();
    const id = pointer?.id;
    if (id !== undefined && preview.hasPointerCapture(id))
      preview.releasePointerCapture(id);
    pointer = undefined;
    stage.style.transform = "";
    selectFeedback.classList.remove("pending");
    rejectFeedback.classList.remove("pending");
    selectFeedback.textContent = "Next";
    rejectFeedback.textContent = "Previous";
  };
  const reset = () => {
    cancelHold();
    rating.closeWheel();
    zoom.resetPointers();
    clearPointer();
  };
  const cancelUnavailableDecision = () => {
    if (pointer?.ratingPending || pointer?.ratingWheel || rating.isWheelOpen())
      reset();
  };
  const pointerDown = (event: PointerEvent) => {
    if (!alive || !isAlive() || !currentPhotoId()) return;
    const pointerCount = zoom.trackPointer(
      event.pointerId,
      event.clientX,
      event.clientY,
    );
    if (pointerCount === 2) {
      event.preventDefault();
      cancelHold();
      rating.closeWheel();
      clearPointer();
      zoom.beginPinch();
      return;
    }
    if (pointerCount > 2 || zoom.isPinching() || pointer || !event.isPrimary)
      return;
    if (!zoom.isManual() && !zoom.hasMeasurableImage()) return;
    const ratingPending =
      event.pointerType === "touch" &&
      !zoom.isManual() &&
      zoom.hasMeasurableImage() &&
      decisionEnabled();
    const photoId = currentPhotoId();
    if (!photoId) return;
    pointer = {
      id: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      startedAt: event.timeStamp,
      vertical: false,
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
    if (!ratingPending) return;
    const id = event.pointerId;
    const surface = pointer.surface;
    holdTimer = window.setTimeout(() => {
      holdTimer = undefined;
      const active = pointer;
      if (
        !active ||
        active.id !== id ||
        !active.ratingPending ||
        active.surface !== surface ||
        active.photoId !== photoId ||
        active.vertical ||
        active.pan ||
        zoom.isManual() ||
        !decisionEnabled() ||
        !currentPhotoId()
      )
        return;
      active.ratingPending = false;
      active.ratingWheel = true;
      stage.style.transform = "";
      selectFeedback.classList.remove("pending");
      rejectFeedback.classList.remove("pending");
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
    const dx = event.clientX - pointer.startX,
      dy = event.clientY - pointer.startY;
    const stepX = event.clientX - pointer.lastX,
      stepY = event.clientY - pointer.lastY;
    pointer.lastX = event.clientX;
    pointer.lastY = event.clientY;
    if (pointer.ratingWheel) {
      rating.updateWheel(event.clientX, event.clientY);
      return;
    }
    if (pointer.pan || zoom.isManual()) {
      zoom.panBy(stepX, stepY);
      return;
    }
    if (
      pointer.ratingPending &&
      Math.hypot(dx, dy) > RATING_WHEEL_MOVE_PIXELS
    ) {
      pointer.ratingPending = false;
      cancelHold();
      // Equal movement belongs to native scrolling, not a decision swipe.
      if (Math.abs(dy) >= Math.abs(dx)) pointer.vertical = true;
    } else if (
      Math.abs(dy) > Math.abs(dx) &&
      Math.abs(dy) > RATING_WHEEL_MOVE_PIXELS
    )
      pointer.vertical = true;
    if (pointer.vertical) {
      const current = currentSelectionState?.() ?? "unflagged";
      const forward = {
        unflagged: "Picked",
        picked: "Rejected",
        rejected: "Unflagged",
      } as const;
      const reverse = {
        unflagged: "Rejected",
        picked: "Unflagged",
        rejected: "Picked",
      } as const;
      stage.style.transform = `translateY(${Math.max(-140, Math.min(140, dy))}px)`;
      selectFeedback.textContent = dy < 0 ? forward[current] : "";
      rejectFeedback.textContent = dy > 0 ? reverse[current] : "";
      selectFeedback.classList.toggle("pending", dy < -SWIPE_PENDING_PIXELS);
      rejectFeedback.classList.toggle("pending", dy > SWIPE_PENDING_PIXELS);
      return;
    }
    stage.style.transform = `translateX(${Math.max(-140, Math.min(140, dx))}px)`;
    selectFeedback.textContent = "Next";
    rejectFeedback.textContent = "Previous";
    selectFeedback.classList.toggle("pending", dx < -SWIPE_PENDING_PIXELS);
    rejectFeedback.classList.toggle("pending", dx > SWIPE_PENDING_PIXELS);
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
      active.photoId !== currentPhotoId()
    )
      return;
    const dx = event.clientX - active.startX;
    const dy = event.clientY - active.startY;
    const horizontal = !active.vertical;
    if (!horizontal && !decisionEnabled()) return;
    const distance = horizontal ? Math.abs(dx) : Math.abs(dy);
    const elapsed = Math.max(1, event.timeStamp - active.startedAt);
    const velocity = distance / elapsed;
    if (
      distance < SWIPE_COMMIT_PIXELS &&
      !(distance >= 48 && velocity >= SWIPE_COMMIT_VELOCITY)
    )
      return;
    if (horizontal) {
      send({ kind: dx < 0 ? "next" : "previous" });
      return;
    }
    const current = currentSelectionState?.() ?? "unflagged";
    const forward = {
      unflagged: "picked",
      picked: "rejected",
      rejected: "unflagged",
    } as const;
    const reverse = {
      unflagged: "rejected",
      picked: "unflagged",
      rejected: "picked",
    } as const;
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: dy < 0 ? forward[current] : reverse[current],
      advance: false,
    });
    return;
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
