import { describe, expect, test } from "bun:test";
import {
  applyRecoveryMappings,
  continueRelocationProposals,
  continueUnavailableReview,
  openUnavailableReview,
  proposeRelocationBatch,
  proposeSingleRelocation,
  RECOVERY_PAGE_LIMIT,
  type RecoveryApplyMapping,
  type RecoveryItem,
  type RecoveryMapping,
  type RecoveryPage,
} from "./recovery.js";

const jsonResponse = (body: unknown, status = 200): Promise<Response> =>
  Promise.resolve(
    new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": "application/json" },
    }),
  );

const requestBody = (init: RequestInit | undefined): unknown =>
  typeof init?.body === "string" ? JSON.parse(init.body) : undefined;

const item = (overrides: Record<string, unknown> = {}): RecoveryItem =>
  ({
    state: "unavailable",
    originalId: "orig-1",
    photoId: "photo-1",
    location: "2024/travel/a.ARW",
    kind: "raw",
    rating: 3,
    selectionState: "selected",
    fingerprintEnrolled: true,
    albumCount: 2,
    webUrl: "https://host/photos/photo-1",
    ...overrides,
  }) as RecoveryItem;

const mapping = (overrides: Record<string, unknown> = {}): RecoveryMapping =>
  ({
    mappingId: "map-1",
    originalId: "orig-1",
    photoId: "photo-1",
    fromLocation: "2023/travel/a.ARW",
    toLocation: "2024/travel/a.ARW",
    kind: "raw",
    outcome: "matched",
    verified: true,
    blockedReason: null,
    retire: null,
    ...overrides,
  }) as RecoveryMapping;

const envelope = <Item>(
  items: ReadonlyArray<Item>,
  overrides: Record<string, unknown> = {},
): RecoveryPage<Item> =>
  ({
    items,
    total: items.length,
    nextCursor: null,
    evaluatedAt: "2026-09-27T10:00:00Z",
    expiresAt: null,
    ...overrides,
  }) as RecoveryPage<Item>;

describe("unavailable review API", () => {
  test("opens one bounded review by POST and reads the first page", async () => {
    let requestedInput = "";
    let requestedInit: RequestInit | undefined;
    const result = await openUnavailableReview(
      (input, init) => {
        requestedInput = input;
        requestedInit = init;
        return jsonResponse(
          envelope([item()], {
            total: 140,
            nextCursor: "cursor/2",
            expiresAt: "2026-09-27T11:00:00Z",
          }),
        );
      },
      { limit: RECOVERY_PAGE_LIMIT },
      new AbortController().signal,
    );

    expect(requestedInput).toBe("/api/recovery/unavailable");
    expect(requestedInit?.method).toBe("POST");
    expect(requestBody(requestedInit)).toEqual({
      limit: RECOVERY_PAGE_LIMIT,
    });
    expect(result).toEqual({
      kind: "ok",
      page: {
        items: [item()],
        total: 140,
        nextCursor: "cursor/2",
        evaluatedAt: "2026-09-27T10:00:00Z",
        expiresAt: "2026-09-27T11:00:00Z",
      },
    });
  });

  test("omits the limit instead of sending an out-of-bound one", async () => {
    let requestedInit: RequestInit | undefined;
    await openUnavailableReview(
      (_input, init) => {
        requestedInit = init;
        return jsonResponse(envelope([]));
      },
      { limit: 500 },
      new AbortController().signal,
    );
    expect(requestBody(requestedInit)).toEqual({});
  });

  test("continues the same review by cursor over GET, encoded", async () => {
    let requestedInput = "";
    let requestedInit: RequestInit | undefined;
    const result = await continueUnavailableReview(
      (input, init) => {
        requestedInput = input;
        requestedInit = init;
        return jsonResponse(envelope([item({ originalId: "orig-2" })]));
      },
      "opaque cursor/2",
      new AbortController().signal,
    );

    expect(requestedInput).toBe(
      `/api/recovery/unavailable/${encodeURIComponent("opaque cursor/2")}`,
    );
    expect(requestedInit?.method).toBe("GET");
    expect(result.kind).toBe("ok");
  });

  test("an expired cursor keeps the server's reason", async () => {
    const result = await continueUnavailableReview(
      () => jsonResponse({ error: "This recovery review expired." }, 409),
      "stale",
      new AbortController().signal,
    );
    expect(result).toEqual({
      kind: "rejected",
      status: 409,
      message: "This recovery review expired.",
    });
  });

  test("a page whose expiry contradicts its cursor is malformed", async () => {
    const result = await openUnavailableReview(
      () =>
        jsonResponse(
          envelope([item()], {
            nextCursor: "cursor/2",
            expiresAt: null,
          }),
        ),
      {},
      new AbortController().signal,
    );
    expect(result).toEqual({ kind: "malformed" });
  });

  test("an item outside the review's states is malformed", async () => {
    const result = await openUnavailableReview(
      () => jsonResponse(envelope([item({ state: "vanished" })])),
      {},
      new AbortController().signal,
    );
    expect(result).toEqual({ kind: "malformed" });
  });

  test("a record that left the Library may remember an empty location", async () => {
    const result = await openUnavailableReview(
      () => jsonResponse(envelope([item({ state: "missing", location: "" })])),
      {},
      new AbortController().signal,
    );
    expect(result.kind).toBe("ok");
  });

  test("a lost transport is rejected with status 0", async () => {
    const result = await openUnavailableReview(
      () => Promise.reject(new Error("offline")),
      {},
      new AbortController().signal,
    );
    expect(result).toEqual({ kind: "rejected", status: 0 });
  });
});

