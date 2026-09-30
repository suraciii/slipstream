import { describe, expect, test } from "bun:test";
import { parseEditFacts, rebindEditRecipe, saveEditRecipe } from "./editor.js";

const controls = {
  exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
  whiteBalanceModes: ["as-shot"],
};

const body = (overrides: Record<string, unknown> = {}) => ({
  sourceRevision: "rev-1",
  recipe: null,
  sourceSupport: "supported",
  supportReason: null,
  processingAvailable: true,
  controls,
  ...overrides,
});

const digest = "a".repeat(64);

const jsonResponse = (body: unknown, status = 200): Promise<Response> =>
  Promise.resolve(
    new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": "application/json" },
    }),
  );

describe("parseEditFacts", () => {
  test("reads a supported Original source with no reason and no proxy", () => {
    const facts = parseEditFacts(body(), "photo-1");
    expect(facts?.sourceSupport).toBe("supported");
    expect(facts?.supportReason).toBe("");
    expect(facts?.editSource).toBe("original");
    expect(facts?.editSourceProxyId).toBeNull();
    expect(facts?.sourceRevision).toBe("rev-1");
  });

  test("accepts the closed retryable reasons only with an unavailable source", () => {
    for (const supportReason of [
      "read-pending",
      "resource-unavailable",
      "original-missing",
      "original-unreadable",
    ] as const) {
      const facts = parseEditFacts(
        body({
          sourceRevision: null,
          sourceSupport: "unavailable",
          supportReason,
        }),
        "photo-1",
      );
      expect(facts?.supportReason).toBe(supportReason);
      expect(facts?.sourceRevision).toBeNull();
    }
  });

  test("refuses an unknown reason", () => {
    expect(
      parseEditFacts(
        body({
          sourceRevision: null,
          sourceSupport: "unavailable",
          supportReason: "reindexing",
        }),
        "photo-1",
      ),
    ).toBeUndefined();
  });

  test("refuses reasons beside supported sources", () => {
    expect(
      parseEditFacts(body({ supportReason: "read-pending" }), "photo-1"),
    ).toBeUndefined();
  });

  test("reads a proxy source identity", () => {
    const facts = parseEditFacts(
      body({ editSource: "development-proxy", editSourceProxyId: digest }),
      "photo-1",
    );
    expect(facts?.editSourceProxyId).toBe(digest);
  });
});

const saveRequest = {
  id: "req-1",
  photoId: "photo-1",
  expectedRecipeVersion: null,
  expectedSourceRevision: "rev-1",
  settings: { exposureEv: 0.2, whiteBalance: { mode: "as-shot" as const } },
};

describe("edit recipe save API", () => {
  test("source refusal preserves its closed reason", async () => {
    for (const supportReason of [
      "read-pending",
      "resource-unavailable",
      "original-missing",
      "original-unreadable",
    ] as const) {
      const result = await saveEditRecipe(
        () =>
          jsonResponse(
            {
              error: {
                code: "resource_unavailable",
                message: "Current source facts cannot be read.",
                effect: "none",
                details: { photoId: "photo-1", supportReason },
              },
            },
            503,
          ),
        saveRequest,
        new AbortController().signal,
      );
      if (result.kind !== "refused") throw new Error("expected a refusal");
      expect(result.refusal.supportReason).toBe(supportReason);
    }
  });

  test("unknown refusal reason is not believed", async () => {
    const result = await saveEditRecipe(
      () =>
        jsonResponse(
          {
            error: {
              code: "resource_unavailable",
              message: "Current source facts cannot be read.",
              effect: "none",
              details: {
                photoId: "photo-1",
                supportReason: "original-rotated",
              },
            },
          },
          503,
        ),
      saveRequest,
      new AbortController().signal,
    );
    if (result.kind !== "refused") throw new Error("expected a refusal");
    expect(result.refusal.supportReason).toBe("");
  });
});

describe("edit recipe write outcomes", () => {
  const invoke = [
    (fetcher: Parameters<typeof saveEditRecipe>[0]) =>
      saveEditRecipe(fetcher, saveRequest, new AbortController().signal),
    (fetcher: Parameters<typeof rebindEditRecipe>[0]) =>
      rebindEditRecipe(fetcher, "photo-1", "recipe-1", "rev-2"),
  ];

  test("accepts unchanged writes and retains their versions", async () => {
    for (const write of invoke) {
      expect(
        await write(() =>
          jsonResponse({
            outcome: "unchanged",
            recipeVersion: "recipe-1",
            sourceRevision: "rev-2",
          }),
        ),
      ).toEqual({
        kind: "saved",
        recipeVersion: "recipe-1",
        sourceRevision: "rev-2",
      });
    }
  });

  test("keeps malformed success uncertain but reads conflict identities", async () => {
    for (const write of invoke) {
      const unknown = await write(() =>
        jsonResponse({
          outcome: "saved",
          recipeVersion: 3,
          sourceRevision: "rev-2",
        }),
      );
      expect(unknown.kind).toBe("refused");
      if (unknown.kind === "refused")
        expect(unknown.refusal.code).toBe("outcome_unknown");

      const conflict = await write(() =>
        jsonResponse(
          {
            error: {
              code: "conflict",
              details: {
                currentRecipeVersion: "recipe-2",
                currentSourceRevision: "rev-3",
              },
            },
          },
          409,
        ),
      );
      expect(conflict.kind).toBe("refused");
      if (conflict.kind === "refused") {
        expect(conflict.refusal.code).toBe("conflict");
        expect(conflict.refusal.currentRecipeVersion).toBe("recipe-2");
        expect(conflict.refusal.currentSourceRevision).toBe("rev-3");
      }
    }
  });
});
