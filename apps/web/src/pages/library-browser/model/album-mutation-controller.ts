import type { AlbumSummary } from "../api/contracts.js";
import type {
  AlbumActionAdmission,
  AlbumActionContext,
  AlbumActionOwner,
  AlbumFormAuthority,
} from "./album-action-owner.js";
import type { ApplicationOwner } from "./application-owner.js";
import type { PhotoAuthority } from "./photo-owner.js";
import type { SourceGridOwner, SourceAuthority } from "./source-grid-owner.js";
import type { RecoveryGate, RecoveryClaim } from "./async-ownership.js";

export type AlbumMutationResult = Readonly<{
  admitted: boolean;
  ok: boolean;
  latest: boolean;
  announce: (text: string) => void;
  removedFromCurrentAlbum?: Readonly<{
    albumId: string;
    photoId: string;
    sourceAuthority: SourceAuthority;
  }>;
  createdAlbum?: AlbumSummary;
  folderAdd?: Readonly<{
    matchedCount: number;
    addedCount: number;
    alreadyMemberCount: number;
  }>;
  membershipAdd?: Readonly<{
    albumId: string;
    addedPhotoIds: ReadonlyArray<string>;
    alreadyMemberPhotoIds: ReadonlyArray<string>;
    albums: ReadonlyArray<AlbumSummary>;
  }>;
  membershipRemove?: Readonly<{
    albumId: string;
    removedPhotoIds: ReadonlyArray<string>;
    alreadyAbsentPhotoIds: ReadonlyArray<string>;
    albums: ReadonlyArray<AlbumSummary>;
  }>;
}>;

