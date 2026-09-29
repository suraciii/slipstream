export type FilmstripPhotoViewModel = Readonly<{
  id: string;
  available: boolean;
  original: Readonly<{ kind: "raw" | "jpeg"; available: boolean }>;
  originalFilename?: string;
  selectionState: "undecided" | "selected" | "rejected";
  rating: number;
  hasSavedEdits: boolean;
  preview: Readonly<{
    state: "inspection-pending" | "ready" | "unavailable" | "failed";
    thumbnailUrl?: string;
  }>;
}>;

export type FilmstripCellViewModel = Readonly<{
  index: number;
  current: boolean;
  photo: FilmstripPhotoViewModel | undefined;
}>;

export type FilmstripViewModel = Readonly<{
  total: number;
  interactive: boolean;
  cells: ReadonlyArray<FilmstripCellViewModel>;
}>;

type FilmstripThumbnailTarget = {
  readonly complete: boolean;
  readonly isConnected: boolean;
  src: string;
  onload: GlobalEventHandlers["onload"];
  onerror: GlobalEventHandlers["onerror"];
  removeAttribute(name: string): void;
  setDeliveryFailed(failed: boolean): void;
};

type FilmstripThumbnailBinding = Readonly<{
  photoId: string;
  preview: FilmstripPhotoViewModel["preview"];
  target: FilmstripThumbnailTarget;
}>;

type RenderedFilmstripCell = {
  readonly button: HTMLButtonElement;
  signature: string;
  deliveryFailed: boolean;
  thumbnail: FilmstripThumbnailBinding | undefined;
  presentsPhoto: boolean;
};

type FilmstripPresenterOptions = Readonly<{
  elements: Readonly<{
    photoView: HTMLElement;
    filmstrip: HTMLElement;
    filmstripHost: HTMLElement;
    filmstripTools: HTMLElement;
  }>;
  layout: Readonly<{
    isDisclosed: () => boolean;
    isNearbyOpen: () => boolean;
    requestResize: () => void;
  }>;
  thumbnails: Readonly<{
    target: (
      image: HTMLImageElement,
      onDeliveryFailure: (failed: boolean) => void,
    ) => FilmstripThumbnailTarget;
    bind: (binding: FilmstripThumbnailBinding) => void;
    release: (binding: FilmstripThumbnailBinding) => void;
  }>;
  openPhoto: (index: number) => void;
}>;

export type FilmstripPresenter = Readonly<{
  render(model: FilmstripViewModel): void;
  syncHost(): void;
  setInteractive(enabled: boolean): void;
  clear(): void;
  dispose(): void;
}>;

