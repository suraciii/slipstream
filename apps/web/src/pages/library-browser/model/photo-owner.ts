import type {
  PhotoSummary,
  PreviewResponse,
  SelectionState,
  UndoDescription,
} from "../api/contracts.js";
import {
  fetchPreview,
  persistPhotoState,
  type PhotoFetch,
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

export type PhotoUndoPreparation = PhotoOperation &
  Readonly<{
    operation: UndoOperation;
    needsWindow: boolean;
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
  isCurrent(authority: PhotoAuthority): boolean;
  ownsWindow(
    authority: PhotoAuthority,
    windowAuthority: PhotoWindowAuthority,
  ): boolean;
  bindSource(binding: PhotoSourceBinding): PhotoAuthority;
  rebindSource(binding: PhotoSourceBinding): PhotoAuthority;
  updateSource(binding: PhotoSourceBinding): boolean;
  beginOpen(index: number): PhotoOperation | undefined;
  finishOpen(authority: PhotoAuthority): PhotoSummary | undefined;
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
  prepareUndo(): PhotoUndoPreparation | undefined;
  cancelUndo(preparation: PhotoUndoPreparation): void;
  performUndo(preparation: PhotoUndoPreparation): Promise<PhotoUndoOutcome>;
  beginRetry(): PhotoRetry | undefined;
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
  ): Lifetime | undefined => {
    releaseReviewImage();
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
    return next;
  };

  const exact = (record: Lifetime, expectedPhotoId?: string): boolean =>
    isCurrent(record.authority) &&
    binding?.sourceAuthority === record.sourceAuthority &&
    (expectedPhotoId === undefined ||
      source.photoAt(record.sourceAuthority, binding.index)?.id ===
        expectedPhotoId);

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
    if (clearUndo) undo = undefined;
    lastCurrentPhotoId = next.preferredPhotoId;
    renewLifetime(next.sourceAuthority);
    return latestAuthority;
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
      return undo !== undefined;
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
        index >= binding.total ||
        !source.movePosition(binding.sourceAuthority, index)
      )
        return undefined;
      binding = Object.freeze({ ...binding, index });
      active = true;
      const record = renewLifetime(binding.sourceAuthority);
      if (!record) return undefined;
      opening = true;
      return operation(record, index);
    },
    finishOpen: (authority) => {
      if (!isCurrent(authority) || !binding) return undefined;
      opening = false;
      const photo = source.photoAt(binding.sourceAuthority, binding.index);
      if (photo) lastCurrentPhotoId = photo.id;
      return photo;
    },
    leave: () => {
      active = false;
      opening = false;
      busyAuthority = undefined;
      undoRecord = undefined;
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
      const admitted = operation(record);
      const captured = Object.freeze({ ...admitted, photoId: photo.id });
      const priorUndo = undo;
      undo = undefined;
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
          if (exact(record, photo.id)) undo = undefined;
          return exact(record, photo.id)
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
        if (!exact(record, photo.id))
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
    },
    prepareUndo: () => {
      const record = lifetime;
      const action = undo;
      if (!record || !action || owner.busy) return undefined;
      const prep = Object.freeze({
        ...operation(record, action.snapshotIndex),
        photoId: action.photoId,
        operation: Object.freeze({}) as UndoOperation,
        needsWindow: !source.photoAt(
          record.sourceAuthority,
          action.snapshotIndex,
        ),
      });
      busyAuthority = record.authority;
      undoRecord = Object.freeze({
        operation: prep.operation,
        preparation: prep,
        action,
      });
      return prep;
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
      const record = renewLifetime(binding.sourceAuthority);
      if (!record) return undefined;
      busyAuthority = record.authority;
      return Object.freeze({
        ...operation(record),
        photoId: expectedPhotoId,
        expectedPhotoId,
      });
    },
    retryPhotoIsCurrent: (retry) =>
      isCurrent(retry.authority) && owner.current?.id === retry.expectedPhotoId,
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
      releaseReviewImage();
      lifetime?.tasks.halt();
      lifetime = undefined;
    },
  };
  return owner;
}
