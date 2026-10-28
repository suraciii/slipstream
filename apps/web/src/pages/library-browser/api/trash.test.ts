import { describe, expect, test } from "bun:test";
import {
  deleteTrash,
  fetchTrashOperation,
  partitionTrashOperation,
  planTrashReview,
  reviewTrash,
  trashAllSelection,
  trashBoundNotice,
  trashExclusionNotice,
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

  test("round-trips a nonempty excludePhotoIds selection and excludes nothing by itself", async () => {
    // No repository test exercised a nonempty exclusion list before: the
    // exclusions travel in the request itself, so the server resolves the
    // remaining current Trash items and an excluded Photo is never named as
    // reviewed, counted, or deleted.
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
          new Response(
            JSON.stringify({
              operationId,
              items: [
                {
                  photoId: "photo-1",
                  removedAtMs: 100,
                  originalId: "original-1",
                  originalLocation: "2024/a.jpg",
                  originalKind: "jpeg",
                  size: 1024,
                  albums: [],
                },
              ],
              rejected: [{ photoId: "photo-2", reason: "missing" }],
            }),
            { status: 200, headers: { "Content-Type": "application/json" } },
          ),
        );
      },
      operationId,
      trashAllSelection({
        excludedPhotoIds: ["photo-2"],
        reviewMaximum: 50,
      }),
    );

    expect(request?.body).toEqual({
      operationId,
      all: true,
      photoIds: [],
      excludePhotoIds: ["photo-2"],
    });
    expect(result.kind).toBe("ok");
    if (result.kind !== "ok") return;
    // The excluded Photo is not one of the reviewed items.
    expect(result.value.items.map((item) => item.photoId)).not.toContain(
      "photo-2",
    );
  });

  test("admits one operation id once: repeating it returns the captured review instead of widening the set", async () => {
    // The captured review is keyed by operation id, so a second submission
    // of the same id with the same intent is a no-op that never adds later
    // Trash arrivals to the reviewed set.
    const responses: string[] = [];
    const respond = (path: string, init?: RequestInit) => {
      responses.push(
        `${path}:${typeof init?.body === "string" ? init.body : ""}`,
      );
      return Promise.resolve(
        new Response(JSON.stringify(reviewResponse()), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    };
    const operationId = "operation-1";
    const selection = trashAllSelection({
      excludedPhotoIds: ["photo-2"],
      reviewMaximum: 50,
    });
    const first = await reviewTrash(respond, operationId, selection);
    const repeated = await reviewTrash(respond, operationId, selection);

    expect(first.kind).toBe("ok");
    expect(repeated.kind).toBe("ok");
    // Both submissions carried the same captured request: the exclusions
    // were part of it, not a filter applied after the fact.
    expect(new Set(responses).size).toBe(1);
    if (repeated.kind === "ok")
      expect(repeated.value.operationId).toBe(operationId);
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
});

describe("trashAllSelection", () => {
  test("submits all current Trash minus the named exclusions", () => {
    // The exclusions are part of the captured request, not a client-side
    // filter applied afterward, so the server resolves the remaining
    // identities itself.
    const selection = trashAllSelection({
      excludedPhotoIds: ["photo-2"],
      reviewMaximum: 50,
    });

    expect(selection).toEqual({
      all: true,
      photoIds: [],
      excludePhotoIds: ["photo-2"],
    });
  });

  test("copies the exclusion identities so a later change cannot widen the captured request", () => {
    const excluded = ["photo-2"];
    const selection = trashAllSelection({
      excludedPhotoIds: excluded,
      reviewMaximum: 50,
    });
    excluded.push("photo-3");

    expect(selection.excludePhotoIds).toEqual(["photo-2"]);
  });

  test("submits no exclusions when nothing was excluded", () => {
    const selection = trashAllSelection({
      excludedPhotoIds: [],
      reviewMaximum: 50,
    });

    expect(selection.excludePhotoIds).toEqual([]);
    expect(selection.all).toBe(true);
  });
});

describe("trashExclusionNotice", () => {
  test("names the excluded count and that those items are never deleted", () => {
    const notice = trashExclusionNotice({
      excludedPhotoIds: ["photo-2", "photo-3"],
      reviewMaximum: 50,
    });

    expect(notice).toBe(
      "2 items excluded from the selection and never deleted.",
    );
  });

  test("names one excluded item in the singular", () => {
    const notice = trashExclusionNotice({
      excludedPhotoIds: ["photo-2"],
      reviewMaximum: 50,
    });

    expect(notice).toBe(
      "1 item excluded from the selection and never deleted.",
    );
  });

  test("stays silent when nothing was excluded", () => {
    expect(
      trashExclusionNotice({ excludedPhotoIds: [], reviewMaximum: 50 }),
    ).toBeUndefined();
  });
});

describe("trashBoundNotice", () => {
  test("names the review bound and the narrowing action instead of a truncated capture", () => {
    const notice = trashBoundNotice({
      excludedPhotoIds: [],
      reviewMaximum: 5000,
    });

    expect(notice).toBe(
      "Selecting all current Trash exceeds the 5,000-item review bound. Narrow the selection by excluding items, then review this batch.",
    );
  });
});
