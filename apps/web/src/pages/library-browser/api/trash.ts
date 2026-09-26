import { hasExactKeys, isRecord, validCount } from "./guards.js";
import type { RemovalFetch } from "./removal.js";

export type TrashSelection = Readonly<{
  all: boolean;
  photoIds: ReadonlyArray<string>;
  excludePhotoIds: ReadonlyArray<string>;
}>;

export type TrashReviewItem = Readonly<{
  photoId: string;
  removedAtMs: number;
  originalId: string;
  originalLocation: string;
  originalKind: "raw" | "jpeg";
  size: number;
  albums: ReadonlyArray<Readonly<{ id: string; name: string }>>;
}>;

export type TrashReview = Readonly<{
  operationId: string;
  items: ReadonlyArray<TrashReviewItem>;
  rejected: ReadonlyArray<
    Readonly<{
      photoId: string;
      reason: "missing" | "changed-elsewhere" | "pending-verification";
    }>
  >;
}>;

export type TrashItemResult = Readonly<{
  photoId: string;
  state:
    | "pending"
    | "deleting"
    | "deleted"
    | "missing"
    | "changed"
    | "failed"
    | "uncertain";
  /// The relative Original Location and kind the review captured, so an
  /// outcome can name the file it reports without a second listing read.
  originalLocation: string;
  originalKind: "raw" | "jpeg";
  size?: number | null;
  message?: string | null;
}>;

export type TrashOperation = Readonly<{
  operationId: string;
  reviewed: number;
  logicalBytesDeleted: number;
  items: ReadonlyArray<TrashItemResult>;
}>;

export type TrashWriteResult<T> =
  | Readonly<{ kind: "ok"; value: T }>
  | Readonly<{ kind: "rejected"; status: number }>
  | Readonly<{ kind: "malformed" }>;

const validAlbums = (
  value: unknown,
): value is ReadonlyArray<Readonly<{ id: string; name: string }>> =>
  Array.isArray(value) &&
  value.every(
    (album) =>
      isRecord(album) &&
      hasExactKeys(album, ["id", "name"]) &&
      typeof album.id === "string" &&
      album.id.length > 0 &&
      typeof album.name === "string" &&
      album.name.length > 0,
  );

const validReview = (
  value: unknown,
  operationId: string,
): value is TrashReview =>
  isRecord(value) &&
  hasExactKeys(value, ["operationId", "items", "rejected"]) &&
  value.operationId === operationId &&
  Array.isArray(value.items) &&
  value.items.every(
    (item) =>
      isRecord(item) &&
      hasExactKeys(item, [
        "photoId",
        "removedAtMs",
        "originalId",
        "originalLocation",
        "originalKind",
        "size",
        "albums",
      ]) &&
      typeof item.photoId === "string" &&
      item.photoId.length > 0 &&
      typeof item.removedAtMs === "number" &&
      Number.isSafeInteger(item.removedAtMs) &&
      item.removedAtMs >= 0 &&
      typeof item.originalId === "string" &&
      item.originalId.length > 0 &&
      typeof item.originalLocation === "string" &&
      item.originalLocation.length > 0 &&
      (item.originalKind === "raw" || item.originalKind === "jpeg") &&
      typeof item.size === "number" &&
      Number.isSafeInteger(item.size) &&
      item.size >= 0 &&
      validAlbums(item.albums),
  ) &&
  Array.isArray(value.rejected) &&
  value.rejected.every(
    (item) =>
      isRecord(item) &&
      hasExactKeys(item, ["photoId", "reason"]) &&
      typeof item.photoId === "string" &&
      item.photoId.length > 0 &&
      (item.reason === "missing" ||
        item.reason === "changed-elsewhere" ||
        item.reason === "pending-verification"),
  );

const validOperation = (
  value: unknown,
  operationId: string,
): value is TrashOperation =>
  isRecord(value) &&
  hasExactKeys(value, [
    "operationId",
    "reviewed",
    "logicalBytesDeleted",
    "items",
  ]) &&
  value.operationId === operationId &&
  validCount(value.reviewed) &&
  typeof value.logicalBytesDeleted === "number" &&
  Number.isSafeInteger(value.logicalBytesDeleted) &&
  value.logicalBytesDeleted >= 0 &&
  Array.isArray(value.items) &&
  value.items.every(
    (item) =>
      isRecord(item) &&
      hasExactKeys(item, [
        "photoId",
        "state",
        "size",
        "message",
        "originalLocation",
        "originalKind",
      ]) &&
      typeof item.photoId === "string" &&
      item.photoId.length > 0 &&
      typeof item.state === "string" &&
      [
        "pending",
        "deleting",
        "deleted",
        "missing",
        "changed",
        "failed",
        "uncertain",
      ].includes(item.state) &&
      typeof item.originalLocation === "string" &&
      item.originalLocation.length > 0 &&
      (item.originalKind === "raw" || item.originalKind === "jpeg") &&
      (item.size === undefined ||
        item.size === null ||
        (typeof item.size === "number" &&
          Number.isSafeInteger(item.size) &&
          item.size >= 0)) &&
      (item.message === undefined ||
        item.message === null ||
        typeof item.message === "string"),
  );

