import { expect, test } from "bun:test";
import {
  removedPanelModel,
  trashReviewModel,
  type TrashReviewState,
} from "./removed-listing-view-model.js";

function panelState(): Parameters<typeof removedPanelModel>[0] {
  return {
    removedPage: undefined,
    removedPending: false,
    removedSelectionPending: false,
    removedLoadFailed: false,
    removedAllSelected: false,
    removedAllMarkers: new Map(),
    removedExcludedIds: new Set(),
    removedPendingIds: new Set(),
    removedAllCount: 0,
    removedMessage: undefined,
    pendingRestore: undefined,
    removedDeleting: false,
    removedSelectedIds: new Set(),
    removedSelectedMarkers: new Map(),
    removedRestoringSelection: false,
    removedRestoringId: undefined,
    trashOutcomeBusy: undefined,
    trashOperation: undefined,
    pageLimit: 50,
  };
}

test("panel projection reports all-selection bounds and unconfirmed restore", () => {
  const model = removedPanelModel({
    ...panelState(),
    removedPage: {
      start: 0,
      total: 5,
      reviewMaximum: 2,
      operation: { operationId: "removal", removed: 5 },
      items: [],
    },
    removedAllSelected: true,
    removedAllCount: 3,
    removedExcludedIds: new Set(["excluded", "pending"]),
    removedPendingIds: new Set(["pending"]),
    pendingRestore: { operationId: "restore", excludePhotoIds: [] },
  });
  expect(model.status).toBe("ready");
  expect(model.selectionCount).toBe(3);
  expect(model.undo).toEqual({ removed: 5 });
  expect(model.canDelete).toBe(false);
  expect(model.canRestore).toBe(true);
  expect(model.restoreUnconfirmed).toBe(true);
  expect(model.message).toContain("except 2 excluded");
  expect(model.message).toContain(
    "1 item pending verification was not selected",
  );
  expect(model.message).toContain("2-item review bound");
  expect(model.message).toContain("Restore outcome is unconfirmed");
});

test("review projection copies presented entries and describes confirmation", () => {
  const review: TrashReviewState = {
    operationId: "operation",
    deleting: true,
    plan: {
      itemCount: 1,
      totalBytes: 42,
      albumCount: 1,
      albumNames: ["Album"],
      items: [
        {
          photoId: "photo",
          originalLocation: "photo.jpg",
          originalKind: "jpeg",
          size: 42,
          albumNames: ["Album"],
        },
      ],
      rejected: [{ photoId: "other", reasonLabel: "Original missing" }],
      rejectedCount: 1,
      canConfirm: true,
    },
  };
  const model = trashReviewModel(review);
  expect(model.confirmLabel).toBe("Permanently delete 1 Original File");
  expect(model.deleting).toBe(true);
  expect(model.items).toEqual(review.plan.items);
  expect(model.items[0]).not.toBe(review.plan.items[0]);
  expect(model.rejected[0]).not.toBe(review.plan.rejected[0]);
  expect(model.albumNames).not.toBe(review.plan.albumNames);
  expect(trashReviewModel(undefined).canConfirm).toBe(false);
});
