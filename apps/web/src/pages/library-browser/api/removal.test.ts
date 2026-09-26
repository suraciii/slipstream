import { describe, expect, test } from "bun:test";
import { fetchRemovedPhotos } from "./removal.js";

const photo = (overrides: Record<string, unknown> = {}) => ({
  removedAtMs: 100,
  originalLocation: "2024/photo.jpg",
  originalKind: "jpeg" as const,
  originalSize: null,
  pendingVerificationOperationId: null,
  photo: {
    id: "photo-1",
    available: false,
    original: { kind: "jpeg" as const, available: false },
    originalFilename: "photo.jpg",
    selectionState: "rejected" as const,
    rating: 0,
    hasSavedEdits: false,
    preview: { state: "unavailable" as const },
  },
  ...overrides,
});

const listingBody = (
  overrides: Partial<Record<string, unknown>> = {},
  photoOverrides: Record<string, unknown> = {},
) => ({
  start: 0,
  limit: 1,
  total: 1,
  operation: null,
  reviewMaximum: 50,
  photos: [photo(photoOverrides)],
  ...overrides,
});

const okListing = (
  body: Record<string, unknown>,
  input: { start?: number; limit?: number } = {},
) =>
  fetchRemovedPhotos(
    () =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      ),
    {
      start: input.start ?? 0,
      limit: input.limit ?? 1,
      signal: new AbortController().signal,
    },
  );

describe("removed Photos API", () => {
  test("accepts an empty page when a restore shrinks the old offset away", async () => {
    const result = await okListing(
      {
        start: 50,
        limit: 50,
        total: 0,
        operation: null,
        reviewMaximum: 50,
        photos: [],
      },
      { start: 50, limit: 50 },
    );

    expect(result).toEqual({
      kind: "ok",
      start: 50,
      limit: 50,
      total: 0,
      operation: undefined,
      reviewMaximum: 50,
      photos: [],
    });
  });

  test("preserves Trash location, kind, and unknown size metadata", async () => {
    const result = await okListing(listingBody());

    expect(result).toEqual({
      kind: "ok",
      start: 0,
      limit: 1,
      total: 1,
      operation: undefined,
      reviewMaximum: 50,
      photos: [photo()],
    });
  });

  test("carries the review maximum and a pending verification operation id", async () => {
    const result = await okListing(
      listingBody(
        { reviewMaximum: 3 },
        { pendingVerificationOperationId: "operation-9" },
      ),
    );

    expect(result.kind).toBe("ok");
    if (result.kind === "ok") {
      expect(result.reviewMaximum).toBe(3);
      expect(result.photos[0]?.pendingVerificationOperationId).toBe(
        "operation-9",
      );
    }
  });

  test("rejects a listing that omits the review maximum", async () => {
    const body = listingBody();
    delete (body as { reviewMaximum?: unknown }).reviewMaximum;
    const result = await okListing(body);

    expect(result).toEqual({ kind: "failed", malformed: true });
  });

  test("rejects a listing item that omits the pending verification field", async () => {
    const body = listingBody();
    const item = (body.photos as Array<Record<string, unknown>>)[0]!;
    delete item.pendingVerificationOperationId;
    const result = await okListing(body);

    expect(result).toEqual({ kind: "failed", malformed: true });
  });

  test("rejects an empty pending verification operation id: only null or a non-empty id", async () => {
    const result = await okListing(
      listingBody({}, { pendingVerificationOperationId: "" }),
    );

    expect(result).toEqual({ kind: "failed", malformed: true });
  });
});
