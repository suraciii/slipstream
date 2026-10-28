import { required } from "./presentation-helpers.js";
import type { LibraryBrowserIntent } from "./library-browser-view-contract.js";

type SummaryAction = Readonly<{
  kind: "retry-library-check" | "refresh-current-source";
  presentationId: number;
}>;
type LibraryCheckState = "idle" | "active" | "failed" | "complete";

/** Renders the current summary on each surface; notice ownership stays with the application. */
export function createSummaryPresenter(
  root: HTMLElement,
  summaryStatus: HTMLElement,
  gridSummary: HTMLElement,
  gridEmptyAction: HTMLButtonElement,
  send: (intent: LibraryBrowserIntent) => void,
): (text: string, action?: SummaryAction, state?: LibraryCheckState) => void {
  const entry = required<HTMLElement>(root, "[data-library-status-entry]");
  const toggle = required<HTMLButtonElement>(
    root,
    "[data-library-status-toggle]",
  );
  const notice = required<HTMLElement>(root, "[data-library-status-notice]");

  toggle.addEventListener("click", () => {
    const expanded = toggle.getAttribute("aria-expanded") !== "true";
    toggle.setAttribute("aria-expanded", String(expanded));
    notice.hidden = !expanded;
  });

  return (text, action, state) => {
    const render = (surface: HTMLElement): void => {
      surface.replaceChildren(document.createTextNode(text));
      if (!action) return;
      const button = document.createElement("button");
      button.type = "button";
      button.className = "summary-action";
      button.textContent =
        action.kind === "retry-library-check"
          ? "Retry Library Check"
          : "Refresh Current Source";
      button.addEventListener("click", () =>
        send({ kind: "summary-action", presentationId: action.presentationId }),
      );
      surface.append(" ", button);
    };

    render(summaryStatus);
    gridSummary.replaceChildren();
    if (state && state !== "idle") render(gridSummary);
    gridEmptyAction.disabled = state === "active";

    const actionable = !!action && (state === "failed" || state === "complete");
    entry.hidden = !actionable;
    if (actionable) {
      render(notice);
    } else {
      notice.replaceChildren();
      notice.hidden = true;
      toggle.setAttribute("aria-expanded", "false");
    }
  };
}
