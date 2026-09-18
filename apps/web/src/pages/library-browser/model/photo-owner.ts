import type {
  PhotoSummary,
  PreviewResponse,
  SelectionState,
  UndoDescription,
} from "../api/contracts.js";
import {
  fetchPreview,
  persistPhotoState,
  persistPhotoStateBatch,
  type PhotoFetch,
  type PhotoStateBatchPhoto,
} from "../api/photo.js";
import { TaskScope } from "./async-ownership.js";
import type {
  PhotoWindowAuthority,
  SourceAuthority,
} from "./source-grid-owner.js";

export type { PhotoFetch } from "../api/photo.js";

declare const photoAuthorityBrand: unique symbol;
export type PhotoAuthority = Readonly<{ [photoAuthorityBrand]: true }>;

declare const undoOperationBrand: unique symbol;
type UndoOperation = Readonly<{ [undoOperationBrand]: true }>;

type PhotoField = "selectionState" | "rating";
type PhotoValue = SelectionState | number;
type SessionUndo = UndoDescription &
  Readonly<{ advanced: boolean; snapshotIndex: number }>;

export interface ReviewImageTransferPort {
  readonly connected: boolean;
  readonly source: string;
  setHandlers(onLoad: () => void, onError: () => void): void;
  clearHandlers(): void;
  setSource(resolvedUrl: string): void;
  clearSource(): void;
}

export interface PhotoSourcePort {
  isSourceCurrent(authority: SourceAuthority): boolean;
  renewPhotoWindow(
    sourceAuthority: SourceAuthority,
  ): PhotoWindowAuthority | undefined;
  photoAt(
    sourceAuthority: SourceAuthority,
    index: number,
  ): PhotoSummary | undefined;
  findPhotoIndex(photoId: string): number | undefined;
  movePosition(sourceAuthority: SourceAuthority, index: number): boolean;
  patchPreview(
    sourceAuthority: SourceAuthority,
    index: number,
    photoId: string,
    preview: PhotoSummary["preview"],
  ): boolean;
  patchSelection(
    sourceAuthority: SourceAuthority,
    index: number,
    photoId: string,
    selectionState: SelectionState,
  ): boolean;
  /// Applies one confirmed batch outcome by stable Photo identity. A loaded
  /// fact is patched and moves the source counts by the state the Grid
  /// believed; a Photo the Grid no longer holds moves the counts by the
  /// server's own prior value instead, so an evicted Photo still moves once.
  applyBatchSelection(
    sourceAuthority: SourceAuthority,
    photoId: string,
    priorValue: SelectionState,
    selectionState: SelectionState,
  ): boolean;
  patchRating(
    sourceAuthority: SourceAuthority,
    index: number,
    photoId: string,
    rating: number,
  ): boolean;
  trimFacts(sourceAuthority: SourceAuthority, anchor: number): void;
}

export type PhotoOwnerEvent = Readonly<{
  kind: "review-image-failed";
  authority: PhotoAuthority;
  photoId: string;
  surface: object;
}>;

export type PhotoSourceBinding = Readonly<{
  sourceAuthority: SourceAuthority;
  total: number;
  index: number;
  albumId?: string;
  preferredPhotoId?: string;
}>;

export type PhotoOperation = Readonly<{
  authority: PhotoAuthority;
  sourceAuthority: SourceAuthority;
  windowAuthority: PhotoWindowAuthority;
  index: number;
  photoId?: string;
}>;

export type PhotoPreviewOutcome =
  | (PhotoOperation &
      Readonly<{
        kind: "ready";
        preview: PreviewResponse & { state: "ready"; url: string };
      }>)
  | (PhotoOperation &
      Readonly<{
        kind: "not-ready";
        preview: PreviewResponse & { state: "unavailable" | "failed" };
      }>)
  | (PhotoOperation &
      Readonly<{
        kind: "failed";
        failure: "answered" | "malformed" | "transport";
        status?: number;
      }>)
  | (PhotoOperation & Readonly<{ kind: "detached" }>);

export type PhotoMutationOutcome = PhotoOperation &
  Readonly<{ field: PhotoField; advance: boolean }> &
  (
    | Readonly<{ kind: "persisted"; applied: boolean }>
    | Readonly<{
        kind: "failed";
        failure: "answered" | "malformed" | "transport";
        connectivity: "unchanged" | "lost";
        status?: number;
      }>
    | Readonly<{ kind: "detached" }>
  );

export type PhotoMutationAdmission = Readonly<{
  authority: PhotoAuthority;
  settlement: Promise<PhotoMutationOutcome>;
}>;

