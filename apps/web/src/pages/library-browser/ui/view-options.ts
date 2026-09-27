import type { ModalSurfaces } from "./modal-surface.js";
import type {
  GridFilterViewModel,
  GridProgressViewModel,
  GridSortViewModel,
  LibraryBrowserIntent,
  ViewSelectionFilter,
  ViewSourceOrder,
} from "./library-browser-view.js";

type ViewOptionsIntentKind = "album-resume" | "view-options-apply";

export type ViewOptionsIntent = Extract<
  LibraryBrowserIntent,
  { kind: ViewOptionsIntentKind }
>;

type ViewOptionsThumbnailSize = "small" | "medium" | "large";

type ViewOption<Value> = Readonly<{ value: Value; label: string }>;
type ViewSourceKind = GridSortViewModel["kind"];

const CAPTURE_TIME_OPTIONS: ReadonlyArray<ViewOption<ViewSourceOrder>> = [
  { value: "source-default", label: "Capture Time, earliest first" },
  { value: "capture-time-desc", label: "Capture Time, latest first" },
];

const SORT_OPTIONS: Record<
  ViewSourceKind,
  ReadonlyArray<ViewOption<ViewSourceOrder>>
> = {
  library: CAPTURE_TIME_OPTIONS,
  folder: CAPTURE_TIME_OPTIONS,
  album: [
    { value: "source-default", label: "Album order" },
    { value: "capture-time-asc", label: "Capture Time, earliest first" },
    { value: "capture-time-desc", label: "Capture Time, latest first" },
  ],
};

const FILTER_OPTIONS: ReadonlyArray<ViewOption<ViewSelectionFilter>> = [
  { value: "all", label: "All" },
  { value: "undecided", label: "Undecided" },
  { value: "selected", label: "Selected" },
  { value: "rejected", label: "Rejected" },
];

export type ViewOptionsElements = Readonly<{
  viewOptionsDialog: HTMLDialogElement;
  viewOptionsOpen: HTMLButtonElement;
  viewOptionsFlag: HTMLElement;
  viewOptionsClose: HTMLButtonElement;
  viewOptionsApply: HTMLButtonElement;
  viewOptionsCancel: HTMLButtonElement;
  albumResume: HTMLButtonElement;
  sortSelect: HTMLSelectElement;
  optionsProgress: HTMLElement;
  optionsVisibleResults: HTMLElement;
  filterSelect: HTMLSelectElement;
  sizeSelect: HTMLSelectElement;
}>;

export interface ViewOptions {
  renderSort(model: GridSortViewModel): void;
  renderFilter(model: GridFilterViewModel): void;
  renderProgress(model: GridProgressViewModel): void;
  dispose(): void;
}

