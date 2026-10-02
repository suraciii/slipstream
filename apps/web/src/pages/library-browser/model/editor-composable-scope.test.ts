import { describe, expect, test } from "bun:test";
import { createEditorComposableRecipe } from "./editor-composable-recipe.js";
import { createEditorProcessingExport } from "./editor-processing-export.js";
import type { BrowserFetch } from "./access-session.js";
import type { ComposableRecipe } from "../api/composable-recipe.js";
import type { ProcessingArtifactRecord } from "./processing-artifact.js";
import { createEditorComposablePreview } from "./editor-composable-preview.js";
import { createPhotoEditor } from "./photo-editor.js";

const savedRecipe = (revision = "recipe-1"): ComposableRecipe => ({
  photoId: "photo-1",
  revision,
  sourceRevision: "source-1",
  currentStepId: "develop-1",
  steps: [
    {
      stepId: "develop-1",
      module: "darktable",
      input: {
        kind: "original",
        photoId: "photo-1",
        sourceRevision: "source-1",
      },
      parameters: { schemaVersion: "darktable-params-1", tree: {} },
    },
  ],
});
const artifact: ProcessingArtifactRecord = {
  artifactId: "a".repeat(64),
  photoId: "photo-1",
  stepId: "develop-1",
  module: "darktable",
  adapterSchemaVersion: "adapter-1",
  parameters: { schemaVersion: "darktable-params-1", tree: {} },
  input: {
    binding: {
      kind: "original",
      photoId: "photo-1",
      sourceRevision: "source-1",
    },
    sha256: "b".repeat(64),
    byteLength: 3,
  },
  outputContract: {
    format: "tiff",
    precision: "float32",
    colorSpace: "prophoto-rgb",
    transfer: "linear",
    geometry: { width: 1, height: 1 },
    encoding: "deflate",
  },
  bundleId: "c".repeat(64),
  byteLength: 3,
  sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
};
function delayedJson(status: number) {
  const started = Promise.withResolvers<void>();
  const body = Promise.withResolvers<unknown>();
  const response = new Response(null, { status });
  response.json = () => {
    started.resolve();
    return body.promise;
  };
  return { response, started: started.promise, resolve: body.resolve };
}
function fixture(fetcher: BrowserFetch) {
  let ownsPhoto = true;
  let renders = 0;
  let previews = 0;
  const recipe = createEditorComposableRecipe(fetcher, {
    isAlive: () => true,
    isCurrentPhoto: () => ownsPhoto,
    currentPhoto: () => undefined,
    editorOwnsPhoto: () => ownsPhoto,
    renderEditor: () => {
      renders += 1;
    },
    stage: () => "develop",
    previewSelection: () => undefined,
    clearEditorPreview: () => {},
    requestEditorPreview: () => {
      previews += 1;
      return Promise.resolve();
    },
    markEditorPreviewStale: () => {},
    describeEditRefusal: async (response) => String(await response.json()),
  });
  const exports = createEditorProcessingExport(fetcher, recipe, {
    editorOwnsPhoto: () => ownsPhoto,
    renderEditor: () => {
      renders += 1;
    },
    processingAvailable: () => true,
    describeEditRefusal: async (response) => String(await response.json()),
  });
  return {
    recipe,
    exports,
    get renders() {
      return renders;
    },
    get previews() {
      return previews;
    },
    reopen: async () => {
      ownsPhoto = false;
      recipe.reset();
      exports.reset();
      ownsPhoto = true;
      await recipe.loadComposableRecipe("photo-1");
    },
  };
}
const recipeRead = () =>
  Response.json({
    photoId: "photo-1",
    sourceRevision: "source-1",
    recipe: savedRecipe(),
  });
const modulesRead = () =>
  Response.json({
    contractVersion: "1",
    modules: [
      {
        id: { name: "darktable", adapterVersion: "adapter-1" },
        parameterVersions: ["darktable-params-1"],
        parameterSchema: {},
        admittedInputs: [],
        admittedOutputs: [],
        limits: {},
        availability: { state: "ready", refusalReasons: [] },
      },
    ],
  });

