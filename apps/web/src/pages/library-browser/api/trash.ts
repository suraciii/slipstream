import type { PhotoSummary } from "./contracts.js";
import { hasExactKeys, isRecord } from "./guards.js";
import { fetchJson } from "./json-response.js";
import type { RemovalFetch } from "./removal.js";
import { validPhotoSummary } from "./source-grid.js";

export type TrashCleanupTarget = Readonly<{
  photoId: string;
  removedAtMs: number;
  originalId: string;
  originalLocation: string;
}>;

export type TrashListingItem = TrashCleanupTarget &
  Readonly<{
    originalKind: "raw" | "jpeg";
    originalSize: number | null;
    originalAvailable: boolean;
    state: "captured" | "changed" | "unavailable";
    photo: PhotoSummary | null;
  }>;

export type TrashListingPage = Readonly<{
  start: number;
  limit: number;
  total: number;
  cleanupMaximum: number;
  restoreMaximum: number;
  evaluatedAt: string;
  expiresAt: string | null;
  nextCursor: string | null;
  libraryRoot: string;
  namespace: "service";
  operation: Readonly<{ operationId: string; removed: number }> | null;
  photos: ReadonlyArray<TrashListingItem>;
}>;

export type TrashBlocker = Readonly<{
  kind: string;
  name: string;
  action: string;
}>;
export type TrashCleanupItemState =
  | "cleaned"
  | "changed"
  | "present"
  | "blocked"
  | "unavailable"
  | "inspection-failed";
export type TrashCleanupItemResult = Readonly<{
  photoId: string;
  originalLocation: string;
  originalKind: "raw" | "jpeg";
  state: TrashCleanupItemState;
  message: string | null;
  blockers: ReadonlyArray<TrashBlocker>;
}>;
export type TrashCleanupOperation = Readonly<{
  operationId: string;
  reviewed: number;
  counts: Readonly<{
    cleaned: number;
    changed: number;
    present: number;
    blocked: number;
    unavailable: number;
    inspectionFailed: number;
  }>;
  items: ReadonlyArray<TrashCleanupItemResult>;
}>;

export type TrashCleanupPreviewItem = Readonly<{
  photoId: string;
  originalLocation: string;
  originalKind: "raw" | "jpeg";
  state: "captured" | "changed" | "unavailable";
  absence: "absent" | "present" | "uninspectable";
  message: string | null;
  blockers: ReadonlyArray<TrashBlocker>;
}>;
export type TrashCleanupPreview = Readonly<{
  items: ReadonlyArray<TrashCleanupPreviewItem>;
}>;
export type TrashWriteResult<T> =
  | Readonly<{ kind: "ok"; value: T }>
  | Readonly<{ kind: "rejected"; status: number }>
  | Readonly<{ kind: "malformed" }>;
export type TrashPageResult =
  | Readonly<{ kind: "ok"; value: TrashListingPage }>
  | Readonly<{ kind: "failed"; status?: number; malformed?: true }>;

const nonempty = (value: unknown): value is string =>
  typeof value === "string" && value.length > 0;
const count = (value: unknown): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const kind = (value: unknown): value is "raw" | "jpeg" =>
  value === "raw" || value === "jpeg";
const cursor = (value: unknown): value is string | null =>
  value === null || nonempty(value);

const validListingItem = (value: unknown): value is TrashListingItem =>
  isRecord(value) &&
  hasExactKeys(value, [
    "photoId",
    "removedAtMs",
    "originalId",
    "originalLocation",
    "originalKind",
    "originalSize",
    "originalAvailable",
    "state",
    "photo",
  ]) &&
  nonempty(value.photoId) &&
  count(value.removedAtMs) &&
  nonempty(value.originalId) &&
  nonempty(value.originalLocation) &&
  kind(value.originalKind) &&
  (value.originalSize === null || count(value.originalSize)) &&
  typeof value.originalAvailable === "boolean" &&
  (value.state === "captured" ||
    value.state === "changed" ||
    value.state === "unavailable") &&
  (value.photo === null ||
    (validPhotoSummary(value.photo) && value.photo.id === value.photoId));

