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
  let cleared = 0;
  let message: string | undefined;
  const surface: RecoveryReviewSurface = {
    setRecoveryNotice: () => {},
    openRecoveryPanel: () => {},
    renderRecoveryEntries: () => {},
    renderRecoveryProposals: () => {},
    clearRecoveryProposals: () => {
      cleared += 1;
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
    cleared: () => cleared,
    message: () => message,
  };
};

describe("recovery review apply outcomes", () => {
  test("clears the reviewed mappings after a confirmed refusal", async () => {
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

    expect(view.cleared()).toBe(1);
    expect(view.message()).toContain("Review is stale.");
  });

  test("clears the reviewed mappings after an unknown outcome", async () => {
    const view = makeSurface();
    const owner = createRecoveryReviewOwner(
      () => Promise.resolve(new Response("{}", { status: 500 })),
      view.surface,
      page,
    );

    await owner.applyRecovery([mapping]);

    expect(view.cleared()).toBe(1);
    expect(view.message()).toContain("Apply outcome is unknown.");
  });
});
