import { expect, test } from "bun:test";
import { createEditorController } from "./editor-controller.js";
import type { LibraryBrowserView } from "../ui/library-browser-view.js";
import type { EditorViewModel } from "../ui/editor-view-contract.js";

function requestUrl(input: RequestInfo | URL): string {
  return typeof input === "string"
    ? input
    : input instanceof URL
      ? input.href
      : input.url;
}

function requestBody(init: RequestInit | undefined): Record<string, unknown> {
  if (typeof init?.body !== "string")
    throw new Error("Expected a JSON request body");
  const value: unknown = JSON.parse(init.body);
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new Error("Expected a JSON object request body");
  return value as Record<string, unknown>;
}

const initial = {
  photoId: "photo-1",
  revision: "r0",
  sourceRevision: "source-1",
  currentStepId: "selected",
  steps: [
    {
      stepId: "selected",
      module: "darktable",
      input: {
        kind: "original",
        photoId: "photo-1",
        sourceRevision: "source-1",
      },
      parameters: { schemaVersion: "v1", tree: { exposure: 0 } },
    },
  ],
};
async function previewResponse(revision: string, comparison: string) {
  const bytes = new Uint8Array(32);
  bytes.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const frame = new DataView(bytes.buffer);
  frame.setUint32(8, 13);
  bytes.set([0x49, 0x48, 0x44, 0x52], 12);
  frame.setUint32(16, 8);
  frame.setUint32(20, 4);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return new Response(bytes, {
    headers: {
      "content-type": "image/png",
      "slipstream-processing-preview-photo-id": "photo-1",
      "slipstream-processing-preview-step-id": "selected",
      "slipstream-processing-preview-source-revision": "736f757263652d31",
      "slipstream-processing-preview-recipe-revision": revision,
      "slipstream-processing-preview-comparison": comparison,
      "slipstream-processing-preview-sha256": Array.from(digest, (byte) =>
        byte.toString(16).padStart(2, "0"),
      ).join(""),
      "slipstream-processing-preview-width": "8",
      "slipstream-processing-preview-height": "4",
      "slipstream-processing-preview-geometry": "1224",
      "slipstream-processing-preview-bundle-id": "b".repeat(64),
      "slipstream-processing-preview-module": "darktable",
      "slipstream-processing-preview-adapter-schema-version":
        "darktable-adapter-1:v1",
      "slipstream-processing-preview-parameter-digest": "c".repeat(64),
      "slipstream-processing-preview-output-contract": "d".repeat(64),
      "slipstream-processing-preview-display-conversion":
        "display-transform-v1",
      "slipstream-processing-preview-identity": "e".repeat(64),
      "slipstream-processing-preview-input-sha256": "f".repeat(64),
      "slipstream-processing-preview-input-byte-length": "32",
    },
  });
}

