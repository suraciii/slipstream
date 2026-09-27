/// The Photo View's bounded neighbor strip (Nearby Photos): the entries the
/// page model bounds around the current Photo.
///
/// The controller owns presentation only. It rebuilds only the entries whose
/// facts changed, keeps every other entry's thumbnail transfer, moves the one
/// strip node between its two homes, and parks a held entry's focus exactly as
/// the Grid does for its held cell. It owns no neighbor state and no image
/// delivery: Thumbnails travel through the view's one bind/release path, and
/// the page model owns which neighbors exist and whether an activation would
/// be admitted.

import type {
  FilmstripCellViewModel,
  FilmstripViewModel,
  GridThumbnailBinding,
  GridThumbnailTarget,
  LibraryBrowserIntent,
} from "./library-browser-view.js";

/// The intents the strip emits, as the page model names them: opening one
/// neighbor and the re-render a disclosure asks of the page model.
type FilmstripIntentKind = "open-photo" | "filmstrip-resize";

export type FilmstripIntent = Extract<
  LibraryBrowserIntent,
  { kind: FilmstripIntentKind }
>;

export type FilmstripElements = Readonly<{
  filmstrip: HTMLElement;
  filmstripHost: HTMLElement;
  filmstripTools: HTMLElement;
  photoView: HTMLElement;
}>;

/// One rendered filmstrip entry. The signature covers the facts and the
/// current marker, so a strip render rebuilds only the entries that changed
/// and every other entry keeps its thumbnail transfer. `presentsPhoto`
/// separates a real entry from a placeholder, whose disabled state is
/// permanent instead of following navigation interactivity.
type RenderedFilmstripCell = {
  readonly button: HTMLButtonElement;
  signature: string;
  deliveryFailed: boolean;
  thumbnail: GridThumbnailBinding | undefined;
  presentsPhoto: boolean;
};

export interface FilmstripStrip {
  /// Presents the bounded neighbors of the current Photo. A strip that is
  /// not presented — a source without neighbors, or a closed Nearby Photos
  /// disclosure — binds no thumbnails either: presenting entries for a hidden
  /// surface is the work the strip promises never to start.
  render(model: FilmstripViewModel): void;
  /// Releases every entry, so a strip that is not presented holds no image
  /// demand at all.
  clear(): void;
  /// Places the one strip node where it is presented, and releases it
  /// everywhere else: a strip that is not presented binds no thumbnail and
  /// admits no window of its own.
  syncHost(): void;
  /// Applies the page model's interactivity fact. A held entry's focus is
  /// parked while an activation would be refused and returned once one is
  /// admitted again.
  setInteractive(enabled: boolean): void;
  dispose(): void;
}

