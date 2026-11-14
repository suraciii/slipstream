import { afterEach, describe, expect, test } from "bun:test";
import { createViewOptions } from "./view-options.js";
import { viewOptionsElements } from "./view-options-elements.js";
import type { ViewSelectionFilter } from "./library-browser-view.js";
import type {
  GridFilterViewModel,
  GridProgressViewModel,
} from "./library-browser-view.js";
import { createModalSurfaces } from "./modal-surface.js";

/// The unit runtime has no DOM. These stubs implement the element operations
/// View options uses, so the tests exercise its own presenters and handlers
/// rather than a mock of them.
class ElementStub {
  hidden = false;
  disabled = false;
  open = false;
  value = "";
  textContent = "";
  title = "";
  className = "";
  readonly dataset: Record<string, string> = {};
  readonly attributes = new Map<string, string>();
  private readonly children: Array<ElementStub | string> = [];
  private readonly listeners = new Map<string, Array<() => void>>();
  readonly selectors = new Map<string, ElementStub>();
  options: Array<{ value: string; textContent: string; disabled: boolean }> =
    [];
  validity = { badInput: false, valid: true };

  get isConnected(): boolean {
    return true;
  }

  get parentElement(): ElementStub | null {
    return null;
  }

  get offsetParent(): ElementStub | null {
    return this;
  }

  get matches(): boolean {
    return false;
  }

  get textContentValue(): string {
    return this.textContent;
  }

  append(...children: Array<ElementStub | string>): void {
    this.children.push(...children);
  }

  replaceChildren(...children: Array<ElementStub | string>): void {
    this.children.length = 0;
    this.children.push(...children);
  }

  querySelectorAll(selector: string): ElementStub[] {
    const found = this.selectors.get(selector);
    return found ? [found] : [];
  }

  querySelector(selector: string): ElementStub | null {
    return this.selectors.get(selector) ?? null;
  }

  contains(): boolean {
    return false;
  }

  setAttribute(name: string, value: string): void {
    this.attributes.set(name, value);
  }

  getAttribute(name: string): string | null {
    return this.attributes.get(name) ?? null;
  }

  removeAttribute(name: string): void {
    this.attributes.delete(name);
  }

  setCustomValidity(): void {}

  reportValidity(): void {}

  focus(): void {}

