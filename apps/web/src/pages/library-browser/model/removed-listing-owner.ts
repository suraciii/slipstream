/// The Removed Photos listing (Trash): the bounded pages of the Library's
/// committed removals, the selection one permanent-deletion review covers,
/// and the Trash operations whose outcomes recover. The owner keeps the
/// listing's state, its admission guards, its cancellation, and its
/// stale-response checks; the page presents the projections it computes and
/// coordinates the owners a committed write touches.

import {
  fetchRemovedPhotos,
  restoreAllTrash,
  type RemovalFetch,
  type RestorationResult,
} from "../api/removal.js";
import { randomUuid } from "./browser-crypto.js";
import {
  readPendingTrashRestore,
  storePendingTrashRestore,
} from "./trash-restore-storage.js";
import {
  deleteTrash,
  fetchTrashOperation,
  partitionTrashOperation,
  planTrashReview,
  reviewTrash,
  trashAllSelection,
  trashExclusionNotice,
  type TrashExclusionSelection,
  type TrashReview,
} from "../api/trash.js";
import type { RemovalOwner } from "./removal-owner.js";
import type {
  RemovedPanelViewModel,
  TrashReviewViewModel,
} from "./removed-listing-contract.js";
import {
  removedPanelModel,
  trashReviewModel,
  type RemovedPageState,
  type TrashOperationState,
  type TrashReviewState,
} from "./removed-listing-view-model.js";

/// The page's half of the listing: the surfaces that present the models the
/// owner computes, and the control update every settled state reaches. The
/// page owns the markup; the owner owns every fact a model carries.
export type RemovedListingPresentation = Readonly<{
  /// Opens the listing surface with its first, pending projection.
  openPanel: (model: RemovedPanelViewModel) => void;
  /// Re-presents the listing's current projection.
  renderPanel: (model: RemovedPanelViewModel) => void;
  /// Closes the listing surface.
  closePanel: () => void;
  /// Opens the permanent-deletion review above the listing.
  openReview: (model: TrashReviewViewModel) => void;
  /// Re-presents the open review's current projection.
  renderReview: (model: TrashReviewViewModel) => void;
  /// Closes the permanent-deletion review.
  closeReview: () => void;
  /// Control availability changed, as every settled listing state does.
  controlsChanged: () => void;
}>;

/// The coordination a committed listing write needs from the page: the
/// owners it refreshes, the source it reopens, and the words one restore's
/// counts report.
export type RemovedListingCoordination = Readonly<{
  /// Refreshes the overview and the derived counts the write invalidated.
  refreshLibrary: () => Promise<void>;
  /// Reopens the current source after a restore changed the Library under
  /// it.
  reopenSourceAfterRestore: () => Promise<void>;
  /// Names one restore's outcome the way the removal surfaces do.
  restorationMessage: (counts: RestorationResult["counts"]) => string;
}>;

export type RemovedListingOwnerOptions = Readonly<{
  /// The removal operations the listing recovers: the Undo record a listing
  /// read keeps current, and the explicit-list restores a row or a
  /// selection asks for.
  removal: RemovalOwner;
  present: RemovedListingPresentation;
  coordinate: RemovedListingCoordination;
}>;

