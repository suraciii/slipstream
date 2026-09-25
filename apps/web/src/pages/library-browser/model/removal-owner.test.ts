import { describe, expect, test } from "bun:test";
import { createRemovalOwner, type RemovalFetch } from "./removal-owner.js";
import type { SourceAuthority } from "./source-grid-owner.js";

type Deferred<T> = Readonly<{
  promise: Promise<T>;
  resolve(value: T): void;
}>;

const deferred = <T>(): Deferred<T> => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((accept) => {
    resolve = accept;
  });
  return { promise, resolve };
};

const authority = Object.freeze({}) as SourceAuthority;

const removalBody = (
  operationId: string,
  input: Readonly<{
    removed?: number;
    changedElsewhere?: ReadonlyArray<string>;
    missing?: ReadonlyArray<string>;
    alreadyRemoved?: ReadonlyArray<string>;
  }>,
) => {
  const changedElsewhere = input.changedElsewhere ?? [];
  const missing = input.missing ?? [];
  const alreadyRemoved = input.alreadyRemoved ?? [];
  return {
    operationId,
    counts: {
      removed: input.removed ?? 0,
      changedElsewhere: changedElsewhere.length,
      missing: missing.length,
      alreadyRemoved: alreadyRemoved.length,
    },
    changedElsewhere,
    missing,
    alreadyRemoved,
  };
};

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });

/// Records every request and answers with the queued response for its path.
const recordingFetch = (
  answer: (path: string, body: unknown) => Response | Promise<Response>,
) => {
  const requests: Array<Readonly<{ path: string; body: unknown }>> = [];
  const fetcher: RemovalFetch = async (path, init) => {
    const body: unknown =
      typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
    requests.push({ path, body });
    return await answer(path, body);
  };
  return { fetcher, requests };
};

const removalPath = "/api/photos/remove";
const restorePath = "/api/photos/restore";

