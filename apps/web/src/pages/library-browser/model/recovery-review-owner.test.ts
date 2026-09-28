import { describe, expect, test } from "bun:test";
import {
  createRecoveryReviewOwner,
  type RecoveryReviewPage,
  type RecoveryReviewSurface,
} from "./recovery-review-owner.js";
import type { RecoveryApplyMapping } from "../api/recovery.js";

const mapping: RecoveryApplyMapping = {
  originalId: "original-1",
  newLocation: "moved/photo.ARW",
  mappingId: "mapping-1",
};

const page: RecoveryReviewPage = {
  isAlive: () => true,
  setGridStatusText: () => {},
  refreshSource: () => Promise.resolve(),
  refreshRecoveryOverview: () => Promise.resolve(),
};

const makePage = () => {
  let refreshes = 0;
  let overviewRefreshes = 0;
  const testPage: RecoveryReviewPage = {
    isAlive: () => true,
    setGridStatusText: () => {},
    refreshSource: () => {
      refreshes += 1;
      return Promise.resolve();
    },
    refreshRecoveryOverview: () => {
      overviewRefreshes += 1;
      return Promise.resolve();
    },
  };
  return {
    testPage,
    refreshes: () => refreshes,
    overviewRefreshes: () => overviewRefreshes,
  };
};

const makeSurface = () => {
  let unusable = 0;
  let rendered: string | undefined;
  let message: string | undefined;
  let notice:
    | Readonly<{ relocatedPhotos: number; unavailablePhotos: number }>
    | undefined;
  let closed = 0;
  const surface: RecoveryReviewSurface = {
    setRecoveryNotice: (model) => {
      notice = {
        relocatedPhotos: model.relocatedPhotos,
        unavailablePhotos: model.unavailablePhotos,
      };
    },
    openRecoveryPanel: () => {},
    renderRecoveryEntries: () => {},
    renderRecoveryProposals: (mappings) => {
      rendered = mappings[0]?.originalId;
    },
    clearRecoveryProposals: () => {},
    markRecoveryProposalsUnusable: () => {
      unusable += 1;
    },
    resetRecoveryProposalChoices: () => {},
    setRecoveryPending: () => {},
    setRecoveryMessage: (text) => {
      message = text;
    },
    closeRecoveryPanel: () => {
      closed += 1;
    },
  };
  return {
    surface,
    unusable: () => unusable,
    rendered: () => rendered,
    message: () => message,
    notice: () => notice,
    closed: () => closed,
  };
};

describe("recovery review apply outcomes", () => {
  test("keeps reviewed mappings visible after a confirmed refusal", async () => {
    const view = makeSurface();
    const owner = createRecoveryReviewOwner(
      () =>
        Promise.resolve(
          new Response(
            JSON.stringify({
              message: "Review is stale.",
              appliedMappings: 0,
              refusedMappings: 1,
              rejections: [{ originalId: mapping.originalId, reason: "stale" }],
            }),
            { status: 409 },
          ),
        ),
      view.surface,
      page,
    );

    await owner.applyRecovery([mapping]);

    expect(view.unusable()).toBe(1);
    expect(view.message()).toContain("Review is stale.");
  });

  test("keeps reviewed mappings visible after an unknown outcome", async () => {
    const view = makeSurface();
    const owner = createRecoveryReviewOwner(
      () => Promise.resolve(new Response("{}", { status: 500 })),
      view.surface,
      page,
    );

    await owner.applyRecovery([mapping]);

    expect(view.unusable()).toBe(1);
    expect(view.message()).toContain("Apply outcome is unknown.");
  });
});
const proposal = (originalId: string) =>
  JSON.stringify({
    items: [
      {
        mappingId: `mapping-${originalId}`,
        originalId,
        photoId: `photo-${originalId}`,
        fromLocation: "old/photo.ARW",
        toLocation: "moved/photo.ARW",
        kind: "raw",
        outcome: "matched",
        verified: true,
        blockedReason: null,
        retire: null,
      },
    ],
    total: 1,
    nextCursor: null,
    evaluatedAt: "2026-01-01T00:00:00.000Z",
    expiresAt: null,
  });

