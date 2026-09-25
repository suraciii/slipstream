import { validPhotoSummary } from "./source-grid.js";
import { hasExactKeys, isRecord, validCount } from "./guards.js";
import type { PhotoSummary } from "./contracts.js";

export type RemovalFetch = (
  input: string,
  init?: RequestInit,
) => Promise<Response>;

/// One confirmed removal. Removed Photos are reported by count: the operation
/// id, not a Photo list, is what Undo restores, while every Photo that was not
/// newly removed is named.
export type RemovalResult = Readonly<{
  operationId: string;
  counts: Readonly<{
    removed: number;
    changedElsewhere: number;
    missing: number;
    alreadyRemoved: number;
  }>;
  changedElsewhere: ReadonlyArray<string>;
  missing: ReadonlyArray<string>;
  alreadyRemoved: ReadonlyArray<string>;
}>;

export type RemovalWriteResult =
  | Readonly<{ kind: "removed"; value: RemovalResult }>
  | Readonly<{ kind: "rejected"; status: number }>
  | Readonly<{ kind: "malformed" }>;

export type RestorationResult = Readonly<{
  counts: Readonly<{
    restored: number;
    changedElsewhere: number;
    missing: number;
  }>;
  changedElsewhere: ReadonlyArray<string>;
  missing: ReadonlyArray<string>;
  /// What each removal operation the restore touched still owns. An operation
  /// with nothing left is reported with zero, and an operation the restore did
  /// not touch is absent, so a surface holding an Undo for one operation never
  /// claims a count the Library no longer holds.
  operations: ReadonlyArray<Readonly<{ operationId: string; removed: number }>>;
}>;

export type RestorationWriteResult =
  | Readonly<{ kind: "restored"; value: RestorationResult }>
  | Readonly<{ kind: "rejected"; status: number }>
  | Readonly<{ kind: "malformed" }>;

export type RemovedPhotoItem = Readonly<{
  /// The removal marker the listing reported: the millisecond the removal was
  /// confirmed. A restore names it so a listing read before a newer removal
  /// cannot restore a Photo past the marker it was read under.
  removedAtMs: number;
  photo: PhotoSummary;
}>;

/// One removed Photo as a listing showed it. A restore names the identity and
/// the marker that must still be in force.
export type RemovalMarker = Readonly<{
  photoId: string;
  removedAtMs: number;
}>;

export type RemovedPhotosResult =
  | Readonly<{
      kind: "ok";
      start: number;
      limit: number;
      total: number;
      operation: Readonly<{ operationId: string; removed: number }> | undefined;
      photos: ReadonlyArray<RemovedPhotoItem>;
    }>
  | Readonly<{ kind: "failed"; status?: number; malformed?: true }>;

/// Identity lists a removal or restore reports beside its counts. They are
/// disjoint, hold no duplicates, and every entry names one Photo.
const validIdentityList = (value: unknown): value is ReadonlyArray<string> =>
  Array.isArray(value) &&
  value.every((photoId) => typeof photoId === "string" && photoId.length > 0) &&
  new Set(value).size === value.length;

const disjoint = (groups: ReadonlyArray<ReadonlyArray<string>>): boolean => {
  const seen = new Set<string>();
  for (const group of groups)
    for (const photoId of group) {
      if (seen.has(photoId)) return false;
      seen.add(photoId);
    }
  return true;
};

const validRemovalCounts = (value: unknown): value is RemovalResult["counts"] =>
  isRecord(value) &&
  hasExactKeys(value, [
    "removed",
    "changedElsewhere",
    "missing",
    "alreadyRemoved",
  ]) &&
  validCount(value.removed) &&
  validCount(value.changedElsewhere) &&
  validCount(value.missing) &&
  validCount(value.alreadyRemoved);

const validRestorationCounts = (
  value: unknown,
): value is RestorationResult["counts"] =>
  isRecord(value) &&
  hasExactKeys(value, ["restored", "changedElsewhere", "missing"]) &&
  validCount(value.restored) &&
  validCount(value.changedElsewhere) &&
  validCount(value.missing);

