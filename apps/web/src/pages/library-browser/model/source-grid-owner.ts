import type {
  PhotoSummary,
  SelectionCounts,
  SelectionFilter,
  SelectionState,
} from "../api/contracts.js";
import {
  fetchBrowseWindow,
  fetchBrowsePosition,
  fetchThumbnail,
  openBrowse,
  releaseBrowse,
  type BrowseSourceRequest,
  type SourceGridFetch,
  type SourceViewOrder,
} from "../api/source-grid.js";
import { TaskScope } from "./async-ownership.js";

const WINDOW_SIZE = 60;
const MAX_RETAINED_FACTS = WINDOW_SIZE * 3;
/// The largest Photo range the Grid presents at a supported large viewport,
/// with one window of buffer on each side.
const MAX_VIEWPORT_RANGE = MAX_RETAINED_FACTS + WINDOW_SIZE * 2;
/// Cap for the retained-fact bound: that range and its buffer again.
const MAX_RETAINED_FACTS_CAP = MAX_VIEWPORT_RANGE + WINDOW_SIZE * 2;
const MAX_RETAINED_THUMBNAILS = WINDOW_SIZE * 4;

declare const sourceAuthorityBrand: unique symbol;
export type SourceAuthority = Readonly<{ [sourceAuthorityBrand]: true }>;

declare const photoWindowAuthorityBrand: unique symbol;
export type PhotoWindowAuthority = Readonly<{
  [photoWindowAuthorityBrand]: true;
}>;

export type SourceGridSource =
  | Readonly<{ kind: "library" }>
  | Readonly<{
      kind: "album";
      album: Readonly<{ id: string; name: string }>;
    }>
  | Readonly<{
      kind: "folder";
      folder: Readonly<{ location: string; name: string }>;
      publication: string;
    }>;

export type SourceWindowOperation =
  | Readonly<{ kind: "source" | "grid"; authority: SourceAuthority }>
  | Readonly<{
      kind: "photo";
      authority: PhotoWindowAuthority;
    }>;

export type SourceWindowOwner =
  | Readonly<{ scope: "source"; generation: number }>
  | Readonly<{ scope: "photo"; authority: PhotoWindowAuthority }>;

export type SourceOpenOutcome =
  | Readonly<{
      kind: "opened";
      authority: SourceAuthority;
      generation: number;
      total: number;
      position: number;
    }>
  | Readonly<{
      kind: "publication-conflict";
      authority: SourceAuthority;
      generation: number;
    }>
  | Readonly<{
      kind: "failed";
      authority: SourceAuthority;
      generation: number;
      transportLost: true;
      status?: number;
    }>
  | Readonly<{
      kind: "detached";
      authority: SourceAuthority;
      generation: number;
    }>;

export type SourceWindowOutcome =
  | Readonly<{
      kind: "loaded";
      authority: SourceAuthority;
      owner: SourceWindowOwner;
      start: number;
      changed: boolean;
    }>
  | Readonly<{
      kind: "expired";
      authority: SourceAuthority;
      owner: SourceWindowOwner;
      start: number;
      index: number;
    }>
  | Readonly<{
      kind: "failed";
      authority: SourceAuthority;
      owner: SourceWindowOwner;
      start: number;
      range: string;
      transportLost: boolean;
      status?: number;
      malformed?: true;
    }>
  | Readonly<{
      kind: "detached";
      authority: SourceAuthority;
      owner: SourceWindowOwner;
      start: number;
    }>;

export type SourcePositionOutcome =
  | Readonly<{
      kind: "resolved";
      authority: SourceAuthority;
      photoId: string;
      position: number;
    }>
  | Readonly<{
      kind: "missing";
      authority: SourceAuthority;
      photoId: string;
    }>
  | Readonly<{
      kind: "expired";
      authority: SourceAuthority;
      photoId: string;
    }>
  | Readonly<{
      kind: "failed";
      authority: SourceAuthority;
      photoId: string;
      transportLost: boolean;
      status?: number;
      malformed?: true;
    }>
  | Readonly<{
      kind: "detached";
      authority: SourceAuthority;
      photoId: string;
    }>;

export interface GridThumbnailImage {
  complete: boolean;
  isConnected: boolean;
  src: string;
  onload: GlobalEventHandlers["onload"];
  onerror: GlobalEventHandlers["onerror"];
  removeAttribute(name: string): void;
  setDeliveryFailed(failed: boolean): void;
}

