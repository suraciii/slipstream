/// The shared reopen the page performs after a Library-wide change: refresh the
/// Overview and Folder counts, then reopen the current source at its current
/// anchor so the Grid reads the newly published facts.
///
/// An explained destination without a Snapshot stays put: no reopen is
/// attempted, because there is no frozen sequence to rebuild. This is
/// page-local coordination, so it holds no state and issues no HTTP of its own.
import type { SourceGridOwner } from "./source-grid-owner.js";
import type { PhotoOwner } from "./photo-owner.js";

/// What one reopen reads and needs.
export interface ReopenAfterChangeDeps {
  sourceGrid: Pick<
    SourceGridOwner,
    "token" | "authority" | "generation" | "isReady" | "readGridPosition"
  >;
  photoOwner: Pick<PhotoOwner, "currentIndex">;
  refreshOverview: () => Promise<unknown>;
  refreshFolderCounts: () => Promise<unknown>;
  reopen: (
    anchor: number,
    generation: number,
    reason: Readonly<{ progress: string; settled: string }>,
  ) => Promise<unknown>;
}

/// One reopen reason: the progress line while the reopen is in flight and the
/// settled line once the new sequence is established. A Library change never
/// claims a percentage or an invented outcome.
export type ReopenReason = Readonly<{ progress: string; settled: string }>;

/// The reason a confirmed Restore reopens the source under.
export const RESTORE_REOPEN: ReopenReason = Object.freeze({
  progress: "Reopening this source after the restore…",
  settled: "Source reopened after the restore.",
});

/// Refreshes shared facts and reopens the current source at its anchor. A
/// source without a ready Snapshot is left exactly as it is.
export const reopenAfterLibraryChange = async (
  deps: ReopenAfterChangeDeps,
  reason: ReopenReason,
): Promise<void> => {
  await deps.refreshOverview();
  await deps.refreshFolderCounts();
  if (
    deps.sourceGrid.token === "" ||
    !deps.sourceGrid.isReady(deps.sourceGrid.authority)
  )
    return;
  const anchor =
    deps.sourceGrid.readGridPosition(deps.sourceGrid.authority) ??
    deps.photoOwner.currentIndex;
  await deps.reopen(anchor, deps.sourceGrid.generation, reason);
};