export function createViewOptions({
  elements,
  surfaces,
  send,
  resetGestures,
  closePhotoTools,
  closeRatingChoices,
  activeAlbumId,
  onSizeChange,
}: Readonly<{
  elements: ViewOptionsElements;
  surfaces: ModalSurfaces;
  send: (intent: ViewOptionsIntent) => void;
  resetGestures: () => void;
  closePhotoTools: (restoreFocus: boolean) => void;
  closeRatingChoices: (restoreFocus: boolean) => void;
  activeAlbumId: () => string | undefined;
  onSizeChange: (size: ViewOptionsThumbnailSize) => void;
}>): ViewOptions {
  const {
    viewOptionsDialog,
    viewOptionsOpen,
    viewOptionsFlag,
    viewOptionsClose,
    viewOptionsApply,
    viewOptionsCancel,
    albumResume,
    sortSelect,
    optionsProgress,
    optionsVisibleResults,
    filterSelect,
    sizeSelect,
  } = elements;
  let alive = true;
  let committedOrder: ViewSourceOrder = "source-default";
  let committedFilter: ViewSelectionFilter = "all";
  let draftOrder: ViewSourceOrder = "source-default";
  let draftFilter: ViewSelectionFilter = "all";
  let renderedSortKind: ViewSourceKind | undefined;
  let filterRendered = false;
  let renderedProgressText = "";
  let renderedSourceProgressText = "";

  surfaces.register("view-options", {
    dialog: viewOptionsDialog,
    modal: () => true,
  });

  const presentViewOptionsFlag = () => {
    const parts: string[] = [];
    if (committedFilter !== "all")
      parts.push(
        FILTER_OPTIONS.find((option) => option.value === committedFilter)
          ?.label ?? "",
      );
    if (committedOrder !== "source-default")
      parts.push(
        SORT_OPTIONS[renderedSortKind ?? "library"].find(
          (option) => option.value === committedOrder,
        )?.label ?? "",
      );
    const text = parts.filter(Boolean).join(" · ");
    viewOptionsFlag.textContent = text ? ` · ${text}` : "";
    viewOptionsFlag.hidden = text === "";
    // The visible label is "Options" on a narrow row, so the accessible name
    // spells out the entry and the committed choices it will show.
    viewOptionsOpen.setAttribute(
      "aria-label",
      text === "" ? "View options" : `View options, ${text}`,
    );
  };
  presentViewOptionsFlag();

  const openViewOptions = () => {
    if (!alive) return;
    draftOrder = committedOrder;
    draftFilter = committedFilter;
    sortSelect.value = draftOrder;
    filterSelect.value = draftFilter;
    // Read the invoker before closing the surface that holds it, exactly as
    // opening Sources does.
    const invoker =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : undefined;
    resetGestures();
    closePhotoTools(false);
    closeRatingChoices(false);
    surfaces.open("view-options", invoker);
    viewOptionsClose.focus();
  };

  const closeViewOptions = (restoreFocus = true) => {
    if (!alive) return;
    // Closing without Apply discards the draft: the selects return to the
    // committed choices the open Grid presents.
    draftOrder = committedOrder;
    draftFilter = committedFilter;
    sortSelect.value = committedOrder;
    filterSelect.value = committedFilter;
    surfaces.close("view-options", restoreFocus);
  };

  const applyViewOptions = () => {
    if (!alive) return;
    const order = draftOrder;
    const filter = draftFilter;
    const viewChanged = order !== committedOrder || filter !== committedFilter;
    draftOrder = committedOrder = order;
    draftFilter = committedFilter = filter;
    surfaces.close("view-options", false);
    if (viewChanged) {
      presentViewOptionsFlag();
      send({ kind: "view-options-apply", order, selection: filter });
    }
  };

  viewOptionsOpen.addEventListener("click", () => {
    if (!alive || viewOptionsOpen.hidden) return;
    openViewOptions();
  });
  // A narrow header cannot fit the active-choice flag inside the View options
  // entry, so the flag names the choices beside it and carries the entry's
  // activation: the entry stays the keyboard and assistive-technology path
  // with its full accessible name, and closing the surface returns focus to it
  // because it is the invoker recorded here.
  viewOptionsFlag.addEventListener("click", () => {
    if (!alive || viewOptionsFlag.hidden) return;
    viewOptionsOpen.focus();
    openViewOptions();
  });
  viewOptionsClose.addEventListener("click", () => closeViewOptions());
  viewOptionsCancel.addEventListener("click", () => closeViewOptions());
  viewOptionsApply.addEventListener("click", () => applyViewOptions());
  albumResume.addEventListener("click", () => {
    if (!alive || albumResume.hidden) return;
    const albumId = activeAlbumId();
    if (albumId) send({ kind: "album-resume", albumId });
  });
  sortSelect.addEventListener("change", () => {
    if (!alive) return;
    // View options holds a draft until Apply commits it.
    draftOrder = sortSelect.value as ViewSourceOrder;
  });
  filterSelect.addEventListener("change", () => {
    if (!alive) return;
    draftFilter = filterSelect.value as ViewSelectionFilter;
  });
  sizeSelect.addEventListener("change", () => {
    if (!alive) return;
    onSizeChange(sizeSelect.value as ViewOptionsThumbnailSize);
  });

  return {
    renderSort(model) {
      if (!alive) return;
      if (renderedSortKind !== model.kind) {
        renderedSortKind = model.kind;
        sortSelect.replaceChildren(
          ...SORT_OPTIONS[model.kind].map((option) => {
            const element = document.createElement("option");
            element.value = option.value;
            element.textContent = option.label;
            return element;
          }),
        );
      }
      const options = SORT_OPTIONS[model.kind];
      committedOrder = options.some((option) => option.value === model.value)
        ? model.value
        : options[0]!.value;
      sortSelect.value = committedOrder;
      sortSelect.disabled = !model.enabled;
      presentViewOptionsFlag();
    },
    renderFilter(model) {
      if (!alive) return;
      if (!filterRendered) {
        filterRendered = true;
        filterSelect.replaceChildren(
          ...FILTER_OPTIONS.map((option) => {
            const element = document.createElement("option");
            element.value = option.value;
            element.textContent = option.label;
            return element;
          }),
        );
      }
      committedFilter = model.value;
      filterSelect.value = model.value;
      filterSelect.disabled = !model.enabled;
      presentViewOptionsFlag();
    },
    renderProgress(model) {
      if (!alive) return;
      // The complete source decision counts and the filtered result count are
      // two different facts, so View options names them separately and never
      // derives either from the loaded cells.
      const visibleText =
        model.visible && model.sourceTotal > 0
          ? `Visible results: ${model.visibleTotal.toLocaleString()} of ${model.sourceTotal.toLocaleString()} Photos`
          : "";
      const sourceText =
        model.visible && model.sourceTotal > 0
          ? `Source progress: ${model.selected.toLocaleString()} selected · ${model.rejected.toLocaleString()} rejected · ${model.undecided.toLocaleString()} undecided`
          : "";
      if (
        visibleText === renderedProgressText &&
        sourceText === renderedSourceProgressText
      )
        return;
      renderedProgressText = visibleText;
      renderedSourceProgressText = sourceText;
      optionsVisibleResults.textContent = visibleText;
      optionsProgress.textContent = sourceText;
    },
    dispose() {
      alive = false;
    },
  };
}
