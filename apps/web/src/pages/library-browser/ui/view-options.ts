import type { ModalSurfaces } from "./modal-surface.js";
import { required } from "./presentation-helpers.js";
import type {
  GridFilterViewModel,
  GridProgressViewModel,
  GridSortViewModel,
  LibraryBrowserIntent,
  ViewSelectionFilter,
  ViewSourceOrder,
} from "./library-browser-view.js";
import type { ViewFilterConditions } from "./library-browser-view-contract.js";
import {
  createViewOptionsFilters,
  type ViewOptionsFilterElements,
} from "./view-options-filters.js";

type ViewOptionsIntentKind =
  | "album-resume"
  | "view-options-apply"
  | "random-photo";

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
  { value: "unflagged", label: "Unflagged" },
  { value: "picked", label: "Picked" },
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
  filters: ViewOptionsFilterElements;
}>;

/// The one Grid action View options presents: it samples the current Browse
/// Snapshot, so its accessible name names the source and committed filter the
/// Photographer is sampling from rather than restating them as "current".
export type RandomPhotoViewModel = Readonly<{
  visible: boolean;
  enabled: boolean;
  pending: boolean;
  sourceName: string;
  filter: ViewSelectionFilter;
}>;

export interface ViewOptions {
  open(): void;
  renderSort(model: GridSortViewModel): void;
  renderFilter(model: GridFilterViewModel): void;
  renderProgress(model: GridProgressViewModel): void;
  renderRandom(model: RandomPhotoViewModel): void;
  dispose(): void;
}

