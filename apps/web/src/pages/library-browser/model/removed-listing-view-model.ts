import type { RemovedPhotoItem } from "../api/removal.js";
import {
  confirmTrashLabel,
  trashBoundNotice,
  type TrashOutcomePartition,
  type TrashReviewPlan,
} from "../api/trash.js";
import type {
  RemovedPanelViewModel,
  TrashReviewViewModel,
} from "./removed-listing-contract.js";
import type { PendingTrashRestore } from "./trash-restore-storage.js";

/// One bounded page of removed Photos the listing presented.
export type RemovedPageState = Readonly<{
  start: number;
  total: number;
  /// The maximum number of Photos one permanent-deletion review can capture.
  reviewMaximum: number;
  operation: Readonly<{ operationId: string; removed: number }> | undefined;
  items: ReadonlyArray<RemovedPhotoItem>;
}>;

/// The permanent-deletion review currently presented. It names the one
/// operation id the confirmation may use; Cancel discards it and deletes
/// nothing.
export type TrashReviewState = Readonly<{
  operationId: string;
  plan: TrashReviewPlan;
  deleting: boolean;
}>;

/// The Trash deletion operation this listing confirmed or is recovering: its
/// partition once a validated response named it, only its id while the
/// response is lost. A lost response is never presented as proof of
/// failure or success.
export type TrashOperationState = Readonly<{
  operationId: string;
  partition?: TrashOutcomePartition;
}>;

type RemovedPanelState = Readonly<{
  removedPage: RemovedPageState | undefined;
  removedPending: boolean;
  removedSelectionPending: boolean;
  removedLoadFailed: boolean;
  removedAllSelected: boolean;
  removedExcludedIds: ReadonlySet<string>;
  removedPendingIds: ReadonlySet<string>;
  removedAllCount: number;
  removedMessage: string | undefined;
  pendingRestore: PendingTrashRestore | undefined;
  removedDeleting: boolean;
  removedSelectedIds: ReadonlySet<string>;
  removedSelectedMarkers: ReadonlyMap<string, number>;
  removedRestoringSelection: boolean;
  removedRestoringId: string | undefined;
  trashOutcomeBusy: "check" | "resume" | undefined;
  trashOperation: TrashOperationState | undefined;
  pageLimit: number;
}>;

export function trashReviewModel(
  review: TrashReviewState | undefined,
): TrashReviewViewModel {
  if (!review)
    return {
      itemCount: 0,
      totalBytes: 0,
      albumCount: 0,
      albumNames: [],
      items: [],
      rejected: [],
      canConfirm: false,
      confirmLabel: confirmTrashLabel(0),
      deleting: false,
    };
  return {
    itemCount: review.plan.itemCount,
    totalBytes: review.plan.totalBytes,
    albumCount: review.plan.albumCount,
    albumNames: [...review.plan.albumNames],
    items: review.plan.items.map((item) => ({ ...item })),
    rejected: review.plan.rejected.map((item) => ({ ...item })),
    canConfirm: review.plan.canConfirm,
    confirmLabel: confirmTrashLabel(review.plan.itemCount),
    deleting: review.deleting,
  };
}

export function removedPanelModel({
  removedPage,
  removedPending,
  removedSelectionPending,
  removedLoadFailed,
  removedAllSelected,
  removedExcludedIds,
  removedPendingIds,
  removedAllCount,
  removedMessage,
  pendingRestore,
  removedDeleting,
  removedSelectedIds,
  removedSelectedMarkers,
  removedRestoringSelection,
  removedRestoringId,
  trashOutcomeBusy,
  trashOperation,
  pageLimit,
}: RemovedPanelState): RemovedPanelViewModel {
  const pending = removedPending || removedSelectionPending;
  const status = removedPending
    ? "loading"
    : removedLoadFailed
      ? "failed"
      : removedPage
        ? "ready"
        : "loading";
  const notices: string[] = [];
  if (removedAllSelected) {
    const excluded = new Set([...removedExcludedIds, ...removedPendingIds])
      .size;
    notices.push(
      `Review scope: all current Trash items except ${excluded.toLocaleString()} excluded.`,
    );
    if (removedPendingIds.size > 0)
      notices.push(
        `${removedPendingIds.size.toLocaleString()} ${removedPendingIds.size === 1 ? "item" : "items"} pending verification ${removedPendingIds.size === 1 ? "was" : "were"} not selected.`,
      );
    if (removedAllCount > (removedPage?.reviewMaximum ?? 0))
      notices.push(
        trashBoundNotice({
          excludedPhotoIds: [...removedExcludedIds],
          reviewMaximum: removedPage?.reviewMaximum ?? 0,
        }),
      );
  }
  if (removedMessage) notices.push(removedMessage);
  if (pendingRestore)
    notices.push(
      "Restore outcome is unconfirmed. Retry the same operation to recover its result.",
    );
  return {
    status,
    start: removedPage?.start ?? 0,
    total: removedPage?.total ?? 0,
    limit: pageLimit,
    pending,
    deleting: removedDeleting,
    selectionCount: removedAllSelected
      ? removedAllCount
      : removedSelectedIds.size,
    allSelected: removedAllSelected,
    canDelete:
      !pendingRestore &&
      (removedAllSelected
        ? removedAllCount > 0
        : removedSelectedIds.size > 0) &&
      !pending &&
      !removedDeleting &&
      !removedRestoringSelection,
    canRestore:
      (pendingRestore !== undefined ||
        (removedAllSelected
          ? removedAllCount > 0
          : removedSelectedIds.size > 0)) &&
      !pending &&
      !removedDeleting &&
      !removedRestoringSelection,
    canRetry: removedLoadFailed,
    restoringSelection: removedRestoringSelection,
    ...(pendingRestore ? { restoreUnconfirmed: true } : {}),
    ...(status === "ready" && removedPage?.operation
      ? { undo: { removed: removedPage.operation.removed } }
      : {}),
    ...(removedRestoringId ? { restoringPhotoId: removedRestoringId } : {}),
    ...(notices.length ? { message: notices.join(" ") } : {}),
    ...(trashOutcomeBusy ? { outcomeBusy: trashOutcomeBusy } : {}),
    ...(trashOperation
      ? {
          outcome: trashOperation.partition
            ? {
                deleted: trashOperation.partition.deleted,
                changed: trashOperation.partition.changed,
                missing: trashOperation.partition.missing,
                failed: trashOperation.partition.failed,
                pendingVerification:
                  trashOperation.partition.pendingVerification,
                logicalBytesDeleted:
                  trashOperation.partition.logicalBytesDeleted,
                items: trashOperation.partition.unresolved.map((item) => ({
                  ...item,
                })),
              }
            : { unconfirmed: true as const },
        }
      : {}),
    items: (removedPage?.items ?? []).map((item) => {
      const excluded =
        removedAllSelected && removedExcludedIds.has(item.photo.id);
      return {
        photoId: item.photo.id,
        filename: item.photo.originalFilename ?? item.photo.id,
        originalLocation: item.originalLocation,
        originalKind: item.originalKind,
        originalSize: item.originalSize,
        removedAtMs: item.removedAtMs,
        pendingVerificationOperationId: item.pendingVerificationOperationId,
        selected: removedAllSelected
          ? !excluded &&
            item.pendingVerificationOperationId === null &&
            !removedPendingIds.has(item.photo.id)
          : removedSelectedIds.has(item.photo.id) &&
            item.pendingVerificationOperationId === null &&
            removedSelectedMarkers.get(item.photo.id) === item.removedAtMs,
        excluded,
        preview: item.photo.preview,
      };
    }),
  };
}
