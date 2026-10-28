import { describe, expect, test } from "bun:test";
import {
  cleanupTarget,
  cleanupTrash,
  fetchTrashCleanupOperation,
  fetchTrashPage,
} from "./trash.js";

const photo = {
  id: "photo-1",
  available: false,
  original: { kind: "jpeg", available: false },
  originalFilename: "a.jpg",
  selectionState: "rejected",
  rating: 0,
  hasSavedEdits: false,
  preview: { state: "unavailable" },
};
const listingItem = (overrides: Record<string, unknown> = {}) => ({
  photoId: "photo-1",
  removedAtMs: 100,
  originalId: "original-1",
  originalLocation: 'folder/a "quoted"\n.jpg',
  originalKind: "jpeg",
  originalSize: null,
  originalAvailable: false,
  state: "captured",
  photo,
  ...overrides,
});
const page = (overrides: Record<string, unknown> = {}) => ({
  start: 0,
  limit: 1,
  total: 1,
  cleanupMaximum: 5000,
  restoreMaximum: 100,
  evaluatedAt: "2026-10-28T10:00:00Z",
  expiresAt: null,
  nextCursor: null,
  libraryRoot: "/srv/photos",
  namespace: "service",
  operation: { operationId: "removal-1", removed: 1 },
  photos: [listingItem()],
  ...overrides,
});
const target = {
  photoId: "photo-1",
  removedAtMs: 100,
  originalId: "original-1",
  originalLocation: 'folder/a "quoted"\n.jpg',
};
const resultItem = (overrides: Record<string, unknown> = {}) => ({
  photoId: "photo-1",
  originalLocation: target.originalLocation,
  originalKind: "jpeg" as const,
  state: "cleaned" as const,
  message: null,
  blockers: [],
  ...overrides,
});
const operation = (overrides: Record<string, unknown> = {}) => ({
  operationId: "attempt/1",
  reviewed: 1,
  counts: {
    cleaned: 1,
    changed: 0,
    present: 0,
    blocked: 0,
    unavailable: 0,
    inspectionFailed: 0,
  },
  items: [resultItem()],
  ...overrides,
});
const respond =
  (body: unknown, status = 200) =>
  () =>
    Promise.resolve(Response.json(body, { status }));

describe("Trash listing", () => {
  test("requests a first capture and retains the exact Location and identity", async () => {
    let path = "";
    const signal = new AbortController().signal;
    let init: RequestInit | undefined;
    const result = await fetchTrashPage(
      (url, request) => {
        path = url;
        init = request;
        return Promise.resolve(Response.json(page()));
      },
      { start: 0, limit: 1, signal },
    );
    expect(path).toBe("/api/trash?start=0&limit=1");
    expect(init?.signal).toBe(signal);
    expect(result.kind).toBe("ok");
    if (result.kind === "ok") {
      expect(result.value.libraryRoot).toBe("/srv/photos");
      expect(result.value.namespace).toBe("service");
      expect(cleanupTarget(result.value.photos[0]!)).toEqual(target);
    }
  });

  test("continues the retained capture with an encoded cursor", async () => {
    const first = await fetchTrashPage(
      respond(
        page({
          total: 2,
          nextCursor: "next/+?",
          expiresAt: "2026-10-28T10:30:00Z",
        }),
      ),
      { start: 0, limit: 1 },
    );
    expect(first.kind).toBe("ok");
    let path = "";
    const second = await fetchTrashPage(
      (url) => {
        path = url;
        return Promise.resolve(
          Response.json(
            page({
              start: 1,
              total: 2,
              photos: [
                listingItem({
                  photoId: "photo-2",
                  originalId: "original-2",
                  state: "changed",
                  photo: null,
                }),
              ],
            }),
          ),
        );
      },
      { cursor: "next/+?" },
    );
    expect(path).toBe("/api/trash?cursor=next%2F%2B%3F");
    expect(second.kind).toBe("ok");
  });

  test("accepts unavailable records and an empty page beyond the capture", async () => {
    const unavailable = page({
      photos: [listingItem({ state: "unavailable", photo: null })],
    });
    expect(
      (await fetchTrashPage(respond(unavailable), { start: 0, limit: 1 })).kind,
    ).toBe("ok");
    expect(
      (
        await fetchTrashPage(
          respond(page({ start: 5, total: 0, photos: [] })),
          { start: 5, limit: 1 },
        )
      ).kind,
    ).toBe("ok");
  });

  test("rejects incomplete, inconsistent, or malformed capture pages", async () => {
    const bad = [
      page({ total: 2, nextCursor: null }),
      page({ total: 2, nextCursor: "next", expiresAt: null }),
      page({ total: 2, nextCursor: "next", expiresAt: "later", photos: [] }),
      page({ namespace: "client" }),
      page({ photos: [listingItem({ photoId: "other" })] }),
      page({ photos: [listingItem({ removedAtMs: -1 })] }),
      page({ photos: [listingItem({ state: "unknown" })] }),
      page({ extra: true }),
      page({ start: 1 }),
    ];
    for (const body of bad)
      expect(
        await fetchTrashPage(respond(body), { start: 0, limit: 1 }),
      ).toEqual({ kind: "failed", malformed: true });
    const duplicate = page({
      limit: 2,
      total: 2,
      photos: [listingItem(), listingItem()],
    });
    expect(
      await fetchTrashPage(respond(duplicate), { start: 0, limit: 2 }),
    ).toEqual({ kind: "failed", malformed: true });
  });

  test("distinguishes expired cursor, transport error and malformed JSON", async () => {
    expect(
      await fetchTrashPage(respond({}, 410), { cursor: "expired" }),
    ).toEqual({ kind: "failed", status: 410 });
    expect(
      await fetchTrashPage(() => Promise.reject(new Error("offline")), {
        cursor: "next",
      }),
    ).toEqual({ kind: "failed" });
    expect(
      await fetchTrashPage(() => Promise.resolve(new Response("not-json")), {
        cursor: "next",
      }),
    ).toEqual({ kind: "failed", malformed: true });
  });
});