describe("recovery proposal API", () => {
  test("proposes a folder-prefix batch with the page limit", async () => {
    let requestedInput = "";
    let requestedInit: RequestInit | undefined;
    const result = await proposeRelocationBatch(
      (input, init) => {
        requestedInput = input;
        requestedInit = init;
        return jsonResponse(
          envelope([mapping()], {
            total: 52,
            nextCursor: "props/2",
            expiresAt: "2026-09-27T11:00:00Z",
          }),
        );
      },
      "2023/travel",
      "2024/travel",
      { limit: RECOVERY_PAGE_LIMIT },
      new AbortController().signal,
    );

    expect(requestedInput).toBe("/api/recovery/propose");
    expect(requestedInit?.method).toBe("POST");
    expect(requestBody(requestedInit)).toEqual({
      oldPrefix: "2023/travel",
      newPrefix: "2024/travel",
      limit: RECOVERY_PAGE_LIMIT,
    });
    if (result.kind !== "ok") throw new Error("expected a proposal page");
    expect(result.page.total).toBe(52);
    expect(result.page.items[0]?.mappingId).toBe("map-1");
  });

  test("a scope beyond the review bound is rejected with 413 and its reason", async () => {
    const result = await proposeRelocationBatch(
      () =>
        jsonResponse(
          { error: "The prefix covers 500 originals; the bound is 100." },
          413,
        ),
      "2023",
      "2024",
      {},
      new AbortController().signal,
    );
    expect(result).toEqual({
      kind: "rejected",
      status: 413,
      message: "The prefix covers 500 originals; the bound is 100.",
    });
  });

  test("proposes a single mapping as one page without continuation", async () => {
    let requestedInit: RequestInit | undefined;
    const result = await proposeSingleRelocation(
      (_input, init) => {
        requestedInit = init;
        return jsonResponse(envelope([mapping({ verified: false })]));
      },
      "orig-7",
      "2024/travel/renamed.ARW",
      new AbortController().signal,
    );

    expect(requestBody(requestedInit)).toEqual({
      originalId: "orig-7",
      newLocation: "2024/travel/renamed.ARW",
    });
    expect(result).toEqual({
      kind: "ok",
      page: {
        items: [mapping({ verified: false })],
        total: 1,
        nextCursor: null,
        evaluatedAt: "2026-09-27T10:00:00Z",
        expiresAt: null,
      },
    });
  });

  test("continues a prefix review by cursor over GET", async () => {
    let requestedInput = "";
    let requestedInit: RequestInit | undefined;
    await continueRelocationProposals(
      (input, init) => {
        requestedInput = input;
        requestedInit = init;
        return jsonResponse(envelope([mapping({ mappingId: "map-2" })]));
      },
      "props/2",
      new AbortController().signal,
    );
    expect(requestedInput).toBe("/api/recovery/proposals/props%2F2");
    expect(requestedInit?.method).toBe("GET");
  });

  test("a mapping with an unknown blocked reason is malformed", async () => {
    const result = await proposeRelocationBatch(
      () =>
        jsonResponse(envelope([mapping({ blockedReason: "somewhere-else" })])),
      "2023/travel",
      "2024/travel",
      {},
      new AbortController().signal,
    );
    expect(result).toEqual({ kind: "malformed" });
  });
});

