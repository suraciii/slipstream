import type { PhotoSummary } from "../api/contracts.js";
import {
  createPhotoOwner,
  type PhotoFetch,
  type PhotoSourcePort,
  type ReviewImageTransferPort,
} from "./photo-owner.js";
import type {
  PhotoWindowAuthority,
  SourceAuthority,
} from "./source-grid-owner.js";

export type Deferred<T> = Readonly<{
  promise: Promise<T>;
  resolve(value: T): void;
  reject(reason?: unknown): void;
}>;

export const deferred = <T>(): Deferred<T> => {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
};

export const fact = (id: string): PhotoSummary => ({
  id,
  available: true,
  original: { kind: "jpeg", available: true },
  selectionState: "unflagged",
  rating: 0,
  hasSavedEdits: false,
  preview: { state: "inspection-pending" },
});

export const reviewImage = () => {
  let onLoad: (() => void) | undefined;
  let onError: (() => void) | undefined;
  let imageSource = "";
  let clearedSources = 0;
  const image: ReviewImageTransferPort = {
    connected: true,
    get source() {
      return imageSource;
    },
    setHandlers(nextLoad, nextError) {
      onLoad = nextLoad;
      onError = nextError;
    },
    clearHandlers() {
      onLoad = undefined;
      onError = undefined;
    },
    setSource(next) {
      imageSource = next;
    },
    clearSource() {
      clearedSources += 1;
      imageSource = "";
    },
  };
  return {
    image,
    load: () => onLoad?.(),
    get source() {
      return imageSource;
    },
    get clearedSources() {
      return clearedSources;
    },
    get hasHandlers() {
      return onLoad !== undefined || onError !== undefined;
    },
  };
};

export const sourceAuthority = () => Object.freeze({}) as SourceAuthority;

export class FakeSource implements PhotoSourcePort {
  authority = sourceAuthority();
  facts = new Map<number, PhotoSummary>();
  decisions = new Map<
    string,
    Readonly<{ field: "selectionState" | "rating"; value: unknown }>
  >();
  moved: number[] = [];
  trimmed: number[] = [];
  windows: PhotoWindowAuthority[] = [];

  isSourceCurrent(authority: SourceAuthority): boolean {
    return authority === this.authority;
  }
  renewPhotoWindow(
    authority: SourceAuthority,
  ): PhotoWindowAuthority | undefined {
    if (!this.isSourceCurrent(authority)) return undefined;
    const value = Object.freeze({}) as PhotoWindowAuthority;
    this.windows.push(value);
    return value;
  }
  photoAt(authority: SourceAuthority, index: number): PhotoSummary | undefined {
    return this.isSourceCurrent(authority) ? this.facts.get(index) : undefined;
  }
  findPhotoIndex(photoId: string): number | undefined {
    for (const [index, photo] of this.facts)
      if (photo.id === photoId) return index;
    return undefined;
  }
  movePosition(authority: SourceAuthority, index: number): boolean {
    if (!this.isSourceCurrent(authority)) return false;
    this.moved.push(index);
    return true;
  }
  patchPreview(
    authority: SourceAuthority,
    index: number,
    photoId: string,
    preview: PhotoSummary["preview"],
  ): boolean {
    return this.patch(authority, index, photoId, { preview });
  }
  patchSelection(
    authority: SourceAuthority,
    index: number,
    photoId: string,
    selectionState: PhotoSummary["selectionState"],
  ): boolean {
    return this.patch(authority, index, photoId, { selectionState });
  }
  patchRating(
    authority: SourceAuthority,
    index: number,
    photoId: string,
    rating: number,
  ): boolean {
    return this.patch(authority, index, photoId, { rating });
  }
  applyBatchSelection(
    authority: SourceAuthority,
    photoId: string,
    _priorValue: PhotoSummary["selectionState"],
    selectionState: PhotoSummary["selectionState"],
  ): boolean {
    const index = this.findPhotoIndex(photoId);
    if (index === undefined) return this.isSourceCurrent(authority);
    return this.patch(authority, index, photoId, { selectionState });
  }
  noteCommittedDecision(
    authority: SourceAuthority,
    photoId: string,
    field: "selectionState" | "rating",
    value: PhotoSummary["selectionState"] | number,
  ): void {
    if (!this.isSourceCurrent(authority)) return;
    this.decisions.set(photoId, { field, value });
  }
  trimFacts(authority: SourceAuthority, anchor: number): void {
    if (this.isSourceCurrent(authority)) this.trimmed.push(anchor);
  }
  patch(
    authority: SourceAuthority,
    index: number,
    photoId: string,
    value: Partial<PhotoSummary>,
  ): boolean {
    const current = this.photoAt(authority, index);
    if (!current || current.id !== photoId) return false;
    this.facts.set(index, Object.freeze({ ...current, ...value }));
    return true;
  }
}

export const batchPhotos = (source: FakeSource, ids: ReadonlyArray<string>) =>
  ids.map((photoId) => ({
    photoId,
    expectedCurrent:
      [...source.facts.values()].find((photo) => photo.id === photoId)
        ?.selectionState ?? "unflagged",
  }));

export const bind = (source: FakeSource, fetcher: PhotoFetch) => {
  const owner = createPhotoOwner(fetcher, source);
  owner.bindSource({
    sourceAuthority: source.authority,
    total: source.facts.size,
    index: 0,
    albumId: "album-1",
  });
  const opened = owner.beginOpen(0);
  if (!opened) throw new Error("expected Photo open");
  owner.commitOpen(opened);
  return { owner, opened };
};

export const mutationBody = (undo: unknown): Response =>
  new Response(JSON.stringify({ undo }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
