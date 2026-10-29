import type { GridViewModel } from "./library-browser-view.js";
import type { BatchAlbumsViewModel } from "../model/grid-batch-albums-contract.js";

type GridBatchResultViewModel = NonNullable<GridViewModel["multi"]["result"]>;

type BatchIntent =
  | Readonly<{ kind: "grid-batch-mutation"; value: "picked" | "rejected" }>
  | Readonly<{ kind: "grid-batch-album-add"; albumId: string }>
  | Readonly<{ kind: "grid-batch-album-remove" }>
  | Readonly<{ kind: "grid-batch-review" }>;

type BatchElements = Readonly<{
  gridBatch: HTMLElement;
  gridTools: HTMLElement;
  gridSelection: HTMLElement;
  gridSelectMode: HTMLButtonElement;
  multiDone: HTMLButtonElement;
  batchCount: HTMLElement;
  batchRetained: HTMLElement;
  batchResult: HTMLElement;
  batchResultText: HTMLElement;
  batchCompensate: HTMLButtonElement;
  batchPick: HTMLButtonElement;
  batchReject: HTMLButtonElement;
  batchAlbumAdd: HTMLButtonElement;
}>;

export type GridBatchPresenter = Readonly<{
  render(model: GridViewModel["multi"]): void;
  reset(): void;
  renderAlbums(model: BatchAlbumsViewModel): void;
  multiMode(): boolean;
  multiCount(): number;
  multiSelected(index: number): boolean;
  applyMulti(): void;
  dispose(): void;
}>;

