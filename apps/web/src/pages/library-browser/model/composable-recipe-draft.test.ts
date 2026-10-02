import { describe, expect, test } from "bun:test";
import type {
  ComposableRecipe,
  ComposableRecipeInput,
} from "../api/composable-recipe.js";
import type { ProcessingModuleDescription } from "../api/processing-modules.js";
import {
  addComposableStep,
  composableDraftDiffers,
  composableModuleChoices,
  composableSaveRequest,
  draftFromRecipe,
  nextComposableStepId,
  removeComposableStep,
  selectComposableStep,
  setComposableStepInput,
  setComposableStepParameters,
  type ComposableModuleChoice,
  type ComposableRecipeDraft,
  type ComposableStepDraft,
} from "./composable-recipe-draft.js";

const originalInput = (sourceRevision = "source-1") =>
  ({
    kind: "original",
    photoId: "photo-1",
    sourceRevision,
  }) as ComposableRecipeInput;

const artifactInput = (artifactId = "a".repeat(64)) =>
  ({
    kind: "artifact",
    artifactId,
    contract: {
      format: "tiff",
      precision: "float32",
      colorSpace: "prophoto-rgb",
      transfer: "linear",
      geometry: { width: 9504, height: 6336 },
      encoding: "deflate",
    },
  }) as ComposableRecipeInput;

const step = (
  stepId: string,
  module: string,
  input: ComposableStepDraft["input"],
  tree: unknown = { stack: [] },
): ComposableStepDraft => ({
  stepId,
  module,
  input,
  parameters: Object.freeze({
    schemaVersion: `${module}-params-1`,
    tree,
  }),
});

const module = (
  name: string,
  parameterVersions: ReadonlyArray<string> = [`${name}-params-1`],
): ComposableModuleChoice =>
  Object.freeze({
    name,
    ready: true,
    refusalNote: "",
    parameterVersions,
    defaultTree: {},
  });

const choices = [module("darktable"), module("spektrafilm")];

const savedRecipe = (
  steps: ReadonlyArray<ComposableStepDraft>,
): ComposableRecipe =>
  Object.freeze({
    photoId: "photo-1",
    revision: "recipe-4",
    sourceRevision: "source-1",
    currentStepId: steps[0]?.stepId ?? null,
    steps,
  });

const emptyDraft = (): ComposableRecipeDraft =>
  draftFromRecipe("photo-1", "source-1", null);

describe("draftFromRecipe", () => {
  test("starts a zero-step draft when no recipe is saved", () => {
    const draft = emptyDraft();
    expect(draft.baseRevision).toBeNull();
    expect(draft.steps).toEqual([]);
    expect(draft.currentStepId).toBeNull();
  });

  test("adopts the saved recipe's steps, selection, and committed revision", () => {
    const steps = [step("darktable-1", "darktable", originalInput())];
    const draft = draftFromRecipe("photo-1", "source-1", savedRecipe(steps));
    expect(draft.baseRevision).toBe("recipe-4");
    expect(draft.currentStepId).toBe("darktable-1");
  });
});

describe("nextComposableStepId", () => {
  test("never reuses a step identity the recipe already holds", () => {
    let draft = emptyDraft();
    draft = addComposableStep(
      draft,
      step(
        nextComposableStepId(draft, "darktable"),
        "darktable",
        originalInput(),
      ),
    );
    expect(draft.steps[0]?.stepId).toBe("darktable-1");
    draft = removeComposableStep(draft, "darktable-1");
    const reused = nextComposableStepId(draft, "darktable");
    expect(reused === "darktable-1").toBe(false);
  });
});

