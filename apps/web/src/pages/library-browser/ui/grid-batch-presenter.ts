import type {
  GridViewModel,
  BatchAlbumsViewModel,
} from "./library-browser-view.js";

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
  batchSelect: HTMLButtonElement;
  batchReject: HTMLButtonElement;
  batchAlbumSelect: HTMLSelectElement;
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
  let albums: BatchAlbumsViewModel["albums"] = [];
  let albumsPending = false,
    albumSelection = "",
    signature = "";
  let held: HTMLButtonElement | HTMLSelectElement | null = null;
  let disposed = false;
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
    albums = model.albums;
    albumsPending = model.pending;
    renderTray();
  };
  const renderTray = () => {
    const visible = count > 0 || result !== undefined;
    e.gridBatch.hidden = !visible;
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
      e.batchAlbumSelect.replaceChildren();
      signature = "";
      albumSelection = "";
      options.applyMultiSelection();
      return;
    }
    const resultAction = result?.compensation ?? result?.review;
    e.batchCount.textContent = `${count.toLocaleString()} / ${limit.toLocaleString()} Photos`;
    e.batchRetained.hidden = count === 0 || result === undefined;
    e.batchResult.hidden = result === undefined;
    e.batchResultText.textContent = result?.message ?? "";
    e.batchCompensate.hidden = resultAction === undefined;
    if (resultAction) e.batchCompensate.textContent = resultAction.label;
    e.batchCompensate.dataset.action = result?.compensation
      ? "compensate"
      : "review";
    if (result) e.batchResult.dataset.tone = result.tone;
    else e.batchResult.removeAttribute("data-tone");
    const canAct = enabled && !albumsPending && count > 0;
    if (count === 0) {
      e.batchAlbumSelect.replaceChildren();
      signature = "";
      albumSelection = "";
      e.batchCompensate.hidden = true;
      options.applyMultiSelection();
    } else {
      const nextSignature = albums.map((album) => album.id).join(",");
      if (nextSignature !== signature) {
        signature = nextSignature;
        if (!albums.some((album) => album.id === albumSelection))
          albumSelection = albums[0]?.id ?? "";
        e.batchAlbumSelect.replaceChildren(
          ...albums.map((album) => {
            const option = document.createElement("option");
            option.value = album.id;
            option.textContent = album.name;
            option.selected = album.id === albumSelection;
            return option;
          }),
        );
      }
    }
    if (!canAct && held === null) {
      const active = document.activeElement;
      if (
        active === e.batchSelect ||
        active === e.batchReject ||
        active === e.batchAlbumSelect ||
        active === e.batchAlbumAdd ||
        active === e.batchCompensate
      ) {
        held = active as HTMLButtonElement | HTMLSelectElement;
        e.multiDone.focus();
      }
    }
    e.batchSelect.disabled = !canAct;
    e.batchReject.disabled = !canAct;
    const hasAlbums = e.batchAlbumSelect.options.length > 0;
    e.batchAlbumSelect.disabled = !canAct || !hasAlbums;
    e.batchAlbumAdd.disabled = !canAct || !hasAlbums || !albumSelection;
    e.batchCompensate.disabled = !canAct || resultAction === undefined;
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
  };
  const onSelectChange = () => {
    if (!options.alive()) return;
    albumSelection = e.batchAlbumSelect.value;
    renderTray();
  };
  const onSelect = () => {
    if (!options.alive() || e.batchSelect.disabled) return;
    options.send({ kind: "grid-batch-mutation", value: "picked" });
  };
  const onReject = () => {
    if (!options.alive() || e.batchReject.disabled) return;
    options.send({ kind: "grid-batch-mutation", value: "rejected" });
  };
  const onAdd = () => {
    if (!options.alive() || e.batchAlbumAdd.disabled || !albumSelection) return;
    options.send({ kind: "grid-batch-album-add", albumId: albumSelection });
  };
  const onCompensate = () => {
    if (!options.alive() || e.batchCompensate.disabled) return;
    options.send(
      e.batchCompensate.dataset.action === "review"
        ? { kind: "grid-batch-review" }
        : { kind: "grid-batch-album-remove" },
    );
  };
  e.batchAlbumSelect.addEventListener("change", onSelectChange);
  e.batchSelect.addEventListener("click", onSelect);
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
      e.batchAlbumSelect.removeEventListener("change", onSelectChange);
      e.batchSelect.removeEventListener("click", onSelect);
      e.batchReject.removeEventListener("click", onReject);
      e.batchAlbumAdd.removeEventListener("click", onAdd);
      e.batchCompensate.removeEventListener("click", onCompensate);
    },
  };
};
