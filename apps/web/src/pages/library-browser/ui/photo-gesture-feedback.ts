import type { PhotoGestureFeedback } from "./photo-gestures.js";
import {
  createNavigationAvailability,
  type PhotoNavigationControls,
} from "./photo-navigation-controls.js";
import { required } from "./presentation-helpers.js";

export type PhotoGestureFeedbackView = PhotoGestureFeedback &
  Readonly<{
    update(
      model: PhotoNavigationControls & Readonly<{ decisionEnabled: boolean }>,
    ): void;
    decisionEnabled(): boolean;
  }>;

export function createPhotoGestureFeedback(
  root: ParentNode,
): PhotoGestureFeedbackView {
  const pick = required<HTMLElement>(root, "[data-pick-feedback]");
  const reject = required<HTMLElement>(root, "[data-reject-feedback]");
  const next = required<HTMLElement>(root, "[data-next-feedback]");
  const previous = required<HTMLElement>(root, "[data-previous-feedback]");
  const navigation = createNavigationAvailability();
  let decisionEnabled = false;
  return {
    pick,
    reject,
    next,
    previous,
    navigationAvailability: navigation.read,
    update(model) {
      decisionEnabled = model.decisionEnabled;
      navigation.update(model);
    },
    decisionEnabled: () => decisionEnabled,
  };
}