describe("add, remove, and select", () => {
  test("adding a step selects it; removing the current step never dangles the selection", () => {
    let draft = emptyDraft();
    draft = addComposableStep(draft, step("a", "darktable", originalInput()));
    expect(draft.currentStepId).toBe("a");
    draft = addComposableStep(
      draft,
      step("b", "darktable", originalInput(), { stack: [] }),
    );
    expect(draft.currentStepId).toBe("b");
    draft = removeComposableStep(draft, "b");
    expect(draft.currentStepId).toBe("a");
    draft = removeComposableStep(draft, "a");
    expect(draft.steps).toEqual([]);
    expect(draft.currentStepId).toBeNull();
  });

  test("selection names only the recipe's own steps", () => {
    let draft = emptyDraft();
    draft = addComposableStep(draft, step("a", "darktable", originalInput()));
    expect(selectComposableStep(draft, "not-a-step").currentStepId).toBe("a");
    expect(selectComposableStep(draft, "a").currentStepId).toBe("a");
  });

  test("steps may repeat a module with distinct identities and artifact bindings", () => {
    let draft = emptyDraft();
    draft = addComposableStep(draft, step("a", "darktable", originalInput()));
    draft = addComposableStep(
      draft,
      step("b", "darktable", artifactInput("b".repeat(64))),
    );
    expect(draft.steps.map((item) => item.stepId)).toEqual(["a", "b"]);
    expect(draft.steps[1]?.input.kind).toBe("artifact");
  });
});

describe("composableDraftDiffers", () => {
  test("a draft that matches the saved recipe is clean", () => {
    const steps = [step("a", "darktable", originalInput())];
    const recipe = savedRecipe(steps);
    const draft = draftFromRecipe("photo-1", "source-1", recipe);
    expect(composableDraftDiffers(draft, recipe)).toBe(false);
  });

  test("parameter, binding, selection, and step-count changes each differ", () => {
    const steps = [step("a", "darktable", originalInput())];
    const recipe = savedRecipe(steps);
    const draft = draftFromRecipe("photo-1", "source-1", recipe);
    expect(
      composableDraftDiffers(
        setComposableStepParameters(draft, "a", {
          schemaVersion: "darktable-params-1",
          tree: {
            stack: [
              {
                operation: "exposure",
                multiPriority: 0,
                enabled: true,
                params: {},
              },
            ],
          },
        }),
        recipe,
      ),
    ).toBe(true);
    expect(
      composableDraftDiffers(
        setComposableStepInput(draft, "a", artifactInput()),
        recipe,
      ),
    ).toBe(true);
    expect(
      composableDraftDiffers(
        addComposableStep(draft, step("b", "darktable", originalInput())),
        recipe,
      ),
    ).toBe(true);
    expect(
      composableDraftDiffers(removeComposableStep(draft, "a"), recipe),
    ).toBe(true);
  });

  test("an empty draft over no saved recipe is clean, and any added step differs", () => {
    const draft = emptyDraft();
    expect(composableDraftDiffers(draft, null)).toBe(false);
    expect(
      composableDraftDiffers(
        addComposableStep(draft, step("a", "darktable", originalInput())),
        null,
      ),
    ).toBe(true);
  });
});

