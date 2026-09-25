import { describe, expect, test } from "bun:test";
import {
  createPhotoEditor,
  type DraftStore,
  type EditorFacts,
  type EditorStep,
} from "./photo-editor.js";

const memoryStore = () => {
  const values = new Map<string, string>();
  const store: DraftStore & { values: Map<string, string> } = {
    values,
    read: (key) => values.get(key) ?? null,
    write: (key, value) => {
      values.set(key, value);
      return true;
    },
    remove: (key) => {
      values.delete(key);
    },
  };
  return store;
};

const facts = (overrides: Partial<EditorFacts> = {}): EditorFacts => ({
  photoId: "photo-1",
  sourceRevision: "rev-1",
  recipeVersion: null,
  settings: { exposureEv: 0, whiteBalance: { mode: "as-shot" } },
  sourceSupport: "supported",
  supportReason: "",
  processingAvailable: true,
  controls: {
    minimumEv: 0,
    maximumEv: 1,
    stepEv: 0.001,
    whiteBalanceModes: ["as-shot"],
    adjustableWhiteBalance: [],
  },
  ...overrides,
});

const request = (step: EditorStep) => {
  if (!step.request) throw new Error("expected a write");
  return step.request;
};

describe("one edit action is one guarded write", () => {
  test("a completed action saves without a Save button", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(facts());
    const step = editor.commitExposure(0.25);
    expect(step.request).toEqual({
      id: "req-1",
      photoId: "photo-1",
      expectedRecipeVersion: null,
      expectedSourceRevision: "rev-1",
      settings: { exposureEv: 0.25, whiteBalance: { mode: "as-shot" } },
    });
    expect(step.presentation.saving).toBe(true);
    expect(step.presentation.dirty).toBe(true);
  });

  test("later actions coalesce into one pending write while a write is in flight", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(facts());
    const first = request(editor.commitExposure(0.1));
    expect(editor.commitExposure(0.2).request).toBeNull();
    const step = editor.acknowledge(first, {
      recipeVersion: "recipe-2",
      sourceRevision: "rev-1",
    });
    // The acknowledgement confirms only its own settings and never marks the
    // later intent saved.
    expect(step.presentation.confirmed.exposureEv).toBe(0.1);
    expect(step.presentation.settings.exposureEv).toBe(0.2);
    expect(step.request?.expectedRecipeVersion).toBe("recipe-2");
  });

  test("a late acknowledgement never confirms later settings", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(facts());
    const first = request(editor.commitExposure(0.1));
    editor.commitExposure(0.9);
    const step = editor.acknowledge(first, {
      recipeVersion: "recipe-2",
      sourceRevision: "rev-1",
    });
    expect(step.presentation.status).not.toBe("Saved.");
    expect(step.presentation.confirmed.exposureEv).toBe(0.1);
  });

  test("an admitted value outside the closed control is clamped to the step", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(facts());
    const step = editor.commitExposure(1.4004);
    expect(request(step).settings.exposureEv).toBe(1);
    const editorTwo = createPhotoEditor({ nextRequestId: () => "req-1" });
    editorTwo.open(facts());
    const stepped = editorTwo.commitExposure(0.12345);
    expect(request(stepped).settings.exposureEv).toBe(0.123);
  });
});

describe("session undo, redo, and reset", () => {
  test("undo and redo write the resulting settings under the same guard", () => {
    let counter = 0;
    const editor = createPhotoEditor({
      nextRequestId: () => `req-${++counter}`,
    });
    editor.open(facts());
    const first = request(editor.commitExposure(0.3));
    editor.acknowledge(first, {
      recipeVersion: "recipe-2",
      sourceRevision: "rev-1",
    });
    const undo = editor.undo();
    expect(undo.presentation.settings.exposureEv).toBe(0);
    const undoWrite = request(undo);
    expect(undoWrite.expectedRecipeVersion).toBe("recipe-2");
    // The undo write is in flight, so the redo intent coalesces behind it and
    // advances the stream once that write settles.
    const redo = editor.redo();
    expect(redo.presentation.settings.exposureEv).toBe(0.3);
    expect(redo.request).toBeNull();
    const settled = editor.acknowledge(undoWrite, {
      recipeVersion: "recipe-3",
      sourceRevision: "rev-1",
    });
    expect(settled.request?.settings.exposureEv).toBe(0.3);
    expect(settled.request?.expectedRecipeVersion).toBe("recipe-3");
  });

  test("reset returns to the as-shot baseline as one action", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(
      facts({
        recipeVersion: "recipe-1",
        settings: { exposureEv: 0.5, whiteBalance: { mode: "as-shot" } },
      }),
    );
    const step = editor.reset();
    expect(step.presentation.settings.exposureEv).toBe(0);
    expect(step.presentation.confirmed.exposureEv).toBe(0.5);
    expect(step.presentation.canRedo).toBe(false);
    expect(request(step).expectedRecipeVersion).toBe("recipe-1");
  });

  test("a settled session keeps no history for a reopened Photo", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(facts());
    editor.commitExposure(0.4);
    editor.open(
      facts({
        recipeVersion: "recipe-9",
        settings: { exposureEv: 0.4, whiteBalance: { mode: "as-shot" } },
      }),
    );
    expect(editor.presentation().canUndo).toBe(false);
    expect(editor.presentation().confirmed.exposureEv).toBe(0.4);
  });
});

