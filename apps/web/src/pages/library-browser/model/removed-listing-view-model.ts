import type {
  TrashCleanupOperation,
  TrashCleanupPreviewItem,
  TrashListingItem,
  TrashListingPage,
} from "../api/trash.js";
import type {
  RemovedPanelViewModel,
  TrashReviewViewModel,
} from "./removed-listing-contract.js";

export type RemovedPageState = TrashListingPage;
export type TrashReviewState = Readonly<{
  operationId: string;
  items: ReadonlyArray<TrashListingItem>;
  albums: ReadonlyMap<string, ReadonlyArray<string> | null>;
  preview?: ReadonlyMap<string, TrashCleanupPreviewItem>;
  cleaning: boolean;
}>;
export type TrashOperationState = Readonly<{
  operationId: string;
  result?: TrashCleanupOperation;
}>;

type RemovedPanelState = Readonly<{
  page: RemovedPageState | undefined;
  pageIndex: number;
  pending: boolean;
  failed: boolean;
  cleaning: boolean;
  selected: ReadonlyMap<string, TrashListingItem>;
  allSelected: boolean;
  excluded: ReadonlySet<string>;
  restoringSelection: boolean;
  restoringId: string | undefined;
  message: string | undefined;
  operation: TrashOperationState | undefined;
  outcomeBusy: boolean;
  pageLimit: number;
}>;

export function trashReviewModel(
  review: TrashReviewState | undefined,
): TrashReviewViewModel {
  const items = review?.items ?? [];
  const albumNames = new Set<string>();
  const projected = items.map((item) => {
    const albums = review?.albums.get(item.photoId) ?? null;
    albums?.forEach((name) => albumNames.add(name));
    const preview = review?.preview?.get(item.photoId);
    return {
      photoId: item.photoId,
      originalLocation: item.originalLocation,
      originalKind: item.originalKind,
      originalSize: item.originalSize,
      originalAvailable: item.originalAvailable,
      state: item.state,
      previewState: preview?.state,
      absence: preview?.absence,
      blockers: preview?.blockers ?? [],
      message: preview?.message ?? null,
      albumNames: albums,
    };
  });
  const albumsComplete = items.every((item) => {
    const albums = review?.albums.get(item.photoId);
    return albums !== undefined && albums !== null;
  });
  const previewComplete =
    items.length > 0 &&
    review?.preview !== undefined &&
    items.every((item) => review.preview?.has(item.photoId));
  return {
    itemCount: items.length,
    albumNames: [...albumNames],
    items: projected,
    canConfirm:
      items.length > 0 &&
      albumsComplete &&
      previewComplete &&
      !review?.cleaning,
    confirmLabel: `Clean up ${items.length.toLocaleString()} Photo records`,
    cleaning: review?.cleaning ?? false,
  };
}

export function removedPanelModel(
  state: RemovedPanelState,
): RemovedPanelViewModel {
  const {
    page,
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
    pageLimit,
    pageIndex,
  } = state;
  const status = pending
    ? "loading"
    : failed
      ? "failed"
      : page
        ? "ready"
        : "loading";
  const cleanupBounded =
    page !== undefined && selected.size <= page.cleanupMaximum;
  const restoreBounded =
    page !== undefined && selected.size <= page.restoreMaximum;
  const busy =
    pending || cleaning || restoringSelection || restoringId !== undefined;
  const notices = [message];
  if (selected.size > (page?.cleanupMaximum ?? Infinity))
    notices.push(
      `Select at most ${page?.cleanupMaximum.toLocaleString()} Photos for one cleanup.`,
    );
  if (selected.size > (page?.restoreMaximum ?? Infinity))
    notices.push(
      `Select at most ${page?.restoreMaximum.toLocaleString()} Photos for one Restore.`,
    );
  return {
    status,
    start: page?.start ?? 0,
    total: page?.total ?? 0,
    limit: pageLimit,
    ...(page
      ? {
          libraryRoot: page.libraryRoot,
          namespace: page.namespace,
          evaluatedAt: page.evaluatedAt,
          expiresAt: page.expiresAt,
        }
      : {}),
    hasPrevious: pageIndex > 0,
    hasNext: page?.nextCursor !== null && page !== undefined,
    pending,
    cleaning,
    selectionCount: selected.size,
    allSelected,
    canCleanup:
      selected.size > 0 &&
      cleanupBounded &&
      !busy &&
      (!operation || operation.result !== undefined),
    canCopy: selected.size > 0 && !pending,
    canRestore: selected.size > 0 && restoreBounded && !busy,
    canRetry: failed,
    restoringSelection,
    ...(restoringId ? { restoringPhotoId: restoringId } : {}),
    ...(notices.filter(Boolean).length
      ? { message: notices.filter(Boolean).join(" ") }
      : {}),
    ...(status === "ready" && page?.operation
      ? { undo: { removed: page.operation.removed } }
      : {}),
    ...(outcomeBusy ? { outcomeBusy: "check" as const } : {}),
    ...(operation
      ? {
          outcome: operation.result
            ? { counts: operation.result.counts, items: operation.result.items }
            : { unconfirmed: true as const },
        }
      : {}),
    items: (page?.photos ?? []).map((item) => ({
      photoId: item.photoId,
      filename:
        item.photo?.originalFilename ??
        item.originalLocation.split("/").at(-1) ??
        item.photoId,
      originalLocation: item.originalLocation,
      originalKind: item.originalKind,
      originalSize: item.originalSize,
      originalAvailable: item.originalAvailable,
      state: item.state,
      removedAtMs: item.removedAtMs,
      selected: selected.has(item.photoId),
      excluded: allSelected && excluded.has(item.photoId),
      preview: item.photo?.preview ?? { state: "unavailable" as const },
    })),
  };
}
