import { expect, test } from "bun:test";
import { createRemovalOwner, type RemovalFetch } from "./removal-owner.js";
import { createRemovedListingOwner } from "./removed-listing-owner.js";
import type { RemovedPanelViewModel } from "./removed-listing-contract.js";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((accept) => {
    resolve = accept;
  });
  return { promise, resolve };
}

function listing(operation: { operationId: string; removed: number } | null) {
  return Response.json({
    start: 0,
    limit: 50,
    total: operation ? 1 : 0,
    operation,
    reviewMaximum: 50,
    photos: operation
      ? [
          {
            removedAtMs: 100,
            originalLocation: "photo.jpg",
            originalKind: "jpeg",
            originalSize: 100,
            pendingVerificationOperationId: null,
            photo: {
              id: "photo-1",
              available: true,
              original: { kind: "jpeg", available: true },
              originalFilename: "photo.jpg",
              selectionState: "rejected",
              rating: 0,
              hasSavedEdits: false,
              preview: { state: "unavailable" },
            },
          },
        ]
      : [],
  });
}

function fixture(answers: Array<Response | Promise<Response>>) {
  const fetcher: RemovalFetch = async () => {
    const answer = answers.shift();
    if (!answer) throw new Error("Unexpected request");
    return answer;
  };
  const removal = createRemovalOwner(fetcher);
  let model!: RemovedPanelViewModel;
  let settled = deferred<RemovedPanelViewModel>();
  const selectionSettled = deferred<RemovedPanelViewModel>();
  const present = (next: RemovedPanelViewModel) => {
    model = next;
    if (next.status !== "loading") settled.resolve(next);
    if (next.allSelected && !next.pending) selectionSettled.resolve(next);
  };
  const owner = createRemovedListingOwner(fetcher, {
    removal,
    present: {
      openPanel: present,
      renderPanel: present,
      closePanel() {},
      openReview() {},
      renderReview() {},
      closeReview() {},
      controlsChanged() {},
    },
    coordinate: {
      refreshLibrary: async () => {},
      reopenSourceAfterRestore: async () => {},
      restorationMessage: () => "Restored.",
    },
  });
  return {
    owner,
    removal,
    fetcher,
    present: {
      openPanel: present,
      renderPanel: present,
      closePanel() {},
      openReview() {},
      renderReview() {},
      closeReview() {},
      controlsChanged() {},
    },
    coordinate: {
      refreshLibrary: async () => {},
      reopenSourceAfterRestore: async () => {},
      restorationMessage: () => "Restored.",
    },
    get model() {
      return model;
    },
    settlement: () => settled.promise,
    selectionSettlement: () => selectionSettled.promise,
    nextRead() {
      settled = deferred<RemovedPanelViewModel>();
    },
  };
}

test("Trash loading and failed reads never expose a previous operation as current Undo", async () => {
  const held = deferred<Response>();
  const f = fixture([
    held.promise,
    listing(null),
    listing({ operationId: "new", removed: 1 }),
  ]);
  f.removal.rememberOperation({ operationId: "old", removed: 23 });
  f.owner.openPanel();
  expect(f.model.status).toBe("loading");
  expect(f.model.undo).toBeUndefined();
  held.resolve(new Response(null, { status: 503 }));
  const failed = await f.settlement();
  expect(failed.status).toBe("failed");
  expect(failed.canRetry).toBe(true);
  expect(failed.undo).toBeUndefined();
  f.nextRead();
  f.owner.retry();
  const empty = await f.settlement();
  expect(empty.status).toBe("ready");
  expect(empty.total).toBe(0);
  expect(empty.undo).toBeUndefined();
  expect(f.removal.operation).toBeUndefined();
  f.nextRead();
  f.owner.closePanel();
  f.owner.openPanel();
  const populated = await f.settlement();
  expect(populated.total).toBe(1);
  expect(populated.undo).toEqual({ removed: 1 });
  expect(f.removal.operation?.operationId).toBe("new");
  f.owner.dispose();
});

test("late Trash read cannot replace the reopened listing or resurrect Undo", async () => {
  const oldRead = deferred<Response>();
  const f = fixture([listing(null), oldRead.promise, listing(null)]);
  f.owner.openPanel();
  await f.settlement();
  const staleRead = f.owner.reloadAfterUndo("Restored.");
  f.owner.closePanel();
  f.nextRead();
  f.owner.openPanel();
  await f.settlement();
  oldRead.resolve(listing({ operationId: "stale", removed: 1 }));
  await staleRead;
  expect(f.model.status).toBe("ready");
  expect(f.model.total).toBe(0);
  expect(f.model.undo).toBeUndefined();
  expect(f.removal.operation).toBeUndefined();
  f.owner.dispose();
});

