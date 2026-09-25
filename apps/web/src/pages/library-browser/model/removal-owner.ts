import {
  removeRejectedResult,
  restoreRemovedPhotos,
  restoreRemovalOperation,
  type RemovalFetch,
  type RemovalResult,
  type RestorationResult,
  type RestorationWriteResult,
} from "../api/removal.js";
import type { SourceAuthority } from "./source-grid-owner.js";

export type { RemovalFetch } from "../api/removal.js";

/// One reviewed removal: the Browse Snapshot the Photographer reviewed as
/// `Rejected`, the count that Snapshot presented, and the operation identity
/// its confirmation will use. The operation id is created with the review, so
/// a retried confirmation repeats one operation instead of inventing a second
/// outcome set.
export type RemovalReview = Readonly<{
  readonly token: string;
  readonly reviewed: number;
  readonly sourceAuthority: SourceAuthority;
  readonly operationId: string;
}>;

export type RemovalOutcome =
  | Readonly<{ kind: "detached" }>
  | Readonly<{ kind: "removed"; result: RemovalResult }>
  | Readonly<{ kind: "failed"; status?: number }>;

export type RestorationOutcome =
  | Readonly<{ kind: "detached" }>
  | Readonly<{ kind: "restored"; result: RestorationResult }>
  | Readonly<{ kind: "failed"; status?: number }>;

export interface RemovalOwner {
  /// The pending review, or undefined when nothing is being reviewed.
  readonly review: RemovalReview | undefined;
  /// The last confirmed operation Undo can still restore.
  readonly operation:
    | Readonly<{ operationId: string; removed: number }>
    | undefined;
  /// True while any removal or restoration write is in flight.
  readonly busy: boolean;
  openReview(
    token: string,
    reviewed: number,
    sourceAuthority: SourceAuthority,
  ): RemovalReview | undefined;
  isReviewCurrent(review: RemovalReview): boolean;
  discardReview(): void;
  confirm():
    | Readonly<{ review: RemovalReview; settlement: Promise<RemovalOutcome> }>
    | undefined;
  undo():
    | Readonly<{
        operationId: string;
        settlement: Promise<RestorationOutcome>;
      }>
    | undefined;
  forgetOperation(operationId: string): void;
  restorePhotos(
    photoIds: ReadonlyArray<string>,
  ): Readonly<{ settlement: Promise<RestorationOutcome> }> | undefined;
  isRestoringPhotos(photoIds: ReadonlyArray<string>): boolean;
  dispose(): void;
}

export type RemovalOwnerOptions = Readonly<{
  /// Test seam: the identity one removal operation is confirmed under.
  newOperationId?: () => string;
}>;

export function createRemovalOwner(
  fetcher: RemovalFetch,
  options: RemovalOwnerOptions = {},
): RemovalOwner {
  const newOperationId = options.newOperationId ?? (() => crypto.randomUUID());
  /// One admission key per in-flight write: a control that would send a
  /// second identical request while the first is unanswered is refused here,
  /// not only in the page.
  const inFlight = new Set<string>();
  let review: RemovalReview | undefined;
  let operation: Readonly<{ operationId: string; removed: number }> | undefined;
  let closed = false;

  /// Admits one write under `admissionKey` and resolves `detached` when the
  /// page is gone by the time the answer lands. Every caller keeps a
  /// settlement, so no outcome is dropped on the floor.
  const run = <T>(admissionKey: string, write: () => Promise<T>) => {
    if (closed || inFlight.has(admissionKey)) return undefined;
    inFlight.add(admissionKey);
    const settlement = (async (): Promise<
      T | Readonly<{ kind: "detached" }>
    > => {
      try {
        const value = await write();
        return closed ? Object.freeze({ kind: "detached" as const }) : value;
      } finally {
        inFlight.delete(admissionKey);
      }
    })();
    return Object.freeze({ settlement });
  };

  const restorationOutcome = (
    result: RestorationWriteResult,
  ): RestorationOutcome =>
    result.kind === "restored"
      ? Object.freeze({ kind: "restored", result: result.value })
      : Object.freeze({
          kind: "failed",
          ...(result.kind === "rejected" ? { status: result.status } : {}),
        });

  return {
    get review() {
      return review;
    },
    get operation() {
      return operation;
    },
    get busy() {
      return inFlight.size > 0;
    },
    openReview: (token, reviewed, sourceAuthority) => {
      if (closed || token === "" || reviewed <= 0) return undefined;
      if (review?.token === token && review.reviewed === reviewed)
        return review;
      review = Object.freeze({
        token,
        reviewed,
        sourceAuthority,
        operationId: newOperationId(),
      });
      return review;
    },
    isReviewCurrent: (candidate) => !closed && review === candidate,
    discardReview: () => {
      review = undefined;
    },
    confirm: () => {
      const pending = review;
      if (!pending) return undefined;
      const admission = run(
        `confirm:${pending.operationId}`,
        async (): Promise<RemovalOutcome> => {
          const result = await removeRejectedResult(fetcher, {
            token: pending.token,
            operationId: pending.operationId,
            reviewed: pending.reviewed,
          });
          if (result.kind === "removed") {
            // A disposed owner mutates nothing: its settlement still reports
            // the outcome, but no Undo record survives the page that made it.
            if (!closed) {
              if (result.value.counts.removed > 0)
                operation = Object.freeze({
                  operationId: result.value.operationId,
                  removed: result.value.counts.removed,
                });
              // The reviewed result is consumed by its confirmation, so the
              // review never survives its own success. A failure keeps it, and
              // the retry repeats the same operation id.
              if (review === pending) review = undefined;
            }
            return Object.freeze({ kind: "removed", result: result.value });
          }
          return Object.freeze({
            kind: "failed",
            ...(result.kind === "rejected" ? { status: result.status } : {}),
          });
        },
      );
      if (!admission) return undefined;
      return Object.freeze({
        review: pending,
        settlement: admission.settlement,
      });
    },
    undo: () => {
      const confirmed = operation;
      if (!confirmed) return undefined;
      const admission = run(`undo:${confirmed.operationId}`, async () =>
        restorationOutcome(
          await restoreRemovalOperation(fetcher, confirmed.operationId),
        ),
      );
      if (!admission) return undefined;
      return Object.freeze({
        operationId: confirmed.operationId,
        settlement: admission.settlement,
      });
    },
    forgetOperation: (operationId) => {
      if (operation?.operationId === operationId) operation = undefined;
    },
    restorePhotos: (photoIds) => {
      if (photoIds.length === 0) return undefined;
      const admission = run(
        `restore:${[...photoIds].sort().join(",")}`,
        async () =>
          restorationOutcome(await restoreRemovedPhotos(fetcher, photoIds)),
      );
      return admission;
    },
    isRestoringPhotos: (photoIds) =>
      photoIds.length > 0 &&
      inFlight.has(`restore:${[...photoIds].sort().join(",")}`),
    dispose: () => {
      if (closed) return;
      closed = true;
      review = undefined;
    },
  };
}