describe("removal owner review", () => {
  test("a review carries the snapshot it reviewed and one stable operation identity", () => {
    let counter = 0;
    const { fetcher } = recordingFetch(() => json(removalBody("unused", {})));
    const owner = createRemovalOwner(fetcher, {
      newOperationId: () => `op-${++counter}`,
    });
    expect(owner.openReview("", 12, authority)).toBeUndefined();
    expect(owner.openReview("token-1", 0, authority)).toBeUndefined();
    const review = owner.openReview("token-1", 12, authority);
    expect(review?.operationId).toBe("op-1");
    expect(owner.review).toBe(review);
    // Reopening the same snapshot repeats the review, so a retried
    // confirmation cannot become a second operation.
    expect(owner.openReview("token-1", 12, authority)).toBe(review);
    const other = owner.openReview("token-2", 12, authority);
    expect(other?.operationId).toBe("op-2");
    expect(owner.isReviewCurrent(review!)).toBe(false);
    expect(owner.isReviewCurrent(other!)).toBe(true);
    owner.dispose();
    expect(owner.review).toBeUndefined();
    expect(owner.isReviewCurrent(other!)).toBe(false);
  });

  test("confirm is refused without a review and admits only one request per review", async () => {
    const held = deferred<Response>();
    const { fetcher, requests } = recordingFetch(() => held.promise);
    const owner = createRemovalOwner(fetcher, {
      newOperationId: () => "op-1",
    });
    expect(owner.confirm()).toBeUndefined();
    owner.openReview("token-1", 3, authority);
    const first = owner.confirm();
    expect(first).toBeDefined();
    // The same review cannot be confirmed twice while its answer is open.
    expect(owner.confirm()).toBeUndefined();
    expect(owner.busy).toBe(true);
    held.resolve(json(removalBody("op-1", { removed: 3 })));
    const outcome = await first!.settlement;
    expect(outcome.kind).toBe("removed");
    expect(requests).toEqual([
      {
        path: removalPath,
        body: { token: "token-1", operationId: "op-1" },
      },
    ]);
    expect(owner.busy).toBe(false);
    // The reviewed result is consumed by its confirmation.
    expect(owner.review).toBeUndefined();
    expect(owner.confirm()).toBeUndefined();
    expect(owner.operation).toEqual({ operationId: "op-1", removed: 3 });
  });

  test("a response that does not account for every reviewed Photo fails and keeps the review", async () => {
    const { fetcher, requests } = recordingFetch(() =>
      json(removalBody("op-1", { removed: 2 })),
    );
    const owner = createRemovalOwner(fetcher, {
      newOperationId: () => "op-1",
    });
    owner.openReview("token-1", 3, authority);
    const first = owner.confirm();
    expect((await first!.settlement).kind).toBe("failed");
    expect(owner.review?.token).toBe("token-1");
    expect(owner.operation).toBeUndefined();
    const retry = owner.confirm();
    await retry!.settlement;
    // The retry repeats one operation instead of inventing a second outcome.
    expect(requests.map((request) => request.body)).toEqual([
      { token: "token-1", operationId: "op-1" },
      { token: "token-1", operationId: "op-1" },
    ]);
  });

  test("a rejected confirmation reports its status and leaves the review retryable", async () => {
    let status = 409;
    const { fetcher } = recordingFetch(() => new Response(null, { status }));
    const owner = createRemovalOwner(fetcher, {
      newOperationId: () => "op-1",
    });
    owner.openReview("token-1", 2, authority);
    const admission = owner.confirm();
    expect(await admission!.settlement).toEqual({
      kind: "failed",
      status: 409,
    });
    status = 503;
    const retry = owner.confirm();
    expect(await retry!.settlement).toEqual({ kind: "failed", status: 503 });
    expect(owner.review).toBeDefined();
  });

  test("a confirmation that removed nothing offers no Undo", async () => {
    const { fetcher } = recordingFetch(() =>
      json(
        removalBody("op-1", {
          alreadyRemoved: ["photo-a"],
          changedElsewhere: ["photo-b"],
        }),
      ),
    );
    const owner = createRemovalOwner(fetcher, {
      newOperationId: () => "op-1",
    });
    owner.openReview("token-1", 2, authority);
    const admission = owner.confirm();
    const outcome = await admission!.settlement;
    expect(outcome.kind).toBe("removed");
    if (outcome.kind !== "removed") return;
    expect(outcome.result.counts).toEqual({
      removed: 0,
      changedElsewhere: 1,
      missing: 0,
      alreadyRemoved: 1,
    });
    expect(owner.operation).toBeUndefined();
    expect(owner.undo()).toBeUndefined();
  });

  test("undo restores the confirmed operation and forgets it only when asked", async () => {
    const { fetcher, requests } = recordingFetch((path) =>
      path === removalPath
        ? json(removalBody("op-1", { removed: 2 }))
        : json({
            counts: { restored: 2, changedElsewhere: 0, missing: 0 },
            changedElsewhere: [],
            missing: [],
          }),
    );
    const owner = createRemovalOwner(fetcher, {
      newOperationId: () => "op-1",
    });
    expect(owner.undo()).toBeUndefined();
    owner.openReview("token-1", 2, authority);
    const admission = owner.confirm();
    await admission!.settlement;
    const undo = owner.undo();
    expect(undo?.operationId).toBe("op-1");
    // One operation is restored once, even if Undo is pressed twice.
    expect(owner.undo()).toBeUndefined();
    const outcome = await undo!.settlement;
    expect(outcome.kind).toBe("restored");
    if (outcome.kind !== "restored") return;
    expect(outcome.result.counts.restored).toBe(2);
    expect(owner.operation).toEqual({ operationId: "op-1", removed: 2 });
    owner.forgetOperation("op-other");
    expect(owner.operation).toBeDefined();
    owner.forgetOperation("op-1");
    expect(owner.operation).toBeUndefined();
    expect(requests.map((request) => request.body)).toEqual([
      { token: "token-1", operationId: "op-1" },
      { operation: "op-1" },
    ]);
  });

  test("restoring explicit Photos admits one request per Photo list", async () => {
    const held = deferred<Response>();
    const { fetcher, requests } = recordingFetch(() => held.promise);
    const owner = createRemovalOwner(fetcher);
    expect(owner.restorePhotos([])).toBeUndefined();
    const admission = owner.restorePhotos(["photo-a"]);
    expect(admission).toBeDefined();
    expect(owner.isRestoringPhotos(["photo-a"])).toBe(true);
    expect(owner.restorePhotos(["photo-a"])).toBeUndefined();
    expect(owner.restorePhotos(["photo-b"])).toBeDefined();
    held.resolve(
      json({
        counts: { restored: 1, changedElsewhere: 0, missing: 0 },
        changedElsewhere: [],
        missing: [],
      }),
    );
    expect((await admission!.settlement).kind).toBe("restored");
    expect(owner.isRestoringPhotos(["photo-a"])).toBe(false);
    expect(requests).toEqual([
      { path: restorePath, body: { photoIds: ["photo-a"] } },
      { path: restorePath, body: { photoIds: ["photo-b"] } },
    ]);
  });

  test("a restore response that does not partition the requested Photos fails", async () => {
    const { fetcher } = recordingFetch(() =>
      json({
        counts: { restored: 1, changedElsewhere: 0, missing: 0 },
        changedElsewhere: [],
        missing: [],
      }),
    );
    const owner = createRemovalOwner(fetcher);
    const admission = owner.restorePhotos(["photo-a", "photo-b"]);
    expect(await admission!.settlement).toEqual({ kind: "failed" });
  });

  test("a disposed owner detaches its settlements and refuses new writes", async () => {
    const held = deferred<Response>();
    const { fetcher } = recordingFetch(() => held.promise);
    const owner = createRemovalOwner(fetcher, {
      newOperationId: () => "op-1",
    });
    owner.openReview("token-1", 1, authority);
    const admission = owner.confirm();
    owner.dispose();
    held.resolve(json(removalBody("op-1", { removed: 1 })));
    expect(await admission!.settlement).toEqual({ kind: "detached" });
    expect(owner.operation).toBeUndefined();
    expect(owner.openReview("token-1", 1, authority)).toBeUndefined();
    expect(owner.confirm()).toBeUndefined();
  });
});
