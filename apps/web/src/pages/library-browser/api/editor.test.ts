import { describe, expect, test } from "bun:test";
import { fetchEditRecipe, saveEditRecipe } from "./editor.js";

const jsonResponse = (body: unknown, status = 200): Promise<Response> =>
  Promise.resolve(
    new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": "application/json" },
    }),
  );

const readDocument = (
  overrides: Record<string, unknown> = {},
): Record<string, unknown> => ({
  photoId: "photo-1",
  sourceRevision: null,
  recipe: null,
  sourceSupport: "unavailable",
  supportReason: "read-pending",
  processingAvailable: false,
  controls: {
    exposure: { minimumEv: -4, maximumEv: 4, stepEv: 0.001 },
    whiteBalanceModes: ["as-shot"],
  },
  ...overrides,
});

describe("edit recipe read API", () => {
  test("accepts the retryable unavailable reasons with no source revision", async () => {
    for (const supportReason of [
      "read-pending",
      "resource-unavailable",
    ] as const) {
      const result = await fetchEditRecipe(
        () => jsonResponse(readDocument({ supportReason })),
        "photo-1",
        new AbortController().signal,
      );
      if (result.kind !== "ok") throw new Error("expected a parsed read");
      expect(result.facts.sourceSupport).toBe("unavailable");
      expect(result.facts.supportReason).toBe(supportReason);
      expect(result.facts.sourceRevision).toBeNull();
      expect(result.facts.processingAvailable).toBe(false);
    }
  });

  test("keeps the confirmed outcomes distinct from the retryable waits", async () => {
    for (const supportReason of [
      "original-missing",
      "original-unreadable",
    ] as const) {
      const result = await fetchEditRecipe(
        () => jsonResponse(readDocument({ supportReason })),
        "photo-1",
        new AbortController().signal,
      );
      if (result.kind !== "ok") throw new Error("expected a parsed read");
      expect(result.facts.supportReason).toBe(supportReason);
    }
  });

  test("refuses a reason outside the closed set or coupled wrongly", async () => {
    for (const overrides of [
      { supportReason: "original-rotated" },
      { supportReason: null },
      { supportReason: "read-pending", sourceSupport: "unsupported" },
      {
        supportReason: "read-pending",
        sourceSupport: "supported",
        sourceRevision: "rev-1",
      },
    ]) {
      const result = await fetchEditRecipe(
        () => jsonResponse(readDocument(overrides)),
        "photo-1",
        new AbortController().signal,
      );
      expect(result.kind).toBe("failed");
    }
  });
});

describe("edit recipe save API", () => {
  const saveRequest = {
    id: "req-1",
    photoId: "photo-1",
    expectedRecipeVersion: null,
    expectedSourceRevision: "rev-1",
    settings: { exposureEv: 0.2, whiteBalance: { mode: "as-shot" as const } },
  };

  test("a source-state refusal carries the same closed reason the read reports", async () => {
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
                message:
                  "Current source facts cannot be read, so no guarded write is possible.",
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
      expect(result.refusal.code).toBe("resource_unavailable");
      expect(result.refusal.supportReason).toBe(supportReason);
    }
  });

  test("a reported reason outside the closed set is not believed", async () => {
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
