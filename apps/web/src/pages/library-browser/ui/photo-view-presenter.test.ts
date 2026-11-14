import { describe, expect, test } from "bun:test";
import { createPhotoViewPresenter } from "./photo-view-presenter.js";

class ElementStub {
  hidden = false;
  textContent = "";
  title = "";
  src = "";
  alt = "";
  draggable = false;
  fetchPriority = "";
  decoding = "";
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  dataset: Record<string, string> = {};
  style = {
    backgroundImage: "",
    removeProperty: () => {},
  };
  focused = false;
  readonly selectors = new Map<string, ElementStub>();
  private listeners = new Map<string, Array<(event: unknown) => void>>();
  private attributes = new Map<string, string>();

  get isConnected(): boolean {
    return true;
  }

  querySelector(selector: string): ElementStub | null {
    return this.selectors.get(selector) ?? null;
  }

  contains(other: unknown): boolean {
    return other === this;
  }

  focus(): void {
    this.focused = true;
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

  replaceChildren(): void {}

  append(): void {}

  addEventListener(type: string, listener: (event: unknown) => void): void {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  click(): void {
    for (const listener of this.listeners.get("click") ?? []) listener({});
  }

  keydown(key: string): void {
    for (const listener of this.listeners.get("keydown") ?? [])
      listener({ key, stopPropagation: () => {} });
  }
}

const fixture = () => {
  const previousDocument = Object.getOwnPropertyDescriptor(
    globalThis,
    "document",
  );
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const documentListeners = new Map<string, Array<(event: unknown) => void>>();
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      createTextNode: (text: string) => text,
      createElement: () => new ElementStub(),
      addEventListener: (type: string, listener: (event: unknown) => void) => {
        const listeners = documentListeners.get(type) ?? [];
        listeners.push(listener);
        documentListeners.set(type, listeners);
      },
      removeEventListener: () => {},
    },
  });
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: { location: { href: "https://slipstream.test/" } },
  });
  const root = new ElementStub();
  for (const selector of [
    "[data-photo-title]",
    "[data-position]",
    "[data-photo-boundary]",
    "[data-photo-filename]",
    "[data-selection]",
    "[data-selection-chip]",
    "[data-stage]",
    "[data-preview-ambient]",
    "[data-photo-facts-line]",
    "[data-photo-facts-capture] [data-photo-facts-value]",
    "[data-photo-facts-aperture] [data-photo-facts-value]",
    "[data-photo-facts-shutter-speed] [data-photo-facts-value]",
    "[data-photo-facts-iso] [data-photo-facts-value]",
    "[data-photo-facts-focal-length] [data-photo-facts-value]",
    "[data-photo-facts-empty]",
    "[data-status]",
    "[data-source]",
    "[data-preview-description]",
    "[data-preview-help]",
    "[data-metadata-capture-time]",
    "[data-metadata-aperture]",
    "[data-metadata-iso]",
    "[data-metadata-shutter-speed]",
    "[data-metadata-focal-length]",
    '[data-photo-tools-entry="recovery"]',
    "[data-quick-action-dock]",
  ])
    root.selectors.set(selector, new ElementStub());
  const dock = root.selectors.get("[data-quick-action-dock]")!;
  const secondary = new ElementStub();
  for (const selector of ["[data-dock-undo]", "[data-dock-more]"])
    secondary.selectors.set(selector, new ElementStub());
  dock.selectors.set("[data-dock-secondary]", secondary);
  dock.selectors.set("[data-dock-overflow]", new ElementStub());
  const presenter = createPhotoViewPresenter({
    root: root as unknown as HTMLElement,
    zoom: { resetForImage: () => {}, applyZoom: () => {} },
    resetGestures: () => {},
    reviewRecovery: () => {},
  });
  return {
    presenter,
    position: root.selectors.get("[data-position]")!,
    boundary: root.selectors.get("[data-photo-boundary]")!,
    factsEmpty: root.selectors.get("[data-photo-facts-empty]")!,
    recoveryEntry: root.selectors.get('[data-photo-tools-entry="recovery"]')!,
    dock: root.selectors.get("[data-quick-action-dock]")!,
    dockToggle: dock.selectors.get("[data-dock-overflow]")!,
    dockSecondary: dock.selectors.get("[data-dock-secondary]")!,
    pointerDown: (event: { target: unknown } = { target: null }) => {
      for (const listener of documentListeners.get("pointerdown") ?? [])
        listener(event);
    },
    restore: () => {
      if (previousDocument)
        Object.defineProperty(globalThis, "document", previousDocument);
      else Reflect.deleteProperty(globalThis, "document");
      if (previousWindow)
        Object.defineProperty(globalThis, "window", previousWindow);
      else Reflect.deleteProperty(globalThis, "window");
    },
  };
};

