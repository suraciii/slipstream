import type {
  RecoveryClaim,
  RecoveryGate,
  RecoveryTransition,
} from "./async-ownership.js";
import type {
  BrowseRangeRecoveryOwner,
  GridRangeRetry,
} from "./browse-range-recovery-owner.js";
import type { PhotoAuthority, PhotoOwner } from "./photo-owner.js";
import type {
  SourceGridOwner,
  SourceWindowOperation,
  SourceWindowOutcome,
} from "./source-grid-owner.js";

type RangeRetry = Readonly<{ claim: RecoveryClaim; retry: GridRangeRetry }>;

export interface BrowseWindowController {
  load(
    this: void,
    index: number,
    operation?: SourceWindowOperation,
    quiet?: boolean,
    priority?: "high" | "low",
    transition?: RecoveryTransition,
    photoAuthority?: PhotoAuthority,
  ): Promise<boolean>;
  retryCurrentRanges(
    this: void,
    rangeRetries: ReadonlyArray<RangeRetry>,
  ): Promise<boolean>;
  dispose(this: void): void;
}

type BrowseWindowDependencies = Readonly<{
  source: Pick<
    SourceGridOwner,
    | "authority"
    | "generation"
    | "total"
    | "isCurrent"
    | "isReady"
    | "describeWindow"
    | "loadWindow"
    | "onWindowSettled"
    | "invalidateWindow"
  >;
  photo: Pick<PhotoOwner, "authority" | "ownsWindow">;
  ranges: BrowseRangeRecoveryOwner;
  gate: Pick<RecoveryGate, "isActive">;
  photoRecoveryKey: (authority: PhotoAuthority) => string;
  formatPhotoCount: (count: number) => string;
  setGridStatus: (message: string) => void;
  setPhotoStatus: (message: string) => void;
  renderGridIfVisible: () => void;
  renderFilmstrip: () => void;
  updateControls: () => void;
  reopenExpired: (index: number, generation: number) => Promise<void>;
}>;

const failureMessage = (
  outcome: Extract<SourceWindowOutcome, { kind: "failed" }>,
): string =>
  outcome.malformed === true
    ? `${outcome.range} returned an invalid response. Retry this range.`
    : outcome.transportLost
      ? `Connection lost while loading ${outcome.range}. Retry this range.`
      : `${outcome.range} could not be loaded (HTTP ${outcome.status}). Retry this range.`;

