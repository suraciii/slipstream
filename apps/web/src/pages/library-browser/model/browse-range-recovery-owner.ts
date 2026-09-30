import type {
  RecoveryClaim,
  RecoveryGate,
  RecoveryTransition,
} from "./async-ownership.js";
import type { SourceAuthority } from "./source-grid-owner.js";

export type GridRangeRetry = Readonly<{
  sourceAuthority: SourceAuthority;
  operationKind: "source" | "grid";
  anchorIndex: number;
  start: number;
  quiet: boolean;
  priority: "high" | "low";
  message: string;
}>;

type BrowseRangeFailure = Readonly<{
  claim: RecoveryClaim;
  ownerScope: "source" | "photo";
  retry?: GridRangeRetry;
}>;

export type AdmittedRange = Readonly<{
  start: number;
  end: number;
  authority: SourceAuthority;
}>;
export interface BrowseRangeRecoveryOwner {
  readonly admittedRange: AdmittedRange | undefined;
  setAdmittedRange(range: AdmittedRange): void;
  presentStatus(): void;
  fail(
    ownerScope: "source" | "photo",
    generation: string,
    start: number,
    transportLost: boolean,
    retry?: GridRangeRetry,
    transition?: RecoveryTransition,
  ): void;
  recover(
    ownerScope: "source" | "photo",
    generation: string,
    start: number,
  ): void;
  clearInactive(): void;
  currentSourceRetries(
    alignedStart?: number,
  ): Array<Readonly<{ claim: RecoveryClaim; retry: GridRangeRetry }>>;
  windowAnchorIndex(windowStart: number): number;
}

/*
 * The owner keeps range claims and the admitted Grid range together: a range
 * report can only present a Retry belonging to its current source.
 */
type BrowseRangeRecoveryDependencies = Readonly<{
  gate: RecoveryGate;
  source: Readonly<{
    isCurrent: (authority: SourceAuthority) => boolean;
    photoAt: (index: number) => unknown;
    alignedStart: (index: number) => number;
    describeWindow: (index: number) => Readonly<{ range: string }>;
    total: () => number;
  }>;
  setRangeStatus: (text: string) => void;
  formatPhotoCount: (count: number) => string;
  onFailure: () => void;
  onRecovered: () => void;
}>;

export function createBrowseRangeRecoveryOwner(
  deps: BrowseRangeRecoveryDependencies,
): BrowseRangeRecoveryOwner {
  const failures = new Map<string, BrowseRangeFailure>();
  let admittedRange: AdmittedRange | undefined;

  const firstMissing = (range: AdmittedRange): number | undefined => {
    for (let index = range.start; index < range.end; index += 1)
      if (deps.source.photoAt(index) === undefined) return index;
    return undefined;
  };

  const failureStatus = (missing: number): string | undefined => {
    for (const failure of failures.values()) {
      if (failure.ownerScope !== "source" || !failure.retry) continue;
      if (!deps.gate.isActive(failure.claim)) continue;
      if (!deps.source.isCurrent(failure.retry.sourceAuthority)) continue;
      if (deps.source.alignedStart(missing) !== failure.retry.start) continue;
      return failure.retry.message;
    }
    return undefined;
  };

  const presentStatus = (): void => {
    const range = admittedRange;
    if (!range || !deps.source.isCurrent(range.authority)) return;
    if (range.end <= range.start) return;
    const missing = firstMissing(range);
    if (missing === undefined) {
      deps.setRangeStatus(
        `Ready · ${deps.formatPhotoCount(deps.source.total())}`,
      );
      return;
    }
    const failure = failureStatus(missing);
    if (failure !== undefined) {
      deps.setRangeStatus(failure);
      return;
    }
    const window = deps.source.describeWindow(missing);
    deps.setRangeStatus(
      `Loading ${window.range} of ${deps.source.total().toLocaleString()}…`,
    );
  };

  const fail = (
    ownerScope: "source" | "photo",
    generation: string,
    start: number,
    transportLost: boolean,
    retry?: GridRangeRetry,
    transition?: RecoveryTransition,
  ): void => {
    const key = `${ownerScope}:${generation}:${start}`;
    const active = failures.get(key);
    if (active && deps.gate.isActive(active.claim)) {
      deps.onFailure();
      return;
    }
    if (active) failures.delete(key);
    const owner = { scope: ownerScope, generation };
    let claim: RecoveryClaim | undefined;
    if (transition) {
      try {
        const replacement = deps.gate.issue("browse-window", key, {
          owner,
          transition,
        });
        if (
          deps.gate.failTransition(transition, replacement, { transportLost })
        )
          claim = replacement;
        else deps.gate.discard(replacement);
      } catch {
        // Superseded transitions cannot affect the current range.
      }
    } else {
      const candidate = deps.gate.issue("browse-window", key, { owner });
      if (deps.gate.fail(candidate, { transportLost })) claim = candidate;
      else deps.gate.discard(candidate);
    }
    if (claim)
      failures.set(key, {
        claim,
        ownerScope,
        ...(retry ? { retry } : {}),
      });
    deps.onFailure();
  };

  const recover = (
    ownerScope: "source" | "photo",
    generation: string,
    start: number,
  ): void => {
    const key = `${ownerScope}:${generation}:${start}`;
    const failure = failures.get(key);
    if (!failure) return;
    failures.delete(key);
    if (deps.gate.recover(failure.claim)) deps.onRecovered();
  };

  return {
    get admittedRange() {
      return admittedRange;
    },
    setAdmittedRange(range) {
      admittedRange = range;
    },
    presentStatus,
    fail,
    recover,
    clearInactive() {
      for (const [key, failure] of failures)
        if (!deps.gate.isActive(failure.claim)) failures.delete(key);
    },
    currentSourceRetries(alignedStart) {
      const retries: Array<
        Readonly<{ claim: RecoveryClaim; retry: GridRangeRetry }>
      > = [];
      for (const failure of failures.values()) {
        if (
          failure.ownerScope !== "source" ||
          !failure.retry ||
          !deps.gate.isActive(failure.claim) ||
          !deps.source.isCurrent(failure.retry.sourceAuthority) ||
          (alignedStart !== undefined && failure.retry.start !== alignedStart)
        )
          continue;
        retries.push({ claim: failure.claim, retry: failure.retry });
      }
      return retries;
    },
    windowAnchorIndex(windowStart) {
      if (deps.source.alignedStart(windowStart) === windowStart)
        return windowStart;
      for (let index = windowStart + 1; index < deps.source.total(); index += 1)
        if (deps.source.alignedStart(index) === windowStart) return index;
      return windowStart;
    },
  };
}
