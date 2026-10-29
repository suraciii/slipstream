import "./thumbnail-placeholder.css";
import { selectionLabel } from "./presentation-helpers.js";

export type GridSelectionState = "unflagged" | "picked" | "rejected";

export type GridPhotoPreview = Readonly<{
  state: "inspection-pending" | "ready" | "unavailable" | "failed";
  thumbnailUrl?: string;
}>;

export type GridThumbnailTarget = {
  readonly complete: boolean;
  readonly isConnected: boolean;
  src: string;
  onload: GlobalEventHandlers["onload"];
  onerror: GlobalEventHandlers["onerror"];
  removeAttribute(name: string): void;
  setDeliveryFailed(failed: boolean): void;
  setThumbnailState(state: "unavailable" | "failed"): void;
};
export type GridThumbnailBinding = Readonly<{
  photoId: string;
  preview: GridPhotoPreview;
  target: GridThumbnailTarget;
}>;

export type GridPhotoViewModel = Readonly<{
  id: string;
  available: boolean;
  original: Readonly<{ kind: "raw" | "jpeg"; available: boolean }>;
  originalFilename?: string;
  selectionState: GridSelectionState;
  rating: number;
  hasSavedEdits: boolean;
  preview: GridPhotoPreview;
}>;

export type GridCell = {
  readonly cell: HTMLButtonElement;
  signature: string;
  deliveryFailed: boolean;
  thumbnailState: "unavailable" | "failed" | undefined;
  thumbnail: GridThumbnailBinding | undefined;
};

type GridCellPresenterOptions = Readonly<{
  compact: () => boolean;
  gapX: () => number;
  insetX: () => number;
  insetY: () => number;
  rowPitch: () => number;
  interactionEnabled: () => boolean;
  multiSelected: (index: number) => boolean;
  multiMode: () => boolean;
  send: (index: number, event: MouseEvent) => void;
  bindThumbnail: (binding: GridThumbnailBinding) => void;
  releaseThumbnail: (binding: GridThumbnailBinding) => void;
  target: (
    image: HTMLImageElement,
    setDeliveryFailed: (failed: boolean) => void,
    setThumbnailState: (state: "unavailable" | "failed") => void,
  ) => GridThumbnailTarget;
}>;

const LOADING_CELL_SIGNATURE = "loading";

function gridPhotoFacts(
  photo: GridPhotoViewModel,
  deliveryFailed: boolean,
  thumbnailState?: "unavailable" | "failed",
): string[] {
  const facts: string[] = [];
  const originalUnavailable = !photo.original.available;
  if (originalUnavailable) facts.push("Original unavailable");
  else if (!photo.available) facts.push("Photo unavailable");
  if (photo.original.kind === "raw") facts.push("RAW");
  if (photo.hasSavedEdits) facts.push("Edited");
  if (photo.preview.state === "unavailable" && !originalUnavailable)
    facts.push("Preview unavailable");
  if (photo.preview.state === "failed") facts.push("Preview failed");
  if (thumbnailState === "unavailable" && photo.preview.state !== "unavailable")
    facts.push("Thumbnail unavailable");
  if (thumbnailState === "failed") facts.push("Thumbnail failed");
  if (deliveryFailed) facts.push("Thumbnail delivery failed");
  return facts;
}

function gridPhotoIssues(
  photo: GridPhotoViewModel,
  deliveryFailed: boolean,
  thumbnailState?: "unavailable" | "failed",
): string[] {
  const issues: string[] = [];
  const originalUnavailable = !photo.original.available;
  if (originalUnavailable) issues.push("Original unavailable");
  else if (!photo.available) issues.push("Photo unavailable");
  if (photo.preview.state === "unavailable" && !originalUnavailable)
    issues.push("Preview unavailable");
  if (photo.preview.state === "failed") issues.push("Preview failed");
  if (thumbnailState === "unavailable" && photo.preview.state !== "unavailable")
    issues.push("Thumbnail unavailable");
  if (thumbnailState === "failed") issues.push("Thumbnail failed");
  if (deliveryFailed) issues.push("Thumbnail delivery failed");
  return issues;
}