  addEventListener(type: string, listener: () => void): void {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(): void {}

  dispatchEvent(event: { type: string }): boolean {
    for (const listener of this.listeners.get(event.type) ?? []) listener();
    return true;
  }

  close(): void {
    this.open = false;
  }

  showModal(): void {
    this.open = true;
  }

  click(): void {
    for (const listener of this.listeners.get("click") ?? []) listener();
  }

  change(): void {
    for (const listener of this.listeners.get("change") ?? []) listener();
  }
}

const element = (stub: ElementStub): HTMLElement =>
  stub as unknown as HTMLElement;

const mediaQuery = (): MediaQueryList =>
  ({
    matches: false,
    addEventListener() {},
    removeEventListener() {},
  }) as unknown as MediaQueryList;

const filterModel = (
  value: ViewSelectionFilter,
  enabled = true,
): GridFilterViewModel => ({ value, enabled, conditions: {} });

const progressModel = (
  overrides: Partial<GridProgressViewModel> = {},
): GridProgressViewModel => ({
  visible: true,
  visibleTotal: 6,
  sourceTotal: 6,
  sourceName: "All Photos",
  picked: 2,
  rejected: 3,
  unflagged: 1,
  ...overrides,
});

/// Builds the View options surface against the shipped markup, so the tests
/// run against the same element set the composition root reads.
const fixture = (search = "") => {
  const previousDocument = Object.getOwnPropertyDescriptor(
    globalThis,
    "document",
  );
  const previousHTMLElement = Object.getOwnPropertyDescriptor(
    globalThis,
    "HTMLElement",
  );
  const root = new ElementStub();
  const surfaces = createModalSurfaces();
  const intents: unknown[] = [];
  const documentStub = {
    createElement: () => new ElementStub(),
    createTextNode: (text: string) => text,
    activeElement: null as ElementStub | null,
  };
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: documentStub,
  });
  // View options reads `document.activeElement` through the global
  // `HTMLElement` constructor, which the unit runtime does not define.
  Object.defineProperty(globalThis, "HTMLElement", {
    configurable: true,
    value: class {},
  });
  // The template is HTML; the tests need the same attribute names. Reading them
  // out of the shipped markup keeps the two in step.
  const template = new Map<string, ElementStub>();
  const segments = new ElementStub();
  for (const segment of ["all", "unflagged", "picked", "rejected"]) {
    const segmentButton = new ElementStub();
    segmentButton.dataset.segment = segment;
    segmentButton.setAttribute("aria-pressed", String(segment === "all"));
    segments.selectors.set(`[data-segment="${segment}"]`, segmentButton);
  }
  segments.querySelectorAll = (selector: string) =>
    selector === "[data-segment]"
      ? Array.from(segments.selectors.values())
      : [];
  const lookup = (selector: string): ElementStub => {
    const segmentButton = segments.selectors.get(selector);
    if (segmentButton) return segmentButton;
    const found = template.get(selector);
    if (found) return found;
    const created = new ElementStub();
    template.set(selector, created);
    return created;
  };
  template.set("[data-selection-segments]", segments);
  const bySelector: Record<string, () => ElementStub> = {
    "[data-selection-segments]": () => segments,
    "[data-view-options]": () => lookup("[data-view-options]"),
    "[data-grid-view-options]": () => lookup("[data-grid-view-options]"),
    "[data-view-options-flag]": () => lookup("[data-view-options-flag]"),
    "[data-view-options-close]": () => lookup("[data-view-options-close]"),
    "[data-view-options-apply]": () => lookup("[data-view-options-apply]"),
    "[data-view-options-cancel]": () => lookup("[data-view-options-cancel]"),
    "[data-options-footer]": () => lookup("[data-options-footer]"),
    "[data-options-main-view]": () => lookup("[data-options-main-view]"),
    "[data-source-actions-view]": () => lookup("[data-source-actions-view]"),
    "[data-source-actions-open]": () => lookup("[data-source-actions-open]"),
    "[data-source-actions-return]": () =>
      lookup("[data-source-actions-return]"),
    "[data-reset-filters]": () => lookup("[data-reset-filters]"),
    "[data-draft-discard-notice]": () => lookup("[data-draft-discard-notice]"),
    "[data-album-resume]": () => lookup("[data-album-resume]"),
    "[data-sort-select]": () => {
      const found = lookup("[data-sort-select]");
      found.options = [];
      return found;
    },
    "[data-grid-source-progress]": () => lookup("[data-grid-source-progress]"),
    "[data-grid-visible-results]": () => lookup("[data-grid-visible-results]"),
    "[data-committed-filter-summary]": () =>
      lookup("[data-committed-filter-summary]"),
    "[data-size-select]": () => {
      const found = lookup("[data-size-select]");
      found.options = [];
      return found;
    },
    "[data-rating-min-select]": () => {
      const found = lookup("[data-rating-min-select]");
      found.options = [];
      return found;
    },
    "[data-rating-max-select]": () => {
      const found = lookup("[data-rating-max-select]");
      found.options = [];
      return found;
    },
    "[data-rating-match]": () => lookup("[data-rating-match]"),
    "[data-rating-collapse]": () => lookup("[data-rating-collapse]"),
    "[data-rating-fields]": () => lookup("[data-rating-fields]"),
    "[data-rating-error]": () => lookup("[data-rating-error]"),
    "[data-kind-select]": () => {
      const found = lookup("[data-kind-select]");
      found.options = [];
      return found;
    },
    "[data-available-select]": () => {
      const found = lookup("[data-available-select]");
      found.options = [];
      return found;
    },
    "[data-captured-from-date]": () => lookup("[data-captured-from-date]"),
    "[data-captured-through-date]": () =>
      lookup("[data-captured-through-date]"),
    "[data-capture-exact]": () => lookup("[data-capture-exact]"),
    "[data-capture-time-error]": () => lookup("[data-capture-time-error]"),
    "[data-captured-from]": () => lookup("[data-captured-from]"),
    "[data-captured-before]": () => lookup("[data-captured-before]"),
    "[data-random-photo]": () => lookup("[data-random-photo]"),
    "[data-grid-tools]": () => lookup("[data-grid-tools]"),
    "[data-browse-actions]": () => lookup("[data-browse-actions]"),
    "[data-removal-open]": () => lookup("[data-removal-open]"),
    "[data-advanced-filters]": () => lookup("[data-advanced-filters]"),
  };
  const dialog = lookup("[data-view-options]");
  // The shipped markup starts the Rating fields hidden and the disclosure
  // collapsed, so the fixture mirrors that initial state.
  const ratingFields = lookup("[data-rating-fields]");
  ratingFields.hidden = true;
  const ratingCollapse = lookup("[data-rating-collapse]");
  ratingCollapse.setAttribute("aria-expanded", "false");
  ratingCollapse.setAttribute("aria-controls", "rating-fields");
  ratingCollapse.textContent = "Rating";
  root.querySelector = (selector: string): ElementStub | null =>
    bySelector[selector]?.() ?? null;
  root.querySelectorAll = () => [];
  dialog.open = false;
  const viewOptions = createViewOptions({
    root: element(root),
    elements: viewOptionsElements(element(root)),
    surfaces,
    send: (intent) => intents.push(intent),
    resetGestures() {},
    closePhotoTools() {},
    closeRatingChoices() {},
    activeAlbumId: () => undefined,
    onSizeChange() {},
    compact: mediaQuery(),
    randomHeader: mediaQuery(),
  });
  const pressed = (segment: string): boolean =>
    lookup(`[data-segment="${segment}"]`).getAttribute("aria-pressed") ===
    "true";
  return {
    intents,
    viewOptions,
    dialog,
    removalOpen: lookup("[data-removal-open]"),
    ratingCollapse: lookup("[data-rating-collapse]"),
    ratingFields: lookup("[data-rating-fields]"),
    ratingMatch: lookup("[data-rating-match]"),
    resetFilters: lookup("[data-reset-filters]"),
    draftDiscardNotice: lookup("[data-draft-discard-notice]"),
    apply: lookup("[data-view-options-apply]"),
    segments,
    pressed,
    lookup,
    press: (segment: string) => lookup(`[data-segment="${segment}"]`).click(),
    restore: () => {
      if (previousDocument)
        Object.defineProperty(globalThis, "document", previousDocument);
      else Reflect.deleteProperty(globalThis, "document");
      if (previousHTMLElement)
        Object.defineProperty(globalThis, "HTMLElement", previousHTMLElement);
      else Reflect.deleteProperty(globalThis, "HTMLElement");
    },
    search,
    template,
  };
};