describe("conflict reconciliation", () => {
  test("a conflict stops autosave and keeps the local settings", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(facts());
    const first = request(editor.commitExposure(0.4));
    const step = editor.refuse(first, {
      status: 409,
      code: "recipe_conflict",
      message: "The expected recipe revision is no longer current",
      currentRecipeVersion: "recipe-7",
      currentSourceRevision: "rev-1",
    });
    expect(step.request).toBeNull();
    expect(step.presentation.conflict?.localSettings.exposureEv).toBe(0.4);
    expect(step.presentation.settings.exposureEv).toBe(0.4);
    // Autosave stays stopped while the conflict is unresolved.
    expect(editor.commitExposure(0.6).request).toBeNull();
    expect(editor.presentation().settings.exposureEv).toBe(0.6);
    expect(editor.presentation().canUndo).toBe(false);
  });

  test("reapplying local settings writes against the newly observed revision", () => {
    let counter = 0;
    const editor = createPhotoEditor({
      nextRequestId: () => `req-${++counter}`,
    });
    editor.open(facts());
    const first = request(editor.commitExposure(0.4));
    editor.refuse(first, {
      status: 409,
      code: "recipe_conflict",
      message: "conflict",
      currentRecipeVersion: "recipe-7",
      currentSourceRevision: "rev-1",
    });
    const step = editor.reapplyLocal();
    expect(request(step).expectedRecipeVersion).toBe("recipe-7");
    expect(request(step).settings.exposureEv).toBe(0.4);
    // The refused identity is not reused for the reapply.
    expect(request(step).id).not.toBe(first.id);
  });

  test("using the saved recipe drops the local settings", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(
      facts({
        recipeVersion: "recipe-1",
        settings: { exposureEv: 0.2, whiteBalance: { mode: "as-shot" } },
      }),
    );
    const first = request(editor.commitExposure(0.4));
    editor.refuse(first, {
      status: 409,
      code: "source_changed",
      message: "source moved",
      currentRecipeVersion: "recipe-1",
      currentSourceRevision: "rev-2",
    });
    const step = editor.useSavedRecipe();
    expect(step.presentation.settings.exposureEv).toBe(0.2);
    expect(step.presentation.conflict).toBeNull();
    expect(step.request).toBeNull();
  });

  test("the saved recipe is the service's, not this client's older copy", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(
      facts({
        recipeVersion: "recipe-1",
        settings: { exposureEv: 0.2, whiteBalance: { mode: "as-shot" } },
      }),
    );
    const first = request(editor.commitExposure(0.4));
    editor.refuse(first, {
      status: 409,
      code: "recipe_conflict",
      message: "the recipe moved",
      currentRecipeVersion: "recipe-2",
      currentSourceRevision: "rev-1",
    });
    // Another client's recipe is read before the saved recipe is adopted.
    editor.refresh(
      facts({
        recipeVersion: "recipe-2",
        settings: { exposureEv: 0.35, whiteBalance: { mode: "as-shot" } },
      }),
    );
    const step = editor.useSavedRecipe();
    expect(step.presentation.settings.exposureEv).toBe(0.35);
    expect(step.presentation.conflict).toBeNull();
    // The next action is guarded by the revision the service reported.
    expect(request(editor.commitExposure(0.4)).expectedRecipeVersion).toBe(
      "recipe-2",
    );
  });

  test("a read while a conflict stands keeps the local intent", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(
      facts({
        recipeVersion: "recipe-1",
        settings: { exposureEv: 0.2, whiteBalance: { mode: "as-shot" } },
      }),
    );
    const first = request(editor.commitExposure(0.4));
    editor.refuse(first, {
      status: 409,
      code: "recipe_conflict",
      message: "the recipe moved",
      currentRecipeVersion: "recipe-2",
      currentSourceRevision: "rev-1",
    });
    const step = editor.refresh(
      facts({
        recipeVersion: "recipe-2",
        settings: { exposureEv: 0.35, whiteBalance: { mode: "as-shot" } },
      }),
    );
    expect(step.presentation.settings.exposureEv).toBe(0.4);
    expect(step.presentation.conflict?.savedSettings.exposureEv).toBe(0.35);
    expect(step.request).toBeNull();
  });

  test("a read that moves the recipe forward resolves the write stream", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(facts());
    editor.commitExposure(0.4);
    const step = editor.refresh(
      facts({
        recipeVersion: "recipe-3",
        settings: { exposureEv: 0.4, whiteBalance: { mode: "as-shot" } },
      }),
    );
    expect(step.presentation.confirmed.exposureEv).toBe(0.4);
    expect(step.presentation.dirty).toBe(false);
  });

  test("an unknown outcome keeps the identity for the same receipt", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(facts());
    const first = request(editor.commitExposure(0.4));
    const step = editor.refuse(first, {
      status: 500,
      code: "outcome_unknown",
      message: "lost response",
      currentRecipeVersion: null,
      currentSourceRevision: null,
    });
    expect(step.request).toBeNull();
    expect(step.presentation.status).toContain("unknown");
    expect(editor.presentation().settings.exposureEv).toBe(0.4);
  });
});

