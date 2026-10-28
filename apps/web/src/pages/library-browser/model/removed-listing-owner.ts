import type { RemovalFetch, RestorationResult } from "../api/removal.js";
import { fetchPhotoAlbums } from "../api/photo.js";
import {
  cleanupTarget,
  cleanupTrash,
  fetchTrashCleanupOperation,
  fetchTrashPage,
  previewTrashCleanup,
  type TrashCleanupOperation,
  type TrashListingItem,
  type TrashListingPage,
} from "../api/trash.js";
import { randomUuid } from "./browser-crypto.js";
import type { RemovalOwner } from "./removal-owner.js";
import type {
  RemovedPanelViewModel,
  TrashReviewViewModel,
} from "./removed-listing-contract.js";
import {
  removedPanelModel,
  trashReviewModel,
  type TrashOperationState,
  type TrashReviewState,
} from "./removed-listing-view-model.js";
import {
  clearPendingTrashCleanup,
  readPendingTrashCleanups,
  storePendingTrashCleanup,
} from "./trash-cleanup-storage.js";

export type RemovedListingPresentation = Readonly<{
  openPanel: (model: RemovedPanelViewModel) => void;
  renderPanel: (model: RemovedPanelViewModel) => void;
  closePanel: () => void;
  openReview: (model: TrashReviewViewModel) => void;
  renderReview: (model: TrashReviewViewModel) => void;
  closeReview: () => void;
  controlsChanged: () => void;
}>;
export type RemovedListingCoordination = Readonly<{
  refreshLibrary: () => Promise<void>;
  reopenSourceAfterRestore: () => Promise<void>;
  restorationMessage: (counts: RestorationResult["counts"]) => string;
}>;
export type RemovedListingOwnerOptions = Readonly<{
  removal: RemovalOwner;
  present: RemovedListingPresentation;
  coordinate: RemovedListingCoordination;
}>;
export interface RemovedListingOwner {
  openPanel(): void;
  closePanel(): void;
  render(): void;
  turnPage(direction: -1 | 1): void;
  retry(): void;
  selectAll(): void;
  clearSelection(): void;
  toggle(photoId: string, selected: boolean): void;
  cleanupSelection(): Promise<void>;
  copyLocations(): Promise<void>;
  restoreSelection(): Promise<void>;
  restorePhoto(photoId: string, removedAtMs: number): void;
  confirmReview(): Promise<void>;
  cancelReview(): void;
  checkOutcome(): Promise<void>;
  presentMessage(message: string): void;
  reloadAfterUndo(message: string): Promise<void>;
  dispose(): void;
}

