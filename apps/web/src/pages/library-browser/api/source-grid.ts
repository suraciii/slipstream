import type {
  BrowseOpenResponse,
  BrowsePositionResponse,
  BrowseWindowResponse,
  PhotoSummary,
  SelectionCounts,
  SelectionFilter,
} from "./contracts.js";
import { isRecord, validOptional } from "./guards.js";
import { fetchJson } from "./json-response.js";

export type SourceGridFetch = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

/// One explicit view order for an open source. `source-default` leaves the
/// order to the server: Capture Time earliest first for `All Photos` and an
/// Original Folder, persisted membership position for an Album.
export type SourceViewOrder =
  | "source-default"
  | "capture-time-asc"
  | "capture-time-desc";

export type BrowseSourceRequest =
  | Readonly<{
      kind: "library";
      preferredPhotoId?: string;
      resume?: boolean;
      order?: SourceViewOrder;
      selection?: SelectionFilter;
    }>
  | Readonly<{
      kind: "album";
      albumId: string;
      preferredPhotoId?: string;
      resume?: boolean;
      order?: SourceViewOrder;
      selection?: SelectionFilter;
    }>
  | Readonly<{
      kind: "folder";
      folderPath: string;
      publication: string;
      preferredPhotoId?: string;
      resume?: boolean;
      order?: SourceViewOrder;
      selection?: SelectionFilter;
    }>;

export type SourceGridApiResult<T> =
  | Readonly<{ kind: "ok"; value: T }>
  | Readonly<{ kind: "failed"; status?: number; malformed?: true }>;

/// One Grid Photo summary as the server presents it. Shared with the removal
/// client, whose Removed Photos listing carries the same facts.
export const validPhotoSummary = (value: unknown): value is PhotoSummary => {
  if (!isRecord(value) || !isRecord(value.preview)) return false;
  const preview = value.preview;
  if (
    preview.state !== "inspection-pending" &&
    preview.state !== "ready" &&
    preview.state !== "failed" &&
    preview.state !== "unavailable"
  )
    return false;
  if (
    !isRecord(value.original) ||
    !(value.original.kind === "raw" || value.original.kind === "jpeg") ||
    typeof value.original.available !== "boolean"
  )
    return false;
  return (
    typeof value.id === "string" &&
    value.id.length > 0 &&
    typeof value.available === "boolean" &&
    validOptional(
      value.originalFilename,
      (item) => typeof item === "string" && item.length > 0,
    ) &&
    (value.selectionState === "undecided" ||
      value.selectionState === "selected" ||
      value.selectionState === "rejected") &&
    Number.isInteger(value.rating) &&
    Number(value.rating) >= 0 &&
    Number(value.rating) <= 5 &&
    typeof value.hasSavedEdits === "boolean" &&
    validOptional(
      preview.source,
      (source) => source === "jpeg-original" || source === "raw-embedded-jpeg",
    ) &&
    validOptional(preview.width, Number.isInteger) &&
    validOptional(preview.height, Number.isInteger) &&
    validOptional(preview.limitedDetail, (item) => typeof item === "boolean") &&
    validOptional(preview.url, (item) => typeof item === "string") &&
    validOptional(preview.thumbnailUrl, (item) => typeof item === "string") &&
    validOptional(preview.message, (item) => typeof item === "string")
  );
};

const validSelectionCounts = (value: unknown): value is SelectionCounts =>
  isRecord(value) &&
  [value.selected, value.rejected, value.undecided].every(
    (count) => Number.isInteger(count) && Number(count) >= 0,
  );

async function browseJson<T>(
  request: () => Promise<Response>,
  validate: (value: unknown) => boolean,
): Promise<SourceGridApiResult<T>> {
  const result = await fetchJson(request);
  if (result.kind === "rejected")
    return result.transport
      ? { kind: "failed" }
      : { kind: "failed", status: result.status };
  if (result.kind === "malformed" || !validate(result.value))
    return { kind: "failed", malformed: true };
  return { kind: "ok", value: result.value as T };
}

