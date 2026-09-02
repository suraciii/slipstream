export type SavedPositionFetch = (
  input: string,
  init?: RequestInit,
) => Promise<Response>;

export type SavedPositionWriteResult =
  | Readonly<{ kind: "persisted" }>
  | Readonly<{ kind: "rejected"; status: number }>;

export async function saveAlbumPosition(
  fetcher: SavedPositionFetch,
  albumId: string,
  photoId: string,
): Promise<SavedPositionWriteResult> {
  const response = await fetcher(`/api/albums/${albumId}/progress`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ photoId }),
  });
  return response.ok
    ? Object.freeze({ kind: "persisted" })
    : Object.freeze({ kind: "rejected", status: response.status });
}