export interface SourceGridOwner {
  readonly authority: SourceAuthority;
  readonly generation: number;
  readonly source: SourceGridSource;
  readonly order: SourceViewOrder;
  readonly selection: SelectionFilter;
  readonly selectionCounts: SelectionCounts;
  readonly lastSource: SourceGridSource | undefined;
  readonly kind: SourceGridSource["kind"];
  readonly albumId: string | undefined;
  readonly name: string;
  readonly folder: Readonly<{ location: string; name: string }> | undefined;
  readonly token: string;
  readonly total: number;
  readonly retryRequired: boolean;
  readonly retainedFactCount: number;
  readonly retainedThumbnailCount: number;
  readonly retainedThumbnailDeliveryFailureCount: number;
  readonly retainedImageCount: number;
  isCurrent(authority: SourceAuthority): boolean;
  isReady(authority: SourceAuthority): boolean;
  renewPhotoWindow(): PhotoWindowAuthority;
  open(
    source: SourceGridSource,
    options?: Readonly<{
      preferredPhotoId?: string;
      mode?: "replace" | "reopen";
      order?: SourceViewOrder;
      selection?: SelectionFilter;
    }>,
  ): Promise<SourceOpenOutcome>;
  establish(authority: SourceAuthority): boolean;
  updateAlbum(album: Readonly<{ id: string; name: string }>): void;
  photoAt(index: number): PhotoSummary | undefined;
  findPhotoIndex(photoId: string): number | undefined;
  resolvePhotoPosition(
    authority: SourceAuthority,
    photoId: string,
  ): Promise<SourcePositionOutcome>;
  readGridPosition(authority: SourceAuthority): number | undefined;
  moveGridPosition(authority: SourceAuthority, index: number): boolean;
  setPhotoPreview(
    authority: SourceAuthority,
    index: number,
    expectedPhotoId: string,
    preview: PhotoSummary["preview"],
  ): boolean;
  setPhotoSelection(
    authority: SourceAuthority,
    index: number,
    expectedPhotoId: string,
    selectionState: PhotoSummary["selectionState"],
  ): boolean;
  setPhotoRating(
    authority: SourceAuthority,
    index: number,
    expectedPhotoId: string,
    rating: number,
  ): boolean;
  invalidateWindow(index: number): void;
  trimFacts(anchor?: number): void;
  alignedStart(index: number): number;
  describeWindow(index: number): Readonly<{ start: number; range: string }>;
  ensureRange(
    start: number,
    end: number,
    operation: SourceWindowOperation,
    options?: Readonly<{ quiet?: boolean; priority?: "high" | "low" }>,
  ): void;
  onWindowSettled(handler: (outcome: SourceWindowOutcome) => void): () => void;
  loadWindow(
    index: number,
    operation: SourceWindowOperation,
    options?: Readonly<{ quiet?: boolean; priority?: "high" | "low" }>,
  ): Promise<SourceWindowOutcome>;
  stopGridWork(): void;
  loadThumbnail(photoId: string, image: GridThumbnailImage): Promise<void>;
  releaseThumbnail(photoId: string, image: GridThumbnailImage): void;
  presentThumbnail(
    photoId: string,
    image: GridThumbnailImage,
    url?: string,
    attachDisconnected?: boolean,
  ): void;
  clearRenderedThumbnails(): void;
  dispose(): void;
}

type ImageTransfer = Readonly<{
  image: GridThumbnailImage;
  finish: () => void;
}>;

type PhotoWindowRecord = Readonly<{
  authority: PhotoWindowAuthority;
  sourceAuthority: SourceAuthority;
  tasks: TaskScope;
}>;

/// One confirmed Selection State transition applied to the open source's
/// counts. The counts come from the server when the source opens, so a
/// confirmed decision moves them by one instead of re-deriving them from
/// loaded windows.
const adjustedSelectionCounts = (
  counts: SelectionCounts,
  prior: SelectionState,
  next: SelectionState,
): SelectionCounts => {
  if (prior === next) return counts;
  const adjusted = { ...counts };
  adjusted[prior] -= 1;
  adjusted[next] += 1;
  return Object.freeze(adjusted);
};

const sourceRequest = (
  source: SourceGridSource,
  order: SourceViewOrder,
  selection: SelectionFilter,
  preferredPhotoId?: string,
): BrowseSourceRequest =>
  source.kind === "library"
    ? {
        kind: "library",
        order,
        selection,
        ...(preferredPhotoId ? { preferredPhotoId } : {}),
      }
    : source.kind === "album"
      ? {
          kind: "album",
          albumId: source.album.id,
          order,
          selection,
          ...(preferredPhotoId ? { preferredPhotoId } : {}),
        }
      : {
          kind: "folder",
          folderPath: source.folder.location,
          publication: source.publication,
          order,
          selection,
          ...(preferredPhotoId ? { preferredPhotoId } : {}),
        };

const freezeSource = (source: SourceGridSource): SourceGridSource =>
  source.kind === "library"
    ? Object.freeze({ kind: "library" })
    : source.kind === "album"
      ? Object.freeze({
          kind: "album",
          album: Object.freeze({ ...source.album }),
        })
      : Object.freeze({
          kind: "folder",
          folder: Object.freeze({ ...source.folder }),
          publication: source.publication,
        });

