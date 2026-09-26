import { describe, expect, test } from "bun:test";
import {
  boundTrashSelection,
  confirmTrashLabel,
  deleteTrash,
  fetchTrashOperation,
  partitionTrashOperation,
  planTrashReview,
  reviewTrash,
  trashSelectAllNotice,
  type TrashOperation,
  type TrashReview,
} from "./trash.js";

const reviewResponse = (): unknown => ({
  operationId: "operation-1",
  items: [
    {
      photoId: "photo-1",
      removedAtMs: 100,
      originalId: "original-1",
      originalLocation: "2024/photo-1.jpg",
      originalKind: "jpeg",
      size: 12,
      albums: [
        { id: "album-1", name: "Keepers" },
        { id: "album-2", name: "Clients" },
      ],
    },
    {
      photoId: "photo-2",
      removedAtMs: 90,
      originalId: "original-2",
      originalLocation: "2024/photo-2.cr2",
      originalKind: "raw",
      size: 2000,
      albums: [{ id: "album-1", name: "Keepers" }],
    },
  ],
  rejected: [
    { photoId: "photo-3", reason: "missing" },
    { photoId: "photo-4", reason: "changed-elsewhere" },
  ],
});

const operationResponse = (
  items: unknown[] = [],
  overrides: Record<string, unknown> = {},
) =>
  ({
    operationId: "operation-2",
    reviewed: 2,
    logicalBytesDeleted: 1536,
    items,
    ...overrides,
  }) as unknown;

