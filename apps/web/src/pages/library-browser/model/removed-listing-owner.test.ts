import { expect, test } from "bun:test";
import type { RemovalFetch } from "../api/removal.js";
import { createRemovalOwner } from "./removal-owner.js";
import { createRemovedListingOwner } from "./removed-listing-owner.js";
import type {
  RemovedPanelViewModel,
  TrashReviewViewModel,
} from "./removed-listing-contract.js";

const item = (index: number) => ({
  photoId: `photo-${index}`,
  removedAtMs: 1000 + index,
  originalId: `original-${index}`,
  originalLocation:
    index === 0 ? 'folder/a "quote"\n.jpg' : `folder/${index}.jpg`,
  originalKind: "jpeg",
  originalSize: 100,
  originalAvailable: false,
  state: "captured",
  photo: null,
});
const page = (start = 0, total = 1) => ({
  start,
  limit: 50,
  total,
  cleanupMaximum: 5000,
  restoreMaximum: 100,
  evaluatedAt: "2026-10-28T00:00:00Z",
  expiresAt: start + 50 < total ? "2026-10-28T01:00:00Z" : null,
  nextCursor: start + 50 < total ? "next" : null,
  libraryRoot: "/srv/photos",
  namespace: "service",
  operation: null,
  photos: Array.from({ length: Math.min(50, total - start) }, (_, offset) =>
    item(start + offset),
  ),
});

const previewResponse = (init?: RequestInit) => {
  const raw = typeof init?.body === "string" ? init.body : "{}";
  const body = JSON.parse(raw) as {
    items: Array<{ photoId: string; originalLocation: string }>;
  };
  return Response.json({
    items: body.items.map((entry) => ({
      photoId: entry.photoId,
      originalLocation: entry.originalLocation,
      originalKind: "jpeg",
      state: "captured",
      absence: "absent",
      message: null,
      blockers: [],
    })),
  });
};

function fixture(fetcher: RemovalFetch) {
  const removal = createRemovalOwner(fetcher);
  let model!: RemovedPanelViewModel;
  let review: TrashReviewViewModel | undefined;
  let waitFor: ((model: RemovedPanelViewModel) => void) | undefined;
  let refreshes = 0;
  let reopens = 0;
  const present = (next: RemovedPanelViewModel) => {
    model = next;
    waitFor?.(next);
  };
  const owner = createRemovedListingOwner(fetcher, {
    removal,
    present: {
      openPanel: present,
      renderPanel: present,
      closePanel() {},
      openReview(next) {
        review = next;
      },
      renderReview(next) {
        review = next;
      },
      closeReview() {
        review = undefined;
      },
      controlsChanged() {},
    },
    coordinate: {
      refreshLibrary: () => {
        refreshes++;
        return Promise.resolve();
      },
      reopenSourceAfterRestore: () => {
        reopens++;
        return Promise.resolve();
      },
      restorationMessage: () => "Restored.",
    },
  });
  return {
    owner,
    get model() {
      return model;
    },
    get review() {
      return review;
    },
    get refreshes() {
      return refreshes;
    },
    get reopens() {
      return reopens;
    },
    until(predicate: (model: RemovedPanelViewModel) => boolean) {
      if (model && predicate(model)) return Promise.resolve();
      const { promise, resolve } = Promise.withResolvers<void>();
      waitFor = (next) => {
        if (predicate(next)) {
          waitFor = undefined;
          resolve();
        }
      };
      return promise;
    },
  };
}

function installStorage() {
  const values = new Map<string, string>();
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      localStorage: {
        get length() {
          return values.size;
        },
        key: (index: number) => [...values.keys()][index] ?? null,
        getItem: (key: string) => values.get(key) ?? null,
        setItem: (key: string, value: string) => {
          values.set(key, value);
        },
        removeItem: (key: string) => {
          values.delete(key);
        },
      },
    },
  });
  return {
    values,
    restore() {
      if (previous) Object.defineProperty(globalThis, "window", previous);
      else Reflect.deleteProperty(globalThis, "window");
    },
  };
}

