import { describe, expect, test } from "bun:test";
import { fetchEditRecipe } from "./editor.js";

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
