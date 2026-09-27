import type {
  AlbumMembershipAddResult,
  AlbumMembershipRemoveResult,
} from "../api/album-actions.js";
import type { SelectionState } from "../api/contracts.js";
import type { PhotoStateBatchPhoto } from "../api/photo.js";
import type {
  AlbumActionAdmission,
  AlbumActionContext,
  AlbumActionOwner,
} from "./album-action-owner.js";
import type { PhotoAuthority, PhotoOwner } from "./photo-owner.js";
import type {
  SourceAuthority,
  SourceGridOwner,
  SourceWindowOperation,
} from "./source-grid-owner.js";
import type { BatchAlbumsViewModel } from "../ui/library-browser-view.js";

const MULTI_SELECTION_LIMIT = 100;

type GridBatchResult = Readonly<{
  tone: "success" | "warning" | "failure";
  message: string;
  review?: Readonly<{ label: string }>;
  compensation?: Readonly<{ label: string }>;
}>;

type BatchAlbumCompensation = Readonly<{
  albumId: string;
  albumName: string;
  photoIds: ReadonlyArray<string>;
  sourceAuthority: SourceAuthority;
  hadSavedPosition: boolean;
}>;

type GridAlbumMutationResult = Readonly<{
  admitted: boolean;
  ok: boolean;
  latest: boolean;
  membershipAdd?: Omit<AlbumMembershipAddResult, "kind">;
  membershipRemove?: Omit<AlbumMembershipRemoveResult, "kind">;
}>;

type GridAlbumMutation = (
  start: (context: AlbumActionContext) => AlbumActionAdmission | undefined,
) => Promise<GridAlbumMutationResult>;

export type GridMultiSelectionModel = Readonly<{
  mode: boolean;
  count: number;
  limit: number;
  enabled: boolean;
  result?: GridBatchResult;
  selected(index: number): boolean;
}>;

export type GridMultiSelectionPresentation = Readonly<{
  renderGrid: (position?: number) => void;
  renderBatchAlbums: (model: BatchAlbumsViewModel) => void;
  resetGridMultiSelection: () => void;
  setDecisionStatus: (text: string) => void;
  focusGridIndex: (index: number) => void;
  updateControls: () => void;
}>;

export type GridMultiSelectionCoordination = Readonly<{
  alive: () => boolean;
  connected: () => boolean;
  pageBusy: () => boolean;
  setPageBusy: (value: boolean) => void;
  gridVisible: () => boolean;
  canOpenGridPhoto: () => boolean;
  sourceGrid: SourceGridOwner;
  photoOwner: PhotoOwner;
  albumActions: AlbumActionOwner;
  albums: () => ReadonlyArray<
    Readonly<{ id: string; name: string; hasSavedPosition: boolean }>
  >;
  mutateAlbum: GridAlbumMutation;
  membershipAlbumName: (albumId: string) => string;
  loadWindow: (
    index: number,
    operation: SourceWindowOperation,
    quiet: boolean,
    priority: "high" | "low",
  ) => Promise<boolean>;
  reopenExpired: (
    anchorIndex: number,
    expectedGeneration: number,
  ) => Promise<void>;
  failPhotoRecovery: (authority: PhotoAuthority, kind: string) => void;
}>;

export type GridMultiSelectionOwnerOptions = Readonly<{
  present: GridMultiSelectionPresentation;
  coordinate: GridMultiSelectionCoordination;
}>;

export interface GridMultiSelectionOwner {
  model(): GridMultiSelectionModel;
  renderBatchAlbums(): void;
  clear(): void;
  setMode(mode: boolean): void;
  toggle(photoId: string): void;
  extend(index: number, photoId: string): void;
  noteExpectedSelection(photoId: string, value: SelectionState): void;
  expireCompensation(): void;
  expireCompensationForAlbum(albumId: string): boolean;
  mutate(value: SelectionState): Promise<void>;
  reviewChanged(): Promise<void>;
  addToAlbum(albumId: string): Promise<void>;
  removeAddedPhotos(): Promise<void>;
  performBatchUndo(): Promise<void>;
  dispose(): void;
}