describe("composableSaveRequest", () => {
  const draftWith = (
    steps: ReadonlyArray<ComposableStepDraft>,
  ): ComposableRecipeDraft => ({
    ...emptyDraft(),
    steps,
    currentStepId: steps[0]?.stepId ?? null,
  });

  test("builds the guarded save body for one Original-bound step", () => {
    const draft = draftWith([step("a", "darktable", originalInput())]);
    const result = composableSaveRequest(draft, "save-1", choices);
    expect(result.kind).toBe("ok");
    if (result.kind !== "ok") return;
    expect(result.request).toEqual({
      requestId: "save-1",
      expectedRecipeRevision: null,
      expectedSourceRevision: "source-1",
      currentStepId: "a",
      steps: draft.steps,
    });
  });

  test("builds the guarded save body for an explicitly bound artifact input", () => {
    const draft = draftWith([step("a", "spektrafilm", artifactInput())]);
    const result = composableSaveRequest(draft, "save-1", choices);
    expect(result.kind).toBe("ok");
  });

  test("refuses an artifact binding without a complete concrete contract", () => {
    const incomplete = {
      kind: "artifact",
      artifactId: "a".repeat(64),
      contract: { format: "tiff" },
    } as const;
    const draft = draftWith([step("a", "spektrafilm", incomplete)]);
    const result = composableSaveRequest(draft, "save-1", choices);
    expect(result.kind).toBe("refused");
  });

  test("refuses a module discovery never settled", () => {
    const draft = draftWith([step("a", "darktable", originalInput())]);
    expect(composableSaveRequest(draft, "save-1", []).kind).toBe("refused");
  });

  test("refuses a module or parameter version discovery does not own", () => {
    const draft = draftWith([step("a", "unknown-module", originalInput())]);
    expect(composableSaveRequest(draft, "save-1", choices).kind).toBe(
      "refused",
    );
    const wrongVersion = draftWith([
      {
        ...step("a", "darktable", originalInput()),
        parameters: { schemaVersion: "darktable-params-9", tree: {} },
      },
    ]);
    expect(composableSaveRequest(wrongVersion, "save-1", choices).kind).toBe(
      "refused",
    );
  });

  test("refuses parameters that are not one JSON object", () => {
    const draft = draftWith([
      step("a", "darktable", originalInput(), [1, 2, 3]),
    ]);
    expect(composableSaveRequest(draft, "save-1", choices).kind).toBe(
      "refused",
    );
  });

  test("refuses an Original binding against another Photo or an earlier revision", () => {
    const stale = draftWith([
      step("a", "darktable", originalInput("source-earlier")),
    ]);
    expect(composableSaveRequest(stale, "save-1", choices).kind).toBe(
      "refused",
    );
    const foreign = draftWith([
      step("a", "darktable", {
        kind: "original",
        photoId: "photo-2",
        sourceRevision: "source-1",
      }),
    ]);
    expect(composableSaveRequest(foreign, "save-1", choices).kind).toBe(
      "refused",
    );
  });

  test("refuses a missing selection while steps exist and a dangling selection", () => {
    const noSelection: ComposableRecipeDraft = {
      ...draftWith([step("a", "darktable", originalInput())]),
      currentStepId: null,
    };
    expect(composableSaveRequest(noSelection, "save-1", choices).kind).toBe(
      "refused",
    );
    const dangling: ComposableRecipeDraft = {
      ...emptyDraft(),
      steps: [step("a", "darktable", originalInput())],
      currentStepId: "not-a-step",
    };
    expect(composableSaveRequest(dangling, "save-1", choices).kind).toBe(
      "refused",
    );
  });

  test("accepts the zero-step recipe with no selection", () => {
    const result = composableSaveRequest(emptyDraft(), "save-1", choices);
    expect(result.kind).toBe("ok");
    if (result.kind !== "ok") return;
    expect(result.request.currentStepId).toBeNull();
    expect(result.request.steps).toEqual([]);
  });

  test("refuses a recipe without an observed source revision", () => {
    const draft: ComposableRecipeDraft = {
      ...emptyDraft(),
      sourceRevision: "",
    };
    expect(composableSaveRequest(draft, "save-1", choices).kind).toBe(
      "refused",
    );
  });
});

describe("composableModuleChoices", () => {
  const description = (
    name: string,
    state: "ready" | "unavailable",
  ): ProcessingModuleDescription =>
    ({
      id: { name, adapterVersion: `${name}-adapter-1` },
      parameterVersions: [`${name}-params-1`],
      parameterSchema: {
        type: "object",
        properties: {
          output: {
            type: "object",
            properties: {
              format: { const: "tiff" },
              encoding: { type: "string" },
            },
          },
          stack: { type: "array" },
        },
      },
      admittedInputs: [],
      admittedOutputs: [],
      limits: {
        maxInputBytes: 1,
        maxParameterBytes: 2,
        maxOutputPixels: 3,
        deadlineMillis: 4,
      },
      availability: {
        state,
        refusalReasons: state === "ready" ? [] : ["runtime-missing"],
      },
    }) as ProcessingModuleDescription;

  test("derives each module's default tree from its own published schema", () => {
    const choicesFromDiscovery = composableModuleChoices([
      description("darktable", "ready"),
      description("spektrafilm", "unavailable"),
    ]);
    expect(choicesFromDiscovery[0]?.defaultTree).toEqual({
      output: { format: "tiff" },
      stack: [],
    });
    expect(choicesFromDiscovery[0]?.ready).toBe(true);
    expect(choicesFromDiscovery[1]?.ready).toBe(false);
    expect(choicesFromDiscovery[1]?.refusalNote).toBe("runtime-missing");
  });
});
