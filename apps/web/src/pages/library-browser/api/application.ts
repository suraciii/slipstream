import type { LibraryOverviewResponse } from "./contracts.js";

export type ApplicationFetch = (
  input: string,
  init?: RequestInit,
) => Promise<Response>;

export async function fetchLibraryOverview(
  fetcher: ApplicationFetch,
): Promise<LibraryOverviewResponse> {
  const response = await fetcher("/api/overview");
  if (!response.ok) throw new Error("overview failed");
  const overview = (await response.json()) as LibraryOverviewResponse;
  if (overview.published && !overview.publication)
    throw new Error("overview omitted publication generation");
  return overview;
}

/// Separates a server that did not answer from a server that answered
/// without a usable status. Only the transport boundary belongs to
/// reachability; an answered error stays a server-side condition.
export type LibraryStatusOutcome =
  | Readonly<{ kind: "answered"; scan: LibraryOverviewResponse["scan"] }>
  | Readonly<{ kind: "rejected" }>
  | Readonly<{ kind: "unreachable" }>;

export async function probeLibraryStatus(
  fetcher: ApplicationFetch,
): Promise<LibraryStatusOutcome> {
  let response: Response;
  try {
    response = await fetcher("/api/status");
  } catch {
    return Object.freeze({ kind: "unreachable" });
  }
  if (!response.ok) return Object.freeze({ kind: "rejected" });
  try {
    return Object.freeze({
      kind: "answered",
      scan: (await response.json()) as LibraryOverviewResponse["scan"],
    });
  } catch {
    return Object.freeze({ kind: "rejected" });
  }
}

export async function fetchLibraryStatus(
  fetcher: ApplicationFetch,
): Promise<LibraryOverviewResponse["scan"]> {
  const outcome = await probeLibraryStatus(fetcher);
  if (outcome.kind !== "answered") throw new Error("status failed");
  return outcome.scan;
}

export type ScanCommandResult =
  | Readonly<{
      kind: "accepted";
      scan: LibraryOverviewResponse["scan"];
    }>
  | Readonly<{ kind: "rejected"; status: number }>;

export async function requestLibraryScan(
  fetcher: ApplicationFetch,
): Promise<ScanCommandResult> {
  const response = await fetcher("/api/scan", { method: "POST" });
  if (!response.ok) return { kind: "rejected", status: response.status };
  return {
    kind: "accepted",
    scan: (await response.json()) as LibraryOverviewResponse["scan"],
  };
}