export function createFilmstripStrip({
  elements,
  send,
  presented,
  inTools,
  selectionLabel,
  bindThumbnail,
  releaseThumbnail,
  gridThumbnailTarget,
  photoSignature,
}: Readonly<{
  elements: FilmstripElements;
  send: (intent: FilmstripIntent) => void;
  /// True while a layout presents the strip: the wide layout beside the
  /// Preview, or the Nearby Photos disclosure inside Photo tools.
  presented: () => boolean;
  /// True while the strip is disclosed inside Photo tools rather than shown
  /// beside the Preview.
  inTools: () => boolean;
  selectionLabel: (value?: "undecided" | "selected" | "rejected") => string;
  bindThumbnail: (binding: GridThumbnailBinding) => void;
  releaseThumbnail: (binding: GridThumbnailBinding) => void;
  gridThumbnailTarget: (
    image: HTMLImageElement,
    setDeliveryFailed: (failed: boolean) => void,
  ) => GridThumbnailTarget;
  photoSignature: (
    index: number,
    photo: NonNullable<FilmstripCellViewModel["photo"]>,
    deliveryFailed: boolean,
  ) => string;
}>): FilmstripStrip {
  const { filmstrip, filmstripHost, filmstripTools, photoView } = elements;
  let alive = true;
  const renderedFilmstripCells = new Map<number, RenderedFilmstripCell>();
  /// The last neighbor facts the page model reported. A disclosure rebuilds
  /// the strip from them, so a closed strip holds no image demand at all.
  let filmstripModel: FilmstripViewModel | undefined;
  /// Whether activating a strip entry would open its Photo. A real entry
  /// mirrors the Grid cell rule: the strip never presents an enabled control
  /// whose activation would be refused silently. Placeholders stay disabled
  /// for their own reason, so only entries that present a Photo follow this.
  let filmstripInteractive = false;
  /// The strip entry a keyboard Photographer had focused when the strip
  /// became non-interactive. Disabling a focused button drops focus to the
  /// body, so the strip parks focus on the Photo View and returns it when
  /// interactivity resumes, exactly as the Grid does for its held cell. The
  /// held entry is remembered by index because a settled decision rebuilds
  /// the strip and replaces the old button element.
  let heldStripIndex: number | null = null;
  /// Whether the strip is being re-homed from the remembered model, whose
  /// interactivity fact can predate the busy gate that just parked the focus.
  /// The live fact is put back immediately after such a rebuild, so a held
  /// entry is never restored against the stale one the rebuild read.
  let rehomingRememberedStrip = false;
  /// The native surface that currently holds the strip, when a compact layout
  /// discloses it inside Photo tools. A modal makes the rest of the document
  /// inert, so a focus move that would park on the Photo View lands on the
  /// surface itself instead.
  const stripSurface = (): HTMLElement | undefined =>
    filmstrip.closest<HTMLElement>("dialog") ?? undefined;
  const restoreHeldStripFocus = () => {
    if (rehomingRememberedStrip) return;
    if (heldStripIndex === null || !filmstripInteractive) return;
    const surface = stripSurface();
    // Only a focus this view parked is one it may return: any other owner
    // keeps the keyboard, so the held entry stays held. A native modal makes
    // the Photo View inert, so the surface that holds the strip — and anything
    // focused inside it — is a parked owner too.
    const parked =
      document.activeElement === document.body ||
      document.activeElement === photoView ||
      (surface !== undefined && surface.contains(document.activeElement));
    if (!parked) return;
    // The held index survives until a presented, enabled entry actually takes
    // the focus. A rebuild that replaces the entry lands after this update, so
    // consuming the index here would leave nothing for that rebuild's retry.
    const entry = filmstrip.querySelector<HTMLButtonElement>(
      `[data-filmstrip-index="${heldStripIndex}"]`,
    );
    if (!entry || entry.disabled || entry.offsetParent === null) return;
    heldStripIndex = null;
    entry.focus();
  };
  const releaseFilmstripCell = (rendered: RenderedFilmstripCell) => {
    const image = rendered.button.querySelector<HTMLImageElement>("img");
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
  const clear = () => {
    for (const rendered of renderedFilmstripCells.values())
      releaseFilmstripCell(rendered);
    renderedFilmstripCells.clear();
    filmstrip.replaceChildren();
    filmstrip.hidden = true;
  };
  const applyFilmstripInteractivity = () => {
    for (const rendered of renderedFilmstripCells.values())
      if (rendered.presentsPhoto)
        rendered.button.disabled = !filmstripInteractive;
  };
  /// One filmstrip entry. A neighbor whose facts are still loading renders as
  /// a disabled placeholder, exactly like a Grid cell outside the loaded
  /// window, so the bounded strip never invents a Photo.
  const buildFilmstripCell = (
    cell: FilmstripCellViewModel,
    total: number,
  ): RenderedFilmstripCell => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "filmstrip-cell";
    button.dataset.filmstripIndex = String(cell.index);
    const rendered: RenderedFilmstripCell = {
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
      rendered.signature = filmstripCellSignature(
        cell,
        total,
        false,
        photoSignature,
      );
      return rendered;
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
    // The entry is named by its position and identity, and its decision and
    // Rating travel in the description instead. A name carrying "Selected" or
    // "Undecided" would make the entry indistinguishable from the Select,
    // Reject, and Undo controls for anything that addresses controls by name.
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
    // The current entry is where the Photographer already is: it is marked as
    // current and presents no activation, so nothing re-opens the same Photo
    // and throws away manual zoom.
    if (!cell.current)
      button.addEventListener("click", () =>
        send({ kind: "open-photo", index: cell.index }),
      );
    rendered.presentsPhoto = true;
    button.disabled = !filmstripInteractive;
    if (alive) {
      const binding: GridThumbnailBinding = {
        photoId: photo.id,
        preview: photo.preview,
        target: gridThumbnailTarget(image, (failed) => {
          rendered.deliveryFailed = failed;
          rendered.signature = filmstripCellSignature(
            cell,
            total,
            failed,
            photoSignature,
          );
        }),
      };
      rendered.thumbnail = binding;
      bindThumbnail(binding);
    }
    rendered.signature = filmstripCellSignature(
      cell,
      total,
      false,
      photoSignature,
    );
    return rendered;
  };
  const render = (model: FilmstripViewModel) => {
    if (!alive || photoView.hidden) return;
    filmstripModel = model;
    // A strip that is not presented — a source without neighbors, or a closed
    // Nearby Photos disclosure — binds no thumbnails either: presenting
    // entries for a hidden surface is the work the strip promises never to
    // start. A compact layout discloses the strip inside Photo tools; a wide
    // layout shows it beside the Preview.
    const presentedNow = presented() && model.cells.length > 1;
    if (!presentedNow || model.cells.length <= 1) {
      clear();
      return;
    }
    filmstrip.hidden = false;
    filmstripInteractive = model.interactive;
    // The Photographer keeps their place when the strip rebuilds around a
    // new current Photo: focus follows the current entry like the Grid's
    // keyboard follows its cell.
    const hadFocus = filmstrip.contains(document.activeElement);
    const wanted = new Set(model.cells.map((cell) => cell.index));
    for (const [index, rendered] of [...renderedFilmstripCells]) {
      if (wanted.has(index)) continue;
      releaseFilmstripCell(rendered);
      rendered.button.remove();
      renderedFilmstripCells.delete(index);
    }
    for (const cell of model.cells) {
      const signature = filmstripCellSignature(
        cell,
        model.total,
        false,
        photoSignature,
      );
      let rendered = renderedFilmstripCells.get(cell.index);
      if (rendered && rendered.signature !== signature) {
        releaseFilmstripCell(rendered);
        rendered.button.remove();
        rendered = undefined;
        renderedFilmstripCells.delete(cell.index);
      }
      if (!rendered) {
        rendered = buildFilmstripCell(cell, model.total);
        renderedFilmstripCells.set(cell.index, rendered);
      } else if (
        rendered.thumbnail &&
        !rendered.deliveryFailed &&
        !rendered.button.querySelector("img")?.getAttribute("src")
      ) {
        // A navigation hands the entry's in-flight thumbnail transfer back to
        // the owner and drops its source, while the retained entry keeps its
        // signature so the build path never runs again for it. Bind the
        // thumbnail it still holds again instead of leaving the entry blank
        // for the rest of the visit. A binding whose delivery already failed
        // is left alone, so a failure cannot become a request on every
        // render.
        bindThumbnail(rendered.thumbnail);
      }
      // Appending an entry the strip already holds keeps its image element,
      // so a moved entry never restarts its thumbnail transfer.
      filmstrip.append(rendered.button);
    }
    applyFilmstripInteractivity();
    filmstrip.hidden = false;
    // A settled decision rebuilds the strip after interactivity resumes, so
    // the held entry's restoration is retried once the rebuilt entry exists.
    restoreHeldStripFocus();
    if (!hadFocus) return;
    const current = model.cells.find((cell) => cell.current);
    if (!current) return;
    const entry = renderedFilmstripCells.get(current.index)?.button;
    if (
      entry &&
      entry.offsetParent !== null &&
      document.activeElement !== entry
    )
      entry.focus();
  };
  const syncHost = () => {
    const shown = presented();
    const host = inTools() ? filmstripTools : filmstripHost;
    if (filmstrip.parentElement !== host) host.append(filmstrip);
    if (!shown) {
      clear();
      return;
    }
    // The page model owns the strip's facts. A presented strip is rebuilt from
    // the remembered facts — while the live readiness fact still decides
    // whether an activation would be admitted — and a remembered model that
    // holds no neighbors asks that owner to render again instead of claiming
    // the source has none.
    if (filmstripModel && filmstripModel.cells.length > 1) {
      const interactive = filmstripInteractive;
      rehomingRememberedStrip = true;
      render(filmstripModel);
      rehomingRememberedStrip = false;
      filmstripInteractive = interactive;
      applyFilmstripInteractivity();
      // The rebuild read the remembered interactivity, so the held entry is
      // restored only now, against the live fact this re-homing put back.
      restoreHeldStripFocus();
      return;
    }
    send({ kind: "filmstrip-resize" });
  };
  const setInteractive = (enabled: boolean) => {
    const wasStripInteractive = filmstripInteractive;
    filmstripInteractive = enabled;
    const heldElement = document.activeElement as HTMLElement | null;
    const holdsStripFocus =
      wasStripInteractive &&
      !enabled &&
      heldElement?.dataset.filmstripIndex !== undefined;
    applyFilmstripInteractivity();
    if (holdsStripFocus) {
      heldStripIndex = Number(heldElement.dataset.filmstripIndex);
      // A native modal makes the rest of the document inert, so the Photo
      // View cannot take a parked focus while the strip is disclosed inside
      // Photo tools: the surface that holds the strip does, and the restore
      // below recognizes it as the parked owner.
      const surface = stripSurface();
      if (surface) surface.focus();
      else photoView.focus();
    } else if (!wasStripInteractive && enabled) {
      restoreHeldStripFocus();
    }
  };
  return {
    render,
    clear,
    syncHost,
    setInteractive,
    dispose() {
      alive = false;
      clear();
    },
  };
}

function filmstripCellSignature(
  cell: FilmstripCellViewModel,
  total: number,
  deliveryFailed: boolean,
  photoSignature: (
    index: number,
    photo: NonNullable<FilmstripCellViewModel["photo"]>,
    deliveryFailed: boolean,
  ) => string,
): string {
  const photo = cell.photo;
  return [
    String(cell.index),
    String(total),
    cell.current ? "current" : "neighbor",
    photo ? photoSignature(cell.index, photo, deliveryFailed) : "loading",
  ].join("|");
}