afterEach(() => {
  // Each test restores its own document; this keeps a leaked stub from
  // affecting a sibling test when one fails mid-way.
  const current = Object.getOwnPropertyDescriptor(globalThis, "document");
  if (current && typeof current.value === "object" && current.value !== null)
    Reflect.deleteProperty(globalThis, "document");
});

describe("View options Selection segments", () => {
  test("one segment is pressed for the committed Selection State filter", () => {
    const ui = fixture();
    try {
      ui.viewOptions.renderFilter(filterModel("all"));
      expect(ui.pressed("all")).toBe(true);
      expect(ui.pressed("unflagged")).toBe(false);

      ui.viewOptions.renderFilter(filterModel("picked"));
      expect(ui.pressed("all")).toBe(false);
      expect(ui.pressed("picked")).toBe(true);
      expect(ui.pressed("unflagged")).toBe(false);
      expect(ui.pressed("rejected")).toBe(false);

      ui.viewOptions.renderFilter(filterModel("rejected"));
      expect(ui.pressed("rejected")).toBe(true);
      expect(ui.pressed("picked")).toBe(false);
    } finally {
      ui.restore();
    }
  });

  test("a segment press commits exactly that Selection State on Apply", () => {
    const ui = fixture();
    try {
      ui.viewOptions.renderFilter(filterModel("all"));
      ui.press("unflagged");
      expect(ui.pressed("unflagged")).toBe(true);
      expect(ui.pressed("all")).toBe(false);
      ui.apply.click();
      expect(ui.intents).toEqual([
        {
          kind: "view-options-apply",
          order: "source-default",
          selection: "unflagged",
          conditions: {},
        },
      ]);
    } finally {
      ui.restore();
    }
  });

  test("All clears the Selection State condition and restores the default", () => {
    const ui = fixture();
    try {
      ui.viewOptions.renderFilter(filterModel("rejected"));
      ui.press("all");
      expect(ui.pressed("all")).toBe(true);
      expect(ui.pressed("rejected")).toBe(false);
      ui.apply.click();
      expect(ui.intents).toEqual([
        {
          kind: "view-options-apply",
          order: "source-default",
          selection: "all",
          conditions: {},
        },
      ]);
    } finally {
      ui.restore();
    }
  });

  test("a committed single value opens with its segment pressed and re-applies unchanged", () => {
    const ui = fixture();
    try {
      // Forward: the committed value marks exactly its own segment pressed.
      ui.viewOptions.renderFilter(filterModel("picked"));
      expect(ui.pressed("picked")).toBe(true);
      expect(ui.pressed("all")).toBe(false);
      expect(ui.pressed("rejected")).toBe(false);
      // Backward: staging the same value again and applying it commits the
      // same filter, so a reopen with that value sends no new destination.
      ui.press("picked");
      ui.apply.click();
      expect(ui.intents).toEqual([]);
      // The opposite direction: staging a different value does commit it.
      ui.press("unflagged");
      ui.apply.click();
      expect(ui.intents).toEqual([
        {
          kind: "view-options-apply",
          order: "source-default",
          selection: "unflagged",
          conditions: {},
        },
      ]);
    } finally {
      ui.restore();
    }
  });

  test("a staged Selection State survives a committed re-render while open", () => {
    const ui = fixture();
    try {
      ui.viewOptions.renderFilter(filterModel("rejected"));
      ui.viewOptions.open();
      ui.press("all");
      expect(ui.pressed("all")).toBe(true);
      // A background committed render updates the committed facts but keeps
      // the staged Selection State, exactly like the condition drafts.
      ui.viewOptions.renderFilter(filterModel("rejected"));
      expect(ui.pressed("all")).toBe(true);
      expect(ui.pressed("rejected")).toBe(false);
      // Applying commits the staged value, not the arrived committed one.
      ui.apply.click();
      expect(ui.intents).toEqual([
        {
          kind: "view-options-apply",
          order: "source-default",
          selection: "all",
          conditions: {},
        },
      ]);
    } finally {
      ui.restore();
    }
  });

  test("a disabled filter disables every segment", () => {
    const ui = fixture();
    try {
      ui.viewOptions.renderFilter(filterModel("picked", false));
      expect(ui.segments.querySelectorAll("[data-segment]").length).toBe(4);
      for (const segment of ui.segments.querySelectorAll("[data-segment]"))
        expect(segment.disabled).toBe(true);
    } finally {
      ui.restore();
    }
  });

  test("Reset filters returns the segments to All", () => {
    const ui = fixture();
    try {
      ui.viewOptions.renderFilter(filterModel("rejected"));
      ui.resetFilters.click();
      expect(ui.pressed("all")).toBe(true);
      expect(ui.pressed("rejected")).toBe(false);
    } finally {
      ui.restore();
    }
  });
});