export interface RemovedListingOwner {
  /// Opens the listing and presents its first page.
  openPanel(): void;
  /// Closes the listing and forgets its session state.
  closePanel(): void;
  /// Re-presents the listing's current projection, as a settlement the
  /// page owns settles around.
  render(): void;
  /// Turns one page within the bounds the presented page names.
  turnPage(direction: -1 | 1): void;
  /// Repeats the last failed read of the listing.
  retry(): void;
  /// Selects current Trash minus exclusions, scanning pages for pending items.
  selectAll(): void;
  clearSelection(): void;
  /// Toggles one listed Photo's membership in the selection.
  toggle(photoId: string, selected: boolean): void;
  /// Opens the permanent-deletion review for the current selection.
  deleteSelection(): Promise<void>;
  restoreSelection(): Promise<void>;
  /// Restores one listed Photo, unless its deletion outcome is unresolved.
  restorePhoto(photoId: string, removedAtMs: number): void;
  confirmReview(): Promise<void>;
  cancelReview(): void;
  /// Fetches the retained result of the operation whose response was lost.
  checkOutcome(): Promise<void>;
  /// Re-POSTs the retained operation whose response was lost.
  retryDelete(): Promise<void>;
  /// Fetches one row's pending operation's retained result.
  checkRow(photoId: string): Promise<void>;
  /// Re-POSTs one row's pending operation.
  resumeRow(photoId: string): Promise<void>;
  /// Presents one message on the listing surface without reading it again.
  presentMessage(message: string): void;
  /// Reads the open listing again and reports `message` when it lands, as
  /// an Undo's restore does.
  reloadAfterUndo(message: string): Promise<void>;
  /// Aborts the listing's in-flight reads, as the page's teardown does.
  dispose(): void;
}

/// This Library's retained Trash deletion operation. The Library Browser
/// serves one Library per origin, so this key is the Library's slot. The id
/// is kept only while a delete response was lost, so reopening Trash can
/// recover the operation's result instead of presenting the loss as proof
/// of failure or success.
const TRASH_OPERATION_STORAGE_KEY = "slipstream:trash-deletion-operation";

const readStoredTrashOperation = (): string | undefined => {
  try {
    const value = window.localStorage.getItem(TRASH_OPERATION_STORAGE_KEY);
    return value === null || value.length === 0 ? undefined : value;
  } catch {
    return undefined;
  }
};

const storeTrashOperation = (operationId: string | undefined): void => {
  try {
    if (operationId === undefined)
      window.localStorage.removeItem(TRASH_OPERATION_STORAGE_KEY);
    else window.localStorage.setItem(TRASH_OPERATION_STORAGE_KEY, operationId);
  } catch {
    // Storage unavailable: the recovery then lives only in this panel.
  }
};