test("ignores a superseded proposal response", async () => {
  let resolveBatch!: (response: Response) => void;
  let resolveSingle!: (response: Response) => void;
  const fetcher = (_input: string, init?: RequestInit) => {
    if (typeof init?.body !== "string") throw new Error("expected JSON body");
    const body = JSON.parse(init.body) as {
      oldPrefix?: string;
    };
    return new Promise<Response>((resolve) => {
      if (body.oldPrefix !== undefined) resolveBatch = resolve;
      else resolveSingle = resolve;
    });
  };
  const view = makeSurface();
  const owner = createRecoveryReviewOwner(fetcher, view.surface, page);
  const batch = owner.proposeRecoveryBatch("old", "moved");
  const single = owner.proposeRecoverySingle("original-2", "moved/photo.ARW");
  resolveSingle(new Response(proposal("original-2")));

  await single;
  resolveBatch(new Response(proposal("original-1")));
  await batch;
  expect(view.rendered()).toBe("original-2");
});

const isApplyBody = (body: unknown): boolean =>
  typeof body === "object" && body !== null && "mappings" in body;

const applyResult = (originalId: string, unavailablePhotos: number) =>
  new Response(
    JSON.stringify({
      appliedMappings: 1,
      refusedMappings: 0,
      unavailablePhotos,
      mappings: [
        {
          originalId,
          photoId: `photo-${originalId}`,
          fromLocation: "old/photo.ARW",
          toLocation: "moved/photo.ARW",
          webUrl: `/photos/photo-${originalId}`,
          retired: null,
        },
      ],
    }),
  );

test("reloads the source for a superseded apply without regressing newer counts", async () => {
  let resolveStaleApply!: (response: Response) => void;
  let resolveCurrentApply!: (response: Response) => void;
  let resolveBatch!: (response: Response) => void;
  let applyRequests = 0;
  const fetcher = (_input: string, init?: RequestInit) => {
    if (typeof init?.body !== "string") throw new Error("expected JSON body");
    const body: unknown = JSON.parse(init.body);
    return new Promise<Response>((resolve) => {
      if (!isApplyBody(body)) resolveBatch = resolve;
      else if (applyRequests++ === 0) resolveStaleApply = resolve;
      else resolveCurrentApply = resolve;
    });
  };
  const view = makeSurface();
  const { testPage, refreshes, overviewRefreshes } = makePage();
  const owner = createRecoveryReviewOwner(fetcher, view.surface, testPage);
  const stale = owner.applyRecovery([mapping]);
  const batch = owner.proposeRecoveryBatch("old", "moved");
  resolveBatch(new Response(proposal("original-2")));
  await batch;
  const current = owner.applyRecovery([
    {
      originalId: "original-2",
      newLocation: "moved/photo.ARW",
      mappingId: "mapping-original-2",
    },
  ]);
  // The newer apply commits first; its counts are the freshest truth.
  resolveCurrentApply(applyResult("original-2", 2));
  await current;
  expect(view.notice()).toEqual({
    relocatedPhotos: 1,
    unavailablePhotos: 2,
  });
  expect(view.closed()).toBe(1);
  expect(overviewRefreshes()).toBe(0);
  // The superseded apply's older answer must still reload the source and
  // re-read the authoritative Overview, but cannot overwrite the newer
  // counts with its stale transaction's.
  resolveStaleApply(applyResult(mapping.originalId, 3));
  await stale;
  expect(refreshes()).toBe(2);
  expect(overviewRefreshes()).toBe(1);
  expect(view.notice()).toEqual({
    relocatedPhotos: 1,
    unavailablePhotos: 2,
  });
  expect(view.rendered()).toBe("original-2");
  expect(view.unusable()).toBe(0);
  expect(view.message()).toBeUndefined();
});

test("drops a superseded apply refusal without touching the newer proposal", async () => {
  let resolveApply!: (response: Response) => void;
  let resolveBatch!: (response: Response) => void;
  const fetcher = (_input: string, init?: RequestInit) => {
    if (typeof init?.body !== "string") throw new Error("expected JSON body");
    const body: unknown = JSON.parse(init.body);
    return new Promise<Response>((resolve) => {
      if (isApplyBody(body)) resolveApply = resolve;
      else resolveBatch = resolve;
    });
  };
  const view = makeSurface();
  const { testPage, refreshes } = makePage();
  const owner = createRecoveryReviewOwner(fetcher, view.surface, testPage);
  const applied = owner.applyRecovery([mapping]);
  const batch = owner.proposeRecoveryBatch("old", "moved");
  resolveBatch(new Response(proposal("original-2")));
  await batch;
  resolveApply(
    new Response(
      JSON.stringify({
        message: "Review is stale.",
        appliedMappings: 0,
        refusedMappings: 1,
        rejections: [{ originalId: mapping.originalId, reason: "stale" }],
      }),
      { status: 409 },
    ),
  );
  await applied;
  expect(refreshes()).toBe(0);
  expect(view.unusable()).toBe(0);
  expect(view.message()).toBeUndefined();
  expect(view.rendered()).toBe("original-2");
});