describe("View options Rating collapse", () => {
  test("the disclosure opens the Rating fields and reports its state", () => {
    const ui = fixture();
    try {
      ui.viewOptions.renderFilter(filterModel("all"));
      expect(ui.ratingFields.hidden).toBe(true);
      expect(ui.ratingCollapse.getAttribute("aria-expanded")).toBe("false");
      expect(ui.ratingCollapse.getAttribute("aria-controls")).toBe(
        "rating-fields",
      );
      expect(ui.ratingCollapse.textContent).toBe("Rating");

      ui.ratingCollapse.click();
      expect(ui.ratingFields.hidden).toBe(false);
      expect(ui.ratingCollapse.getAttribute("aria-expanded")).toBe("true");

      ui.ratingCollapse.click();
      expect(ui.ratingFields.hidden).toBe(true);
      expect(ui.ratingCollapse.getAttribute("aria-expanded")).toBe("false");
    } finally {
      ui.restore();
    }
  });

  test("a committed Rating bound names itself and opens its disclosure", () => {
    const ui = fixture();
    try {
      ui.viewOptions.renderFilter({
        value: "all",
        enabled: true,
        conditions: { ratingMinimum: 3 },
      });
      expect(ui.ratingCollapse.textContent).toBe("Rating 3 and up");
      expect(ui.ratingFields.hidden).toBe(false);
      expect(ui.ratingCollapse.getAttribute("aria-expanded")).toBe("true");
      ui.viewOptions.renderFilter({
        value: "all",
        enabled: true,
        conditions: { ratingMinimum: 2, ratingMaximum: 4 },
      });
      expect(ui.ratingCollapse.textContent).toBe("Rating 2–4");
      ui.viewOptions.renderFilter({
        value: "all",
        enabled: true,
        conditions: { ratingMinimum: 3, ratingMaximum: 3 },
      });
      expect(ui.ratingCollapse.textContent).toBe("Rating 3");
    } finally {
      ui.restore();
    }
  });

  test("Match minimum keeps pairing an exact Rating bound", () => {
    const ui = fixture();
    try {
      ui.viewOptions.renderFilter({
        value: "all",
        enabled: true,
        conditions: {},
      });
      // A real select pair, so the pairing runs through the shipped handler.
      const minimum = ui.lookup("[data-rating-min-select]");
      const maximum = ui.lookup("[data-rating-max-select]");
      minimum.value = "3";
      maximum.value = "";
      minimum.change();
      expect(ui.ratingMatch.disabled).toBe(false);
      ui.ratingMatch.click();
      expect(maximum.value).toBe("3");
      expect(ui.ratingMatch.disabled).toBe(true);
      // Applying the paired bounds keeps the exact Rating condition.
      ui.apply.click();
      expect(ui.intents).toEqual([
        {
          kind: "view-options-apply",
          order: "source-default",
          selection: "all",
          conditions: { ratingMinimum: 3, ratingMaximum: 3 },
        },
      ]);
    } finally {
      ui.restore();
    }
  });
});

