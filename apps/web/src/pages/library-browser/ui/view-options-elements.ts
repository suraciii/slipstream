/// The View options element lookups: every control the dialog owns, read from
/// the composition root exactly once.
///
/// This module exists so the Library Browser view stays one bounded
/// composition root: the dialog's controls are named in one place beside the
/// draft owner that reads them, instead of inline in the view.
import type { ViewOptionsElements } from "./view-options.js";

const required = <T extends Element>(root: ParentNode, selector: string): T => {
  const value = root.querySelector<T>(selector);
  if (!value) throw new Error(`Missing ${selector}`);
  return value;
};

export function viewOptionsElements(root: ParentNode): ViewOptionsElements {
  return {
    viewOptionsDialog: required<HTMLDialogElement>(root, "[data-view-options]"),
    viewOptionsOpen: required<HTMLButtonElement>(
      root,
      "[data-grid-view-options]",
    ),
    viewOptionsFlag: required<HTMLElement>(root, "[data-view-options-flag]"),
    viewOptionsClose: required<HTMLButtonElement>(
      root,
      "[data-view-options-close]",
    ),
    viewOptionsApply: required<HTMLButtonElement>(
      root,
      "[data-view-options-apply]",
    ),
    viewOptionsCancel: required<HTMLButtonElement>(
      root,
      "[data-view-options-cancel]",
    ),
    albumResume: required<HTMLButtonElement>(root, "[data-album-resume]"),
    sortSelect: required<HTMLSelectElement>(root, "[data-sort-select]"),
    optionsProgress: required<HTMLElement>(root, "[data-grid-source-progress]"),
    optionsVisibleResults: required<HTMLElement>(
      root,
      "[data-grid-visible-results]",
    ),
    filterSelect: required<HTMLSelectElement>(root, "[data-filter-select]"),
    sizeSelect: required<HTMLSelectElement>(root, "[data-size-select]"),
    filters: {
      ratingMinimum: required<HTMLSelectElement>(
        root,
        "[data-rating-min-select]",
      ),
      ratingMaximum: required<HTMLSelectElement>(
        root,
        "[data-rating-max-select]",
      ),
      kindSelect: required<HTMLSelectElement>(root, "[data-kind-select]"),
      availableSelect: required<HTMLSelectElement>(
        root,
        "[data-available-select]",
      ),
      capturedFrom: required<HTMLInputElement>(root, "[data-captured-from]"),
      capturedBefore: required<HTMLInputElement>(
        root,
        "[data-captured-before]",
      ),
    },
  };
}
