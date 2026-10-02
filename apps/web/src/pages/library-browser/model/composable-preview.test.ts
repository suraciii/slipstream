import { describe, expect, test } from "bun:test";
import type { ComposableRecipeRead } from "./composable-preview.js";
import {
  composablePreviewDigestRefusal,
  composablePreviewIdentityRefusal,
  composablePreviewPngRefusal,
  composablePreviewReadRefusal,
  composablePreviewRefusalNote,
  composablePreviewTarget,
  selectedComposableStep,
} from "./composable-preview.js";

const step = (stepId: string, module = "darktable") => ({
  stepId,
  module,
  input: {
    kind: "original" as const,
    photoId: "photo-1",
    sourceRevision: "source-1",
  },
  parameters: { schemaVersion: `${module}-params-1`, tree: { stack: [] } },
});

const read = (
  recipe: ComposableRecipeRead["recipe"],
): ComposableRecipeRead => ({ sourceRevision: "source-1", recipe });

describe("selectedComposableStep", () => {
  test("selects the recipe's own named current step", () => {
    const selected = selectedComposableStep(
      read({
        photoId: "photo-1",
        revision: "recipe-1",
        sourceRevision: "source-1",
        currentStepId: "film-2",
        steps: [step("develop-1"), step("film-2", "spektrafilm")],
      }),
    );
    expect(selected?.stepId).toBe("film-2");
    expect(selected?.module).toBe("spektrafilm");
  });

  test("selects nothing while the read is unsettled or no recipe is saved", () => {
    expect(selectedComposableStep(undefined)).toBeNull();
    expect(selectedComposableStep(read(null))).toBeNull();
  });

  test("selects nothing for an unselected or dangling current step", () => {
    expect(
      selectedComposableStep(
        read({
          photoId: "photo-1",
          revision: "recipe-1",
          sourceRevision: "source-1",
          currentStepId: null,
          steps: [step("develop-1")],
        }),
      ),
    ).toBeNull();
    // A selection that names no step of the recipe itself has no selected-step
    // Preview to request; the route refuses a defaulted predecessor.
    expect(
      selectedComposableStep(
        read({
          photoId: "photo-1",
          revision: "recipe-1",
          sourceRevision: "source-1",
          currentStepId: "gone-3",
          steps: [step("develop-1")],
        }),
      ),
    ).toBeNull();
  });

  test("keeps the selected step's module parameters as the recipe wrote them", () => {
    const selected = selectedComposableStep(
      read({
        photoId: "photo-1",
        revision: "recipe-1",
        sourceRevision: "source-1",
        currentStepId: "develop-1",
        steps: [step("develop-1")],
      }),
    );
    expect(selected?.parameters.schemaVersion).toBe("darktable-params-1");
    expect(selected?.parameters.tree).toEqual({ stack: [] });
  });
});

describe("composablePreviewTarget", () => {
  const recipeWithCurrent = (): NonNullable<
    ComposableRecipeRead["recipe"]
  > => ({
    photoId: "photo-1",
    revision: "recipe-1",
    sourceRevision: "source-1",
    currentStepId: "develop-1",
    steps: [step("develop-1")],
  });

  test("targets the selected current step of a saved recipe", () => {
    const target = composablePreviewTarget(read(recipeWithCurrent()));
    expect(target.kind).toBe("step");
    expect(target.kind === "step" && target.step.stepId).toBe("develop-1");
  });

  test("an unsettled or failed read never falls back to the legacy path", () => {
    expect(composablePreviewTarget(undefined).kind).toBe("unreadable");
  });

  test("a Photo with no saved composable recipe keeps the legacy path", () => {
    expect(composablePreviewTarget(read(null)).kind).toBe("legacy");
  });

  test("a saved zero-step or unselected recipe renders no processing result", () => {
    const zero = read({
      photoId: "photo-1",
      revision: "recipe-1",
      sourceRevision: "source-1",
      currentStepId: null,
      steps: [],
    });
    expect(composablePreviewTarget(zero).kind).toBe("none");
    const dangling = read({
      photoId: "photo-1",
      revision: "recipe-1",
      sourceRevision: "source-1",
      currentStepId: "film-1",
      steps: [step("develop-1")],
    });
    expect(composablePreviewTarget(dangling).kind).toBe("none");
  });
});

describe("composablePreviewRefusalNote", () => {
  test("says every refusal the selected-step route owns in the workspace's words", () => {
    const notes = [
      "missing_recipe",
      "source_changed",
      "step_not_current",
      "unknown_step",
      "unknown_module",
      "incompatible_input",
      "module_parameters_unavailable",
    ].map((code) => composablePreviewRefusalNote(code));
    for (const note of notes) expect(note).toBeDefined();
    expect(
      notes.filter((note, index) => notes.indexOf(note) === index).length,
    ).toBe(notes.length);
    // A refusal that leaves nothing to reload must say so instead of offering
    // one; the wording is the workspace's, not the service's code.
    expect(composablePreviewRefusalNote("missing_recipe")).toBe(
      "No Processing Recipe is saved for this Photo anymore, so no Preview is shown. Reload to check again.",
    );
    expect(composablePreviewRefusalNote("step_not_current")).toBe(
      "The selected Processing Step is no longer the recipe's current step, so no Preview is shown. Reload to check again.",
    );
    expect(composablePreviewRefusalNote("module_parameters_unavailable")).toBe(
      "The selected Processing Step's parameters have no qualified rendering adapter in this deployment yet, so no Preview is shown.",
    );
  });

  test("owns no code the selected-step route does not answer", () => {
    // A code this client does not know stays the caller's disclosure of the
    // service's own answer; it is never reworded into a composable refusal.
    expect(
      composablePreviewRefusalNote("processing_unavailable"),
    ).toBeUndefined();
    expect(
      composablePreviewRefusalNote("resource_unavailable"),
    ).toBeUndefined();
    expect(composablePreviewRefusalNote("made-up-code")).toBeUndefined();
    expect(composablePreviewRefusalNote("")).toBeUndefined();
  });
});