/// One confirmed removal of one reviewed result.
///
/// The server resolves the reviewed Snapshot's complete frozen sequence, so
/// the four outcome counts must add up to exactly the count the Photographer
/// reviewed, and every named Photo must belong to the outcome its list names.
/// A response that omits, duplicates, or invents an outcome cannot be trusted
/// to move the Grid's facts or counts.
export async function removeRejectedResult(
  fetcher: RemovalFetch,
  input: Readonly<{
    token: string;
    operationId: string;
    reviewed: number;
  }>,
): Promise<RemovalWriteResult> {
  let response: Response;
  try {
    response = await fetcher("/api/photos/remove", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        token: input.token,
        operationId: input.operationId,
      }),
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
  if (
    !isRecord(value) ||
    !hasExactKeys(value, [
      "operationId",
      "counts",
      "changedElsewhere",
      "missing",
      "alreadyRemoved",
    ]) ||
    value.operationId !== input.operationId ||
    !validRemovalCounts(value.counts) ||
    !validIdentityList(value.changedElsewhere) ||
    !validIdentityList(value.missing) ||
    !validIdentityList(value.alreadyRemoved)
  )
    return Object.freeze({ kind: "malformed" });
  const counts = value.counts;
  if (
    value.changedElsewhere.length !== counts.changedElsewhere ||
    value.missing.length !== counts.missing ||
    value.alreadyRemoved.length !== counts.alreadyRemoved ||
    !disjoint([value.changedElsewhere, value.missing, value.alreadyRemoved]) ||
    counts.removed +
      counts.changedElsewhere +
      counts.missing +
      counts.alreadyRemoved !==
      input.reviewed
  )
    return Object.freeze({ kind: "malformed" });
  return Object.freeze({
    kind: "removed",
    value: Object.freeze({
      operationId: value.operationId,
      counts: Object.freeze({ ...counts }),
      changedElsewhere: Object.freeze([...value.changedElsewhere]),
      missing: Object.freeze([...value.missing]),
      alreadyRemoved: Object.freeze([...value.alreadyRemoved]),
    }),
  });
}

/// Restores every Photo one removal operation still owns. The operation is
/// named, never a Photo list, so the request cannot grow the set it restores.
export async function restoreRemovalOperation(
  fetcher: RemovalFetch,
  operationId: string,
): Promise<RestorationWriteResult> {
  return restorationWrite(fetcher, { operation: operationId }, undefined);
}

/// Restores one bounded explicit list of removed Photos. Every marker names the
/// identity to restore and the removal the listing showed, so a Photo that was
/// removed again since the listing was read is reported as changed elsewhere
/// instead of being restored past its newer removal.
export async function restoreRemovedPhotos(
  fetcher: RemovalFetch,
  markers: ReadonlyArray<RemovalMarker>,
): Promise<RestorationWriteResult> {
  return restorationWrite(
    fetcher,
    {
      photos: markers.map((marker) => ({
        id: marker.photoId,
        removedAtMs: marker.removedAtMs,
      })),
    },
    markers.map((marker) => marker.photoId),
  );
}

