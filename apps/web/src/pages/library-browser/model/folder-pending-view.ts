import type { LibraryOverviewResponse } from "../api/contracts.js";
import type { FolderPendingViewModel } from "../ui/library-browser-view-contract.js";

const progress = (scan: LibraryOverviewResponse["scan"]): string => {
  switch (scan.state) {
    case "discovering":
      return scan.completed !== undefined
        ? `Discovering files… ${scan.completed.toLocaleString()}`
        : "Discovering files in this folder…";
    case "inspecting":
      return scan.total !== undefined && scan.completed !== undefined
        ? `Reading Capture Time… ${scan.completed.toLocaleString()} / ${scan.total.toLocaleString()}`
        : "Reading Capture Time…";
    case "recovering":
      return scan.total !== undefined && scan.completed !== undefined
        ? `Checking files… ${scan.completed.toLocaleString()} / ${scan.total.toLocaleString()}`
        : "Checking files…";
    case "applying":
      return "Applying folder updates…";
    default:
      return "Checking this folder…";
  }
};

export const folderPendingViewModel = (
  input: Readonly<{
    location: string | undefined;
    photoActive: boolean;
    pending: Readonly<{ files: number; hasDescendants: boolean }> | undefined;
    scan: LibraryOverviewResponse["scan"] | undefined;
  }>,
): FolderPendingViewModel | undefined => {
  if (input.photoActive || input.location === undefined) return undefined;
  const pending = input.pending;
  if (!pending || pending.files === 0) return undefined;
  const scan = input.scan;
  const checkActive =
    scan !== undefined && scan.state !== "idle" && scan.state !== "failed";
  const active =
    scan !== undefined &&
    scan.state !== "idle" &&
    scan.state !== "failed" &&
    scan.scope === input.location;
  return {
    location: input.location,
    pendingFiles: pending.files,
    active,
    ...(active && scan ? { progress: progress(scan) } : {}),
    actionEnabled: !checkActive,
  };
};
