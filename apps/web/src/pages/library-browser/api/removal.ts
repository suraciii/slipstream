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
}>;

export type RestorationWriteResult =
  | Readonly<{ kind: "restored"; value: RestorationResult }>
  | Readonly<{ kind: "rejected"; status: number }>
  | Readonly<{ kind: "malformed" }>;

export type RemovedPhotoItem = Readonly<{
  removedAt: string;
  photo: PhotoSummary;
}>;

export type RemovedPhotosResult =
  | Readonly<{
      kind: "ok";
      start: number;
      limit: number;
      total: number;
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

/// Restores one bounded explicit list of Photos. Every requested Photo yields
/// exactly one outcome, so the counts and the named lists must partition the
/// requested identities.
export async function restoreRemovedPhotos(
  fetcher: RemovalFetch,
  photoIds: ReadonlyArray<string>,
): Promise<RestorationWriteResult> {
  return restorationWrite(fetcher, { photoIds }, photoIds);
}

async function restorationWrite(
  fetcher: RemovalFetch,
  body:
    | Readonly<{ operation: string }>
    | Readonly<{ photoIds: ReadonlyArray<string> }>,
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
    !hasExactKeys(value, ["counts", "changedElsewhere", "missing"]) ||
    !validRestorationCounts(value.counts) ||
    !validIdentityList(value.changedElsewhere) ||
    !validIdentityList(value.missing)
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
    }),
  });
}

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
      value.start !== input.start ||
      value.limit !== input.limit ||
      !validCount(value.total) ||
      !Array.isArray(value.photos) ||
      // A page is complete for its position: a response that omits rows it
      // claims to have cannot be presented as the whole page.
      value.photos.length !==
        Math.min(input.limit, value.total - input.start) ||
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
      photos: Object.freeze(
        photos.map((item) =>
          Object.freeze({ removedAt: item.removedAt, photo: item.photo }),
        ),
      ),
    });
  } catch {
    return Object.freeze({ kind: "failed", malformed: true });
  }
}

const validRemovedPhotoItem = (value: unknown): value is RemovedPhotoItem =>
  isRecord(value) &&
  hasExactKeys(value, ["removedAt", "photo"]) &&
  typeof value.removedAt === "string" &&
  value.removedAt.length > 0 &&
  validPhotoSummary(value.photo);
