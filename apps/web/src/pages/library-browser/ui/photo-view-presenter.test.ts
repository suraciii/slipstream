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
  readonly selectors = new Map<string, ElementStub>();
  private listeners = new Map<string, Array<() => void>>();

  get isConnected(): boolean {
    return true;
  }

  querySelector(selector: string): ElementStub | null {
    return this.selectors.get(selector) ?? null;
  }

  setAttribute(): void {}

  replaceChildren(): void {}

  append(): void {}

  addEventListener(type: string, listener: () => void): void {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  removeAttribute(): void {}
}

const fixture = () => {
  const previousDocument = Object.getOwnPropertyDescriptor(
    globalThis,
    "document",
  );
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      createTextNode: (text: string) => text,
      createElement: () => new ElementStub(),
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
    "[data-status]",
    "[data-source]",
    "[data-preview-description]",
    "[data-preview-help]",
    "[data-metadata-capture-time]",
    "[data-metadata-aperture]",
    "[data-metadata-iso]",
    "[data-metadata-shutter-speed]",
    "[data-metadata-focal-length]",
  ])
    root.selectors.set(selector, new ElementStub());
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