export function createFilmstripPresenter(
  options: FilmstripPresenterOptions,
): FilmstripPresenter {
  const { elements, layout, thumbnails, openPhoto } = options;
  const { photoView, filmstrip, filmstripHost, filmstripTools } = elements;
  const rendered = new Map<number, RenderedFilmstripCell>();
  let alive = true;
  let model: FilmstripViewModel | undefined;
  let interactive = false;
  let heldIndex: number | null = null;
  let rehoming = false;

  const stripSurface = (): HTMLElement | undefined =>
    filmstrip.closest<HTMLElement>("dialog") ?? undefined;

  const releaseCell = (cell: RenderedFilmstripCell) => {
    const image = cell.button.querySelector<HTMLImageElement>("img");
    if (image) {
      image.onload = null;
      image.onerror = null;
      image.removeAttribute("src");
    }
    if (cell.thumbnail) {
      thumbnails.release(cell.thumbnail);
      cell.thumbnail = undefined;
    }
  };

  const clear = () => {
    for (const cell of rendered.values()) releaseCell(cell);
    rendered.clear();
    filmstrip.replaceChildren();
    filmstrip.hidden = true;
  };

  const restoreHeldFocus = () => {
    if (rehoming || heldIndex === null || !interactive) return;
    const surface = stripSurface();
    const active = document.activeElement;
    const parked =
      active === document.body ||
      active === photoView ||
      (surface !== undefined && surface.contains(active));
    if (!parked) return;
    const entry = filmstrip.querySelector<HTMLButtonElement>(
      `[data-filmstrip-index="${heldIndex}"]`,
    );
    if (!entry || entry.disabled || entry.offsetParent === null) return;
    heldIndex = null;
    entry.focus();
  };

  const applyInteractivity = () => {
    for (const cell of rendered.values())
      if (cell.presentsPhoto) cell.button.disabled = !interactive;
  };

  const photoSignature = (
    index: number,
    photo: FilmstripPhotoViewModel,
    deliveryFailed: boolean,
  ): string =>
    [
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

  const cellSignature = (
    cell: FilmstripCellViewModel,
    total: number,
    deliveryFailed: boolean,
  ): string =>
    [
      String(cell.index),
      String(total),
      cell.current ? "current" : "neighbor",
      cell.photo
        ? photoSignature(cell.index, cell.photo, deliveryFailed)
        : "loading",
    ].join("|");

  const buildCell = (
    cell: FilmstripCellViewModel,
    total: number,
  ): RenderedFilmstripCell => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "filmstrip-cell";
    button.dataset.filmstripIndex = String(cell.index);
    const result: RenderedFilmstripCell = {
      button,
      signature: "",
      deliveryFailed: false,
      thumbnail: undefined,
      presentsPhoto: false,
    };
    const photo = cell.photo;
    if (!photo) {
      button.disabled = true;
      button.textContent = "…";
      button.setAttribute("aria-label", `Photo ${cell.index + 1} of ${total}`);
      result.signature = cellSignature(cell, total, false);
      return result;
    }
    const image = document.createElement("img");
    image.alt = "";
    image.loading = "lazy";
    image.fetchPriority = "low";
    image.decoding = "async";
    image.draggable = false;
    image.className = "thumbnail";
    button.append(image);
    if (photo.selectionState !== "undecided") {
      const badge = document.createElement("span");
      badge.className = `cell-state ${photo.selectionState}`;
      badge.textContent = photo.selectionState === "selected" ? "✓" : "×";
      button.append(badge);
    }
    if (cell.current) button.setAttribute("aria-current", "true");
    button.setAttribute(
      "aria-label",
      [
        `Photo ${cell.index + 1} of ${total}`,
        ...(photo.originalFilename ? [photo.originalFilename] : []),
      ].join(" — "),
    );
    button.title = [
      selectionLabel(photo.selectionState),
      photo.rating === 1 ? "1 star" : `${photo.rating} stars`,
    ].join(" · ");
    if (!cell.current)
      button.addEventListener("click", () => openPhoto(cell.index));
    result.presentsPhoto = true;
    button.disabled = !interactive;
    if (alive) {
      const binding: FilmstripThumbnailBinding = {
        photoId: photo.id,
        preview: photo.preview,
        target: thumbnails.target(image, (failed) => {
          result.deliveryFailed = failed;
          result.signature = cellSignature(cell, total, failed);
        }),
      };
      result.thumbnail = binding;
      thumbnails.bind(binding);
    }
    result.signature = cellSignature(cell, total, false);
    return result;
  };

  const render = (next: FilmstripViewModel) => {
    if (!alive || photoView.hidden) return;
    model = next;
    const presented =
      (!layout.isDisclosed() || layout.isNearbyOpen()) && next.cells.length > 1;
    if (!presented) {
      clear();
      return;
    }
    filmstrip.hidden = false;
    interactive = next.interactive;
    const hadFocus = filmstrip.contains(document.activeElement);
    const wanted = new Set(next.cells.map((cell) => cell.index));
    for (const [index, cell] of [...rendered]) {
      if (wanted.has(index)) continue;
      releaseCell(cell);
      cell.button.remove();
      rendered.delete(index);
    }
    for (const cell of next.cells) {
      const signature = cellSignature(cell, next.total, false);
      let current = rendered.get(cell.index);
      if (current && current.signature !== signature) {
        releaseCell(current);
        current.button.remove();
        rendered.delete(cell.index);
        current = undefined;
      }
      if (!current) {
        current = buildCell(cell, next.total);
        rendered.set(cell.index, current);
      } else if (
        current.thumbnail &&
        !current.deliveryFailed &&
        !current.button.querySelector("img")?.getAttribute("src")
      ) {
        thumbnails.bind(current.thumbnail);
      }
      filmstrip.append(current.button);
    }
    applyInteractivity();
    filmstrip.hidden = false;
    restoreHeldFocus();
    if (!hadFocus) return;
    const current = next.cells.find((cell) => cell.current);
    if (!current) return;
    const entry = rendered.get(current.index)?.button;
    if (
      entry &&
      entry.offsetParent !== null &&
      document.activeElement !== entry
    )
      entry.focus();
  };

  const syncHost = () => {
    const presented = !layout.isDisclosed() || layout.isNearbyOpen();
    const host = layout.isNearbyOpen() ? filmstripTools : filmstripHost;
    if (filmstrip.parentElement !== host) host.append(filmstrip);
    if (!presented) {
      clear();
      return;
    }
    if (model && model.cells.length > 1) {
      const rememberedInteractive = interactive;
      rehoming = true;
      render(model);
      rehoming = false;
      interactive = rememberedInteractive;
      applyInteractivity();
      restoreHeldFocus();
      return;
    }
    layout.requestResize();
  };

  const setInteractive = (enabled: boolean) => {
    const wasInteractive = interactive;
    interactive = enabled;
    const focused = document.activeElement as HTMLElement | null;
    const holdsFocus =
      wasInteractive &&
      !enabled &&
      focused?.dataset.filmstripIndex !== undefined;
    applyInteractivity();
    if (holdsFocus) {
      heldIndex = Number(focused.dataset.filmstripIndex);
      const surface = stripSurface();
      if (surface) surface.focus();
      else photoView.focus();
    } else if (!wasInteractive && enabled) restoreHeldFocus();
  };

  return {
    render,
    syncHost,
    setInteractive,
    clear,
    dispose() {
      if (!alive) return;
      alive = false;
      clear();
    },
  };
}

function selectionLabel(
  state: FilmstripPhotoViewModel["selectionState"],
): string {
  if (state === "selected") return "Selected";
  if (state === "rejected") return "Rejected";
  return "Undecided";
}
