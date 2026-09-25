import { describe, expect, test } from "bun:test";
import { fetchRemovedPhotos } from "./removal.js";

describe("removed Photos API", () => {
  test("accepts an empty page when a restore shrinks the old offset away", async () => {
    const result = await fetchRemovedPhotos(
      () =>
        Promise.resolve(
          new Response(
            JSON.stringify({
              start: 50,
              limit: 50,
              total: 0,
              operation: null,
              photos: [],
            }),
            { status: 200, headers: { "Content-Type": "application/json" } },
          ),
        ),
      { start: 50, limit: 50, signal: new AbortController().signal },
    );

    expect(result).toEqual({
      kind: "ok",
      start: 50,
      limit: 50,
      total: 0,
      operation: undefined,
      photos: [],
    });
  });
});