const model = (photoId: string) => ({
  sourceName: "Library",
  index: 0,
  total: 2,
  photoId,
  available: true,
  originalFilename: `${photoId}.jpg`,
  selectionState: "unflagged" as const,
  rating: 0,
  previewSource: "jpeg-original" as const,
  limitedDetail: false,
  previewUrl: `/api/private/derivatives/${photoId}/review/key.jpg`,
});

describe("Photo view image surface", () => {
  test("a status update does not detach the presented image failure", () => {
    const ui = fixture();
    try {
      const image = ui.presenter.renderPhotoShell(model("photo-1"));
      if (!image) throw new Error("expected a presented review image");

      // renderPhotoShell presents the status before the image.
      expect(ui.presenter.isReviewImageSurfaceCurrent(image.photoSurface)).toBe(
        true,
      );
      expect(ui.presenter.isPhotoStatusSurfaceCurrent(image.surface)).toBe(
        true,
      );

      ui.presenter.setPhotoStatus("Connection lost.");
      expect(ui.presenter.isReviewImageSurfaceCurrent(image.photoSurface)).toBe(
        true,
      );
      expect(ui.presenter.isPhotoStatusSurfaceCurrent(image.surface)).toBe(
        false,
      );
    } finally {
      ui.restore();
    }
  });

  test("presenting the next Photo detaches the previous image failure", () => {
    const ui = fixture();
    try {
      const first = ui.presenter.renderPhotoShell(model("photo-1"));
      if (!first) throw new Error("expected a presented review image");
      ui.presenter.renderPhotoShell(model("photo-2"));
      expect(ui.presenter.isReviewImageSurfaceCurrent(first.photoSurface)).toBe(
        false,
      );
    } finally {
      ui.restore();
    }
  });

  test("names the end of the view only on the last Photo", () => {
    const ui = fixture();
    try {
      ui.presenter.renderPhotoShell(model("photo-1"));
      expect(ui.position.textContent).toBe("1 / 2");
      expect(ui.boundary.hidden).toBe(true);

      ui.presenter.renderPhotoShell({
        ...model("photo-2"),
        index: 1,
      });
      expect(ui.position.textContent).toBe("2 / 2");
      expect(ui.boundary.hidden).toBe(false);
      expect(ui.boundary.textContent).toBe("End of view");

      ui.presenter.renderPhotoShell(model("photo-1"));
      expect(ui.boundary.hidden).toBe(true);
      expect(ui.boundary.textContent).toBe("");

      ui.presenter.renderPhotoShell({
        ...model("only-photo"),
        total: 1,
      });
      expect(ui.position.textContent).toBe("1 / 1");
      expect(ui.boundary.hidden).toBe(false);
      expect(ui.boundary.textContent).toBe("End of view");

      ui.presenter.renderPhotoFacts({ index: 0, total: 0 });
      expect(ui.boundary.hidden).toBe(true);
      expect(ui.boundary.textContent).toBe("");
    } finally {
      ui.restore();
    }
  });
});