const validListingPage = (value: unknown): value is TrashListingPage => {
  if (
    !isRecord(value) ||
    !hasExactKeys(value, [
      "start",
      "limit",
      "total",
      "cleanupMaximum",
      "restoreMaximum",
      "evaluatedAt",
      "expiresAt",
      "nextCursor",
      "libraryRoot",
      "namespace",
      "operation",
      "photos",
    ]) ||
    !count(value.start) ||
    !count(value.limit) ||
    value.limit === 0 ||
    !count(value.total) ||
    !count(value.cleanupMaximum) ||
    value.cleanupMaximum === 0 ||
    !count(value.restoreMaximum) ||
    value.restoreMaximum === 0 ||
    !nonempty(value.evaluatedAt) ||
    !cursor(value.expiresAt) ||
    !cursor(value.nextCursor) ||
    (value.expiresAt === null) !== (value.nextCursor === null) ||
    !nonempty(value.libraryRoot) ||
    value.namespace !== "service" ||
    !(
      value.operation === null ||
      (isRecord(value.operation) &&
        hasExactKeys(value.operation, ["operationId", "removed"]) &&
        nonempty(value.operation.operationId) &&
        count(value.operation.removed))
    ) ||
    !Array.isArray(value.photos) ||
    !value.photos.every(validListingItem) ||
    value.photos.length !==
      Math.max(0, Math.min(value.limit, value.total - value.start)) ||
    (value.nextCursor !== null) !==
      value.start + value.photos.length < value.total
  )
    return false;
  return (
    new Set(value.photos.map((item) => item.photoId)).size ===
    value.photos.length
  );
};

/** Continue with the returned cursor; a new offset request captures a new listing. */
export async function fetchTrashPage(
  fetcher: RemovalFetch,
  input:
    | Readonly<{ start: number; limit: number; signal?: AbortSignal }>
    | Readonly<{ cursor: string; signal?: AbortSignal }>,
): Promise<TrashPageResult> {
  const query =
    "cursor" in input
      ? `cursor=${encodeURIComponent(input.cursor)}`
      : `start=${input.start}&limit=${input.limit}`;
  const result = await fetchJson(() =>
    fetcher(`/api/trash?${query}`, {
      ...(input.signal === undefined ? {} : { signal: input.signal }),
      priority: "high",
    }),
  );
  if (result.kind === "rejected")
    return result.status === 0
      ? { kind: "failed" }
      : { kind: "failed", status: result.status };
  if (
    result.kind === "malformed" ||
    !validListingPage(result.value) ||
    (!("cursor" in input) &&
      (result.value.start !== input.start ||
        result.value.limit !== input.limit))
  )
    return { kind: "failed", malformed: true };
  return { kind: "ok", value: result.value };
}

/** Preserve the inspected identity and marker rather than resolving a live selection. */
export const cleanupTarget = (item: TrashListingItem): TrashCleanupTarget => ({
  photoId: item.photoId,
  removedAtMs: item.removedAtMs,
  originalId: item.originalId,
  originalLocation: item.originalLocation,
});

const validBlocker = (value: unknown): value is TrashBlocker =>
  isRecord(value) &&
  hasExactKeys(value, ["kind", "name", "action"]) &&
  nonempty(value.kind) &&
  nonempty(value.name) &&
  nonempty(value.action);

const validCleanupItem = (value: unknown): value is TrashCleanupItemResult =>
  isRecord(value) &&
  hasExactKeys(value, [
    "photoId",
    "originalLocation",
    "originalKind",
    "state",
    "message",
    "blockers",
  ]) &&
  nonempty(value.photoId) &&
  nonempty(value.originalLocation) &&
  kind(value.originalKind) &&
  (value.state === "cleaned" ||
    value.state === "changed" ||
    value.state === "present" ||
    value.state === "blocked" ||
    value.state === "unavailable" ||
    value.state === "inspection-failed") &&
  (value.message === null || typeof value.message === "string") &&
  Array.isArray(value.blockers) &&
  value.blockers.every(validBlocker);

