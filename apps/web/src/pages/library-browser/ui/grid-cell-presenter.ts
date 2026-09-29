export type GridSelectionState = "undecided" | "selected" | "rejected";

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
  thumbnail: GridThumbnailBinding | undefined;
};

type GridCellPresenterOptions = Readonly<{
  compact: () => boolean;
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
  ) => GridThumbnailTarget;
}>;

const GRID_CELL_GAP_X = 10;
const LOADING_CELL_SIGNATURE = "loading";

function selectionLabel(value: GridSelectionState): string {
  return value === "selected"
    ? "Selected"
    : value === "rejected"
      ? "Rejected"
      : "Undecided";
}

function gridPhotoFacts(
  photo: GridPhotoViewModel,
  deliveryFailed: boolean,
): string[] {
  const facts: string[] = [];
  if (!photo.available) facts.push("Photo unavailable");
  if (photo.original.kind === "raw") facts.push("RAW");
  if (photo.hasSavedEdits) facts.push("Edited");
  if (photo.preview.state === "unavailable") facts.push("Preview unavailable");
  if (photo.preview.state === "failed") facts.push("Preview failed");
  if (deliveryFailed) facts.push("Thumbnail delivery failed");
  return facts;
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
    cell.style.left = `${(index % count) * stride}px`;
    cell.style.top = `${Math.floor(index / count) * rowPitch()}px`;
    cell.style.width = compact() ? `${stride - GRID_CELL_GAP_X}px` : "";
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
    image.loading = "lazy";
    image.fetchPriority = "low";
    image.decoding = "async";
    image.draggable = false;
    image.className = "thumbnail";
    media.append(image);

    const footer = document.createElement("span");
    footer.className = "cell-footer";
    const caption = document.createElement("span");
    caption.className = "cell-caption";
    const identity = photo.originalFilename
      ? `${index + 1} · ${photo.originalFilename}`
      : String(index + 1);
    caption.textContent = photo.rating
      ? `${identity} · ${photo.rating}★`
      : identity;
    if (photo.originalFilename) caption.title = photo.originalFilename;

    const facts = document.createElement("span");
    facts.className = "cell-facts";
    const rendered: GridCell = {
      cell,
      signature: "",
      deliveryFailed: false,
      thumbnail: undefined,
    };
    const presentFacts = (): void => {
      const values = gridPhotoFacts(photo, rendered.deliveryFailed);
      facts.textContent = values.join(" · ");
      facts.hidden = values.length === 0;
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
    presentFacts();
    rendered.signature = gridCellSignature(index, photo, false);

    if (photo.selectionState === "undecided") {
      caption.classList.add("cell-caption-wide");
      footer.append(caption, facts);
    } else {
      const badge = document.createElement("span");
      badge.className = `cell-state ${photo.selectionState}`;
      badge.textContent = photo.selectionState === "selected" ? "✓" : "×";
      footer.append(badge, caption, facts);
    }
    cell.append(media, footer);
    cell.addEventListener("click", (event) => send(index, event));
    applyMulti(cell, index);

    rendered.thumbnail = {
      photoId: photo.id,
      preview: photo.preview,
      target: target(image, (failed) => {
        rendered.deliveryFailed = failed;
        rendered.signature = gridCellSignature(index, photo, failed);
        presentFacts();
      }),
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
