import type { LibraryBrowserIntent } from "./library-browser-view.js";
import type { ModalSurfaces } from "./modal-surface.js";

type RatingIntent = Extract<LibraryBrowserIntent, { kind: "photo-mutation" }>;

const RATING_WHEEL_RADIUS = 78;
const RATING_WHEEL_OPTION_COUNT = 6;

export interface RatingControls {
  render(value: number): void;
  openChoices(): void;
  closeChoices(restoreFocus?: boolean): void;
  openWheel(clientX: number, clientY: number): void;
  updateWheel(clientX: number, clientY: number): void;
  closeWheel(): void;
  isWheelOpen(): boolean;
  candidate(): number | undefined;
  setDecisionEnabled(enabled: boolean): void;
  dispose(): void;
}

export function createRatingControls({
  elements,
  surfaces,
  send,
  onSurfaceChange,
}: Readonly<{
  elements: Readonly<{
    ratingDialog: HTMLDialogElement;
    ratingChoicesClose: HTMLButtonElement;
    ratings: HTMLElement;
    ratingWheel: HTMLElement;
    ratingWheelInstructions: HTMLElement;
    ratingWheelStatus: HTMLElement;
    ratingWheelOptions: HTMLElement;
    preview: HTMLElement;
    rating: HTMLElement;
    dockRating: HTMLButtonElement;
  }>;
  surfaces: ModalSurfaces;
  send: (intent: RatingIntent) => void;
  onSurfaceChange: () => void;
}>): RatingControls {
  const {
    ratingDialog,
    ratingChoicesClose,
    ratings,
    ratingWheel,
    ratingWheelInstructions,
    ratingWheelStatus,
    ratingWheelOptions,
    preview,
    rating,
    dockRating,
  } = elements;
  let alive = true;
  let currentRating = 0;
  let ratingWheelOpen = false;
  let ratingWheelCandidate: number | undefined;
  let ratingWheelCenter: Readonly<{ x: number; y: number }> | undefined;
  let ratingWheelRadius = RATING_WHEEL_RADIUS;
  const listeners = new AbortController();

  const ratingLabel = (value: number): string =>
    value === 0 ? "Clear Rating" : `${value} ${value === 1 ? "star" : "stars"}`;

  const syncRatingWheelOptions = (): void => {
    for (const button of Array.from(
      ratingWheelOptions.querySelectorAll<HTMLButtonElement>(
        "[data-rating-wheel-value]",
      ),
    )) {
      const value = Number(button.dataset.ratingWheelValue);
      button.dataset.current = String(value === currentRating);
      button.setAttribute(
        "aria-current",
        value === currentRating ? "true" : "false",
      );
      button.setAttribute(
        "aria-pressed",
        String(value === ratingWheelCandidate),
      );
    }
  };

  const setRatingWheelCandidate = (value: number | undefined): void => {
    ratingWheelCandidate = value;
    if (value === undefined) {
      ratingWheelStatus.textContent = `Current Rating: ${ratingLabel(currentRating)}. Move across a Rating and release to save.`;
      ratingWheel.dataset.ratingWheelCandidate = "";
    } else {
      ratingWheelStatus.textContent = `${ratingLabel(value)}. Release to save.`;
      ratingWheel.dataset.ratingWheelCandidate = String(value);
    }
    syncRatingWheelOptions();
  };

  const closeRatingWheel = (): void => {
    ratingWheelOpen = false;
    ratingWheelCandidate = undefined;
    ratingWheelCenter = undefined;
    ratingWheel.hidden = true;
    delete ratingWheel.dataset.ratingWheelCandidate;
    preview.classList.remove("rating-wheel-open");
    preview.style.removeProperty("touch-action");
    ratingWheelStatus.textContent = "";
    syncRatingWheelOptions();
  };

  const openRatingWheel = (clientX: number, clientY: number): void => {
    const bounds = preview.getBoundingClientRect();
    const radius = Math.max(
      46,
      Math.min(
        RATING_WHEEL_RADIUS,
        (bounds.width - 96) / 2,
        (bounds.height - 96) / 2,
      ),
    );
    const extent = radius + 28;
    const centerX = clamp(
      clientX - bounds.left,
      Math.min(extent, bounds.width / 2),
      Math.max(bounds.width / 2, bounds.width - extent),
    );
    const centerY = clamp(
      clientY - bounds.top,
      Math.min(extent, bounds.height / 2),
      Math.max(bounds.height / 2, bounds.height - extent),
    );
    ratingWheelRadius = radius;
    ratingWheelCenter = { x: centerX, y: centerY };
    ratingWheel.style.left = `${centerX}px`;
    ratingWheel.style.top = `${centerY}px`;
    ratingWheel.style.setProperty("--rating-wheel-radius", `${radius}px`);
    ratingWheelOpen = true;
    ratingWheel.hidden = false;
    ratingWheelInstructions.textContent =
      "Rating Wheel open. Move across a Rating and release to save.";
    preview.classList.add("rating-wheel-open");
    preview.style.touchAction = "none";
    setRatingWheelCandidate(undefined);
  };

  const updateRatingWheel = (clientX: number, clientY: number): void => {
    if (!ratingWheelOpen || !ratingWheelCenter) return;
    const dx =
      clientX - (preview.getBoundingClientRect().left + ratingWheelCenter.x);
    const dy =
      clientY - (preview.getBoundingClientRect().top + ratingWheelCenter.y);
    const distance = Math.hypot(dx, dy);
    const outer = ratingWheelRadius + 56;
    if (distance < 30 || distance > outer) {
      setRatingWheelCandidate(undefined);
      return;
    }
    const degrees = (Math.atan2(dy, dx) * (180 / Math.PI) + 90 + 360) % 360;
    const value =
      Math.floor(
        (degrees + 180 / RATING_WHEEL_OPTION_COUNT) /
          (360 / RATING_WHEEL_OPTION_COUNT),
      ) % RATING_WHEEL_OPTION_COUNT;
    setRatingWheelCandidate(value);
  };

  surfaces.register("rating", {
    dialog: ratingDialog,
    modal: () => true,
    onOpen: onSurfaceChange,
  });

  for (let value = 0; value <= 5; value += 1) {
    const button = document.createElement("button");
    button.type = "button";
    button.dataset.ratingValue = String(value);
    button.setAttribute(
      "aria-label",
      value === 0
        ? "Clear Rating, 0 stars"
        : `Rate ${value} ${value === 1 ? "star" : "stars"}`,
    );
    button.setAttribute("aria-pressed", String(value === 0));
    button.textContent = value === 0 ? "0" : `${value}★`;
    ratings.append(button);

    const wheelButton = document.createElement("button");
    wheelButton.type = "button";
    wheelButton.className = "rating-wheel-option";
    wheelButton.dataset.ratingWheelValue = String(value);
    wheelButton.style.setProperty(
      "--wheel-angle",
      `${value * (360 / RATING_WHEEL_OPTION_COUNT)}deg`,
    );
    wheelButton.tabIndex = -1;
    wheelButton.setAttribute(
      "aria-label",
      value === 0
        ? "Clear Rating, 0 stars"
        : `${value} ${value === 1 ? "star" : "stars"}`,
    );
    wheelButton.setAttribute("aria-pressed", "false");
    wheelButton.textContent = value === 0 ? "0" : `${value}★`;
    ratingWheelOptions.append(wheelButton);
  }

  ratings.addEventListener(
    "click",
    (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
        "[data-rating-value]",
      );
      if (!button) return;
      send({
        kind: "photo-mutation",
        field: "rating",
        value: Number(button.dataset.ratingValue),
        advance: false,
      });
    },
    { signal: listeners.signal },
  );
  ratingChoicesClose.addEventListener("click", () => closeChoices(), {
    signal: listeners.signal,
  });
  ratingDialog.addEventListener("close", onSurfaceChange, {
    signal: listeners.signal,
  });

  const closeChoices = (restoreFocus = true): void => {
    if (!alive) return;
    surfaces.close("rating", restoreFocus);
    onSurfaceChange();
  };

  return {
    render(value) {
      if (!alive) return;
      currentRating = value;
      rating.textContent =
        value === 0
          ? "No rating"
          : `${value} ${value === 1 ? "star" : "stars"}`;
      dockRating.textContent = value === 0 ? "Rating" : `Rating ${value}★`;
      dockRating.setAttribute(
        "aria-label",
        value === 0
          ? "Rating, no Rating; open Rating controls"
          : `Rating ${value} stars; open Rating controls`,
      );
      syncRatingWheelOptions();
      for (const button of Array.from(
        ratings.querySelectorAll<HTMLButtonElement>("[data-rating-value]"),
      ))
        button.setAttribute(
          "aria-pressed",
          String(Number(button.dataset.ratingValue) === value),
        );
    },
    openChoices() {
      if (!alive) return;
      surfaces.open(
        "rating",
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : undefined,
      );
      onSurfaceChange();
      const current = ratings.querySelector<HTMLButtonElement>(
        `[data-rating-value="${currentRating}"]`,
      );
      (current ?? ratings.querySelector<HTMLButtonElement>("button"))?.focus();
    },
    closeChoices,
    openWheel: openRatingWheel,
    updateWheel: updateRatingWheel,
    closeWheel: closeRatingWheel,
    isWheelOpen: () => ratingWheelOpen,
    candidate: () => ratingWheelCandidate,
    setDecisionEnabled(enabled) {
      if (!alive) return;
      dockRating.disabled = !enabled;
      for (const button of Array.from(
        ratings.querySelectorAll<HTMLButtonElement>("button"),
      ))
        button.disabled = !enabled;
    },
    dispose() {
      alive = false;
      listeners.abort();
      closeRatingWheel();
    },
  };
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.max(minimum, Math.min(maximum, value));
}
