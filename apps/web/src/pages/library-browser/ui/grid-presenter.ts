import {
  createGridCellPresenter,
  type GridCell,
  type GridPhotoViewModel,
  type GridThumbnailBinding,
} from "./grid-cell-presenter.js";
import { gridThumbnailTarget } from "./grid-thumbnail-target.js";
import type { NavigationGridRestoration } from "../model/browser-navigation.js";

type GridThumbnailSize = "small" | "medium" | "large";
const GRID_CELL_GAP_X = 10;
const GRID_CELL_GAP_Y = 12;
const GRID_THUMBNAIL_SIZE_STEPS: Readonly<
  Record<
    GridThumbnailSize,
    Readonly<{ width: number; height: number; label: string }>
  >
> = {
  small: { width: 108, height: 130, label: "Small" },
  medium: { width: 140, height: 166, label: "Medium" },
  large: { width: 216, height: 256, label: "Large" },
};

type GridPresentation = Readonly<{
  total: number;
  photoAt(index: number): GridPhotoViewModel | undefined;
}>;

type GridIntent =
  | Readonly<{ kind: "grid-render" | "grid-resize" | "grid-multi-clear" }>
  | Readonly<{ kind: "grid-range"; start: number; end: number }>
  | Readonly<{
      kind: "open-photo";
      index: number;
      range?: boolean;
      toggle?: boolean;
    }>
  | Readonly<{
      kind: "grid-photo-mutation";
      index: number;
      field: "rating" | "selectionState";
      value: number | "selected" | "rejected" | "undecided";
    }>;