export const createGridBatchPresenter = (
  options: Readonly<{
    elements: BatchElements;
    alive: () => boolean;
    send: (intent: BatchIntent) => void;
    openAlbumChooser: (invoker: HTMLElement, count: number) => void;
    applyMultiSelection: () => void;
  }>,
): GridBatchPresenter => {
  const { elements: e } = options;
  let mode = false,
    count = 0,
    limit = 0,
    enabled = false;
  let result: GridBatchResultViewModel | undefined;
  let selected: (index: number) => boolean = () => false;
  let albumsPending = false;
  let held: HTMLButtonElement | null = null;
  let disposed = false;
  const gridView = e.gridBatch.closest<HTMLElement>(".grid-view");
  const gridCanvas = gridView?.querySelector<HTMLElement>(".grid-canvas");
  const batchClearance = document.createElement("div");
  batchClearance.className = "grid-batch-clearance";
  batchClearance.setAttribute("aria-hidden", "true");
  gridCanvas?.after(batchClearance);
  const review = document.createElement("button");
  review.type = "button";
  review.className = "quiet";
  review.hidden = true;
  e.batchCompensate.before(review);
  const syncBatchHeight = () => {
    gridView?.style.setProperty(
      "--grid-batch-height",
      `${e.gridBatch.hidden ? 0 : Math.ceil(e.gridBatch.getBoundingClientRect().height)}px`,
    );
  };
  const batchResize =
    typeof ResizeObserver === "undefined"
      ? undefined
      : new ResizeObserver(syncBatchHeight);
  batchResize?.observe(e.gridBatch);
  const render = (multi: GridViewModel["multi"]) => {
    if (disposed || !options.alive()) return;
    mode = multi.mode;
    count = multi.count;
    limit = multi.limit;
    enabled = multi.enabled;
    result = multi.result;
    selected = multi.selected;
    renderTray();
  };
  const reset = () => {
    if (disposed || !options.alive()) return;
    mode = false;
    count = 0;
    enabled = false;
    result = undefined;
    selected = () => false;
    renderTray();
    options.applyMultiSelection();
  };
  const renderAlbums = (model: BatchAlbumsViewModel) => {
    if (disposed || !options.alive()) return;
    albumsPending = model.pending;
    renderTray();
  };
  const renderTray = () => {
    const visible = mode || count > 0;
    gridView?.classList.toggle("multi-mode", visible);
    e.gridBatch.hidden = count === 0;
    e.gridSelectMode.setAttribute("aria-pressed", String(mode));
    const toolsFocus = e.gridTools.contains(document.activeElement),
      selectionFocus = e.gridSelection.contains(document.activeElement);
    e.gridTools.hidden = visible;
    e.gridSelection.hidden = !visible;
    if (toolsFocus && visible) e.multiDone.focus();
    else if (selectionFocus && !visible) e.gridSelectMode.focus();
    if (!visible) {
      e.batchCount.textContent = "";
      e.batchRetained.hidden = true;
      e.batchResult.hidden = true;
      e.batchResultText.textContent = "";
      e.batchCompensate.hidden = true;
      e.batchCompensate.disabled = true;
      review.hidden = true;
      options.applyMultiSelection();
      syncBatchHeight();
      return;
    }
    e.batchCount.textContent = `${count.toLocaleString()} / ${limit.toLocaleString()} selected`;
    e.batchRetained.hidden = count === 0 || result === undefined;
    e.batchResult.hidden = result === undefined;
    e.batchResultText.textContent = result?.message ?? "";
    const resultEntry = e.batchResult.querySelector("summary")!;
    resultEntry.textContent =
      result?.tone === "success" ? "Result" : "Review result";
    resultEntry.setAttribute("aria-label", `Result: ${result?.message ?? ""}`);
    if (!result) e.batchResult.removeAttribute("open");
    e.batchCompensate.hidden = result?.compensation === undefined;
    if (result?.compensation)
      e.batchCompensate.textContent = result.compensation.label;
    review.hidden = result?.review === undefined;
    if (result?.review) review.textContent = result.review.label;
    if (result) e.batchResult.dataset.tone = result.tone;
    else e.batchResult.removeAttribute("data-tone");
    const canAct = enabled && !albumsPending && count > 0;
    if (count === 0) options.applyMultiSelection();
    if (!canAct && held === null) {
      const active = document.activeElement;
      if (
        active === e.batchPick ||
        active === e.batchReject ||
        active === e.batchAlbumAdd ||
        active === e.batchCompensate
      ) {
        held = active as HTMLButtonElement;
        e.multiDone.focus();
      }
    }
    e.batchPick.disabled = !canAct;
    e.batchReject.disabled = !canAct;
    e.batchAlbumAdd.disabled = !canAct;
    e.batchCompensate.disabled = !canAct || result?.compensation === undefined;
    review.disabled = !canAct || result?.review === undefined;
    if (canAct && held) {
      const control = held;
      held = null;
      if (
        control.isConnected &&
        !control.disabled &&
        (document.activeElement === document.body ||
          document.activeElement === e.multiDone)
      )
        control.focus();
    }
    syncBatchHeight();
  };
  const onPick = () => {
    if (!options.alive() || e.batchPick.disabled) return;
    options.send({ kind: "grid-batch-mutation", value: "picked" });
  };
  const onReject = () => {
    if (!options.alive() || e.batchReject.disabled) return;
    options.send({ kind: "grid-batch-mutation", value: "rejected" });
  };
  const onAdd = () => {
    if (!options.alive() || e.batchAlbumAdd.disabled) return;
    options.openAlbumChooser(e.batchAlbumAdd, count);
  };
  const onCompensate = () => {
    if (!options.alive() || e.batchCompensate.disabled) return;
    options.send({ kind: "grid-batch-album-remove" });
  };
  const onReview = () => {
    if (!options.alive() || review.disabled) return;
    e.batchResult.removeAttribute("open");
    options.send({ kind: "grid-batch-review" });
  };
  review.addEventListener("click", onReview);
  e.batchPick.addEventListener("click", onPick);
  e.batchReject.addEventListener("click", onReject);
  e.batchAlbumAdd.addEventListener("click", onAdd);
  e.batchCompensate.addEventListener("click", onCompensate);
  return {
    render,
    reset,
    renderAlbums,
    multiMode: () => mode,
    multiCount: () => count,
    multiSelected: (index) => selected(index),
    applyMulti: options.applyMultiSelection,
    dispose: () => {
      disposed = true;
      batchResize?.disconnect();
      batchClearance.remove();
      review.remove();
      e.batchPick.removeEventListener("click", onPick);
      e.batchReject.removeEventListener("click", onReject);
      e.batchAlbumAdd.removeEventListener("click", onAdd);
      e.batchCompensate.removeEventListener("click", onCompensate);
    },
  };
};