export function createSourceGridOwner(
  fetcher: SourceGridFetch,
): SourceGridOwner {
  let closed = false;
  let generation = 0;
  const authorityGenerations = new WeakMap<object, number>();
  const makeAuthority = () => {
    const next = Object.freeze({}) as SourceAuthority;
    authorityGenerations.set(next, generation);
    return next;
  };
  let authority = makeAuthority();
  let source: SourceGridSource = freezeSource({ kind: "library" });
  let viewOrder: SourceViewOrder = "source-default";
  let viewSelection: SelectionFilter = "all";
  // The server's per-state counts for the open source. They describe the
  // source order, so a filtered view still reports source-wide progress.
  let selectionCounts: SelectionCounts = Object.freeze({
    selected: 0,
    rejected: 0,
    undecided: 0,
  });
  let lastSource: SourceGridSource | undefined;
  let token = "";
  let total = 0;
  let gridPosition = 0;
  let retryRequired = false;
  // A replacement's retained Grid DOM stays display-only until its source
  // window has established current facts.
  let sourceReady = false;
  let sourceTasks = new TaskScope();
  let gridTasks = new TaskScope();
  let facts = new Map<number, PhotoSummary>();
  // Latest visible range reported through range admission. Fact eviction
  // anchors here; window requests never anchor eviction to a request-time
  // index.
  let visibleRange: Readonly<{ start: number; end: number }> | undefined;
  // Fallback anchor for control-flow callers that never reported a range: the
  // most recently settled window, never the index a request captured.
  let latestSettledWindowStart: number | undefined;
  const windowSettledHandlers = new Set<
    (outcome: SourceWindowOutcome) => void
  >();
  let thumbnails = new Map<string, string>();
  // null means the endpoint failed before it supplied a Thumbnail URL.
  let thumbnailDeliveryFailures = new Map<string, string | null>();
  const renderedImages = new Map<string, GridThumbnailImage>();
  const imageTransfers = new Map<string, ImageTransfer>();
  const knownTokens = new Set<string>();
  const releasesStarted = new Set<string>();
  const photoWindows = new WeakMap<object, PhotoWindowRecord>();
  let currentPhotoWindow: PhotoWindowRecord | undefined;
  const haltedPhotoTasks = new TaskScope();
  haltedPhotoTasks.halt();

  const isCurrent = (candidate: SourceAuthority) =>
    !closed && candidate === authority;

  const releaseToken = (released: string) => {
    if (!released || releasesStarted.has(released)) return;
    releasesStarted.add(released);
    knownTokens.delete(released);
    void releaseBrowse(fetcher, released);
  };

  const detachImages = () => {
    for (const transfer of [...imageTransfers.values()]) transfer.finish();
    imageTransfers.clear();
    renderedImages.clear();
  };

  const renewSourceWork = () => {
    sourceTasks.halt();
    sourceTasks = new TaskScope();
  };

  const stopPhotoWindowWork = () => {
    currentPhotoWindow?.tasks.halt();
    currentPhotoWindow = undefined;
  };

  const renewPhotoWindow = (): PhotoWindowAuthority => {
    stopPhotoWindowWork();
    const photoAuthority = Object.freeze({}) as PhotoWindowAuthority;
    const record = Object.freeze({
      authority: photoAuthority,
      sourceAuthority: authority,
      tasks: new TaskScope(),
    });
    photoWindows.set(photoAuthority, record);
    currentPhotoWindow = record;
    return photoAuthority;
  };

  const stopGridWork = () => {
    detachImages();
    gridTasks.halt();
    gridTasks = new TaskScope();
  };

  const alignedStart = (index: number) =>
    Math.max(
      0,
      Math.min(
        Math.floor(index / WINDOW_SIZE) * WINDOW_SIZE,
        Math.max(0, total - WINDOW_SIZE),
      ),
    );

  const windowLoaded = (start: number) => {
    const end = Math.min(total, start + WINDOW_SIZE);
    for (let index = start; index < end; index += 1)
      if (!facts.has(index)) return false;
    return start < end;
  };

  /// Aligned window starts a control-flow caller is awaiting. Their outcome
  /// settles through that caller, which presents it with the recovery
  /// transition it owns, so the merged notification reports only demanded
  /// windows no caller joined - the range-driven admissions.
  const awaitedWindowStarts = new Map<number, number>();

  const beginAwaitWindow = (start: number) => {
    awaitedWindowStarts.set(start, (awaitedWindowStarts.get(start) ?? 0) + 1);
    return () => {
      const remaining = (awaitedWindowStarts.get(start) ?? 0) - 1;
      if (remaining <= 0) awaitedWindowStarts.delete(start);
      else awaitedWindowStarts.set(start, remaining);
    };
  };

  const notifyWindowSettled = (outcome: SourceWindowOutcome) => {
    // A subscriber that throws owns its own failure: it must not silence the
    // other subscribers or surface as an unhandled rejection on the shared
    // window task.
    for (const handler of windowSettledHandlers) {
      try {
        handler(outcome);
      } catch {
        /* the failing subscriber owns its own failure */
      }
    }
  };

  const onWindowSettled = (
    handler: (outcome: SourceWindowOutcome) => void,
  ): (() => void) => {
    windowSettledHandlers.add(handler);
    return () => {
      windowSettledHandlers.delete(handler);
    };
  };

  const trimFacts = (anchor?: number) => {
    if (visibleRange) {
      // The protected span is the largest range the Grid presents at a
      // supported viewport, anchored at the reported range start: a
      // whole-source report cannot pin every loaded fact. The bound covers the
      // reported range with one window of buffer on each side plus two windows
      // of settlement slack, so a window that settles cannot evict the facts
      // it just committed, capped so retention never follows the Library
      // total.
      const span = Math.max(
        0,
        Math.min(visibleRange.end, visibleRange.start + MAX_VIEWPORT_RANGE) -
          visibleRange.start,
      );
      const bound = Math.max(
        MAX_RETAINED_FACTS,
        Math.min(span + WINDOW_SIZE * 3, MAX_RETAINED_FACTS_CAP),
      );
      const protectedStart = Math.max(0, visibleRange.start - WINDOW_SIZE);
      const protectedEnd = Math.min(
        total,
        visibleRange.start + span + WINDOW_SIZE,
      );
      for (const index of [...facts.keys()]) {
        if (facts.size <= bound) break;
        if (index < protectedStart || index >= protectedEnd)
          facts.delete(index);
      }
      return;
    }
    const fallback = anchor ?? latestSettledWindowStart;
    if (fallback === undefined || facts.size <= MAX_RETAINED_FACTS) return;
    for (const index of [...facts.keys()])
      if (
        Math.abs(index - fallback) > WINDOW_SIZE &&
        facts.size > MAX_RETAINED_FACTS
      )
        facts.delete(index);
  };

  const detachedOpen = (
    ownerAuthority: SourceAuthority,
    ownerGeneration: number,
  ): SourceOpenOutcome => ({
    kind: "detached",
    authority: ownerAuthority,
    generation: ownerGeneration,
  });

  async function open(
    nextSource: SourceGridSource,
    options: Readonly<{
      preferredPhotoId?: string;
      mode?: "replace" | "reopen";
      order?: SourceViewOrder;
      selection?: SelectionFilter;
    }> = {},
  ): Promise<SourceOpenOutcome> {
    if (closed) return detachedOpen(authority, generation);
    const priorToken = token;
    const mode = options.mode ?? "replace";
    renewSourceWork();
    stopGridWork();
    stopPhotoWindowWork();
    generation += 1;
    authority = makeAuthority();
    const ownerAuthority = authority;
    const ownerGeneration = generation;
    source = freezeSource(nextSource);
    viewOrder = options.order ?? "source-default";
    viewSelection = options.selection ?? "all";
    lastSource = source;
    token = "";
    if (priorToken) releaseToken(priorToken);
    retryRequired = false;
    visibleRange = undefined;
    latestSettledWindowStart = undefined;
    if (mode === "replace") {
      sourceReady = false;
      total = 0;
      gridPosition = 0;
      selectionCounts = Object.freeze({
        selected: 0,
        rejected: 0,
        undecided: 0,
      });
      facts = new Map();
      thumbnails = new Map();
      thumbnailDeliveryFailures = new Map();
    }
    const task = sourceTasks.beginLatest("browse-open", {
      abortTransport: true,
    });
    try {
      const result = await openBrowse(
        fetcher,
        sourceRequest(
          source,
          viewOrder,
          viewSelection,
          options.preferredPhotoId,
        ),
        task.signal!,
      );
      if (result.kind === "ok") {
        knownTokens.add(result.value.token);
        if (!task.isCurrent() || !isCurrent(ownerAuthority)) {
          releaseToken(result.value.token);
          return detachedOpen(ownerAuthority, ownerGeneration);
        }
        token = result.value.token;
        total = result.value.total;
        selectionCounts = Object.freeze({ ...result.value.selectionCounts });
        const position = Math.min(
          result.value.position,
          Math.max(0, result.value.total - 1),
        );
        gridPosition = position;
        if (mode === "reopen") {
          sourceReady = false;
          facts = new Map();
          thumbnails = new Map();
          thumbnailDeliveryFailures = new Map();
        }
        return {
          kind: "opened",
          authority: ownerAuthority,
          generation: ownerGeneration,
          total,
          position,
        };
      }
      if (!task.isCurrent() || !isCurrent(ownerAuthority))
        return detachedOpen(ownerAuthority, ownerGeneration);
      retryRequired = true;
      if (result.status === 409 && source.kind === "folder")
        return {
          kind: "publication-conflict",
          authority: ownerAuthority,
          generation: ownerGeneration,
        };
      return {
        kind: "failed",
        authority: ownerAuthority,
        generation: ownerGeneration,
        transportLost: true,
        ...(result.status !== undefined ? { status: result.status } : {}),
      };
    } finally {
      task.finish();
    }
  }

  const operationOwner = (
    operation: SourceWindowOperation,
  ): SourceWindowOwner => {
    const photoWindow =
      operation.kind === "photo"
        ? photoWindows.get(operation.authority)
        : undefined;
    return operation.kind === "photo"
      ? {
          scope: "photo",
          authority: photoWindow?.authority ?? operation.authority,
        }
      : {
          scope: "source",
          generation: authorityGenerations.get(operation.authority) ?? -1,
        };
  };

  const operationAuthority = (
    operation: SourceWindowOperation,
  ): SourceAuthority =>
    operation.kind === "photo"
      ? (photoWindows.get(operation.authority)?.sourceAuthority ?? authority)
      : operation.authority;

  const operationIsCurrent = (operation: SourceWindowOperation): boolean => {
    if (operation.kind !== "photo") return isCurrent(operation.authority);
    const record = photoWindows.get(operation.authority);
    return (
      record !== undefined &&
      record === currentPhotoWindow &&
      record.sourceAuthority === authority &&
      !record.tasks.halted &&
      !closed
    );
  };

  /// Windows are admitted by aligned start, so Source- and Grid-kind demands
  /// for one window share the Grid scope and its `window:<start>` key: one
  /// in-flight request per window, cancelled when the Grid surface hands off.
  const operationTasks = (operation: SourceWindowOperation) =>
    operation.kind === "photo"
      ? (photoWindows.get(operation.authority)?.tasks ?? haltedPhotoTasks)
      : gridTasks;

  const detachedWindow = (
    operation: SourceWindowOperation,
    start: number,
  ): SourceWindowOutcome => ({
    kind: "detached",
    authority: operationAuthority(operation),
    owner: operationOwner(operation),
    start,
  });

  const startWindowLoad = (
    operation: SourceWindowOperation,
    index: number,
    start: number,
    options: Readonly<{ quiet?: boolean; priority?: "high" | "low" }>,
  ): Promise<SourceWindowOutcome> => {
    const owner = operationOwner(operation);
    const ownerAuthority = operationAuthority(operation);
    const capturedToken = token;
    const expectedTotal = total;
    const tasks = operationTasks(operation);
    const priority = options.priority ?? (options.quiet ? "low" : "high");
    const shared = tasks.joinOrStart<SourceWindowOutcome>(
      `window:${start}`,
      {
        abortTransport: true,
        onCancel: () => detachedWindow(operation, start),
      },
      async (signal) => {
        const result = await fetchBrowseWindow(fetcher, {
          token: capturedToken,
          start,
          limit: WINDOW_SIZE,
          expectedTotal,
          signal: signal!,
          priority,
        });
        if (
          !operationIsCurrent(operation) ||
          token !== capturedToken ||
          signal?.aborted
        )
          return detachedWindow(operation, start);
        if (result.kind === "failed") {
          if (result.status === 404) retryRequired = true;
          if (result.status === 404)
            return {
              kind: "expired",
              authority: ownerAuthority,
              owner,
              start,
              index,
            };
          const end = Math.min(total, start + WINDOW_SIZE);
          const range = `Photos ${start + 1}–${end}`;
          return {
            kind: "failed",
            authority: ownerAuthority,
            owner,
            start,
            range,
            transportLost:
              result.status === undefined && result.malformed !== true,
            ...(result.status !== undefined ? { status: result.status } : {}),
            ...(result.malformed ? { malformed: true as const } : {}),
          };
        }
        for (const [offset, photo] of result.value.photos.entries())
          facts.set(result.value.start + offset, photo);
        // The fallback eviction anchor only moves forward: a window that
        // settles late never re-anchors eviction to a position captured when
        // an older request started.
        latestSettledWindowStart =
          latestSettledWindowStart === undefined
            ? start
            : Math.max(latestSettledWindowStart, start);
        trimFacts();
        if (operation.kind === "source") sourceReady = true;
        return {
          kind: "loaded",
          authority: ownerAuthority,
          owner,
          start,
          changed: true,
        };
      },
    );
    // One notification per completed window however many consumers joined the
    // shared task, and only when no control-flow caller awaits it: a window
    // the open, reopen, retry, or Photo path awaits settles through that
    // caller, whose recovery transition a notification would consume first
    // and strand.
    if (shared.started)
      void shared.promise.then(
        (outcome) => {
          if (awaitedWindowStarts.has(start)) return;
          notifyWindowSettled(outcome);
        },
        () => undefined,
      );
    return shared.promise;
  };

  async function loadWindow(
    index: number,
    operation: SourceWindowOperation,
    options: Readonly<{ quiet?: boolean; priority?: "high" | "low" }> = {},
  ): Promise<SourceWindowOutcome> {
    const start = alignedStart(index);
    const owner = operationOwner(operation);
    if (
      !operationIsCurrent(operation) ||
      !token ||
      total === 0 ||
      operationTasks(operation).halted
    ) {
      if (operationIsCurrent(operation) && total === 0) {
        if (operation.kind === "source") sourceReady = true;
        return { kind: "loaded", authority, owner, start, changed: false };
      }
      return detachedWindow(operation, start);
    }
    if (windowLoaded(start))
      return { kind: "loaded", authority, owner, start, changed: false };
    const releaseAwait = beginAwaitWindow(start);
    try {
      return await startWindowLoad(operation, index, start, options);
    } finally {
      releaseAwait();
    }
  }

  const ensureRange = (
    start: number,
    end: number,
    operation: SourceWindowOperation,
    options: Readonly<{ quiet?: boolean; priority?: "high" | "low" }> = {},
  ): void => {
    if (closed || !Number.isFinite(start) || !Number.isFinite(end)) return;
    if (!operationIsCurrent(operation) || !token) return;
    if (total === 0) {
      if (operation.kind === "source") sourceReady = true;
      return;
    }
    if (operationTasks(operation).halted) return;
    const from = Math.max(0, Math.min(start, total));
    const to = Math.max(from, Math.min(end, total));
    if (from >= to) return;
    visibleRange = { start: from, end: to };
    trimFacts();
    // The clamped tail window rarely sits on the 60-boundary the range start
    // aligns to, so a plain `+= WINDOW_SIZE` walk would skip it (a 400-Photo
    // range [220,400) visits 180, 240, 300 and then the tail 340). Each step
    // is clamped to the last aligned window and the walk stops once it has
    // visited that window exactly once.
    const lastStart = alignedStart(to - 1);
    for (let windowStart = alignedStart(from); ; windowStart += WINDOW_SIZE) {
      const current = Math.min(windowStart, lastStart);
      if (!windowLoaded(current))
        void startWindowLoad(operation, current, current, options);
      if (windowStart >= lastStart) break;
    }
  };

  const findPhotoIndex = (photoId: string): number | undefined => {
    if (closed || !photoId) return undefined;
    for (const [index, photo] of facts) if (photo.id === photoId) return index;
    return undefined;
  };

  const detachedPosition = (
    ownerAuthority: SourceAuthority,
    photoId: string,
  ): SourcePositionOutcome => ({
    kind: "detached",
    authority: ownerAuthority,
    photoId,
  });

  async function resolvePhotoPosition(
    ownerAuthority: SourceAuthority,
    photoId: string,
  ): Promise<SourcePositionOutcome> {
    if (!isCurrent(ownerAuthority) || !token)
      return detachedPosition(ownerAuthority, photoId);
    const retained = findPhotoIndex(photoId);
    if (retained !== undefined)
      return {
        kind: "resolved",
        authority: ownerAuthority,
        photoId,
        position: retained,
      };
    const capturedToken = token;
    let task: ReturnType<TaskScope["beginLatest"]>;
    try {
      task = sourceTasks.beginLatest(`position:${photoId}`, {
        abortTransport: true,
      });
    } catch {
      return detachedPosition(ownerAuthority, photoId);
    }
    try {
      const result = await fetchBrowsePosition(fetcher, {
        token: capturedToken,
        photoId,
        signal: task.signal!,
      });
      if (
        !task.isCurrent() ||
        !isCurrent(ownerAuthority) ||
        token !== capturedToken
      )
        return detachedPosition(ownerAuthority, photoId);
      if (result.kind === "failed") {
        if (result.status === 404)
          return { kind: "expired", authority: ownerAuthority, photoId };
        return {
          kind: "failed",
          authority: ownerAuthority,
          photoId,
          transportLost:
            result.status === undefined && result.malformed !== true,
          ...(result.status !== undefined ? { status: result.status } : {}),
          ...(result.malformed ? { malformed: true as const } : {}),
        };
      }
      const position = result.value.position;
      if (position === null)
        return { kind: "missing", authority: ownerAuthority, photoId };
      if (position < 0 || position >= total)
        return {
          kind: "failed",
          authority: ownerAuthority,
          photoId,
          transportLost: false,
          malformed: true,
        };
      return {
        kind: "resolved",
        authority: ownerAuthority,
        photoId,
        position,
      };
    } finally {
      task.finish();
    }
  }

  const finishImage = (photoId: string, image: GridThumbnailImage) => {
    const current = imageTransfers.get(photoId);
    if (current?.image === image) imageTransfers.delete(photoId);
  };

  const registerImage = (photoId: string, image: GridThumbnailImage) => {
    const existing = imageTransfers.get(photoId);
    if (existing?.image !== image) existing?.finish();
    renderedImages.set(photoId, image);
  };

  const markDeliveryFailed = (
    photoId: string,
    image: GridThumbnailImage,
    failedUrl: string | null,
  ) => {
    if (renderedImages.get(photoId) !== image || closed) return;
    thumbnailDeliveryFailures.delete(photoId);
    thumbnailDeliveryFailures.set(photoId, failedUrl);
    while (thumbnailDeliveryFailures.size > MAX_RETAINED_THUMBNAILS) {
      const oldest = thumbnailDeliveryFailures.keys().next().value;
      if (oldest === undefined) break;
      thumbnailDeliveryFailures.delete(oldest);
    }
    image.removeAttribute("src");
    image.setDeliveryFailed(true);
  };

  const attachThumbnail = (
    photoId: string,
    image: GridThumbnailImage,
    url?: string,
    attachDisconnected = false,
  ) => {
    if (!url || closed || renderedImages.get(photoId) !== image) return;
    registerImage(photoId, image);
    const expectedUrl = new URL(
      url,
      globalThis.location?.href ?? "http://slipstream.test/",
    ).href;
    const failedUrl = thumbnailDeliveryFailures.get(photoId);
    if (failedUrl === expectedUrl) {
      image.setDeliveryFailed(true);
      return;
    }
    if (failedUrl !== undefined) thumbnailDeliveryFailures.delete(photoId);
    image.setDeliveryFailed(false);
    const transfer = gridTasks.beginLatest(`image:${photoId}`, {
      abortTransport: false,
    });
    const finish = () => {
      transfer.finish();
      finishImage(photoId, image);
    };
    const record = { image, finish };
    imageTransfers.set(photoId, record);
    transfer.onCleanup(() => {
      image.onload = null;
      image.onerror = null;
      if (!image.complete && (image.src === expectedUrl || image.src === url))
        image.removeAttribute("src");
      finishImage(photoId, image);
    });
    image.onload = finish;
    image.onerror = () => {
      if (!transfer.isCurrent()) return;
      markDeliveryFailed(photoId, image, expectedUrl);
      finish();
    };
    if (attachDisconnected || image.isConnected) image.src = url;
  };

  const rememberThumbnail = (photoId: string, url: string) => {
    thumbnails.delete(photoId);
    thumbnails.set(photoId, url);
    while (thumbnails.size > MAX_RETAINED_THUMBNAILS) {
      const oldest = thumbnails.keys().next().value;
      if (oldest === undefined) break;
      thumbnails.delete(oldest);
    }
  };

  async function loadThumbnail(
    photoId: string,
    image: GridThumbnailImage,
  ): Promise<void> {
    if (closed) return;
    registerImage(photoId, image);
    const cached = thumbnails.get(photoId);
    if (cached) {
      attachThumbnail(photoId, image, cached, true);
      return;
    }
    if (thumbnailDeliveryFailures.has(photoId)) {
      image.setDeliveryFailed(true);
      return;
    }
    const ownerAuthority = authority;
    const tasks = gridTasks;
    const request = tasks.joinOrStart(
      `thumbnail:${photoId}`,
      { abortTransport: true, onCancel: () => undefined },
      (signal) => fetchThumbnail(fetcher, photoId, signal!),
    );
    const url = await request.promise;
    if (
      !isCurrent(ownerAuthority) ||
      tasks !== gridTasks ||
      request.signal?.aborted
    )
      return;
    // A request may begin before the image-transfer lease exists. The latest
    // registered image object is authoritative while the request is coalesced.
    if (renderedImages.get(photoId) !== image) return;
    if (url) {
      rememberThumbnail(photoId, url);
      attachThumbnail(photoId, image, url);
    } else {
      markDeliveryFailed(photoId, image, null);
    }
  }

  const clearRenderedThumbnails = () => detachImages();

  /// Releases the owner's hold on a Grid image the view dropped or rebuilt:
  /// the browser-managed transfer stops owning a source and the Photo's
  /// delivery-failure memory stays, so a re-rendered cell re-attaches from the
  /// rebuildable URL cache. Retention follows the visible Grid, never the
  /// number of Photos rendered in one session.
  const releaseThumbnail = (photoId: string, image: GridThumbnailImage) => {
    if (renderedImages.get(photoId) !== image) return;
    renderedImages.delete(photoId);
    const transfer = imageTransfers.get(photoId);
    if (transfer?.image !== image) return;
    imageTransfers.delete(photoId);
    transfer.finish();
  };

  return {
    get authority() {
      return authority;
    },
    get generation() {
      return generation;
    },
    get source() {
      return source;
    },
    get order() {
      return viewOrder;
    },
    get selection() {
      return viewSelection;
    },
    get selectionCounts() {
      return selectionCounts;
    },
    get lastSource() {
      return lastSource;
    },
    get kind() {
      return source.kind;
    },
    get albumId() {
      return source.kind === "album" ? source.album.id : undefined;
    },
    get name() {
      return source.kind === "album"
        ? source.album.name
        : source.kind === "folder"
          ? `${source.folder.name} · Folder`
          : "All Photos";
    },
    get folder() {
      return source.kind === "folder" ? source.folder : undefined;
    },
    get token() {
      return token;
    },
    get total() {
      return total;
    },
    get retryRequired() {
      return retryRequired;
    },
    get retainedFactCount() {
      return facts.size;
    },
    get retainedThumbnailCount() {
      return thumbnails.size;
    },
    get retainedThumbnailDeliveryFailureCount() {
      return thumbnailDeliveryFailures.size;
    },
    get retainedImageCount() {
      return renderedImages.size;
    },
    isCurrent,
    isReady(candidate) {
      return isCurrent(candidate) && sourceReady;
    },
    renewPhotoWindow,
    open,
    establish(candidate) {
      if (!isCurrent(candidate)) return false;
      retryRequired = false;
      sourceReady = true;
      return true;
    },
    updateAlbum(album) {
      if (source.kind === "album" && source.album.id === album.id)
        source = freezeSource({ kind: "album", album });
      if (lastSource?.kind === "album" && lastSource.album.id === album.id)
        lastSource = freezeSource({ kind: "album", album });
    },
    photoAt(index) {
      return facts.get(index);
    },
    findPhotoIndex,
    resolvePhotoPosition,
    readGridPosition(candidate) {
      return isCurrent(candidate) ? gridPosition : undefined;
    },
    moveGridPosition(candidate, index) {
      if (!isCurrent(candidate) || index < 0 || index >= total) return false;
      gridPosition = index;
      return true;
    },
    setPhotoPreview(candidate, index, expectedPhotoId, preview) {
      if (!isCurrent(candidate) || index < 0 || index >= total) return false;
      const current = facts.get(index);
      if (!current || current.id !== expectedPhotoId) return false;
      facts.set(index, { ...current, preview });
      return true;
    },
    setPhotoSelection(candidate, index, expectedPhotoId, selectionState) {
      if (!isCurrent(candidate) || index < 0 || index >= total) return false;
      const current = facts.get(index);
      if (!current || current.id !== expectedPhotoId) return false;
      facts.set(index, { ...current, selectionState });
      if (current.selectionState !== selectionState)
        selectionCounts = adjustedSelectionCounts(
          selectionCounts,
          current.selectionState,
          selectionState,
        );
      return true;
    },
    setPhotoRating(candidate, index, expectedPhotoId, rating) {
      if (
        !isCurrent(candidate) ||
        index < 0 ||
        index >= total ||
        !Number.isInteger(rating) ||
        rating < 0 ||
        rating > 5
      )
        return false;
      const current = facts.get(index);
      if (!current || current.id !== expectedPhotoId) return false;
      facts.set(index, { ...current, rating });
      return true;
    },
    invalidateWindow(index) {
      const start = alignedStart(index);
      const end = Math.min(total, start + WINDOW_SIZE);
      for (let offset = start; offset < end; offset += 1) facts.delete(offset);
    },
    trimFacts,
    ensureRange,
    onWindowSettled,
    alignedStart,
    describeWindow(index) {
      const start = alignedStart(index);
      return {
        start,
        range: `Photos ${start + 1}–${Math.min(total, start + WINDOW_SIZE)}`,
      };
    },
    loadWindow,
    stopGridWork,
    loadThumbnail,
    releaseThumbnail,
    presentThumbnail(photoId, image, url, attachDisconnected) {
      registerImage(photoId, image);
      attachThumbnail(photoId, image, url, attachDisconnected);
    },
    clearRenderedThumbnails,
    dispose() {
      if (closed) return;
      closed = true;
      generation += 1;
      authority = makeAuthority();
      windowSettledHandlers.clear();
      detachImages();
      sourceTasks.halt();
      gridTasks.halt();
      stopPhotoWindowWork();
      token = "";
      for (const known of [...knownTokens]) releaseToken(known);
    },
  };
}
