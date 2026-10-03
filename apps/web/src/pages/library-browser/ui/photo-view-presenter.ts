import { formatCaptureTime } from "./capture-time.js";
import type { ViewSelectionState } from "./library-browser-view.js";

export type ViewPreviewSource = "jpeg-original" | "raw-embedded-jpeg";

/** Full wording behind the compact limited-detail marker in the Preview fact. */
const LIMITED_PREVIEW_DETAIL = "Limited by camera Preview resolution";

export type PhotoFactsViewModel = Readonly<{
  index: number;
  total: number;
  originalFilename?: string | undefined;
  selectionState?: ViewSelectionState | undefined;
  rating?: number | undefined;
}>;

export type PhotoMetadataViewModel = Readonly<{
  captureTime?: string;
  aperture?: string;
  iso?: number;
  shutterSpeed?: string;
  focalLength?: string;
}>;

export type PhotoShellViewModel = PhotoFactsViewModel &
  Readonly<{
    sourceName: string;
    photoId?: string | undefined;
    available?: boolean | undefined;
    canReviewRecovery?: boolean | undefined;
    previewSource?: ViewPreviewSource | undefined;
    limitedDetail?: boolean | undefined;
    previewUrl?: string | undefined;
  }>;

export type ReviewImageTarget = {
  readonly connected: boolean;
  readonly source: string;
  setHandlers(onLoad: () => void, onError: () => void): void;
  clearHandlers(): void;
  setSource(resolvedUrl: string): void;
  clearSource(): void;
};

export type ReviewImagePresentation = Readonly<{
  target: ReviewImageTarget;
  resolvedUrl: string;
  surface: object;
}>;

/** Photo operations exposed by the composed Library Browser view. */
export interface PhotoViewPresentation {
  readonly photoStatusSurface: object;
  readonly photoStatusEmpty: boolean;
  isPhotoStatusSurfaceCurrent(this: void, surface: object): boolean;
  setPhotoStatus(this: void, text: string): void;
  renderPhotoFacts(this: void, model: PhotoFactsViewModel): void;
  renderPhotoMetadata(this: void, model?: PhotoMetadataViewModel): void;
  renderPhotoShell(
    this: void,
    model: PhotoShellViewModel,
  ): ReviewImagePresentation | undefined;
  presentReviewImage(
    this: void,
    url: string,
    index: number,
    total: number,
  ): ReviewImagePresentation | undefined;
  /// True while the presented camera Preview is the given URL.
  reviewImageMatches(this: void, url: string): boolean;
  showPreviewUnavailable(this: void, text: string): void;
  setPreviewFacts(
    this: void,
    source: ViewPreviewSource | undefined,
    limited: boolean,
  ): void;
}

export interface PhotoViewPresenter extends PhotoViewPresentation {
  readonly currentPhotoId: string | undefined;
  readonly currentSelection: ViewSelectionState;
  readonly photoSurface: object;
  setSourceTitle(name: string): void;
  clearPreview(): void;
  resetSourceIdentity(): void;
  resetForPhotoEntry(): void;
  dispose(): void;
}

