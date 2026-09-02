import type {
  PreviewResponse,
  SelectionState,
  UndoDescription,
} from "./contracts.js";

export type PhotoFetch = (
  input: string,
  init?: RequestInit,
) => Promise<Response>;

export type PreviewResult =
  | Readonly<{
      kind: "ready";
      value: PreviewResponse & { state: "ready"; url: string };
    }>
  | Readonly<{
      kind: "not-ready";
      value: PreviewResponse & { state: "unavailable" | "failed" };
    }>
  | Readonly<{ kind: "rejected"; status: number }>
  | Readonly<{ kind: "malformed" }>
  | Readonly<{ kind: "accepted" }>;

export type PhotoStateResult =
  | Readonly<{ kind: "persisted"; undo?: UndoDescription }>
  | Readonly<{ kind: "rejected"; status: number }>
  | Readonly<{ kind: "malformed" }>;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

const optional = (
  value: unknown,
  predicate: (candidate: unknown) => boolean,
): boolean => value === undefined || predicate(value);

const validPreview = (value: unknown): value is PreviewResponse =>
  isRecord(value) &&
  (value.state === "ready" ||
    value.state === "unavailable" ||
    value.state === "failed") &&
  optional(
    value.source,
    (source) => source === "matching-jpeg" || source === "embedded-raw-jpeg",
  ) &&
  optional(value.stale, (stale) => typeof stale === "boolean") &&
  optional(value.width, Number.isInteger) &&
  optional(value.height, Number.isInteger) &&
  optional(value.limitedDetail, (limited) => typeof limited === "boolean") &&
  optional(value.url, (url) => typeof url === "string") &&
  optional(value.message, (message) => typeof message === "string");

const validStateValue = (
  field: "selectionState" | "rating",
  value: unknown,
): value is SelectionState | number =>
  field === "selectionState"
    ? value === "undecided" || value === "selected" || value === "rejected"
    : Number.isInteger(value) && Number(value) >= 0 && Number(value) <= 5;

const validUndo = (value: unknown): value is UndoDescription =>
  isRecord(value) &&
  typeof value.photoId === "string" &&
  value.photoId.length > 0 &&
  (value.field === "selectionState" || value.field === "rating") &&
  validStateValue(value.field, value.priorValue) &&
  validStateValue(value.field, value.expectedCurrent);

export async function fetchPreview(
  fetcher: PhotoFetch,
  photoId: string,
  priority: "current" | "adjacent",
  signal: AbortSignal,
): Promise<PreviewResult> {
  const response = await fetcher(
    `/api/photos/${photoId}/preview${priority === "adjacent" ? "?priority=adjacent" : ""}`,
    {
      signal,
      priority: priority === "adjacent" ? "low" : "high",
    },
  );
  if (priority === "adjacent")
    return response.ok
      ? Object.freeze({ kind: "accepted" })
      : Object.freeze({ kind: "rejected", status: response.status });

  let value: unknown;
  try {
    value = await response.json();
  } catch {
    return Object.freeze({ kind: "malformed" });
  }
  if (!validPreview(value)) return Object.freeze({ kind: "malformed" });
  if (response.ok && value.state === "ready" && value.url)
    return Object.freeze({
      kind: "ready",
      value: Object.freeze({ ...value, state: "ready", url: value.url }),
    });
  if (
    response.status === 404 &&
    (value.state === "unavailable" || value.state === "failed")
  )
    return Object.freeze({
      kind: "not-ready",
      value: Object.freeze({
        ...value,
        state: value.state,
      }),
    });
  return Object.freeze({ kind: "rejected", status: response.status });
}

export async function persistPhotoState(
  fetcher: PhotoFetch,
  input: Readonly<{
    photoId: string;
    field: "selectionState" | "rating";
    value: SelectionState | number;
    albumId?: string;
    expectedCurrent?: SelectionState | number;
    requireUndo: boolean;
  }>,
): Promise<PhotoStateResult> {
  const response = await fetcher(`/api/photos/${input.photoId}/state`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      field: input.field,
      value: input.value,
      ...(input.expectedCurrent !== undefined
        ? { expectedCurrent: input.expectedCurrent }
        : {}),
      ...(input.albumId ? { albumId: input.albumId } : {}),
    }),
  });
  if (!response.ok)
    return Object.freeze({ kind: "rejected", status: response.status });
  if (!input.requireUndo) return Object.freeze({ kind: "persisted" });
  try {
    const value: unknown = await response.json();
    return isRecord(value) && validUndo(value.undo)
      ? Object.freeze({ kind: "persisted", undo: Object.freeze(value.undo) })
      : Object.freeze({ kind: "malformed" });
  } catch {
    return Object.freeze({ kind: "malformed" });
  }
}