const validCleanupOperation = (
  value: unknown,
  operationId: string,
): value is TrashCleanupOperation => {
  if (
    !isRecord(value) ||
    !hasExactKeys(value, ["operationId", "reviewed", "counts", "items"]) ||
    value.operationId !== operationId ||
    !count(value.reviewed) ||
    !isRecord(value.counts) ||
    !hasExactKeys(value.counts, [
      "cleaned",
      "changed",
      "present",
      "blocked",
      "unavailable",
      "inspectionFailed",
    ]) ||
    !count(value.counts.cleaned) ||
    !count(value.counts.changed) ||
    !count(value.counts.present) ||
    !count(value.counts.blocked) ||
    !count(value.counts.unavailable) ||
    !count(value.counts.inspectionFailed) ||
    !Array.isArray(value.items) ||
    !value.items.every(validCleanupItem) ||
    value.items.length !== value.reviewed ||
    new Set(value.items.map((item) => item.photoId)).size !== value.items.length
  )
    return false;
  const counts = {
    cleaned: 0,
    changed: 0,
    present: 0,
    blocked: 0,
    unavailable: 0,
    inspectionFailed: 0,
  };
  for (const item of value.items)
    counts[
      item.state === "inspection-failed" ? "inspectionFailed" : item.state
    ]++;
  return Object.entries(counts).every(
    ([key, amount]) =>
      (value.counts as Record<string, unknown>)[key] === amount,
  );
};

export async function previewTrashCleanup(
  fetcher: RemovalFetch,
  items: ReadonlyArray<TrashCleanupTarget>,
  signal?: AbortSignal,
): Promise<TrashWriteResult<TrashCleanupPreview>> {
  const result = await fetchJson(() =>
    fetcher("/api/trash/cleanup-preview", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ items }),
      ...(signal === undefined ? {} : { signal }),
    }),
  );
  if (result.kind !== "ok") return result;
  if (
    !isRecord(result.value) ||
    !hasExactKeys(result.value, ["items"]) ||
    !Array.isArray(result.value.items) ||
    result.value.items.length !== items.length
  )
    return { kind: "malformed" };
  const requested = new Map(
    items.map((item) => [item.photoId, item.originalLocation]),
  );
  if (
    requested.size !== items.length ||
    !result.value.items.every((value: unknown) => {
      if (
        !isRecord(value) ||
        !hasExactKeys(value, [
          "photoId",
          "originalLocation",
          "originalKind",
          "state",
          "absence",
          "message",
          "blockers",
        ]) ||
        !nonempty(value.photoId) ||
        !nonempty(value.originalLocation) ||
        !kind(value.originalKind) ||
        (value.state !== "captured" &&
          value.state !== "changed" &&
          value.state !== "unavailable") ||
        (value.absence !== "absent" &&
          value.absence !== "present" &&
          value.absence !== "uninspectable") ||
        (value.message !== null && typeof value.message !== "string") ||
        !Array.isArray(value.blockers) ||
        !value.blockers.every(validBlocker)
      )
        return false;
      return requested.get(value.photoId) === value.originalLocation;
    })
  )
    return { kind: "malformed" };
  return { kind: "ok", value: result.value as TrashCleanupPreview };
}

export async function cleanupTrash(
  fetcher: RemovalFetch,
  operationId: string,
  items: ReadonlyArray<TrashCleanupTarget>,
): Promise<TrashWriteResult<TrashCleanupOperation>> {
  const result = await fetchJson(() =>
    fetcher("/api/trash/cleanup", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ operationId, items }),
    }),
  );
  if (result.kind !== "ok") return result;
  if (
    !validCleanupOperation(result.value, operationId) ||
    result.value.items.length !== items.length
  )
    return { kind: "malformed" };
  const requested = new Map(
    items.map((item) => [item.photoId, item.originalLocation]),
  );
  if (
    requested.size !== items.length ||
    !result.value.items.every(
      (item) => requested.get(item.photoId) === item.originalLocation,
    )
  )
    return { kind: "malformed" };
  return { kind: "ok", value: result.value };
}

/** Read the existing attempt's receipt without submitting the cleanup again. */
export async function fetchTrashCleanupOperation(
  fetcher: RemovalFetch,
  operationId: string,
): Promise<TrashWriteResult<TrashCleanupOperation>> {
  const result = await fetchJson(() =>
    fetcher(`/api/trash/cleanup-operations/${encodeURIComponent(operationId)}`),
  );
  if (result.kind !== "ok") return result;
  return validCleanupOperation(result.value, operationId)
    ? { kind: "ok", value: result.value }
    : { kind: "malformed" };
}