for (const operation of ["undo", "redo"] as const) {
  test(`${operation} aborts current and baseline Preview owners before saving history`, async () => {
    let recipe = structuredClone(initial);
    let snapshot: EditorViewModel | undefined;
    let settled = Promise.withResolvers<void>();
    const presented: string[] = [];
    const pending: {
      signal: AbortSignal;
      response: {
        promise: Promise<Response>;
        resolve: (value: Response | PromiseLike<Response>) => void;
        reject: (reason?: unknown) => void;
      };
      comparison: string;
    }[] = [];
    let holdPreviews = false;
    let saveStarted = Promise.withResolvers<void>();
    const saveGate = Promise.withResolvers<Response>();
    let holdSave = false;
    let abortedAtSave: boolean[] = [];
    const controller = createEditorController(
      async (input, init) => {
        const path = input instanceof Request ? input.url : input.toString();
        if (path.endsWith("/edit"))
          return Response.json({
            photoId: "photo-1",
            editRevision: recipe.revision,
            sourceRevision: "source-1",
            currentSourceRevision: "source-1",
            sourceAvailable: true,
            requiresRebind: false,
            canSave: true,
            canPreview: true,
            canExport: true,
            current: {
              engine: "darktable",
              input: recipe.steps[0]!.input,
              controls: { exposure: { ev: 0 } },
            },
          });
        if (
          path.includes("processing-preview") ||
          path.includes("/edit/preview")
        ) {
          if (holdPreviews) {
            const response = Promise.withResolvers<Response>();
            if (!init?.signal)
              throw new Error("Preview request must own cancellation");
            pending.push({
              signal: init.signal,
              response,
              comparison: path.includes("comparison=baseline")
                ? "baseline"
                : "current",
            });
            return response.promise;
          }
          return previewResponse(
            recipe.revision,
            path.includes("comparison=baseline") ? "baseline" : "current",
          );
        }
        if (path.endsWith("processing-recipe")) {
          if (init?.method === "POST") {
            if (holdSave) {
              abortedAtSave = pending.map((item) => item.signal.aborted);
              saveStarted.resolve();
              return saveGate.promise;
            }
            if (typeof init.body !== "string")
              throw new Error("Expected recipe JSON");
            const body = JSON.parse(init.body) as Pick<
              typeof recipe,
              "steps" | "currentStepId"
            >;
            recipe = {
              ...recipe,
              ...body,
              revision: `${recipe.revision}-next`,
            };
            return Response.json({
              outcome: "saved",
              sourceRevision: "source-1",
              recipeVersion: recipe.revision,
              recipe,
            });
          }
          return Response.json({
            photoId: "photo-1",
            sourceRevision: "source-1",
            recipe,
          });
        }
        if (path.endsWith("processing-exports"))
          return Response.json({
            photoId: "photo-1",
            exports: [],
            artifacts: [],
          });
        return Response.json({ exports: [] });
      },
      {
        editorVisible: () => true,
        renderEditor: (next: EditorViewModel) => {
          snapshot = next;
          if (next.canCompare && !next.saving) settled.resolve();
        },
        presentEditorPreview: (url: string) => {
          presented.push(url);
        },
        clearEditorPreview: () => {},
      } as unknown as LibraryBrowserView,
      {
        isAlive: () => true,
        isCurrentPhoto: () => true,
        currentPhoto: () => ({
          id: "photo-1",
          available: true,
          original: { kind: "raw", available: true },
          selectionState: "unflagged",
          rating: 0,
          hasSavedEdits: true,
          preview: { state: "ready" },
        }),
      },
    );
    try {
      controller.open("photo-1");
      await settled.promise;
      settled = Promise.withResolvers<void>();
      controller.composableParameters(
        "photo-1",
        JSON.stringify({ exposure: 1 }),
      );
      await settled.promise;
      if (operation === "redo") {
        settled = Promise.withResolvers<void>();
        controller.stepHistory("photo-1", "undo");
        await settled.promise;
        expect(snapshot?.canRedo).toBe(true);
      } else expect(snapshot?.canUndo).toBe(true);
      holdPreviews = true;
      controller.setComparison("photo-1", true);
      controller.requestPreview("photo-1");
      expect(pending).toHaveLength(2);
      holdSave = true;
      saveStarted = Promise.withResolvers<void>();
      controller.stepHistory("photo-1", operation);
      await saveStarted.promise;
      expect(abortedAtSave).toEqual([true, true]);
      expect(snapshot?.comparing).toBe(false);
      const before = presented.length;
      for (const item of pending)
        item.response.resolve(
          await previewResponse(recipe.revision, item.comparison),
        );
      await Promise.resolve();
      await Promise.resolve();
      expect(presented).toHaveLength(before);
    } finally {
      controller.leave();
      saveGate.resolve(
        Response.json(
          { error: { code: "invalid_parameters" } },
          { status: 422 },
        ),
      );
    }
  });
}

test("XMP exports use the confirmed recipe binding while the Original observation is absent", async () => {
  const ready = Promise.withResolvers<void>();
  const submitted = Promise.withResolvers<void>();
  let body: unknown;
  let snapshot: EditorViewModel | undefined;
  const controller = createEditorController(
    (input, init) => {
      const path = input instanceof Request ? input.url : input.toString();
      if (path.endsWith("/edit"))
        return Promise.resolve(
          Response.json({
            photoId: "photo-1",
            editRevision: initial.revision,
            sourceRevision: "source-1",
            currentSourceRevision: null,
            sourceAvailable: false,
            requiresRebind: false,
            canSave: false,
            canPreview: false,
            canExport: false,
            current: {
              engine: "darktable",
              input: initial.steps[0]!.input,
              controls: { exposure: { ev: 0 } },
            },
          }),
        );
      if (path.endsWith("processing-recipe"))
        return Promise.resolve(
          Response.json({
            photoId: "photo-1",
            sourceRevision: "",
            currentSourceRevision: null,
            sourceAvailable: false,
            recipe: initial,
          }),
        );
      if (init?.method === "POST") {
        if (typeof init.body !== "string") throw new Error("Expected XMP JSON");
        body = JSON.parse(init.body);
        submitted.resolve();
        return Promise.resolve(
          Response.json(
            { error: { code: "invalid_parameters" } },
            { status: 422 },
          ),
        );
      }
      if (path.includes("processing-preview") || path.includes("/edit/preview"))
        return Promise.resolve(
          Response.json(
            { error: { code: "resource_unavailable" } },
            { status: 503 },
          ),
        );
      if (path.endsWith("processing-exports"))
        return Promise.resolve(
          Response.json({ photoId: "photo-1", exports: [], artifacts: [] }),
        );
      return Promise.resolve(Response.json({ exports: [] }));
    },
    {
      editorVisible: () => true,
      renderEditor: (next: EditorViewModel) => {
        snapshot = next;
        if (next.outputs.xmp.canSubmit) ready.resolve();
      },
      presentEditorPreview: () => {},
      clearEditorPreview: () => {},
    } as unknown as LibraryBrowserView,
    {
      isAlive: () => true,
      isCurrentPhoto: () => true,
      currentPhoto: () => ({
        id: "photo-1",
        available: false,
        original: { kind: "raw", available: false },
        selectionState: "unflagged",
        rating: 0,
        hasSavedEdits: true,
        preview: { state: "unavailable" },
      }),
    },
  );
  try {
    controller.open("photo-1");
    await ready.promise;
    expect(snapshot?.conflict).toBeNull();
    expect(snapshot?.rebindAvailable).toBe(false);
    controller.submitXmp("photo-1");
    await submitted.promise;
    expect(body).toEqual(
      expect.objectContaining({
        expectedRecipeVersion: "r0",
        expectedSourceRevision: "source-1",
      }),
    );
  } finally {
    controller.leave();
  }
});