export function createViewOptions({
  root,
  elements,
  surfaces,
  send,
  resetGestures,
  closePhotoTools,
  closeRatingChoices,
  activeAlbumId,
  onSizeChange,
  compact,
  syncSecondarySurface,
}: Readonly<{
  root: ParentNode;
  elements: ViewOptionsElements;
  surfaces: ModalSurfaces;
  send: (intent: ViewOptionsIntent) => void;
  resetGestures: () => void;
  closePhotoTools: (restoreFocus: boolean) => void;
  closeRatingChoices: (restoreFocus: boolean) => void;
  activeAlbumId: () => string | undefined;
  onSizeChange: (size: ViewOptionsThumbnailSize) => void;
  compact: MediaQueryList;
  syncSecondarySurface?: () => void;
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
    filters: filterControls,
  } = elements;
  // One owner for the staged fact conditions: Rating bounds, Original kind,
  // Original availability, and the Capture Time range. They commit together
  // with order and Selection State on one Apply.
  const filters = createViewOptionsFilters(filterControls);
  const randomPhoto = required<HTMLButtonElement>(root, "[data-random-photo]");
  const gridTools = required<HTMLElement>(root, "[data-grid-tools]");
  const browseActions = required<HTMLElement>(root, "[data-browse-actions]");
  const syncRandomHost = () => {
    const host = compact.matches ? browseActions : gridTools;
    if (randomPhoto.parentElement !== host) host.append(randomPhoto);
  };
  const onCompactChange = () => syncRandomHost();
  compact.addEventListener("change", onCompactChange);
  syncRandomHost();

  let alive = true;
  let committedOrder: ViewSourceOrder = "source-default";
  let committedConditions: ViewFilterConditions = {};
  let committedFilter: ViewSelectionFilter = "all";
  let draftOrder: ViewSourceOrder = "source-default";
  let draftFilter: ViewSelectionFilter = "all";
  let committedSize: ViewOptionsThumbnailSize = "medium";
  let draftSize: ViewOptionsThumbnailSize = committedSize;
  let renderedSortKind: ViewSourceKind | undefined;
  let filterRendered = false;
  let renderedProgressText = "";
  let renderedSourceProgressText = "";

  const resetDraft = () => {
    draftOrder = committedOrder;
    draftFilter = committedFilter;
    draftSize = committedSize;
    sizeSelect.value = committedSize;
    sortSelect.value = committedOrder;
    filterSelect.value = committedFilter;
    filters.reset(committedConditions);
  };
  viewOptionsDialog.addEventListener("close", () => {
    resetDraft();
    viewOptionsOpen.setAttribute("aria-expanded", "false");
    syncSecondarySurface?.();
  });

  surfaces.register("view-options", {
    dialog: viewOptionsDialog,
    modal: () => true,
    // Returning from a management surface must also discard unapplied choices.
    onOpen: resetDraft,
  });

  const presentViewOptionsFlag = () => {
    const parts: string[] = [];
    if (committedFilter !== "all")
      parts.push(
        FILTER_OPTIONS.find((option) => option.value === committedFilter)
          ?.label ?? "",
      );
    parts.push(...filters.committedLabels());
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
    viewOptionsOpen.setAttribute("aria-expanded", "true");
    syncSecondarySurface?.();
    viewOptionsClose.focus();
  };

  const closeViewOptions = (restoreFocus = true) => {
    if (!alive) return;
    // Closing without Apply discards the draft: the selects return to the
    // committed choices the open Grid presents.
    resetDraft();
    viewOptionsOpen.setAttribute("aria-expanded", "false");
    syncSecondarySurface?.();
    surfaces.close("view-options", restoreFocus);
  };

  const applyViewOptions = () => {
    if (!alive) return;
    const previousLabels = filters.committedLabels().join(" · ");
    const order = draftOrder;
    const filter = draftFilter;
    const conditions = filters.commit();
    const conditionsChanged =
      filters.committedLabels().join(" · ") !== previousLabels;
    const viewChanged =
      order !== committedOrder ||
      filter !== committedFilter ||
      conditionsChanged;
    const sizeChanged = draftSize !== committedSize;
    committedSize = draftSize;
    committedConditions = conditions;
    draftOrder = committedOrder = order;
    draftFilter = committedFilter = filter;
    surfaces.close("view-options", !viewChanged);
    viewOptionsOpen.setAttribute("aria-expanded", "false");
    syncSecondarySurface?.();
    if (sizeChanged) onSizeChange(committedSize);
    if (viewChanged) {
      presentViewOptionsFlag();
      send({
        kind: "view-options-apply",
        order,
        selection: filter,
        conditions,
      });
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
  randomPhoto.addEventListener("click", () => {
    if (!alive || randomPhoto.disabled) return;
    if (viewOptionsDialog.open) closeViewOptions(false);
    send({ kind: "random-photo" });
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
    draftSize = sizeSelect.value as ViewOptionsThumbnailSize;
  });

  return {
    open: openViewOptions,
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
      if (!viewOptionsDialog.open) sortSelect.value = committedOrder;
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
      committedConditions = model.conditions;
      if (viewOptionsDialog.open) {
        filters.setCommitted(model.conditions);
      } else {
        filterSelect.value = model.value;
        filters.reset(model.conditions);
      }
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
          ? `Source progress: ${model.picked.toLocaleString()} picked · ${model.rejected.toLocaleString()} rejected · ${model.unflagged.toLocaleString()} unflagged`
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
    renderRandom(model) {
      if (!alive) return;
      randomPhoto.hidden = !model.visible;
      randomPhoto.disabled = !model.enabled;
      randomPhoto.textContent = model.pending
        ? "Opening random Photo…"
        : "Open a random Photo";
      randomPhoto.setAttribute("aria-busy", String(model.pending));
      const filterLabel = FILTER_OPTIONS.find(
        (option) => option.value === model.filter,
      )?.label;
      const filterSuffix =
        model.filter === "all" || !filterLabel
          ? ""
          : ` · ${filterLabel} filter`;
      randomPhoto.setAttribute(
        "aria-label",
        `Open a random Photo from ${model.sourceName}${filterSuffix}`,
      );
    },
    dispose() {
      alive = false;
      viewOptionsDialog.removeEventListener("close", resetDraft);
      compact.removeEventListener("change", onCompactChange);
    },
  };
}