test("select all follows the retained cursor, excludes a row, and reviews captured evidence without mutation", async () => {
  const calls: string[] = [];
  const f = fixture((path, init) => {
    calls.push(path);
    if (path.includes("cursor="))
      return Promise.resolve(Response.json(page(50, 51)));
    if (path.startsWith("/api/trash?"))
      return Promise.resolve(Response.json(page(0, 51)));
    if (path.includes("/albums"))
      return Promise.resolve(
        Response.json({ albums: [{ id: "album", name: "Keepers" }] }),
      );
    if (path === "/api/trash/cleanup-preview")
      return Promise.resolve(previewResponse(init));
    throw new Error(`Unexpected request: ${path}`);
  });
  f.owner.openPanel();
  await f.until((model) => model.status === "ready");
  f.owner.selectAll();
  await f.until((model) => model.allSelected && !model.pending);
  expect(f.model.selectionCount).toBe(51);
  f.owner.turnPage(1);
  f.owner.toggle("photo-50", false);
  expect(f.model.selectionCount).toBe(50);
  await f.owner.cleanupSelection();
  expect(f.review?.itemCount).toBe(50);
  expect(f.review?.items.some((entry) => entry.photoId === "photo-50")).toBe(
    false,
  );
  expect(f.review?.items[0]?.albumNames).toEqual(["Keepers"]);
  expect(calls.filter((path) => path.includes("cursor="))).toEqual([
    "/api/trash?cursor=next",
  ]);
  expect(calls.some((path) => path === "/api/trash/cleanup")).toBe(false);
  f.owner.cancelReview();
  expect(f.review).toBeUndefined();
  f.owner.dispose();
});

test("a definitive cleanup refusal releases the attempt identity for a new review", async () => {
  const storage = installStorage();
  try {
    let posts = 0;
    const fetcher: RemovalFetch = (path, init) => {
      if (path.startsWith("/api/trash?"))
        return Promise.resolve(Response.json(page()));
      if (path.includes("/albums"))
        return Promise.resolve(Response.json({ albums: [] }));
      if (path === "/api/trash/cleanup-preview")
        return Promise.resolve(previewResponse(init));
      if (path === "/api/trash/cleanup") {
        posts++;
        return Promise.resolve(new Response(null, { status: 413 }));
      }
      throw new Error(`Unexpected request: ${path}`);
    };
    const f = fixture(fetcher);
    f.owner.openPanel();
    await f.until((model) => model.status === "ready");
    f.owner.toggle("photo-0", true);
    await f.owner.cleanupSelection();
    await f.owner.confirmReview();
    expect(posts).toBe(1);
    expect(
      [...storage.values.keys()].some((key) =>
        key.startsWith("slipstream:trash-cleanup-attempt:"),
      ),
    ).toBe(false);
    expect(f.model.outcome).toBeUndefined();
    expect(f.model.canCleanup).toBe(true);
    await f.owner.cleanupSelection();
    await f.owner.confirmReview();
    expect(posts).toBe(2);
  } finally {
    storage.restore();
  }
});

test("cleanup persists its identity before sending and checks a lost result without resubmission", async () => {
  const storage = installStorage();
  try {
    let request:
      | {
          operationId: string;
          items: Array<{
            photoId: string;
            removedAtMs: number;
            originalId: string;
            originalLocation: string;
          }>;
        }
      | undefined;
    let posts = 0;
    const fetcher: RemovalFetch = (path, init) => {
      if (path.startsWith("/api/trash?"))
        return Promise.resolve(Response.json(page()));
      if (path.includes("/albums"))
        return Promise.resolve(Response.json({ albums: [] }));
      if (path === "/api/trash/cleanup-preview")
        return Promise.resolve(previewResponse(init));
      if (path === "/api/trash/cleanup") {
        posts++;
        expect(typeof init?.body).toBe("string");
        if (typeof init?.body !== "string")
          throw new Error("cleanup request body is not JSON text");
        request = JSON.parse(init.body) as typeof request;
        expect(
          storage.values.get(
            `slipstream:trash-cleanup-attempt:${request?.operationId}`,
          ),
        ).toContain(request?.operationId);
        throw new Error("lost response");
      }
      if (path.startsWith("/api/trash/cleanup-operations/"))
        return Promise.resolve(
          Response.json({
            operationId: request!.operationId,
            reviewed: 1,
            counts: {
              cleaned: 0,
              changed: 0,
              present: 0,
              blocked: 1,
              unavailable: 0,
              inspectionFailed: 0,
            },
            items: [
              {
                photoId: "photo-0",
                originalLocation: item(0).originalLocation,
                originalKind: "jpeg",
                state: "blocked",
                message: null,
                blockers: [
                  { kind: "workspace", name: "work", action: "Remove work" },
                ],
              },
            ],
          }),
        );
      throw new Error(`Unexpected request: ${path}`);
    };
    const f = fixture(fetcher);
    f.owner.openPanel();
    await f.until((model) => model.status === "ready");
    f.owner.toggle("photo-0", true);
    await f.owner.cleanupSelection();
    await f.owner.confirmReview();
    expect(request?.items).toEqual([
      {
        photoId: "photo-0",
        removedAtMs: 1000,
        originalId: "original-0",
        originalLocation: item(0).originalLocation,
      },
    ]);
    expect(f.model.outcome).toEqual({ unconfirmed: true });
    f.owner.closePanel();
    f.owner.openPanel();
    await f.until((model) => model.status === "ready");
    expect(f.model.outcome).toEqual({ unconfirmed: true });
    await f.owner.checkOutcome();
    expect(posts).toBe(1);
    expect(f.model.outcome).toMatchObject({
      counts: { blocked: 1 },
      items: [{ blockers: [{ name: "work" }] }],
    });
    expect(f.refreshes).toBe(1);
    expect(f.reopens).toBe(1);
    expect(
      [...storage.values.keys()].some((key) =>
        key.startsWith("slipstream:trash-cleanup-attempt:"),
      ),
    ).toBe(false);
    f.owner.dispose();
  } finally {
    storage.restore();
  }
});

