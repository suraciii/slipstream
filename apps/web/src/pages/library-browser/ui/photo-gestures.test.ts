import { expect, test } from "bun:test";
import { createPhotoGestures } from "./photo-gestures.js";
import type { PhotoZoomController } from "./photo-zoom.js";
import type { RatingControls } from "./rating-controls.js";
import type { LibraryBrowserIntent } from "./library-browser-view.js";

class Surface extends EventTarget {
  style = { transform: "" };
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
  let photoId = "photo-a";
  let surface = {};
  let enabled = true;
  let manual = false;
  let pinch = false;
  let wheel = false;
  let candidate: number | undefined = 4;
  let panX = 0;
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
    pointer(type: string, x: number, y = 0, id = 1) {
      const event = new Event(type, { cancelable: true });
      Object.defineProperties(event, {
        pointerId: { value: id },
        clientX: { value: x },
        clientY: { value: y },
        pointerType: { value: "touch" },
        isPrimary: { value: id === 1 },
        timeStamp: { value: now },
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
    manual() {
      manual = true;
    },
    noCandidate() {
      candidate = undefined;
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

test("movement beyond the hold boundary hands equal-axis motion to native scrolling", () => {
  const f = fixture();
  try {
    f.pointer("pointerdown", 0);
    f.pointer("pointermove", 9, 9);
    f.advance(450);
    f.pointer("pointermove", 90, 90);
    f.pointer("pointerup", 90, 90);
    expect(f.wheel).toBe(false);
    expect(f.stage.style.transform).toBe("");
    expect(f.writes).toEqual([]);
  } finally {
    f.dispose();
  }
});

test("below-threshold swipe returns to origin; committed swipe records one decision", () => {
  const f = fixture();
  try {
    f.pointer("pointerdown", 0);
    f.pointer("pointermove", 20);
    f.advance(500);
    f.pointer("pointerup", 47);
    expect(f.writes).toEqual([]);
    expect(f.stage.style.transform).toBe("");
    f.pointer("pointerdown", 0);
    f.pointer("pointermove", 80);
    f.advance(500);
    f.pointer("pointerup", 80);
    f.pointer("pointerup", 80);
    expect(f.writes).toEqual([
      {
        kind: "photo-mutation",
        field: "selectionState",
        value: "selected",
        advance: true,
      },
    ]);
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
    f.pointer("pointerdown", 30, 0, 2);
    f.advance(450);
    f.pointer("pointerup", 30, 0, 2);
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