export function createGridPresenter(
  options: Readonly<{
    browser: HTMLElement;
    gridView: HTMLElement;
    viewport: HTMLElement;
    canvas: HTMLElement;
    layer: HTMLElement;
    sizeSelect: HTMLSelectElement;
    gridTools: HTMLElement;
    gridSelection: HTMLElement;
    gridBatch: HTMLElement;
    compact: MediaQueryList;
    isAlive(): boolean;
    multiMode(): boolean;
    multiCount(): number;
    multiSelected(index: number): boolean;
    renderBatch(): void;
    send(intent: GridIntent): void;
    bindThumbnail(binding: GridThumbnailBinding): void;
    releaseThumbnail(binding: GridThumbnailBinding): void;
  }>,
) {
  const {
    browser,
    gridView,
    viewport,
    canvas,
    layer,
    sizeSelect,
    gridTools,
    gridSelection,
    gridBatch,
    compact,
    isAlive,
    multiMode,
    multiCount,
    multiSelected,
    renderBatch,
    send,
  } = options;
  let total = 0;
  let keyboardIndex: number | undefined;
  let pendingCellFocus: number | undefined;
  let thumbnailSize: GridThumbnailSize = "medium";
  let pendingAnchor: number | undefined;
  let renderedColumns = 0;
  let renderedStride = 0;
  let renderedHeight = 0;
  let renderFrame: number | undefined;
  const renderedCells = new Map<number, GridCell>();
  let reportedRange: Readonly<{ start: number; end: number }> | undefined;
  let photoAt: GridPresentation["photoAt"] = () => undefined;

  const cellBox = () => GRID_THUMBNAIL_SIZE_STEPS[thumbnailSize];
  const columnPitch = () => cellBox().width + GRID_CELL_GAP_X;
  const rowPitch = () => cellBox().height + GRID_CELL_GAP_Y;
  const columns = () =>
    Math.max(
      1,
      Math.floor(Math.max(320, viewport.clientWidth) / columnPitch()),
    );
  const columnStride = (count = columns()) =>
    compact.matches ? viewport.clientWidth / count : columnPitch();
  const viewportHeight = () =>
    Math.max(360, Math.min(viewport.clientHeight, window.innerHeight));
  const firstVisibleIndex = (count: number): number =>
    total === 0
      ? 0
      : Math.max(
          0,
          Math.min(
            total - 1,
            Math.floor(viewport.scrollTop / rowPitch()) * count,
          ),
        );
  const applySize = () => {
    const box = cellBox();
    browser.style.setProperty("--grid-cell-width", `${box.width}px`);
    browser.style.setProperty("--grid-cell-height", `${box.height}px`);
  };
  for (const [size, step] of Object.entries(GRID_THUMBNAIL_SIZE_STEPS)) {
    const option = document.createElement("option");
    option.value = size;
    option.textContent = step.label;
    sizeSelect.append(option);
  }
  sizeSelect.value = thumbnailSize;
  applySize();

  const schedule = () => {
    if (!isAlive() || renderFrame !== undefined) return;
    renderFrame = requestAnimationFrame(() => {
      renderFrame = undefined;
      send({ kind: "grid-render" });
    });
  };
  const cancel = () => {
    if (renderFrame === undefined) return;
    cancelAnimationFrame(renderFrame);
    renderFrame = undefined;
  };
  const cellPresenter = createGridCellPresenter({
    compact: () => compact.matches,
    rowPitch,
    interactionEnabled: () => interactive,
    multiSelected,
    multiMode,
    send: (index, event) =>
      send({
        kind: "open-photo",
        index,
        ...(event.shiftKey ? { range: true } : {}),
        ...(event.ctrlKey || event.metaKey ? { toggle: true } : {}),
      }),
    bindThumbnail: options.bindThumbnail,
    releaseThumbnail: options.releaseThumbnail,
    target: gridThumbnailTarget,
  });
  let interactive = false;

  const clear = () => {
    for (const rendered of renderedCells.values())
      cellPresenter.release(rendered);
    renderedCells.clear();
    reportedRange = undefined;
    layer.replaceChildren();
  };
  const applyMulti = () => {
    for (const [index, rendered] of renderedCells)
      cellPresenter.applyMulti(rendered.cell, index);
  };
  // Rebind only images detached by a source boundary; retained cells otherwise
  // keep their in-flight thumbnails across merged renders.
  const rebindDetached = (model: GridPresentation) => {
    if (!isAlive() || gridView.hidden) return;
    const count = columns();
    const stride = columnStride(count);
    for (const [index, rendered] of [...renderedCells]) {
      const image = rendered.cell.querySelector<HTMLImageElement>("img");
      if (!rendered.thumbnail || !image || image.getAttribute("src")) continue;
      const position = rendered.cell.nextSibling;
      cellPresenter.release(rendered);
      rendered.cell.remove();
      const rebuilt = cellPresenter.build(
        index,
        model.photoAt(index),
        model.total,
        count,
        stride,
      );
      layer.insertBefore(rebuilt.cell, position);
      renderedCells.set(index, rebuilt);
    }
  };
  // A missing cell leaves focus on the viewport until its window arrives.
  // A control outside the Grid keeps its own focus unless explicitly requested.
  const restoreKeyboardFocus = () => {
    for (const [position, rendered] of renderedCells)
      rendered.cell.tabIndex = position === keyboardIndex ? 0 : -1;
    const active = document.activeElement;
    const requested = pendingCellFocus !== undefined;
    pendingCellFocus = undefined;
    const owns =
      requested ||
      active === null ||
      active === document.body ||
      (viewport.contains(active) &&
        !gridTools.contains(active) &&
        !gridSelection.contains(active) &&
        !gridBatch.contains(active));
    if (!owns || keyboardIndex === undefined) return;
    const cell = layer.querySelector<HTMLButtonElement>(
      `[data-photo-index="${keyboardIndex}"]`,
    );
    if (cell && !cell.disabled) {
      if (active !== cell) cell.focus({ preventScroll: true });
    } else if (active !== viewport) viewport.focus();
  };
  const setInteractive = (enabled: boolean) => {
    const focused = document.activeElement;
    const heldCell = Boolean(
      focused instanceof HTMLElement &&
        layer.contains(focused) &&
        focused.matches("[data-photo-index]"),
    );
    const becameInteractive = !interactive && enabled;
    interactive = enabled;
    for (const cell of Array.from(
      layer.querySelectorAll<HTMLButtonElement>(
        ".photo-cell[data-photo-index]",
      ),
    ))
      cell.disabled = !enabled;
    if (heldCell && !enabled) viewport.focus();
    else if (becameInteractive) restoreKeyboardFocus();
  };
  // Window admission belongs to the page model. This render only reports the
  // presented range and preserves unchanged cell nodes and image transfers.
  const render = (model: GridPresentation, position?: number) => {
    total = model.total;
    photoAt = model.photoAt;
    if (!isAlive() || gridView.hidden) return;
    renderBatch();
    const count = columns();
    const stride = columnStride(count);
    const pitch = rowPitch();
    const height = viewportHeight();
    renderedColumns = count;
    renderedStride = stride;
    renderedHeight = height;
    const canvasHeight = `${Math.ceil(model.total / count) * pitch}px`;
    canvas.style.height = canvasHeight;
    layer.style.height = canvasHeight;
    if (pendingAnchor !== undefined) {
      viewport.scrollTop = Math.floor(pendingAnchor / count) * pitch;
      pendingAnchor = undefined;
    }
    if (position !== undefined)
      viewport.scrollTop = Math.floor(position / count) * pitch;
    const firstRow = Math.max(0, Math.floor(viewport.scrollTop / pitch) - 2);
    const start = firstRow * count;
    const end = Math.min(
      model.total,
      start + (Math.ceil(height / pitch) + 4) * count,
    );
    for (const [index, rendered] of renderedCells) {
      if (index < start || index >= end) {
        rendered.cell.remove();
        cellPresenter.release(rendered);
        renderedCells.delete(index);
      }
    }
    let anchor: ChildNode | null = null;
    let incomplete = false;
    for (let index = end - 1; index >= start; index -= 1) {
      const photo = model.photoAt(index);
      const existing = renderedCells.get(index);
      const signature = cellPresenter.signature(
        index,
        photo,
        existing?.deliveryFailed ?? false,
      );
      if (!photo) incomplete = true;
      let rendered: GridCell;
      if (existing && existing.signature === signature) {
        rendered = existing;
        cellPresenter.position(rendered.cell, index, count, stride);
      } else {
        if (existing) {
          cellPresenter.release(existing);
          existing.cell.remove();
        }
        rendered = cellPresenter.build(
          index,
          photo,
          model.total,
          count,
          stride,
        );
        renderedCells.set(index, rendered);
      }
      if (
        rendered.cell.parentNode !== layer ||
        rendered.cell.nextSibling !== anchor
      )
        layer.insertBefore(rendered.cell, anchor);
      anchor = rendered.cell;
    }
    restoreKeyboardFocus();
    applyMulti();
    // An incomplete range is reported again so the owner can join or retry it.
    if (
      end > start &&
      (incomplete ||
        reportedRange?.start !== start ||
        reportedRange.end !== end)
    ) {
      reportedRange = { start, end };
      send({ kind: "grid-range", start, end });
    }
  };
  const keyboardTarget = (count: number): number | undefined => {
    if (total === 0) return undefined;
    const first = firstVisibleIndex(count);
    const index = keyboardIndex;
    if (index === undefined || index >= total) return first;
    const firstRow = Math.floor(viewport.scrollTop / rowPitch());
    const rows = Math.ceil(viewportHeight() / rowPitch());
    const row = Math.floor(index / count);
    return row >= firstRow && row < firstRow + rows ? index : first;
  };
  const focusCell = (index: number, count: number) => {
    keyboardIndex = index;
    const row = Math.floor(index / count) * rowPitch();
    if (viewport.scrollTop !== row) viewport.scrollTop = row;
    schedule();
  };
  const applyKey = (event: KeyboardEvent) => {
    const selectedCount = multiCount();
    if (event.key === "Escape" && (selectedCount > 0 || multiMode())) {
      event.preventDefault();
      send({ kind: "grid-multi-clear" });
      return;
    }
    const count = columns();
    const step =
      event.key === "ArrowRight"
        ? 1
        : event.key === "ArrowLeft"
          ? -1
          : event.key === "ArrowDown"
            ? count
            : event.key === "ArrowUp"
              ? -count
              : 0;
    if (step !== 0) {
      event.preventDefault();
      const current = keyboardTarget(count);
      if (current === undefined) return;
      const next = keyboardIndex === current ? current + step : current;
      if (next < 0 || next >= total) return;
      focusCell(next, count);
      return;
    }
    const key = event.key.toLowerCase();
    const field =
      key === "p" || key === "x" || key === "u"
        ? "selectionState"
        : /^[0-5]$/.test(event.key)
          ? "rating"
          : undefined;
    if (!field) return;
    const index = keyboardTarget(count);
    if (index === undefined) return;
    event.preventDefault();
    if (keyboardIndex !== index) focusCell(index, count);
    send({
      kind: "grid-photo-mutation",
      index,
      field,
      value:
        field === "rating"
          ? Number(event.key)
          : key === "p"
            ? "selected"
            : key === "x"
              ? "rejected"
              : "undecided",
    });
  };
  const onResize = () => {
    requestAnimationFrame(() => {
      if (!isAlive() || gridView.hidden) return;
      if (
        columns() === renderedColumns &&
        columnStride() === renderedStride &&
        viewportHeight() === renderedHeight
      )
        return;
      send({ kind: "grid-resize" });
    });
  };
  const onScroll = () => {
    if (isAlive() && !gridView.hidden) schedule();
  };
  viewport.addEventListener("scroll", onScroll);
  window.addEventListener("resize", onResize);

  return {
    render,
    schedule,
    cancel,
    clear,
    rebindDetached,
    applyMulti,
    setInteractive,
    setSize(size: GridThumbnailSize) {
      if (!isAlive() || size === thumbnailSize) return;
      pendingAnchor = firstVisibleIndex(columns());
      thumbnailSize = size;
      applySize();
      schedule();
    },
    handleKey(event: KeyboardEvent) {
      if (viewport.contains(document.activeElement)) applyKey(event);
    },
    resetKeyboard() {
      keyboardIndex = undefined;
    },
    scrollTo(index: number) {
      viewport.scrollTop = Math.floor(index / columns()) * rowPitch();
    },
    captureRestoration(): NavigationGridRestoration | undefined {
      if (!isAlive() || gridView.hidden || total === 0) return undefined;
      const index = firstVisibleIndex(columns());
      const photo = photoAt(index);
      if (!photo) return undefined;
      const offset = viewport.scrollTop % rowPitch();
      const active = document.activeElement;
      const holdsKeyboard =
        active === viewport ||
        (active instanceof HTMLElement && layer.contains(active));
      const activeCellIndex =
        active instanceof HTMLElement && active.dataset.photoIndex !== undefined
          ? Number(active.dataset.photoIndex)
          : keyboardIndex;
      const focusedId =
        activeCellIndex !== undefined &&
        Number.isInteger(activeCellIndex) &&
        activeCellIndex >= 0 &&
        activeCellIndex < total
          ? photoAt(activeCellIndex)?.id
          : undefined;
      return {
        anchor: { photoId: photo.id, indexHint: index, offset },
        focus:
          holdsKeyboard && focusedId
            ? { kind: "photo", photoId: focusedId }
            : { kind: "grid" },
      };
    },
    restoreAnchor(
      model: Readonly<{ index: number; offset: number; focusIndex?: number }>,
    ) {
      if (!isAlive() || gridView.hidden) return;
      keyboardIndex =
        model.focusIndex === undefined
          ? undefined
          : Math.max(0, Math.min(total - 1, model.focusIndex));
      if (
        keyboardIndex === undefined &&
        !layer.contains(document.activeElement)
      )
        viewport.focus({ preventScroll: true });
      if (total === 0) return;
      const target = Math.max(0, Math.min(total - 1, model.index));
      viewport.scrollTop =
        Math.floor(target / columns()) * rowPitch() + model.offset;
      schedule();
    },
    focusIndex(index: number) {
      if (!isAlive()) return;
      const target = Math.max(0, Math.min(Math.max(total - 1, 0), index));
      keyboardIndex = target;
      pendingCellFocus = target;
      viewport.scrollTop = Math.floor(target / columns()) * rowPitch();
      schedule();
    },
    returnFromPhoto(index: number | undefined) {
      keyboardIndex = index;
      if (index !== undefined)
        viewport.scrollTop = Math.floor(index / columns()) * rowPitch();
      schedule();
    },
    dispose() {
      cancel();
      viewport.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onResize);
      clear();
    },
  };
}