describe("Trash API", () => {
  test("posts the fixed selection and accepts a review", async () => {
    let request: { path: string; body: unknown } | undefined;
    const operationId = "operation-1";
    const result = await reviewTrash(
      (path, init) => {
        request = {
          path,
          body:
            typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
        };
        return Promise.resolve(
          new Response(JSON.stringify(reviewResponse()), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        );
      },
      operationId,
      { all: false, photoIds: ["photo-1"], excludePhotoIds: [] },
    );

    expect(request).toEqual({
      path: "/api/trash/review",
      body: {
        operationId,
        all: false,
        photoIds: ["photo-1"],
        excludePhotoIds: [],
      },
    });
    expect(result.kind).toBe("ok");
    if (result.kind === "ok") expect(result.value.items).toHaveLength(2);
  });

  test("accepts a review that rejects an unsettled Photo as pending verification", async () => {
    const result = await reviewTrash(
      () =>
        Promise.resolve(
          new Response(
            JSON.stringify({
              operationId: "operation-1",
              items: [],
              rejected: [
                { photoId: "photo-5", reason: "pending-verification" },
              ],
            }),
            { status: 200, headers: { "Content-Type": "application/json" } },
          ),
        ),
      "operation-1",
      { all: false, photoIds: ["photo-5"], excludePhotoIds: [] },
    );

    expect(result.kind).toBe("ok");
    if (result.kind === "ok")
      expect(result.value.rejected[0]?.reason).toBe("pending-verification");
  });

  test("rejects a review whose operation id differs from the requested one", async () => {
    const result = await reviewTrash(
      () =>
        Promise.resolve(
          new Response(JSON.stringify(reviewResponse()), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        ),
      "operation-9",
      { all: false, photoIds: ["photo-1"], excludePhotoIds: [] },
    );

    expect(result.kind).toBe("malformed");
  });

  test("accepts nullable result details from a settled operation", async () => {
    const result = await deleteTrash(
      () =>
        Promise.resolve(
          new Response(
            JSON.stringify({
              operationId: "operation-2",
              reviewed: 1,
              logicalBytesDeleted: 0,
              items: [
                {
                  photoId: "photo-2",
                  state: "missing",
                  originalLocation: "2024/photo-2.cr2",
                  originalKind: "raw",
                  size: null,
                  message: null,
                },
              ],
            }),
            { status: 200, headers: { "Content-Type": "application/json" } },
          ),
        ),
      "operation-2",
    );

    expect(result.kind).toBe("ok");
    if (result.kind === "ok") {
      expect(result.value.items[0]).toMatchObject({
        photoId: "photo-2",
        state: "missing",
        originalLocation: "2024/photo-2.cr2",
        originalKind: "raw",
        size: null,
        message: null,
      });
    }
  });

  test("rejects an operation item that omits the reviewed Location", async () => {
    const body = operationResponse([
      {
        photoId: "photo-2",
        state: "missing",
        originalKind: "raw",
        size: null,
        message: null,
      },
    ]);
    const result = await deleteTrash(
      () =>
        Promise.resolve(
          new Response(JSON.stringify(body), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        ),
      "operation-2",
    );

    expect(result.kind).toBe("malformed");
  });

  test("rejects an operation item with an unknown kind", async () => {
    const body = operationResponse([
      {
        photoId: "photo-2",
        state: "missing",
        originalLocation: "2024/photo-2.cr2",
        originalKind: "tiff",
        size: null,
        message: null,
      },
    ]);
    const result = await deleteTrash(
      () =>
        Promise.resolve(
          new Response(JSON.stringify(body), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        ),
      "operation-2",
    );

    expect(result.kind).toBe("malformed");
  });

  test("fetches a retained operation result", async () => {
    let path: string | undefined;
    const result = await fetchTrashOperation((input) => {
      path = input;
      return Promise.resolve(
        new Response(
          JSON.stringify(
            operationResponse(
              [
                {
                  photoId: "photo-1",
                  state: "deleting",
                  originalLocation: "2024/photo-1.jpg",
                  originalKind: "jpeg",
                  size: null,
                  message: null,
                },
              ],
              { operationId: "operation 7" },
            ),
          ),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
      );
    }, "operation 7");

    expect(path).toBe("/api/trash/operations/operation%207");
    expect(result.kind).toBe("ok");
  });

  test("reports a lost response as rejected, not as an outcome", async () => {
    const result = await fetchTrashOperation(
      () => Promise.reject(new Error("offline")),
      "operation-2",
    );

    expect(result).toEqual({ kind: "rejected", status: 0 });
  });
});

describe("partitionTrashOperation", () => {
  test("partitions one settled mixed batch and reports bytes for deletions only", () => {
    const operation = {
      operationId: "operation-2",
      reviewed: 5,
      logicalBytesDeleted: 1536,
      items: [
        {
          photoId: "photo-1",
          state: "deleted",
          originalLocation: "2024/photo-1.jpg",
          originalKind: "jpeg",
          size: 1536,
        },
        {
          photoId: "photo-2",
          state: "changed",
          originalLocation: "2024/photo-2.cr2",
          originalKind: "raw",
          size: 2000,
        },
        {
          photoId: "photo-3",
          state: "missing",
          originalLocation: "2024/photo-3.jpg",
          originalKind: "jpeg",
        },
        {
          photoId: "photo-4",
          state: "failed",
          originalLocation: "2024/photo-4.jpg",
          originalKind: "jpeg",
          message: "Permission denied",
        },
      ],
    } as unknown as TrashOperation;

    const partition = partitionTrashOperation(operation);

    expect(partition).toEqual({
      deleted: 1,
      changed: 1,
      missing: 1,
      failed: 1,
      pendingVerification: 0,
      logicalBytesDeleted: 1536,
      settled: true,
      unresolved: [
        {
          photoId: "photo-2",
          state: "changed",
          originalLocation: "2024/photo-2.cr2",
          originalKind: "raw",
          size: 2000,
        },
        {
          photoId: "photo-3",
          state: "missing",
          originalLocation: "2024/photo-3.jpg",
          originalKind: "jpeg",
          size: null,
        },
        {
          photoId: "photo-4",
          state: "failed",
          originalLocation: "2024/photo-4.jpg",
          originalKind: "jpeg",
          size: null,
          message: "Permission denied",
        },
      ],
    });
  });

  test("groups pending, deleting, and uncertain states as pending verification", () => {
    const operation = {
      operationId: "operation-3",
      reviewed: 3,
      logicalBytesDeleted: 0,
      items: [
        {
          photoId: "photo-1",
          state: "pending",
          originalLocation: "2024/photo-1.jpg",
          originalKind: "jpeg",
        },
        {
          photoId: "photo-2",
          state: "deleting",
          originalLocation: "2024/photo-2.jpg",
          originalKind: "jpeg",
        },
        {
          photoId: "photo-3",
          state: "uncertain",
          originalLocation: "2024/photo-3.jpg",
          originalKind: "raw",
        },
      ],
    } as unknown as TrashOperation;

    const partition = partitionTrashOperation(operation);

    expect(partition.pendingVerification).toBe(3);
    expect(partition.deleted).toBe(0);
    expect(partition.settled).toBe(false);
    expect(partition.unresolved).toHaveLength(3);
  });
});

describe("planTrashReview", () => {
  test("summarizes the reviewed files, Albums, and rejections", () => {
    const plan = planTrashReview(reviewResponse() as TrashReview);

    expect(plan.itemCount).toBe(2);
    expect(plan.totalBytes).toBe(2012);
    expect(plan.albumCount).toBe(2);
    expect(plan.albumNames).toEqual(["Keepers", "Clients"]);
    expect(plan.canConfirm).toBe(true);
    expect(plan.items).toEqual([
      {
        photoId: "photo-1",
        originalLocation: "2024/photo-1.jpg",
        originalKind: "jpeg",
        size: 12,
        albumNames: ["Keepers", "Clients"],
      },
      {
        photoId: "photo-2",
        originalLocation: "2024/photo-2.cr2",
        originalKind: "raw",
        size: 2000,
        albumNames: ["Keepers"],
      },
    ]);
    expect(plan.rejected).toEqual([
      { photoId: "photo-3", reasonLabel: "Original missing" },
      {
        photoId: "photo-4",
        reasonLabel: "Original changed since it was removed",
      },
    ]);
    expect(plan.rejectedCount).toBe(2);
  });

  test("offers no delete action when every selected item was rejected", () => {
    const plan = planTrashReview({
      operationId: "operation-1",
      items: [],
      rejected: [{ photoId: "photo-3", reason: "missing" }],
    });

    expect(plan.itemCount).toBe(0);
    expect(plan.canConfirm).toBe(false);
    expect(plan.rejectedCount).toBe(1);
  });

  test("labels a pending-verification rejection", () => {
    const plan = planTrashReview({
      operationId: "operation-1",
      items: [],
      rejected: [{ photoId: "photo-5", reason: "pending-verification" }],
    });

    expect(plan.rejected).toEqual([
      { photoId: "photo-5", reasonLabel: "Deletion result pending" },
    ]);
  });

  test("writes the final action label exactly", () => {
    expect(confirmTrashLabel(2)).toBe("Permanently delete 2 Original Files");
    expect(confirmTrashLabel(1)).toBe("Permanently delete 1 Original Files");
  });
});

const items = [
  {
    photoId: "photo-1",
    removedAtMs: 300,
    pendingVerificationOperationId: null,
  },
  {
    photoId: "photo-2",
    removedAtMs: 200,
    pendingVerificationOperationId: "operation-9",
  },
  {
    photoId: "photo-3",
    removedAtMs: 100,
    pendingVerificationOperationId: null,
  },
];

describe("boundTrashSelection", () => {
  test("stops at the review maximum, keeping the newest eligible items", () => {
    const selection = boundTrashSelection(items, 3, 1);

    expect(selection.selectedCount).toBe(1);
    expect(selection.markers).toEqual([
      { photoId: "photo-1", removedAtMs: 300 },
    ]);
    expect(selection.bounded).toBe(true);
    expect(selection.skippedPending).toBe(1);
  });

  test("selects everything when the Trash fits the review maximum", () => {
    const selection = boundTrashSelection(items, 3, 50);

    expect(selection.selectedCount).toBe(2);
    expect(selection.bounded).toBe(false);
    expect(selection.skippedPending).toBe(1);
  });

  test("never selects an item whose deletion outcome is not settled", () => {
    const selection = boundTrashSelection([items[1]!], 1, 50);

    expect(selection.selectedCount).toBe(0);
    expect(selection.bounded).toBe(false);
    expect(selection.skippedPending).toBe(1);
  });
});

describe("trashSelectAllNotice", () => {
  const bounded = boundTrashSelection(items, 120, 50);

  test("names the total, the review maximum, and the remainder for a bounded capture", () => {
    const notice = trashSelectAllNotice(bounded, 120, 50);

    expect(notice).toBe(
      "Selected the newest 2 of 120 Trash items; a review captures at most 50. Review this batch, then select the rest. 1 item pending verification was not selected.",
    );
  });

  test("is silent about a complete capture that skipped nothing", () => {
    const notice = trashSelectAllNotice(
      boundTrashSelection(
        [
          {
            photoId: "photo-1",
            removedAtMs: 300,
            pendingVerificationOperationId: null,
          },
        ],
        1,
        50,
      ),
      1,
      50,
    );

    expect(notice).toBeUndefined();
  });
});