/// A multi-item Trash listing whose rows the exclusion tests toggle. The
/// pending-verification row is never selectable, exactly as the listing
/// contract says.
function exclusionListing(
  photoIds: ReadonlyArray<string>,
  pendingId: string | null = null,
  start = 0,
  reviewMaximum = 50,
) {
  return Response.json({
    start,
    limit: 50,
    total: photoIds.length,
    operation: null,
    reviewMaximum,
    photos: photoIds.slice(start, start + 50).map((photoId, offset) => ({
      removedAtMs: 1000 - start - offset,
      originalLocation: `${photoId}.jpg`,
      originalKind: "jpeg",
      originalSize: 100,
      pendingVerificationOperationId:
        photoId === pendingId ? "operation-9" : null,
      photo: {
        id: photoId,
        available: true,
        original: { kind: "jpeg", available: true },
        originalFilename: `${photoId}.jpg`,
        selectionState: "rejected",
        rating: 0,
        hasSavedEdits: false,
        preview: { state: "unavailable" },
      },
    })),
  });
}

test("select all submits every current Trash item minus the exclusions, and an excluded row is never counted as reviewed", async () => {
  const reviewBodies: unknown[] = [];
  const f = fixture([
    exclusionListing(["photo-1", "photo-2", "photo-3"]),
    exclusionListing(["photo-1", "photo-2", "photo-3"]),
    Response.json({
      operationId: "operation-review",
      items: [
        {
          photoId: "photo-1",
          removedAtMs: 1000,
          originalId: "original-1",
          originalLocation: "photo-1.jpg",
          originalKind: "jpeg",
          size: 100,
          albums: [],
        },
      ],
      rejected: [],
    }),
  ]);
  // Record the identities sent to review before any response is processed.
  const recording: RemovalFetch = async (path, init) => {
    if (path === "/api/trash/review")
      reviewBodies.push(
        typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
      );
    return f.fetcher(path, init);
  };
  const owner = createRemovedListingOwner(recording, {
    removal: f.removal,
    present: f.present,
    coordinate: f.coordinate,
  });
  owner.openPanel();
  await f.settlement();
  expect(f.model.total).toBe(3);

  // Select all names the complete selection, not only the visible cells.
  owner.selectAll();
  await f.selectionSettlement();
  expect(f.model.selectionCount).toBe(3);
  expect(f.model.allSelected).toBe(true);
  expect(f.model.canDelete).toBe(true);
  expect(f.model.message).toBe(
    "Review scope: all current Trash items except 0 excluded.",
  );

  // Excluding one row before capture removes it from the captured set.
  owner.toggle("photo-2", false);
  expect(f.model.selectionCount).toBe(2);
  const excludedRow = f.model.items.find((item) => item.photoId === "photo-2");
  expect(excludedRow?.excluded).toBe(true);
  expect(excludedRow?.selected).toBe(false);
  expect(f.model.message).toBe(
    "Review scope: all current Trash items except 1 excluded. 1 item excluded from the selection and never deleted.",
  );

  // The server resolves all current Trash minus the named exclusion.
  await owner.deleteSelection();
  const captured = reviewBodies as ReadonlyArray<{
    all?: boolean;
    photoIds?: string[];
    excludePhotoIds?: string[];
  }>;
  expect(captured).toHaveLength(1);
  const submission = captured[0];
  expect(submission?.all).toBe(true);
  expect(submission?.photoIds).toEqual([]);
  expect(submission?.excludePhotoIds).toEqual(["photo-2"]);
  // The listing row still shows the item — it is in Trash — but marked
  // excluded, never selected.
  const rowAfterCapture = f.model.items.find(
    (item) => item.photoId === "photo-2",
  );
  expect(rowAfterCapture?.excluded).toBe(true);
  expect(rowAfterCapture?.selected).toBe(false);
  owner.dispose();
});