test("ordinary exposure and reset use primary routes while advanced parameters keep recipe writes", async () => {
  let recipe = structuredClone(initial);
  let ev = 0;
  let snapshot: EditorViewModel | undefined;
  let settled = Promise.withResolvers<void>();
  let expectedRevision = "r0";
  const writes: { path: string; body: Record<string, unknown> }[] = [];
  const controller = createEditorController(
    async (input, options) => {
      const path = requestUrl(input);
      if (options?.method === "POST") {
        const body = requestBody(options);
        writes.push({ path, body });
        recipe = { ...recipe, revision: `${recipe.revision}-next` };
        if (path.endsWith("/edit/set")) ev = body["value"] as number;
        if (path.endsWith("/edit/reset")) ev = 0;
        if (path.endsWith("processing-recipe")) {
          recipe = {
            ...recipe,
            steps: body["steps"] as typeof recipe.steps,
            currentStepId: body["currentStepId"] as string,
          };
          return Response.json({
            outcome: "saved",
            sourceRevision: "source-1",
            recipeVersion: recipe.revision,
            recipe,
          });
        }
        return Response.json({ outcome: "saved" });
      }
      if (path.endsWith("/edit"))
        return Response.json({
          photoId: "photo-1",
          editRevision: recipe.revision,
          sourceRevision: "source-1",
          currentSourceRevision: "source-1",
          sourceAvailable: true,
          requiresRebind: false,
          canSave: true,
          canPreview: true,
          canExport: true,
          current: {
            engine: "darktable",
            input: recipe.steps[0]!.input,
            controls: { exposure: { ev } },
          },
        });
      if (path.endsWith("processing-recipe"))
        return Response.json({
          photoId: "photo-1",
          sourceRevision: "source-1",
          recipe,
        });
      if (path.includes("/edit/preview"))
        return previewResponse(recipe.revision, "current");
      return Response.json({ exports: [], artifacts: [] });
    },
    {
      editorVisible: () => true,
      renderEditor: (next: EditorViewModel) => {
        snapshot = next;
        if (
          next.primary?.revision === expectedRevision &&
          next.primary.canSet &&
          !next.saving &&
          !next.dirty
        )
          settled.resolve();
      },
      presentEditorPreview: () => {},
      clearEditorPreview: () => {},
    } as unknown as LibraryBrowserView,
    {
      isAlive: () => true,
      isCurrentPhoto: () => true,
      currentPhoto: () => ({
        id: "photo-1",
        available: true,
        original: { kind: "raw", available: true },
        selectionState: "undecided",
        rating: 0,
        hasSavedEdits: true,
        preview: { state: "ready" },
      }),
    },
  );
  try {
    controller.open("photo-1");
    await settled.promise;
    settled = Promise.withResolvers<void>();
    expectedRevision = "r0-next";
    controller.setExposure("photo-1", 1.25);
    await settled.promise;
    expect(snapshot?.primary?.exposureEv).toBe(1.25);
    expect(writes[0]?.path).toBe("/api/photos/photo-1/edit/set");
    expect(writes[0]?.body).toMatchObject({
      expectedEditRevision: "r0",
      target: "darktable.exposure",
      control: "ev",
      value: 1.25,
    });
    settled = Promise.withResolvers<void>();
    expectedRevision = "r0-next-next";
    controller.resetExposure("photo-1");
    await settled.promise;
    expect(snapshot?.primary?.exposureEv).toBe(0);
    expect(writes[1]?.path).toBe("/api/photos/photo-1/edit/reset");
    expect(writes[1]?.body).toMatchObject({
      expectedEditRevision: "r0-next",
      target: "darktable.exposure",
      control: "ev",
    });
    settled = Promise.withResolvers<void>();
    expectedRevision = "r0-next-next-next";
    controller.composableParameters("photo-1", JSON.stringify({ exposure: 2 }));
    await settled.promise;
    expect(writes.map((request) => request.path)).toEqual([
      "/api/photos/photo-1/edit/set",
      "/api/photos/photo-1/edit/reset",
      "/api/photos/photo-1/processing-recipe",
    ]);
  } finally {
    controller.leave();
  }
});