export function createRemovedListingOwner(
  fetcher: RemovalFetch,
  { removal, present, coordinate }: RemovedListingOwnerOptions,
): RemovedListingOwner {
  const limit = 50;
  let pages: TrashListingPage[] = [];
  let pageIndex = 0;
  let panelOpen = false;
  let pending = false;
  let failed = false;
  let cleaning = false;
  let restoringSelection = false;
  let restoringId: string | undefined;
  let message: string | undefined;
  let selected = new Map<string, TrashListingItem>();
  let allSelected = false;
  let excluded = new Set<string>();
  let review: TrashReviewState | undefined;
  let operation: TrashOperationState | undefined;
  const unresolved = new Set<string>();
  let outcomeBusy = false;
  let readAbort: AbortController | undefined;
  let selectionAbort: AbortController | undefined;
  let failedCursor: string | undefined;

  const model = () =>
    removedPanelModel({
      page: pages[pageIndex],
      pageIndex,
      pending,
      failed,
      cleaning,
      selected,
      allSelected,
      excluded,
      restoringSelection,
      restoringId,
      message,
      operation,
      outcomeBusy,
      pageLimit: limit,
    });
  const render = () => {
    if (panelOpen) present.renderPanel(model());
    present.controlsChanged();
  };
  const busy = () =>
    pending || cleaning || restoringSelection || restoringId !== undefined;
  const resetSelection = () => {
    selected = new Map();
    allSelected = false;
    excluded = new Set();
  };

  const load = async (
    cursor?: string,
    successMessage?: string,
  ): Promise<void> => {
    readAbort?.abort();
    const controller = new AbortController();
    readAbort = controller;
    pending = true;
    failed = false;
    message = undefined;
    render();
    const result = await fetchTrashPage(
      fetcher,
      cursor
        ? { cursor, signal: controller.signal }
        : { start: 0, limit, signal: controller.signal },
    );
    if (controller.signal.aborted || readAbort !== controller) return;
    readAbort = undefined;
    pending = false;
    if (result.kind === "failed") {
      failed = true;
      failedCursor = cursor;
      message = cursor
        ? "Trash continuation expired or could not be read. Retry, or reopen Trash for a fresh listing."
        : "Trash could not be loaded. Retry to continue.";
    } else {
      const page = result.value;
      const prior = pages[pages.length - 1];
      if (
        cursor &&
        (!prior ||
          prior.nextCursor !== cursor ||
          page.start !== prior.start + prior.photos.length ||
          page.total !== prior.total ||
          page.evaluatedAt !== prior.evaluatedAt ||
          page.libraryRoot !== prior.libraryRoot ||
          page.cleanupMaximum !== prior.cleanupMaximum ||
          page.restoreMaximum !== prior.restoreMaximum ||
          page.photos.some((item) =>
            pages.some((p) =>
              p.photos.some((old) => old.photoId === item.photoId),
            ),
          ))
      ) {
        failed = true;
        failedCursor = cursor;
        message =
          "Trash continuation did not match the captured listing. Reopen Trash and try again.";
      } else {
        if (cursor) {
          pages.push(page);
          pageIndex = pages.length - 1;
        } else {
          pages = [page];
          pageIndex = 0;
        }
        removal.rememberOperation(page.operation ?? undefined);
        message = successMessage;
        failedCursor = undefined;
      }
    }
    render();
  };

  const selectAll = async () => {
    if (busy() || !pages.length) return;
    selectionAbort?.abort();
    const controller = new AbortController();
    selectionAbort = controller;
    pending = true;
    message = undefined;
    render();
    const captured = [...pages];
    const ids = new Set(
      captured.flatMap((page) => page.photos.map((item) => item.photoId)),
    );
    let ok = true;
    while (captured[captured.length - 1]!.nextCursor !== null) {
      const previous = captured[captured.length - 1]!;
      const result = await fetchTrashPage(fetcher, {
        cursor: previous.nextCursor!,
        signal: controller.signal,
      });
      if (controller.signal.aborted || selectionAbort !== controller) return;
      if (result.kind !== "ok") {
        ok = false;
        break;
      }
      const next = result.value;
      if (
        next.start !== previous.start + previous.photos.length ||
        next.total !== previous.total ||
        next.evaluatedAt !== previous.evaluatedAt ||
        next.libraryRoot !== previous.libraryRoot ||
        next.cleanupMaximum !== previous.cleanupMaximum ||
        next.restoreMaximum !== previous.restoreMaximum ||
        next.photos.some((item) => ids.has(item.photoId))
      ) {
        ok = false;
        break;
      }
      next.photos.forEach((item) => ids.add(item.photoId));
      captured.push(next);
    }
    selectionAbort = undefined;
    pending = false;
    if (ok && ids.size === captured[0]!.total) {
      pages = captured;
      selected = new Map(
        captured.flatMap((page) =>
          page.photos.map((item) => [item.photoId, item] as const),
        ),
      );
      allSelected = true;
      excluded = new Set();
    } else
      message =
        "Trash continuation expired or could not be read. Reopen Trash and try again.";
    render();
  };

  const openReview = async () => {
    const page = pages[pageIndex];
    if (
      busy() ||
      review ||
      !page ||
      selected.size === 0 ||
      selected.size > page.cleanupMaximum ||
      (operation && !operation.result)
    )
      return;
    const items = [...selected.values()];
    const controller = new AbortController();
    selectionAbort = controller;
    pending = true;
    render();
    const albums = new Map<string, ReadonlyArray<string> | null>();
    let index = 0;
    const worker = async () => {
      while (index < items.length && !controller.signal.aborted) {
        const item = items[index++]!;
        const result = await fetchPhotoAlbums(
          fetcher,
          item.photoId,
          controller.signal,
        );
        albums.set(
          item.photoId,
          result.kind === "ok"
            ? result.value.albums.map((album) => album.name)
            : null,
        );
      }
    };
    await Promise.all(
      Array.from({ length: Math.min(8, items.length) }, worker),
    );
    if (controller.signal.aborted || selectionAbort !== controller) return;
    const preview = await previewTrashCleanup(
      fetcher,
      items.map(cleanupTarget),
      controller.signal,
    );
    if (controller.signal.aborted || selectionAbort !== controller) return;
    selectionAbort = undefined;
    pending = false;
    if (preview.kind !== "ok") {
      message =
        "Current absence evidence could not be read. Reopen Trash and try again.";
      render();
      return;
    }
    review = {
      operationId: randomUuid(),
      items,
      albums,
      preview: new Map(
        preview.value.items.map((item) => [item.photoId, item] as const),
      ),
      cleaning: false,
    };
    render();
    present.openReview(trashReviewModel(review));
  };

  const cancelReview = () => {
    if (!review) return;
    review = undefined;
    present.closeReview();
    render();
  };

  const settle = async (result: TrashCleanupOperation) => {
    unresolved.delete(result.operationId);
    operation = { operationId: result.operationId, result };
    resetSelection();
    await coordinate.refreshLibrary();
    if (panelOpen) await load(undefined);
    await coordinate.reopenSourceAfterRestore();
    clearPendingTrashCleanup(result.operationId);
    render();
  };

  const confirmReview = async () => {
    const current = review;
    if (!current || current.cleaning || cleaning) return;
    if (
      !storePendingTrashCleanup(
        current.operationId,
        current.items.map(cleanupTarget),
      )
    ) {
      message =
        "Cleanup cannot be sent because its operation identity could not be saved for recovery.";
      cancelReview();
      return;
    }
    cleaning = true;
    review = { ...current, cleaning: true };
    present.renderReview(trashReviewModel(review));
    render();
    const result = await cleanupTrash(
      fetcher,
      current.operationId,
      current.items.map(cleanupTarget),
    );
    cleaning = false;
    review = undefined;
    present.closeReview();
    if (result.kind === "ok") await settle(result.value);
    else if (
      result.kind === "rejected" &&
      result.status >= 400 &&
      result.status < 500
    ) {
      // The service answered before admitting the mutation, so no receipt can
      // exist. Release the attempt identity and let the Photographer retry.
      clearPendingTrashCleanup(current.operationId);
      operation = undefined;
      message =
        "Cleanup was refused before it ran. Adjust the selection or retry.";
      render();
    } else {
      operation = { operationId: current.operationId };
      message =
        "Cleanup outcome is unconfirmed. Check the retained result before another cleanup.";
      render();
    }
  };

  const checkOutcome = async () => {
    const current = operation;
    if (!current || current.result || outcomeBusy || cleaning) return;
    outcomeBusy = true;
    render();
    const result = await fetchTrashCleanupOperation(
      fetcher,
      current.operationId,
    );
    outcomeBusy = false;
    if (result.kind === "ok") {
      await settle(result.value);
      if (unresolved.size > 0) {
        operation = { operationId: [...unresolved][0]! };
        message =
          "Another earlier cleanup outcome is unconfirmed. Retain its result before another cleanup.";
        render();
      }
    } else if (
      result.kind === "rejected" &&
      (result.status === 404 || result.status === 409)
    ) {
      const pending = readPendingTrashCleanups().find(
        (entry) => entry.operationId === current.operationId,
      );
      if (pending) {
        const resent = await cleanupTrash(
          fetcher,
          current.operationId,
          pending.items,
        );
        if (resent.kind === "ok") {
          await settle(resent.value);
          if (unresolved.size > 0) {
            operation = { operationId: [...unresolved][0]! };
            message =
              "Another earlier cleanup outcome is unconfirmed. Retain its result before another cleanup.";
            render();
          }
          return;
        }
      }
      message =
        "The cleanup result is not available. Its outcome remains unconfirmed.";
      render();
    } else {
      message =
        "The cleanup result is not available. Its outcome remains unconfirmed.";
      render();
    }
  };

  const restore = async (
    items: ReadonlyArray<TrashListingItem>,
    single: boolean,
  ) => {
    if (
      busy() ||
      items.length === 0 ||
      items.length > (pages[pageIndex]?.restoreMaximum ?? 0)
    )
      return;
    const admission = removal.restorePhotos(
      items.map(({ photoId, removedAtMs }) => ({ photoId, removedAtMs })),
    );
    if (!admission) return;
    if (single) restoringId = items[0]!.photoId;
    else restoringSelection = true;
    message = undefined;
    render();
    const outcome = await admission.settlement;
    restoringId = undefined;
    restoringSelection = false;
    if (outcome.kind === "detached") return;
    if (outcome.kind === "failed") {
      message =
        "Restore was not confirmed. Refresh Trash to inspect the current records before trying again.";
      render();
      return;
    }
    resetSelection();
    await coordinate.refreshLibrary();
    if (panelOpen)
      await load(
        undefined,
        coordinate.restorationMessage(outcome.result.counts),
      );
    await coordinate.reopenSourceAfterRestore();
    render();
  };

  const recoverPending = async () => {
    const pending = readPendingTrashCleanups();
    const entry = pending[0];
    if (!entry) return;
    cleaning = true;
    render();
    const receipt = await fetchTrashCleanupOperation(
      fetcher,
      entry.operationId,
    );
    if (receipt.kind === "ok") {
      await settle(receipt.value);
      cleaning = false;
      render();
      return;
    }
    if (
      receipt.kind === "rejected" &&
      (receipt.status === 404 || receipt.status === 409)
    ) {
      // No receipt exists, so the exact confirmed intent can be sent again
      // under the retained identity. The service replays it if it did arrive.
      const resent = await cleanupTrash(
        fetcher,
        entry.operationId,
        entry.items,
      );
      if (resent.kind === "ok") {
        await settle(resent.value);
        cleaning = false;
        render();
        return;
      }
      if (
        resent.kind === "rejected" &&
        resent.status >= 400 &&
        resent.status < 500 &&
        resent.status !== 401 &&
        resent.status !== 403
      ) {
        clearPendingTrashCleanup(entry.operationId);
        message =
          "An earlier cleanup was refused before it ran. Reopen Trash to try again.";
        cleaning = false;
        render();
        return;
      }
    }
    // Leave the other stored attempts untouched. They are recovered in
    // subsequent visits instead of overwriting this outcome.
    unresolved.add(entry.operationId);
    operation = { operationId: entry.operationId };
    message =
      "An earlier cleanup outcome is unconfirmed. Retain its result before another cleanup.";
    cleaning = false;
    render();
  };

  const owner: RemovedListingOwner = {
    openPanel: () => {
      panelOpen = true;
      pages = [];
      pageIndex = 0;
      failed = false;
      message = undefined;
      resetSelection();
      review = undefined;
      present.openPanel(model());
      void load();
    },
    closePanel: () => {
      panelOpen = false;
      readAbort?.abort();
      selectionAbort?.abort();
      readAbort = undefined;
      selectionAbort = undefined;
      pending = false;
      resetSelection();
      if (review) {
        review = undefined;
        present.closeReview();
      }
      present.closePanel();
    },
    render,
    turnPage: (direction) => {
      if (busy() || !pages.length) return;
      const target = pageIndex + direction;
      if (target < 0) return;
      if (target < pages.length) {
        pageIndex = target;
        render();
        return;
      }
      const cursor = pages[pageIndex]?.nextCursor;
      if (cursor) void load(cursor);
    },
    retry: () => {
      if (!busy()) void load(failedCursor);
    },
    selectAll: () => {
      void selectAll();
    },
    clearSelection: () => {
      if (!busy()) {
        resetSelection();
        message = undefined;
        render();
      }
    },
    toggle: (photoId, include) => {
      if (busy()) return;
      const item = pages[pageIndex]?.photos.find(
        (entry) => entry.photoId === photoId,
      );
      if (!item) return;
      if (include) {
        selected.set(photoId, item);
        excluded.delete(photoId);
      } else {
        selected.delete(photoId);
        if (allSelected) excluded.add(photoId);
      }
      render();
    },
    cleanupSelection: openReview,
    copyLocations: async () => {
      const page = pages[pageIndex];
      if (!page || selected.size === 0) return;
      const lines = [
        `Library Folder: ${JSON.stringify(page.libraryRoot)}`,
        `Namespace: ${page.namespace}`,
        `Captured: ${page.evaluatedAt}`,
        `Selected: ${selected.size} of ${page.total} captured Trash records`,
        `Cleanup bound: ${page.cleanupMaximum} records per attempt`,
        `Restore bound: ${page.restoreMaximum} Photos per attempt`,
        `Capture continuation expires: ${pages[0]?.expiresAt ?? "none"}`,
        ...[...selected.values()].map((item) =>
          JSON.stringify({
            photoId: item.photoId,
            removedAtMs: item.removedAtMs,
            originalId: item.originalId,
            originalLocation: item.originalLocation,
            originalKind: item.originalKind,
          }),
        ),
      ];
      try {
        await navigator.clipboard.writeText(lines.join("\n"));
        message = "Selected file locations copied.";
      } catch {
        message = "File locations could not be copied.";
      }
      render();
    },
    restoreSelection: () => restore([...selected.values()], false),
    restorePhoto: (photoId, marker) => {
      const item = pages[pageIndex]?.photos.find(
        (entry) => entry.photoId === photoId && entry.removedAtMs === marker,
      );
      if (item) void restore([item], true);
    },
    confirmReview,
    cancelReview,
    checkOutcome,
    presentMessage: (next) => {
      message = next;
      render();
    },
    reloadAfterUndo: async (next) => {
      if (panelOpen) {
        resetSelection();
        await load(undefined, next);
      }
    },
    dispose: () => {
      panelOpen = false;
      readAbort?.abort();
      selectionAbort?.abort();
    },
  };
  void Promise.resolve().then(recoverPending);
  return owner;
}