describe("recovery apply API", () => {
  const applyItems: ReadonlyArray<RecoveryApplyMapping> = [
    {
      originalId: "orig-1",
      newLocation: "2024/travel/a.ARW",
      mappingId: "map-1",
    },
    {
      originalId: "orig-2",
      newLocation: "2024/travel/b.ARW",
      mappingId: "map-2",
      confirmUnverifiedContent: true,
      retirePhotoId: "photo-9",
    },
  ];

  test("commits the reviewed batch and reports the committed mappings", async () => {
    let requestedInput = "";
    let requestedInit: RequestInit | undefined;
    const committed = [
      {
        originalId: "orig-1",
        photoId: "photo-1",
        fromLocation: "2023/travel/a.ARW",
        toLocation: "2024/travel/a.ARW",
        webUrl: "https://host/photos/photo-1",
        retired: null,
      },
      {
        originalId: "orig-2",
        photoId: "photo-2",
        fromLocation: "2023/travel/b.ARW",
        toLocation: "2024/travel/b.ARW",
        webUrl: "https://host/photos/photo-2",
        retired: {
          photoId: "photo-9",
          originalId: "orig-9",
          location: "2024/travel/b.ARW",
        },
      },
    ];
    const result = await applyRecoveryMappings(
      (input, init) => {
        requestedInput = input;
        requestedInit = init;
        return jsonResponse({
          appliedMappings: 2,
          refusedMappings: 0,
          unavailablePhotos: 3,
          mappings: committed,
        });
      },
      applyItems,
      new AbortController().signal,
    );

    expect(requestedInput).toBe("/api/recovery/apply");
    expect(requestedInit?.method).toBe("POST");
    expect(requestBody(requestedInit)).toEqual({
      mappings: applyItems,
    });
    expect(result).toEqual({
      kind: "applied",
      appliedMappings: 2,
      refusedMappings: 0,
      unavailablePhotos: 3,
      mappings: committed,
    });
  });

  test("a refusal carries the message, per-mapping reasons, and nothing applied", async () => {
    const result = await applyRecoveryMappings(
      () =>
        jsonResponse(
          {
            message: "The reviewed mappings changed. Nothing was applied.",
            rejections: [
              { originalId: "orig-1", reason: "mapping no longer current" },
              { originalId: "orig-2", reason: "destination removed" },
            ],
            appliedMappings: 0,
            refusedMappings: 2,
          },
          409,
        ),
      applyItems,
      new AbortController().signal,
    );
    expect(result).toEqual({
      kind: "refused",
      status: 409,
      message: "The reviewed mappings changed. Nothing was applied.",
      rejections: [
        { originalId: "orig-1", reason: "mapping no longer current" },
        { originalId: "orig-2", reason: "destination removed" },
      ],
      appliedMappings: 0,
      refusedMappings: 2,
    });
  });

  test("a non-conflict response remains unknown after submission", async () => {
    const result = await applyRecoveryMappings(
      () => jsonResponse({ message: "Library is scanning." }, 503),
      applyItems.slice(0, 1),
      new AbortController().signal,
    );
    expect(result).toEqual({
      kind: "unknown",
      mappings: applyItems.slice(0, 1),
    });
  });

  test("an unusable success body reports an unknown apply outcome", async () => {
    const result = await applyRecoveryMappings(
      () =>
        jsonResponse({
          appliedMappings: 1,
          refusedMappings: 0,
          unavailablePhotos: 0,
        }),
      applyItems.slice(0, 1),
      new AbortController().signal,
    );
    expect(result).toEqual({
      kind: "unknown",
      mappings: applyItems.slice(0, 1),
    });
  });

  test("an unusable refusal body reports an unknown apply outcome", async () => {
    const result = await applyRecoveryMappings(
      () =>
        jsonResponse(
          { message: "Refused.", appliedMappings: 0, refusedMappings: 1 },
          409,
        ),
      applyItems.slice(0, 1),
      new AbortController().signal,
    );
    expect(result).toEqual({
      kind: "unknown",
      mappings: applyItems.slice(0, 1),
    });
  });

  test("a transport failure reports an unknown apply outcome", async () => {
    expect(
      await applyRecoveryMappings(
        () => Promise.reject(new Error("network lost")),
        applyItems,
        new AbortController().signal,
      ),
    ).toEqual({ kind: "unknown", mappings: applyItems });
  });
});
