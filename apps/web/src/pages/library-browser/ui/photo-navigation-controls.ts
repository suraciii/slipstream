import type {
  NavigationAvailability,
  NavigationFeedback,
} from "./photo-gestures.js";

/// The navigation subset of the review controls. `enabled` owns interaction
/// admission; `boundary` explains an unavailable end of the source.
export type PhotoNavigationControls = Readonly<{
  previousEnabled: boolean;
  nextEnabled: boolean;
  previousBoundary: boolean;
  nextBoundary: boolean;
}>;

export function photoNavigationControls(
  index: number,
  total: number,
  enabled: boolean,
): PhotoNavigationControls {
  return {
    previousEnabled: enabled && index > 0,
    nextEnabled: enabled && total > 0 && index < total - 1,
    previousBoundary: index <= 0 || total === 0,
    nextBoundary: total === 0 || index >= total - 1,
  };
}

const feedbackState = (
  enabled: boolean,
  boundary: boolean,
): NavigationFeedback =>
  enabled ? "enabled" : boundary ? "boundary" : "disabled";

export type NavigationAvailabilityController = Readonly<{
  update(model: PhotoNavigationControls): void;
  read(): NavigationAvailability;
}>;

export function createNavigationAvailability(): NavigationAvailabilityController {
  let current: NavigationAvailability = {
    previous: "disabled",
    next: "disabled",
  };
  return {
    update(model) {
      current = {
        previous: feedbackState(model.previousEnabled, model.previousBoundary),
        next: feedbackState(model.nextEnabled, model.nextBoundary),
      };
    },
    read: () => current,
  };
}
