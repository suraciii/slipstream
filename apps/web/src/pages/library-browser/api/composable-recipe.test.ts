import { describe, expect, test } from "bun:test";
import { parseComposableRecipe } from "./composable-recipe.js";
import {
  rebindComposableRecipe,
  saveComposableRecipeBody,
} from "./composable-recipe.js";

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

test("retains bound and observed source revisions and unsupported execution disclosure", () => {
  const value = parseComposableRecipe(
    {
      photoId: "p1",
      sourceRevision: "bound\u0000source",
      currentSourceRevision: "current",
      sourceAvailable: true,
      recipe: {
        photoId: "p1",
        revision: "r1",
        sourceRevision: "bound\u0000source",
        steps: [],
        currentStepId: null,
        executionRefusals: [
          {
            stepId: "unsupported",
            code: "unsupported_parameters",
            message: "Retained without execution",
          },
        ],
      },
    },
    "p1",
  );
  expect(value?.sourceRevision).toBe("bound\u0000source");
  expect(value?.currentSourceRevision).toBe("current");
  expect(value?.recipe?.executionRefusals?.[0]?.code).toBe(
    "unsupported_parameters",
  );
});

test("exact-body reconciliation and explicit rebind use the shared guarded routes", async () => {
  const requests: { url: string; method: string | undefined; body: unknown }[] =
    [];
  const fetcher = (url: RequestInfo | URL, init?: RequestInit) => {
    requests.push({
      url: url instanceof Request ? url.url : url.toString(),
      method: init?.method,
      body: init?.body,
    });
    return Promise.resolve(Response.json({}));
  };
  const body =
    '{"requestId":"r","expectedSourceRevision":"s\\u0000opaque","steps":[]}';
  await saveComposableRecipeBody(fetcher, "p/1", body);
  await rebindComposableRecipe(fetcher, "p/1", {
    requestId: "rebind",
    expectedRecipeRevision: "old",
    newSourceRevision: "new",
  });
  expect(requests[0]).toEqual({
    url: "/api/photos/p%2F1/processing-recipe",
    method: "POST",
    body,
  });
  expect(requests[1]?.url).toBe("/api/photos/p%2F1/processing-recipe/rebind");
  expect(requests[1]?.method).toBe("POST");
  const rebindBody = requests[1]?.body;
  if (typeof rebindBody !== "string")
    throw new Error("Expected a JSON rebind body");
  expect(JSON.parse(rebindBody) as unknown).toEqual({
    requestId: "rebind",
    expectedRecipeRevision: "old",
    newSourceRevision: "new",
  });
});
