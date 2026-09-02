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

export async function fetchLibraryStatus(
  fetcher: ApplicationFetch,
): Promise<LibraryOverviewResponse["scan"]> {
  const response = await fetcher("/api/status");
  if (!response.ok) throw new Error("status failed");
  return (await response.json()) as LibraryOverviewResponse["scan"];
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
