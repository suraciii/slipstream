import type { ModalSurfaces } from "./modal-surface.js";
import { required } from "./presentation-helpers.js";
import { formatPhotoCount } from "./photo-count.js";
import type {
  GridFilterViewModel,
  GridProgressViewModel,
  GridSortViewModel,
  LibraryBrowserIntent,
  ViewSelectionFilter,
  ViewSourceOrder,
} from "./library-browser-view.js";
import type { ViewFilterConditions } from "./library-browser-view-contract.js";
import { sameConditions } from "../model/navigation-address.js";
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

/// The Selection State segments. The committed filter is one value, so exactly
/// one segment carries `aria-pressed="true"`: `all` means no Selection State
/// condition, and each state segment is that single value.
const SELECTION_SEGMENTS: ReadonlyArray<ViewOption<ViewSelectionFilter>> = [
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
  optionsFooter: HTMLElement;
  optionsMainView: HTMLElement;
  sourceActionsView: HTMLElement;
  sourceActionsOpen: HTMLButtonElement;
  sourceActionsReturn: HTMLButtonElement;
  resetFilters: HTMLButtonElement;
  draftDiscardNotice: HTMLElement;
  advancedFilters: HTMLDetailsElement | null;
  albumResume: HTMLButtonElement;
  sortSelect: HTMLSelectElement;
  optionsProgress: HTMLElement;
  optionsVisibleResults: HTMLElement;
  committedFilters: HTMLElement;
  selectionSegments: HTMLElement;
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
  discardDraftForImmediateAction(): void;
  clearFilters(): void;
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
  randomHeader,
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
  randomHeader: MediaQueryList;
  syncSecondarySurface?: () => void;
}>): ViewOptions {
  const {
    viewOptionsDialog,
    viewOptionsOpen,
    viewOptionsFlag,
    viewOptionsClose,
    viewOptionsApply,
    viewOptionsCancel,
    optionsFooter,
    optionsMainView,
    sourceActionsView,
    sourceActionsOpen,
    sourceActionsReturn,
    resetFilters,
    draftDiscardNotice,
    advancedFilters,
    albumResume,
    sortSelect,
    optionsProgress,
    optionsVisibleResults,
    committedFilters,
    selectionSegments,
    sizeSelect,
    filters: filterControls,
  } = elements;
  // One owner for the staged fact conditions: Rating bounds, Original kind,
  // Original availability, and the Capture Time range. They commit together
  // with order and Selection State on one Apply.
  const filters = createViewOptionsFilters(filterControls, advancedFilters);
  const segmentButtons = Array.from(
    selectionSegments.querySelectorAll<HTMLButtonElement>("[data-segment]"),
  );
  /// Selection State is one committed value, so the segment set marks exactly
  /// one pressed segment and the rest unpressed.
  const presentSelectionSegments = (value: ViewSelectionFilter): void => {
    for (const button of segmentButtons) {
      const segment = button.dataset.segment as ViewSelectionFilter | undefined;
      button.setAttribute(
        "aria-pressed",
        String(segment === undefined ? false : segment === value),
      );
    }
  };
  const randomPhoto = required<HTMLButtonElement>(root, "[data-random-photo]");
  const removalOpen = required<HTMLButtonElement>(root, "[data-removal-open]");
  const gridTools = required<HTMLElement>(root, "[data-grid-tools]");
  const browseActions = required<HTMLElement>(root, "[data-browse-actions]");
  const syncRandomHost = () => {
    // The normal header keeps the complete Random Photo label only while it
    // has room beside the current source title. The intermediate wide layout
    // would otherwise squeeze the title to a single character.
    const host =
      compact.matches || randomHeader.matches ? browseActions : gridTools;
    if (randomPhoto.parentElement !== host) host.append(randomPhoto);
  };
  const onCompactChange = () => syncRandomHost();
  compact.addEventListener("change", onCompactChange);
  randomHeader.addEventListener("change", onCompactChange);
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
  let renderedProgressText = "";
  let renderedProgressSummary = "";
  const wideSummary = document.createElement("span");
  wideSummary.className = "options-summary-wide";
  const compactSummary = document.createElement("span");
  compactSummary.className = "options-summary-compact";
  viewOptionsFlag.setAttribute("aria-hidden", "true");
  viewOptionsFlag.replaceChildren(wideSummary, compactSummary);

  const syncDraftNotice = () => {
    const dirty =
      draftOrder !== committedOrder ||
      draftFilter !== committedFilter ||
      draftSize !== committedSize ||
      filters.draftChanged();
    draftDiscardNotice.hidden = !dirty;
    draftDiscardNotice.textContent = dirty
      ? "Browse and Source actions use committed choices and discard unapplied changes."
      : "";
  };

  const resetDraft = () => {
    draftOrder = committedOrder;
    draftFilter = committedFilter;
    draftSize = committedSize;
    sizeSelect.value = committedSize;
    sortSelect.value = committedOrder;
    presentSelectionSegments(committedFilter);
    filters.reset(committedConditions);
    if (advancedFilters) advancedFilters.open = filters.advancedActive();
    syncDraftNotice();
  };
  const onClose = () => {
    resetDraft();
    resetOptionsView();
    viewOptionsOpen.setAttribute("aria-expanded", "false");
    syncSecondarySurface?.();
  };
  viewOptionsDialog.addEventListener("close", onClose);

  surfaces.register("view-options", {
    dialog: viewOptionsDialog,
    modal: () => true,
    // Returning from a management surface must also discard unapplied choices.
    onOpen: resetDraft,
  });

  const summaryToken = (text: string): HTMLSpanElement => {
    const token = document.createElement("span");
    token.className = "options-summary-token";
    token.textContent = text;
    return token;
  };

  const presentViewOptionsFlag = () => {
    const filterParts: string[] = [];
    if (committedFilter !== "all")
      filterParts.push(
        SELECTION_SEGMENTS.find((option) => option.value === committedFilter)
          ?.label ?? "",
      );
    filterParts.push(...filters.committedLabels());
    const parts = [...filterParts];
    if (committedOrder !== "source-default")
      parts.push(
        SORT_OPTIONS[renderedSortKind ?? "library"].find(
          (option) => option.value === committedOrder,
        )?.label ?? "",
      );
    // The summary exists for filter and order state; decision counts ride
    // along only while it is shown, so the idle entry keeps its one-row
    // header budget.
    const summaryParts = parts.length
      ? [...parts, renderedProgressSummary].filter(Boolean)
      : [];
    const text = summaryParts.join(" · ");
    const compactParts = [
      filterParts.length ? `Filters: ${filterParts.length}` : "",
      committedOrder !== "source-default" ? "Custom order" : "",
      parts.length ? renderedProgressSummary : "",
    ].filter(Boolean);
    wideSummary.replaceChildren(...summaryParts.map(summaryToken));
    compactSummary.textContent = compactParts.length
      ? ` · ${compactParts.join(" · ")}`
      : "";
    committedFilters.replaceChildren();
    if (filterParts.length > 0) {
      const label = document.createElement("span");
      label.className = "options-summary-label";
      label.textContent = "Applied filter";
      committedFilters.append(label, ...filterParts.map(summaryToken));
    }
    committedFilters.hidden = filterParts.length === 0;
    viewOptionsFlag.hidden = text === "";
    viewOptionsOpen.setAttribute(
      "aria-label",
      text === "" ? "View options" : `View options, ${text}`,
    );
  };
  presentViewOptionsFlag();

  /// Source actions name their consequence before activation: the removal
  /// review entry names the Rejected count of the current source and the
  /// source itself, so the Photographer sees what would leave before opening
  /// the review. The count comes from the server's complete-source facts, the
  /// same numbers Source progress reports, never from loaded Grid windows.
  /// A zero count disables the entry instead of hiding it, so the capability
  /// stays discoverable and its scope stays stated.
  let renderedRemovalCount = 0;
  let renderedRemovalSource = "";
  const presentRemovalAction = (model: {
    rejected: number;
    sourceName: string;
  }): void => {
    if (
      model.rejected === renderedRemovalCount &&
      model.sourceName === renderedRemovalSource
    )
      return;
    renderedRemovalCount = model.rejected;
    renderedRemovalSource = model.sourceName;
    // The label states the count with the same Photo/Photos wording the rest
    // of the surface uses, so the entry names the reviewed result exactly.
    removalOpen.textContent =
      model.rejected === 0
        ? "Remove rejected Photos"
        : `Remove ${formatPhotoCount(model.rejected)} reviewed as Rejected from ${model.sourceName}`;
    removalOpen.disabled = model.rejected === 0;
  };

  const showOptionsView = (
    view: "main" | "source-actions",
    focus?: HTMLElement,
  ): void => {
    const sourceActions = view === "source-actions";
    optionsMainView.hidden = sourceActions;
    sourceActionsView.hidden = !sourceActions;
    optionsFooter.hidden = sourceActions;
    focus?.focus();
  };
  const resetOptionsView = (): void => showOptionsView("main");

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
    resetOptionsView();
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
    const order = draftOrder;
    const filter = draftFilter;
    const conditions = filters.read();
    if (!conditions) return;
    const viewChanged =
      order !== committedOrder ||
      filter !== committedFilter ||
      !sameConditions(conditions, committedConditions);
    const sizeChanged = draftSize !== committedSize;
    committedSize = draftSize;
    committedConditions = conditions;
    filters.setCommitted(conditions);
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
  viewOptionsClose.addEventListener("click", () => closeViewOptions());
  viewOptionsCancel.addEventListener("click", () => closeViewOptions());
  sourceActionsOpen.addEventListener("click", () => {
    if (!alive) return;
    showOptionsView("source-actions", sourceActionsReturn);
  });
  sourceActionsReturn.addEventListener("click", () => {
    if (!alive) return;
    showOptionsView("main", sourceActionsOpen);
  });
  viewOptionsApply.addEventListener("click", () => applyViewOptions());
  resetFilters.addEventListener("click", () => {
    draftFilter = "all";
    presentSelectionSegments("all");
    filters.resetDraft();
    syncDraftNotice();
  });
  viewOptionsDialog.addEventListener("input", syncDraftNotice);
  viewOptionsDialog.addEventListener("change", syncDraftNotice);
  albumResume.addEventListener("click", () => {
    if (!alive || albumResume.hidden) return;
    const albumId = activeAlbumId();
    if (viewOptionsDialog.open) closeViewOptions(false);
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
    syncDraftNotice();
  });
  // One Selection State is committed at a time, so a segment press replaces
  // the draft value and re-marks exactly one pressed segment.
  for (const button of segmentButtons) {
    button.addEventListener("click", () => {
      if (!alive) return;
      const value = button.dataset.segment as ViewSelectionFilter;
      if (SELECTION_SEGMENTS.some((option) => option.value === value)) {
        draftFilter = value;
        presentSelectionSegments(value);
        syncDraftNotice();
      }
    });
  }
  sizeSelect.addEventListener("change", () => {
    if (!alive) return;
    draftSize = sizeSelect.value as ViewOptionsThumbnailSize;
    syncDraftNotice();
  });

  return {
    open: openViewOptions,
    discardDraftForImmediateAction() {
      if (viewOptionsDialog.open) closeViewOptions(false);
    },
    clearFilters() {
      if (!alive) return;
      send({
        kind: "view-options-apply",
        order: committedOrder,
        selection: "all",
        conditions: {},
      });
    },
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
      if (viewOptionsDialog.open) syncDraftNotice();
      presentViewOptionsFlag();
    },
    renderFilter(model) {
      if (!alive) return;
      committedFilter = model.value;
      committedConditions = model.conditions;
      if (viewOptionsDialog.open) {
        // A background render updates the committed facts but never the
        // staged choices: the Photographer's unapplied Selection State and
        // conditions survive until Apply, Cancel, or an immediate source
        // action discards the draft.
        filters.setCommitted(model.conditions);
      } else {
        filters.reset(model.conditions);
        draftFilter = model.value;
        presentSelectionSegments(committedFilter);
      }
      for (const button of segmentButtons) {
        button.disabled = !model.enabled;
      }
      if (viewOptionsDialog.open) syncDraftNotice();
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
      const progressSummary =
        model.visible && model.sourceTotal > 0
          ? `${model.picked.toLocaleString()} picked · ${model.rejected.toLocaleString()} rejected · ${model.unflagged.toLocaleString()} unflagged`
          : "";
      const sourceText = progressSummary
        ? `Source progress: ${progressSummary}`
        : "";
      presentRemovalAction(model);
      if (
        visibleText === renderedProgressText &&
        progressSummary === renderedProgressSummary
      )
        return;
      renderedProgressText = visibleText;
      renderedProgressSummary = progressSummary;
      optionsVisibleResults.textContent = visibleText;
      optionsProgress.textContent = sourceText;
      presentViewOptionsFlag();
    },
    renderRandom(model) {
      if (!alive) return;
      randomPhoto.hidden = !model.visible;
      randomPhoto.disabled = !model.enabled;
      randomPhoto.textContent = model.pending
        ? "Opening random Photo…"
        : "Open a random Photo";
      randomPhoto.setAttribute("aria-busy", String(model.pending));
      const filterLabel = SELECTION_SEGMENTS.find(
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
      viewOptionsDialog.removeEventListener("close", onClose);
      compact.removeEventListener("change", onCompactChange);
      randomHeader.removeEventListener("change", onCompactChange);
    },
  };
}