export function createAlbumMutationController(
  options: Readonly<{
    actions: AlbumActionOwner;
    application: ApplicationOwner;
    source: Pick<SourceGridOwner, "authority" | "generation" | "isCurrent">;
    gate: RecoveryGate;
    photo: Readonly<{
      readonly authority: PhotoAuthority;
      readonly active: boolean;
      isCurrent(authority: PhotoAuthority): boolean;
    }>;
    presentation: Readonly<{
      readonly surface: object;
      isCurrent(surface: object): boolean;
      status(message: string): void;
    }>;
    connectionChanged(): void;
    reachable(): void;
  }>,
) {
  let closed = false;
  let recovery:
    | Readonly<{ claim: RecoveryClaim; sourceAuthority: SourceAuthority }>
    | undefined;
  const clearInactive = () => {
    if (
      recovery &&
      (!options.source.isCurrent(recovery.sourceAuthority) ||
        !options.gate.isActive(recovery.claim))
    )
      recovery = undefined;
  };
  const disconnect = (authority: SourceAuthority, key: string) => {
    if (!options.source.isCurrent(authority)) return;
    clearInactive();
    if (recovery?.sourceAuthority === authority) {
      options.gate.fail(recovery.claim, { transportLost: true });
    } else {
      const claim = options.gate.issue("album", key, {
        owner: {
          scope: "source",
          generation: String(options.source.generation),
        },
      });
      if (options.gate.fail(claim, { transportLost: true }))
        recovery = { claim, sourceAuthority: authority };
      else options.gate.discard(claim);
    }
    options.connectionChanged();
  };
  const recover = (authority: SourceAuthority) => {
    clearInactive();
    if (recovery?.sourceAuthority !== authority) return;
    if (
      options.gate.recover(recovery.claim) ||
      !options.gate.isActive(recovery.claim)
    )
      recovery = undefined;
  };
  return {
    clearInactive,
    async mutate(
      start: (context: AlbumActionContext) => AlbumActionAdmission | undefined,
      surface: "photo" | "summary",
      photoAuthority = options.photo.authority,
      form?: AlbumFormAuthority,
    ): Promise<AlbumMutationResult> {
      const silent = {
        admitted: false,
        ok: false,
        latest: false,
        announce: () => {},
      };
      if (closed) return silent;
      const statusSurface = options.presentation.surface;
      const ownsPhotoSurface = () =>
        !closed &&
        surface === "photo" &&
        options.photo.isCurrent(photoAuthority) &&
        options.photo.active &&
        options.presentation.isCurrent(statusSurface);
      const action = start({
        sourceAuthority: options.source.authority,
        surface:
          surface === "photo"
            ? { kind: "photo", isCurrent: ownsPhotoSurface }
            : { kind: "summary" },
        ...(form ? { form } : {}),
      });
      if (!action) return silent;
      const summary = options.application.claimAlbumSummary(action.noticeKey);
      try {
        const outcome = await action.settlement;
        if (closed)
          return {
            ...silent,
            admitted: true,
            ok: outcome.kind === "persisted",
            latest: options.actions.isLatest(outcome.mutation),
          };
        if (outcome.kind === "failed") {
          const photo = options.actions.canPresent(outcome.surface);
          if (photo) {
            options.presentation.status(outcome.failureMessage);
            options.application.releaseAlbumSummary(summary);
          } else
            options.application.presentAlbumSummary(
              summary,
              outcome.failureMessage,
            );
          if (outcome.connectivity === "lost-if-latest") {
            if (action.invalidatesSavedPositionFor)
              options.application.invalidateSavedPositionAuthority(
                action.invalidatesSavedPositionFor,
              );
            if (options.actions.isLatest(outcome.mutation)) {
              options.application.advanceAlbumMutationFloor();
              disconnect(outcome.sourceAuthority, action.noticeKey);
            }
          }
          return {
            ...silent,
            admitted: true,
            latest: options.actions.isLatest(outcome.mutation),
          };
        }
        let disconnectAfterRefresh = false;
        if (action.invalidatesSavedPositionFor)
          options.application.invalidateSavedPositionAuthority(
            action.invalidatesSavedPositionFor,
          );
        if (options.application.advanceAlbumMutationFloor()) {
          try {
            const committed = await options.application.refreshOverview();
            if (closed)
              return {
                ...silent,
                admitted: true,
                ok: true,
                latest: options.actions.isLatest(outcome.mutation),
              };
            if (
              committed &&
              options.actions.isLatest(outcome.mutation) &&
              options.source.isCurrent(outcome.sourceAuthority)
            ) {
              recover(outcome.sourceAuthority);
              options.reachable();
            }
            options.application.resolveAlbumSummary(summary);
          } catch {
            if (!closed && options.actions.isLatest(outcome.mutation)) {
              disconnectAfterRefresh = true;
              options.application.presentAlbumSummary(
                summary,
                "The Album was saved but the Library summary could not be refreshed.",
              );
            } else options.application.releaseAlbumSummary(summary);
          }
        }
        const present = options.actions.canPresent(outcome.surface);
        if (disconnectAfterRefresh)
          disconnect(outcome.sourceAuthority, action.noticeKey);
        return {
          admitted: true,
          ok: true,
          latest: options.actions.isLatest(outcome.mutation),
          announce(text) {
            if (present && ownsPhotoSurface())
              options.presentation.status(text);
          },
          ...(outcome.removedFromCurrentAlbum
            ? { removedFromCurrentAlbum: outcome.removedFromCurrentAlbum }
            : {}),
          ...(outcome.createdAlbum
            ? { createdAlbum: outcome.createdAlbum }
            : {}),
          ...(outcome.folderAdd
            ? {
                folderAdd: {
                  matchedCount: outcome.folderAdd.matchedCount,
                  addedCount: outcome.folderAdd.addedCount,
                  alreadyMemberCount: outcome.folderAdd.alreadyMemberCount,
                },
              }
            : {}),
          ...(outcome.membershipAdd
            ? { membershipAdd: outcome.membershipAdd }
            : {}),
          ...(outcome.membershipRemove
            ? { membershipRemove: outcome.membershipRemove }
            : {}),
        };
      } finally {
        options.actions.finish(action.mutation);
      }
    },
    dispose() {
      if (closed) return;
      closed = true;
      recovery = undefined;
    },
  };
}