test("select all reports the review bound without claiming a truncated capture", async () => {
  const ids = Array.from({ length: 120 }, (_, index) => `photo-${index}`);
  const f = fixture([
    exclusionListing(ids),
    exclusionListing(ids, null, 50),
    exclusionListing(ids, null, 100),
  ]);
  f.owner.openPanel();
  await f.settlement();
  f.owner.selectAll();
  await f.selectionSettlement();
  expect(f.model.allSelected).toBe(true);
  expect(f.model.message).toBe(
    "Review scope: all current Trash items except 0 excluded. Selecting all current Trash exceeds the 50-item review bound. Narrow the selection by excluding items, then review this batch.",
  );
  expect(f.model.selectionCount).toBe(120);
  expect(f.model.canRestore).toBe(true);
  f.owner.dispose();
});

test("select all never selects an item whose deletion outcome is not settled", async () => {
  const f = fixture([
    exclusionListing(["photo-1", "photo-pending"], "photo-pending"),
  ]);
  f.owner.openPanel();
  await f.settlement();

  f.owner.selectAll();
  await f.selectionSettlement();
  const pendingRow = f.model.items.find(
    (item) => item.photoId === "photo-pending",
  );
  expect(pendingRow?.selected).toBe(false);
  expect(pendingRow?.excluded).toBe(false);
  expect(f.model.selectionCount).toBe(1);
  expect(f.model.canRestore).toBe(true);
  expect(f.model.message).toBe(
    "Review scope: all current Trash items except 1 excluded. 1 item pending verification was not selected.",
  );
  f.owner.dispose();
});

test("later Trash arrivals stay unselected until all-mode capture", async () => {
  const f = fixture([
    exclusionListing(["photo-1", "photo-2"]),
    exclusionListing(["photo-1", "photo-2", "photo-3"]),
  ]);
  f.owner.openPanel();
  await f.settlement();
  f.owner.selectAll();
  await f.selectionSettlement();
  expect(f.model.selectionCount).toBe(2);

  // Before review, all mode still resolves current Trash at admission.
  f.nextRead();
  await f.owner.reloadAfterUndo("Restored.");
  expect(f.model.total).toBe(3);
  expect(f.model.allSelected).toBe(true);
  expect(f.model.selectionCount).toBe(2);
  expect(
    f.model.items.find((item) => item.photoId === "photo-3")?.selected,
  ).toBe(false);
  f.owner.toggle("photo-3", true);
  expect(
    f.model.items.find((item) => item.photoId === "photo-3")?.selected,
  ).toBe(true);
  expect(f.model.selectionCount).toBe(3);
  f.owner.toggle("photo-3", false);
  expect(f.model.selectionCount).toBe(2);
  expect(
    f.model.items.find((item) => item.photoId === "photo-3")?.excluded,
  ).toBe(true);
  f.owner.dispose();
});

test("all-mode review captures current Trash minus exclusions and reports the resolved count", async () => {
  const items = ["photo-1", "photo-3"].map((photoId, index) => ({
    photoId,
    removedAtMs: 1000 - index * 2,
    originalId: `original-${index}`,
    originalLocation: `${photoId}.jpg`,
    originalKind: "jpeg",
    size: 100,
    albums: [],
  }));
  const f = fixture([
    exclusionListing(["photo-1", "photo-2"]),
    exclusionListing(["photo-1", "photo-2", "photo-3"]),
    exclusionListing(["photo-1", "photo-2", "photo-3"]),
  ]);
  let request: unknown;
  let reviewed = 0;
  const owner = createRemovedListingOwner(
    async (path, init) => {
      if (path === "/api/trash/review") {
        const body: unknown = JSON.parse(
          typeof init?.body === "string" ? init.body : "",
        );
        if (!body || typeof body !== "object" || !("operationId" in body))
          throw new Error("Missing review operation identity");
        request = body;
        return Response.json({
          operationId: body.operationId,
          items,
          rejected: [],
        });
      }
      return f.fetcher(path, init);
    },
    {
      removal: f.removal,
      present: {
        ...f.present,
        openReview(model) {
          reviewed = model.itemCount;
        },
      },
      coordinate: f.coordinate,
    },
  );
  owner.openPanel();
  await f.settlement();
  owner.selectAll();
  await f.selectionSettlement();
  owner.toggle("photo-2", false);
  await owner.reloadAfterUndo("Restored.");
  expect(f.model.selectionCount).toBe(1);
  expect(
    f.model.items.find((item) => item.photoId === "photo-3")?.selected,
  ).toBe(false);
  await owner.deleteSelection();
  expect(request).toMatchObject({
    all: true,
    photoIds: [],
    excludePhotoIds: ["photo-2"],
  });
  expect(reviewed).toBe(2);
  owner.dispose();
});