async function postJson<T>(
  fetcher: RemovalFetch,
  path: string,
  body: unknown,
  validate: (value: unknown) => value is T,
): Promise<TrashWriteResult<T>> {
  let response: Response;
  try {
    response = await fetcher(path, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch {
    return Object.freeze({ kind: "rejected", status: 0 });
  }
  if (!response.ok)
    return Object.freeze({ kind: "rejected", status: response.status });
  let value: unknown;
  try {
    value = await response.json();
  } catch {
    return Object.freeze({ kind: "malformed" });
  }
  return validate(value)
    ? Object.freeze({ kind: "ok", value })
    : Object.freeze({ kind: "malformed" });
}

export function reviewTrash(
  fetcher: RemovalFetch,
  operationId: string,
  selection: TrashSelection,
): Promise<TrashWriteResult<TrashReview>> {
  return postJson(
    fetcher,
    "/api/trash/review",
    // The operation id is browser-supplied: the response must echo it, so
    // the confirmation can name exactly the set this review fixed.
    { ...selection, operationId },
    (value): value is TrashReview => validReview(value, operationId),
  );
}

export function deleteTrash(
  fetcher: RemovalFetch,
  operationId: string,
): Promise<TrashWriteResult<TrashOperation>> {
  return postJson(
    fetcher,
    "/api/trash/delete",
    { operationId },
    (value): value is TrashOperation => validOperation(value, operationId),
  );
}

export async function fetchTrashOperation(
  fetcher: RemovalFetch,
  operationId: string,
): Promise<TrashWriteResult<TrashOperation>> {
  let response: Response;
  try {
    response = await fetcher(
      `/api/trash/operations/${encodeURIComponent(operationId)}`,
    );
  } catch {
    return Object.freeze({ kind: "rejected", status: 0 });
  }
  if (!response.ok)
    return Object.freeze({ kind: "rejected", status: response.status });
  let value: unknown;
  try {
    value = await response.json();
  } catch {
    return Object.freeze({ kind: "malformed" });
  }
  return validOperation(value, operationId)
    ? Object.freeze({ kind: "ok", value })
    : Object.freeze({ kind: "malformed" });
}

/// What a Photographer must read before confirming a permanent deletion, and
/// what the confirmation surface presents for it. Derived from one review so
/// the dialog and its tests share one computation.
export const TRASH_REJECTION_REASONS = {
  missing: "Original missing",
  "changed-elsewhere": "Original changed since it was removed",
  "pending-verification": "Deletion result pending",
} as const;

export type TrashReviewPlan = Readonly<{
  itemCount: number;
  /// The summed logical size of the reviewed Originals. Unknown sizes cannot
  /// reach the plan: a review item always names a non-negative size.
  totalBytes: number;
  albumCount: number;
  albumNames: ReadonlyArray<string>;
  items: ReadonlyArray<
    Readonly<{
      photoId: string;
      originalLocation: string;
      originalKind: "raw" | "jpeg";
      size: number;
      albumNames: ReadonlyArray<string>;
    }>
  >;
  rejected: ReadonlyArray<Readonly<{ photoId: string; reasonLabel: string }>>;
  rejectedCount: number;
  /// False when every selected item was rejected: the rejections are shown
  /// and the surface offers no delete action.
  canConfirm: boolean;
}>;

export const planTrashReview = (review: TrashReview): TrashReviewPlan => {
  const albums = new Map<string, string>();
  for (const item of review.items)
    for (const album of item.albums) albums.set(album.id, album.name);
  return Object.freeze({
    itemCount: review.items.length,
    totalBytes: review.items.reduce((total, item) => total + item.size, 0),
    albumCount: albums.size,
    albumNames: Object.freeze([...albums.values()]),
    items: Object.freeze(
      review.items.map((item) =>
        Object.freeze({
          photoId: item.photoId,
          originalLocation: item.originalLocation,
          originalKind: item.originalKind,
          size: item.size,
          albumNames: Object.freeze(item.albums.map((album) => album.name)),
        }),
      ),
    ),
    rejected: Object.freeze(
      review.rejected.map((item) =>
        Object.freeze({
          photoId: item.photoId,
          reasonLabel: TRASH_REJECTION_REASONS[item.reason],
        }),
      ),
    ),
    rejectedCount: review.rejected.length,
    canConfirm: review.items.length > 0,
  });
};

/// The review's final action label. The count is always written the same way,
/// so the confirmation the Photographer reads is the confirmation the
/// contract names.
export const confirmTrashLabel = (itemCount: number): string =>
  `Permanently delete ${itemCount.toLocaleString()} Original Files`;

/// One delete or fetched operation, partitioned for the outcome surface. The
/// result always reports the five outcome groups, logical bytes for confirmed
/// deletions only, and every item that did not confirm deletion.
export type TrashOutcomePartition = Readonly<{
  deleted: number;
  changed: number;
  missing: number;
  failed: number;
  /// The items whose outcome is not settled yet: `pending`, `deleting`, and
  /// `uncertain` states. They never present as success or failure.
  pendingVerification: number;
  /// Logical bytes the server credited to confirmed deletions only. It is not
  /// measured free space.
  logicalBytesDeleted: number;
  /// Every item that did not confirm deletion, in operation order.
  unresolved: ReadonlyArray<
    Readonly<{
      photoId: string;
      state:
        | "pending"
        | "deleting"
        | "missing"
        | "changed"
        | "failed"
        | "uncertain";
      originalLocation: string;
      originalKind: "raw" | "jpeg";
      size: number | null;
      message?: string;
    }>
  >;
  /// True when no item is left pending verification.
  settled: boolean;
}>;

export const partitionTrashOperation = (
  operation: TrashOperation,
): TrashOutcomePartition => {
  const counts = {
    deleted: 0,
    changed: 0,
    missing: 0,
    failed: 0,
    pendingVerification: 0,
  };
  const unresolved: {
    photoId: string;
    state:
      | "pending"
      | "deleting"
      | "missing"
      | "changed"
      | "failed"
      | "uncertain";
    originalLocation: string;
    originalKind: "raw" | "jpeg";
    size: number | null;
    message?: string;
  }[] = [];
  for (const item of operation.items) {
    if (item.state === "deleted") {
      counts.deleted += 1;
      continue;
    }
    if (item.state === "changed") counts.changed += 1;
    else if (item.state === "missing") counts.missing += 1;
    else if (item.state === "failed") counts.failed += 1;
    else counts.pendingVerification += 1;
    unresolved.push({
      photoId: item.photoId,
      state: item.state,
      originalLocation: item.originalLocation,
      originalKind: item.originalKind,
      size: item.size ?? null,
      ...(item.message ? { message: item.message } : {}),
    });
  }
  return Object.freeze({
    ...Object.freeze(counts),
    logicalBytesDeleted: operation.logicalBytesDeleted,
    unresolved: Object.freeze(
      unresolved.map((item) => Object.freeze(item)),
    ) as TrashOutcomePartition["unresolved"],
    settled: counts.pendingVerification === 0,
  });
};

/// One Trash listing item reduced to what a bounded selection captures.
export type TrashListingSelectionItem = Readonly<{
  photoId: string;
  removedAtMs: number;
  pendingVerificationOperationId: string | null;
}>;

export type BoundedTrashSelection = Readonly<{
  /// The captured selection in listing order, newest removal first: the
  /// deterministic prefix a bound keeps.
  markers: ReadonlyArray<Readonly<{ photoId: string; removedAtMs: number }>>;
  selectedCount: number;
  /// True when eligible Trash items exist beyond the captured prefix.
  bounded: boolean;
  skippedPending: number;
}>;

/// Select all keeps at most `reviewMaximum` items: the newest ones, in the
/// listing's order. Items whose permanent-deletion outcome is not settled are
/// never selected. `total` is the complete Trash size the capture ran
/// against, which is larger than `items` when the capture stopped at the
/// bound before reading the rest.
export const boundTrashSelection = (
  items: ReadonlyArray<TrashListingSelectionItem>,
  total: number,
  reviewMaximum: number,
): BoundedTrashSelection => {
  const eligible = items.filter(
    (item) => item.pendingVerificationOperationId === null,
  );
  const markers = eligible
    .slice(0, reviewMaximum)
    .map((item) =>
      Object.freeze({ photoId: item.photoId, removedAtMs: item.removedAtMs }),
    );
  return Object.freeze({
    markers,
    selectedCount: markers.length,
    bounded: total > items.length || eligible.length > markers.length,
    skippedPending: items.length - eligible.length,
  });
};

/// The truthful select-all report: it names a bounded capture and the part
/// of the selection that was skipped, and stays silent when the capture was
/// complete and nothing was skipped.
export const trashSelectAllNotice = (
  selection: BoundedTrashSelection,
  total: number,
  reviewMaximum: number,
): string | undefined => {
  const parts: string[] = [];
  if (selection.bounded)
    parts.push(
      `Selected the newest ${selection.selectedCount.toLocaleString()} of ${total.toLocaleString()} Trash items; a review captures at most ${reviewMaximum.toLocaleString()}. Review this batch, then select the rest.`,
    );
  if (selection.skippedPending > 0)
    parts.push(
      `${selection.skippedPending.toLocaleString()} ${
        selection.skippedPending === 1 ? "item" : "items"
      } pending verification ${selection.skippedPending === 1 ? "was" : "were"} not selected.`,
    );
  return parts.length === 0 ? undefined : parts.join(" ");
};
