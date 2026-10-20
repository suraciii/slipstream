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
  const present = (next: RemovedPanelViewModel) => {
    model = next;
    if (next.status !== "loading") settled.resolve(next);
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
    get model() {
      return model;
    },
    settlement: () => settled.promise,
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