describe("bounded local draft", () => {
  test("a draft is persisted before the write and cleared on confirmation", () => {
    const store = memoryStore();
    const editor = createPhotoEditor({
      store,
      nextRequestId: () => "req-1",
    });
    editor.open(facts());
    const first = request(editor.commitExposure(0.4));
    const persisted = store.read("slipstream.photo-draft.v1.photo-1");
    expect(persisted).not.toBeNull();
    expect(
      (JSON.parse(persisted ?? "{}") as { requestId?: string }).requestId,
    ).toBe("req-1");
    editor.acknowledge(first, {
      recipeVersion: "recipe-2",
      sourceRevision: "rev-1",
    });
    expect(store.read("slipstream.photo-draft.v1.photo-1")).toBeNull();
  });

  test("a recovered draft on the current revision replays its own identity", () => {
    const store = memoryStore();
    const first = createPhotoEditor({ store, nextRequestId: () => "req-1" });
    first.open(facts());
    request(first.commitExposure(0.4));
    const reopened = createPhotoEditor({ store, nextRequestId: () => "req-2" });
    const step = reopened.open(facts());
    expect(step.presentation.settings.exposureEv).toBe(0.4);
    expect(step.presentation.draft.kind).toBe("recovered");
    expect(step.request?.id).toBe("req-1");
  });

  test("a recovered draft from an earlier revision is never replayed silently", () => {
    const store = memoryStore();
    const first = createPhotoEditor({ store, nextRequestId: () => "req-1" });
    first.open(facts());
    request(first.commitExposure(0.4));
    const reopened = createPhotoEditor({ store, nextRequestId: () => "req-2" });
    const step = reopened.open(
      facts({
        recipeVersion: "recipe-8",
        settings: { exposureEv: 0.9, whiteBalance: { mode: "as-shot" } },
      }),
    );
    expect(step.request).toBeNull();
    expect(step.presentation.conflict?.savedSettings.exposureEv).toBe(0.9);
    expect(step.presentation.conflict?.localSettings.exposureEv).toBe(0.4);
    expect(step.presentation.settings.exposureEv).toBe(0.4);
  });

  test("an unavailable store keeps the draft for the session and discloses it", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(facts());
    const step = editor.commitExposure(0.4);
    expect(step.request).not.toBeNull();
    expect(step.presentation.draft.kind).toBe("session");
    expect(step.presentation.draft.note).toContain("session");
  });

  test("an unconfirmed draft is never evicted to make room for another", () => {
    const values = new Map<string, string>();
    let acceptWrites = true;
    const store: DraftStore = {
      read: (key) => values.get(key) ?? null,
      write: (key, value) => {
        if (!acceptWrites) return false;
        values.set(key, value);
        return true;
      },
      remove: (key) => {
        values.delete(key);
      },
    };
    const editor = createPhotoEditor({ store, nextRequestId: () => "req-1" });
    editor.open(facts());
    request(editor.commitExposure(0.4));
    const held = values.get("slipstream.photo-draft.v1.photo-1");
    acceptWrites = false;
    const second = createPhotoEditor({ store, nextRequestId: () => "req-2" });
    second.open(facts({ photoId: "photo-2" }));
    const step = second.commitExposure(0.3);
    expect(step.presentation.draft.kind).toBe("session");
    expect(values.get("slipstream.photo-draft.v1.photo-1")).toBe(held);
  });

  test("discarding a draft removes it without touching the settings", () => {
    const store = memoryStore();
    const editor = createPhotoEditor({ store, nextRequestId: () => "req-1" });
    editor.open(facts());
    request(editor.commitExposure(0.4));
    const presentation = editor.discardDraft();
    expect(store.read("slipstream.photo-draft.v1.photo-1")).toBeNull();
    expect(presentation.settings.exposureEv).toBe(0.4);
    expect(presentation.draft.kind).toBe("none");
  });
});