describe("composablePreviewReadRefusal", () => {
  test("accepts only an image body with bytes", () => {
    expect(composablePreviewReadRefusal("image/jpeg", 1024)).toBe("");
    expect(composablePreviewReadRefusal("application/json", 1024)).not.toBe("");
    expect(composablePreviewReadRefusal(null, 1024)).not.toBe("");
    expect(composablePreviewReadRefusal("image/jpeg", 0)).not.toBe("");
  });
});

describe("composablePreviewIdentityRefusal", () => {
  const expected = {
    photoId: "photo-1",
    stepId: "develop-1",
    recipeRevision: "recipe-1",
    sourceRevision: "source-1",
  };
  const identityHeaders = () =>
    new Headers({
      "slipstream-processing-preview-photo-id": "photo-1",
      "slipstream-processing-preview-step-id": "develop-1",
      "slipstream-processing-preview-source-revision": "736f757263652d31",
      "slipstream-processing-preview-recipe-revision": "recipe-1",
      "slipstream-processing-preview-sha256": "a".repeat(64),
      "slipstream-processing-preview-width": "8",
      "slipstream-processing-preview-height": "4",
      "slipstream-processing-preview-geometry": "1224",
      "slipstream-processing-preview-bundle-id": "b".repeat(64),
      "slipstream-processing-preview-module": "darktable",
      "slipstream-processing-preview-adapter-schema-version":
        "darktable-adapter-1:darktable-params-1",
      "slipstream-processing-preview-parameter-digest": "c".repeat(64),
      "slipstream-processing-preview-output-contract": "d".repeat(64),
      "slipstream-processing-preview-display-conversion":
        "display-transform-v1",
      "slipstream-processing-preview-identity": "e".repeat(64),
    });

  test("accepts complete identity facts for the captured selection", () => {
    expect(composablePreviewIdentityRefusal(identityHeaders(), expected)).toBe(
      "",
    );
  });

  test("rejects a foreign or partial identity", () => {
    const foreign = identityHeaders();
    foreign.set("slipstream-processing-preview-step-id", "film-1");
    expect(composablePreviewIdentityRefusal(foreign, expected)).not.toBe("");
    const partial = new Headers({
      "slipstream-processing-preview-photo-id": "photo-1",
    });
    expect(composablePreviewIdentityRefusal(partial, expected)).not.toBe("");
  });

  test("refuses a response that publishes no identity framing", () => {
    // Missing framing is not compatibility: a rendition this client cannot
    // identify as the selected step's own result is never presented.
    expect(composablePreviewIdentityRefusal(new Headers(), expected)).not.toBe(
      "",
    );
  });

  test("refuses a response that omits the geometry or bundle framing", () => {
    const missingGeometry = identityHeaders();
    missingGeometry.delete("slipstream-processing-preview-geometry");
    expect(
      composablePreviewIdentityRefusal(missingGeometry, expected),
    ).not.toBe("");
    const missingBundle = identityHeaders();
    missingBundle.delete("slipstream-processing-preview-bundle-id");
    expect(composablePreviewIdentityRefusal(missingBundle, expected)).not.toBe(
      "",
    );
  });
});

describe("composablePreviewPngRefusal", () => {
  const pngFrame = (width: number, height: number): Uint8Array => {
    const bytes = new Uint8Array(32);
    bytes.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    const chunkLength = new DataView(bytes.buffer);
    chunkLength.setUint32(8, 13);
    bytes.set([0x49, 0x48, 0x44, 0x52], 12);
    chunkLength.setUint32(16, width);
    chunkLength.setUint32(20, height);
    return bytes;
  };

  test("accepts a PNG whose own IHDR frame matches the declared geometry", () => {
    expect(composablePreviewPngRefusal(pngFrame(8, 4), "8", "4")).toBe("");
  });

  test("refuses non-PNG bytes and a frame that contradicts the headers", () => {
    expect(composablePreviewPngRefusal(new Uint8Array(32), "8", "4")).not.toBe(
      "",
    );
    expect(composablePreviewPngRefusal(pngFrame(9, 4), "8", "4")).not.toBe("");
  });
});

describe("composablePreviewDigestRefusal", () => {
  test("accepts bytes that hash to the published digest and refuses any other", async () => {
    const bytes = new TextEncoder().encode("rendition bytes");
    const digest = await crypto.subtle.digest("SHA-256", bytes);
    const claimed = Array.from(new Uint8Array(digest), (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
    expect(await composablePreviewDigestRefusal(claimed, bytes.buffer)).toBe(
      "",
    );
    expect(
      (await composablePreviewDigestRefusal("a".repeat(64), bytes.buffer))
        .length,
    ).toBeGreaterThan(0);
    expect(
      (await composablePreviewDigestRefusal(null, bytes.buffer)).length,
    ).toBeGreaterThan(0);
  });
});
