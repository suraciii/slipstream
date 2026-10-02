import { expect, test } from "bun:test";
import { createEditorComposablePreview } from "./editor-composable-preview.js";
import { createEditorProxyController } from "./editor-proxy-controller.js";
import type { ComposableRecipe } from "../api/composable-recipe.js";
const recipe: ComposableRecipe = {
  photoId: "photo-1",
  revision: "recipe-1",
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
      parameters: { schemaVersion: "darktable-params-1", tree: {} },
    },
  ],
};

test("a delayed Preview body cannot change a reopened photo scope", async () => {
  const started = Promise.withResolvers<void>();
  const body = Promise.withResolvers<unknown>();
  const response = new Response(null, { status: 202 });
  response.json = () => {
    started.resolve();
    return body.promise;
  };
  let owned = true;
  let renders = 0;
  const preview = createEditorComposablePreview(
    () => Promise.resolve(response),
    {
      cameraReference: () => false,
      read: () => ({ sourceRevision: "source-1", recipe }),
      isDirty: () => false,
      editorOwnsPhoto: () => owned,
      renderEditor: () => {
        renders++;
      },
      present: () => {
        throw new Error("An obsolete Preview must not be presented");
      },
      clearPresented: () => {},
      describePreviewRefusal: () => Promise.resolve("refused"),
    },
  );
  const request = preview.requestCurrent("photo-1");
  await started.promise;
  owned = false;
  preview.clear();
  owned = true;
  const before = renders;
  body.resolve({ state: "running" });
  await request;
  expect(preview.current.note).toBe("");
  expect(renders).toBe(before);
});

test("a parameter edit invalidates a pending Preview of the same selected step", async () => {
  const started = Promise.withResolvers<void>();
  const body = Promise.withResolvers<unknown>();
  const response = new Response(null, { status: 202 });
  response.json = () => {
    started.resolve();
    return body.promise;
  };
  let renders = 0;
  let presented = 0;
  const preview = createEditorComposablePreview(
    () => Promise.resolve(response),
    {
      cameraReference: () => false,
      read: () => ({ sourceRevision: "source-1", recipe }),
      isDirty: () => false,
      editorOwnsPhoto: () => true,
      renderEditor: () => {
        renders++;
      },
      present: () => {
        presented++;
      },
      clearPresented: () => {},
      describePreviewRefusal: () => Promise.resolve("refused"),
    },
  );
  const pending = preview.requestCurrent("photo-1");
  await started.promise;
  preview.markStale();
  const before = renders;
  const note = preview.current.note;
  body.resolve({ state: "running" });
  await pending;
  expect(preview.current.note).toBe(note);
  expect(renders).toBe(before);
  expect(presented).toBe(0);
});

test("Camera reference and an empty selected step issue no processing Preview", async () => {
  let cameraReference = true;
  let current: ComposableRecipe = recipe;
  const paths: string[] = [];
  const preview = createEditorComposablePreview(
    (input) => {
      paths.push(input instanceof Request ? input.url : input.toString());
      return Promise.resolve(new Response(null, { status: 500 }));
    },
    {
      cameraReference: () => cameraReference,
      read: () => ({ sourceRevision: "source-1", recipe: current }),
      isDirty: () => false,
      editorOwnsPhoto: () => true,
      renderEditor: () => {},
      present: () => {},
      clearPresented: () => {},
      describePreviewRefusal: () => Promise.resolve("refused"),
    },
  );
  await preview.requestCurrent("photo-1");
  cameraReference = false;
  current = { ...recipe, currentStepId: null };
  await preview.requestCurrent("photo-1");
  expect(paths).toEqual([]);
});

test("proxy creation uses current source facts without a legacy edit session", async () => {
  const bodies: unknown[] = [];
  const proxy = createEditorProxyController(
    (_input, init) => {
      if (init?.method === "POST") {
        if (typeof init.body !== "string")
          throw new Error("Expected a JSON request body");
        bodies.push(JSON.parse(init.body));
      }
      return Promise.resolve(
        Response.json({
          photoId: "photo-1",
          state: "absent",
          proxy: null,
          failure: null,
        }),
      );
    },
    {
      sourceRevision: () => "current-source",
      editorOwnsPhoto: () => true,
      renderEditor: () => {},
      refreshSource: () => {},
    },
  );
  await proxy.create("photo-1");
  expect(bodies).toEqual([
    expect.objectContaining({ expectedSourceRevision: "current-source" }),
  ]);
  proxy.leave();
});
