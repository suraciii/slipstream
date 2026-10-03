import { expect, test } from "bun:test";
import { createEditorComposableRecipe } from "./editor-composable-recipe.js";
import type { ProcessingArtifactRecord } from "./processing-artifact.js";
import type { ComposableSaveRequest } from "./composable-recipe-draft.js";

type SaveRequest = Extract<ComposableSaveRequest, { kind: "ok" }>["request"];

const artifact: ProcessingArtifactRecord = {
  artifactId: "a".repeat(64),
  photoId: "p1",
  stepId: "develop",
  module: "darktable",
  adapterSchemaVersion: "v1",
  parameters: { schemaVersion: "v1", tree: {} },
  input: {
    binding: { kind: "original", photoId: "p1", sourceRevision: "s1" },
    sha256: "b".repeat(64),
    byteLength: 128,
  },
  outputContract: {
    format: "tiff",
    precision: "float32",
    colorSpace: "prophoto-rgb",
    transfer: "linear",
    geometry: { width: 100, height: 80 },
    encoding: "tiff",
  },
  bundleId: "bundle",
  sha256: "c".repeat(64),
  byteLength: 1024,
  filename: "output.tiff",
  publishedAt: "2026-10-01T00:00:00Z",
  expiresAt: "2099-10-01T00:00:00Z",
  orientation: "top-left",
  iccEmbedded: true,
  sampleFormat: "float",
};

test("artifact-only module adds no Original placeholder and saves the explicitly selected export", async () => {
  const writes: string[] = [];
  const confirmed = Promise.withResolvers<void>();
  const editor = createEditorComposableRecipe(
    (input, init) => {
      const url = input instanceof Request ? input.url : input.toString();
      if (url.endsWith("/api/processing/modules"))
        return Promise.resolve(
          Response.json({
            contractVersion: "v1",
            modules: [
              {
                id: { name: "spektrafilm", adapterVersion: "v1" },
                parameterVersions: ["film-v1"],
                parameterSchema: {
                  type: "object",
                  default: { simulation: "fixed" },
                },
                admittedInputs: [
                  {
                    format: "tiff",
                    precisionBits: 32,
                    colorSpace: "prophoto-rgb",
                    transferFunction: "linear",
                  },
                ],
                admittedOutputs: [],
                limits: {},
                availability: { state: "ready", refusalReasons: [] },
              },
            ],
          }),
        );
      if (init?.method === "POST" && url.endsWith("/processing-recipe")) {
        if (typeof init.body !== "string")
          throw new Error("Expected a JSON recipe body");
        writes.push(init.body);
        const body = JSON.parse(init.body) as SaveRequest;
        return Promise.resolve(
          Response.json({
            outcome: "saved",
            recipeVersion: "r1",
            sourceRevision: "s1",
            recipe: {
              photoId: "p1",
              sourceRevision: "s1",
              revision: "r1",
              steps: body.steps,
              currentStepId: body.currentStepId,
            },
          }),
        );
      }
      return Promise.resolve(
        Response.json({ photoId: "p1", sourceRevision: "s1", recipe: null }),
      );
    },
    {
      isAlive: () => true,
      isCurrentPhoto: () => true,
      currentPhoto: () => undefined,
      editorOwnsPhoto: () => true,
      renderEditor: () => {
        if (editor.state?.read.recipe?.revision === "r1") confirmed.resolve();
      },
      previewSelection: () => undefined,
      clearEditorPreview: () => {},
      requestEditorPreview: () => Promise.resolve(),
      markEditorPreviewStale: () => {},
      describeEditRefusal: () => Promise.resolve("refused"),
    },
  );
  await editor.loadComposableRecipe("p1");
  await editor.loadProcessingModules("p1");
  editor.addEditorComposableStep("p1", "spektrafilm");
  expect(writes).toEqual([]);
  expect(editor.state?.draft.steps).toEqual([]);
  editor.retainArtifact(artifact);
  editor.addEditorComposableStep("p1", "spektrafilm", artifact.artifactId);
  const savedRequest = JSON.parse(writes[0]!) as SaveRequest;
  expect(savedRequest.steps[0]?.input).toMatchObject({
    kind: "artifact",
    artifactId: artifact.artifactId,
  });
  expect(savedRequest.steps[0]?.parameters.tree).toEqual({
    simulation: "fixed",
  });
  await confirmed.promise;
  expect(editor.state?.read.recipe?.steps[0]?.input).toMatchObject({
    kind: "artifact",
    artifactId: artifact.artifactId,
  });
  expect(editor.dirty()).toBe(false);
});

test("two autosaved edits keep a noncurrent step open and leave the current step unchanged", async () => {
  const steps = ["A", "B"].map((stepId) => ({
    stepId,
    module: "darktable",
    input: { kind: "original", photoId: "p1", sourceRevision: "s1" },
    parameters: { schemaVersion: "v1", tree: { exposure: 0 } },
  }));
  const writes: SaveRequest[] = [];
  let confirmed = Promise.withResolvers<void>();
  const editor = createEditorComposableRecipe(
    (_input, init) => {
      if (init?.method !== "POST")
        return Promise.resolve(
          Response.json({
            photoId: "p1",
            sourceRevision: "s1",
            recipe: {
              photoId: "p1",
              sourceRevision: "s1",
              revision: "r0",
              currentStepId: "A",
              steps,
            },
          }),
        );
      if (typeof init.body !== "string")
        throw new Error("Expected JSON recipe body");
      const body = JSON.parse(init.body) as SaveRequest;
      writes.push(body);
      const revision = `r${writes.length}`;
      return Promise.resolve(
        Response.json({
          outcome: "saved",
          recipeVersion: revision,
          sourceRevision: "s1",
          recipe: {
            photoId: "p1",
            sourceRevision: "s1",
            revision,
            steps: body.steps,
            currentStepId: body.currentStepId,
          },
        }),
      );
    },
    {
      isAlive: () => true,
      isCurrentPhoto: () => true,
      currentPhoto: () => undefined,
      editorOwnsPhoto: () => true,
      renderEditor: () => {
        if (
          editor.state?.read.recipe?.revision === `r${writes.length}` &&
          writes.length
        )
          confirmed.resolve();
      },
      previewSelection: () => undefined,
      clearEditorPreview: () => {},
      requestEditorPreview: () => Promise.resolve(),
      markEditorPreviewStale: () => {},
      describeEditRefusal: () => Promise.resolve("refused"),
    },
  );
  await editor.loadComposableRecipe("p1");
  editor.editEditorComposableStep("p1", "B");
  for (const exposure of [1, 2]) {
    confirmed = Promise.withResolvers<void>();
    editor.editEditorComposableParameters("p1", JSON.stringify({ exposure }));
    editor.commitEditorComposableParameters("p1");
    expect(editor.view().editing?.stepId).toBe("B");
    await confirmed.promise;
    expect(editor.view().editing?.stepId).toBe("B");
    expect(JSON.parse(editor.view().editing!.parametersText)).toEqual({
      exposure,
    });
  }
  expect(writes.map((write) => write.currentStepId)).toEqual(["A", "A"]);
  expect(
    writes.map((write) => write.steps.map((step) => step.parameters.tree)),
  ).toEqual([
    [{ exposure: 0 }, { exposure: 1 }],
    [{ exposure: 0 }, { exposure: 2 }],
  ]);
  editor.removeEditorComposableStep("p1", "B");
  expect(editor.view().editing?.stepId).toBe("A");
});