describe("Trash cleanup", () => {
  test("submits only explicit captured records to the cleanup route", async () => {
    let path = "";
    let request: RequestInit | undefined;
    const result = await cleanupTrash(
      (url, init) => {
        path = url;
        request = init;
        return Promise.resolve(Response.json(operation()));
      },
      "attempt/1",
      [target],
    );
    expect(path).toBe("/api/trash/cleanup");
    expect(request?.method).toBe("POST");
    expect(typeof request?.body).toBe("string");
    if (typeof request?.body !== "string")
      throw new Error("cleanup request body is not JSON text");
    expect(JSON.parse(request.body)).toEqual({
      operationId: "attempt/1",
      items: [target],
    });
    expect(result).toEqual({ kind: "ok", value: operation() });
  });

  test("accepts mixed record outcomes and named retained-work blockers", async () => {
    const second = {
      ...target,
      photoId: "photo-2",
      originalId: "original-2",
      originalLocation: "b.raw",
    };
    const body = operation({
      reviewed: 2,
      counts: {
        cleaned: 1,
        changed: 0,
        present: 0,
        blocked: 1,
        unavailable: 0,
        inspectionFailed: 0,
      },
      items: [
        resultItem(),
        resultItem({
          photoId: "photo-2",
          originalLocation: "b.raw",
          originalKind: "raw",
          state: "blocked",
          message: "Retained work",
          blockers: [
            {
              kind: "workspace",
              name: "work-1",
              action: "Remove retained work",
            },
          ],
        }),
      ],
    });
    expect(
      await cleanupTrash(respond(body), "attempt/1", [target, second]),
    ).toEqual({ kind: "ok", value: body });
  });

  test("accepts each distinct refused state without treating it as cleanup", async () => {
    for (const [state, field] of [
      ["changed", "changed"],
      ["present", "present"],
      ["unavailable", "unavailable"],
      ["inspection-failed", "inspectionFailed"],
    ] as const) {
      const body = operation({
        counts: {
          cleaned: 0,
          changed: 0,
          present: 0,
          blocked: 0,
          unavailable: 0,
          inspectionFailed: 0,
          [field]: 1,
        },
        items: [resultItem({ state, message: "Not cleaned" })],
      });
      expect(
        await fetchTrashCleanupOperation(respond(body), "attempt/1"),
      ).toEqual({ kind: "ok", value: body });
    }
  });

  test("validates every result, exact counts, identity and Location", async () => {
    const bad = [
      operation({ operationId: "other" }),
      operation({ reviewed: 2 }),
      operation({
        counts: {
          cleaned: 0,
          changed: 1,
          present: 0,
          blocked: 0,
          unavailable: 0,
          inspectionFailed: 0,
        },
      }),
      operation({ items: [resultItem({ state: "deleted" })] }),
      operation({ items: [resultItem({ originalLocation: "other.jpg" })] }),
      operation({ items: [resultItem({ photoId: "other" })] }),
      operation({
        items: [
          resultItem({
            blockers: [{ kind: "workspace", name: "", action: "remove" }],
          }),
        ],
      }),
      operation({ extra: true }),
    ];
    for (const body of bad)
      expect(await cleanupTrash(respond(body), "attempt/1", [target])).toEqual({
        kind: "malformed",
      });
    const duplicate = operation({
      reviewed: 2,
      counts: {
        cleaned: 2,
        changed: 0,
        present: 0,
        blocked: 0,
        unavailable: 0,
        inspectionFailed: 0,
      },
      items: [resultItem(), resultItem()],
    });
    expect(
      await fetchTrashCleanupOperation(respond(duplicate), "attempt/1"),
    ).toEqual({ kind: "malformed" });
  });

  test("looks up the same attempt without sending another mutation", async () => {
    let path = "";
    let init: RequestInit | undefined;
    const result = await fetchTrashCleanupOperation((url, request) => {
      path = url;
      init = request;
      return Promise.resolve(Response.json(operation()));
    }, "attempt/1");
    expect(path).toBe("/api/trash/cleanup-operations/attempt%2F1");
    expect(init).toBeUndefined();
    expect(result).toEqual({ kind: "ok", value: operation() });
  });

  test("does not invent an outcome for a lost response or rejected lookup", async () => {
    const offline = () => Promise.reject(new Error("offline"));
    expect(await cleanupTrash(offline, "attempt/1", [target])).toMatchObject({
      kind: "rejected",
      status: 0,
    });
    expect(
      await fetchTrashCleanupOperation(respond({}, 404), "attempt/1"),
    ).toEqual({ kind: "rejected", status: 404 });
    expect(
      await fetchTrashCleanupOperation(
        () => Promise.resolve(new Response("bad")),
        "attempt/1",
      ),
    ).toEqual({ kind: "malformed" });
  });
});