/** Settles windows and replays exact range retries without owning their state. */
export function createBrowseWindowController(
  deps: BrowseWindowDependencies,
): BrowseWindowController {
  const { source, photo, ranges } = deps;
  let alive = true;

  const load: BrowseWindowController["load"] = async (
    index,
    operation = { kind: "grid", authority: source.authority },
    quiet = false,
    priority = quiet ? "low" : "high",
    transition,
    photoAuthority = photo.authority,
  ) => {
    if (!alive) return false;
    if (source.total === 0) return true;
    const sourceAuthority =
      operation.kind === "photo" ? source.authority : operation.authority;
    const windowAuthority =
      operation.kind === "photo" ? operation.authority : undefined;
    const ownerScope = operation.kind === "photo" ? "photo" : "source";
    const ownerGeneration =
      operation.kind === "photo"
        ? deps.photoRecoveryKey(photoAuthority)
        : String(source.generation);
    if (!quiet) {
      const { range } = source.describeWindow(index);
      deps.setGridStatus(
        `Loading ${range} of ${source.total.toLocaleString()}…`,
      );
    }
    const outcome = await source.loadWindow(index, operation, {
      quiet,
      priority,
    });
    try {
      const exactOwner =
        alive &&
        outcome.authority === sourceAuthority &&
        source.isCurrent(sourceAuthority) &&
        (operation.kind === "photo"
          ? outcome.owner.scope === "photo" &&
            outcome.owner.authority === windowAuthority &&
            photo.ownsWindow(photoAuthority, outcome.owner.authority)
          : outcome.owner.scope === "source" &&
            String(outcome.owner.generation) === ownerGeneration);
      if (!exactOwner || outcome.kind === "detached") return false;
      if (outcome.kind === "expired") {
        await deps.reopenExpired(outcome.index, source.generation);
        return false;
      }
      if (outcome.kind === "failed") {
        const message = failureMessage(outcome);
        if (ownerScope === "photo") deps.setPhotoStatus(message);
        else deps.setGridStatus(message);
        ranges.fail(
          ownerScope,
          ownerGeneration,
          outcome.start,
          outcome.transportLost,
          operation.kind === "photo"
            ? undefined
            : {
                sourceAuthority,
                operationKind: operation.kind,
                anchorIndex: index,
                start: outcome.start,
                quiet,
                priority,
                message,
              },
          transition,
        );
        return false;
      }
      ranges.recover(ownerScope, ownerGeneration, outcome.start);
      // SourceGridOwner suppresses notifications when a caller awaits the
      // shared window. That caller owns its recovery and render admission.
      if (outcome.changed) deps.renderGridIfVisible();
      if (!quiet) {
        if (
          ranges.admittedRange &&
          source.isCurrent(ranges.admittedRange.authority)
        )
          ranges.presentStatus();
        else
          deps.setGridStatus(`Ready · ${deps.formatPhotoCount(source.total)}`);
      }
      return true;
    } finally {
      if (alive) deps.updateControls();
    }
  };

  // The source owner emits once per shared window, only with no awaited
  // callers. Do not add another waiter map or settle an awaited transition here.
  const unsubscribe = source.onWindowSettled((outcome) => {
    if (!alive || !source.isCurrent(outcome.authority)) return;
    if (
      outcome.owner.scope === "photo"
        ? !photo.ownsWindow(photo.authority, outcome.owner.authority)
        : outcome.owner.generation !== source.generation
    )
      return;
    if (outcome.kind === "loaded") deps.renderFilmstrip();
    if (outcome.owner.scope !== "source") return;
    const generation = String(outcome.owner.generation);
    switch (outcome.kind) {
      case "loaded":
        ranges.recover("source", generation, outcome.start);
        if (outcome.changed) deps.renderGridIfVisible();
        ranges.presentStatus();
        return;
      case "failed": {
        const message = failureMessage(outcome);
        deps.setGridStatus(message);
        ranges.fail(
          "source",
          generation,
          outcome.start,
          outcome.transportLost,
          {
            sourceAuthority: outcome.authority,
            operationKind: source.isReady(outcome.authority)
              ? "grid"
              : "source",
            // A clamped tail start is not necessarily an index that aligns to
            // that window. The range owner supplies the actual replay anchor.
            anchorIndex: ranges.windowAnchorIndex(outcome.start),
            start: outcome.start,
            quiet: false,
            priority: "high",
            message,
          },
        );
        deps.updateControls();
        return;
      }
      case "expired":
        // Reopening supersedes this generation before sibling expiries settle.
        void deps.reopenExpired(outcome.start, source.generation);
        return;
      case "detached":
        deps.renderGridIfVisible();
        return;
    }
  });

  return {
    load,
    async retryCurrentRanges(rangeRetries) {
      if (!alive) return false;
      let recovered = true;
      for (const { claim, retry: range } of rangeRetries) {
        if (
          !alive ||
          !source.isCurrent(range.sourceAuthority) ||
          !deps.gate.isActive(claim)
        ) {
          recovered = false;
          continue;
        }
        source.invalidateWindow(range.anchorIndex);
        const loaded = await load(
          range.anchorIndex,
          { kind: range.operationKind, authority: range.sourceAuthority },
          range.quiet,
          range.priority,
        );
        if (!loaded || deps.gate.isActive(claim)) recovered = false;
      }
      return recovered;
    },
    dispose() {
      if (!alive) return;
      alive = false;
      unsubscribe();
    },
  };
}
