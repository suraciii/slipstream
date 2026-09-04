import type { PhotoSummary } from "../api/contracts.js";
import {
  fetchBrowseWindow,
  fetchThumbnail,
  openBrowse,
  releaseBrowse,
  type BrowseSourceRequest,
  type SourceGridFetch,
} from "../api/source-grid.js";
import { TaskScope } from "./async-ownership.js";

const WINDOW_SIZE = 60;
const MAX_RETAINED_FACTS = WINDOW_SIZE * 3;
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
  isCurrent(authority: SourceAuthority): boolean;
  isReady(authority: SourceAuthority): boolean;
  renewPhotoWindow(): PhotoWindowAuthority;
  open(
    source: SourceGridSource,
    options?: Readonly<{
      preferredPhotoId?: string;
      mode?: "replace" | "reopen";
    }>,
  ): Promise<SourceOpenOutcome>;
  establish(authority: SourceAuthority): boolean;
  updateAlbum(album: Readonly<{ id: string; name: string }>): void;
  photoAt(index: number): PhotoSummary | undefined;
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
  trimFacts(anchor: number): void;
  alignedStart(index: number): number;
  describeWindow(index: number): Readonly<{ start: number; range: string }>;
  loadWindow(
    index: number,
    operation: SourceWindowOperation,
    options?: Readonly<{ quiet?: boolean; priority?: "high" | "low" }>,
  ): Promise<SourceWindowOutcome>;
  stopGridWork(): void;
  beginGridRender(): void;
  loadThumbnail(photoId: string, image: GridThumbnailImage): Promise<void>;
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

const sourceRequest = (
  source: SourceGridSource,
  preferredPhotoId?: string,
): BrowseSourceRequest =>
  source.kind === "library"
    ? { kind: "library", ...(preferredPhotoId ? { preferredPhotoId } : {}) }
    : source.kind === "album"
      ? {
          kind: "album",
          albumId: source.album.id,
          ...(preferredPhotoId ? { preferredPhotoId } : {}),
        }
      : {
          kind: "folder",
          folderPath: source.folder.location,
          publication: source.publication,
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

  const trimFacts = (anchor: number) => {
    if (facts.size <= MAX_RETAINED_FACTS) return;
    for (const index of [...facts.keys()])
      if (
        Math.abs(index - anchor) > WINDOW_SIZE &&
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
    lastSource = source;
    token = "";
    if (priorToken) releaseToken(priorToken);
    retryRequired = false;
    if (mode === "replace") {
      sourceReady = false;
      total = 0;
      gridPosition = 0;
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
        sourceRequest(source, options.preferredPhotoId),
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

  const operationTasks = (operation: SourceWindowOperation) =>
    operation.kind === "photo"
      ? (photoWindows.get(operation.authority)?.tasks ?? haltedPhotoTasks)
      : operation.kind === "source"
        ? sourceTasks
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

  async function loadWindow(
    index: number,
    operation: SourceWindowOperation,
    options: Readonly<{ quiet?: boolean; priority?: "high" | "low" }> = {},
  ): Promise<SourceWindowOutcome> {
    const start = alignedStart(index);
    const owner = operationOwner(operation);
    const ownerAuthority = operationAuthority(operation);
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
        trimFacts(index);
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
    return shared.promise;
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

  const beginGridRender = () => detachImages();

  const clearRenderedThumbnails = () => detachImages();

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
    beginGridRender,
    loadThumbnail,
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
      detachImages();
      sourceTasks.halt();
      gridTasks.halt();
      stopPhotoWindowWork();
      token = "";
      for (const known of [...knownTokens]) releaseToken(known);
    },
  };
}