/// One bounded batch Selection State write from the Grid's multi-selection.
/// It shares the single write admission and the one-level Undo, and it reports
/// the per-Photo outcomes the Grid presents.
export type PhotoBatchOutcome =
  | Readonly<{
      kind: "persisted";
      value: SelectionState;
      applied: ReadonlyArray<
        Readonly<{ photoId: string; priorValue: SelectionState }>
      >;
      changedElsewhere: ReadonlyArray<
        Readonly<{ photoId: string; currentValue: SelectionState }>
      >;
      missing: ReadonlyArray<Readonly<{ photoId: string }>>;
    }>
  | Readonly<{
      kind: "failed";
      failure: "answered" | "malformed" | "transport";
      connectivity: "unchanged" | "lost";
      status?: number;
    }>
  | Readonly<{ kind: "detached" }>;

export type PhotoBatchAdmission = Readonly<{
  settlement: Promise<PhotoBatchOutcome>;
}>;

declare const batchUndoOperationBrand: unique symbol;
type BatchUndoOperation = Readonly<{ [batchUndoOperationBrand]: true }>;

export type PhotoBatchUndoPreparation = Readonly<{
  authority: PhotoAuthority;
  operation: BatchUndoOperation;
  count: number;
}>;

export type PhotoBatchUndoOutcome =
  | Readonly<{
      kind: "settled";
      restored: ReadonlyArray<string>;
      restoredValues: ReadonlyArray<
        Readonly<{ photoId: string; value: SelectionState }>
      >;
      conflicts: ReadonlyArray<string>;
      failed: ReadonlyArray<string>;
      connectivity: "unchanged" | "lost";
    }>
  | Readonly<{ kind: "detached" }>;

export type PhotoUndoPreparation = PhotoOperation &
  Readonly<{
    operation: UndoOperation;
    needsWindow: boolean;
    needsPosition: boolean;
  }>;

export type PhotoUndoOutcome =
  | (PhotoOperation & Readonly<{ kind: "persisted" }>)
  | (PhotoOperation &
      Readonly<{
        kind: "failed";
        failure: "answered" | "transport";
        connectivity: "unchanged" | "lost";
        status?: number;
        retryable: boolean;
      }>)
  | (PhotoOperation & Readonly<{ kind: "detached" }>);

export type PhotoRetry = PhotoOperation & Readonly<{ expectedPhotoId: string }>;

export interface PhotoOwner {
  readonly authority: PhotoAuthority;
  readonly sourceAuthority: SourceAuthority | undefined;
  readonly windowAuthority: PhotoWindowAuthority | undefined;
  readonly currentIndex: number;
  readonly total: number;
  readonly current: PhotoSummary | undefined;
  readonly lastCurrentPhotoId: string | undefined;
  readonly busy: boolean;
  readonly opening: boolean;
  readonly active: boolean;
  readonly canUndo: boolean;
  readonly undoPhotoId: string | undefined;
  isCurrent(authority: PhotoAuthority): boolean;
  ownsWindow(
    authority: PhotoAuthority,
    windowAuthority: PhotoWindowAuthority,
  ): boolean;
  bindSource(binding: PhotoSourceBinding): PhotoAuthority;
  rebindSource(binding: PhotoSourceBinding): PhotoAuthority;
  updateSource(binding: PhotoSourceBinding): boolean;
  beginOpen(index: number): PhotoOperation | undefined;
  commitOpen(open: PhotoOperation): PhotoSummary | undefined;
  cancelOpen(authority: PhotoAuthority): void;
  leave(): PhotoAuthority;
  loadCurrentPreview(authority: PhotoAuthority): Promise<PhotoPreviewOutcome>;
  prefetchAdjacent(authority: PhotoAuthority, index: number): Promise<void>;
  attachReviewImage(
    authority: PhotoAuthority,
    image: ReviewImageTransferPort,
    resolvedUrl: string,
    surface: object,
  ): boolean;
  mutate(
    field: PhotoField,
    value: PhotoValue,
    advance: boolean,
  ): PhotoMutationAdmission | undefined;
  mutateAt(
    index: number,
    field: PhotoField,
    value: PhotoValue,
  ): PhotoMutationAdmission | undefined;
  /// Whether the pending Undo action advanced away from its Photo. A Grid
  /// decision never advances, so Undo restores that cell in place.
  readonly undoAdvanced: boolean;
  /// Whether the pending Undo action is one batch Selection State change.
  readonly undoBatch: boolean;
  mutateBatch(
    photos: ReadonlyArray<PhotoStateBatchPhoto>,
    value: SelectionState,
  ): PhotoBatchAdmission | undefined;
  prepareBatchUndo(): PhotoBatchUndoPreparation | undefined;
  cancelBatchUndo(preparation: PhotoBatchUndoPreparation): void;
  performBatchUndo(
    preparation: PhotoBatchUndoPreparation,
  ): Promise<PhotoBatchUndoOutcome>;
  prepareUndo(resolvedIndex?: number): PhotoUndoPreparation | undefined;
  discardUndo(): void;
  cancelUndo(preparation: PhotoUndoPreparation): void;
  performUndo(preparation: PhotoUndoPreparation): Promise<PhotoUndoOutcome>;
  beginRetry(): PhotoRetry | undefined;
  retryIsCurrent(retry: PhotoRetry): boolean;
  retryPhotoIsCurrent(retry: PhotoRetry): boolean;
  finishRetry(retry: PhotoRetry): void;
  dispose(): void;
}

