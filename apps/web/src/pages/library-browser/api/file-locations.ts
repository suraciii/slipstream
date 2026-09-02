import type { FileLocationsResponse } from "./contracts.js";

export type FileLocationWindowRequest = Readonly<{
  parent: string;
  start: number;
  limit: number;
  publication?: string;
}>;

export type FileLocationWindowResult =
  | Readonly<{ kind: "loaded"; window: FileLocationsResponse }>
  | Readonly<{ kind: "publication-conflict" }>
  | Readonly<{ kind: "failed" }>;

export type FileLocationFetch = (url: string) => Promise<Response>;

export async function fetchFileLocationWindow(
  fetcher: FileLocationFetch,
  request: FileLocationWindowRequest,
): Promise<FileLocationWindowResult> {
  const parameters = new URLSearchParams({
    start: String(request.start),
    limit: String(request.limit),
  });
  if (request.parent) parameters.set("parent", request.parent);
  if (request.publication) parameters.set("publication", request.publication);

  const response = await fetcher(`/api/file-locations?${parameters}`);
  if (response.status === 409) return { kind: "publication-conflict" };
  if (!response.ok) return { kind: "failed" };
  return {
    kind: "loaded",
    window: (await response.json()) as FileLocationsResponse,
  };
}
