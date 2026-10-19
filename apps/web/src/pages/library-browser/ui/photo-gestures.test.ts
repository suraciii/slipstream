import { expect, test } from "bun:test";
import { createPhotoGestures } from "./photo-gestures.js";
import type { PhotoZoomController } from "./photo-zoom.js";
import type { RatingControls } from "./rating-controls.js";
import type { LibraryBrowserIntent } from "./library-browser-view.js";

class Surface extends EventTarget {
  style = { transform: "" };
  parent: Surface | undefined;
  captured = new Set<number>();
  classes = new Set<string>();
  classList = {
    remove: (value: string) => this.classes.delete(value),
    toggle: (value: string, active: boolean) =>
      active ? this.classes.add(value) : this.classes.delete(value),
  };
  setPointerCapture(id: number) {
    this.captured.add(id);
  }
  hasPointerCapture(id: number) {
    return this.captured.has(id);
  }
  releasePointerCapture(id: number) {
    this.captured.delete(id);
    const event = new Event("lostpointercapture");
    Object.defineProperty(event, "pointerId", { value: id });
    this.dispatchEvent(event);
  }
}

function fixture() {
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const timers = new Map<number, { at: number; callback: () => void }>();
  let now = 0;
  let sequence = 0;
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      setTimeout(callback: () => void, delay: number) {
        const id = ++sequence;
        timers.set(id, { at: now + delay, callback });
        return id;
      },
      clearTimeout(id: number) {
        timers.delete(id);
      },
    },
  });
  const preview = new Surface();
  const stage = new Surface();
  const select = new Surface();
  const reject = new Surface();
  const imageTarget = new Surface();
  stage.parent = preview;
  imageTarget.parent = stage;
  let renderedImage = true;
  const isRenderedImage = (event: PointerEvent) =>
    renderedImage && event.target === (imageTarget as unknown as EventTarget);
  let photoId = "photo-a";
  let surface = {};
  let enabled = true;
  let manual = false;
  let pinch = false;
  let wheel = false;
  let panX = 0;
  const candidate = 4;
  const pointers = new Set<number>();
  const writes: LibraryBrowserIntent[] = [];
  const zoom = {
    trackPointer(id: number) {
      pointers.add(id);
      return pointers.size;
    },
    updatePointer() {},
    removePointer(id: number) {
      pointers.delete(id);
      return pointers.size;
    },
    beginPinch() {
      pinch = true;
      manual = true;
    },
    isPinching: () => pinch,
    updatePinch() {},
    endPinch() {
      pinch = false;
    },
    isManual: () => manual,
    hasMeasurableImage: () => true,
    panBy(x: number) {
      panX += x;
    },
    resetPointers() {
      pointers.clear();
      pinch = false;
    },
  } as unknown as PhotoZoomController;
  const rating = {
    openWheel() {
      wheel = true;
    },
    updateWheel() {},
    closeWheel() {
      wheel = false;
    },
    isWheelOpen: () => wheel,
    candidate: () => candidate,
  } as unknown as RatingControls;
  const gestures = createPhotoGestures({
    preview: preview as unknown as HTMLElement,
    stage: stage as unknown as HTMLElement,
    selectFeedback: select as unknown as HTMLElement,
    rejectFeedback: reject as unknown as HTMLElement,
    zoom,
    rating,
    isAlive: () => true,
    isRenderedImage,
    currentPhotoId: () => photoId,
    currentSurface: () => surface,
    decisionEnabled: () => enabled,
    send: (intent) => writes.push(intent),
  });
  return {
    gestures,
    writes,
    preview,
    stage,
    get wheel() {
      return wheel;
    },
    get panX() {
      return panX;
    },
    advance(ms: number) {
      now += ms;
      for (const [id, timer] of timers)
        if (timer.at <= now) {
          timers.delete(id);
          timer.callback();
        }
    },
    pointer(
      type: string,
      x: number,
      y = 0,
      id = 1,
      target: "image" | "letterbox" = "image",
    ) {
      const event = new Event(type, { cancelable: true });
      const targetNode = target === "image" ? imageTarget : stage;
      Object.defineProperties(event, {
        pointerId: { value: id },
        clientX: { value: x },
        clientY: { value: y },
        pointerType: { value: "touch" },
        isPrimary: { value: id === 1 },
        timeStamp: { value: now },
        target: { value: targetNode },
      });
      preview.dispatchEvent(event);
    },
    changePhoto() {
      photoId = "photo-b";
      surface = {};
      gestures.reset();
    },
    disable() {
      enabled = false;
      gestures.cancelUnavailableDecision();
    },
    setRenderedImage(value: boolean) {
      renderedImage = value;
    },
    manual() {
      manual = true;
    },
    dispose() {
      gestures.dispose();
      if (previousWindow)
        Object.defineProperty(globalThis, "window", previousWindow);
      else Reflect.deleteProperty(globalThis, "window");
    },
  };
}
test("Rating hold boundary and release submit exactly once", () => {
  const f = fixture();
  try {
    f.pointer("pointerdown", 0);
    f.advance(449);
    expect(f.wheel).toBe(false);
    f.advance(1);
    expect(f.wheel).toBe(true);
    expect(f.writes).toEqual([]);
    f.pointer("pointerup", 0);
    f.pointer("lostpointercapture", 0);
    expect(f.writes).toEqual([
      { kind: "photo-mutation", field: "rating", value: 4, advance: false },
    ]);
    expect(f.preview.captured.size).toBe(0);
  } finally {
    f.dispose();
  }
});