function gridCellSignature(
  index: number,
  photo: GridPhotoViewModel | undefined,
  deliveryFailed: boolean,
): string {
  if (!photo) return LOADING_CELL_SIGNATURE;
  return [
    String(index),
    photo.id,
    photo.originalFilename ?? "",
    photo.available ? "available" : "unavailable",
    photo.original.kind,
    photo.selectionState,
    String(photo.rating),
    photo.hasSavedEdits ? "edited" : "unedited",
    photo.preview.state,
    photo.preview.thumbnailUrl ?? "",
    deliveryFailed ? "delivery-failed" : "delivered",
  ].join("|");
}

export function createGridCellPresenter(options: GridCellPresenterOptions) {
  const {
    compact,
    gapX,
    insetX,
    insetY,
    rowPitch,
    interactionEnabled,
    multiSelected,
    multiMode,
    send,
    bindThumbnail,
    releaseThumbnail,
    target,
  } = options;

  const position = (
    cell: HTMLButtonElement,
    index: number,
    count: number,
    stride: number,
  ): void => {
    cell.style.left = `${insetX() + (index % count) * stride}px`;
    cell.style.top = `${insetY() + Math.floor(index / count) * rowPitch()}px`;
    cell.style.width = compact() ? `${stride - gapX()}px` : "";
  };

  const applyMulti = (cell: HTMLButtonElement, index: number): void => {
    const selected = multiSelected(index);
    cell.classList.toggle("multi-selected", selected);
    cell.dataset.multiSelected = String(selected);
    if (selected) cell.setAttribute("aria-pressed", "true");
    else if (multiMode()) cell.setAttribute("aria-pressed", "false");
    else cell.removeAttribute("aria-pressed");
  };

  const release = (rendered: GridCell): void => {
    const image = rendered.cell.querySelector<HTMLImageElement>("img");
    if (image) {
      image.onload = null;
      image.onerror = null;
      image.removeAttribute("src");
    }
    if (rendered.thumbnail) {
      releaseThumbnail(rendered.thumbnail);
      rendered.thumbnail = undefined;
    }
  };

  const build = (
    index: number,
    photo: GridPhotoViewModel | undefined,
    total: number,
    count: number,
    stride: number,
  ): GridCell => {
    const cell = document.createElement("button");
    cell.type = "button";
    cell.className = "photo-cell";
    position(cell, index, count, stride);

    if (!photo) {
      cell.disabled = true;
      const placeholder = document.createElement("span");
      placeholder.className = "cell-placeholder";
      placeholder.textContent = "Loading…";
      cell.append(placeholder);
      const rendered: GridCell = {
        cell,
        signature: LOADING_CELL_SIGNATURE,
        deliveryFailed: false,
        thumbnailState: undefined,
        thumbnail: undefined,
      };
      return rendered;
    }

    cell.dataset.photoIndex = String(index);
    cell.disabled = !interactionEnabled();

    const media = document.createElement("span");
    media.className = "cell-media";
    const image = document.createElement("img");
    image.alt = `Photo ${index + 1} of ${total}`;
    // The virtualized window bounds requests; hidden lazy images never load.
    image.loading = "eager";
    image.fetchPriority = "low";
    image.decoding = "async";
    image.draggable = false;
    image.className = "thumbnail";
    image.hidden = true;
    const placeholder = document.createElement("span");
    placeholder.className = "thumbnail-placeholder";
    placeholder.setAttribute("aria-hidden", "true");
    media.append(image, placeholder);

    const footer = document.createElement("span");
    footer.className = "cell-footer";
    const caption = document.createElement("span");
    caption.className = "cell-caption";
    const line = document.createElement("span");
    line.className = "cell-line";
    const check = document.createElement("span");
    check.className = "cell-check";
    check.setAttribute("aria-hidden", "true");
    check.textContent = "✓";
    const name = document.createElement("span");
    name.className = "cell-name";
    name.textContent = photo.originalFilename ?? "Photo";
    name.title = photo.originalFilename ?? "";
    const captionPosition = document.createElement("span");
    captionPosition.className = "cell-pos";
    captionPosition.textContent = String(index + 1);
    line.append(check, name, captionPosition);

    const meta = document.createElement("span");
    meta.className = `cell-meta state-${photo.selectionState}`;
    const stateMark = document.createElement("span");
    stateMark.className =
      photo.selectionState === "unflagged"
        ? "state-dot state-unflagged"
        : `cell-state ${photo.selectionState}`;
    if (photo.selectionState === "picked") stateMark.textContent = "✓";
    else if (photo.selectionState === "rejected") stateMark.textContent = "×";
    stateMark.setAttribute("aria-hidden", "true");
    const stateText = document.createElement("span");
    stateText.className = "state-text";
    stateText.textContent = selectionLabel(photo.selectionState);
    const stars = document.createElement("span");
    stars.className = "stars";
    stars.setAttribute("aria-hidden", "true");
    for (let star = 1; star <= 5; star += 1) {
      const glyph = document.createElement("span");
      // Shape carries the rated/unrated split; color only reinforces it.
      glyph.textContent = star > photo.rating ? "☆" : "★";
      if (star > photo.rating) glyph.className = "star-empty";
      stars.append(glyph);
    }
    const kind = document.createElement("span");
    kind.className = "cell-kind";
    kind.textContent = `${photo.original.kind === "raw" ? "RAW" : "JPEG"}${
      photo.hasSavedEdits ? " · Edited" : ""
    }`;
    meta.append(stateMark, stateText, stars, kind);
    caption.append(line, meta);
    const rendered: GridCell = {
      cell,
      signature: "",
      deliveryFailed: false,
      thumbnailState: undefined,
      thumbnail: undefined,
    };
    const presentFacts = (): void => {
      const values = gridPhotoFacts(
        photo,
        rendered.deliveryFailed,
        rendered.thumbnailState,
      );
      const issues = gridPhotoIssues(
        photo,
        rendered.deliveryFailed,
        rendered.thumbnailState,
      );
      // Exceptional facts replace the kind slot instead of adding a third
      // caption line. The media placeholder names the same condition, so no
      // text escapes the uniform cell.
      kind.textContent =
        issues.length === 0
          ? `${photo.original.kind === "raw" ? "RAW" : "JPEG"}${
              photo.hasSavedEdits ? " · Edited" : ""
            }`
          : issues.join(" · ");
      kind.classList.toggle("cell-issue", issues.length > 0);
      cell.setAttribute(
        "aria-label",
        [
          `Photo ${index + 1} of ${total}`,
          ...(photo.originalFilename ? [photo.originalFilename] : []),
          selectionLabel(photo.selectionState),
          photo.rating === 1 ? "1 star" : `${photo.rating} stars`,
          ...values,
        ].join(" — "),
      );
    };
    const presentThumbnail = (): void => {
      placeholder.hidden = !image.hidden;
      placeholder.textContent = !photo.original.available
        ? "Original unavailable"
        : rendered.deliveryFailed
          ? "Thumbnail delivery failed"
          : rendered.thumbnailState === "failed"
            ? "Thumbnail failed"
            : rendered.thumbnailState === "unavailable"
              ? "Thumbnail unavailable"
              : photo.preview.state === "failed"
                ? "Preview failed"
                : photo.preview.state === "unavailable"
                  ? "Preview unavailable"
                  : photo.preview.state === "inspection-pending"
                    ? "Preparing Preview…"
                    : "Loading thumbnail…";
    };
    presentThumbnail();
    presentFacts();
    rendered.signature = gridCellSignature(index, photo, false);

    footer.append(caption);
    cell.append(media, footer);
    cell.addEventListener("click", (event) => send(index, event));
    applyMulti(cell, index);

    rendered.thumbnail = {
      photoId: photo.id,
      preview: photo.preview,
      target: target(
        image,
        (failed) => {
          rendered.deliveryFailed = failed;
          if (!failed) rendered.thumbnailState = undefined;
          rendered.signature = gridCellSignature(index, photo, failed);
          presentFacts();
          presentThumbnail();
        },
        (state) => {
          rendered.thumbnailState = state;
          rendered.deliveryFailed = false;
          presentFacts();
          presentThumbnail();
        },
      ),
    };
    bindThumbnail(rendered.thumbnail);
    return rendered;
  };

  return {
    release,
    build,
    position,
    applyMulti,
    signature: gridCellSignature,
  };
}
