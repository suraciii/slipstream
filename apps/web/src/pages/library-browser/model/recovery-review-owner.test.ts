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
  refreshSource: async () => {},
};

const makeSurface = () => {
  let unusable = 0;
  let rendered: string | undefined;
  let message: string | undefined;
  const surface: RecoveryReviewSurface = {
    setRecoveryNotice: () => {},
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
    closeRecoveryPanel: () => {},
  };
  return {
    surface,
    unusable: () => unusable,
    rendered: () => rendered,
    message: () => message,
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