test("clearing the selection drops the exclusions with it", async () => {
  const f = fixture([exclusionListing(["photo-1", "photo-2"])]);
  f.owner.openPanel();
  await f.settlement();
  f.owner.selectAll();
  await f.selectionSettlement();
  f.owner.clearSelection();
  expect(f.model.selectionCount).toBe(0);
  expect(f.model.allSelected).toBe(false);
  expect(f.model.items.every((item) => !item.excluded)).toBe(true);
  f.owner.dispose();
});

test("new cross-page pending items are excluded and duplicate review is refused", async () => {
  const ids = Array.from({ length: 51 }, (_, index) => `photo-${index}`);
  const f = fixture([
    exclusionListing(ids),
    exclusionListing(ids, null, 50),
    exclusionListing(ids, "photo-50"),
    exclusionListing(ids, "photo-50", 50),
  ]);
  const held = deferred<Response>();
  const started = deferred<void>();
  const bodies: Array<{
    all: boolean;
    photoIds: string[];
    excludePhotoIds: string[];
  }> = [];
  const fetcher: RemovalFetch = async (path, init) => {
    if (path === "/api/trash/review") {
      bodies.push(
        JSON.parse(typeof init?.body === "string" ? init.body : "") as {
          all: boolean;
          photoIds: string[];
          excludePhotoIds: string[];
        },
      );
      started.resolve();
      return held.promise;
    }
    return f.fetcher(path, init);
  };
  const owner = createRemovedListingOwner(fetcher, {
    removal: f.removal,
    present: f.present,
    coordinate: f.coordinate,
  });
  owner.openPanel();
  await f.settlement();
  owner.selectAll();
  await f.selectionSettlement();
  expect(f.model.selectionCount).toBe(51);
  const first = owner.deleteSelection();
  await owner.deleteSelection();
  await started.promise;
  expect(bodies).toHaveLength(1);
  expect(bodies[0]?.all).toBe(true);
  expect(bodies[0]?.photoIds).toEqual([]);
  expect(bodies[0]?.excludePhotoIds).toContain("photo-50");
  held.resolve(new Response(null, { status: 503 }));
  await first;
  owner.dispose();
});

test("all-mode Restore excludes pending and unchecked rows and retries the same intent", async () => {
  const ids = ["photo-1", "photo-2", "photo-pending"];
  const f = fixture([
    exclusionListing(ids, "photo-pending"),
    exclusionListing(ids, "photo-pending"),
    exclusionListing(["photo-2", "photo-pending"], "photo-pending"),
  ]);
  const requests: Array<{
    operationId: string;
    all: boolean;
    photoIds: string[];
    excludePhotoIds: string[];
  }> = [];
  const fetcher: RemovalFetch = async (path, init) => {
    if (path !== "/api/trash/restore") return f.fetcher(path, init);
    const body = JSON.parse(
      typeof init?.body === "string" ? init.body : "",
    ) as {
      operationId: string;
      all: boolean;
      photoIds: string[];
      excludePhotoIds: string[];
    };
    requests.push(body);
    if (requests.length === 1) throw new Error("response lost");
    return Response.json({
      operationId: body.operationId,
      counts: {
        restored: 1,
        alreadyActive: 0,
        changedElsewhere: 0,
        missing: 0,
      },
      results: [{ photoId: "photo-1", outcome: "restored" }],
    });
  };
  const owner = createRemovedListingOwner(fetcher, {
    removal: f.removal,
    present: f.present,
    coordinate: f.coordinate,
  });
  owner.openPanel();
  await f.settlement();
  owner.selectAll();
  await f.selectionSettlement();
  owner.toggle("photo-2", false);
  await owner.restoreSelection();
  expect(f.model.restoreUnconfirmed).toBe(true);
  expect(f.model.canRestore).toBe(true);
  owner.toggle("photo-2", true);
  await owner.restoreSelection();
  expect(requests).toHaveLength(2);
  expect(requests[1]).toEqual(requests[0]);
  expect(requests[0]?.all).toBe(true);
  expect(requests[0]?.photoIds).toEqual([]);
  expect(requests[0]?.excludePhotoIds).toEqual(["photo-2", "photo-pending"]);
  expect(f.model.restoreUnconfirmed).toBeUndefined();
  expect(f.model.message).toBe(
    "1 Photo restored. Restore resolved 1 item: 0 already active, 0 changed, 0 unavailable.",
  );
  owner.dispose();
});
