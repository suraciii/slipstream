import type { ViewSelectionState } from "./library-browser-view-contract.js";

export function required<T extends Element>(
  root: ParentNode,
  selector: string,
): T {
  const value = root.querySelector<T>(selector);
  if (!value) throw new Error(`Missing ${selector}`);
  return value;
}

export function paragraph(text: string): HTMLParagraphElement {
  const value = document.createElement("p");
  value.textContent = text;
  return value;
}

/// Keep the ready announcement in the accessible status while showing only the
/// result count in the compact Grid header.
export function presentGridStatus(element: HTMLElement, text: string): void {
  const prefix = "Ready · ";
  if (!text.startsWith(prefix)) {
    element.textContent = text;
    return;
  }
  const ready = document.createElement("span");
  ready.className = "grid-status-ready";
  ready.textContent = prefix;
  element.replaceChildren(
    ready,
    document.createTextNode(text.slice(prefix.length)),
  );
}

export function selectionLabel(value?: ViewSelectionState): string {
  return value === "picked"
    ? "Picked"
    : value === "rejected"
      ? "Rejected"
      : "Unflagged";
}