type Lifetime = Readonly<{
  authority: PhotoAuthority;
  sourceAuthority: SourceAuthority;
  windowAuthority: PhotoWindowAuthority;
  tasks: TaskScope;
}>;

type UndoRecord = Readonly<{
  operation: UndoOperation;
  preparation: PhotoUndoPreparation;
  action: SessionUndo;
}>;

/// The one-level Undo description for a batch Selection State change: the
/// value the batch wrote and the state each confirmed Photo held before it.
/// A Photo whose write failed is never part of it.
type SessionBatchUndo = Readonly<{
  value: SelectionState;
  entries: ReadonlyArray<
    Readonly<{ photoId: string; priorValue: SelectionState }>
  >;
}>;

type BatchUndoRecord = Readonly<{
  operation: BatchUndoOperation;
  preparation: PhotoBatchUndoPreparation;
  action: SessionBatchUndo;
}>;

type ReviewImageLease = Readonly<{
  image: ReviewImageTransferPort;
  resolvedUrl: string;
}>;

const detachedPreview = (operation: PhotoOperation): PhotoPreviewOutcome =>
  Object.freeze({ ...operation, kind: "detached" });

export function createPhotoOwner(
  fetcher: PhotoFetch,
  source: PhotoSourcePort,
  options: Readonly<{ emit?: (event: PhotoOwnerEvent) => void }> = {},
): PhotoOwner {
  let closed = false;
  let lifetime: Lifetime | undefined;
  let binding: PhotoSourceBinding | undefined;
  let active = false;
  let opening = false;
  let busyAuthority: PhotoAuthority | undefined;
  let undo: SessionUndo | undefined;
  let undoRecord: UndoRecord | undefined;
  let batchUndo: SessionBatchUndo | undefined;
  let batchUndoRecord: BatchUndoRecord | undefined;
  let lastCurrentPhotoId: string | undefined;
  let reviewImage: ReviewImageLease | undefined;

  const makeAuthority = () => Object.freeze({}) as PhotoAuthority;
  let latestAuthority = makeAuthority();

  const isCurrent = (authority: PhotoAuthority): boolean =>
    !closed &&
    lifetime?.authority === authority &&
    source.isSourceCurrent(lifetime.sourceAuthority);

  const releaseReviewImage = () => {
    const lease = reviewImage;
    if (!lease) return;
    reviewImage = undefined;
    lease.image.clearHandlers();
    if (lease.image.source === lease.resolvedUrl) lease.image.clearSource();
  };

  const operation = (
    record: Lifetime,
    index = binding?.index ?? 0,
  ): PhotoOperation => {
    const photoId = source.photoAt(record.sourceAuthority, index)?.id;
    return Object.freeze({
      authority: record.authority,
      sourceAuthority: record.sourceAuthority,
      windowAuthority: record.windowAuthority,
      index,
      ...(photoId ? { photoId } : {}),
    });
  };

  const renewLifetime = (
    sourceAuthority: SourceAuthority,
    options: Readonly<{ preserveReviewImage?: boolean }> = {},
  ): Lifetime | undefined => {
    if (!options.preserveReviewImage) releaseReviewImage();
    lifetime?.tasks.halt();
    lifetime = undefined;
    latestAuthority = makeAuthority();
    const windowAuthority = source.renewPhotoWindow(sourceAuthority);
    if (closed || !windowAuthority) return undefined;
    const next = Object.freeze({
      authority: latestAuthority,
      sourceAuthority,
      windowAuthority,
      tasks: new TaskScope(),
    });
    lifetime = next;
    opening = false;
    busyAuthority = undefined;
    undoRecord = undefined;
    batchUndoRecord = undefined;
    return next;
  };

  /// Whether one admitted operation still addresses the same Photo. The
  /// address is the current Photo for a Photo View write, and the Grid
  /// position for a Grid write, so neither write is judged by the other's
  /// position.
  const exact = (
    record: Lifetime,
    expectedPhotoId?: string,
    index: number = binding?.index ?? 0,
  ): boolean =>
    isCurrent(record.authority) &&
    binding?.sourceAuthority === record.sourceAuthority &&
    (expectedPhotoId === undefined ||
      source.photoAt(record.sourceAuthority, index)?.id === expectedPhotoId);

  const currentOperation = (): PhotoOperation | undefined =>
    lifetime && binding ? operation(lifetime) : undefined;

  const patchState = (
    record: Lifetime,
    index: number,
    photoId: string,
    field: PhotoField,
    value: PhotoValue,
  ): boolean =>
    field === "selectionState"
      ? source.patchSelection(
          record.sourceAuthority,
          index,
          photoId,
          value as SelectionState,
        )
      : source.patchRating(
          record.sourceAuthority,
          index,
          photoId,
          value as number,
        );

  const setBinding = (
    next: PhotoSourceBinding,
    clearUndo: boolean,
    preserveActive: boolean,
  ): PhotoAuthority => {
    binding = Object.freeze({ ...next });
    active = preserveActive && active;
    opening = false;
    busyAuthority = undefined;
    undoRecord = undefined;
    batchUndoRecord = undefined;
    if (clearUndo) {
      undo = undefined;
      batchUndo = undefined;
    }
    lastCurrentPhotoId = next.preferredPhotoId;
    renewLifetime(next.sourceAuthority);
    return latestAuthority;
  };

  /// Admits one Selection State or Rating write and settles it with the
  /// shared classification. Photo View and Grid writes differ only in the
  /// Photo they address and in whether a committed write advances the current
  /// Photo; they share the admission, the one-level Undo, and every failure
  /// rule.
  const admitStateWrite = (
    record: Lifetime,
    index: number,
    photo: PhotoSummary,
    field: PhotoField,
    value: PhotoValue,
    advance: boolean,
  ): PhotoMutationAdmission => {
    const admitted = operation(record, index);
    const captured = Object.freeze({ ...admitted, photoId: photo.id });
    const priorUndo = undo;
    const priorBatchUndo = batchUndo;
    undo = undefined;
    batchUndo = undefined;
    busyAuthority = record.authority;
    const settlement = (async (): Promise<PhotoMutationOutcome> => {
      let result;
      try {
        result = await persistPhotoState(fetcher, {
          photoId: photo.id,
          field,
          value,
          ...(binding?.albumId ? { albumId: binding.albumId } : {}),
          requireUndo: true,
        });
      } catch {
        if (exact(record, photo.id, index)) undo = undefined;
        return exact(record, photo.id, index)
          ? Object.freeze({
              ...captured,
              kind: "failed",
              field,
              advance,
              failure: "transport",
              connectivity: "lost",
            })
          : Object.freeze({
              ...captured,
              kind: "detached",
              field,
              advance,
            });
      } finally {
        if (busyAuthority === record.authority) busyAuthority = undefined;
      }
      if (!exact(record, photo.id, index))
        return Object.freeze({
          ...captured,
          kind: "detached",
          field,
          advance,
        });
      if (result.kind === "persisted" && result.undo) {
        const applied = patchState(
          record,
          captured.index,
          photo.id,
          field,
          value,
        );
        if (applied)
          undo = Object.freeze({
            ...result.undo,
            advanced: advance && captured.index < (binding?.total ?? 0) - 1,
            snapshotIndex: captured.index,
          });
        return Object.freeze({
          ...captured,
          kind: "persisted",
          field,
          advance: Boolean(undo?.advanced),
          applied,
        });
      }
      if (result.kind === "rejected") undo = priorUndo;
      else undo = undefined;
      if (result.kind === "rejected") batchUndo = priorBatchUndo;
      else batchUndo = undefined;
      return Object.freeze({
        ...captured,
        kind: "failed",
        field,
        advance,
        failure: result.kind === "rejected" ? "answered" : "malformed",
        connectivity:
          result.kind === "rejected" && result.status !== 409
            ? "unchanged"
            : "lost",
        ...(result.kind === "rejected" ? { status: result.status } : {}),
      });
    })();
    return Object.freeze({ authority: record.authority, settlement });
  };

  /// Admits one batch Selection State write. It shares the one admission, the
  /// one-level Undo, and every failure classification with a single write; its
  /// address is the stable Photo identifiers the Grid multi-selected, so it
  /// keeps its identity while the source generation remains current.
  const admitBatchWrite = (
    record: Lifetime,
    photos: ReadonlyArray<PhotoStateBatchPhoto>,
    value: SelectionState,
  ): PhotoBatchAdmission => {
    const priorUndo = undo;
    const priorBatchUndo = batchUndo;
    undo = undefined;
    batchUndo = undefined;
    busyAuthority = record.authority;
    const settlement = (async (): Promise<PhotoBatchOutcome> => {
      let result;
      try {
        result = await persistPhotoStateBatch(fetcher, { photos, value });
      } catch {
        // A transport failure cannot prove the batch did not commit, so the
        // prior Undo description is consumed exactly as a single write's is.
        return isCurrent(record.authority)
          ? Object.freeze({
              kind: "failed",
              failure: "transport",
              connectivity: "lost",
            })
          : Object.freeze({ kind: "detached" });
      } finally {
        if (busyAuthority === record.authority) busyAuthority = undefined;
      }
      if (!isCurrent(record.authority))
        return Object.freeze({ kind: "detached" });
      if (result.kind === "persisted") {
        for (const entry of result.applied)
          source.applyBatchSelection(
            record.sourceAuthority,
            entry.photoId,
            entry.priorValue,
            value,
          );
        // Changed and missing Photos are not written, so the owner moves no
        // local fact for either outcome. Only applied Photos can enter Undo.
        const entries = result.applied.filter(
          (entry) => entry.priorValue !== value,
        );
        if (entries.length > 0)
          batchUndo = Object.freeze({
            value,
            entries: Object.freeze(entries),
          });
        return Object.freeze({
          kind: "persisted",
          value,
          applied: result.applied,
          changedElsewhere: result.changedElsewhere,
          missing: result.missing,
        });
      }
      // The route rejects before any write, so a rejected batch never
      // happened and the prior Undo description stays available.
      if (result.kind === "rejected") {
        undo = priorUndo;
        batchUndo = priorBatchUndo;
      }
      return Object.freeze({
        kind: "failed",
        failure: result.kind === "rejected" ? "answered" : "malformed",
        connectivity:
          result.kind === "rejected" && result.status !== 409
            ? "unchanged"
            : "lost",
        ...(result.kind === "rejected" ? { status: result.status } : {}),
      });
    })();
    return Object.freeze({ settlement });
  };

  const owner: PhotoOwner = {
    get authority() {
      return latestAuthority;
    },
    get sourceAuthority() {
      return binding?.sourceAuthority;
    },
    get windowAuthority() {
      return lifetime?.windowAuthority;
    },
    get currentIndex() {
      return binding?.index ?? 0;
    },
    get total() {
      return binding?.total ?? 0;
    },
    get current() {
      return binding
        ? source.photoAt(binding.sourceAuthority, binding.index)
        : undefined;
    },
    get lastCurrentPhotoId() {
      return lastCurrentPhotoId;
    },
    get busy() {
      return busyAuthority !== undefined && isCurrent(busyAuthority);
    },
    get opening() {
      return opening;
    },
    get active() {
      return active;
    },
    get canUndo() {
      return undo !== undefined || batchUndo !== undefined;
    },
    get undoPhotoId() {
      return undo?.photoId;
    },
    get undoBatch() {
      return batchUndo !== undefined;
    },
    isCurrent,
    ownsWindow: (authority, windowAuthority) =>
      isCurrent(authority) && lifetime?.windowAuthority === windowAuthority,
    bindSource: (next) => setBinding(next, true, false),
    rebindSource: (next) => setBinding(next, false, true),
    updateSource: (next) => {
      if (
        closed ||
        !binding ||
        binding.sourceAuthority !== next.sourceAuthority ||
        !source.isSourceCurrent(next.sourceAuthority)
      )
        return false;
      binding = Object.freeze({ ...next });
      return true;
    },
    beginOpen: (index) => {
      if (
        closed ||
        !binding ||
        owner.busy ||
        opening ||
        index < 0 ||
        index >= binding.total
      )
        return undefined;
      const record = renewLifetime(binding.sourceAuthority, {
        preserveReviewImage: true,
      });
      if (!record) return undefined;
      active = true;
      opening = true;
      return operation(record, index);
    },
    commitOpen: (open) => {
      if (
        !opening ||
        !binding ||
        !isCurrent(open.authority) ||
        binding.sourceAuthority !== open.sourceAuthority ||
        lifetime?.windowAuthority !== open.windowAuthority
      )
        return undefined;
      const photo = source.photoAt(open.sourceAuthority, open.index);
      if (!photo || !source.movePosition(open.sourceAuthority, open.index))
        return undefined;
      binding = Object.freeze({ ...binding, index: open.index });
      opening = false;
      lastCurrentPhotoId = photo.id;
      releaseReviewImage();
      return photo;
    },
    cancelOpen: (authority) => {
      if (isCurrent(authority)) opening = false;
    },
    leave: () => {
      active = false;
      opening = false;
      busyAuthority = undefined;
      undoRecord = undefined;
      batchUndoRecord = undefined;
      if (binding) renewLifetime(binding.sourceAuthority);
      else latestAuthority = makeAuthority();
      return latestAuthority;
    },
    loadCurrentPreview: async (authority) => {
      const record = lifetime;
      const initial = currentOperation();
      if (!record || !initial || record.authority !== authority)
        return detachedPreview(
          initial ??
            Object.freeze({
              authority,
              sourceAuthority: binding?.sourceAuthority as SourceAuthority,
              windowAuthority:
                lifetime?.windowAuthority as PhotoWindowAuthority,
              index: binding?.index ?? 0,
            }),
        );
      const photo = source.photoAt(record.sourceAuthority, initial.index);
      if (!photo) return detachedPreview(initial);
      const owned = Object.freeze({ ...initial, photoId: photo.id });
      const task = record.tasks.beginLatest(`preview:current:${photo.id}`, {
        abortTransport: true,
      });
      try {
        let result;
        try {
          result = await fetchPreview(
            fetcher,
            photo.id,
            "current",
            task.signal!,
          );
        } catch {
          return exact(record, photo.id) && task.isCurrent()
            ? Object.freeze({
                ...owned,
                kind: "failed",
                failure: "transport",
              })
            : detachedPreview(owned);
        }
        if (!exact(record, photo.id) || !task.isCurrent())
          return detachedPreview(owned);
        if (result.kind === "ready") {
          if (!result.value.stale) {
            const latest = source.photoAt(
              record.sourceAuthority,
              initial.index,
            );
            if (latest?.id === photo.id)
              source.patchPreview(
                record.sourceAuthority,
                initial.index,
                photo.id,
                Object.freeze({
                  ...latest.preview,
                  state: "ready",
                  ...(result.value.source
                    ? { source: result.value.source }
                    : {}),
                  ...(result.value.width !== undefined
                    ? { width: result.value.width }
                    : {}),
                  ...(result.value.height !== undefined
                    ? { height: result.value.height }
                    : {}),
                  ...(result.value.limitedDetail !== undefined
                    ? { limitedDetail: result.value.limitedDetail }
                    : {}),
                  url: result.value.url,
                }),
              );
          }
          return Object.freeze({
            ...owned,
            kind: "ready",
            preview: result.value,
          });
        }
        if (result.kind === "not-ready")
          return Object.freeze({
            ...owned,
            kind: "not-ready",
            preview: result.value,
          });
        return Object.freeze({
          ...owned,
          kind: "failed",
          failure: result.kind === "malformed" ? "malformed" : "answered",
          ...(result.kind === "rejected" ? { status: result.status } : {}),
        });
      } finally {
        task.finish();
      }
    },
    prefetchAdjacent: async (authority, index) => {
      const record = lifetime;
      if (
        !record ||
        record.authority !== authority ||
        !isCurrent(authority) ||
        !binding ||
        index < 0 ||
        index >= binding.total
      )
        return;
      const photo = source.photoAt(record.sourceAuthority, index);
      if (!photo?.available) return;
      const task = record.tasks.beginLatest(`preview:adjacent:${photo.id}`, {
        abortTransport: true,
      });
      try {
        await fetchPreview(fetcher, photo.id, "adjacent", task.signal!);
      } catch {
        // Adjacent Preview is best effort and never affects presentation.
      } finally {
        task.finish();
      }
    },
    attachReviewImage: (authority, image, resolvedUrl, surface) => {
      const record = lifetime;
      const photo = owner.current;
      if (!record || record.authority !== authority || !photo) return false;
      const photoId = photo.id;
      releaseReviewImage();
      const lease = Object.freeze({ image, resolvedUrl });
      reviewImage = lease;
      const transfer = record.tasks.beginLatest("review-image", {
        abortTransport: false,
      });
      let loaded = false;
      transfer.onCleanup(() => {
        image.clearHandlers();
        if (!loaded && image.source === resolvedUrl) image.clearSource();
      });
      image.setHandlers(
        () => {
          loaded = true;
          transfer.finish();
        },
        () => {
          if (!transfer.isCurrent()) return;
          if (
            isCurrent(authority) &&
            image.connected &&
            owner.current?.id === photoId
          )
            options.emit?.(
              Object.freeze({
                kind: "review-image-failed",
                authority,
                photoId,
                surface,
              }),
            );
          image.clearSource();
          if (reviewImage === lease) reviewImage = undefined;
          transfer.finish();
        },
      );
      image.setSource(resolvedUrl);
      return true;
    },
    mutate: (field, value, advance) => {
      const record = lifetime;
      const photo = owner.current;
      if (!record || !photo || !active || owner.busy) return undefined;
      return admitStateWrite(
        record,
        binding?.index ?? 0,
        photo,
        field,
        value,
        advance,
      );
    },
    mutateAt: (index, field, value) => {
      const record = lifetime;
      if (!record || closed || owner.busy) return undefined;
      const photo = source.photoAt(record.sourceAuthority, index);
      if (!photo) return undefined;
      // Clearing an already-undecided Photo is not a change. Refusing it keeps
      // the one-level Undo description on the last real change.
      if (
        field === "selectionState" &&
        value === "undecided" &&
        photo.selectionState === "undecided"
      )
        return undefined;
      return admitStateWrite(record, index, photo, field, value, false);
    },
    get undoAdvanced() {
      return undo?.advanced ?? false;
    },
    mutateBatch: (photos, value) => {
      const record = lifetime;
      if (!record || closed || owner.busy || photos.length === 0)
        return undefined;
      return admitBatchWrite(record, [...photos], value);
    },
    prepareBatchUndo: () => {
      const record = lifetime;
      const action = batchUndo;
      if (!record || !action || owner.busy) return undefined;
      const preparation = Object.freeze({
        authority: record.authority,
        operation: Object.freeze({}) as BatchUndoOperation,
        count: action.entries.length,
      });
      busyAuthority = record.authority;
      batchUndoRecord = Object.freeze({
        operation: preparation.operation,
        preparation,
        action,
      });
      return preparation;
    },
    cancelBatchUndo: (preparation) => {
      if (batchUndoRecord?.operation !== preparation.operation) return;
      batchUndoRecord = undefined;
      if (busyAuthority === preparation.authority) busyAuthority = undefined;
    },
    performBatchUndo: async (preparation) => {
      const record = lifetime;
      const captured = batchUndoRecord;
      if (
        !record ||
        !captured ||
        captured.operation !== preparation.operation ||
        record.authority !== preparation.authority ||
        !isCurrent(record.authority)
      ) {
        owner.cancelBatchUndo(preparation);
        return Object.freeze({ kind: "detached" });
      }
      const restored: string[] = [];
      const restoredValues: Array<
        Readonly<{ photoId: string; value: SelectionState }>
      > = [];
      const conflicts: string[] = [];
      const failed: Array<
        Readonly<{ photoId: string; priorValue: SelectionState }>
      > = [];
      let connectivity: "unchanged" | "lost" = "unchanged";
      const entries = captured.action.entries;
      for (let position = 0; position < entries.length; position += 1) {
        const entry = entries[position]!;
        if (!isCurrent(record.authority)) {
          owner.cancelBatchUndo(preparation);
          return Object.freeze({ kind: "detached" });
        }
        let result;
        try {
          result = await persistPhotoState(fetcher, {
            photoId: entry.photoId,
            field: "selectionState",
            value: entry.priorValue,
            expectedCurrent: captured.action.value,
            requireUndo: false,
          });
        } catch {
          // The connection is gone: this Photo and every Photo after it stay
          // part of the one-level description so the Photographer can retry.
          connectivity = "lost";
          failed.push(...entries.slice(position));
          break;
        }
        if (result.kind === "persisted") {
          restored.push(entry.photoId);
          restoredValues.push(
            Object.freeze({ photoId: entry.photoId, value: entry.priorValue }),
          );
          source.applyBatchSelection(
            record.sourceAuthority,
            entry.photoId,
            captured.action.value,
            entry.priorValue,
          );
        } else if (result.kind === "rejected" && result.status === 409) {
          // The Photo changed elsewhere, so the value this Undo would restore
          // is no longer the current one. It retires from the description.
          conflicts.push(entry.photoId);
        } else {
          failed.push(entry);
        }
      }
      if (batchUndoRecord?.operation === preparation.operation)
        batchUndoRecord = undefined;
      if (busyAuthority === preparation.authority) busyAuthority = undefined;
      batchUndo =
        failed.length > 0
          ? Object.freeze({
              value: captured.action.value,
              entries: Object.freeze(failed),
            })
          : undefined;
      return Object.freeze({
        kind: "settled",
        restored: Object.freeze(restored),
        restoredValues: Object.freeze(restoredValues),
        conflicts: Object.freeze(conflicts),
        failed: Object.freeze(failed.map((entry) => entry.photoId)),
        connectivity,
      });
    },
    prepareUndo: (resolvedIndex) => {
      const record = lifetime;
      const action = undo;
      if (!record || !action || owner.busy) return undefined;
      const retainedIndex = source.findPhotoIndex(action.photoId);
      const index = resolvedIndex ?? retainedIndex ?? action.snapshotIndex;
      const target = source.photoAt(record.sourceAuthority, index);
      const prep = Object.freeze({
        ...operation(record, index),
        photoId: action.photoId,
        operation: Object.freeze({}) as UndoOperation,
        needsWindow: !target || target.id !== action.photoId,
        needsPosition:
          resolvedIndex === undefined && retainedIndex === undefined,
      });
      busyAuthority = record.authority;
      undoRecord = Object.freeze({
        operation: prep.operation,
        preparation: prep,
        action,
      });
      return prep;
    },
    discardUndo: () => {
      undo = undefined;
      undoRecord = undefined;
      batchUndo = undefined;
      batchUndoRecord = undefined;
    },
    cancelUndo: (preparation) => {
      if (undoRecord?.operation !== preparation.operation) return;
      undoRecord = undefined;
      if (busyAuthority === preparation.authority) busyAuthority = undefined;
    },
    performUndo: async (preparation) => {
      const record = lifetime;
      const captured = undoRecord;
      if (
        !record ||
        !captured ||
        captured.operation !== preparation.operation ||
        record.authority !== preparation.authority ||
        !exact(record)
      ) {
        owner.cancelUndo(preparation);
        return Object.freeze({ ...preparation, kind: "detached" });
      }
      const photo = source.photoAt(record.sourceAuthority, preparation.index);
      if (!photo || photo.id !== captured.action.photoId) {
        owner.cancelUndo(preparation);
        return Object.freeze({ ...preparation, kind: "detached" });
      }
      undo = undefined;
      let result;
      try {
        result = await persistPhotoState(fetcher, {
          photoId: captured.action.photoId,
          field: captured.action.field,
          value: captured.action.priorValue,
          expectedCurrent: captured.action.expectedCurrent,
          ...(binding?.albumId ? { albumId: binding.albumId } : {}),
          requireUndo: false,
        });
      } catch {
        if (!exact(record))
          return Object.freeze({ ...preparation, kind: "detached" });
        return Object.freeze({
          ...preparation,
          kind: "failed",
          failure: "transport",
          connectivity: "lost",
          retryable: false,
        });
      } finally {
        if (undoRecord?.operation === preparation.operation)
          undoRecord = undefined;
        if (busyAuthority === preparation.authority) busyAuthority = undefined;
      }
      if (!exact(record))
        return Object.freeze({ ...preparation, kind: "detached" });
      if (result.kind !== "persisted") {
        const conflict = result.kind === "rejected" && result.status === 409;
        if (!conflict) undo = captured.action;
        return Object.freeze({
          ...preparation,
          kind: "failed",
          failure: result.kind === "rejected" ? "answered" : "transport",
          connectivity: conflict ? "lost" : "unchanged",
          retryable: !conflict,
          ...(result.kind === "rejected" ? { status: result.status } : {}),
        });
      }
      const applied = patchState(
        record,
        preparation.index,
        captured.action.photoId,
        captured.action.field,
        captured.action.priorValue,
      );
      if (
        !applied ||
        !source.movePosition(record.sourceAuthority, preparation.index) ||
        !binding
      )
        return Object.freeze({ ...preparation, kind: "detached" });
      source.trimFacts(record.sourceAuthority, preparation.index);
      binding = Object.freeze({ ...binding, index: preparation.index });
      lastCurrentPhotoId = captured.action.photoId;
      const next = renewLifetime(record.sourceAuthority);
      if (!next) return Object.freeze({ ...preparation, kind: "detached" });
      active = true;
      return Object.freeze({
        ...operation(next, preparation.index),
        photoId: captured.action.photoId,
        kind: "persisted",
      });
    },
    beginRetry: () => {
      if (!binding || !active || owner.busy) return undefined;
      const expectedPhotoId = owner.current?.id ?? lastCurrentPhotoId;
      if (!expectedPhotoId) return undefined;
      const record = renewLifetime(binding.sourceAuthority, {
        preserveReviewImage: true,
      });
      if (!record) return undefined;
      busyAuthority = record.authority;
      return Object.freeze({
        ...operation(record),
        photoId: expectedPhotoId,
        expectedPhotoId,
      });
    },
    retryIsCurrent: (retry) =>
      isCurrent(retry.authority) &&
      binding?.sourceAuthority === retry.sourceAuthority &&
      binding.index === retry.index,
    retryPhotoIsCurrent: (retry) =>
      owner.retryIsCurrent(retry) &&
      owner.current?.id === retry.expectedPhotoId,
    finishRetry: (retry) => {
      if (busyAuthority === retry.authority) busyAuthority = undefined;
    },
    dispose: () => {
      if (closed) return;
      closed = true;
      active = false;
      opening = false;
      busyAuthority = undefined;
      undo = undefined;
      undoRecord = undefined;
      batchUndo = undefined;
      batchUndoRecord = undefined;
      releaseReviewImage();
      lifetime?.tasks.halt();
      lifetime = undefined;
    },
  };
  return owner;
}
