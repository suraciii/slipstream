/// The Grid's Thumbnails: the bounded rebuildable URL cache, the terminal
/// Thumbnail answers, the delivery-failure memory, and the browser-managed
/// image transfers that render a Photo's cell. The owner keeps every fact a
/// Thumbnail carries, the admission guards that decide which registered image
/// a settled answer may still mutate, and the cancellation that stops the
/// Thumbnail work when the Grid hands off; the source owner presents the
/// projections it computes and supplies the source lifetime every request is
/// checked against.

import type { PhotoSummary } from "../api/contracts.js";
import { fetchThumbnail, type SourceGridFetch } from "../api/source-grid.js";
import { TaskScope } from "./async-ownership.js";
import type { SourceAuthority } from "./source-grid-owner.js";

/// Thumbnail retention follows the visible Grid, never the number of Photos
/// rendered in one session: four Grid windows of retained Photos bound the
/// rebuildable URL cache, the delivery-failure memory, and the terminal
/// answers alike, independently of the Library total.
const MAX_RETAINED_THUMBNAILS = 60 * 4;

export interface GridThumbnailImage {
  complete: boolean;
  isConnected: boolean;
  src: string;
  onload: GlobalEventHandlers["onload"];
  onerror: GlobalEventHandlers["onerror"];
  removeAttribute(name: string): void;
  setDeliveryFailed(failed: boolean): void;
  setThumbnailState(state: "unavailable" | "failed"): void;
}

/// One browser-managed image transfer: the registered image and the finish
/// that releases the lease holding its source.
type ImageTransfer = Readonly<{
  image: GridThumbnailImage;
  finish: () => void;
}>;

/// The source lifetime a Thumbnail request is checked against. A request that
/// outlives the source generation that admitted it must never mutate an image,
/// so the owner captures the authority current when it starts one and asks the
/// source whether that authority still owns the Grid.
export type GridThumbnailSourcePort = Readonly<{
  currentAuthority: () => SourceAuthority;
  isCurrent: (authority: SourceAuthority) => boolean;
}>;

export interface GridThumbnailOwner {
  readonly retainedThumbnailCount: number;
  readonly retainedThumbnailDeliveryFailureCount: number;
  readonly retainedImageCount: number;
  loadThumbnail(
    this: void,
    photoId: string,
    image: GridThumbnailImage,
  ): Promise<void>;
  releaseThumbnail(
    this: void,
    photoId: string,
    image: GridThumbnailImage,
  ): void;
  presentThumbnail(
    this: void,
    photoId: string,
    image: GridThumbnailImage,
    url?: string,
    attachDisconnected?: boolean,
  ): void;
  clearRenderedThumbnails(this: void): void;
  /// A Review Preview fact that changed supersedes the remembered terminal
  /// answer: a Photo whose Preview state moved at all, or one that gained a
  /// readable Original, is requested again instead of reusing an answer
  /// recorded for the fact the Grid held before.
  notePreviewReplaced(
    this: void,
    photoId: string,
    prior: PhotoSummary["preview"],
    next: PhotoSummary["preview"],
  ): void;
  /// Drops the remembered answers a replacement source must not reuse.
  reset(this: void): void;
  /// Detaches every rendered image and stops the in-flight Thumbnail work.
  stop(this: void): void;
  dispose(this: void): void;
}

export function createGridThumbnailOwner(
  fetcher: SourceGridFetch,
  source: GridThumbnailSourcePort,
): GridThumbnailOwner {
  let closed = false;
  let thumbnails = new Map<string, string>();
  // null means the endpoint failed before it supplied a Thumbnail URL.
  let thumbnailDeliveryFailures = new Map<string, string | null>();
  // Terminal Thumbnail results are distinct from Review Preview facts.
  let thumbnailStates = new Map<string, "unavailable" | "failed">();
  const renderedImages = new Map<string, GridThumbnailImage>();
  const imageTransfers = new Map<string, ImageTransfer>();
  let tasks = new TaskScope();

  const detachImages = () => {
    for (const transfer of [...imageTransfers.values()]) transfer.finish();
    imageTransfers.clear();
    renderedImages.clear();
  };

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
    // A real URL supersedes an earlier terminal Thumbnail answer.
    thumbnailStates.delete(photoId);
    image.setDeliveryFailed(false);
    const transfer = tasks.beginLatest(`image:${photoId}`, {
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

  const noteTerminalThumbnailState = (
    photoId: string,
    image: GridThumbnailImage,
    state: "unavailable" | "failed",
  ) => {
    if (renderedImages.get(photoId) !== image || closed) return;
    thumbnailStates.delete(photoId);
    thumbnailStates.set(photoId, state);
    while (thumbnailStates.size > MAX_RETAINED_THUMBNAILS) {
      const oldest = thumbnailStates.keys().next().value;
      if (oldest === undefined) break;
      thumbnailStates.delete(oldest);
    }
    image.setThumbnailState(state);
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
    const remembered = thumbnailStates.get(photoId);
    if (remembered) {
      image.setThumbnailState(remembered);
      return;
    }
    if (thumbnailDeliveryFailures.has(photoId)) {
      image.setDeliveryFailed(true);
      return;
    }
    const ownerAuthority = source.currentAuthority();
    const requestScope = tasks;
    const request = requestScope.joinOrStart(
      `thumbnail:${photoId}`,
      { abortTransport: true, onCancel: () => undefined },
      (signal) => fetchThumbnail(fetcher, photoId, signal!),
    );
    const outcome = await request.promise;
    // A cancelled request resolves with the onCancel value, no outcome.
    if (
      outcome === undefined ||
      !source.isCurrent(ownerAuthority) ||
      requestScope !== tasks ||
      request.signal?.aborted
    )
      return;
    // A request may begin before the image-transfer lease exists. The latest
    // registered image object is authoritative while the request is coalesced.
    if (renderedImages.get(photoId) !== image) return;
    if (outcome.kind === "ready") {
      rememberThumbnail(photoId, outcome.url);
      attachThumbnail(photoId, image, outcome.url);
    } else if (outcome.kind === "not-ready") {
      noteTerminalThumbnailState(photoId, image, outcome.state);
    } else {
      markDeliveryFailed(photoId, image, null);
    }
  }

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

  const presentThumbnail = (
    photoId: string,
    image: GridThumbnailImage,
    url?: string,
    attachDisconnected?: boolean,
  ) => {
    registerImage(photoId, image);
    attachThumbnail(photoId, image, url, attachDisconnected);
  };

  const notePreviewReplaced = (
    photoId: string,
    prior: PhotoSummary["preview"],
    next: PhotoSummary["preview"],
  ) => {
    if (next.state !== prior.state || next.state === "ready")
      thumbnailStates.delete(photoId);
  };

  const reset = () => {
    thumbnails = new Map();
    thumbnailDeliveryFailures = new Map();
    thumbnailStates = new Map();
  };

  const stop = () => {
    detachImages();
    tasks.halt();
    tasks = new TaskScope();
  };

  const dispose = () => {
    if (closed) return;
    closed = true;
    detachImages();
    tasks.halt();
  };

  return {
    get retainedThumbnailCount() {
      return thumbnails.size;
    },
    get retainedThumbnailDeliveryFailureCount() {
      return thumbnailDeliveryFailures.size;
    },
    get retainedImageCount() {
      return renderedImages.size;
    },
    loadThumbnail,
    releaseThumbnail,
    presentThumbnail,
    clearRenderedThumbnails: detachImages,
    notePreviewReplaced,
    reset,
    stop,
    dispose,
  };
}
