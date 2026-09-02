import type {
  BrowseOpenResponse,
  BrowseWindowResponse,
  PhotoSummary,
} from "./contracts.js";

export type SourceGridFetch = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

export type BrowseSourceRequest =
  | Readonly<{ kind: "library"; preferredPhotoId?: string }>
  | Readonly<{
      kind: "album";
      albumId: string;
      preferredPhotoId?: string;
    }>
  | Readonly<{
      kind: "folder";
      folderPath: string;
      publication: string;
      preferredPhotoId?: string;
    }>;

export type SourceGridApiResult<T> =
  | Readonly<{ kind: "ok"; value: T }>
  | Readonly<{ kind: "failed"; status?: number; malformed?: true }>;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

const validOptional = (
  value: unknown,
  predicate: (candidate: unknown) => boolean,
): boolean => value === undefined || predicate(value);

const validPhotoSummary = (value: unknown): value is PhotoSummary => {
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
    !Array.isArray(value.originals) ||
    !value.originals.every(
      (original) =>
        isRecord(original) &&
        (original.kind === "raw" || original.kind === "jpeg") &&
        typeof original.available === "boolean",
    )
  )
    return false;
  return (
    typeof value.id === "string" &&
    value.id.length > 0 &&
    typeof value.available === "boolean" &&
    typeof value.ambiguous === "boolean" &&
    (value.selectionState === "undecided" ||
      value.selectionState === "selected" ||
      value.selectionState === "rejected") &&
    Number.isInteger(value.rating) &&
    Number(value.rating) >= 0 &&
    Number(value.rating) <= 5 &&
    validOptional(
      preview.source,
      (source) => source === "matching-jpeg" || source === "embedded-raw-jpeg",
    ) &&
    validOptional(preview.width, Number.isInteger) &&
    validOptional(preview.height, Number.isInteger) &&
    validOptional(preview.limitedDetail, (item) => typeof item === "boolean") &&
    validOptional(preview.url, (item) => typeof item === "string") &&
    validOptional(preview.thumbnailUrl, (item) => typeof item === "string") &&
    validOptional(preview.message, (item) => typeof item === "string")
  );
};

export async function openBrowse(
  fetcher: SourceGridFetch,
  source: BrowseSourceRequest,
  signal: AbortSignal,
): Promise<SourceGridApiResult<BrowseOpenResponse>> {
  let response: Response;
  try {
    response = await fetcher("/api/browse", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(
        source.kind === "library"
          ? {
              source: "library",
              ...(source.preferredPhotoId
                ? { photoId: source.preferredPhotoId }
                : {}),
            }
          : source.kind === "folder"
            ? {
                source: "folder",
                folderPath: source.folderPath,
                publication: source.publication,
                ...(source.preferredPhotoId
                  ? { photoId: source.preferredPhotoId }
                  : {}),
              }
            : {
                source: "album",
                albumId: source.albumId,
                ...(source.preferredPhotoId
                  ? { photoId: source.preferredPhotoId }
                  : {}),
              },
      ),
      signal,
      priority: "high",
    });
  } catch {
    return { kind: "failed" };
  }
  if (!response.ok) return { kind: "failed", status: response.status };
  try {
    const value: unknown = await response.json();
    if (
      !isRecord(value) ||
      typeof value.token !== "string" ||
      value.token.length === 0 ||
      !Number.isInteger(value.total) ||
      Number(value.total) < 0 ||
      !Number.isInteger(value.position) ||
      Number(value.position) < 0 ||
      (Number(value.total) === 0
        ? Number(value.position) !== 0
        : Number(value.position) >= Number(value.total))
    )
      return { kind: "failed", malformed: true };
    return { kind: "ok", value: value as BrowseOpenResponse };
  } catch {
    return { kind: "failed", malformed: true };
  }
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
  let response: Response;
  try {
    response = await fetcher(
      `/api/browse/${encodeURIComponent(input.token)}?start=${input.start}&limit=${input.limit}`,
      { signal: input.signal, priority: input.priority },
    );
  } catch {
    return { kind: "failed" };
  }
  if (!response.ok) return { kind: "failed", status: response.status };
  try {
    const value: unknown = await response.json();
    if (
      !isRecord(value) ||
      value.start !== input.start ||
      value.total !== input.expectedTotal ||
      !Array.isArray(value.photos) ||
      value.photos.length !==
        Math.min(input.limit, input.expectedTotal - input.start) ||
      input.start + value.photos.length > input.expectedTotal ||
      !value.photos.every(validPhotoSummary)
    )
      return { kind: "failed", malformed: true };
    return { kind: "ok", value: value as BrowseWindowResponse };
  } catch {
    return { kind: "failed", malformed: true };
  }
}

export async function fetchThumbnail(
  fetcher: SourceGridFetch,
  photoId: string,
  signal: AbortSignal,
): Promise<string | undefined> {
  try {
    const response = await fetcher(`/api/photos/${photoId}/thumbnail`, {
      signal,
      priority: "low",
    });
    if (!response.ok) return undefined;
    const value: unknown = await response.json();
    return isRecord(value) && typeof value.url === "string"
      ? value.url
      : undefined;
  } catch {
    return undefined;
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
