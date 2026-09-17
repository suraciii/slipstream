import type { AlbumSummary } from "./contracts.js";

export type AlbumActionFetch = (
  input: string,
  init?: RequestInit,
) => Promise<Response>;

export type AlbumWriteResult =
  | Readonly<{ kind: "persisted" }>
  | AlbumFolderAddResult
  | Readonly<{ kind: "rejected"; status: number }>;

export type AlbumFolderAddResult = Readonly<{
  kind: "persisted";
  folderPath: string;
  matchedCount: number;
  addedCount: number;
  alreadyMemberCount: number;
  albums: ReadonlyArray<AlbumSummary>;
}>;

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

const albumNameKey = (name: string): string =>
  name.replace(/[A-Z]/g, (letter) => letter.toLowerCase());

const validCount = (value: unknown): value is number =>
  Number.isInteger(value) && Number(value) >= 0;

const validAlbumSummaries = (
  value: unknown,
): value is ReadonlyArray<AlbumSummary> => {
  if (!Array.isArray(value) || !value.every(isAlbumSummary)) return false;
  if (new Set(value.map((album) => album.id)).size !== value.length)
    return false;
  return (
    new Set(value.map((album) => albumNameKey(album.name))).size ===
    value.length
  );
};

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
  if (
    new Set(albums.map((album) => albumNameKey(album.name))).size !==
    albums.length
  )
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
): Promise<AlbumWriteResult> => addAlbumMembers(fetcher, albumId, [photoId]);

/// Adds every named Photo to one Album through the bounded membership route.
/// A Photo that already belongs is skipped without changing its membership
/// position, exactly as a single addition is.
export const addAlbumMembers = (
  fetcher: AlbumActionFetch,
  albumId: string,
  photoIds: ReadonlyArray<string>,
): Promise<AlbumWriteResult> =>
  postAlbumAction(fetcher, `/api/albums/${albumId}/members`, { photoIds });

export const addFolderToAlbum = async (
  fetcher: AlbumActionFetch,
  albumId: string,
  folderPath: string,
  publication: string,
): Promise<AlbumWriteResult> => {
  const response = await requestAlbumAction(
    fetcher,
    `/api/albums/${albumId}/folder-members`,
    { folderPath, publication },
  );
  if (!response.ok)
    return Object.freeze({ kind: "rejected", status: response.status });
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    return Object.freeze({ kind: "rejected", status: 502 });
  }
  if (
    !isRecord(body) ||
    body.albumId !== albumId ||
    body.folderPath !== folderPath ||
    !validCount(body.matchedCount) ||
    !validCount(body.addedCount) ||
    !validCount(body.alreadyMemberCount) ||
    body.matchedCount !== body.addedCount + body.alreadyMemberCount ||
    !validAlbumSummaries(body.albums)
  )
    return Object.freeze({ kind: "rejected", status: 502 });
  return Object.freeze({
    kind: "persisted",
    folderPath,
    matchedCount: body.matchedCount,
    addedCount: body.addedCount,
    alreadyMemberCount: body.alreadyMemberCount,
    albums: Object.freeze(
      body.albums.map((album) => Object.freeze({ ...album })),
    ),
  });
};

export const removeAlbumMember = (
  fetcher: AlbumActionFetch,
  albumId: string,
  photoId: string,
): Promise<AlbumWriteResult> =>
  postAlbumAction(fetcher, `/api/albums/${albumId}/members/remove`, {
    photoId,
  });
