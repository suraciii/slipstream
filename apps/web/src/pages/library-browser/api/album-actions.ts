export type AlbumActionFetch = (
  input: string,
  init?: RequestInit,
) => Promise<Response>;

export type AlbumWriteResult =
  | Readonly<{ kind: "persisted" }>
  | Readonly<{ kind: "rejected"; status: number }>;

async function postAlbumAction(
  fetcher: AlbumActionFetch,
  path: string,
  body: unknown,
): Promise<AlbumWriteResult> {
  const response = await fetcher(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  return response.ok
    ? Object.freeze({ kind: "persisted" })
    : Object.freeze({ kind: "rejected", status: response.status });
}

export const createAlbum = (
  fetcher: AlbumActionFetch,
  name: string,
): Promise<AlbumWriteResult> =>
  postAlbumAction(fetcher, "/api/albums", { name });

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
