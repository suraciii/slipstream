import { describe, expect, test } from "bun:test";
import { createRemovalOwner } from "./removal-owner.js";
import {
  createRemovalReviewController,
  type RemovalReviewModel,
} from "./removal-review-controller.js";
import type { SourceAuthority } from "./source-grid-owner.js";

type Deferred<T> = Readonly<{ promise: Promise<T>; resolve(value: T): void }>;
const deferred = <T>(): Deferred<T> => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((accept) => {
    resolve = accept;
  });
  return { promise, resolve };
};

const authority = Object.freeze({}) as SourceAuthority;
const removalResponse = (operationId: string) =>
  new Response(
    JSON.stringify({
      operationId,
      counts: {
        removed: 1,
        changedElsewhere: 0,
        missing: 0,
        alreadyRemoved: 0,
      },
      changedElsewhere: [],
      missing: [],
      alreadyRemoved: [],
    }),
    { status: 200, headers: { "Content-Type": "application/json" } },
  );

const setup = (
  fetcher: (input: string, init?: RequestInit) => Promise<Response>,
) => {
  let current = {
    token: "snapshot-1",
    total: 1,
    authority,
    selection: "rejected" as const,
  };
  const rendered: RemovalReviewModel[] = [];
  const owner = createRemovalOwner(fetcher, {
    newOperationId: () => "operation-1",
  });
  const controller = createRemovalReviewController({
    removal: owner,
    current: () => current,
    connected: () => true,
    presentation: {
      render: (model) => rendered.push(model),
      open: (model) => rendered.push(model),
      close: () => {},
      renderListingPending: () => {},
      reloadListingAfterUndo: async () => {},
      presentListingMessage: () => {},
    },
    refreshAfterChange: async () => {},
    refreshOverview: async () => {},
    refreshFolders: async () => {},
    reopenAfterRestore: async () => {},
    updateControls: () => {},
  });
  return {
    controller,
    owner,
    rendered,
    replaceCurrent: (token: string) => (current = { ...current, token }),
  };
};

describe("removal review controller fences", () => {
  test("withdraws a review when the presented snapshot changes", () => {
    const { controller, owner, rendered, replaceCurrent } = setup(() =>
      Promise.resolve(removalResponse("operation-1")),
    );
    controller.open();
    expect(owner.review).toBeDefined();
    replaceCurrent("snapshot-2");
    controller.reconcile();
    expect(owner.review).toBeUndefined();
    expect(rendered.at(-1)?.canConfirm).toBe(false);
    expect(rendered.at(-1)?.message).toContain("reviewed result changed");
  });

  test("suppresses late presentation after disposal while admitted write settles", async () => {
    const answer = deferred<Response>();
    const { controller, owner, rendered } = setup(async () => answer.promise);
    controller.open();
    const pending = controller.confirm();
    controller.dispose();
    answer.resolve(removalResponse("operation-1"));
    await pending;
    expect(owner.operation?.removed).toBe(1);
    expect(rendered.at(-1)?.pending).toBe(true);
    expect(rendered.at(-1)?.tone).toBeUndefined();
  });
});
