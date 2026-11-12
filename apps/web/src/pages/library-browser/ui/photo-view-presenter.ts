import { formatCaptureTime } from "./capture-time.js";
import type { ViewSelectionState } from "./library-browser-view.js";
import { isPhotoSequenceEnd } from "./photo-navigation-controls.js";
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
  photoSurface: object;
}>;

/** Photo operations exposed by the composed Library Browser view. */
export interface PhotoViewPresentation {
  readonly photoStatusSurface: object;
  readonly photoStatusEmpty: boolean;
  isPhotoStatusSurfaceCurrent(this: void, surface: object): boolean;
  isReviewImageSurfaceCurrent(this: void, surface: object): boolean;
  /// Presents a transient status message. Empty text restores the
  /// informational status the current Photo shell owns.
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
  const boundary = required<HTMLElement>(root, "[data-photo-boundary]");
  const filename = required<HTMLElement>(root, "[data-photo-filename]");
  const selection = required<HTMLElement>(root, "[data-selection]");
  const selectionChip = required<HTMLElement>(root, "[data-selection-chip]");
  const stage = required<HTMLElement>(root, "[data-stage]");
  const ambient = required<HTMLElement>(root, "[data-preview-ambient]");
  const factsCapture = required<HTMLElement>(
    root,
    "[data-photo-facts-capture] [data-photo-facts-value]",
  );
  const factsAperture = required<HTMLElement>(
    root,
    "[data-photo-facts-aperture] [data-photo-facts-value]",
  );
  const factsShutterSpeed = required<HTMLElement>(
    root,
    "[data-photo-facts-shutter-speed] [data-photo-facts-value]",
  );
  const factsIso = required<HTMLElement>(
    root,
    "[data-photo-facts-iso] [data-photo-facts-value]",
  );
  const factsFocalLength = required<HTMLElement>(
    root,
    "[data-photo-facts-focal-length] [data-photo-facts-value]",
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
  const detailsIdentity = root.querySelector<HTMLElement>(
    "[data-details-identity]",
  );
  const metadataEmpty = root.querySelector<HTMLElement>(
    "[data-metadata-empty]",
  );
  const metadataRows = {
    captureTime: root.querySelector<HTMLElement>(
      "[data-metadata-row='captureTime']",
    ),
    aperture: root.querySelector<HTMLElement>("[data-metadata-row='aperture']"),
    iso: root.querySelector<HTMLElement>("[data-metadata-row='iso']"),
    shutterSpeed: root.querySelector<HTMLElement>(
      "[data-metadata-row='shutterSpeed']",
    ),
    focalLength: root.querySelector<HTMLElement>(
      "[data-metadata-row='focalLength']",
    ),
  };
  let alive = true;
  let currentPhotoId: string | undefined;
  let currentSelection: ViewSelectionState = "unflagged";
  let photoSurface: object = {};
  let photoStatusSurface: object = {};
  let source: ViewPreviewSource | undefined;
  let limited = false;
  let baseStatusText = "";
  let dimensions: Readonly<{ width: number; height: number }> | undefined;
  let enlarged = false;
  let canReviewRecovery = false;

  const renderPhotoFacts = (model: PhotoFactsViewModel) => {
    if (!alive) return;
    position.textContent = `${(model.index + 1).toLocaleString()} / ${model.total.toLocaleString()}`;
    const atEnd = isPhotoSequenceEnd(model.index, model.total);
    boundary.textContent = atEnd ? "End of view" : "";
    boundary.hidden = !atEnd;
    filename.textContent = model.originalFilename ?? "—";
    filename.title = model.originalFilename ?? "";
    if (detailsIdentity)
      detailsIdentity.textContent = `${model.originalFilename ?? "Photo"} · ${position.textContent}`;
    currentSelection = model.selectionState ?? "unflagged";
    selectionChip.textContent = selectionLabel(currentSelection);
    selection.setAttribute(
      "aria-label",
      `Selection State: ${selectionLabel(currentSelection)}`,
    );
  };
  const renderPhotoMetadata = (model: PhotoMetadataViewModel = {}) => {
    if (!alive) return;
    const captureTimeValue =
      model.captureTime === undefined
        ? undefined
        : formatCaptureTime(model.captureTime);
    captureTime.textContent = captureTimeValue ?? "—";
    factsCapture.textContent = captureTime.textContent;
    aperture.textContent = model.aperture ?? "—";
    factsAperture.textContent = aperture.textContent;
    iso.textContent = model.iso === undefined ? "—" : String(model.iso);
    factsIso.textContent =
      model.iso === undefined ? "—" : `ISO ${String(model.iso)}`;
    shutterSpeed.textContent = model.shutterSpeed ?? "—";
    factsShutterSpeed.textContent = shutterSpeed.textContent;
    focalLength.textContent = model.focalLength ?? "—";
    factsFocalLength.textContent = focalLength.textContent;
    // Details is a reading surface: show the facts that exist and one clear
    // empty state instead of five rows of dashes.
    const rows = [
      [metadataRows.captureTime, captureTimeValue],
      [metadataRows.aperture, model.aperture],
      [metadataRows.iso, model.iso],
      [metadataRows.shutterSpeed, model.shutterSpeed],
      [metadataRows.focalLength, model.focalLength],
    ] as const;
    for (const [row, value] of rows) if (row) row.hidden = value === undefined;
    if (metadataEmpty)
      metadataEmpty.hidden = rows.some(([, value]) => value !== undefined);
  };
  const presentReviewImage = (
    url: string,
    index: number,
    total: number,
  ): ReviewImagePresentation | undefined => {
    if (!alive) return undefined;
    const surface = photoStatusSurface;
    const imageSurface = photoSurface;
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
      photoSurface: imageSurface,
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
    // Details names the Preview source and its actual pixels. The complete
    // detail-limit explanation stays beside the Preview and in Help.
    previewSource.textContent = dimensions
      ? `${sourceLabel(source)} · ${dimensions.width} × ${dimensions.height}`
      : sourceLabel(source);
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
    renderStatus(text || baseStatusText);
  };
  const clearAmbient = () => {
    ambient.hidden = true;
    ambient.style.removeProperty("background-image");
  };
  const renderPhotoShell = (model: PhotoShellViewModel) => {
    if (!alive) return undefined;
    resetGestures();
    canReviewRecovery = Boolean(model.photoId && model.available === false);
    title.textContent = model.sourceName;
    title.title = model.sourceName;
    currentPhotoId = model.photoId;
    photoSurface = {};
    renderPhotoFacts(model);
    renderPhotoMetadata();
    setPreviewFacts(model.previewSource, Boolean(model.limitedDetail));
    baseStatusText =
      model.photoId && model.available === false
        ? "Current Photo: Original File is unavailable. Decisions remain available."
        : "";
    setPhotoStatus(baseStatusText);
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
    isReviewImageSurfaceCurrent: (surface) => alive && surface === photoSurface,
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
      if (!alive) return;
      title.textContent = name;
      title.title = name;
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