test("pending intent is recovered under the same operation identity after reload", async () => {
  const storage = installStorage();
  try {
    const operationId = "00000000-0000-4000-8000-000000000777";
    const target = {
      photoId: item(0).photoId,
      removedAtMs: item(0).removedAtMs,
      originalId: item(0).originalId,
      originalLocation: item(0).originalLocation,
    };
    storage.values.set(
      `slipstream:trash-cleanup-attempt:${operationId}`,
      JSON.stringify({ operationId, items: [target] }),
    );
    let posts = 0;
    const fetcher: RemovalFetch = (path, init) => {
      if (path.startsWith("/api/trash?"))
        return Promise.resolve(Response.json(page()));
      if (path.includes("/albums"))
        return Promise.resolve(Response.json({ albums: [] }));
      if (path.startsWith("/api/trash/cleanup-operations/"))
        return Promise.resolve(new Response(null, { status: 404 }));
      if (path === "/api/trash/cleanup") {
        posts++;
        const raw = typeof init?.body === "string" ? init.body : "{}";
        expect(JSON.parse(raw)).toEqual({
          operationId,
          items: [target],
        });
        return Promise.resolve(
          Response.json({
            operationId,
            reviewed: 1,
            counts: {
              cleaned: 1,
              changed: 0,
              present: 0,
              blocked: 0,
              unavailable: 0,
              inspectionFailed: 0,
            },
            items: [
              {
                photoId: target.photoId,
                originalLocation: target.originalLocation,
                originalKind: "jpeg",
                state: "cleaned",
                message: null,
                blockers: [],
              },
            ],
          }),
        );
      }
      throw new Error(`Unexpected request: ${path}`);
    };
    const f = fixture(fetcher);
    f.owner.openPanel();
    await f.until((model) => {
      const outcome = model.outcome;
      return (
        outcome !== undefined &&
        !("unconfirmed" in outcome) &&
        outcome.counts.cleaned === 1 &&
        !model.cleaning
      );
    });
    expect(posts).toBe(1);
    expect(
      [...storage.values.keys()].some((key) =>
        key.startsWith("slipstream:trash-cleanup-attempt:"),
      ),
    ).toBe(false);
    expect(f.model.outcome).toMatchObject({ counts: { cleaned: 1 } });
    f.owner.dispose();
  } finally {
    storage.restore();
  }
});

test("a failed continuation does not claim a complete selection", async () => {
  const f = fixture((path) =>
    Promise.resolve(
      path.includes("cursor=")
        ? new Response(null, { status: 410 })
        : Response.json(page(0, 51)),
    ),
  );
  f.owner.openPanel();
  await f.until((model) => model.status === "ready");
  f.owner.selectAll();
  await f.until((model) => !model.pending && Boolean(model.message));
  expect(f.model.selectionCount).toBe(0);
  expect(f.model.allSelected).toBe(false);
  f.owner.dispose();
});