describe("composable editor scope during response bodies", () => {
  for (const status of [201, 409, 503]) {
    test(`a save body (${status}) cannot change the reopened Photo`, async () => {
      const delayed = delayedJson(status);
      const f = fixture((input, init) => {
        const path = input instanceof Request ? input.url : input.toString();
        if (init?.method === "POST") return Promise.resolve(delayed.response);
        if (path === "/api/processing/modules")
          return Promise.resolve(modulesRead());
        return Promise.resolve(recipeRead());
      });
      await f.recipe.loadComposableRecipe("photo-1");
      await f.recipe.loadProcessingModules("photo-1");
      f.recipe.composeEditorSteps("photo-1");
      const saving = f.recipe.saveEditorComposable("photo-1");
      await delayed.started;
      await f.reopen();
      const renders = f.renders;
      const previews = f.previews;
      delayed.resolve({
        outcome: "saved",
        recipeVersion: "recipe-2",
        sourceRevision: "source-1",
        recipe: savedRecipe("recipe-2"),
        error: { code: "recipe_conflict" },
      });
      await saving;
      expect(f.recipe.read?.recipe?.revision).toBe("recipe-1");
      expect(f.recipe.view().saving).toBe(false);
      expect(f.recipe.view().savePending).toBe(false);
      expect(f.recipe.view().note).toBe("");
      expect(f.renders).toBe(renders);
      expect(f.previews).toBe(previews);
    });
  }
  for (const status of [201, 202, 409, 503]) {
    test(`an Export body (${status}) cannot publish into the reopened Photo`, async () => {
      const delayed = delayedJson(status);
      const f = fixture((_path, init) =>
        Promise.resolve(
          init?.method === "POST" ? delayed.response : recipeRead(),
        ),
      );
      await f.recipe.loadComposableRecipe("photo-1");
      const submitting = f.exports.submit("photo-1");
      await delayed.started;
      const requestId = f.exports.view().processingRequestId;
      await f.reopen();
      const renders = f.renders;
      delayed.resolve({
        artifact,
        receipt: {
          photoId: "photo-1",
          requestId,
          stepId: "develop-1",
          state: "accepted",
        },
        error: { code: "source_changed" },
      });
      await submitting;
      expect(f.exports.view().state).toBe("idle");
      expect(f.exports.view().processingArtifact).toBeNull();
      expect(f.exports.view().processingRequestId).toBeNull();
      expect(f.recipe.artifacts).toEqual([]);
      expect(f.renders).toBe(renders);
    });
  }
  test("ordinary HTTP saves and exports without crypto.randomUUID", async () => {
    const previousUuid = Object.getOwnPropertyDescriptor(crypto, "randomUUID");
    Object.defineProperty(crypto, "randomUUID", {
      configurable: true,
      value: undefined,
    });
    const requests: string[] = [];
    try {
      const f = fixture((input, init) => {
        const path = input instanceof Request ? input.url : input.toString();
        if (init?.method === "POST") {
          if (typeof init.body !== "string")
            throw new Error("Expected a JSON request body");
          const request = JSON.parse(init.body) as {
            requestId: string;
          };
          requests.push(request.requestId);
          return Promise.resolve(
            path.endsWith("processing-exports")
              ? Response.json({ artifact }, { status: 201 })
              : Response.json({
                  outcome: "saved",
                  recipeVersion: "recipe-1",
                  sourceRevision: "source-1",
                  recipe: savedRecipe(),
                }),
          );
        }
        return Promise.resolve(
          path === "/api/processing/modules" ? modulesRead() : recipeRead(),
        );
      });
      await f.recipe.loadComposableRecipe("photo-1");
      await f.recipe.loadProcessingModules("photo-1");
      await f.recipe.saveEditorComposable("photo-1");
      expect(f.recipe.view().savePending).toBe(false);
      await f.exports.submit("photo-1");
      expect(f.exports.view().state).toBe("succeeded");
      expect(f.recipe.artifacts).toEqual([artifact]);
      expect(requests[0]).toMatch(/^web-recipe-[0-9a-f]{24}$/);
      expect(requests[1]).toMatch(/^web-processing-export-[0-9a-f]{24}$/);
    } finally {
      if (previousUuid)
        Object.defineProperty(crypto, "randomUUID", previousUuid);
      else Reflect.deleteProperty(crypto, "randomUUID");
    }
  });
  test("a delayed Export poll cannot change the reopened Photo", async () => {
    const delayed = delayedJson(200);
    const f = fixture((input, init) => {
      const path = input instanceof Request ? input.url : input.toString();
      if (init?.method === "POST")
        return Promise.resolve(Response.json({ artifact }, { status: 201 }));
      if (path.includes("processing-exports/"))
        return Promise.resolve(delayed.response);
      return Promise.resolve(recipeRead());
    });
    await f.recipe.loadComposableRecipe("photo-1");
    await f.exports.submit("photo-1");
    const requestId = f.exports.view().processingRequestId;
    const checking = f.exports.check("photo-1");
    await delayed.started;
    await f.reopen();
    const renders = f.renders;
    delayed.resolve({
      requestId,
      photoId: "photo-1",
      state: "succeeded",
      artifactId: artifact.artifactId,
    });
    await checking;
    expect(f.exports.view().state).toBe("idle");
    expect(f.recipe.artifacts).toEqual([]);
    expect(f.renders).toBe(renders);
  });
  test("a delayed cancellation refusal cannot change the reopened Photo", async () => {
    const delayed = delayedJson(409);
    const f = fixture((input, init) => {
      const path = input instanceof Request ? input.url : input.toString();
      if (path.endsWith("/cancel")) return Promise.resolve(delayed.response);
      if (init?.method === "POST")
        return Promise.resolve(Response.json({ artifact }, { status: 201 }));
      return Promise.resolve(recipeRead());
    });
    await f.recipe.loadComposableRecipe("photo-1");
    await f.exports.submit("photo-1");
    const cancelling = f.exports.cancel("photo-1");
    await delayed.started;
    await f.reopen();
    const renders = f.renders;
    delayed.resolve({ error: { code: "export_terminal" } });
    await cancelling;
    expect(f.exports.view().state).toBe("idle");
    expect(f.exports.view().note).toBe("");
    expect(f.renders).toBe(renders);
  });
  for (const outcome of ["queued", "unreadable"] as const) {
    test(`a delayed ${outcome} Preview body cannot change a reopened scope`, async () => {
      const started = Promise.withResolvers<void>();
      const body = Promise.withResolvers<unknown>();
      const response = new Response(null, {
        status: outcome === "queued" ? 202 : 200,
      });
      response.json = () => {
        started.resolve();
        return body.promise;
      };
      response.blob = async () => {
        started.resolve();
        await body.promise;
        throw new Error("unreadable body");
      };
      const editor = createPhotoEditor({ nextRequestId: () => "edit-1" });
      editor.open({
        photoId: "photo-1",
        sourceRevision: "source-1",
        recipeVersion: null,
        settings: { exposureEv: 0, whiteBalance: { mode: "as-shot" } },
        sourceSupport: "supported",
        supportReason: "",
        editSource: "original",
        editSourceProxyId: null,
        processingAvailable: true,
        controls: {
          minimumEv: 0,
          maximumEv: 1,
          stepEv: 0.001,
          whiteBalanceModes: ["as-shot"],
          adjustableWhiteBalance: [],
        },
      });
      let renders = 0;
      let ownsPhoto = true;
      const preview = createEditorComposablePreview(
        () => Promise.resolve(response),
        {
          session: () => editor,
          stage: () => "develop",
          read: () => ({ sourceRevision: "source-1", recipe: savedRecipe() }),
          isDirty: () => false,
          editorOwnsPhoto: () => ownsPhoto,
          renderEditor: () => {
            renders += 1;
          },
          present: () => {
            throw new Error("stale Preview presented");
          },
          clearPresented: () => {},
          describePreviewRefusal: () => Promise.resolve("refused"),
        },
      );
      const requesting = preview.requestCurrent("photo-1");
      await started.promise;
      ownsPhoto = false;
      preview.clear();
      ownsPhoto = true;
      const previousRenders = renders;
      body.resolve({ state: "running" });
      await requesting;
      expect(preview.current.note).toBe("");
      expect(renders).toBe(previousRenders);
    });
  }
  test("artifact provenance read cannot retain an old scope's artifact", async () => {
    const delayed = delayedJson(200);
    const f = fixture((input) => {
      const path = input instanceof Request ? input.url : input.toString();
      return Promise.resolve(
        path.includes("processing-artifacts") ? delayed.response : recipeRead(),
      );
    });
    await f.recipe.loadComposableRecipe("photo-1");
    const reading = f.recipe.fetchEditorArtifact(
      "photo-1",
      artifact.artifactId,
    );
    await delayed.started;
    await f.reopen();
    const renders = f.renders;
    delayed.resolve(artifact);
    await reading;
    expect(f.recipe.artifacts).toEqual([]);
    expect(f.recipe.view().note).toBe("");
    expect(f.renders).toBe(renders);
  });
  test("a delayed download body cannot trigger a download after reopening", async () => {
    const started = Promise.withResolvers<void>();
    const body = Promise.withResolvers<Blob>();
    const response = new Response(null, {
      headers: {
        "slipstream-artifact-id": artifact.artifactId,
        "slipstream-artifact-photo-id": artifact.photoId,
        "slipstream-artifact-step-id": artifact.stepId,
        "slipstream-artifact-module": artifact.module,
        "slipstream-artifact-adapter-schema-version":
          artifact.adapterSchemaVersion,
        "slipstream-artifact-bundle-id": artifact.bundleId,
        "slipstream-artifact-width": "1",
        "slipstream-artifact-height": "1",
        "slipstream-artifact-byte-length": "3",
        "slipstream-artifact-sha256": artifact.sha256,
      },
    });
    response.blob = () => {
      started.resolve();
      return body.promise;
    };
    const f = fixture((input) => {
      const path = input instanceof Request ? input.url : input.toString();
      return Promise.resolve(path.endsWith("/bytes") ? response : recipeRead());
    });
    f.recipe.retainArtifact(artifact);
    const downloading = f.exports.downloadArtifact(
      "photo-1",
      artifact.artifactId,
    );
    await started.promise;
    await f.reopen();
    const renders = f.renders;
    body.resolve(new Blob(["abc"]));
    await downloading;
    expect(f.exports.view().note).toBe("");
    expect(f.renders).toBe(renders);
  });
});
