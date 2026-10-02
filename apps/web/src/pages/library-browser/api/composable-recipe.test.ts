import { describe, expect, test } from "bun:test";
import { parseComposableRecipe } from "./composable-recipe.js";

describe("composable recipe wire", () => {
  test("preserves repeated modules and explicit artifact bindings", () => {
    const recipe = parseComposableRecipe(
      {
        photoId: "photo-1",
        sourceRevision: "source-1",
        recipe: {
          photoId: "photo-1",
          revision: "recipe-1",
          sourceRevision: "source-1",
          currentStepId: "film-2",
          steps: [
            {
              stepId: "develop-1",
              module: "darktable",
              input: {
                kind: "original",
                photoId: "photo-1",
                sourceRevision: "source-1",
              },
              parameters: {
                schemaVersion: "darktable-params-1",
                tree: { stack: [] },
              },
            },
            {
              stepId: "film-2",
              module: "spektrafilm",
              input: {
                kind: "artifact",
                artifactId: "exp-1",
                contract: { format: "tiff", precision: "uint16" },
              },
              parameters: {
                schemaVersion: "spektrafilm-params-1",
                tree: { scanner: {} },
              },
            },
          ],
        },
      },
      "photo-1",
    );
    expect(recipe?.recipe?.currentStepId).toBe("film-2");
    expect(recipe?.recipe?.steps[1]?.input.kind).toBe("artifact");
    expect(recipe?.recipe?.steps[1]?.module).toBe("spektrafilm");
  });

  test("keeps an empty recipe distinct from a malformed recipe", () => {
    expect(
      parseComposableRecipe(
        { photoId: "photo-1", sourceRevision: "source-1", recipe: null },
        "photo-1",
      )?.recipe,
    ).toBeNull();
    expect(
      parseComposableRecipe({ photoId: "photo-1" }, "photo-1"),
    ).toBeUndefined();
  });
});