describe("sources without editing", () => {
  test("an unavailable source explains itself and admits no write", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    const step = editor.open(
      facts({
        sourceRevision: null,
        sourceSupport: "unavailable",
        supportReason: "original-missing",
      }),
    );
    expect(step.request).toBeNull();
    expect(step.presentation.canEdit).toBe(false);
    expect(step.presentation.status).toContain("missing");
    expect(editor.commitExposure(0.4).request).toBeNull();
  });

  test("a later read that loses the source invalidates the session", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(
      facts({
        recipeVersion: "recipe-1",
        settings: { exposureEv: 0.2, whiteBalance: { mode: "as-shot" } },
      }),
    );
    const saved = request(editor.commitExposure(0.4));
    editor.acknowledge(saved, {
      recipeVersion: "recipe-2",
      sourceRevision: "rev-1",
    });
    const step = editor.refresh(
      facts({
        sourceRevision: null,
        recipeVersion: null,
        sourceSupport: "unavailable",
        supportReason: "original-unreadable",
        settings: { exposureEv: 0, whiteBalance: { mode: "as-shot" } },
      }),
    );
    // The workspace explains the source, and no stale local history can be
    // replayed against a source the service no longer admits.
    expect(step.presentation.status).toContain("cannot be read right now");
    expect(step.presentation.canEdit).toBe(false);
    expect(step.presentation.canUndo).toBe(false);
    expect(step.presentation.canRedo).toBe(false);
    expect(step.request).toBeNull();
    expect(editor.commitExposure(0.5).request).toBeNull();
  });

  test("an unsupported source class admits no write", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    const step = editor.open(facts({ sourceSupport: "unsupported" }));
    expect(step.presentation.canEdit).toBe(false);
    expect(step.presentation.status).toContain("approved profile");
  });
});

