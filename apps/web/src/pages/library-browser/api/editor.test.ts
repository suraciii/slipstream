import { describe, expect, test } from "bun:test";
import { parseEditFacts, saveEditRecipe } from "./editor.js";

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
