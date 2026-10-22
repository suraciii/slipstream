import type { FileLocationsResponse } from "./contracts.js";
import { fetchJson } from "./json-response.js";

export type FileLocationWindowRequest = Readonly<{
  parent: string;
  start: number;
  limit: number;
  publication?: string;
}>;

/// One File Location window read. A lost transport, an answered refusal, and a
/// body that is not JSON are three distinct failures: the first reports no
/// status, the second reports the status the server answered, and the last
/// reports the answer as unusable.
export type FileLocationWindowResult =
  | Readonly<{ kind: "loaded"; window: FileLocationsResponse }>
  | Readonly<{ kind: "publication-conflict" }>
  | Readonly<{ kind: "failed"; status?: number; malformed?: true }>;

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

  const result = await fetchJson(() =>
    fetcher(`/api/file-locations?${parameters}`),
  );
  if (result.kind === "rejected") {
    if (result.transport) return { kind: "failed" };
    return result.status === 409
      ? { kind: "publication-conflict" }
      : { kind: "failed", status: result.status };
  }
  if (result.kind === "malformed") return { kind: "failed", malformed: true };
  return {
    kind: "loaded",
    window: result.value as FileLocationsResponse,
  };
}