describe("Photo dock secondary disclosure", () => {
  test("the toggle opens and closes the Undo and More pair", () => {
    const ui = fixture();
    try {
      expect(ui.dockToggle.getAttribute("aria-expanded")).toBe("false");
      expect(ui.dockSecondary.getAttribute("data-open")).toBeNull();

      ui.dockToggle.click();
      expect(ui.dockToggle.getAttribute("aria-expanded")).toBe("true");
      expect(ui.dockSecondary.getAttribute("data-open")).toBe("");
      // The Undo and More buttons stay the dock's own controls; opening the
      // disclosure only changes disclosure state, so their handlers stay
      // attached and no second copy exists.
      expect(ui.dockSecondary.querySelector("[data-dock-undo]")).not.toBeNull();
      expect(ui.dockSecondary.querySelector("[data-dock-more]")).not.toBeNull();

      ui.dockToggle.click();
      expect(ui.dockToggle.getAttribute("aria-expanded")).toBe("false");
      expect(ui.dockSecondary.getAttribute("data-open")).toBeNull();
    } finally {
      ui.restore();
    }
  });

  test("Escape closes an open disclosure and returns focus to the toggle", () => {
    const ui = fixture();
    try {
      ui.dockToggle.click();
      ui.dock.keydown("Escape");

      expect(ui.dockToggle.getAttribute("aria-expanded")).toBe("false");
      expect(ui.dockSecondary.getAttribute("data-open")).toBeNull();
      expect(ui.dockToggle.focused).toBe(true);
    } finally {
      ui.restore();
    }
  });

  test("a pointer press outside the dock closes the disclosure", () => {
    const ui = fixture();
    try {
      ui.dockToggle.click();
      expect(ui.dockSecondary.getAttribute("data-open")).toBe("");

      ui.pointerDown({ target: {} });
      expect(ui.dockSecondary.getAttribute("data-open")).toBeNull();

      ui.dockToggle.click();
      ui.pointerDown({ target: ui.dockToggle });
      expect(ui.dockSecondary.getAttribute("data-open")).toBe("");
    } finally {
      ui.restore();
    }
  });

  test("a press inside an open modal keeps the disclosure open", () => {
    const ui = fixture();
    try {
      ui.dockToggle.click();
      expect(ui.dockSecondary.getAttribute("data-open")).toBe("");

      ui.pointerDown({ target: { closest: () => ({}) } });
      expect(ui.dockSecondary.getAttribute("data-open")).toBe("");
    } finally {
      ui.restore();
    }
  });
});

describe("Photo tools recovery entry", () => {
  test("stays hidden while the presented Photo's Original is available", () => {
    const ui = fixture();
    try {
      ui.presenter.renderPhotoShell(model("photo-1"));
      expect(ui.recoveryEntry.hidden).toBe(true);
    } finally {
      ui.restore();
    }
  });

  test("is visible while the presented Photo reports an unavailable Original", () => {
    const ui = fixture();
    try {
      ui.presenter.renderPhotoShell({
        ...model("photo-1"),
        originalAvailable: false,
      });
      expect(ui.recoveryEntry.hidden).toBe(false);

      ui.presenter.renderPhotoShell(model("photo-2"));
      expect(ui.recoveryEntry.hidden).toBe(true);
    } finally {
      ui.restore();
    }
  });

  test("a source change re-evaluates the entry", () => {
    const ui = fixture();
    try {
      ui.presenter.renderPhotoShell({
        ...model("photo-1"),
        originalAvailable: false,
      });
      expect(ui.recoveryEntry.hidden).toBe(false);

      ui.presenter.renderPhotoShell(model("photo-2"));
      expect(ui.recoveryEntry.hidden).toBe(true);
    } finally {
      ui.restore();
    }
  });
});

describe("capture facts empty line", () => {
  test("states the absence exactly when no capture fact exists", () => {
    const ui = fixture();
    try {
      ui.presenter.renderPhotoShell(model("photo-1"));
      ui.presenter.renderPhotoMetadata();
      expect(ui.factsEmpty.hidden).toBe(false);

      ui.presenter.renderPhotoMetadata({ aperture: "f/2.8" });
      expect(ui.factsEmpty.hidden).toBe(true);

      ui.presenter.renderPhotoMetadata();
      expect(ui.factsEmpty.hidden).toBe(false);
    } finally {
      ui.restore();
    }
  });
});
