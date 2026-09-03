import type { AlbumSummary } from "./contracts.js";

export type AlbumActionFetch = (
  input: string,
  init?: RequestInit,
) => Promise<Response>;

export type AlbumWriteResult =
  | Readonly<{ kind: "persisted" }>
  | Readonly<{ kind: "rejected"; status: number }>;

export type AlbumCreateResult =
  | Readonly<{ kind: "persisted"; createdAlbum: AlbumSummary }>
  | Readonly<{ kind: "rejected"; status: number }>
  | Readonly<{ kind: "malformed" }>;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

const isAlbumSummary = (value: unknown): value is AlbumSummary =>
  isRecord(value) &&
  typeof value.id === "string" &&
  value.id.length > 0 &&
  typeof value.name === "string" &&
  Number.isInteger(value.photoCount) &&
  Number(value.photoCount) >= 0 &&
  typeof value.hasSavedPosition === "boolean";

const requestAlbumAction = (
  fetcher: AlbumActionFetch,
  path: string,
  body: unknown,
): Promise<Response> =>
  fetcher(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });

async function postAlbumAction(
  fetcher: AlbumActionFetch,
  path: string,
  body: unknown,
): Promise<AlbumWriteResult> {
  const response = await requestAlbumAction(fetcher, path, body);
  return response.ok
    ? Object.freeze({ kind: "persisted" })
    : Object.freeze({ kind: "rejected", status: response.status });
}

export const createAlbum = async (
  fetcher: AlbumActionFetch,
  name: string,
): Promise<AlbumCreateResult> => {
  const response = await requestAlbumAction(fetcher, "/api/albums", { name });
  if (!response.ok)
    return Object.freeze({ kind: "rejected", status: response.status });
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    return Object.freeze({ kind: "malformed" });
  }
  if (!isRecord(body) || !Array.isArray(body.albums))
    return Object.freeze({ kind: "malformed" });
  const albums = body.albums.filter(isAlbumSummary);
  if (albums.length !== body.albums.length)
    return Object.freeze({ kind: "malformed" });
  if (new Set(albums.map((album) => album.id)).size !== albums.length)
    return Object.freeze({ kind: "malformed" });
  const matches = albums.filter((album) => album.name === name);
  const createdAlbum = matches[0];
  if (
    !createdAlbum ||
    matches.length !== 1 ||
    createdAlbum.photoCount !== 0 ||
    createdAlbum.hasSavedPosition
  )
    return Object.freeze({ kind: "malformed" });
  return Object.freeze({
    kind: "persisted",
    createdAlbum: Object.freeze({ ...createdAlbum }),
  });
};

export const renameAlbum = (
  fetcher: AlbumActionFetch,
  albumId: string,
  name: string,
): Promise<AlbumWriteResult> =>
  postAlbumAction(fetcher, `/api/albums/${albumId}/rename`, { name });

export const deleteAlbum = (
  fetcher: AlbumActionFetch,
  albumId: string,
): Promise<AlbumWriteResult> =>
  postAlbumAction(fetcher, `/api/albums/${albumId}/delete`, {});

export const addAlbumMember = (
  fetcher: AlbumActionFetch,
  albumId: string,
  photoId: string,
): Promise<AlbumWriteResult> =>
  postAlbumAction(fetcher, `/api/albums/${albumId}/members`, {
    photoIds: [photoId],
  });

export const removeAlbumMember = (
  fetcher: AlbumActionFetch,
  albumId: string,
  photoId: string,
): Promise<AlbumWriteResult> =>
  postAlbumAction(fetcher, `/api/albums/${albumId}/members/remove`, {
    photoId,
  });
