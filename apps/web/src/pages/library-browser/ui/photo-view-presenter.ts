import { formatCaptureTime } from "./capture-time.js";
import type { ViewSelectionState } from "./library-browser-view.js";
import { paragraph, required, selectionLabel } from "./presentation-helpers.js";

export type ViewPreviewSource = "jpeg-original" | "raw-embedded-jpeg";

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
  setPreviewDisplay(
    dimensions: Readonly<{ width: number; height: number }> | undefined,
    enlarged: boolean,
  ): void;
  setSourceTitle(name: string): void;
  clearPreview(): void;
  resetSourceIdentity(): void;
  resetForPhotoEntry(): void;
  dispose(): void;
}

export function createPhotoViewPresenter({
  root,
  zoom,
  resetGestures,
  reviewRecovery,
}: Readonly<{
  root: HTMLElement;
  zoom: Readonly<{ resetForImage(): void; applyZoom(): void }>;
  resetGestures(): void;
  reviewRecovery(): void;
}>): PhotoViewPresenter {
  const title = required<HTMLElement>(root, "[data-photo-title]");
  const position = required<HTMLElement>(root, "[data-position]");
  const filename = required<HTMLElement>(root, "[data-photo-filename]");
  const selection = required<HTMLElement>(root, "[data-selection]");
  const selectionChip = required<HTMLElement>(root, "[data-selection-chip]");
  const stage = required<HTMLElement>(root, "[data-stage]");
  const ambient = required<HTMLElement>(root, "[data-preview-ambient]");
  const factsKind = required<HTMLElement>(root, "[data-photo-facts-kind]");
  const factsCapture = required<HTMLElement>(
    root,
    "[data-photo-facts-capture]",
  );
  const factsAvailability = required<HTMLElement>(
    root,
    "[data-photo-facts-availability]",
  );
  const status = required<HTMLElement>(root, "[data-status]");
  const previewSource = required<HTMLElement>(root, "[data-source]");
  const previewDescription = required<HTMLElement>(
    root,
    "[data-preview-description]",
  );
  const previewHelp = required<HTMLElement>(root, "[data-preview-help]");
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
  let currentSelection: ViewSelectionState = "unflagged";
  let photoSurface: object = {};
  let photoStatusSurface: object = {};
  let source: ViewPreviewSource | undefined;
  let limited = false;
  let dimensions: Readonly<{ width: number; height: number }> | undefined;
  let enlarged = false;
  let canReviewRecovery = false;

  const renderPhotoFacts = (model: PhotoFactsViewModel) => {
    if (!alive) return;
    position.textContent = `${(model.index + 1).toLocaleString()} / ${model.total.toLocaleString()}`;
    filename.textContent = model.originalFilename ?? "—";
    filename.title = model.originalFilename ?? "";
    currentSelection = model.selectionState ?? "unflagged";
    selectionChip.textContent = selectionLabel(currentSelection);
    selection.setAttribute(
      "aria-label",
      `Selection State: ${selectionLabel(currentSelection)}`,
    );
  };
  const renderPhotoMetadata = (model: PhotoMetadataViewModel = {}) => {
    if (!alive) return;
    captureTime.textContent =
      model.captureTime === undefined
        ? "—"
        : formatCaptureTime(model.captureTime);
    factsCapture.textContent = captureTime.textContent;
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
    const resolvedUrl = new URL(url, window.location.href).href;
    // The matte copies a Preview that actually exists. Show it with the sharp
    // image, so a failed request never leaves an ambient image behind.
    ambient.hidden = true;
    ambient.style.backgroundImage = `url("${resolvedUrl.replaceAll('"', "%22")}")`;
    const image = document.createElement("img");
    image.alt = `Photo ${index + 1} of ${total}`;
    image.draggable = false;
    image.fetchPriority = "high";
    image.decoding = "async";
    stage.replaceChildren(image);
    zoom.resetForImage();
    // Fit depends on the Preview's natural pixels, so the geometry is
    // applied when the bytes arrive.
    image.dataset.measured = "false";
    image.addEventListener("load", () => {
      if (!alive || !image.isConnected) return;
      ambient.hidden = false;
      zoom.applyZoom();
      image.dataset.measured = "true";
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
      resolvedUrl,
      surface,
    };
  };
  const renderPreviewEvidence = () => {
    if (!alive) return;
    if (!source) {
      previewSource.textContent = sourceLabel(source);
      previewDescription.textContent = previewSource.textContent;
      previewHelp.textContent = previewSource.textContent;
      return;
    }
    const parts = [sourceLabel(source)];
    if (dimensions) parts.push(`${dimensions.width} × ${dimensions.height}`);
    parts.push(limited ? "limited detail" : "normal review");
    if (dimensions)
      parts.push(
        enlarged
          ? "Enlarged Preview pixels cannot add source detail"
          : "Preview pixels are not enlarged",
      );
    previewSource.textContent = parts.join(" · ");
    const explanation = `${parts.join(" · ")}. Preview detail is limited by its source pixels, not the Original's full resolution.`;
    previewDescription.textContent = explanation;
    previewHelp.textContent = explanation;
  };
  const setPreviewFacts = (
    nextSource: ViewPreviewSource | undefined,
    isLimited: boolean,
  ) => {
    source = nextSource;
    limited = isLimited;
    if (nextSource)
      factsKind.textContent =
        nextSource === "raw-embedded-jpeg" ? "RAW" : "JPEG";
    renderPreviewEvidence();
  };
  const renderStatus = (text: string): void => {
    status.hidden = !text && !canReviewRecovery;
    status.replaceChildren();
    if (text) status.append(document.createTextNode(text));
    if (canReviewRecovery) {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = "Review unavailable Originals";
      button.addEventListener("click", reviewRecovery);
      status.append(" ", button);
    }
  };
  const setPhotoStatus = (text: string) => {
    if (!alive) return;
    photoStatusSurface = {};
    renderStatus(text);
  };
  const clearAmbient = () => {
    ambient.hidden = true;
    ambient.style.removeProperty("background-image");
  };
  const kindLabel = (model: PhotoShellViewModel): string => {
    if (model.previewSource === "raw-embedded-jpeg") return "RAW";
    if (model.previewSource === "jpeg-original") return "JPEG";
    const extension = model.originalFilename?.split(".").at(-1)?.toLowerCase();
    if (!extension) return "—";
    return extension === "jpg" || extension === "jpeg" ? "JPEG" : "RAW";
  };
  const renderPhotoShell = (model: PhotoShellViewModel) => {
    if (!alive) return undefined;
    resetGestures();
    canReviewRecovery = Boolean(model.photoId && model.available === false);
    title.textContent = model.sourceName;
    currentPhotoId = model.photoId;
    photoSurface = {};
    renderPhotoFacts(model);
    renderPhotoMetadata();
    setPreviewFacts(model.previewSource, Boolean(model.limitedDetail));
    factsKind.textContent = kindLabel(model);
    factsAvailability.textContent =
      model.photoId === undefined
        ? "—"
        : model.available === false
          ? "Unavailable"
          : "Available";
    factsAvailability.dataset.available = String(model.available !== false);
    let image: ReviewImagePresentation | undefined;
    if (model.previewUrl)
      image = presentReviewImage(model.previewUrl, model.index, model.total);
    else {
      clearAmbient();
      stage.replaceChildren(
        paragraph(
          !model.photoId
            ? "Photo unavailable"
            : model.available === false
              ? "Preview unavailable"
              : "Loading Preview…",
        ),
      );
      zoom.resetForImage();
    }
    setPhotoStatus(
      model.photoId && model.available === false
        ? "Current Photo: Original File is unavailable. Decisions remain available."
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
    setPreviewDisplay(nextDimensions, nextEnlarged) {
      dimensions = nextDimensions;
      enlarged = nextEnlarged;
      renderPreviewEvidence();
    },
    setPreviewFacts,
    setSourceTitle(name) {
      if (alive) title.textContent = name;
    },
    clearPreview() {
      if (!alive) return;
      clearAmbient();
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
      if (!alive) return;
      clearAmbient();
      if (stage.querySelector("img")) return;
      stage.replaceChildren(paragraph(text));
      zoom.resetForImage();
    },
    dispose() {
      alive = false;
    },
  };
}

function sourceLabel(source?: ViewPreviewSource): string {
  return source === "jpeg-original"
    ? "JPEG"
    : source === "raw-embedded-jpeg"
      ? "RAW embedded JPEG"
      : "—";
}
