import {
  saveAlbumPosition,
  type SavedPositionFetch,
  type SavedPositionWriteResult,
} from "../api/saved-position.js";
import type { PhotoAuthority } from "./photo-owner.js";
import type { SourceAuthority } from "./source-grid-owner.js";

export type { SavedPositionFetch } from "../api/saved-position.js";

export type SavedPositionTarget = Readonly<{
  sourceAuthority: SourceAuthority;
  photoAuthority: PhotoAuthority;
  albumId: string;
  photoId: string;
}>;

export interface SavedPositionAuthorityPort {
  isSourceCurrent(authority: SourceAuthority, albumId: string): boolean;
  isPhotoCurrent(authority: PhotoAuthority, photoId: string): boolean;
}

export type SavedPositionOutcome = Readonly<{ target: SavedPositionTarget }> &
  (
    | Readonly<{ kind: "confirmed" }>
    | Readonly<{ kind: "stale"; status: 404 | 409 }>
    | Readonly<{
        kind: "failed";
        status?: number;
        transportLost: boolean;
      }>
    | Readonly<{ kind: "skipped" | "detached" }>
  );

type SavedPositionOutcomeValue =
  | Readonly<{ kind: "confirmed" }>
  | Readonly<{ kind: "stale"; status: 404 | 409 }>
  | Readonly<{ kind: "failed"; status?: number; transportLost: boolean }>
  | Readonly<{ kind: "skipped" | "detached" }>;

export type SavedPositionAdmission = Readonly<{
  target: SavedPositionTarget;
  settlement: Promise<SavedPositionOutcome>;
}>;

export interface SavedPositionOwner {
  save(target: SavedPositionTarget): SavedPositionAdmission | undefined;
  isCurrent(target: SavedPositionTarget): boolean;
  dispose(): void;
}

export function createSavedPositionOwner(
  fetcher: SavedPositionFetch,
  authorities: SavedPositionAuthorityPort,
): SavedPositionOwner {
  let closed = false;
  let queue: Promise<void> = Promise.resolve();

  const isCurrent = (target: SavedPositionTarget): boolean =>
    !closed &&
    authorities.isSourceCurrent(target.sourceAuthority, target.albumId) &&
    authorities.isPhotoCurrent(target.photoAuthority, target.photoId);

  const outcome = (
    target: SavedPositionTarget,
    value: SavedPositionOutcomeValue,
  ): SavedPositionOutcome =>
    Object.freeze({ target, ...value }) as SavedPositionOutcome;

  const classify = (
    target: SavedPositionTarget,
    result: SavedPositionWriteResult,
  ): SavedPositionOutcome => {
    if (!isCurrent(target)) return outcome(target, { kind: "detached" });
    if (result.kind === "persisted")
      return outcome(target, { kind: "confirmed" });
    if (result.status === 404 || result.status === 409)
      return outcome(target, { kind: "stale", status: result.status });
    return outcome(target, {
      kind: "failed",
      status: result.status,
      transportLost: false,
    });
  };

  const owner: SavedPositionOwner = {
    save: (candidate) => {
      if (closed) return undefined;
      const target = Object.freeze({ ...candidate });
      const settlement = queue.then(async (): Promise<SavedPositionOutcome> => {
        if (closed || !isCurrent(target))
          return outcome(target, { kind: "skipped" });
        try {
          return classify(
            target,
            await saveAlbumPosition(fetcher, target.albumId, target.photoId),
          );
        } catch {
          return isCurrent(target)
            ? outcome(target, { kind: "failed", transportLost: true })
            : outcome(target, { kind: "detached" });
        }
      });
      queue = settlement.then(
        () => undefined,
        () => undefined,
      );
      return Object.freeze({ target, settlement });
    },
    isCurrent,
    dispose: () => {
      closed = true;
    },
  };
  return owner;
}
