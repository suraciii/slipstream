import { expect, test } from "bun:test";
import type { TrashListingItem, TrashListingPage } from "../api/trash.js";
import {
  removedPanelModel,
  trashReviewModel,
} from "./removed-listing-view-model.js";

const item: TrashListingItem = {
  photoId: "photo",
  removedAtMs: 42,
  originalId: "original",
  originalLocation: "folder/a.jpg",
  originalKind: "jpeg",
  originalSize: null,
  originalAvailable: false,
  state: "captured",
  photo: null,
};
const page: TrashListingPage = {
  start: 0,
  limit: 50,
  total: 1,
  cleanupMaximum: 1,
  restoreMaximum: 100,
  evaluatedAt: "2026-10-28T00:00:00Z",
  expiresAt: null,
  nextCursor: null,
  libraryRoot: "/photos",
  namespace: "service",
  operation: null,
  photos: [item],
};

const preview = (
  photoId = "photo",
  absence: "absent" | "present" | "uninspectable" = "absent",
) => ({
  photoId,
  originalLocation: "folder/a.jpg",
  originalKind: "jpeg" as const,
  state: "captured" as const,
  absence,
  message: null,
  blockers: [],
});

function state(): Parameters<typeof removedPanelModel>[0] {
  return {
    page,
    pageIndex: 0,
    pending: false,
    failed: false,
    cleaning: false,
    selected: new Map([[item.photoId, item]]),
    allSelected: false,
    excluded: new Set(),
    restoringSelection: false,
    restoringId: undefined,
    message: undefined,
    operation: undefined,
    outcomeBusy: false,
    pageLimit: 50,
  };
}

test("panel presents captured evidence and unknown cleanup outcome", () => {
  const model = removedPanelModel({
    ...state(),
    operation: { operationId: "attempt" },
  });
  expect(model.libraryRoot).toBe("/photos");
  expect(model.namespace).toBe("service");
  expect(model.items[0]).toMatchObject({
    photoId: "photo",
    originalAvailable: false,
    state: "captured",
    selected: true,
  });
  expect(model.outcome).toEqual({ unconfirmed: true });
  expect(model.canCleanup).toBe(false);
  expect(model.canRestore).toBe(true);
});

test("cleanup review describes captured items and unknown Albums without claiming absence", () => {
  const model = trashReviewModel({
    operationId: "attempt",
    items: [item],
    albums: new Map([["photo", null]]),
    preview: new Map([["photo", preview()]]),
    cleaning: false,
  });
  expect(model.confirmLabel).toBe("Clean up 1 Photo records");
  expect(model.items[0]).toMatchObject({
    originalLocation: "folder/a.jpg",
    originalAvailable: false,
    albumNames: null,
  });
  expect(model.canConfirm).toBe(false);
  const complete = trashReviewModel({
    operationId: "attempt",
    items: [item],
    albums: new Map([["photo", []]]),
    preview: new Map([["photo", preview()]]),
    cleaning: false,
  });
  expect(complete.canConfirm).toBe(true);
  const present = trashReviewModel({
    operationId: "attempt",
    items: [item],
    albums: new Map([["photo", []]]),
    preview: new Map([["photo", preview("photo", "present")]]),
    cleaning: false,
  });
  expect(present.items[0]?.absence).toBe("present");
  expect(present.canConfirm).toBe(true);
  expect(trashReviewModel(undefined).canConfirm).toBe(false);
});

test("restore bound is separate from the cleanup bound", () => {
  const items = Array.from({ length: 101 }, (_, index) => ({
    ...item,
    photoId: `photo-${index}`,
    originalLocation: `folder/${index}.jpg`,
  }));
  const model = removedPanelModel({
    ...state(),
    page: {
      ...page,
      total: items.length,
      cleanupMaximum: 5000,
      restoreMaximum: 100,
      photos: items,
    },
    selected: new Map(items.map((entry) => [entry.photoId, entry])),
  });
  expect(model.selectionCount).toBe(101);
  expect(model.canCleanup).toBe(true);
  expect(model.canRestore).toBe(false);
  expect(model.message).toContain("100");
});

test("oversized captured selection refuses cleanup and restore without truncation", () => {
  const second = { ...item, photoId: "other" };
  const model = removedPanelModel({
    ...state(),
    page: { ...page, restoreMaximum: 1 },
    selected: new Map([
      [item.photoId, item],
      [second.photoId, second],
    ]),
  });
  expect(model.selectionCount).toBe(2);
  expect(model.canCleanup).toBe(false);
  expect(model.canRestore).toBe(false);
  expect(model.message).toContain("at most 1");
});
