import {
  createGridCellPresenter,
  type GridCell,
  type GridPhotoViewModel,
  type GridThumbnailBinding,
} from "./grid-cell-presenter.js";
import { gridThumbnailTarget } from "./grid-thumbnail-target.js";
import type { NavigationGridRestoration } from "../model/browser-navigation.js";

type GridThumbnailSize = "small" | "medium" | "large";
const DESKTOP_GAP = 22;
const COMPACT_GAP = 12;
const DESKTOP_INSET_X = 28;
const DESKTOP_INSET_Y = 24;
const COMPACT_INSET = 16;
const GRID_THUMBNAIL_SIZE_STEPS: Readonly<
  Record<
    GridThumbnailSize,
    Readonly<{
      width: number;
      compactWidth: number;
      label: string;
    }>
  >
> = {
  small: { width: 120, compactWidth: 96, label: "Small" },
  medium: { width: 180, compactWidth: 132, label: "Medium" },
  large: { width: 260, compactWidth: 220, label: "Large" },
};

type GridPresentation = Readonly<{
  total: number;
  photoAt(index: number): GridPhotoViewModel | undefined;
}>;

type GridIntent =
  | Readonly<{ kind: "grid-render" | "grid-multi-clear" }>
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
      value: number | "picked" | "rejected" | "unflagged";
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
  let renderedGeometry:
    | Readonly<{
        count: number;
        stride: number;
        pitch: number;
        inset: number;
        gap: number;
      }>
    | undefined;
  let renderFrame: number | undefined;
  const renderedCells = new Map<number, GridCell>();
  let reportedRange: Readonly<{ start: number; end: number }> | undefined;
  let photoAt: GridPresentation["photoAt"] = () => undefined;

  const gapX = () => (compact.matches ? COMPACT_GAP : DESKTOP_GAP);
  const gapY = () => (compact.matches ? COMPACT_GAP : DESKTOP_GAP);
  const insetX = () => (compact.matches ? COMPACT_INSET : DESKTOP_INSET_X);
  const insetY = () => (compact.matches ? COMPACT_INSET : DESKTOP_INSET_Y);
  const viewportHeight = () =>
    Math.max(360, Math.min(viewport.clientHeight, window.innerHeight));

  // Measure the longest required state and a full Rating using the same CSS
  // as a real caption. This is independent of the Photos currently loaded.
  const probe = document.createElement("button");
  probe.type = "button";
  probe.className = "grid-caption-probe";
  probe.tabIndex = -1;
  probe.setAttribute("aria-hidden", "true");
  Object.assign(probe.style, {
    visibility: "hidden",
    pointerEvents: "none",
    height: "auto",
    display: "block",
    position: "absolute",
    left: "0",
    top: "0",
    padding: "0",
    border: "0",
    textAlign: "left",
    containerType: "inline-size",
  });
  probe.innerHTML = `
    <span class="cell-footer"><span class="cell-caption">
      <span class="cell-line"><span class="cell-check">✓</span><span class="cell-name">Photo</span><span class="cell-pos"></span></span>
      <span class="cell-meta"><span class="cell-state-group"><span class="state-dot state-unflagged"></span><span class="state-text">Unflagged</span></span><span class="stars">☆☆☆☆☆</span><span class="cell-kind"></span></span>
    </span></span>`;
  gridView.append(probe);
  const probeFooter = probe.querySelector<HTMLElement>(".cell-footer")!;
  const probePosition = probe.querySelector<HTMLElement>(".cell-pos")!;
  const probeMeta = probe.querySelector<HTMLElement>(".cell-meta")!;
  const probeState = probe.querySelector<HTMLElement>(".cell-state-group")!;
  const probeStars = probe.querySelector<HTMLElement>(".stars")!;
  const probeStateText = probe.querySelector<HTMLElement>(".state-text")!;
  const probeMarker = probeState.firstElementChild as HTMLElement;
  const stateSamples = [
    ["Unflagged", "state-dot state-unflagged"],
    ["Picked", "cell-state picked"],
    ["Rejected", "cell-state rejected"],
  ] as const;

  const layout = () => {
    const available = Math.max(0, viewport.clientWidth - insetX() * 2);
    const step = GRID_THUMBNAIL_SIZE_STEPS[thumbnailSize];
    const target = compact.matches ? step.compactWidth : step.width;
    const count = Math.max(
      1,
      Math.floor((available + gapX()) / (target + gapX())),
    );
    const stride = (available + gapX()) / count;
    const width = Math.max(0, stride - gapX());
    probe.style.width = `${width}px`;
    probePosition.textContent = String(Math.max(1, total));
    let widestState: (typeof stateSamples)[number] = stateSamples[0];
    let stateWidth = 0;
    for (const sample of stateSamples) {
      probeStateText.textContent = sample[0];
      probeMarker.className = sample[1];
      const measured = probeState.getBoundingClientRect().width;
      if (measured > stateWidth) {
        widestState = sample;
        stateWidth = measured;
      }
    }
    probeStateText.textContent = widestState[0];
    probeMarker.className = widestState[1];
    probeStars.textContent = "☆☆☆☆☆";
    const emptyStarsWidth = probeStars.getBoundingClientRect().width;
    probeStars.textContent = "★★★★★";
    if (emptyStarsWidth >= probeStars.getBoundingClientRect().width)
      probeStars.textContent = "☆☆☆☆☆";
    const metaGap =
      Number.parseFloat(getComputedStyle(probeMeta).columnGap) || 0;
    browser.dataset.gridKind =
      probeState.getBoundingClientRect().width +
        probeStars.getBoundingClientRect().width +
        2 * metaGap +
        36 <=
      width
        ? "visible"
        : "hidden";
    const caption = Math.ceil(probeFooter.getBoundingClientRect().height);
    const cellHeight = Math.ceil(width * (2 / 3)) + caption;
    return {
      count,
      stride,
      width,
      caption,
      cellHeight,
      pitch: cellHeight + gapY(),
    };
  };
  const columns = () => renderedGeometry?.count ?? layout().count;
  const columnStride = () => renderedGeometry?.stride ?? layout().stride;
  const rowPitch = () => renderedGeometry?.pitch ?? layout().pitch;
  const firstVisibleIndex = (count: number): number => {
    if (total === 0) return 0;
    const inset = renderedGeometry?.inset ?? insetY();
    const gap = renderedGeometry?.gap ?? gapY();
    const row = Math.floor(
      Math.max(0, viewport.scrollTop - inset + gap) / rowPitch(),
    );
    return Math.min(total - 1, row * count);
  };
  const applySize = () => {
    browser.dataset.gridSize = thumbnailSize;
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
    gapX,
    insetX,
    insetY,
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
  let currentIndex: number | undefined;

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
  const applyCurrent = () => {
    for (const [index, rendered] of renderedCells)
      rendered.cell.classList.toggle("is-current", index === currentIndex);
  };
  // Rebind only images detached by a source boundary; retained cells otherwise
  // keep their in-flight thumbnails across merged renders.
  const rebindDetached = (model: GridPresentation) => {
    if (!isAlive() || gridView.hidden) return;
    const count = columns();
    const stride = columnStride();
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
    const previous = renderedGeometry;
    const geometry = layout();
    const { count, stride, pitch } = geometry;
    const height = viewportHeight();
    let resizeAnchor: Readonly<{ index: number; offset: number }> | undefined;
    if (
      previous &&
      pendingAnchor === undefined &&
      position === undefined &&
      (previous.count !== count ||
        previous.stride !== stride ||
        previous.pitch !== pitch)
    ) {
      resizeAnchor = {
        index: firstVisibleIndex(previous.count),
        offset: Math.min(viewport.scrollTop % previous.pitch, pitch - 1),
      };
    }
    renderedGeometry = { count, stride, pitch, inset: insetY(), gap: gapY() };
    browser.style.setProperty("--grid-cell-width", `${geometry.width}px`);
    browser.style.setProperty("--grid-cell-height", `${geometry.cellHeight}px`);
    browser.style.setProperty("--grid-caption-height", `${geometry.caption}px`);
    viewport.dataset.columns = String(count);
    const canvasHeight = `${
      Math.ceil(model.total / count) * pitch - gapY() + insetY() * 2
    }px`;
    canvas.style.height = canvasHeight;
    layer.style.height = canvasHeight;
    if (resizeAnchor)
      viewport.scrollTop =
        Math.floor(resizeAnchor.index / count) * pitch + resizeAnchor.offset;
    if (pendingAnchor !== undefined) {
      viewport.scrollTop = Math.floor(pendingAnchor / count) * pitch;
      pendingAnchor = undefined;
    }
    if (position !== undefined)
      viewport.scrollTop = Math.floor(position / count) * pitch;
    const firstRow = Math.max(
      0,
      Math.floor(Math.max(0, viewport.scrollTop - insetY()) / pitch) - 2,
    );
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
    applyCurrent();
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
            ? "picked"
            : key === "x"
              ? "rejected"
              : "unflagged",
    });
  };
  const onResize = () => {
    if (isAlive() && !gridView.hidden) schedule();
  };
  const resizeObserver = new ResizeObserver(onResize);
  resizeObserver.observe(viewport);
  resizeObserver.observe(probeFooter);
  document.fonts.addEventListener("loadingdone", onResize);
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
      currentIndex = undefined;
      applyCurrent();
    },
    scrollTo(index: number) {
      viewport.scrollTop = Math.floor(index / columns()) * rowPitch();
    },
    captureRestoration(): NavigationGridRestoration | undefined {
      if (!isAlive() || gridView.hidden || total === 0) return undefined;
      // Capture the row at the scroll boundary and the pixel offset into it.
      // The presentation inset is not scroll distance: subtracting it here
      // would name the previous, offscreen row and restore one row too high.
      const index = Math.min(
        total - 1,
        Math.floor(viewport.scrollTop / rowPitch()) * columns(),
      );
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
      if (index !== undefined) {
        currentIndex = index;
        viewport.scrollTop = Math.floor(index / columns()) * rowPitch();
      }
      applyCurrent();
      schedule();
    },
    dispose() {
      cancel();
      viewport.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onResize);
      resizeObserver.disconnect();
      document.fonts.removeEventListener("loadingdone", onResize);
      probe.remove();
      clear();
    },
  };
}