export function createPhotoViewPresenter({
  root,
  zoom,
  renderRating,
  resetGestures,
  reviewRecovery,
}: Readonly<{
  root: HTMLElement;
  zoom: Readonly<{ resetForImage(): void; applyZoom(): void }>;
  renderRating(rating: number): void;
  resetGestures(): void;
  reviewRecovery(): void;
}>): PhotoViewPresenter {
  const title = required<HTMLElement>(root, "[data-photo-title]");
  const position = required<HTMLElement>(root, "[data-position]");
  const filename = required<HTMLElement>(root, "[data-photo-filename]");
  const selection = required<HTMLElement>(root, "[data-selection]");
  const stage = required<HTMLElement>(root, "[data-stage]");
  const status = required<HTMLElement>(root, "[data-status]");
  const previewSource = required<HTMLElement>(root, "[data-source]");
  const detailLimit = required<HTMLElement>(root, "[data-detail-limit]");
  const captureTime = required<HTMLElement>(
    root,
    "[data-metadata-capture-time]",
  );
  const aperture = required<HTMLElement>(root, "[data-metadata-aperture]");
  const iso = required<HTMLElement>(root, "[data-metadata-iso]");
  const shutterSpeed = required<HTMLElement>(
    root,
    "[data-metadata-shutter-speed]",
  );
  const focalLength = required<HTMLElement>(
    root,
    "[data-metadata-focal-length]",
  );
  let alive = true;
  let currentPhotoId: string | undefined;
  let currentSelection: ViewSelectionState = "undecided";
  let photoSurface: object = {};
  let photoStatusSurface: object = {};
  let canReviewRecovery = false;

  const renderPhotoFacts = (model: PhotoFactsViewModel) => {
    if (!alive) return;
    position.textContent = `${model.index + 1} / ${model.total}`;
    filename.textContent = model.originalFilename ?? "—";
    filename.title = model.originalFilename ?? "";
    currentSelection = model.selectionState ?? "undecided";
    selection.textContent = selectionLabel(currentSelection);
    renderRating(model.rating ?? 0);
  };
  const renderPhotoMetadata = (model: PhotoMetadataViewModel = {}) => {
    if (!alive) return;
    captureTime.textContent =
      model.captureTime === undefined
        ? "—"
        : formatCaptureTime(model.captureTime);
    aperture.textContent = model.aperture ?? "—";
    iso.textContent = model.iso === undefined ? "—" : String(model.iso);
    shutterSpeed.textContent = model.shutterSpeed ?? "—";
    focalLength.textContent = model.focalLength ?? "—";
  };
  const presentReviewImage = (
    url: string,
    index: number,
    total: number,
  ): ReviewImagePresentation | undefined => {
    if (!alive) return undefined;
    const surface = photoStatusSurface;
    const image = document.createElement("img");
    image.alt = `Photo ${index + 1} of ${total}`;
    image.draggable = false;
    image.fetchPriority = "high";
    image.decoding = "async";
    stage.replaceChildren(image);
    zoom.resetForImage();
    // Fit depends on the Preview's natural pixels, so the geometry is
    // applied when the bytes arrive.
    image.addEventListener("load", () => {
      if (!alive || !image.isConnected) return;
      zoom.applyZoom();
    });
    const target: ReviewImageTarget = {
      get connected() {
        return image.isConnected;
      },
      get source() {
        return image.src;
      },
      setHandlers(onLoad, onError) {
        image.onload = onLoad;
        image.onerror = onError;
      },
      clearHandlers() {
        image.onload = null;
        image.onerror = null;
      },
      setSource(nextUrl) {
        image.src = nextUrl;
      },
      clearSource() {
        image.removeAttribute("src");
      },
    };
    return {
      target,
      resolvedUrl: new URL(url, window.location.href).href,
      surface,
    };
  };
  const setPreviewFacts = (
    source: ViewPreviewSource | undefined,
    isLimited: boolean,
  ) => {
    if (!alive) return;
    previewSource.textContent = isLimited
      ? `${sourceLabel(source)} · limited detail`
      : sourceLabel(source);
    // The Preview Source fact and the detail-limit explanation are one
    // statement: a limited derivative names the resolution it came from.
    detailLimit.hidden = !isLimited;
    if (isLimited) previewSource.title = LIMITED_PREVIEW_DETAIL;
    else previewSource.removeAttribute("title");
  };
  const renderStatus = (text: string): void => {
    status.replaceChildren();
    if (text) status.append(document.createTextNode(text));
    if (canReviewRecovery) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "summary-action";
      button.textContent = "Review unavailable originals";
      button.addEventListener("click", reviewRecovery);
      status.append(" ", button);
    }
  };
  const setPhotoStatus = (text: string) => {
    if (!alive) return;
    photoStatusSurface = {};
    renderStatus(text);
  };
  const renderPhotoShell = (model: PhotoShellViewModel) => {
    if (!alive) return undefined;
    resetGestures();
    canReviewRecovery = Boolean(
      model.photoId && model.available === false && model.canReviewRecovery,
    );
    title.textContent = model.sourceName;
    photoSurface = {};
    renderPhotoFacts(model);
    renderPhotoMetadata();
    setPreviewFacts(model.previewSource, Boolean(model.limitedDetail));
    let image: ReviewImagePresentation | undefined;
    if (model.previewUrl)
      image = presentReviewImage(model.previewUrl, model.index, model.total);
    else {
      stage.replaceChildren(
        paragraph(model.photoId ? "Loading Preview…" : "Photo unavailable"),
      );
      zoom.resetForImage();
    }
    setPhotoStatus(
      model.photoId && model.available === false
        ? "Original File is unavailable. Decisions remain available."
        : "",
    );
    return image;
  };

  return {
    get currentPhotoId() {
      return currentPhotoId;
    },
    get currentSelection() {
      return currentSelection;
    },
    get photoSurface() {
      return photoSurface;
    },
    get photoStatusSurface() {
      return photoStatusSurface;
    },
    get photoStatusEmpty() {
      return status.textContent === "";
    },
    isPhotoStatusSurfaceCurrent: (surface) =>
      alive && surface === photoStatusSurface,
    setPhotoStatus,
    renderPhotoFacts,
    renderPhotoMetadata,
    renderPhotoShell,
    presentReviewImage,
    setPreviewFacts,
    setSourceTitle(name) {
      if (alive) title.textContent = name;
    },
    clearPreview() {
      if (!alive) return;
      stage.replaceChildren();
      zoom.resetForImage();
    },
    resetSourceIdentity() {
      if (!alive) return;
      currentPhotoId = undefined;
      photoSurface = {};
    },
    resetForPhotoEntry() {
      if (!alive) return;
      zoom.resetForImage();
      photoSurface = {};
    },
    reviewImageMatches(url) {
      if (!alive) return false;
      const image = stage.querySelector<HTMLImageElement>("img");
      return Boolean(
        image && image.src === new URL(url, window.location.href).href,
      );
    },
    showPreviewUnavailable(text) {
      if (alive && !stage.querySelector("img")) {
        stage.replaceChildren(paragraph(text));
        zoom.resetForImage();
      }
    },
    dispose() {
      alive = false;
    },
  };
}

function required<T extends Element>(root: ParentNode, selector: string): T {
  const value = root.querySelector<T>(selector);
  if (!value) throw new Error(`Missing ${selector}`);
  return value;
}

function paragraph(text: string): HTMLParagraphElement {
  const value = document.createElement("p");
  value.textContent = text;
  return value;
}

function selectionLabel(value: ViewSelectionState): string {
  return value === "selected"
    ? "Selected"
    : value === "rejected"
      ? "Rejected"
      : "Undecided";
}

function sourceLabel(source?: ViewPreviewSource): string {
  return source === "jpeg-original"
    ? "JPEG"
    : source === "raw-embedded-jpeg"
      ? "RAW embedded JPEG"
      : "—";
}