describe("the closed white balance", () => {
  const admittedRange = {
    mode: "temperature-tint" as const,
    temperatureKelvin: { minimum: 2500, maximum: 10000 },
    tintMilli: { minimum: -100, maximum: 100 },
  };

  test("a retained intent the deployment does not admit stays readable and unchanged", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(
      facts({
        recipeVersion: "recipe-1",
        settings: {
          exposureEv: 0.5,
          whiteBalance: {
            mode: "temperature-tint",
            temperatureKelvin: 6500,
            tintMilli: -10,
          },
        },
      }),
    );
    const presentation = editor.presentation();
    expect(presentation.whiteBalance.intent).toEqual({
      mode: "temperature-tint",
      temperatureKelvin: 6500,
      tintMilli: -10,
    });
    expect(presentation.whiteBalance.adjustable).toBe(false);
    expect(presentation.whiteBalance.temperatureKelvin).toBeNull();
    expect(presentation.whiteBalance.note).toContain(
      "as-shot white balance only",
    );
    // An unrelated edit writes the retained intent back, never as-shot.
    const step = editor.commitExposure(0.7);
    expect(request(step).settings).toEqual({
      exposureEv: 0.7,
      whiteBalance: {
        mode: "temperature-tint",
        temperatureKelvin: 6500,
        tintMilli: -10,
      },
    });
  });

  test("a deployment that admits the mode but reports no range keeps it read-only", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(
      facts({
        controls: {
          ...facts().controls,
          whiteBalanceModes: ["as-shot", "temperature-tint"],
        },
      }),
    );
    const presentation = editor.presentation();
    expect(presentation.whiteBalance.adjustable).toBe(false);
    expect(presentation.whiteBalance.note).toContain(
      "does not report the range",
    );
    expect(
      editor.selectWhiteBalanceMode("temperature-tint").request,
    ).toBeNull();
  });

  test("an admitted adjustable mode enables the controls from the reported range", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(
      facts({
        controls: {
          ...facts().controls,
          whiteBalanceModes: ["as-shot", "temperature-tint"],
          adjustableWhiteBalance: [admittedRange],
        },
      }),
    );
    const presentation = editor.presentation();
    expect(presentation.whiteBalance.adjustable).toBe(true);
    expect(presentation.whiteBalance.note).toBe("");
    expect(presentation.whiteBalance.temperatureKelvin).toEqual({
      value: 2500,
      minimum: 2500,
      maximum: 10000,
      enabled: true,
    });
    const selected = editor.selectWhiteBalanceMode("temperature-tint");
    expect(request(selected).settings.whiteBalance).toEqual({
      mode: "temperature-tint",
      temperatureKelvin: 6250,
      tintMilli: 0,
    });
  });

  test("temperature and tint commit as integers inside the admitted range", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(
      facts({
        controls: {
          ...facts().controls,
          whiteBalanceModes: ["as-shot", "temperature-tint"],
          adjustableWhiteBalance: [admittedRange],
        },
      }),
    );
    const temperature = request(editor.commitTemperature(30000.4));
    expect(temperature.settings.whiteBalance).toEqual({
      mode: "temperature-tint",
      temperatureKelvin: 10000,
      tintMilli: 0,
    });
    editor.acknowledge(temperature, {
      recipeVersion: "recipe-2",
      sourceRevision: "rev-1",
    });
    const tint = request(editor.commitTint(-100000));
    expect(tint.settings.whiteBalance).toEqual({
      mode: "temperature-tint",
      temperatureKelvin: 10000,
      tintMilli: -100,
    });
  });

  test("selecting as-shot and the individual resets each keep the other control", () => {
    const editor = createPhotoEditor({ nextRequestId: () => "req-1" });
    editor.open(
      facts({
        recipeVersion: "recipe-1",
        controls: {
          ...facts().controls,
          whiteBalanceModes: ["as-shot", "temperature-tint"],
          adjustableWhiteBalance: [admittedRange],
        },
      }),
    );
    const warm = request(editor.commitTemperature(8000));
    editor.acknowledge(warm, {
      recipeVersion: "recipe-2",
      sourceRevision: "rev-1",
    });
    const exposure = request(editor.commitExposure(0.6));
    editor.acknowledge(exposure, {
      recipeVersion: "recipe-3",
      sourceRevision: "rev-1",
    });
    // Reset exposure keeps the temperature and tint intent.
    const exposureReset = request(editor.resetExposure());
    expect(exposureReset.settings).toEqual({
      exposureEv: 0,
      whiteBalance: {
        mode: "temperature-tint",
        temperatureKelvin: 8000,
        tintMilli: 0,
      },
    });
    editor.acknowledge(exposureReset, {
      recipeVersion: "recipe-4",
      sourceRevision: "rev-1",
    });
    // Reset white balance restores as-shot and keeps exposure.
    expect(editor.presentation().whiteBalance.resettable).toBe(true);
    const whiteBalanceReset = request(editor.resetWhiteBalance());
    expect(whiteBalanceReset.settings).toEqual({
      exposureEv: 0,
      whiteBalance: { mode: "as-shot" },
    });
    expect(editor.presentation().whiteBalance.resettable).toBe(false);
  });

  test("a draft carries the closed intent through a reload", () => {
    const store = memoryStore();
    const first = createPhotoEditor({
      store,
      nextRequestId: () => "req-1",
    });
    first.open(
      facts({
        controls: {
          ...facts().controls,
          whiteBalanceModes: ["as-shot", "temperature-tint"],
          adjustableWhiteBalance: [admittedRange],
        },
      }),
    );
    request(first.commitTemperature(5000));
    const reopened = createPhotoEditor({ store, nextRequestId: () => "req-2" });
    const step = reopened.open(
      facts({
        controls: {
          ...facts().controls,
          whiteBalanceModes: ["as-shot", "temperature-tint"],
          adjustableWhiteBalance: [admittedRange],
        },
      }),
    );
    expect(step.presentation.settings.whiteBalance).toEqual({
      mode: "temperature-tint",
      temperatureKelvin: 5000,
      tintMilli: 0,
    });
    expect(step.request?.id).toBe("req-1");
  });
});

