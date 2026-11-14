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
    optionsFooter: required<HTMLElement>(root, "[data-options-footer]"),
    optionsMainView: required<HTMLElement>(root, "[data-options-main-view]"),
    sourceActionsView: required<HTMLElement>(
      root,
      "[data-source-actions-view]",
    ),
    sourceActionsOpen: required<HTMLButtonElement>(
      root,
      "[data-source-actions-open]",
    ),
    sourceActionsReturn: required<HTMLButtonElement>(
      root,
      "[data-source-actions-return]",
    ),
    resetFilters: required<HTMLButtonElement>(root, "[data-reset-filters]"),
    draftDiscardNotice: required<HTMLElement>(
      root,
      "[data-draft-discard-notice]",
    ),
    advancedFilters: root.querySelector<HTMLDetailsElement>(
      "[data-advanced-filters]",
    ),
    albumResume: required<HTMLButtonElement>(root, "[data-album-resume]"),
    sortSelect: required<HTMLSelectElement>(root, "[data-sort-select]"),
    optionsProgress: required<HTMLElement>(root, "[data-grid-source-progress]"),
    optionsVisibleResults: required<HTMLElement>(
      root,
      "[data-grid-visible-results]",
    ),
    committedFilters: required<HTMLElement>(
      root,
      "[data-committed-filter-summary]",
    ),
    selectionSegments: required<HTMLElement>(root, "[data-selection-segments]"),
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
      ratingMatch: required<HTMLButtonElement>(root, "[data-rating-match]"),
      ratingCollapse: required<HTMLButtonElement>(
        root,
        "[data-rating-collapse]",
      ),
      ratingFields: required<HTMLElement>(root, "[data-rating-fields]"),
      ratingError: required<HTMLElement>(root, "[data-rating-error]"),
      kindSelect: required<HTMLSelectElement>(root, "[data-kind-select]"),
      availableSelect: required<HTMLSelectElement>(
        root,
        "[data-available-select]",
      ),
      capturedFromDate: required<HTMLInputElement>(
        root,
        "[data-captured-from-date]",
      ),
      capturedThroughDate: required<HTMLInputElement>(
        root,
        "[data-captured-through-date]",
      ),
      captureExact: required<HTMLDetailsElement>(root, "[data-capture-exact]"),
      captureError: required<HTMLElement>(root, "[data-capture-time-error]"),
      capturedFrom: required<HTMLInputElement>(root, "[data-captured-from]"),
      capturedBefore: required<HTMLInputElement>(
        root,
        "[data-captured-before]",
      ),
    },
  };
}