describe("View options source action labels", () => {
  test("the removal entry names the count and the source, and disables at zero", () => {
    const ui = fixture();
    try {
      ui.viewOptions.renderProgress(progressModel({ rejected: 3 }));
      expect(ui.removalOpen.textContent).toBe(
        "Remove 3 Photos reviewed as Rejected from All Photos",
      );
      expect(ui.removalOpen.disabled).toBe(false);

      ui.viewOptions.renderProgress(progressModel({ rejected: 0 }));
      expect(ui.removalOpen.textContent).toBe("Remove rejected Photos");
      expect(ui.removalOpen.disabled).toBe(true);

      ui.viewOptions.renderProgress(
        progressModel({ rejected: 1, sourceName: "Trip" }),
      );
      expect(ui.removalOpen.textContent).toBe(
        "Remove 1 Photo reviewed as Rejected from Trip",
      );
      expect(ui.removalOpen.disabled).toBe(false);
    } finally {
      ui.restore();
    }
  });
});

describe("View options draft notice", () => {
  test("a staged Selection State marks the draft dirty", () => {
    const ui = fixture();
    try {
      // Opening the dialog resets the draft to the committed choices.
      ui.viewOptions.renderFilter(filterModel("all"));
      ui.viewOptions.open();
      expect(ui.draftDiscardNotice.hidden).toBe(true);
      // Staging a different Selection State marks the draft dirty.
      ui.press("picked");
      expect(ui.draftDiscardNotice.hidden).toBe(false);
      // Returning to the committed value clears the notice again.
      ui.press("all");
      expect(ui.draftDiscardNotice.hidden).toBe(true);
    } finally {
      ui.restore();
    }
  });
});