describe("resolving a save whose outcome is unknown", () => {
  const unknownRefusal = () => ({
    status: 500,
    code: "outcome_unknown",
    message: "lost response",
    currentRecipeVersion: null,
    currentSourceRevision: null,
  });

  test("the identical retry carries the payload the identity was sent with", () => {
    let counter = 0;
    const editor = createPhotoEditor({
      nextRequestId: () => `req-${++counter}`,
    });
    editor.open(facts());
    const first = request(editor.commitExposure(0.4));
    editor.refuse(first, unknownRefusal());
    // A later intent does not advance the stream: the unresolved operation is
    // resolved first, under its own identity and its own payload.
    const retry = request(editor.commitExposure(0.9));
    expect(retry.id).toBe("req-1");
    expect(retry.settings.exposureEv).toBe(0.4);
    const resolved = editor.acknowledge(retry, {
      recipeVersion: "recipe-2",
      sourceRevision: "rev-1",
    });
    // The coalesced intent follows under a fresh identity once the operation
    // has settled.
    expect(resolved.request?.id).not.toBe("req-1");
    expect(resolved.request?.settings.exposureEv).toBe(0.9);
    expect(resolved.request?.expectedRecipeVersion).toBe("recipe-2");
  });

  test("an expired receipt writes again under a new identity", () => {
    let counter = 0;
    const editor = createPhotoEditor({
      nextRequestId: () => `req-${++counter}`,
    });
    editor.open(facts());
    const first = request(editor.commitExposure(0.4));
    const step = editor.refuse(first, {
      status: 410,
      code: "receipt_expired",
      message: "the receipt expired",
      currentRecipeVersion: null,
      currentSourceRevision: null,
    });
    // Expiry never frees the identity, so the intent is written again under a
    // new one instead of replaying a receipt that cannot resolve it.
    expect(step.request?.id).toBe("req-2");
    expect(step.request?.settings.exposureEv).toBe(0.4);
    expect(step.presentation.status).toContain("expired");
  });

  test("a reused identity with another payload writes again under a new one", () => {
    let counter = 0;
    const editor = createPhotoEditor({
      nextRequestId: () => `req-${++counter}`,
    });
    editor.open(facts());
    const first = request(editor.commitExposure(0.4));
    const step = editor.refuse(first, {
      status: 409,
      code: "request_conflict",
      message: "the identity was used with another payload",
      currentRecipeVersion: null,
      currentSourceRevision: null,
    });
    expect(step.request?.id).toBe("req-2");
    expect(step.request?.settings.exposureEv).toBe(0.4);
  });

  test("the export barrier writes the settings in force and nothing else", () => {
    let counter = 0;
    const editor = createPhotoEditor({
      nextRequestId: () => `req-${++counter}`,
    });
    editor.open(facts());
    // Settings already confirmed: the barrier commits nothing.
    expect(editor.commitCurrent().request).toBeNull();
    const written = request(editor.commitExposure(0.3));
    editor.acknowledge(written, {
      recipeVersion: "recipe-2",
      sourceRevision: "rev-1",
    });
    // A refused save leaves the settings unconfirmed and nothing in flight,
    // which is exactly when the Export must commit the visible intent itself.
    const refused = request(editor.commitExposure(0.6));
    editor.refuse(refused, {
      status: 422,
      code: "invalid_settings",
      message: "the settings are not valid",
      currentRecipeVersion: null,
      currentSourceRevision: null,
    });
    const committed = request(editor.commitCurrent());
    expect(committed.settings.exposureEv).toBe(0.6);
    expect(committed.expectedRecipeVersion).toBe("recipe-2");
    // Once confirmed, the barrier has nothing left to write.
    editor.acknowledge(committed, {
      recipeVersion: "recipe-3",
      sourceRevision: "rev-1",
    });
    expect(editor.commitCurrent().request).toBeNull();
  });
});