export function createGridMultiSelectionOwner(
  options: GridMultiSelectionOwnerOptions,
): GridMultiSelectionOwner {
  const { present, coordinate } = options;
  const {
    sourceGrid,
    photoOwner,
    albumActions,
    mutateAlbum,
    membershipAlbumName,
  } = coordinate;
  let multiSelection = new Set<string>();
  let multiMissingIds = new Set<string>();
  let multiChangedIds = new Set<string>();
  let multiExpectedSelection = new Map<string, SelectionState>();
  let multiAnchorId: string | undefined;
  let selectMode = false;
  let gridBatchResult: GridBatchResult | undefined;
  let batchAlbumCompensation: BatchAlbumCompensation | undefined;
  let batchAlbumPending = false;
  let closed = false;

  const photoCountText = (count: number): string =>
    `${count.toLocaleString()} ${count === 1 ? "Photo" : "Photos"}`;

  const renderBatchAlbums = (): void => {
    if (closed || !coordinate.alive()) return;
    present.renderBatchAlbums({
      albums: coordinate.albums().map(({ id, name }) => ({ id, name })),
      pending: batchAlbumPending,
    });
  };

  const render = (position?: number): void => {
    if (closed || !coordinate.alive()) return;
    present.renderGrid(position);
  };

  const expireBatchAlbumCompensation = (): void => {
    batchAlbumCompensation = undefined;
    if (!gridBatchResult?.compensation) return;
    gridBatchResult = {
      tone: gridBatchResult.tone,
      message: gridBatchResult.message,
      ...(gridBatchResult.review ? { review: gridBatchResult.review } : {}),
    };
  };

  const clear = (): void => {
    multiSelection = new Set();
    multiMissingIds = new Set();
    multiChangedIds = new Set();
    multiExpectedSelection = new Map();
    multiAnchorId = undefined;
    selectMode = false;
    gridBatchResult = undefined;
    batchAlbumCompensation = undefined;
    present.resetGridMultiSelection();
  };

  const refuseBeyondBatchBound = (): void => {
    present.setDecisionStatus(
      "Selection limit reached. Remove a Photo to extend the range.",
    );
  };

  const toggle = (photoId: string): void => {
    if (multiSelection.delete(photoId)) {
      multiExpectedSelection.delete(photoId);
      multiMissingIds.delete(photoId);
      multiChangedIds.delete(photoId);
    } else {
      if (multiSelection.size >= MULTI_SELECTION_LIMIT) {
        refuseBeyondBatchBound();
        return;
      }
      const photoIndex = sourceGrid.findPhotoIndex(photoId);
      const photo =
        photoIndex === undefined ? undefined : sourceGrid.photoAt(photoIndex);
      if (!photo) return;
      multiSelection.add(photoId);
      multiMissingIds.delete(photoId);
      multiExpectedSelection.set(photoId, photo.selectionState);
    }
    gridBatchResult = undefined;
    multiAnchorId = photoId;
    render();
  };

  const extend = (index: number, photoId: string): void => {
    const anchorIndex =
      multiAnchorId === undefined
        ? undefined
        : sourceGrid.findPhotoIndex(multiAnchorId);
    if (anchorIndex === undefined) {
      if (multiSelection.size >= MULTI_SELECTION_LIMIT) {
        refuseBeyondBatchBound();
        return;
      }
      const photo = sourceGrid.photoAt(index);
      if (!photo) return;
      multiSelection.add(photoId);
      multiMissingIds.delete(photoId);
      multiChangedIds.delete(photoId);
      multiExpectedSelection.set(photoId, photo.selectionState);
      gridBatchResult = undefined;
      multiAnchorId = photoId;
      render();
      return;
    }
    const joined: string[] = [];
    for (
      let position = Math.min(anchorIndex, index);
      position <= Math.max(anchorIndex, index);
      position += 1
    ) {
      const photo = sourceGrid.photoAt(position);
      if (photo && !multiSelection.has(photo.id)) joined.push(photo.id);
    }
    if (multiSelection.size + joined.length > MULTI_SELECTION_LIMIT) {
      refuseBeyondBatchBound();
      return;
    }
    for (const photoIdToAdd of joined) {
      const indexToAdd = sourceGrid.findPhotoIndex(photoIdToAdd);
      const photo =
        indexToAdd === undefined ? undefined : sourceGrid.photoAt(indexToAdd);
      if (!photo) continue;
      multiSelection.add(photoIdToAdd);
      multiMissingIds.delete(photoIdToAdd);
      multiChangedIds.delete(photoIdToAdd);
      multiExpectedSelection.set(photoIdToAdd, photo.selectionState);
    }
    gridBatchResult = undefined;
    multiAnchorId = photoId;
    render();
  };

  const mutate = async (value: SelectionState): Promise<void> => {
    if (
      !coordinate.connected() ||
      coordinate.pageBusy() ||
      !coordinate.gridVisible() ||
      !coordinate.canOpenGridPhoto()
    )
      return;
    const photoIds = [...multiSelection].filter(
      (photoId) => !multiMissingIds.has(photoId),
    );
    if (photoIds.length === 0) {
      const message =
        "No selected Photos remain in this Library. Clear the selection to continue.";
      gridBatchResult = { tone: "warning", message };
      present.setDecisionStatus(message);
      render();
      return;
    }
    const photos = photoIds.flatMap<PhotoStateBatchPhoto>((photoId) => {
      const expectedCurrent = multiExpectedSelection.get(photoId);
      return expectedCurrent === undefined
        ? []
        : [{ photoId, expectedCurrent }];
    });
    if (photos.length !== photoIds.length) {
      const message =
        "The selected Photos need a refresh before this batch can be retried.";
      gridBatchResult = { tone: "failure", message };
      present.setDecisionStatus(message);
      render();
      return;
    }
    const admission = photoOwner.mutateBatch(photos, value);
    if (!admission) return;
    gridBatchResult = undefined;
    render();
    present.setDecisionStatus(`Saving ${photoCountText(photoIds.length)}…`);
    const outcome = await admission.settlement;
    render();
    if (outcome.kind === "detached") return;
    if (outcome.kind === "failed") {
      if (outcome.failure === "answered") {
        present.setDecisionStatus(
          outcome.status === 400
            ? `The change could not be saved. A batch holds up to ${MULTI_SELECTION_LIMIT} Photos.`
            : "The change could not be saved.",
        );
      } else {
        present.setDecisionStatus(
          "Connection lost before the change was confirmed. Retry to refresh.",
        );
      }
      if (outcome.connectivity === "lost")
        coordinate.failPhotoRecovery(photoOwner.authority, "photo-write");
      gridBatchResult = {
        tone: "failure",
        message:
          outcome.failure === "transport"
            ? "Connection lost before the batch was confirmed. Retry to refresh."
            : "The batch could not be saved. Retry to refresh the selected Photos.",
      };
      render();
      present.updateControls();
      return;
    }
    const persisted = outcome;
    const applied = persisted.applied.length;
    const changed = persisted.changedElsewhere.length;
    const missing = persisted.missing.length;
    multiChangedIds = new Set(
      persisted.changedElsewhere.map((entry) => entry.photoId),
    );
    for (const entry of persisted.missing) {
      multiMissingIds.add(entry.photoId);
      multiExpectedSelection.delete(entry.photoId);
    }
    const decision = value === "selected" ? "selected" : "rejected";
    const resumeMessage =
      sourceGrid.kind === "album" ? " Album resume point unchanged." : "";
    for (const entry of persisted.applied)
      multiExpectedSelection.set(entry.photoId, value);
    const parts = [`${photoCountText(applied)} ${decision}.`];
    if (changed > 0)
      parts.push(
        `${photoCountText(changed)} changed elsewhere. Review them before retrying.`,
      );
    if (missing > 0)
      parts.push(`${photoCountText(missing)} no longer in this Library.`);
    const message = `${parts.join(" ")}${resumeMessage}`;
    gridBatchResult = {
      tone: changed > 0 || missing > 0 ? "warning" : "success",
      message,
      ...(changed > 0
        ? { review: { label: `Review ${photoCountText(changed)}` } }
        : {}),
    };
    present.setDecisionStatus(message);
    render();
    present.updateControls();
  };

  const reviewChanged = async (): Promise<void> => {
    if (
      closed ||
      !coordinate.alive() ||
      !coordinate.connected() ||
      coordinate.pageBusy() ||
      !coordinate.gridVisible() ||
      multiChangedIds.size === 0
    )
      return;
    const reviewIds = [...multiChangedIds].filter(
      (photoId) => multiSelection.has(photoId) && !multiMissingIds.has(photoId),
    );
    if (reviewIds.length === 0) return;
    const sourceAuthority = sourceGrid.authority;
    const loadedWindows = new Set<number>();
    const reviewed = new Set<string>();
    const becameMissing = new Set<string>();
    let firstReviewedIndex: number | undefined;
    let failed = false;
    coordinate.setPageBusy(true);
    render();
    present.setDecisionStatus(
      `Reviewing ${photoCountText(reviewIds.length)} changed elsewhere…`,
    );
    try {
      for (const photoId of reviewIds) {
        if (!sourceGrid.isCurrent(sourceAuthority)) return;
        let index = sourceGrid.findPhotoIndex(photoId);
        if (index === undefined) {
          const position = await sourceGrid.resolvePhotoPosition(
            sourceAuthority,
            photoId,
          );
          if (!sourceGrid.isCurrent(sourceAuthority)) return;
          if (position.kind === "missing") {
            becameMissing.add(photoId);
            continue;
          }
          if (position.kind === "expired") {
            await coordinate.reopenExpired(
              sourceGrid.readGridPosition(sourceAuthority) ?? 0,
              sourceGrid.generation,
            );
            return;
          }
          if (position.kind !== "resolved") {
            failed = true;
            break;
          }
          index = position.position;
        }
        const believedState = multiExpectedSelection.get(photoId);
        const { start } = sourceGrid.describeWindow(index);
        const refreshWindow = async (): Promise<boolean> => {
          sourceGrid.invalidateWindow(index);
          const loaded = await coordinate.loadWindow(
            index,
            { kind: "grid", authority: sourceAuthority },
            true,
            "high",
          );
          if (!loaded || !sourceGrid.isCurrent(sourceAuthority)) return false;
          loadedWindows.add(start);
          return true;
        };
        if (!loadedWindows.has(start) && !(await refreshWindow())) {
          failed = true;
          break;
        }
        let refreshedIndex = sourceGrid.findPhotoIndex(photoId);
        if (refreshedIndex === undefined && !(await refreshWindow())) {
          failed = true;
          break;
        }
        refreshedIndex = sourceGrid.findPhotoIndex(photoId);
        let photo =
          refreshedIndex === undefined
            ? undefined
            : sourceGrid.photoAt(refreshedIndex);
        if (!photo) {
          const currentPosition = await sourceGrid.resolvePhotoPosition(
            sourceAuthority,
            photoId,
          );
          if (!sourceGrid.isCurrent(sourceAuthority)) return;
          if (currentPosition.kind === "missing") {
            becameMissing.add(photoId);
            continue;
          }
          if (currentPosition.kind === "expired") {
            await coordinate.reopenExpired(
              sourceGrid.readGridPosition(sourceAuthority) ?? 0,
              sourceGrid.generation,
            );
            return;
          }
          if (currentPosition.kind !== "resolved") {
            failed = true;
            break;
          }
          if (!(await refreshWindow())) {
            failed = true;
            break;
          }
          refreshedIndex = sourceGrid.findPhotoIndex(photoId);
          photo =
            refreshedIndex === undefined
              ? undefined
              : sourceGrid.photoAt(refreshedIndex);
          if (!photo) {
            failed = true;
            break;
          }
        }
        if (
          believedState !== undefined &&
          !sourceGrid.reconcilePhotoSelection(
            sourceAuthority,
            refreshedIndex!,
            photoId,
            believedState,
            photo.selectionState,
          )
        ) {
          failed = true;
          break;
        }
        multiExpectedSelection.set(photoId, photo.selectionState);
        reviewed.add(photoId);
        firstReviewedIndex ??= refreshedIndex;
      }
    } finally {
      if (sourceGrid.isCurrent(sourceAuthority)) {
        coordinate.setPageBusy(false);
        present.updateControls();
      }
    }
    if (closed || !coordinate.alive() || !sourceGrid.isCurrent(sourceAuthority))
      return;
    for (const photoId of becameMissing) {
      multiMissingIds.add(photoId);
      multiExpectedSelection.delete(photoId);
      multiChangedIds.delete(photoId);
    }
    for (const photoId of reviewed) multiChangedIds.delete(photoId);
    const remaining = [...multiChangedIds].filter(
      (photoId) => multiSelection.has(photoId) && !multiMissingIds.has(photoId),
    );
    if (failed) {
      const missingMessage =
        becameMissing.size > 0
          ? ` ${photoCountText(becameMissing.size)} no longer in this Library.`
          : "";
      const message = `Some changed Photos could not be refreshed.${missingMessage} Retry Review to continue.`;
      gridBatchResult = {
        tone: "warning",
        message,
        ...(remaining.length > 0
          ? { review: { label: `Review ${photoCountText(remaining.length)}` } }
          : {}),
      };
      present.setDecisionStatus(message);
    } else {
      const reviewedCount = reviewed.size;
      const missingMessage =
        becameMissing.size > 0
          ? ` ${photoCountText(becameMissing.size)} no longer in this Library.`
          : "";
      const message = `${photoCountText(reviewedCount)} reviewed.${missingMessage} Retry the batch when ready.`;
      gridBatchResult = {
        tone: becameMissing.size > 0 ? "warning" : "success",
        message,
      };
      present.setDecisionStatus(message);
    }
    render(firstReviewedIndex);
    if (firstReviewedIndex !== undefined)
      present.focusGridIndex(firstReviewedIndex);
    present.updateControls();
  };

  const addToAlbum = async (albumId: string): Promise<void> => {
    expireBatchAlbumCompensation();
    if (closed || !coordinate.alive() || batchAlbumPending || !albumId) return;
    if (multiSelection.size === 0) return;
    const albumBefore = coordinate
      .albums()
      .find((album) => album.id === albumId);
    if (!albumBefore) return;
    const photoIds = [...multiSelection].filter(
      (photoId) => !multiMissingIds.has(photoId),
    );
    if (photoIds.length === 0) {
      const message =
        "No selected Photos remain in this Library. Clear the selection to continue.";
      gridBatchResult = { tone: "warning", message };
      present.setDecisionStatus(message);
      render();
      return;
    }
    const sourceAuthority = sourceGrid.authority;
    const name = membershipAlbumName(albumId);
    batchAlbumPending = true;
    gridBatchResult = undefined;
    render();
    renderBatchAlbums();
    present.setDecisionStatus(
      `Adding ${photoCountText(photoIds.length)} to “${name}”…`,
    );
    const result = await mutateAlbum((context) =>
      albumActions.addMemberships(albumId, photoIds, context),
    );
    batchAlbumPending = false;
    if (closed || !coordinate.alive()) return;
    renderBatchAlbums();
    if (
      !result.admitted ||
      !result.latest ||
      !sourceGrid.isCurrent(sourceAuthority)
    )
      return;
    let message: string;
    let compensation: GridBatchResult["compensation"];
    if (result.ok && result.membershipAdd) {
      const added = result.membershipAdd.addedPhotoIds.length;
      const alreadyMember = result.membershipAdd.alreadyMemberPhotoIds.length;
      const parts = [
        ...(added > 0 ? [`${photoCountText(added)} added to “${name}”.`] : []),
        ...(alreadyMember > 0
          ? [`${photoCountText(alreadyMember)} already in “${name}”.`]
          : []),
      ];
      if (sourceGrid.kind === "album")
        parts.push("Album resume point unchanged.");
      message = parts.join(" ");
      if (added > 0) {
        batchAlbumCompensation = Object.freeze({
          albumId,
          albumName: name,
          photoIds: Object.freeze([...result.membershipAdd.addedPhotoIds]),
          sourceAuthority,
          hadSavedPosition: albumBefore.hasSavedPosition,
        });
        compensation = { label: "Remove added Photos" };
      } else batchAlbumCompensation = undefined;
    } else {
      batchAlbumCompensation = undefined;
      message = result.ok
        ? `${photoCountText(photoIds.length)} added to “${name}”.${
            sourceGrid.kind === "album" ? " Album resume point unchanged." : ""
          }`
        : `Could not add the selected Photos to “${name}”.`;
    }
    gridBatchResult = {
      tone: result.ok ? "success" : "failure",
      message,
      ...(compensation ? { compensation } : {}),
    };
    present.setDecisionStatus(message);
    render();
  };

  const removeAddedPhotos = async (): Promise<void> => {
    const compensation = batchAlbumCompensation;
    if (
      !compensation ||
      batchAlbumPending ||
      closed ||
      !coordinate.alive() ||
      !sourceGrid.isCurrent(compensation.sourceAuthority)
    )
      return;
    const savedPositionBefore =
      coordinate.albums().find((album) => album.id === compensation.albumId)
        ?.hasSavedPosition ?? compensation.hadSavedPosition;
    batchAlbumPending = true;
    render();
    renderBatchAlbums();
    present.setDecisionStatus(
      `Removing ${photoCountText(compensation.photoIds.length)} added Photos from “${compensation.albumName}”…`,
    );
    const result = await mutateAlbum((context) =>
      albumActions.removeAddedMemberships(
        compensation.albumId,
        compensation.photoIds,
        context,
      ),
    );
    batchAlbumPending = false;
    if (closed || !coordinate.alive()) return;
    renderBatchAlbums();
    if (
      !result.admitted ||
      !result.latest ||
      batchAlbumCompensation !== compensation ||
      !sourceGrid.isCurrent(compensation.sourceAuthority)
    ) {
      render();
      return;
    }
    if (!result.ok || !result.membershipRemove) {
      const message = `Could not remove the added Photos from “${compensation.albumName}”. Retry to continue.`;
      gridBatchResult = {
        tone: "failure",
        message,
        compensation: { label: "Remove added Photos" },
      };
      present.setDecisionStatus(message);
      render();
      return;
    }
    const removed = result.membershipRemove.removedPhotoIds.length;
    const absent = result.membershipRemove.alreadyAbsentPhotoIds.length;
    const currentAlbum = coordinate
      .albums()
      .find((album) => album.id === compensation.albumId);
    const savedPositionMessage =
      savedPositionBefore && !currentAlbum?.hasSavedPosition
        ? " Album resume point cleared."
        : savedPositionBefore && currentAlbum?.hasSavedPosition
          ? " Album resume point remains."
          : "";
    const message = [
      `${photoCountText(removed)} removed from “${compensation.albumName}”.`,
      ...(absent > 0
        ? [
            `${photoCountText(absent)} already absent from “${compensation.albumName}”.`,
          ]
        : []),
      savedPositionMessage.trim(),
    ]
      .filter(Boolean)
      .join(" ");
    batchAlbumCompensation = undefined;
    gridBatchResult = {
      tone: absent > 0 ? "warning" : "success",
      message,
    };
    present.setDecisionStatus(message);
    render();
  };

  const performBatchUndo = async (): Promise<void> => {
    const preparation = photoOwner.prepareBatchUndo();
    if (!preparation) return;
    present.updateControls();
    present.setDecisionStatus(
      `Restoring ${photoCountText(preparation.count)}…`,
    );
    const outcome = await photoOwner.performBatchUndo(preparation);
    render();
    present.updateControls();
    if (outcome.kind === "detached") return;
    if (outcome.connectivity === "lost")
      coordinate.failPhotoRecovery(photoOwner.authority, "undo");
    const restored = outcome.restored.length;
    const conflicts = outcome.conflicts.length;
    const failed = outcome.failed.length;
    for (const entry of outcome.restoredValues) {
      if (multiSelection.has(entry.photoId))
        multiExpectedSelection.set(entry.photoId, entry.value);
    }
    const message =
      failed > 0
        ? `${photoCountText(restored)} restored. ${photoCountText(failed)} not restored; Undo again to retry.`
        : conflicts > 0
          ? `${photoCountText(restored)} restored. ${photoCountText(conflicts)} could not be restored because ${conflicts === 1 ? "it changed" : "they changed"} elsewhere.`
          : `${photoCountText(restored)} restored.`;
    gridBatchResult = {
      tone: failed > 0 || conflicts > 0 ? "warning" : "success",
      message,
    };
    present.setDecisionStatus(message);
    render();
  };

  return {
    model: () => ({
      mode: selectMode,
      count: multiSelection.size,
      limit: MULTI_SELECTION_LIMIT,
      enabled: coordinate.connected() && coordinate.canOpenGridPhoto(),
      ...(gridBatchResult ? { result: gridBatchResult } : {}),
      selected: (index: number) => {
        const photo = sourceGrid.photoAt(index);
        return photo !== undefined && multiSelection.has(photo.id);
      },
    }),
    renderBatchAlbums,
    clear,
    setMode: (mode) => {
      if (closed || selectMode === mode) return;
      selectMode = mode;
      render();
    },
    toggle,
    extend,
    noteExpectedSelection: (photoId, value) => {
      if (multiSelection.has(photoId))
        multiExpectedSelection.set(photoId, value);
    },
    expireCompensation: expireBatchAlbumCompensation,
    expireCompensationForAlbum: (albumId) => {
      if (batchAlbumCompensation?.albumId !== albumId) return false;
      expireBatchAlbumCompensation();
      return true;
    },
    mutate,
    reviewChanged,
    addToAlbum,
    removeAddedPhotos,
    performBatchUndo,
    dispose: () => {
      closed = true;
    },
  };
}