export function createRemovedListingOwner(
  fetcher: RemovalFetch,
  options: RemovedListingOwnerOptions,
): RemovedListingOwner {
  const { removal, present, coordinate } = options;
  /// One bounded page of removed Photos. The listing is not a source and
  /// creates no second browsing model: it presents the recovery path for
  /// the removals the Library has committed.
  const REMOVED_PAGE_LIMIT = 50;
  let removedPage: RemovedPageState | undefined;
  let removedLoadFailed = false;
  let removedPanelOpen = false;
  let removedAbort: AbortController | undefined;
  let removedPending = false;
  let removedRestoringId: string | undefined;
  let removedRestoringSelection = false;
  let pendingRestore = readPendingTrashRestore();
  let removedMessage: string | undefined;
  const removedSelectedIds = new Set<string>();
  const removedSelectedMarkers = new Map<string, number>();
  /// All mode is resolved by the server at review admission. Pending rows and
  /// manually unchecked rows are excluded before that capture.
  let removedAllSelected = false;
  let removedExcludedIds: ReadonlySet<string> = new Set<string>();
  let removedPendingIds: ReadonlySet<string> = new Set<string>();
  let removedAllCount = 0;
  let selectionAbort: AbortController | undefined;
  let removedSelectionPending = false;
  let removedDeleting = false;
  let trashReview: TrashReviewState | undefined;
  let trashOperation: TrashOperationState | undefined;
  /// The retained-operation action in flight, so its control cannot be
  /// re-entered while it settles.
  let trashOutcomeBusy: "check" | "resume" | undefined;

  /// Scan all pages for pending-verification rows, which must not be sent to
  /// review. Refuse inconsistent pages instead of silently omitting rows.
  const selectAllRemoved = async (refresh = false): Promise<boolean> => {
    if (
      removedPending ||
      removedSelectionPending ||
      removedDeleting ||
      pendingRestore !== undefined ||
      removedRestoringSelection ||
      removedPage === undefined
    )
      return false;

    const controller = new AbortController();
    selectionAbort = controller;
    removedSelectionPending = true;
    removedMessage = undefined;
    renderRemovedPanel();
    present.controlsChanged();

    const pendingIds = new Set<string>();
    const seen = new Set<string>();
    let total: number | undefined;
    let maximum: number | undefined;
    let failed = false;
    for (
      let start = 0;
      total === undefined || start < total;
      start += REMOVED_PAGE_LIMIT
    ) {
      const page =
        !refresh && start === removedPage.start
          ? { kind: "ok" as const, ...removedPage, photos: removedPage.items }
          : await fetchRemovedPhotos(fetcher, {
              start,
              limit: REMOVED_PAGE_LIMIT,
              signal: controller.signal,
            });
      if (controller.signal.aborted || selectionAbort !== controller)
        return false;
      if (page.kind !== "ok") {
        failed = true;
        break;
      }
      if (total === undefined) {
        total = page.total;
        maximum = page.reviewMaximum;
      } else if (page.total !== total || page.reviewMaximum !== maximum) {
        failed = true;
        break;
      }
      for (const item of page.photos) {
        if (seen.has(item.photo.id)) {
          failed = true;
          break;
        }
        seen.add(item.photo.id);
        if (item.pendingVerificationOperationId !== null)
          pendingIds.add(item.photo.id);
      }
      if (failed) break;
    }
    selectionAbort = undefined;
    removedSelectionPending = false;
    if (failed) {
      removedMessage =
        "Trash changed or could not be read while selecting. Reload and try again.";
    } else {
      if (!refresh) {
        removedSelectedIds.clear();
        removedSelectedMarkers.clear();
        removedExcludedIds = new Set<string>();
      }
      removedPendingIds = pendingIds;
      removedAllCount =
        (total ?? 0) -
        pendingIds.size -
        [...removedExcludedIds].filter(
          (id) => seen.has(id) && !pendingIds.has(id),
        ).length;
      removedAllSelected = true;
    }
    renderRemovedPanel();
    present.controlsChanged();
    return !failed;
  };

  const trashExclusionSelection = (): TrashExclusionSelection => ({
    excludedPhotoIds: [...removedExcludedIds],
    reviewMaximum: removedPage?.reviewMaximum ?? 0,
  });

  const clearRemovedSelection = (): void => {
    if (
      removedPending ||
      removedSelectionPending ||
      removedDeleting ||
      removedRestoringSelection ||
      pendingRestore !== undefined
    )
      return;
    removedSelectedIds.clear();
    removedSelectedMarkers.clear();
    removedAllSelected = false;
    removedExcludedIds = new Set<string>();
    removedMessage = undefined;
    renderRemovedPanel();
    present.controlsChanged();
  };

  /// Opens a review for the current all-minus-exclusions or explicit selection.
  const deleteSelectedTrash = async (): Promise<void> => {
    if (
      removedPending ||
      removedSelectionPending ||
      removedDeleting ||
      removedRestoringSelection ||
      pendingRestore !== undefined ||
      (removedAllSelected
        ? removedAllCount === 0
        : removedSelectedIds.size === 0) ||
      trashReview !== undefined
    )
      return;
    if (removedAllSelected && !(await selectAllRemoved(true))) return;
    if (removedAllSelected && removedAllCount === 0) return;
    const operationId = randomUuid();
    const controller = new AbortController();
    selectionAbort = controller;
    removedSelectionPending = true;
    renderRemovedPanel();
    present.controlsChanged();
    const selection = removedAllSelected
      ? trashAllSelection({
          excludedPhotoIds: [
            ...new Set([...removedExcludedIds, ...removedPendingIds]),
          ],
          reviewMaximum: removedPage?.reviewMaximum ?? 0,
        })
      : { all: false, photoIds: [...removedSelectedIds], excludePhotoIds: [] };
    const reviewResult = await reviewTrash(fetcher, operationId, selection);
    if (controller.signal.aborted || selectionAbort !== controller) return;
    selectionAbort = undefined;
    removedSelectionPending = false;
    if (reviewResult.kind !== "ok") {
      removedMessage =
        reviewResult.kind === "rejected" && reviewResult.status === 409
          ? "Trash changed while it was being reviewed. Reload and select the files again."
          : "The permanent deletion review could not be created. Nothing was deleted.";
      renderRemovedPanel();
      present.controlsChanged();
      return;
    }
    const review: TrashReview = reviewResult.value;
    if (
      !removedAllSelected &&
      review.items.some(
        (item) => removedSelectedMarkers.get(item.photoId) !== item.removedAtMs,
      )
    ) {
      removedMessage =
        "Trash changed while it was being reviewed. Reload and select the files again.";
      renderRemovedPanel();
      present.controlsChanged();
      return;
    }
    trashReview = {
      operationId: review.operationId,
      plan: planTrashReview(review),
      deleting: false,
    };
    renderRemovedPanel();
    present.controlsChanged();
    present.openReview(trashReviewModel(trashReview));
  };

  const cancelTrashReview = (): void => {
    if (!trashReview) return;
    trashReview = undefined;
    // Cancel deletes nothing: the delete route is never called for a
    // discarded review.
    present.closeReview();
    // Closing the review hands focus back into the listing, which had to
    // close for the review; presenting it again rebinds its thumbnails.
    renderRemovedPanel();
    present.controlsChanged();
  };

  /// Confirms the open review. The confirmation names only the reviewed
  /// operation id, so the delete route can never widen the reviewed set.
  const confirmTrashReview = async (): Promise<void> => {
    const review = trashReview;
    if (!review || review.deleting || !review.plan.canConfirm || pendingRestore)
      return;
    trashReview = { ...review, deleting: true };
    renderTrashReview();
    await settleTrashDelete(review.operationId);
    trashReview = undefined;
    present.closeReview();
    renderRemovedPanel();
    present.controlsChanged();
  };

  const renderTrashReview = (): void => {
    if (!trashReview) return;
    present.renderReview(trashReviewModel(trashReview));
  };

  /// Confirms one reviewed operation id, or retries it: the server repeats
  /// only its unresolved items and never changes the reviewed set. A lost
  /// response keeps the operation id and offers recovery instead of claiming
  /// failure or success.
  const settleTrashDelete = async (operationId: string): Promise<void> => {
    if (pendingRestore) return;
    removedDeleting = true;
    renderRemovedPanel();
    renderTrashReview();
    present.controlsChanged();
    const result = await deleteTrash(fetcher, operationId);
    removedDeleting = false;
    if (result.kind === "ok") {
      removedSelectedIds.clear();
      removedSelectedMarkers.clear();
      removedAllSelected = false;
      removedExcludedIds = new Set<string>();
      const partition = partitionTrashOperation(result.value);
      // The result is durable server-side: the retained id is only needed
      // while an item is still pending verification.
      storeTrashOperation(partition.settled ? undefined : operationId);
      trashOperation = { operationId, partition };
      removedMessage = undefined;
      // The confirmed deletions leave Trash, so the derived counts and the
      // listing read again before they report the outcome — the same refresh
      // the restore path uses.
      await coordinate.refreshLibrary();
      await loadRemovedPage(removedPage?.start ?? 0, undefined, {
        keepOutcome: true,
      });
      return;
    }
    // The response was lost or unusable. The operation id is retained so
    // reopening Trash can recover the result; nothing is claimed yet.
    storeTrashOperation(operationId);
    trashOperation = { operationId };
    renderRemovedPanel();
    present.controlsChanged();
  };

  /// Fetches the retained result of the operation whose response was lost.
  const checkTrashResult = async (): Promise<void> => {
    const current = trashOperation;
    if (!current || trashOutcomeBusy || removedDeleting) return;
    trashOutcomeBusy = "check";
    renderRemovedPanel();
    present.controlsChanged();
    const result = await fetchTrashOperation(fetcher, current.operationId);
    trashOutcomeBusy = undefined;
    if (result.kind === "ok") {
      const partition = partitionTrashOperation(result.value);
      if (partition.settled) storeTrashOperation(undefined);
      trashOperation = { operationId: current.operationId, partition };
    }
    renderRemovedPanel();
    present.controlsChanged();
  };

  /// Re-POSTs the retained operation: the server repeats only its unresolved
  /// items and never changes the reviewed set.
  const retryTrashDelete = async (): Promise<void> => {
    const current = trashOperation;
    if (!current || trashOutcomeBusy || removedDeleting) return;
    await settleTrashDelete(current.operationId);
    renderRemovedPanel();
    present.controlsChanged();
  };

  /// The recovery one pending-verification row offers: Check result fetches
  /// the operation's retained result; Resume re-POSTs it.
  const checkTrashRow = async (photoId: string): Promise<void> => {
    if (trashOutcomeBusy || removedDeleting) return;
    const item = removedPage?.items.find(
      (candidate) => candidate.photo.id === photoId,
    );
    const operationId = item?.pendingVerificationOperationId;
    if (!operationId) return;
    trashOutcomeBusy = "check";
    renderRemovedPanel();
    present.controlsChanged();
    const result = await fetchTrashOperation(fetcher, operationId);
    trashOutcomeBusy = undefined;
    if (result.kind === "ok") {
      const partition = partitionTrashOperation(result.value);
      if (partition.settled && readStoredTrashOperation() === operationId)
        storeTrashOperation(undefined);
      trashOperation = { operationId, partition };
    }
    renderRemovedPanel();
    present.controlsChanged();
  };

  const resumeTrashRow = async (photoId: string): Promise<void> => {
    if (trashOutcomeBusy || removedDeleting) return;
    const item = removedPage?.items.find(
      (candidate) => candidate.photo.id === photoId,
    );
    const operationId = item?.pendingVerificationOperationId;
    if (!operationId) return;
    await settleTrashDelete(operationId);
    renderRemovedPanel();
    present.controlsChanged();
  };

  const renderRemovedPanel = (): void => {
    present.renderPanel(
      removedPanelModel({
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
        pageLimit: REMOVED_PAGE_LIMIT,
      }),
    );
  };

  const loadRemovedPage = async (
    start: number,
    successMessage?: string,
    options?: Readonly<{ keepOutcome?: boolean }>,
  ): Promise<void> => {
    removedAbort?.abort();
    const controller = new AbortController();
    removedAbort = controller;
    removedPending = true;
    removedLoadFailed = false;
    removedMessage = undefined;
    // A reload of the listing dismisses the outcome the panel presented,
    // unless the reload is the one the deletion itself triggered.
    if (!options?.keepOutcome) trashOperation = undefined;
    renderRemovedPanel();
    present.controlsChanged();
    const result = await fetchRemovedPhotos(fetcher, {
      start,
      limit: REMOVED_PAGE_LIMIT,
      signal: controller.signal,
    });
    if (controller.signal.aborted || removedAbort !== controller) return;
    removedAbort = undefined;
    removedPending = false;
    if (result.kind === "ok") {
      const pageStart =
        result.total === 0
          ? 0
          : result.start >= result.total
            ? Math.floor((result.total - 1) / REMOVED_PAGE_LIMIT) *
              REMOVED_PAGE_LIMIT
            : result.start;
      if (pageStart !== result.start) {
        void loadRemovedPage(pageStart, successMessage, options);
        return;
      }
      removal.rememberOperation(result.operation);
      removedPage = {
        start: result.start,
        total: result.total,
        reviewMaximum: result.reviewMaximum,
        operation: result.operation,
        items: result.photos,
      };
      for (const item of result.photos) {
        if (
          removedSelectedIds.has(item.photo.id) &&
          (item.pendingVerificationOperationId !== null ||
            removedSelectedMarkers.get(item.photo.id) !== item.removedAtMs)
        ) {
          removedSelectedIds.delete(item.photo.id);
          removedSelectedMarkers.delete(item.photo.id);
        }
      }
      removedMessage = successMessage;
    } else {
      removedLoadFailed = true;
      removedMessage = "Trash could not be loaded. Retry to continue.";
    }
    renderRemovedPanel();
    present.controlsChanged();
  };

  const openRemovedPanel = (): void => {
    removedPanelOpen = true;
    removedPage = undefined;
    removedLoadFailed = false;
    removedRestoringId = undefined;
    removedRestoringSelection = false;
    pendingRestore ??= readPendingTrashRestore();
    removedMessage = undefined;
    removedSelectedIds.clear();
    removedSelectedMarkers.clear();
    removedAllSelected = false;
    removedExcludedIds = new Set<string>();
    removedSelectionPending = false;
    removedDeleting = false;
    trashReview = undefined;
    trashOutcomeBusy = undefined;
    trashOperation = undefined;
    present.openPanel({
      status: "loading",
      start: 0,
      total: 0,
      limit: REMOVED_PAGE_LIMIT,
      pending: true,
      deleting: false,
      selectionCount: 0,
      allSelected: false,
      canDelete: false,
      canRestore: false,
      canRetry: false,
      restoringSelection: false,
      items: [],
    });
    void reopenStoredTrashOperation();
    void loadRemovedPage(0);
  };

  /// Reopening Trash recovers the operation whose response was lost before
  /// the reload: the retained id is fetched and its outcome or pending state
  /// is presented. The retained id is cleared once no returned item is
  /// pending verification.
  const reopenStoredTrashOperation = async (): Promise<void> => {
    const stored = readStoredTrashOperation();
    if (!stored) return;
    trashOperation = { operationId: stored };
    renderRemovedPanel();
    present.controlsChanged();
    const result = await fetchTrashOperation(fetcher, stored);
    if (result.kind === "ok") {
      const partition = partitionTrashOperation(result.value);
      if (partition.settled) storeTrashOperation(undefined);
      trashOperation = { operationId: stored, partition };
    }
    renderRemovedPanel();
    present.controlsChanged();
  };

  const closeRemovedPanel = (): void => {
    removedPanelOpen = false;
    selectionAbort?.abort();
    selectionAbort = undefined;
    removedAbort?.abort();
    removedAbort = undefined;
    removedPending = false;
    removedSelectionPending = false;
    removedDeleting = false;
    removedRestoringSelection = false;
    removedSelectedIds.clear();
    removedSelectedMarkers.clear();
    removedAllSelected = false;
    removedExcludedIds = new Set<string>();
    removedRestoringId = undefined;
    if (trashReview) {
      trashReview = undefined;
      present.closeReview();
    }
    trashOperation = undefined;
    trashOutcomeBusy = undefined;
    present.closePanel();
  };

  const restoreRemovedPhoto = async (
    photoId: string,
    removedAtMs: number,
  ): Promise<void> => {
    const admission = removal.restorePhotos([{ photoId, removedAtMs }]);
    if (!admission) return;
    removedRestoringId = photoId;
    removedMessage = undefined;
    renderRemovedPanel();
    present.controlsChanged();
    const outcome = await admission.settlement;
    removedRestoringId = undefined;
    if (outcome.kind === "detached") return;
    if (outcome.kind === "failed") {
      removedMessage =
        outcome.status === 404
          ? "That Photo is no longer removed from the Library. Reload this listing."
          : "The Photo could not be restored. Retry to continue.";
      renderRemovedPanel();
      present.controlsChanged();
      return;
    }
    removedSelectedIds.delete(photoId);
    removedSelectedMarkers.delete(photoId);
    // The listing reads the Library again before it reports the restore, so a
    // row never outlives the state it presents.
    await coordinate.refreshLibrary();
    await loadRemovedPage(
      removedPage?.start ?? 0,
      coordinate.restorationMessage(outcome.result.counts),
    );
    await coordinate.reopenSourceAfterRestore();
    present.controlsChanged();
  };

  /// Restores the selected Trash items. Restore is the reversible action, so
  /// it stays available beside the listing's primary controls; permanent
  /// deletion is a separately reviewed second action and never the default.
  const restoreSelectedTrash = async (): Promise<void> => {
    if (
      removedPending ||
      removedSelectionPending ||
      removedDeleting ||
      removedRestoringSelection ||
      (!pendingRestore &&
        (removedAllSelected
          ? removedAllCount === 0
          : removedSelectedIds.size === 0))
    )
      return;
    if (removedAllSelected || pendingRestore) {
      const retrying = pendingRestore !== undefined;
      if (!pendingRestore) {
        if (!(await selectAllRemoved(true)) || removedAllCount === 0) return;
        pendingRestore = {
          operationId: randomUuid(),
          excludePhotoIds: [
            ...new Set([...removedExcludedIds, ...removedPendingIds]),
          ],
        };
        storePendingTrashRestore(pendingRestore);
      }
      const intent = pendingRestore;
      removedRestoringSelection = true;
      removedMessage = undefined;
      renderRemovedPanel();
      present.controlsChanged();
      const outcome = await restoreAllTrash(
        fetcher,
        intent.operationId,
        intent.excludePhotoIds,
      );
      removedRestoringSelection = false;
      if (outcome.kind !== "restored") {
        if (
          !retrying &&
          outcome.kind === "rejected" &&
          (outcome.status === 400 || outcome.status === 409)
        ) {
          pendingRestore = undefined;
          storePendingTrashRestore(undefined);
          removedMessage =
            "Restore was not admitted. Narrow the selection or reload Trash and try again.";
          renderRemovedPanel();
          present.controlsChanged();
          return;
        }
        removedMessage =
          "Restore was not confirmed; retry the retained operation to recover its result.";
        renderRemovedPanel();
        present.controlsChanged();
        return;
      }
      pendingRestore = undefined;
      storePendingTrashRestore(undefined);
      removedSelectedIds.clear();
      removedSelectedMarkers.clear();
      removedAllSelected = false;
      removedExcludedIds = new Set<string>();
      const counts = outcome.value.counts;
      const message = `${counts.restored.toLocaleString()} ${counts.restored === 1 ? "Photo" : "Photos"} restored. Restore resolved ${outcome.value.results.length.toLocaleString()} ${outcome.value.results.length === 1 ? "item" : "items"}: ${counts.alreadyActive.toLocaleString()} already active, ${counts.changedElsewhere.toLocaleString()} changed, ${counts.missing.toLocaleString()} unavailable.`;
      await coordinate.refreshLibrary();
      if (removedPanelOpen)
        await loadRemovedPage(removedPage?.start ?? 0, message);
      await coordinate.reopenSourceAfterRestore();
      present.controlsChanged();
      return;
    }
    const markers = [...removedSelectedIds].flatMap((photoId) => {
      const removedAtMs = removedSelectedMarkers.get(photoId);
      return removedAtMs === undefined ? [] : [{ photoId, removedAtMs }];
    });
    if (markers.length !== removedSelectedIds.size) {
      removedMessage =
        "Some selected Trash items need a refresh before they can be restored.";
      renderRemovedPanel();
      present.controlsChanged();
      return;
    }
    const admission = removal.restorePhotos(markers);
    if (!admission) return;
    removedRestoringSelection = true;
    removedMessage = undefined;
    renderRemovedPanel();
    present.controlsChanged();
    const outcome = await admission.settlement;
    removedRestoringSelection = false;
    if (outcome.kind === "detached") return;
    if (outcome.kind === "failed") {
      removedMessage =
        outcome.status === 404
          ? "Some selected Trash items are no longer removed. Reload this listing."
          : "The selected Trash items could not be restored. Retry to continue.";
      renderRemovedPanel();
      present.controlsChanged();
      return;
    }
    removedSelectedIds.clear();
    removedSelectedMarkers.clear();
    removedAllSelected = false;
    removedExcludedIds = new Set<string>();
    await coordinate.refreshLibrary();
    await loadRemovedPage(
      removedPage?.start ?? 0,
      coordinate.restorationMessage(outcome.result.counts),
    );
    await coordinate.reopenSourceAfterRestore();
    present.controlsChanged();
  };

  return {
    openPanel: openRemovedPanel,
    closePanel: closeRemovedPanel,
    render: renderRemovedPanel,
    turnPage: (direction) => {
      if (
        removedPending ||
        removedSelectionPending ||
        removedDeleting ||
        removedPage === undefined
      )
        return;
      const start = removedPage.start + direction * REMOVED_PAGE_LIMIT;
      if (start < 0 || start >= removedPage.total) return;
      void loadRemovedPage(start);
    },
    retry: () => {
      if (removedPending || removedSelectionPending || removedDeleting) return;
      void loadRemovedPage(removedPage?.start ?? 0);
    },
    selectAll: () => {
      void selectAllRemoved();
    },
    clearSelection: clearRemovedSelection,
    toggle: (photoId, selected) => {
      if (
        removedPending ||
        removedSelectionPending ||
        removedDeleting ||
        pendingRestore
      )
        return;
      const item = removedPage?.items.find(
        (candidate) => candidate.photo.id === photoId,
      );
      if (!item) return;
      // An item whose permanent-deletion outcome is not settled is never
      // selected: Restore and a new deletion stay unavailable for it.
      if (selected && item.pendingVerificationOperationId !== null) return;
      if (removedAllSelected) {
        const excluded = new Set(removedExcludedIds);
        if (selected) excluded.delete(photoId);
        else excluded.add(photoId);
        if (selected && removedExcludedIds.has(photoId)) removedAllCount += 1;
        if (!selected && !removedExcludedIds.has(photoId)) removedAllCount -= 1;
        removedExcludedIds = excluded;
        removedMessage = trashExclusionNotice(trashExclusionSelection());
        renderRemovedPanel();
        present.controlsChanged();
        return;
      }
      if (selected) {
        removedSelectedIds.add(photoId);
        removedSelectedMarkers.set(photoId, item.removedAtMs);
      } else {
        removedSelectedIds.delete(photoId);
        removedSelectedMarkers.delete(photoId);
      }
      renderRemovedPanel();
      present.controlsChanged();
    },
    deleteSelection: deleteSelectedTrash,
    restoreSelection: restoreSelectedTrash,
    restorePhoto: (photoId, removedAtMs) => {
      if (pendingRestore) return;
      const item = removedPage?.items.find(
        (candidate) => candidate.photo.id === photoId,
      );
      // An unresolved deletion outcome is never answered with a restore.
      if (
        !item ||
        item.removedAtMs !== removedAtMs ||
        item.pendingVerificationOperationId !== null
      )
        return;
      void restoreRemovedPhoto(photoId, removedAtMs);
    },
    confirmReview: confirmTrashReview,
    cancelReview: cancelTrashReview,
    checkOutcome: checkTrashResult,
    retryDelete: retryTrashDelete,
    checkRow: checkTrashRow,
    resumeRow: resumeTrashRow,
    presentMessage: (message) => {
      removedMessage = message;
      renderRemovedPanel();
      present.controlsChanged();
    },
    reloadAfterUndo: async (message) => {
      if (!removedPanelOpen) return;
      await loadRemovedPage(removedPage?.start ?? 0, message);
    },
    dispose: () => {
      selectionAbort?.abort();
      removedAbort?.abort();
    },
  };
}