test("horizontal decisions use Select and Reject and advance", () => {
  const f = fixture();
  try {
    f.pointer("pointerdown", 0, 0);
    f.pointer("pointermove", 90, 0);
    f.pointer("pointerup", 90, 0);
    expect(f.writes).toEqual([
      {
        kind: "photo-mutation",
        field: "selectionState",
        value: "picked",
        advance: true,
      },
    ]);
    f.pointer("pointerdown", 100, 0);
    f.pointer("pointermove", 10, 0);
    f.pointer("pointerup", 10, 0);
    expect(f.writes.at(-1)).toEqual({
      kind: "photo-mutation",
      field: "selectionState",
      value: "rejected",
      advance: true,
    });
  } finally {
    f.dispose();
  }
});

test("vertical decisions navigate without changing Photo facts", () => {
  const f = fixture();
  try {
    f.pointer("pointerdown", 100, 120);
    f.pointer("pointermove", 100, 30);
    f.pointer("pointerup", 100, 30);
    f.pointer("pointerdown", 100, 30);
    f.pointer("pointermove", 100, 120);
    f.pointer("pointerup", 100, 120);
    expect(f.writes).toEqual([{ kind: "next" }, { kind: "previous" }]);
  } finally {
    f.dispose();
  }
});
test("non-image Preview regions remain native and emit no gesture action", () => {
  const f = fixture();
  try {
    f.pointer("pointerdown", 100, 120, 1, "letterbox");
    f.pointer("pointermove", 100, 20, 1, "letterbox");
    f.pointer("pointerup", 100, 20, 1, "letterbox");
    expect(f.writes).toEqual([]);
    expect(f.preview.captured.size).toBe(0);
  } finally {
    f.dispose();
  }
});

test("equal-axis movement cancels without an action", () => {
  const f = fixture();
  try {
    f.pointer("pointerdown", 0, 0);
    f.pointer("pointermove", 80, 80);
    f.pointer("pointerup", 80, 80);
    expect(f.writes).toEqual([]);
  } finally {
    f.dispose();
  }
});

test("cancellation, Photo change and disabled decisions withdraw held Rating without writes", () => {
  for (const action of ["cancel", "photo", "disable", "dispose"] as const) {
    const f = fixture();
    try {
      f.pointer("pointerdown", 0);
      f.advance(450);
      if (action === "cancel") f.pointer("pointercancel", 0);
      if (action === "photo") f.changePhoto();
      if (action === "disable") f.disable();
      if (action === "dispose") f.gestures.dispose();
      f.pointer("pointerup", 100);
      f.advance(450);
      expect(f.wheel).toBe(false);
      expect(f.writes).toEqual([]);
      expect(f.preview.captured.size).toBe(0);
    } finally {
      f.dispose();
    }
  }
});

test("pinch handoff and remaining finger cannot become a decision swipe", () => {
  const f = fixture();
  try {
    f.pointer("pointerdown", 0);
    f.pointer("pointerdown", 30, 0, 2, "letterbox");
    f.advance(450);
    f.pointer("pointerup", 30, 0, 2, "letterbox");
    f.pointer("pointermove", 100);
    f.pointer("pointerup", 100);
    expect(f.wheel).toBe(false);
    expect(f.writes).toEqual([]);
  } finally {
    f.dispose();
  }
});

test("manual Preview pan stays available with disabled decisions", () => {
  const f = fixture();
  try {
    f.manual();
    f.disable();
    f.pointer("pointerdown", 0);
    f.pointer("pointermove", 100);
    f.pointer("pointerup", 100);
    expect(f.panX).toBe(100);
    expect(f.writes).toEqual([]);
  } finally {
    f.dispose();
  }
});