export async function openBrowse(
  fetcher: SourceGridFetch,
  source: BrowseSourceRequest,
  signal: AbortSignal,
): Promise<SourceGridApiResult<BrowseOpenResponse>> {
  const sourceFields =
    source.kind === "folder"
      ? { folderPath: source.folderPath, publication: source.publication }
      : source.kind === "album"
        ? { albumId: source.albumId }
        : {};
  return browseJson(
    () =>
      fetcher("/api/browse", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          source: source.kind,
          ...sourceFields,
          ...(source.order && source.order !== "source-default"
            ? { order: source.order }
            : {}),
          ...(source.selection && source.selection !== "all"
            ? { selection: source.selection }
            : {}),
          ...(source.preferredPhotoId
            ? { photoId: source.preferredPhotoId }
            : {}),
          ...(source.resume ? { resume: true } : {}),
        }),
        signal,
        priority: "high",
      }),
    (value) =>
      isRecord(value) &&
      typeof value.token === "string" &&
      value.token.length > 0 &&
      Number.isInteger(value.total) &&
      Number(value.total) >= 0 &&
      Number.isInteger(value.position) &&
      Number(value.position) >= 0 &&
      validSelectionCounts(value.selectionCounts) &&
      (Number(value.total) === 0
        ? Number(value.position) === 0
        : Number(value.position) < Number(value.total)),
  );
}

export async function fetchBrowseWindow(
  fetcher: SourceGridFetch,
  input: Readonly<{
    token: string;
    start: number;
    limit: number;
    expectedTotal: number;
    signal: AbortSignal;
    priority: "high" | "low";
  }>,
): Promise<SourceGridApiResult<BrowseWindowResponse>> {
  return browseJson(
    () =>
      fetcher(
        `/api/browse/${encodeURIComponent(input.token)}?start=${input.start}&limit=${input.limit}`,
        { signal: input.signal, priority: input.priority },
      ),
    (value) =>
      isRecord(value) &&
      value.start === input.start &&
      value.total === input.expectedTotal &&
      Array.isArray(value.photos) &&
      value.photos.length ===
        Math.min(input.limit, input.expectedTotal - input.start) &&
      input.start + value.photos.length <= input.expectedTotal &&
      value.photos.every(validPhotoSummary),
  );
}

export async function fetchBrowsePosition(
  fetcher: SourceGridFetch,
  input: Readonly<{
    token: string;
    photoId: string;
    signal: AbortSignal;
  }>,
): Promise<SourceGridApiResult<BrowsePositionResponse>> {
  return browseJson(
    () =>
      fetcher(
        `/api/browse/${encodeURIComponent(input.token)}/position?photoId=${encodeURIComponent(input.photoId)}`,
        { signal: input.signal, priority: "high" },
      ),
    (value) =>
      isRecord(value) &&
      (value.position === null ||
        (Number.isInteger(value.position) && Number(value.position) >= 0)),
  );
}

/// The Thumbnail endpoint's answer, classified by what the Grid may claim.
/// A 404 names the Preview fact (`unavailable` or `failed`); only a missing
/// or unreadable transfer is a delivery failure.
export type ThumbnailResult =
  | Readonly<{ kind: "ready"; url: string }>
  | Readonly<{ kind: "not-ready"; state: "unavailable" | "failed" }>
  | Readonly<{ kind: "delivery-failed" }>;

export async function fetchThumbnail(
  fetcher: SourceGridFetch,
  photoId: string,
  signal: AbortSignal,
): Promise<ThumbnailResult> {
  try {
    const response = await fetcher(`/api/photos/${photoId}/thumbnail`, {
      signal,
      priority: "low",
    });
    let value: unknown;
    try {
      value = await response.json();
    } catch {
      return { kind: "delivery-failed" };
    }
    if (!isRecord(value)) return { kind: "delivery-failed" };
    if (response.ok && value.state === "ready" && typeof value.url === "string")
      return { kind: "ready", url: value.url };
    if (
      response.status === 404 &&
      (value.state === "unavailable" || value.state === "failed")
    )
      return { kind: "not-ready", state: value.state };
    return { kind: "delivery-failed" };
  } catch {
    return { kind: "delivery-failed" };
  }
}

export async function releaseBrowse(
  fetcher: SourceGridFetch,
  token: string,
): Promise<void> {
  try {
    await fetcher(`/api/browse/${encodeURIComponent(token)}`, {
      method: "DELETE",
      keepalive: true,
    });
  } catch {
    // Bounded server expiry remains the fallback.
  }
}