async function restorationWrite(
  fetcher: RemovalFetch,
  body:
    | Readonly<{ operation: string }>
    | Readonly<{ photos: ReadonlyArray<{ id: string; removedAtMs: number }> }>,
  requested: ReadonlyArray<string> | undefined,
): Promise<RestorationWriteResult> {
  let response: Response;
  try {
    response = await fetcher("/api/photos/restore", {
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
  if (
    !isRecord(value) ||
    !hasExactKeys(value, [
      "counts",
      "changedElsewhere",
      "missing",
      "operations",
    ]) ||
    !validRestorationCounts(value.counts) ||
    !validIdentityList(value.changedElsewhere) ||
    !validIdentityList(value.missing) ||
    !validRestoredOperations(value.operations)
  )
    return Object.freeze({ kind: "malformed" });
  const counts = value.counts;
  if (
    value.changedElsewhere.length !== counts.changedElsewhere ||
    value.missing.length !== counts.missing ||
    !disjoint([value.changedElsewhere, value.missing]) ||
    (requested !== undefined &&
      (counts.restored + counts.changedElsewhere + counts.missing !==
        requested.length ||
        ![...value.changedElsewhere, ...value.missing].every((photoId) =>
          requested.includes(photoId),
        )))
  )
    return Object.freeze({ kind: "malformed" });
  return Object.freeze({
    kind: "restored",
    value: Object.freeze({
      counts: Object.freeze({ ...counts }),
      changedElsewhere: Object.freeze([...value.changedElsewhere]),
      missing: Object.freeze([...value.missing]),
      operations: Object.freeze(
        value.operations.map((entry) =>
          Object.freeze({
            operationId: entry.operationId,
            removed: entry.removed,
          }),
        ),
      ),
    }),
  });
}

/// The operation ids a restore touched, each naming how many Photos it still
/// owns. Ids are distinct and non-empty and every count is a non-negative
/// integer: zero says the operation owns nothing and may be withdrawn.
/// The newest removal operation that still owns at least one removed Photo.
const validRemovedOperation = (
  value: unknown,
): value is Readonly<{ operationId: string; removed: number }> | null =>
  value === null ||
  (isRecord(value) &&
    hasExactKeys(value, ["operationId", "removed"]) &&
    typeof value.operationId === "string" &&
    value.operationId.length > 0 &&
    validCount(value.removed));

/// The operation ids a restore touched, each naming how many Photos it still
/// owns. Ids are distinct and non-empty and every count is a non-negative
/// integer: zero says the operation owns nothing and may be withdrawn.
const validRestoredOperations = (
  value: unknown,
): value is ReadonlyArray<
  Readonly<{ operationId: string; removed: number }>
> => {
  if (!Array.isArray(value)) return false;
  const operationIds = new Set<string>();
  for (const entry of value) {
    if (
      !isRecord(entry) ||
      !hasExactKeys(entry, ["operationId", "removed"]) ||
      typeof entry.operationId !== "string" ||
      entry.operationId.length === 0 ||
      typeof entry.removed !== "number" ||
      !Number.isInteger(entry.removed) ||
      entry.removed < 0 ||
      operationIds.has(entry.operationId)
    )
      return false;
    operationIds.add(entry.operationId);
  }
  return true;
};

/// One bounded page of removed Photos, newest removal first.
export async function fetchRemovedPhotos(
  fetcher: RemovalFetch,
  input: Readonly<{
    start: number;
    limit: number;
    signal: AbortSignal;
  }>,
): Promise<RemovedPhotosResult> {
  let response: Response;
  try {
    response = await fetcher(
      `/api/photos/removed?start=${input.start}&limit=${input.limit}`,
      { signal: input.signal, priority: "high" },
    );
  } catch {
    return Object.freeze({ kind: "failed" });
  }
  if (!response.ok)
    return Object.freeze({ kind: "failed", status: response.status });
  try {
    const value: unknown = await response.json();
    if (
      !isRecord(value) ||
      !hasExactKeys(value, [
        "start",
        "limit",
        "total",
        "operation",
        "photos",
      ]) ||
      value.start !== input.start ||
      value.limit !== input.limit ||
      !validCount(value.total) ||
      !validRemovedOperation(value.operation) ||
      !Array.isArray(value.photos) ||
      // A page is complete for its position: a response that omits rows it
      // claims to have cannot be presented as the whole page. A stale offset
      // after a restore is a valid empty page, not a malformed response.
      value.photos.length !==
        Math.max(0, Math.min(input.limit, value.total - input.start)) ||
      !value.photos.every(validRemovedPhotoItem)
    )
      return Object.freeze({ kind: "failed", malformed: true });
    const photos = value.photos as ReadonlyArray<RemovedPhotoItem>;
    if (new Set(photos.map((item) => item.photo.id)).size !== photos.length)
      return Object.freeze({ kind: "failed", malformed: true });
    return Object.freeze({
      kind: "ok",
      start: input.start,
      limit: input.limit,
      total: value.total,
      operation:
        value.operation === null
          ? undefined
          : Object.freeze({
              operationId: value.operation.operationId,
              removed: value.operation.removed,
            }),
      photos: Object.freeze(
        photos.map((item) =>
          Object.freeze({ removedAtMs: item.removedAtMs, photo: item.photo }),
        ),
      ),
    });
  } catch {
    return Object.freeze({ kind: "failed", malformed: true });
  }
}

const validRemovedPhotoItem = (value: unknown): value is RemovedPhotoItem =>
  isRecord(value) &&
  hasExactKeys(value, ["removedAtMs", "photo"]) &&
  typeof value.removedAtMs === "number" &&
  Number.isSafeInteger(value.removedAtMs) &&
  value.removedAtMs >= 0 &&
  validPhotoSummary(value.photo);
