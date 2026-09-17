import "./library-browser.css";

import { formatCaptureTime } from "./capture-time.js";
import { formatPhotoCount } from "./photo-count.js";

type ViewSelectionState = "undecided" | "selected" | "rejected";
type ViewPreviewSource = "matching-jpeg" | "embedded-raw-jpeg";

/**
 * Grid thumbnail sizes. Each step is the cell box the CSS renders; the Grid
 * adds the ordinary inter-cell gap to get the column and row pitch of its
 * virtualized layout, so one step drives the CSS cell box and every geometry
 * calculation together.
 */
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
const DEFAULT_GRID_THUMBNAIL_SIZE: GridThumbnailSize = "medium";
/** Full wording behind the compact limited-detail marker in the Preview fact. */
const LIMITED_PREVIEW_DETAIL = "Limited by camera Preview resolution";
const SWIPE_PENDING_PIXELS = 24;
const SWIPE_COMMIT_PIXELS = 72;
const SWIPE_COMMIT_VELOCITY = 0.5;

type SourceReference =
  | Readonly<{ kind: "library" }>
  | Readonly<{ kind: "album"; id: string }>
  | Readonly<{ kind: "folder"; location: string; name: string }>;

export type AlbumFormReference = Readonly<{
  formId: string;
  kind: "create" | "rename" | "delete";
  albumId?: string;
  name: string;
}>;

export type ViewSourceOrder =
  | "source-default"
  | "capture-time-asc"
  | "capture-time-desc";

/// One Selection State filter for the open source. `all` keeps every Photo of
/// the source order.
export type ViewSelectionFilter = "all" | ViewSelectionState;

type ViewSourceKind = "library" | "album" | "folder";

type ViewOption<Value> = Readonly<{ value: Value; label: string }>;

/// Capture Time is ascending by default, so only the reversed direction needs
/// its own option.
const CAPTURE_TIME_OPTIONS: ReadonlyArray<ViewOption<ViewSourceOrder>> = [
  { value: "source-default", label: "Capture Time, earliest first" },
  { value: "capture-time-desc", label: "Capture Time, latest first" },
];

/// The Grid's explicit order selection for the open source. Option labels are
/// presentation: `source-default` names the order the server applies when no
/// explicit order is requested (Capture Time earliest first, or Album order).
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

export type GridSortViewModel = Readonly<{
  kind: ViewSourceKind;
  value: ViewSourceOrder;
  enabled: boolean;
}>;

/// The Grid's Selection State filter for the open source. The filter is a view
/// option of every source kind, so no option list depends on the kind.
const FILTER_OPTIONS: ReadonlyArray<ViewOption<ViewSelectionFilter>> = [
  { value: "all", label: "All" },
  { value: "undecided", label: "Undecided" },
  { value: "selected", label: "Selected" },
  { value: "rejected", label: "Rejected" },
];

export type GridFilterViewModel = Readonly<{
  value: ViewSelectionFilter;
  enabled: boolean;
}>;

/// Decision progress for the open source: the per-state counts the server
/// reports for that source. `visible` is false while the Grid presents no
/// source, so a source transition never shows the previous source's progress.
export type GridProgressViewModel = Readonly<{
  visible: boolean;
  selected: number;
  rejected: number;
  undecided: number;
}>;

export type LibraryBrowserIntent =
  | Readonly<{ kind: "summary-action"; presentationId: number }>
  | Readonly<{ kind: "sort-change"; order: ViewSourceOrder }>
  | Readonly<{ kind: "filter-change"; selection: ViewSelectionFilter }>
  | Readonly<{ kind: "source-open"; source: SourceReference }>
  | Readonly<{ kind: "file-location-retry"; key: string }>
  | Readonly<{ kind: "folder-toggle"; location: string; expanded: boolean }>
  | Readonly<{ kind: "folder-page"; location: string; direction: -1 | 1 }>
  | Readonly<{ kind: "folder-album-add"; albumId: string }>
  | Readonly<{ kind: "album-form-open"; form: AlbumFormReference }>
  | Readonly<{ kind: "album-form-close"; formId: string }>
  | Readonly<{
      kind: "album-form-submit";
      formId: string;
      name?: string;
    }>
  | Readonly<{ kind: "grid-render" | "grid-resize" }>
  | Readonly<{ kind: "filmstrip-resize" }>
  | Readonly<{ kind: "grid-range"; start: number; end: number }>
  | Readonly<{
      kind: "open-photo";
      index: number;
      range?: boolean;
      toggle?: boolean;
    }>
  | Readonly<{ kind: "grid-select-mode"; mode: boolean }>
  | Readonly<{ kind: "grid-multi-clear" }>
  | Readonly<{ kind: "grid-batch-mutation"; value: ViewSelectionState }>
  | Readonly<{ kind: "grid-batch-album-add"; albumId: string }>
  | Readonly<{
      kind:
        | "show-grid"
        | "library-check"
        | "refresh"
        | "retry-source"
        | "retry-photo"
        | "previous"
        | "next"
        | "undo";
    }>
  | Readonly<{
      kind: "photo-mutation";
      field: "selectionState" | "rating";
      value: ViewSelectionState | number;
      advance: boolean;
    }>
  | Readonly<{
      kind: "grid-photo-mutation";
      index: number;
      field: "selectionState" | "rating";
      value: ViewSelectionState | number;
    }>
  | Readonly<{ kind: "membership-toggle"; albumId: string; member: boolean }>
  | Readonly<{ kind: "membership-retry" }>;

type GridThumbnailBinding = Readonly<{
  photoId: string;
  preview: GridPhotoViewModel["preview"];
  target: GridThumbnailTarget;
}>;

interface GridThumbnailTarget {
  readonly complete: boolean;
  readonly isConnected: boolean;
  src: string;
  onload: GlobalEventHandlers["onload"];
  onerror: GlobalEventHandlers["onerror"];
  removeAttribute(name: string): void;
  setDeliveryFailed(failed: boolean): void;
}

interface ReviewImageTarget {
  readonly connected: boolean;
  readonly source: string;
  setHandlers(onLoad: () => void, onError: () => void): void;
  clearHandlers(): void;
  setSource(resolvedUrl: string): void;
  clearSource(): void;
}

type ReviewImagePresentation = Readonly<{
  target: ReviewImageTarget;
  resolvedUrl: string;
  surface: object;
}>;

export type FolderViewModel = Readonly<{
  location: string;
  name: string;
  photoCount: number;
  hasDescendantFolders: boolean;
  expanded: boolean;
  enabled: boolean;
  active: boolean;
  children: ReadonlyArray<FolderViewModel>;
  pager?: Readonly<{
    page: number;
    pages: number;
    hasPrevious: boolean;
    hasNext: boolean;
  }>;
}>;

export type SourceListViewModel = Readonly<{
  libraryCount: number;
  libraryActive: boolean;
  fileLocationsEnabled: boolean;
  fileLocationFailures: ReadonlyArray<Readonly<{ key: string; range: string }>>;
  rootExpanded: boolean;
  rootActive: boolean;
  rootChildren: ReadonlyArray<FolderViewModel>;
  rootPager?: FolderViewModel["pager"];
  albums: ReadonlyArray<
    Readonly<{
      id: string;
      name: string;
      photoCount: number;
      hasSavedPosition: boolean;
      active: boolean;
    }>
  >;
}>;

export type FolderAlbumViewModel = Readonly<{
  visible: boolean;
  folderPath: string;
  albums: ReadonlyArray<Readonly<{ id: string; name: string }>>;
  selectedAlbumId: string;
  pending: boolean;
  status?: string;
}>;

type GridPhotoViewModel = Readonly<{
  id: string;
  available: boolean;
  ambiguous: boolean;
  originalFilename?: string;
  selectionState: ViewSelectionState;
  rating: number;
  preview: Readonly<{
    state: "inspection-pending" | "ready" | "unavailable" | "failed";
    thumbnailUrl?: string;
  }>;
}>;

type GridViewModel = Readonly<{
  total: number;
  /// The Grid's multi-selection: whether every cell activation toggles its
  /// Photo, how many Photos are multi-selected, the bound one batch may
  /// address, and whether a batch action would be admitted now. `selected` is
  /// asked per rendered index, so the Grid presents exactly the Photos the
  /// page model holds.
  multi: Readonly<{
    mode: boolean;
    count: number;
    limit: number;
    enabled: boolean;
    selected(index: number): boolean;
  }>;
  photoAt(index: number): GridPhotoViewModel | undefined;
}>;

/// One rendered Grid cell. The signature covers everything the cell presents,
/// so a merged render rebuilds only the cells whose Photo facts or delivery
/// state changed and leaves every other button and its image in place.
type RenderedGridCell = {
  readonly cell: HTMLButtonElement;
  signature: string;
  deliveryFailed: boolean;
  /// The thumbnail ownership this cell holds while it presents a Photo. A
  /// cell that leaves the rendered range or is rebuilt hands it back so the
  /// owner's image state follows the rendered Grid.
  thumbnail: GridThumbnailBinding | undefined;
};

/// Placeholder cells present no Photo: they never initiate loading.
const LOADING_CELL_SIGNATURE = "loading";

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

type PhotoFactsViewModel = Readonly<{
  index: number;
  total: number;
  originalFilename?: string | undefined;
  selectionState?: ViewSelectionState | undefined;
  rating?: number | undefined;
}>;

type PhotoMetadataViewModel = Readonly<{
  captureTime?: string;
  aperture?: string;
  iso?: number;
  shutterSpeed?: string;
  focalLength?: string;
}>;

/// One bounded neighbor entry. `photo` is absent while its facts are still
/// loading, so the entry renders a quiet placeholder instead of guessing.
type FilmstripCellViewModel = Readonly<{
  index: number;
  current: boolean;
  photo: GridPhotoViewModel | undefined;
}>;

type FilmstripViewModel = Readonly<{
  total: number;
  /// True while activating an entry would open its Photo. Every real entry is
  /// disabled otherwise, so the strip never offers an activation that would
  /// be refused silently.
  interactive: boolean;
  cells: ReadonlyArray<FilmstripCellViewModel>;
}>;

type PhotoShellViewModel = PhotoFactsViewModel &
  Readonly<{
    sourceName: string;
    photoId?: string | undefined;
    available?: boolean | undefined;
    previewSource?: ViewPreviewSource | undefined;
    limitedDetail?: boolean | undefined;
    previewUrl?: string | undefined;
  }>;

type MembershipAlbumViewModel = Readonly<{ id: string; name: string }>;

type MembershipViewModel = Readonly<{
  photoPresent: boolean;
  loading: boolean;
  failed: boolean;
  message?: string;
  containing: ReadonlyArray<MembershipAlbumViewModel>;
  options: ReadonlyArray<
    Readonly<{ id: string; name: string; member: boolean }>
  >;
  pendingAlbumIds: ReadonlyArray<string>;
}>;

type ControlsViewModel = Readonly<{
  gridEnabled: boolean;
  filmstripEnabled: boolean;
  decisionEnabled: boolean;
  clearEnabled: boolean;
  backEnabled: boolean;
  refreshEnabled: boolean;
  recoveryEnabled: boolean;
  previousEnabled: boolean;
  nextEnabled: boolean;
  undoEnabled: boolean;
}>;

/// The batch bar's Album choices. `pending` covers one Add to Album settling
/// and `status` reports its outcome beside the control.
export type BatchAlbumsViewModel = Readonly<{
  albums: ReadonlyArray<Readonly<{ id: string; name: string }>>;
  pending: boolean;
}>;

export interface LibraryBrowserView {
  readonly photoStatusSurface: object;
  readonly photoStatusEmpty: boolean;
  isPhotoStatusSurfaceCurrent(surface: object): boolean;
  setPhotoStatus(text: string): void;
  presentSummary(
    text: string,
    action?: Readonly<{
      kind: "retry-library-check" | "refresh-current-source";
      presentationId: number;
    }>,
    libraryCheckState?: "idle" | "active" | "failed" | "complete",
  ): void;
  setConnection(
    connected: boolean,
    sourceRetryVisible: boolean,
    photoRetryVisible: boolean,
  ): void;
  setSourceTitle(name: string): void;
  setGridStatus(text: string): void;
  setGridEmpty(text?: string, libraryCheck?: boolean): void;
  renderSources(model: SourceListViewModel): void;
  renderFolderAlbum(model: FolderAlbumViewModel): void;
  renderSort(model: GridSortViewModel): void;
  renderFilter(model: GridFilterViewModel): void;
  renderProgress(model: GridProgressViewModel): void;
  setControls(model: ControlsViewModel): void;
  renderMembership(model: MembershipViewModel): void;
  prepareSourceOpen(name: string): void;
  renderGrid(model: GridViewModel, position?: number): void;
  /// Presents the multi-selection the page model has just emptied: the batch
  /// bar hides and the retained cells drop their markers. A failed source open
  /// or reopen calls it instead of a render, so a bar can never name Photos
  /// the Grid no longer holds.
  resetGridMultiSelection(): void;
  /// Presents the batch bar's Album list and its pending state. The list is
  /// the bounded Album summary the Sources panel already presents.
  renderBatchAlbums(model: BatchAlbumsViewModel): void;
  scheduleGridRender(): void;
  cancelGridRender(): void;
  clearGridCells(): void;
  /// Builds the retained cells whose image the owner detached at a Grid
  /// boundary again, so a reopen that detached them mid-flight does not leave
  /// blank cells behind. It presents no Photo facts of its own: the retained
  /// cells already hold them, and every rebuilt cell reads the Grid's current
  /// multi-selection presentation.
  rebindDetachedGridCells(
    model: Readonly<{
      total: number;
      photoAt(index: number): GridPhotoViewModel | undefined;
    }>,
  ): void;
  gridVisible(): boolean;
  scrollToGridIndex(index: number): void;
  /// Moves the Grid keyboard to one Photo. Used when Undo restores a Grid
  /// decision and must return the Photographer to the affected Photo.
  focusGridIndex(index: number): void;
  showGrid(index?: number): void;
  enterPhoto(): void;
  /// Presents the bounded neighbor entries around the current Photo. The
  /// strip rebuilds only the entries whose facts or delivery state changed,
  /// so navigating keeps every other entry's thumbnail in place.
  renderFilmstrip(model: FilmstripViewModel): void;
  renderPhotoFacts(model: PhotoFactsViewModel): void;
  renderPhotoMetadata(model?: PhotoMetadataViewModel): void;
  renderPhotoShell(
    model: PhotoShellViewModel,
  ): ReviewImagePresentation | undefined;
  presentReviewImage(
    url: string,
    index: number,
    total: number,
  ): ReviewImagePresentation | undefined;
  reviewImageMatches(url: string): boolean;
  showPreviewUnavailable(text: string): void;
  setPreviewFacts(
    source: ViewPreviewSource | undefined,
    limited: boolean,
  ): void;
  setAlbumFormMessage(formId: string, message: string): void;
  setAlbumFormPending(formId: string, pending: boolean, name?: string): void;
  dismissAlbumForm(formId: string): void;
  dispose(): void;
}

type AlbumFormState = {
  kind: "create" | "rename" | "delete";
  formId: string;
  albumId?: string;
  name: string;
  returnFocusKey: string;
  pending: boolean;
  message?: string;
};

type AlbumFocusRequest =
  | Readonly<{ kind: "form"; formId: string }>
  | Readonly<{ kind: "return"; focusKey: string }>;

export function createLibraryBrowserView(
  root: HTMLElement,
  emit: (intent: LibraryBrowserIntent) => void,
  bindThumbnail: (binding: GridThumbnailBinding) => void,
  releaseThumbnail: (binding: GridThumbnailBinding) => void,
): LibraryBrowserView {
  let alive = true;
  root.innerHTML = `
    <div class="app-shell">
      <header class="app-header"><h1>Slipstream</h1><p data-connection role="status">Connecting…</p></header>
      <section class="browser" data-browser aria-labelledby="browser-title">
        <div class="source-scrim" data-source-scrim aria-hidden="true"></div>
        <nav class="source-panel" id="source-panel" data-library-screen aria-label="Library sources">
          <header class="source-header"><h2 id="browser-title">Sources</h2><button type="button" class="quiet source-close" data-source-close>Close</button></header>
          <p data-summary-status role="status">Loading Library…</p>
          <div class="source-list" data-source-list></div>
          <footer class="source-footer"><button type="button" data-refresh>Refresh Source</button><button type="button" data-retry hidden>Retry connection</button></footer>
        </nav>
        <div class="source-resizer" data-source-resizer role="separator" aria-label="Resize sources" aria-orientation="vertical" tabindex="0"></div>
        <section class="grid-view" data-grid-view aria-labelledby="grid-title">
          <header class="grid-header"><button type="button" class="quiet source-toggle" data-source-toggle aria-controls="source-panel" aria-expanded="false">Sources</button><div class="grid-heading"><h2 id="grid-title" data-grid-title>All Photos</h2><p data-grid-status role="status"></p></div><p class="grid-progress" data-grid-progress hidden></p><div class="grid-filter" data-grid-filter hidden><label for="grid-filter-select">Show</label><select id="grid-filter-select" data-filter-select></select></div><div class="grid-size" data-grid-size><label for="grid-size-select">Size</label><select id="grid-size-select" data-size-select></select></div><div class="grid-sort" data-grid-sort hidden><label for="grid-sort-select">Sort</label><select id="grid-sort-select" data-sort-select></select></div><div class="grid-select-mode"><button type="button" class="quiet" data-grid-select-mode aria-pressed="false">Select mode</button></div><div class="folder-album-controls" data-folder-album-controls hidden><label for="folder-album-select">Add Folder to</label><select id="folder-album-select" data-folder-album-select></select><button type="button" data-add-folder-to-album>Add Folder</button><p data-folder-album-status role="status" aria-live="polite"></p></div><p class="grid-summary" data-grid-summary role="status" aria-live="polite"></p><div class="grid-batch" data-grid-batch hidden><p class="grid-batch-count" data-batch-count role="status"></p><div class="grid-batch-actions" role="group" aria-label="Batch actions"><button type="button" data-batch-select>Select</button><button type="button" data-batch-reject>Reject</button><label for="batch-album-select">Add to</label><select id="batch-album-select" data-batch-album-select></select><button type="button" data-batch-album-add>Add to Album</button><button type="button" class="quiet" data-batch-clear>Clear</button></div></div></header>
          <div class="grid-viewport" data-grid-viewport tabindex="0" aria-label="Photo Library Grid"><div class="grid-canvas" data-grid-canvas></div><div class="grid-layer" data-grid-layer></div><div class="grid-empty" data-grid-empty hidden><p data-grid-empty-message role="status"></p><button type="button" data-grid-empty-action hidden>Check Library</button></div></div>
        </section>
        <section class="photo-view" data-review data-photo-view hidden tabindex="-1" aria-labelledby="photo-title">
          <header class="photo-header"><button type="button" class="quiet" data-back>Back to Grid</button><div><h2 id="photo-title" data-photo-title>Photo</h2><p data-position>0 / 0</p></div><div class="photo-header-actions"><button type="button" class="quiet photo-source-toggle" data-photo-source-toggle aria-controls="source-panel" aria-expanded="false">Sources</button><button type="button" class="quiet" data-retry-photo hidden>Retry</button></div></header>
          <section class="preview" data-preview aria-label="Photo Preview">
            <div class="zoom-controls" data-zoom-controls role="group" aria-label="Preview zoom">
              <button type="button" class="zoom-control" data-zoom-fit aria-pressed="true" aria-label="Fit Window">Fit Window</button>
              <button type="button" class="zoom-control zoom-step" data-zoom-out aria-label="Zoom out">−</button>
              <input class="zoom-slider" type="range" data-zoom-slider min="10" max="800" step="1" value="100" aria-label="Zoom percentage" />
              <button type="button" class="zoom-control zoom-step" data-zoom-in aria-label="Zoom in">+</button>
              <span class="zoom-level" data-zoom-level>—</span>
              <button type="button" class="zoom-control" data-zoom-100 aria-label="Zoom to 100 percent">100%</button>
            </div>
            <div class="swipe-feedback reject" data-reject-feedback>Reject</div>
            <div class="image-stage" data-stage><p>Loading Preview…</p></div>
            <div class="swipe-feedback select" data-select-feedback>Select</div>
          </section>
          <div class="filmstrip" data-filmstrip role="group" aria-label="Neighbor Photos" hidden></div>
          <section class="review-bar" aria-label="Photo review">
            <div class="review-state"><dl class="facts"><div><dt>File</dt><dd data-photo-filename>—</dd></div><div><dt>Selection</dt><dd data-selection>Undecided</dd></div><div><dt>Rating</dt><dd data-rating>No rating</dd></div><div><dt>Preview</dt><dd data-source>—</dd></div></dl><div class="metadata" data-metadata aria-label="Capture details"><strong>Details</strong><dl><div><dt>Captured</dt><dd data-metadata-capture-time>—</dd></div><div><dt>Aperture</dt><dd data-metadata-aperture>—</dd></div><div><dt>ISO</dt><dd data-metadata-iso>—</dd></div><div><dt>Shutter</dt><dd data-metadata-shutter-speed>—</dd></div><div><dt>Focal Length</dt><dd data-metadata-focal-length>—</dd></div></dl></div><p class="status" data-status role="status" aria-live="polite"></p></div>
            <div class="decision-controls" aria-label="Selection controls"><button type="button" class="reject-button" data-reject>Reject <span aria-hidden="true">X</span></button><button type="button" class="quiet" data-clear>Clear <span aria-hidden="true">U</span></button><button type="button" class="select-button" data-select>Select <span aria-hidden="true">P</span></button></div>
          </section>
          <section class="review-tools" aria-label="Review tools">
            <fieldset class="rating-controls"><legend>Rating</legend><div data-ratings></div></fieldset>
            <div class="membership" data-membership aria-label="Album membership"><div class="membership-facts"><p class="membership-heading">Albums</p><p class="membership-status" data-membership-status role="status">Loading Albums…</p><ul class="membership-list" data-membership-list hidden></ul><p class="membership-message" data-membership-message role="alert" hidden></p><div class="membership-actions"><button type="button" class="quiet" data-membership-manage aria-expanded="false" aria-controls="membership-panel">Manage</button><button type="button" data-membership-retry hidden>Retry Albums</button></div></div><div class="membership-panel" id="membership-panel" data-membership-panel hidden><div class="membership-options" data-membership-options></div></div></div>
            <div class="photo-controls"><button type="button" class="quiet" data-previous>Previous</button><button type="button" class="quiet" data-undo disabled>Undo</button><button type="button" class="quiet" data-next>Next</button></div>
          </section>
        </section>
      </section>
    </div>`;

  const browser = required<HTMLElement>(root, "[data-browser]");
  const sourcePanel = required<HTMLElement>(root, "#source-panel");
  const sourceResizer = required<HTMLElement>(root, "[data-source-resizer]");
  const sourceToggle = required<HTMLButtonElement>(
    root,
    "[data-source-toggle]",
  );
  const photoSourceToggle = required<HTMLButtonElement>(
    root,
    "[data-photo-source-toggle]",
  );
  const sourceClose = required<HTMLButtonElement>(root, "[data-source-close]");
  const sourceScrim = required<HTMLElement>(root, "[data-source-scrim]");
  const connection = required<HTMLElement>(root, "[data-connection]");
  const summaryStatus = required<HTMLElement>(root, "[data-summary-status]");
  const sourceList = required<HTMLElement>(root, "[data-source-list]");
  const retry = required<HTMLButtonElement>(root, "[data-retry]");
  const refresh = required<HTMLButtonElement>(root, "[data-refresh]");
  const gridView = required<HTMLElement>(root, "[data-grid-view]");
  const gridTitle = required<HTMLElement>(root, "[data-grid-title]");
  const gridStatus = required<HTMLElement>(root, "[data-grid-status]");
  const gridSelectMode = required<HTMLButtonElement>(
    root,
    "[data-grid-select-mode]",
  );
  const gridBatch = required<HTMLElement>(root, "[data-grid-batch]");
  const batchCount = required<HTMLElement>(root, "[data-batch-count]");
  const batchSelect = required<HTMLButtonElement>(root, "[data-batch-select]");
  const batchReject = required<HTMLButtonElement>(root, "[data-batch-reject]");
  const batchAlbumSelect = required<HTMLSelectElement>(
    root,
    "[data-batch-album-select]",
  );
  const batchAlbumAdd = required<HTMLButtonElement>(
    root,
    "[data-batch-album-add]",
  );
  const batchClear = required<HTMLButtonElement>(root, "[data-batch-clear]");
  const folderAlbumControls = required<HTMLElement>(
    root,
    "[data-folder-album-controls]",
  );
  const folderAlbumSelect = required<HTMLSelectElement>(
    root,
    "[data-folder-album-select]",
  );
  const addFolderToAlbum = required<HTMLButtonElement>(
    root,
    "[data-add-folder-to-album]",
  );
  const folderAlbumStatus = required<HTMLElement>(
    root,
    "[data-folder-album-status]",
  );
  const gridSummary = required<HTMLElement>(root, "[data-grid-summary]");
  const gridSort = required<HTMLElement>(root, "[data-grid-sort]");
  const sortSelect = required<HTMLSelectElement>(root, "[data-sort-select]");
  const gridProgress = required<HTMLElement>(root, "[data-grid-progress]");
  const gridFilter = required<HTMLElement>(root, "[data-grid-filter]");
  const filterSelect = required<HTMLSelectElement>(
    root,
    "[data-filter-select]",
  );
  const sizeSelect = required<HTMLSelectElement>(root, "[data-size-select]");
  const gridViewport = required<HTMLElement>(root, "[data-grid-viewport]");
  const gridCanvas = required<HTMLElement>(root, "[data-grid-canvas]");
  const gridLayer = required<HTMLElement>(root, "[data-grid-layer]");
  const gridEmpty = required<HTMLElement>(root, "[data-grid-empty]");
  const gridEmptyMessage = required<HTMLElement>(
    root,
    "[data-grid-empty-message]",
  );
  const gridEmptyAction = required<HTMLButtonElement>(
    root,
    "[data-grid-empty-action]",
  );
  const photoView = required<HTMLElement>(root, "[data-photo-view]");
  const photoTitle = required<HTMLElement>(root, "[data-photo-title]");
  const position = required<HTMLElement>(root, "[data-position]");
  const stage = required<HTMLElement>(root, "[data-stage]");
  const preview = required<HTMLElement>(root, "[data-preview]");
  const zoomControls = required<HTMLElement>(root, "[data-zoom-controls]");
  const zoomFit = required<HTMLButtonElement>(root, "[data-zoom-fit]");
  const zoomOut = required<HTMLButtonElement>(root, "[data-zoom-out]");
  const zoomIn = required<HTMLButtonElement>(root, "[data-zoom-in]");
  const zoomSlider = required<HTMLInputElement>(root, "[data-zoom-slider]");
  const zoomLevel = required<HTMLElement>(root, "[data-zoom-level]");
  const zoom100 = required<HTMLButtonElement>(root, "[data-zoom-100]");
  const selection = required<HTMLElement>(root, "[data-selection]");
  const filmstrip = required<HTMLElement>(root, "[data-filmstrip]");
  const photoFilename = required<HTMLElement>(root, "[data-photo-filename]");
  const rating = required<HTMLElement>(root, "[data-rating]");
  const previewSource = required<HTMLElement>(root, "[data-source]");
  const metadataCaptureTime = required<HTMLElement>(
    root,
    "[data-metadata-capture-time]",
  );
  const metadataAperture = required<HTMLElement>(
    root,
    "[data-metadata-aperture]",
  );
  const metadataIso = required<HTMLElement>(root, "[data-metadata-iso]");
  const metadataShutterSpeed = required<HTMLElement>(
    root,
    "[data-metadata-shutter-speed]",
  );
  const metadataFocalLength = required<HTMLElement>(
    root,
    "[data-metadata-focal-length]",
  );
  const status = required<HTMLElement>(root, "[data-status]");
  const retryPhoto = required<HTMLButtonElement>(root, "[data-retry-photo]");
  const back = required<HTMLButtonElement>(root, "[data-back]");
  const previous = required<HTMLButtonElement>(root, "[data-previous]");
  const next = required<HTMLButtonElement>(root, "[data-next]");
  const select = required<HTMLButtonElement>(root, "[data-select]");
  const membershipStatus = required<HTMLElement>(
    root,
    "[data-membership-status]",
  );
  const membershipList = required<HTMLElement>(root, "[data-membership-list]");
  const membershipMessage = required<HTMLElement>(
    root,
    "[data-membership-message]",
  );
  const membershipManage = required<HTMLButtonElement>(
    root,
    "[data-membership-manage]",
  );
  const membershipRetry = required<HTMLButtonElement>(
    root,
    "[data-membership-retry]",
  );
  const membershipPanel = required<HTMLElement>(
    root,
    "[data-membership-panel]",
  );
  const membershipOptions = required<HTMLElement>(
    root,
    "[data-membership-options]",
  );
  const reject = required<HTMLButtonElement>(root, "[data-reject]");
  const clear = required<HTMLButtonElement>(root, "[data-clear]");
  const undo = required<HTMLButtonElement>(root, "[data-undo]");
  const ratings = required<HTMLElement>(root, "[data-ratings]");
  const selectFeedback = required<HTMLElement>(root, "[data-select-feedback]");
  const rejectFeedback = required<HTMLElement>(root, "[data-reject-feedback]");

  const send = (intent: LibraryBrowserIntent): void => {
    if (alive) emit(intent);
  };

  for (let value = 0; value <= 5; value += 1) {
    const button = document.createElement("button");
    button.type = "button";
    button.dataset.ratingValue = String(value);
    button.setAttribute(
      "aria-label",
      value === 0
        ? "Clear Rating, 0 stars"
        : `Rate ${value} ${value === 1 ? "star" : "stars"}`,
    );
    button.setAttribute("aria-pressed", String(value === 0));
    button.textContent = value === 0 ? "0" : `${value}★`;
    ratings.append(button);
  }

  let photoStatusSurface: object = {};
  let sourceReturn: "grid" | "photo" = "grid";
  let sourceModel: SourceListViewModel | undefined;
  let membershipModel: MembershipViewModel | undefined;
  let albumFormCounter = 0;
  let albumForm: AlbumFormState | undefined;
  let albumFocusRequest: AlbumFocusRequest | undefined;
  let gridKeyboardIndex: number | undefined;
  let gridTotal = 0;
  let thumbnailSize: GridThumbnailSize = DEFAULT_GRID_THUMBNAIL_SIZE;
  // The Photo whose row a pending size change keeps as the first visible row;
  // the render that applies the new pitch consumes it.
  let pendingGridAnchor: number | undefined;
  let renderedColumns = 0;
  let renderedColumnStride = 0;
  let renderedViewportHeight = 0;
  let gridRenderFrame: number | undefined;
  const renderedCells = new Map<number, RenderedGridCell>();
  const renderedFilmstripCells = new Map<number, RenderedFilmstripCell>();
  // The range the Grid last reported for admission. A render reports a
  // changed range, or the same range again while part of it has no Photo.
  let reportedGridRange: Readonly<{ start: number; end: number }> | undefined;
  let membershipManageOpen = false;
  let membershipFocusAlbumId: string | undefined;
  let folderAlbumSelection = "";
  let batchAlbumSelection = "";
  // The Grid's multi-selection presentation: the page model owns which Photos
  // are multi-selected, and these mirror the last rendered model so a cell
  // build, a keyboard key, and a merged render all read one state.
  let gridMultiMode = false;
  let gridMultiCount = 0;
  let gridMultiLimit = 0;
  let gridMultiEnabled = false;
  let gridMultiSelected: (index: number) => boolean = () => false;
  /// Presents the multi-selection the page model has just emptied, or one
  /// whose bound the page model reports. A hidden bar clears the markers too,
  /// so a cell never keeps a marker the bar no longer names, and a hidden Grid
  /// keeps its retained DOM: only the visible Grid touches it.
  const resetGridMultiSelection = () => {
    if (!alive) return;
    gridMultiMode = false;
    gridMultiCount = 0;
    gridMultiEnabled = false;
    gridMultiSelected = () => false;
    if (gridView.hidden) return;
    renderBatch();
    applyGridMultiSelection();
  };
  // Whether one batch Add to Album is settling: its control stays disabled
  // until the outcome is presented.
  let batchAlbumsPending = false;
  /// The batch-bar control that held keyboard focus when a settling batch
  /// disabled the bar; parked and returned by renderBatch.
  let heldBatchControl: HTMLButtonElement | HTMLSelectElement | null = null;
  // The Album choices the batch bar presents. A hidden bar binds no options,
  // so an option list never answers a query for a surface the Grid is not
  // presenting.
  let batchAlbums: ReadonlyArray<Readonly<{ id: string; name: string }>> = [];
  let renderedBatchAlbumSignature = "";
  const MIN_ZOOM_PERCENT = 10;
  const MAX_ZOOM_PERCENT = 800;
  const ZOOM_STEP = 1.25;
  const DETAIL_ZOOM_PERCENT = 200;
  // Reported while no Preview image has measurable pixels, so the live
  // percentage never claims a value the stage cannot show.
  const ZOOM_LEVEL_UNMEASURED = "—";
  // Preview zoom is presentation state: `zoomManual` distinguishes Fit
  // from a manual percentage anchored to the Preview's own pixels, so
  // `zoomPercent` maps 1 image pixel to `zoomPercent / 100` CSS pixels.
  let zoomManual = false;
  let zoomPercent = 100;
  let renderedSortKind: ViewSourceKind | undefined;
  let panX = 0;
  let panY = 0;
  let imageNaturalWidth = 0;
  let imageNaturalHeight = 0;
  const activePointers = new Map<number, { x: number; y: number }>();
  type PinchGesture = Readonly<{
    startPercent: number;
    startScale: number;
    startDistance: number;
    offsetX: number;
    offsetY: number;
  }>;
  let pinch: PinchGesture | undefined;
  let photoSurface: object = {};
  let currentPhotoId: string | undefined;
  let currentSelection: ViewSelectionState = "undecided";
  let filterRendered = false;
  let renderedProgressText = "";
  let gridInteractionEnabled = false;
  let decisionInteractionEnabled = false;
  let pointer:
    | {
        id: number;
        startX: number;
        startY: number;
        lastX: number;
        lastY: number;
        startedAt: number;
        vertical: boolean;
        // The zoom mode the gesture started in; it owns the drag until
        // release even if Fit returns mid-gesture.
        pan: boolean;
        surface: object;
        photoId: string;
      }
    | undefined;
  let sourceWidth = 224;
  const setSourceWidth = (width: number) => {
    sourceWidth = clamp(width, 176, 360);
    browser.style.setProperty("--source-width", `${sourceWidth}px`);
  };
  let resizing = false;
  sourceResizer.addEventListener("pointerdown", (event) => {
    if (compactSources.matches) return;
    resizing = true;
    sourceResizer.setPointerCapture(event.pointerId);
    sourceResizer.classList.add("active");
  });
  sourceResizer.addEventListener("pointermove", (event) => {
    if (resizing)
      setSourceWidth(event.clientX - browser.getBoundingClientRect().left);
  });
  const stopResize = () => {
    resizing = false;
    sourceResizer.classList.remove("active");
  };
  sourceResizer.addEventListener("pointerup", stopResize);
  sourceResizer.addEventListener("pointercancel", stopResize);
  sourceResizer.addEventListener("keydown", (event) => {
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      setSourceWidth(sourceWidth + (event.key === "ArrowRight" ? 16 : -16));
    }
  });
  setSourceWidth(sourceWidth);

  const compactSources = window.matchMedia("(max-width: 760px)");
  // The CSS yields the strip on a short viewport so the Preview and the
  // decision controls keep their space; the view mirrors that condition so a
  // hidden strip binds no thumbnails and rebuilds when the space returns.
  const shortViewport = window.matchMedia("(max-height: 480px)");
  const syncSourcePanel = () => {
    const drawerMode = compactSources.matches || !photoView.hidden;
    const drawerOpen = drawerMode && browser.classList.contains("sources-open");
    const concealed = drawerMode && !drawerOpen;
    sourcePanel.inert = concealed;
    sourcePanel.setAttribute("aria-hidden", String(concealed));
    gridView.inert = drawerOpen;
    photoView.inert = drawerOpen;
  };
  const setSourcesExpanded = (expanded: boolean) => {
    sourceToggle.setAttribute("aria-expanded", String(expanded));
    photoSourceToggle.setAttribute("aria-expanded", String(expanded));
  };
  const openSources = (returnTo: "grid" | "photo") => {
    if (!alive) return;
    sourceReturn = returnTo;
    browser.classList.add("sources-open");
    setSourcesExpanded(true);
    syncSourcePanel();
    sourceClose.focus();
  };
  const closeSources = (restoreFocus = true) => {
    if (!alive) return;
    browser.classList.remove("sources-open");
    setSourcesExpanded(false);
    syncSourcePanel();
    if (restoreFocus)
      (sourceReturn === "grid" ? sourceToggle : photoSourceToggle).focus();
  };
  const onSourceViewportChange = () => {
    if (!alive) return;
    browser.classList.remove("sources-open");
    setSourcesExpanded(false);
    syncSourcePanel();
  };
  const cellBox = () => GRID_THUMBNAIL_SIZE_STEPS[thumbnailSize];
  /// The column and row pitch of the virtualized layout: one cell box plus
  /// the ordinary inter-cell gap. Every geometry calculation derives from
  /// these, so the CSS cell box and the layout can never disagree.
  const columnPitch = () => cellBox().width + GRID_CELL_GAP_X;
  const rowPitch = () => cellBox().height + GRID_CELL_GAP_Y;
  const applyGridThumbnailSize = () => {
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
  applyGridThumbnailSize();
  /// Re-lays out the Grid at another thumbnail size. The size is presentation
  /// state of the open Grid: it changes cell geometry only, keeps the row the
  /// Photographer was looking at as the first visible row, and reports the
  /// new range through the merged render, so window admission follows exactly
  /// as it does for scrolling.
  const setGridThumbnailSize = (size: GridThumbnailSize) => {
    if (!alive || size === thumbnailSize) return;
    // The anchor is applied by the render that lays the Grid out at the new
    // pitch: scrolling before that render would clamp against the previous
    // canvas height.
    pendingGridAnchor = firstVisibleGridIndex(columns());
    thumbnailSize = size;
    applyGridThumbnailSize();
    scheduleGridRender();
  };
  const columns = () =>
    Math.max(
      1,
      Math.floor(Math.max(320, gridViewport.clientWidth) / columnPitch()),
    );
  const columnStride = (count = columns()) =>
    compactSources.matches ? gridViewport.clientWidth / count : columnPitch();
  const effectiveViewportHeight = () =>
    Math.max(360, Math.min(gridViewport.clientHeight, window.innerHeight));

  const clearPointer = () => {
    const id = pointer?.id;
    pointer = undefined;
    if (id !== undefined && preview.hasPointerCapture(id))
      preview.releasePointerCapture(id);
    stage.style.transform = "";
    selectFeedback.classList.remove("pending");
    rejectFeedback.classList.remove("pending");
  };
  const resetGestures = () => {
    for (const id of Array.from(activePointers.keys()))
      if (preview.hasPointerCapture(id)) preview.releasePointerCapture(id);
    activePointers.clear();
    pinch = undefined;
    preview.style.removeProperty("touch-action");
    clearPointer();
  };
  const stageCenter = () => {
    const box = stage.getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
  };
  const fitScale = () => {
    if (!imageNaturalWidth || !imageNaturalHeight) return 0;
    const width = stage.clientWidth;
    const height = stage.clientHeight;
    if (!width || !height) return 0;
    return Math.min(width / imageNaturalWidth, height / imageNaturalHeight);
  };
  const currentScale = () => (zoomManual ? zoomPercent / 100 : fitScale());
  const currentPercent = () => {
    const scale = currentScale();
    return scale > 0 ? scale * 100 : zoomPercent;
  };
  const panLimitX = () =>
    Math.max(0, (imageNaturalWidth * currentScale() - stage.clientWidth) / 2);
  const panLimitY = () =>
    Math.max(0, (imageNaturalHeight * currentScale() - stage.clientHeight) / 2);
  const clampPan = () => {
    panX = clamp(panX, -panLimitX(), panLimitX());
    panY = clamp(panY, -panLimitY(), panLimitY());
  };
  // Zoom acts on the Preview's own pixels. A retained Preview can be shown
  // again without a new load event, so the size is measured from the image
  // element whenever the cached measurement is missing, and a Preview whose
  // bytes never arrived has no pixels to measure at all.
  const loadedPreviewImage = () => {
    const image = stage.querySelector<HTMLImageElement>("img");
    if (!image || !image.complete || image.naturalWidth === 0) return undefined;
    if (!imageNaturalWidth || !imageNaturalHeight) {
      imageNaturalWidth = image.naturalWidth;
      imageNaturalHeight = image.naturalHeight;
    }
    return image;
  };
  const measurableImage = () => Boolean(loadedPreviewImage());
  const syncZoomControls = () => {
    const enabled = alive && measurableImage();
    zoomFit.disabled = !enabled;
    zoomOut.disabled = !enabled;
    zoomIn.disabled = !enabled;
    zoomSlider.disabled = !enabled;
    zoom100.disabled = !enabled;
  };
  const applyZoom = () => {
    if (!alive) return;
    preview.dataset.zoomState = zoomManual ? "manual" : "fit";
    zoomFit.setAttribute("aria-pressed", String(!zoomManual));
    const image = loadedPreviewImage();
    const scale = currentScale();
    if (!image || !imageNaturalWidth || !imageNaturalHeight || scale <= 0) {
      // Without measurable pixels there is no percentage to report: the
      // presentation returns to Fit instead of keeping a stale manual value.
      zoomLevel.textContent = ZOOM_LEVEL_UNMEASURED;
      zoomSlider.value = String(MIN_ZOOM_PERCENT);
      zoomSlider.setAttribute("aria-valuetext", "Fit");
      syncZoomControls();
      return;
    }
    image.style.width = `${imageNaturalWidth * scale}px`;
    image.style.height = `${imageNaturalHeight * scale}px`;
    // The Preview is centered on the stage center, so the stage center is
    // the origin that pan and pointer-anchored zoom are measured from.
    image.style.transform = `translate(${panX}px, ${panY}px)`;
    const percent = Math.round(scale * 100);
    // A Fit can land below the manual floor on a large derivative: the label
    // reports it truthfully while the slider keeps the value it can hold.
    const sliderPercent = clamp(percent, MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT);
    zoomLevel.textContent = `${percent}%`;
    zoomSlider.value = String(sliderPercent);
    zoomSlider.setAttribute("aria-valuetext", `${sliderPercent}%`);
    syncZoomControls();
  };
  const applyFit = () => {
    if (!alive) return;
    zoomManual = false;
    panX = 0;
    panY = 0;
    applyZoom();
  };
  const applyManualZoom = (percent: number) => {
    if (!alive) return;
    zoomManual = true;
    zoomPercent = clamp(percent, MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT);
    clampPan();
    applyZoom();
  };
  const zoomBy = (factor: number) => {
    const current = currentPercent();
    const base = zoomManual ? zoomPercent : Math.max(MIN_ZOOM_PERCENT, current);
    const target = clamp(base * factor, MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT);
    // Stepping stays monotonic: the manual floor cannot magnify a Fit that
    // sits below it by zooming out, and it cannot shrink a 800% manual zoom
    // by zooming in.
    if (factor < 1 ? target >= current : target <= current) return;
    applyManualZoom(target);
  };
  /// Changes zoom while keeping the image point under `anchor` (client
  /// coordinates) stationary, then re-clamps the bounded pan.
  const zoomAt = (percent: number, anchor: { x: number; y: number }) => {
    const nextPercent = clamp(percent, MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT);
    const previousScale = currentScale();
    const center = stageCenter();
    const offsetX = anchor.x - (center.x + panX);
    const offsetY = anchor.y - (center.y + panY);
    if (previousScale > 0 && imageNaturalWidth > 0) {
      const ratio = nextPercent / 100 / previousScale;
      panX = anchor.x - center.x - offsetX * ratio;
      panY = anchor.y - center.y - offsetY * ratio;
    }
    zoomManual = true;
    zoomPercent = nextPercent;
    clampPan();
    applyZoom();
  };
  const resetZoomForImage = () => {
    zoomManual = false;
    zoomPercent = 100;
    panX = 0;
    panY = 0;
    imageNaturalWidth = 0;
    imageNaturalHeight = 0;
    applyZoom();
  };
  const toggleDetail = () => {
    if (!alive || !measurableImage()) return;
    if (zoomManual && Math.abs(zoomPercent - DETAIL_ZOOM_PERCENT) < 0.5)
      applyFit();
    else applyManualZoom(DETAIL_ZOOM_PERCENT);
  };
  const beginPinch = () => {
    const [first, second] = Array.from(activePointers.values());
    clearPointer();
    const scale = currentScale();
    if (!first || !second || scale <= 0 || !imageNaturalWidth) {
      pinch = undefined;
      return;
    }
    const midpoint = {
      x: (first.x + second.x) / 2,
      y: (first.y + second.y) / 2,
    };
    const center = stageCenter();
    pinch = {
      startPercent: currentPercent(),
      startScale: scale,
      startDistance: Math.max(
        1,
        Math.hypot(first.x - second.x, first.y - second.y),
      ),
      offsetX: midpoint.x - (center.x + panX),
      offsetY: midpoint.y - (center.y + panY),
    };
    for (const id of Array.from(activePointers.keys()))
      preview.setPointerCapture(id);
    preview.style.touchAction = "none";
  };
  const updatePinch = () => {
    const active = pinch;
    const [first, second] = Array.from(activePointers.values());
    if (!active || !first || !second) return;
    const midpoint = {
      x: (first.x + second.x) / 2,
      y: (first.y + second.y) / 2,
    };
    const distance = Math.max(
      1,
      Math.hypot(first.x - second.x, first.y - second.y),
    );
    const nextPercent = clamp(
      active.startPercent * (distance / active.startDistance),
      MIN_ZOOM_PERCENT,
      MAX_ZOOM_PERCENT,
    );
    const ratio = nextPercent / 100 / active.startScale;
    const center = stageCenter();
    zoomManual = true;
    zoomPercent = nextPercent;
    panX = midpoint.x - center.x - active.offsetX * ratio;
    panY = midpoint.y - center.y - active.offsetY * ratio;
    clampPan();
    applyZoom();
  };
  const wheelZoom = (event: WheelEvent) => {
    if (!alive || !stage.querySelector("img") || !imageNaturalWidth) return;
    event.preventDefault();
    const factor = event.deltaY < 0 ? ZOOM_STEP : 1 / ZOOM_STEP;
    const base = zoomManual
      ? zoomPercent
      : Math.max(MIN_ZOOM_PERCENT, currentPercent());
    zoomAt(base * factor, { x: event.clientX, y: event.clientY });
  };

  const createSourceButton = (
    name: string,
    count: number,
    active: boolean,
    saved = false,
    disableWhenEmpty = true,
  ) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `source-card${active ? " active" : ""}`;
    if (active) button.setAttribute("aria-current", "true");
    button.disabled = disableWhenEmpty && count === 0;
    // The name may be visually truncated; the title keeps the full name
    // available on hover without changing the accessible name.
    button.title = name;
    button.innerHTML = "<strong></strong><span></span>";
    required<HTMLElement>(button, "strong").textContent = name;
    required<HTMLElement>(button, "span").textContent =
      `${formatPhotoCount(count)}${saved ? " · Resume" : ""}`;
    return button;
  };

  const createFolderPager = (
    location: string,
    pager: NonNullable<FolderViewModel["pager"]>,
  ) => {
    const depth = location ? location.split("/").length : 0;
    const controls = document.createElement("div");
    controls.className = "folder-pager";
    controls.style.marginLeft = `${Math.min(depth, 6) * 12}px`;
    const prior = document.createElement("button");
    prior.type = "button";
    prior.className = "folder-page-button";
    prior.textContent = "Previous Folders";
    prior.disabled = !pager.hasPrevious;
    prior.addEventListener("click", () =>
      send({ kind: "folder-page", location, direction: -1 }),
    );
    const label = document.createElement("span");
    label.className = "folder-page-label";
    label.textContent = `${pager.page + 1} / ${pager.pages}`;
    const more = document.createElement("button");
    more.type = "button";
    more.className = "folder-page-button";
    more.textContent = "More Folders";
    more.disabled = !pager.hasNext;
    more.addEventListener("click", () =>
      send({ kind: "folder-page", location, direction: 1 }),
    );
    controls.append(prior, label, more);
    return controls;
  };

  const appendFolder = (
    fragment: DocumentFragment,
    folder: FolderViewModel,
  ) => {
    const depth = folder.location.split("/").length;
    const row = document.createElement("div");
    row.className = "folder-row folder-child";
    row.style.marginLeft = `${Math.min(depth - 1, 6) * 12}px`;
    if (folder.hasDescendantFolders) {
      const expand = document.createElement("button");
      expand.type = "button";
      expand.className = "folder-expand";
      expand.setAttribute("aria-expanded", String(folder.expanded));
      expand.textContent = folder.expanded ? "▾" : "▸";
      expand.setAttribute("aria-label", `Toggle ${folder.name} subfolders`);
      expand.addEventListener("click", () =>
        send({
          kind: "folder-toggle",
          location: folder.location,
          expanded: folder.expanded,
        }),
      );
      row.append(expand);
    }
    const button = createSourceButton(
      `${folder.name}${folder.hasDescendantFolders ? " · Subfolders" : ""}`,
      folder.photoCount,
      folder.active,
      false,
      false,
    );
    button.disabled = !folder.enabled;
    button.addEventListener("click", () =>
      send({
        kind: "source-open",
        source: {
          kind: "folder",
          location: folder.location,
          name: folder.name,
        },
      }),
    );
    row.append(button);
    fragment.append(row);
    if (!folder.expanded) return;
    for (const child of folder.children) appendFolder(fragment, child);
    if (folder.pager)
      fragment.append(createFolderPager(folder.location, folder.pager));
  };

  const nextAlbumFormId = () => `album-form-${++albumFormCounter}`;
  const albumActionFocusKey = (kind: AlbumFormState["kind"], albumId = "") =>
    kind === "create" ? "album:create" : `album:${kind}:${albumId}`;
  const openAlbumForm = (
    kind: AlbumFormState["kind"],
    albumId = "",
    name = "",
  ) => {
    if (!alive) return;
    albumForm = {
      kind,
      formId: nextAlbumFormId(),
      ...(albumId ? { albumId } : {}),
      name,
      returnFocusKey: albumActionFocusKey(kind, albumId),
      pending: false,
    };
    albumFocusRequest = { kind: "form", formId: albumForm.formId };
    send({ kind: "album-form-open", form: { ...albumForm } });
    if (sourceModel) renderSources(sourceModel);
  };
  const closeAlbumForm = (form: AlbumFormState) => {
    if (!alive || albumForm !== form) return;
    albumFocusRequest = {
      kind: "return",
      focusKey: form.returnFocusKey,
    };
    albumForm = undefined;
    send({ kind: "album-form-close", formId: form.formId });
    if (sourceModel) renderSources(sourceModel);
  };
  const albumFormMessage = () => {
    const message = document.createElement("p");
    message.className = "album-form-message";
    message.setAttribute("role", "alert");
    return message;
  };
  const albumNameInput = (form: AlbumFormState) => {
    const input = document.createElement("input");
    input.type = "text";
    input.name = "name";
    input.dataset.albumFormId = form.formId;
    input.dataset.focusKey = `album:form:${form.formId}:name`;
    input.setAttribute("aria-label", "Album name");
    input.value = form.name;
    input.addEventListener("input", () => {
      if (alive && albumForm === form) {
        form.name = input.value;
        delete form.message;
      }
    });
    return input;
  };
  const createAlbumEditForm = (
    form: AlbumFormState,
    label: string,
    saveText: string,
  ) => {
    const element = document.createElement("form");
    element.className = "album-form";
    element.setAttribute("aria-label", label);
    const input = albumNameInput(form);
    const message = albumFormMessage();
    message.textContent = form.message ?? "";
    const save = document.createElement("button");
    save.type = "submit";
    save.dataset.albumFormId = form.formId;
    save.dataset.focusKey = `album:form:${form.formId}:submit`;
    save.textContent = saveText;
    save.disabled = form.pending;
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.dataset.albumFormId = form.formId;
    cancel.dataset.focusKey = `album:form:${form.formId}:cancel`;
    cancel.textContent = "Cancel";
    cancel.addEventListener("click", () => closeAlbumForm(form));
    element.append(input, save, cancel, message);
    element.addEventListener("submit", (event) => {
      event.preventDefault();
      if (albumForm !== form || form.pending) return;
      send({
        kind: "album-form-submit",
        formId: form.formId,
        name: input.value,
      });
    });
    return element;
  };
  const createAlbumTools = (album: SourceListViewModel["albums"][number]) => {
    const tools = document.createElement("div");
    tools.className = "album-tools";
    if (albumForm?.kind === "rename" && albumForm.albumId === album.id) {
      tools.append(createAlbumEditForm(albumForm, "Rename Album", "Save Name"));
      return tools;
    }
    if (albumForm?.kind === "delete" && albumForm.albumId === album.id) {
      const form = albumForm;
      const confirmBox = document.createElement("div");
      confirmBox.className = "album-confirm";
      confirmBox.setAttribute("role", "alert");
      const text = paragraph("Photos and Original Files remain unchanged.");
      const confirm = document.createElement("button");
      confirm.type = "button";
      confirm.dataset.albumFormId = form.formId;
      confirm.dataset.focusKey = `album:form:${form.formId}:confirm`;
      confirm.textContent = "Delete Album";
      confirm.disabled = form.pending;
      confirm.addEventListener("click", () => {
        if (albumForm === form && !form.pending)
          send({ kind: "album-form-submit", formId: form.formId });
      });
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.dataset.albumFormId = form.formId;
      cancel.dataset.focusKey = `album:form:${form.formId}:cancel`;
      cancel.textContent = "Cancel";
      cancel.addEventListener("click", () => closeAlbumForm(form));
      confirmBox.append(text, confirm, cancel);
      tools.append(confirmBox);
      return tools;
    }
    const rename = document.createElement("button");
    rename.type = "button";
    rename.className = "album-tool";
    rename.textContent = "Rename";
    rename.dataset.focusKey = albumActionFocusKey("rename", album.id);
    rename.setAttribute("aria-label", `Rename ${album.name}`);
    rename.addEventListener("click", () =>
      openAlbumForm("rename", album.id, album.name),
    );
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "album-tool";
    remove.textContent = "Delete";
    remove.dataset.focusKey = albumActionFocusKey("delete", album.id);
    remove.setAttribute("aria-label", `Delete ${album.name}`);
    remove.addEventListener("click", () =>
      openAlbumForm("delete", album.id, album.name),
    );
    tools.append(rename, remove);
    return tools;
  };

  const renderSources = (model: SourceListViewModel) => {
    if (!alive) return;
    sourceModel = model;
    const focused = document.activeElement;
    const focusedFormId =
      focused instanceof HTMLElement ? focused.dataset.albumFormId : undefined;
    const focusedKey =
      focused instanceof HTMLElement ? focused.dataset.focusKey : undefined;
    const focusedSelection =
      focused instanceof HTMLInputElement
        ? [focused.selectionStart, focused.selectionEnd]
        : undefined;
    sourceList.replaceChildren();
    const library = createSourceButton(
      "All Photos",
      model.libraryCount,
      model.libraryActive,
      false,
      false,
    );
    library.dataset.focusKey = "source:library";
    library.addEventListener("click", () =>
      send({ kind: "source-open", source: { kind: "library" } }),
    );
    sourceList.append(library);
    const fileHeading = document.createElement("h3");
    fileHeading.textContent = "Folders";
    sourceList.append(fileHeading);
    for (const failure of model.fileLocationFailures) {
      const retryFolders = document.createElement("button");
      retryFolders.type = "button";
      retryFolders.className = "folder-more";
      retryFolders.textContent = `Retry Folders (${failure.range})`;
      retryFolders.addEventListener("click", () =>
        send({ kind: "file-location-retry", key: failure.key }),
      );
      sourceList.append(retryFolders);
    }
    const rootCard = createSourceButton(
      "Library Folder",
      model.libraryCount,
      model.rootActive,
      false,
      false,
    );
    rootCard.dataset.focusKey = "source:folder:";
    rootCard.disabled = !model.fileLocationsEnabled;
    rootCard.addEventListener("click", () =>
      send({
        kind: "source-open",
        source: { kind: "folder", location: "", name: "Library Folder" },
      }),
    );
    const rootRow = document.createElement("div");
    rootRow.className = "folder-row folder-root";
    const rootExpand = document.createElement("button");
    rootExpand.type = "button";
    rootExpand.className = "folder-expand";
    rootExpand.setAttribute("aria-expanded", String(model.rootExpanded));
    rootExpand.textContent = model.rootExpanded ? "▾" : "▸";
    rootExpand.setAttribute("aria-label", "Toggle Library Folder subfolders");
    rootExpand.addEventListener("click", () =>
      send({
        kind: "folder-toggle",
        location: "",
        expanded: model.rootExpanded,
      }),
    );
    rootRow.append(rootExpand, rootCard);
    const folders = document.createDocumentFragment();
    folders.append(rootRow);
    if (model.rootExpanded) {
      for (const folder of model.rootChildren) appendFolder(folders, folder);
      if (model.rootPager)
        folders.append(createFolderPager("", model.rootPager));
    }
    sourceList.append(folders);
    const albumHeadingRow = document.createElement("div");
    albumHeadingRow.className = "album-heading";
    const albumHeading = document.createElement("h3");
    albumHeading.textContent = "Albums";
    const newAlbum = document.createElement("button");
    newAlbum.type = "button";
    newAlbum.className = "album-new";
    newAlbum.textContent = "New Album";
    newAlbum.dataset.focusKey = albumActionFocusKey("create");
    newAlbum.addEventListener("click", () => openAlbumForm("create"));
    albumHeadingRow.append(albumHeading, newAlbum);
    sourceList.append(albumHeadingRow);
    if (albumForm?.kind === "create")
      sourceList.append(
        createAlbumEditForm(albumForm, "Create Album", "Create Album"),
      );
    for (const album of model.albums) {
      const button = createSourceButton(
        album.name,
        album.photoCount,
        album.active,
        album.hasSavedPosition,
        false,
      );
      button.dataset.focusKey = `source:album:${album.id}`;
      button.addEventListener("click", () =>
        send({ kind: "source-open", source: { kind: "album", id: album.id } }),
      );
      const row = document.createElement("div");
      row.className = "album-row";
      row.append(button, createAlbumTools(album));
      sourceList.append(row);
    }
    const focusTarget = (focusKey: string) =>
      Array.from(
        sourceList.querySelectorAll<HTMLElement>("[data-focus-key]"),
      ).find((candidate) => candidate.dataset.focusKey === focusKey);
    const focusForm = (form: AlbumFormState, selectName: boolean) => {
      const selector =
        form.kind === "delete"
          ? `[data-album-form-id="${form.formId}"][data-focus-key$=":confirm"]`
          : `input[data-album-form-id="${form.formId}"]`;
      let target = sourceList.querySelector<HTMLElement>(selector);
      if (target?.matches(":disabled"))
        target = sourceList.querySelector<HTMLElement>(
          `[data-album-form-id="${form.formId}"][data-focus-key$=":cancel"]`,
        );
      if (!target) return false;
      target.focus();
      if (selectName && target instanceof HTMLInputElement) target.select();
      return document.activeElement === target;
    };
    const request = albumFocusRequest;
    if (request?.kind === "form" && albumForm?.formId === request.formId) {
      albumFocusRequest = undefined;
      focusForm(albumForm, true);
      return;
    }
    if (request?.kind === "return") {
      albumFocusRequest = undefined;
      const target =
        focusTarget(request.focusKey) ??
        focusTarget(albumActionFocusKey("create"));
      target?.focus();
      return;
    }
    const restored = focusedKey ? focusTarget(focusedKey) : undefined;
    if (restored && !restored.matches(":disabled")) {
      restored.focus();
      if (restored instanceof HTMLInputElement) {
        const end = restored.value.length;
        restored.setSelectionRange(
          Math.min(focusedSelection?.[0] ?? end, end),
          Math.min(focusedSelection?.[1] ?? end, end),
        );
      }
      return;
    }
    if (focusedFormId && albumForm?.formId === focusedFormId)
      focusForm(albumForm, false);
  };

  const scheduleGridRender = () => {
    if (!alive || gridRenderFrame !== undefined) return;
    gridRenderFrame = requestAnimationFrame(() => {
      gridRenderFrame = undefined;
      send({ kind: "grid-render" });
    });
  };
  const cancelGridRender = () => {
    if (gridRenderFrame === undefined) return;
    cancelAnimationFrame(gridRenderFrame);
    gridRenderFrame = undefined;
  };
  /// Drops every rendered cell so the next render builds the range from
  /// scratch. The Grid DOM is cleared here when it is rebuilt from nothing:
  /// while a source is replaced, and when Photo View hands the surface back
  /// without the Grid images it detached.
  const clearGridCells = () => {
    for (const rendered of renderedCells.values()) releaseGridCell(rendered);
    renderedCells.clear();
    reportedGridRange = undefined;
    gridLayer.replaceChildren();
  };
  /// Builds the retained cells whose image the owner detached at a Grid
  /// boundary again, in place. Only an image that had not finished loading
  /// loses its source there, and binding the cell anew uses the URL the owner
  /// still holds, so the thumbnails come back without a new request and
  /// without a render: the range, its other cells, and the reported status
  /// stay exactly as the boundary found them.
  const rebindDetachedGridCells = (
    model: Readonly<{
      total: number;
      photoAt(index: number): GridPhotoViewModel | undefined;
    }>,
  ) => {
    if (!alive || gridView.hidden) return;
    const count = columns();
    const stride = columnStride(count);
    for (const [index, rendered] of [...renderedCells]) {
      const image = rendered.cell.querySelector<HTMLImageElement>("img");
      if (!rendered.thumbnail || !image || image.getAttribute("src")) continue;
      const position = rendered.cell.nextSibling;
      releaseGridCell(rendered);
      rendered.cell.remove();
      const rebuilt = buildGridCell(
        index,
        model.photoAt(index),
        model.total,
        count,
        stride,
      );
      gridLayer.insertBefore(rebuilt.cell, position);
      renderedCells.set(index, rebuilt);
    }
  };
  /// Detaches the image of a cell that leaves the rendered range or is rebuilt
  /// in place: an already-started transfer cannot keep owning a connection,
  /// and its late error cannot claim a delivery failure for the Photo. The
  /// cell also hands its thumbnail ownership back to the owner, whose image
  /// state then follows the rendered Grid instead of every Photo a session
  /// rendered.
  const releaseGridCell = (rendered: RenderedGridCell) => {
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
  const positionGridCell = (
    cell: HTMLButtonElement,
    index: number,
    count: number,
    stride: number,
  ) => {
    cell.style.left = `${(index % count) * stride}px`;
    cell.style.top = `${Math.floor(index / count) * rowPitch()}px`;
    cell.style.width = compactSources.matches
      ? `${stride - GRID_CELL_GAP_X}px`
      : "";
  };
  const buildGridCell = (
    index: number,
    photo: GridPhotoViewModel | undefined,
    total: number,
    count: number,
    stride: number,
  ): RenderedGridCell => {
    const cell = document.createElement("button");
    cell.type = "button";
    cell.className = "photo-cell";
    positionGridCell(cell, index, count, stride);
    if (!photo) {
      cell.disabled = true;
      const placeholder = document.createElement("span");
      placeholder.className = "cell-placeholder";
      placeholder.textContent = "Loading…";
      cell.append(placeholder);
      return {
        cell,
        signature: LOADING_CELL_SIGNATURE,
        deliveryFailed: false,
        thumbnail: undefined,
      };
    }
    cell.dataset.photoIndex = String(index);
    cell.disabled = !gridInteractionEnabled;
    // The image keeps its own media area so the complete Photo displays
    // at its true aspect ratio; state, rating, and fact indicators render
    // in the footer beneath it instead of over the image.
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
    // The position number and the Original filename are the visible identity
    // of the cell; a Rating star follows when one is recorded.
    const identity = photo.originalFilename
      ? `${index + 1} · ${photo.originalFilename}`
      : String(index + 1);
    caption.textContent = photo.rating
      ? `${identity} · ${photo.rating}★`
      : identity;
    if (photo.originalFilename) caption.title = photo.originalFilename;
    const facts = document.createElement("span");
    facts.className = "cell-facts";
    const rendered: RenderedGridCell = {
      cell,
      signature: "",
      deliveryFailed: false,
      thumbnail: undefined,
    };
    const presentFacts = () => {
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
    rendered.signature = gridCellSignature(
      index,
      photo,
      rendered.deliveryFailed,
    );
    // Only a recorded decision earns a badge. An empty badge on every
    // undecided cell reads as an unchecked control instead of a fact.
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
    cell.addEventListener("click", (event) =>
      send({
        kind: "open-photo",
        index,
        ...(event.shiftKey ? { range: true } : {}),
        ...(event.ctrlKey || event.metaKey ? { toggle: true } : {}),
      }),
    );
    applyGridCellMulti(cell, index);
    if (alive) {
      const binding: GridThumbnailBinding = {
        photoId: photo.id,
        preview: photo.preview,
        target: gridThumbnailTarget(image, (failed) => {
          rendered.deliveryFailed = failed;
          rendered.signature = gridCellSignature(index, photo, failed);
          presentFacts();
        }),
      };
      rendered.thumbnail = binding;
      bindThumbnail(binding);
    }
    return rendered;
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
  const clearFilmstripCells = () => {
    for (const rendered of renderedFilmstripCells.values())
      releaseFilmstripCell(rendered);
    renderedFilmstripCells.clear();
    filmstrip.replaceChildren();
    filmstrip.hidden = true;
  };
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
  const restoreHeldStripFocus = () => {
    if (heldStripIndex === null || !filmstripInteractive) return;
    const parked =
      document.activeElement === document.body ||
      document.activeElement === photoView;
    const index = heldStripIndex;
    heldStripIndex = null;
    if (!parked) return;
    const entry = filmstrip.querySelector<HTMLButtonElement>(
      `[data-filmstrip-index="${index}"]`,
    );
    if (entry && !entry.disabled) entry.focus();
  };
  const applyFilmstripInteractivity = () => {
    for (const rendered of renderedFilmstripCells.values())
      if (rendered.presentsPhoto)
        rendered.button.disabled = !filmstripInteractive;
  };
  /// Presents one cell's multi-selection. A multi-selected cell carries a
  /// marker that does not depend on color alone, and every cell exposes the
  /// pressed state while Select mode makes its activation toggle.
  const applyGridCellMulti = (cell: HTMLButtonElement, index: number) => {
    const selected = gridMultiSelected(index);
    cell.classList.toggle("multi-selected", selected);
    cell.dataset.multiSelected = String(selected);
    if (selected) cell.setAttribute("aria-pressed", "true");
    else if (gridMultiMode) cell.setAttribute("aria-pressed", "false");
    else cell.removeAttribute("aria-pressed");
  };
  /// Applies the multi-selection to every rendered cell in place. Rebuilding a
  /// cell would restart its Thumbnail transfer, so the marker is patched onto
  /// the cell that already presents the Photo.
  const applyGridMultiSelection = () => {
    for (const [index, rendered] of renderedCells)
      applyGridCellMulti(rendered.cell, index);
  };
  const renderBatch = () => {
    if (!alive) return;
    const count = gridMultiCount;
    gridBatch.hidden = count === 0;
    gridSelectMode.setAttribute("aria-pressed", String(gridMultiMode));
    if (count === 0) {
      batchCount.textContent = "";
      batchAlbumSelect.replaceChildren();
      renderedBatchAlbumSignature = "";
      batchAlbumSelection = "";
      // A hidden bar names no Photo, so no cell keeps a multi-selection
      // marker beside it.
      applyGridMultiSelection();
      return;
    }
    const signature = batchAlbums.map((album) => album.id).join(",");
    if (signature !== renderedBatchAlbumSignature) {
      renderedBatchAlbumSignature = signature;
      if (!batchAlbums.some((album) => album.id === batchAlbumSelection))
        batchAlbumSelection = batchAlbums[0]?.id ?? "";
      batchAlbumSelect.replaceChildren(
        ...batchAlbums.map((album) => {
          const option = document.createElement("option");
          option.value = album.id;
          option.textContent = album.name;
          option.selected = album.id === batchAlbumSelection;
          return option;
        }),
      );
    }
    // The bar names the bound only once the selection reaches it, so the
    // Photographer learns the batch limit before an action is refused.
    batchCount.textContent =
      count >= gridMultiLimit && gridMultiLimit > 0
        ? `${count.toLocaleString()} of ${gridMultiLimit.toLocaleString()} selected`
        : `${count.toLocaleString()} selected`;
    const enabled = gridMultiEnabled && !batchAlbumsPending;
    // Disabling a focused batch control would drop keyboard focus to the
    // body, so the bar parks focus on the Select mode toggle while a batch
    // settles and returns it when interactivity resumes, mirroring the
    // Grid's held-cell hand-off.
    if (!enabled && heldBatchControl === null) {
      const active = document.activeElement;
      if (
        active === batchSelect ||
        active === batchReject ||
        active === batchAlbumSelect ||
        active === batchAlbumAdd
      ) {
        heldBatchControl = active as HTMLButtonElement | HTMLSelectElement;
        gridSelectMode.focus();
      }
    }
    batchSelect.disabled = !enabled;
    batchReject.disabled = !enabled;
    batchClear.disabled = false;
    const albums = batchAlbumSelect.options.length > 0;
    batchAlbumSelect.disabled = !enabled || !albums;
    batchAlbumAdd.disabled = !enabled || !albums || !batchAlbumSelection;
    if (enabled && heldBatchControl) {
      const control = heldBatchControl;
      heldBatchControl = null;
      if (
        control.isConnected &&
        !control.disabled &&
        (document.activeElement === document.body ||
          document.activeElement === gridSelectMode)
      )
        control.focus();
    }
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
      button.setAttribute("aria-label", `Photo ${cell.index + 1} of ${total}`);
      rendered.signature = filmstripCellSignature(cell, total, false);
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
          rendered.signature = filmstripCellSignature(cell, total, failed);
        }),
      };
      rendered.thumbnail = binding;
      bindThumbnail(binding);
    }
    rendered.signature = filmstripCellSignature(cell, total, false);
    return rendered;
  };
  const renderFilmstrip = (model: FilmstripViewModel) => {
    if (!alive || photoView.hidden) return;
    // A strip the CSS hides on a short viewport, or one a source without
    // neighbors hides, must not bind thumbnails either: presenting entries
    // for a hidden surface is the work the strip promises never to start.
    if (shortViewport.matches || model.cells.length <= 1) {
      clearFilmstripCells();
      return;
    }
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
      const signature = filmstripCellSignature(cell, model.total, false);
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
  /// True while the Grid owns keyboard focus, so Grid keys never act while
  /// another surface (the Sources drawer, an Album form, the Photo View) has
  /// it.
  const gridHoldsKeyboard = (): boolean => {
    const active = document.activeElement;
    return Boolean(active && gridViewport.contains(active));
  };
  const firstVisibleGridIndex = (count: number): number => {
    if (gridTotal === 0) return 0;
    const first = Math.floor(gridViewport.scrollTop / rowPitch()) * count;
    return Math.max(0, Math.min(gridTotal - 1, first));
  };
  /// The Photo a Grid key addresses: the cell the keyboard owns while the
  /// viewport still shows its row, or the first visible Photo after the
  /// keyboard enters the Grid or a pointer scroll moved away from that row.
  const gridKeyboardTarget = (count: number): number | undefined => {
    if (gridTotal === 0) return undefined;
    const first = firstVisibleGridIndex(count);
    const index = gridKeyboardIndex;
    if (index === undefined || index >= gridTotal) return first;
    const firstRow = Math.floor(gridViewport.scrollTop / rowPitch());
    const rows = Math.ceil(effectiveViewportHeight() / rowPitch());
    const row = Math.floor(index / count);
    return row >= firstRow && row < firstRow + rows ? index : first;
  };
  /// Moves the Grid keyboard to one cell. Scrolling reports the new range
  /// through the merged render, so keyboard movement loads the same bounded
  /// windows as scrolling.
  const focusGridCell = (index: number, count: number) => {
    gridKeyboardIndex = index;
    const row = Math.floor(index / count) * rowPitch();
    if (gridViewport.scrollTop !== row) gridViewport.scrollTop = row;
    scheduleGridRender();
  };
  /// Keeps the Grid keyboard's cell focused across merged re-renders and
  /// window replacement, and keeps exactly one Grid cell in the Tab order.
  /// Focus is never taken from another surface that owns it.
  const restoreGridKeyboardFocus = () => {
    const index = gridKeyboardIndex;
    for (const [position, rendered] of renderedCells)
      rendered.cell.tabIndex = position === index ? 0 : -1;
    const active = document.activeElement;
    const owns =
      active === null ||
      active === document.body ||
      gridViewport.contains(active);
    if (!owns || index === undefined) return;
    const cell = gridLayer.querySelector<HTMLButtonElement>(
      `[data-photo-index="${index}"]`,
    );
    if (cell && !cell.disabled) {
      if (active !== cell) cell.focus();
      return;
    }
    // The bounded window that contains the cell is still loading. The Grid
    // keeps focus and returns it to the cell once the window renders.
    if (active !== gridViewport) gridViewport.focus();
  };
  /// Applies one Grid View key to the focused cell. Arrow keys move the cell
  /// focus, and the decision and Rating keys address the focused Photo
  /// through the page model exactly like the Photo View shortcuts.
  const applyGridKey = (event: KeyboardEvent): void => {
    if (event.key === "Escape" && gridMultiCount > 0) {
      // Escape takes the same exit as the bar's Clear control: the
      // multi-selection empties and Select mode ends.
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
      // The Grid owns arrow movement even at its edges, so a boundary key
      // never scrolls the viewport by its native amount.
      event.preventDefault();
      const current = gridKeyboardTarget(count);
      if (current === undefined) return;
      // The first arrow enters the Grid at its first visible Photo instead of
      // stepping past it.
      const next = gridKeyboardIndex === current ? current + step : current;
      if (next < 0 || next >= gridTotal) return;
      focusGridCell(next, count);
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
    const index = gridKeyboardTarget(count);
    if (index === undefined) return;
    event.preventDefault();
    // A decision key also moves the keyboard to its Photo, so the focused
    // cell always shows where the decision or Rating applies.
    if (gridKeyboardIndex !== index) focusGridCell(index, count);
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
  const renderGrid = (model: GridViewModel, position?: number) => {
    // Photo View may keep the source Grid state alive while it owns the
    // visible workflow. Do not let a retained hidden Grid admit window work;
    // the visible Grid render after showGrid() owns that admission.
    gridTotal = model.total;
    gridMultiMode = model.multi.mode;
    gridMultiCount = model.multi.count;
    gridMultiLimit = model.multi.limit;
    gridMultiEnabled = model.multi.enabled;
    gridMultiSelected = model.multi.selected;
    if (!alive || gridView.hidden) return;
    renderBatch();
    const count = columns();
    const stride = columnStride(count);
    const pitch = rowPitch();
    const viewportHeight = effectiveViewportHeight();
    renderedColumns = count;
    renderedColumnStride = stride;
    renderedViewportHeight = viewportHeight;
    const height = `${Math.ceil(model.total / count) * pitch}px`;
    gridCanvas.style.height = height;
    gridLayer.style.height = height;
    if (pendingGridAnchor !== undefined) {
      // A size change scrolls the Photo it anchors on into the first row once
      // the new canvas height can hold it.
      gridViewport.scrollTop = Math.floor(pendingGridAnchor / count) * pitch;
      pendingGridAnchor = undefined;
    }
    if (position !== undefined)
      gridViewport.scrollTop = Math.floor(position / count) * pitch;
    const firstRow = Math.max(
      0,
      Math.floor(gridViewport.scrollTop / pitch) - 2,
    );
    const visibleRows = Math.ceil(viewportHeight / pitch) + 4;
    const start = firstRow * count;
    const end = Math.min(model.total, start + visibleRows * count);
    // Rendering is presentational: a cell that stays in the range and still
    // presents the same Photo facts keeps its button and its thumbnail image,
    // so a merged update never restarts a Thumbnail transfer. Only entering,
    // leaving, or changed cells touch the DOM.
    for (const [index, rendered] of renderedCells)
      if (index < start || index >= end) {
        rendered.cell.remove();
        releaseGridCell(rendered);
        renderedCells.delete(index);
      }
    let anchor: ChildNode | null = null;
    let incomplete = false;
    for (let index = end - 1; index >= start; index -= 1) {
      const photo = model.photoAt(index);
      const existing = renderedCells.get(index);
      const signature = photo
        ? gridCellSignature(index, photo, existing?.deliveryFailed ?? false)
        : LOADING_CELL_SIGNATURE;
      if (!photo) incomplete = true;
      let rendered: RenderedGridCell;
      if (existing && existing.signature === signature) {
        rendered = existing;
        positionGridCell(rendered.cell, index, count, stride);
      } else {
        // A rebuilt cell replaces its old node, so a stale placeholder or a
        // changed rendering never stays in the layer.
        if (existing) {
          releaseGridCell(existing);
          existing.cell.remove();
        }
        rendered = buildGridCell(index, photo, model.total, count, stride);
        renderedCells.set(index, rendered);
      }
      // Walking down keeps rendered cells in source order with the fewest
      // moves: a cell already positioned before the next rendered index is
      // left untouched.
      if (
        rendered.cell.parentNode !== gridLayer ||
        rendered.cell.nextSibling !== anchor
      )
        gridLayer.insertBefore(rendered.cell, anchor);
      anchor = rendered.cell;
    }
    restoreGridKeyboardFocus();
    applyGridMultiSelection();
    // Report the presented range whenever it changes, and keep reporting it
    // while part of it still has no Photo: the owner recomputes the windows
    // it is missing for that range, coalesces them with any request already in
    // flight, and retries a window that failed while the Grid presents it.
    if (
      end > start &&
      (incomplete ||
        reportedGridRange?.start !== start ||
        reportedGridRange.end !== end)
    ) {
      reportedGridRange = { start, end };
      send({ kind: "grid-range", start, end });
    }
  };

  const renderPhotoFacts = (model: PhotoFactsViewModel) => {
    if (!alive) return;
    position.textContent = `${model.index + 1} / ${model.total}`;
    photoFilename.textContent = model.originalFilename ?? "—";
    photoFilename.title = model.originalFilename ?? "";
    currentSelection = model.selectionState ?? "undecided";
    selection.textContent = selectionLabel(currentSelection);
    const value = model.rating ?? 0;
    rating.textContent =
      value === 0 ? "No rating" : `${value} ${value === 1 ? "star" : "stars"}`;
    for (const button of Array.from(
      ratings.querySelectorAll<HTMLButtonElement>("[data-rating-value]"),
    ))
      button.setAttribute(
        "aria-pressed",
        String(Number(button.dataset.ratingValue) === value),
      );
  };
  const renderPhotoMetadata = (model: PhotoMetadataViewModel = {}) => {
    if (!alive) return;
    metadataCaptureTime.textContent =
      model.captureTime === undefined
        ? "—"
        : formatCaptureTime(model.captureTime);
    metadataAperture.textContent = model.aperture ?? "—";
    metadataIso.textContent = model.iso === undefined ? "—" : String(model.iso);
    metadataShutterSpeed.textContent = model.shutterSpeed ?? "—";
    metadataFocalLength.textContent = model.focalLength ?? "—";
  };
  const presentReviewImage = (
    url: string,
    index: number,
    total: number,
  ): ReviewImagePresentation | undefined => {
    if (!alive) return undefined;
    const surface = photoStatusSurface;
    const image = document.createElement("img");
    image.alt = `Photo ${index + 1} of ${total}`;
    image.draggable = false;
    image.fetchPriority = "high";
    image.decoding = "async";
    stage.replaceChildren(image);
    resetZoomForImage();
    // Fit depends on the Preview's natural pixels, so the geometry is
    // applied when the bytes arrive.
    image.addEventListener("load", () => {
      if (!alive || !image.isConnected) return;
      imageNaturalWidth = image.naturalWidth;
      imageNaturalHeight = image.naturalHeight;
      applyZoom();
    });
    const target: ReviewImageTarget = {
      get connected() {
        return image.isConnected;
      },
      get source() {
        return image.src;
      },
      setHandlers(onLoad, onError) {
        image.onload = onLoad;
        image.onerror = onError;
      },
      clearHandlers() {
        image.onload = null;
        image.onerror = null;
      },
      setSource(nextUrl) {
        image.src = nextUrl;
      },
      clearSource() {
        image.removeAttribute("src");
      },
    };
    return {
      target,
      resolvedUrl: new URL(url, window.location.href).href,
      surface,
    };
  };
  const applyPreviewFact = (
    source: ViewPreviewSource | undefined,
    isLimited: boolean,
  ) => {
    previewSource.textContent = isLimited
      ? `${sourceLabel(source)} · limited detail`
      : sourceLabel(source);
    if (isLimited) previewSource.title = LIMITED_PREVIEW_DETAIL;
    else previewSource.removeAttribute("title");
  };

  const renderPhotoShell = (model: PhotoShellViewModel) => {
    if (!alive) return undefined;
    photoTitle.textContent = model.sourceName;
    currentPhotoId = model.photoId;
    photoSurface = {};
    renderPhotoFacts(model);
    renderPhotoMetadata();
    applyPreviewFact(model.previewSource, Boolean(model.limitedDetail));
    let image: ReviewImagePresentation | undefined;
    if (model.previewUrl)
      image = presentReviewImage(model.previewUrl, model.index, model.total);
    else {
      stage.replaceChildren(
        paragraph(model.photoId ? "Loading Preview…" : "Photo unavailable"),
      );
      resetZoomForImage();
    }
    setPhotoStatus(
      model.photoId && model.available === false
        ? "Original File is unavailable. Decisions remain available."
        : "",
    );
    return image;
  };

  const setPhotoStatus = (text: string) => {
    if (!alive || status.textContent === text) return;
    photoStatusSurface = {};
    status.textContent = text;
  };

  const pointerDown = (event: PointerEvent) => {
    if (!alive || !currentPhotoId) return;
    activePointers.set(event.pointerId, {
      x: event.clientX,
      y: event.clientY,
    });
    if (activePointers.size === 2) {
      event.preventDefault();
      beginPinch();
      return;
    }
    if (activePointers.size > 2 || pinch || pointer || !event.isPrimary) return;
    // Fit owns decision swipes; a manual zoom owns bounded panning. Neither
    // state ever records a decision from a drag.
    if (!zoomManual && !decisionInteractionEnabled) return;
    pointer = {
      id: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      startedAt: event.timeStamp,
      vertical: false,
      pan: zoomManual,
      surface: photoSurface,
      photoId: currentPhotoId,
    };
    preview.setPointerCapture(event.pointerId);
  };
  const pointerMove = (event: PointerEvent) => {
    if (!alive) return;
    const tracked = activePointers.get(event.pointerId);
    if (tracked) {
      tracked.x = event.clientX;
      tracked.y = event.clientY;
    }
    if (pinch) {
      updatePinch();
      return;
    }
    if (!pointer || pointer.id !== event.pointerId) return;
    const dx = event.clientX - pointer.startX;
    const dy = event.clientY - pointer.startY;
    const stepX = event.clientX - pointer.lastX;
    const stepY = event.clientY - pointer.lastY;
    pointer.lastX = event.clientX;
    pointer.lastY = event.clientY;
    if (pointer.pan || zoomManual) {
      panX = clamp(panX + stepX, -panLimitX(), panLimitX());
      panY = clamp(panY + stepY, -panLimitY(), panLimitY());
      applyZoom();
      return;
    }
    if (Math.abs(dy) > Math.abs(dx) && Math.abs(dy) > 12)
      pointer.vertical = true;
    if (pointer.vertical) return;
    stage.style.transform = `translateX(${clamp(dx, -140, 140)}px)`;
    selectFeedback.classList.toggle("pending", dx > SWIPE_PENDING_PIXELS);
    rejectFeedback.classList.toggle("pending", dx < -SWIPE_PENDING_PIXELS);
  };
  const finishPointer = (event: PointerEvent, cancelled = false) => {
    if (!alive) return;
    activePointers.delete(event.pointerId);
    if (pinch) {
      // A pinch keeps ownership until fewer than two pointers remain; the
      // finger that is still down never becomes a decision swipe.
      if (activePointers.size >= 2) return;
      pinch = undefined;
      preview.style.removeProperty("touch-action");
      return;
    }
    if (!pointer || pointer.id !== event.pointerId) return;
    const active = pointer;
    clearPointer();
    if (
      active.pan ||
      zoomManual ||
      cancelled ||
      active.vertical ||
      !decisionInteractionEnabled ||
      active.surface !== photoSurface ||
      active.photoId !== currentPhotoId
    )
      return;
    const dx = event.clientX - active.startX;
    const elapsed = Math.max(1, event.timeStamp - active.startedAt);
    const velocity = Math.abs(dx) / elapsed;
    if (
      Math.abs(dx) >= SWIPE_COMMIT_PIXELS ||
      (Math.abs(dx) >= 48 && velocity >= SWIPE_COMMIT_VELOCITY)
    )
      send({
        kind: "photo-mutation",
        field: "selectionState",
        value: dx > 0 ? "selected" : "rejected",
        advance: true,
      });
  };
  const keydown = (event: KeyboardEvent) => {
    if (!alive) return;
    const target = event.target as HTMLElement | null;
    if (
      event.isComposing ||
      target?.isContentEditable ||
      event.altKey ||
      target?.matches("textarea, select, [contenteditable=true]") ||
      (target?.matches("input") &&
        (target as HTMLInputElement).type !== "range")
    )
      return;
    const sourcesOpen = browser.classList.contains("sources-open");
    if (
      event.key === "Escape" &&
      (compactSources.matches || !photoView.hidden) &&
      sourcesOpen
    ) {
      event.preventDefault();
      closeSources();
      return;
    }
    if (sourcesOpen) return;
    const modifier = event.ctrlKey || event.metaKey;
    if (modifier && !event.shiftKey && event.key.toLowerCase() === "z") {
      event.preventDefault();
      send({ kind: "undo" });
      return;
    }
    if (photoView.hidden) {
      // Grid View keys act only while the Grid owns keyboard focus.
      if (!modifier && !event.shiftKey && gridHoldsKeyboard())
        applyGridKey(event);
      return;
    }
    if (modifier) return;
    if (event.key === "+" || event.key === "=") {
      if (!measurableImage()) return;
      event.preventDefault();
      zoomBy(ZOOM_STEP);
      return;
    }
    if (event.key === "-" || event.key === "_") {
      if (!measurableImage()) return;
      event.preventDefault();
      zoomBy(1 / ZOOM_STEP);
      return;
    }
    // A focused zoom slider keeps its own key handling; every other Photo
    // View shortcut stays available while it holds focus.
    if (target?.matches("input[type=range]") && rangeAdjustmentKey(event.key))
      return;
    if (event.shiftKey) return;
    if (event.key === "ArrowLeft") send({ kind: "previous" });
    else if (event.key === "ArrowRight") send({ kind: "next" });
    else if (event.key.toLowerCase() === "f") {
      event.preventDefault();
      applyFit();
    } else if (event.key.toLowerCase() === "d") {
      event.preventDefault();
      toggleDetail();
    } else if (event.key.toLowerCase() === "p")
      send({
        kind: "photo-mutation",
        field: "selectionState",
        value: "selected",
        advance: true,
      });
    else if (event.key.toLowerCase() === "x")
      send({
        kind: "photo-mutation",
        field: "selectionState",
        value: "rejected",
        advance: true,
      });
    else if (
      event.key.toLowerCase() === "u" &&
      currentSelection !== "undecided"
    )
      send({
        kind: "photo-mutation",
        field: "selectionState",
        value: "undecided",
        advance: false,
      });
    else if (/^[0-5]$/.test(event.key))
      send({
        kind: "photo-mutation",
        field: "rating",
        value: Number(event.key),
        advance: false,
      });
  };

  const onResize = () => {
    requestAnimationFrame(() => {
      if (!alive || gridView.hidden) return;
      if (
        columns() === renderedColumns &&
        columnStride() === renderedColumnStride &&
        effectiveViewportHeight() === renderedViewportHeight
      )
        return;
      send({ kind: "grid-resize" });
    });
  };
  const onScroll = () => {
    if (!alive || gridView.hidden) return;
    // Scrolling reports the visible range through the merged render; it never
    // starts per-cell work.
    scheduleGridRender();
  };

  const renderMembership = (model: MembershipViewModel) => {
    if (!alive) return;
    membershipModel = model;
    const pending = new Set(model.pendingAlbumIds);
    const renderFacts = () => {
      membershipList.replaceChildren();
      membershipMessage.hidden = true;
      membershipMessage.textContent = "";
      membershipRetry.hidden = true;
      if (!model.photoPresent) {
        membershipStatus.hidden = false;
        membershipStatus.textContent = "No current Photo.";
        membershipList.hidden = true;
        return;
      }
      if (model.loading) {
        membershipStatus.hidden = false;
        membershipStatus.textContent = "Loading Albums…";
        membershipList.hidden = true;
      } else if (model.failed) {
        membershipStatus.hidden = false;
        membershipStatus.textContent = "Albums could not be loaded.";
        membershipList.hidden = true;
        membershipRetry.hidden = false;
      } else if (model.containing.length === 0) {
        membershipStatus.hidden = false;
        membershipStatus.textContent = "Not in any Album yet";
        membershipList.hidden = true;
      } else {
        membershipStatus.hidden = true;
        membershipStatus.textContent = "";
        membershipList.hidden = false;
        for (const album of model.containing) {
          const item = document.createElement("li");
          item.className = "membership-item";
          item.textContent = album.name;
          membershipList.append(item);
        }
      }
      if (model.message) {
        membershipMessage.hidden = false;
        membershipMessage.textContent = model.message;
      }
    };
    const renderOptions = () => {
      // Rebuilding the options must not drop keyboard focus from a checkbox
      // the visitor is operating, even while that checkbox is disabled for
      // its in-flight toggle.
      const focused = document.activeElement;
      const focusedAlbumId =
        focused instanceof HTMLInputElement &&
        membershipOptions.contains(focused)
          ? focused.dataset.membershipAlbumId
          : undefined;
      if (focusedAlbumId !== undefined && pending.has(focusedAlbumId))
        membershipFocusAlbumId = focusedAlbumId;
      membershipOptions.replaceChildren();
      if (!model.options.length) {
        const empty = paragraph(
          model.photoPresent ? "No Albums yet." : "No current Photo.",
        );
        empty.className = "membership-empty";
        membershipOptions.append(empty);
        return;
      }
      for (const album of model.options) {
        const option = document.createElement("label");
        option.className = "membership-option";
        const input = document.createElement("input");
        input.type = "checkbox";
        input.dataset.membershipAlbumId = album.id;
        input.checked = album.member;
        input.disabled = !model.photoPresent || pending.has(album.id);
        input.addEventListener("change", () => {
          if (!alive) return;
          send({
            kind: "membership-toggle",
            albumId: album.id,
            member: input.checked,
          });
        });
        const name = document.createElement("span");
        name.textContent = album.name;
        option.append(input, name);
        membershipOptions.append(option);
      }
      const targetId = focusedAlbumId ?? membershipFocusAlbumId;
      if (targetId !== undefined) {
        const restored = Array.from(
          membershipOptions.querySelectorAll("input"),
        ).find((input) => input.dataset.membershipAlbumId === targetId);
        if (!restored) membershipFocusAlbumId = undefined;
        else if (!restored.disabled) {
          if (document.activeElement === document.body) restored.focus();
          membershipFocusAlbumId = undefined;
        }
      }
    };
    renderFacts();
    membershipManage.disabled = !model.photoPresent;
    membershipManage.setAttribute(
      "aria-expanded",
      String(membershipManageOpen),
    );
    membershipPanel.hidden = !membershipManageOpen;
    if (membershipManageOpen) renderOptions();
    else {
      membershipFocusAlbumId = undefined;
      membershipOptions.replaceChildren();
    }
  };

  const renderFolderAlbum = (model: FolderAlbumViewModel) => {
    if (!alive) return;
    folderAlbumControls.hidden = !model.visible;
    if (!model.visible) {
      folderAlbumSelect.replaceChildren();
      folderAlbumStatus.textContent = "";
      folderAlbumSelection = "";
      return;
    }
    const selectedStillExists = model.albums.some(
      (album) => album.id === folderAlbumSelection,
    );
    if (!selectedStillExists)
      folderAlbumSelection = model.selectedAlbumId || model.albums[0]?.id || "";
    folderAlbumSelect.replaceChildren();
    for (const album of model.albums) {
      const option = document.createElement("option");
      option.value = album.id;
      option.textContent = album.name;
      option.selected = album.id === folderAlbumSelection;
      folderAlbumSelect.append(option);
    }
    folderAlbumSelect.disabled = model.pending || !model.albums.length;
    addFolderToAlbum.disabled =
      model.pending || !model.albums.length || !folderAlbumSelection;
    folderAlbumStatus.textContent = model.status ?? "";
  };

  compactSources.addEventListener("change", onSourceViewportChange);
  const onShortViewportChange = () => {
    if (!alive) return;
    if (shortViewport.matches) {
      clearFilmstripCells();
      return;
    }
    send({ kind: "filmstrip-resize" });
  };
  shortViewport.addEventListener("change", onShortViewportChange);
  sourceToggle.addEventListener("click", () => openSources("grid"));
  photoSourceToggle.addEventListener("click", () => openSources("photo"));
  sourceClose.addEventListener("click", () => closeSources());
  sourceScrim.addEventListener("click", () => closeSources());
  gridViewport.addEventListener("scroll", onScroll);
  window.addEventListener("resize", onResize);
  window.addEventListener("keydown", keydown);
  const stageObserver = new ResizeObserver(() => {
    if (!alive) return;
    // Fit is recomputed and a manual percentage keeps its value, while a
    // shrinking stage re-clamps how far the Preview may be panned.
    clampPan();
    applyZoom();
  });
  stageObserver.observe(stage);
  back.addEventListener("click", () => send({ kind: "show-grid" }));
  refresh.addEventListener("click", () => send({ kind: "refresh" }));
  gridEmptyAction.addEventListener("click", () =>
    send({ kind: "library-check" }),
  );
  retry.addEventListener("click", () => send({ kind: "retry-source" }));
  retryPhoto.addEventListener("click", () => send({ kind: "retry-photo" }));
  previous.addEventListener("click", () => send({ kind: "previous" }));
  next.addEventListener("click", () => send({ kind: "next" }));
  undo.addEventListener("click", () => send({ kind: "undo" }));
  stage.addEventListener("dblclick", toggleDetail);
  zoomFit.addEventListener("click", applyFit);
  zoomOut.addEventListener("click", () => zoomBy(1 / ZOOM_STEP));
  zoomIn.addEventListener("click", () => zoomBy(ZOOM_STEP));
  zoom100.addEventListener("click", () => applyManualZoom(100));
  zoomSlider.addEventListener("input", () =>
    applyManualZoom(Number(zoomSlider.value)),
  );
  preview.addEventListener("wheel", wheelZoom, { passive: false });
  zoomControls.addEventListener("pointerdown", (event) =>
    event.stopPropagation(),
  );
  zoomControls.addEventListener("pointermove", (event) =>
    event.stopPropagation(),
  );
  zoomControls.addEventListener("pointerup", (event) =>
    event.stopPropagation(),
  );
  select.addEventListener("click", () =>
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: "selected",
      advance: true,
    }),
  );
  reject.addEventListener("click", () =>
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: "rejected",
      advance: true,
    }),
  );
  clear.addEventListener("click", () =>
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: "undecided",
      advance: false,
    }),
  );
  ratings.addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
      "[data-rating-value]",
    );
    if (button)
      send({
        kind: "photo-mutation",
        field: "rating",
        value: Number(button.dataset.ratingValue),
        advance: false,
      });
  });
  membershipManage.addEventListener("click", () => {
    if (!alive) return;
    membershipManageOpen = !membershipManageOpen;
    if (membershipModel) renderMembership(membershipModel);
  });
  membershipRetry.addEventListener("click", () =>
    send({ kind: "membership-retry" }),
  );
  folderAlbumSelect.addEventListener("change", () => {
    if (!alive) return;
    folderAlbumSelection = folderAlbumSelect.value;
    addFolderToAlbum.disabled = !folderAlbumSelection;
  });
  addFolderToAlbum.addEventListener("click", () => {
    if (folderAlbumSelection)
      send({ kind: "folder-album-add", albumId: folderAlbumSelection });
  });
  gridSelectMode.addEventListener("click", () => {
    if (!alive) return;
    send({ kind: "grid-select-mode", mode: !gridMultiMode });
  });
  batchSelect.addEventListener("click", () => {
    if (!alive || batchSelect.disabled) return;
    send({ kind: "grid-batch-mutation", value: "selected" });
  });
  batchReject.addEventListener("click", () => {
    if (!alive || batchReject.disabled) return;
    send({ kind: "grid-batch-mutation", value: "rejected" });
  });
  batchClear.addEventListener("click", () =>
    send({ kind: "grid-multi-clear" }),
  );
  batchAlbumSelect.addEventListener("change", () => {
    if (!alive) return;
    batchAlbumSelection = batchAlbumSelect.value;
    renderBatch();
  });
  batchAlbumAdd.addEventListener("click", () => {
    if (!alive || batchAlbumAdd.disabled || !batchAlbumSelection) return;
    send({ kind: "grid-batch-album-add", albumId: batchAlbumSelection });
  });
  sortSelect.addEventListener("change", () => {
    if (!alive || sortSelect.disabled) return;
    send({ kind: "sort-change", order: sortSelect.value as ViewSourceOrder });
  });
  filterSelect.addEventListener("change", () => {
    if (!alive || filterSelect.disabled) return;
    send({
      kind: "filter-change",
      selection: filterSelect.value as ViewSelectionFilter,
    });
  });
  sizeSelect.addEventListener("change", () => {
    if (!alive) return;
    setGridThumbnailSize(sizeSelect.value as GridThumbnailSize);
  });
  preview.addEventListener("pointerdown", pointerDown);
  preview.addEventListener("pointermove", pointerMove);
  preview.addEventListener("pointerup", (event) => finishPointer(event));
  preview.addEventListener("pointercancel", (event) =>
    finishPointer(event, true),
  );
  preview.addEventListener("lostpointercapture", (event) =>
    finishPointer(event, true),
  );
  syncSourcePanel();

  return {
    get photoStatusSurface() {
      return photoStatusSurface;
    },
    get photoStatusEmpty() {
      return status.textContent === "";
    },
    isPhotoStatusSurfaceCurrent: (surface) =>
      alive && surface === photoStatusSurface,
    setPhotoStatus,
    presentSummary(text, action, libraryCheckState) {
      if (!alive) return;
      const render = (surface: HTMLElement) => {
        surface.replaceChildren(document.createTextNode(text));
        if (!action) return;
        const button = document.createElement("button");
        button.type = "button";
        button.className = "summary-action";
        button.textContent =
          action.kind === "retry-library-check"
            ? "Retry Library Check"
            : "Refresh Current Source";
        button.addEventListener("click", () =>
          send({
            kind: "summary-action",
            presentationId: action.presentationId,
          }),
        );
        surface.append(" ", button);
      };
      render(summaryStatus);
      gridSummary.replaceChildren();
      if (libraryCheckState && libraryCheckState !== "idle")
        render(gridSummary);
      gridEmptyAction.disabled = libraryCheckState === "active";
    },
    setConnection(isConnected, sourceRetryVisible, photoRetryVisible) {
      if (!alive) return;
      if (!isConnected) resetGestures();
      connection.textContent = isConnected ? "Connected" : "Disconnected";
      connection.classList.toggle("offline", !isConnected);
      retry.hidden = !sourceRetryVisible;
      retryPhoto.hidden = !photoRetryVisible;
    },
    setSourceTitle(name) {
      if (!alive) return;
      gridTitle.textContent = name;
      photoTitle.textContent = name;
    },
    setGridStatus(text) {
      if (!alive) return;
      gridStatus.textContent = text;
      gridEmpty.hidden = true;
      gridEmptyMessage.textContent = "";
      gridEmptyAction.hidden = true;
    },
    setGridEmpty(text, libraryCheck = false) {
      if (!alive) return;
      gridEmptyMessage.textContent = text ?? "";
      gridEmptyAction.hidden = !text || !libraryCheck;
      gridEmpty.hidden = !text;
    },
    renderSources,
    renderFolderAlbum,
    renderBatchAlbums(model) {
      if (!alive) return;
      batchAlbums = model.albums;
      batchAlbumsPending = model.pending;
      renderBatch();
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
      sortSelect.value = options.some((option) => option.value === model.value)
        ? model.value
        : options[0]!.value;
      sortSelect.disabled = !model.enabled;
      gridSort.hidden = false;
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
      filterSelect.value = model.value;
      filterSelect.disabled = !model.enabled;
      gridFilter.hidden = false;
    },
    renderProgress(model) {
      if (!alive) return;
      const decided = model.selected + model.rejected;
      const total = decided + model.undecided;
      // A source whose facts report no Photo has no progress to show.
      const text =
        model.visible && total > 0
          ? `${decided.toLocaleString()} of ${total.toLocaleString()} decided · ${model.selected.toLocaleString()} selected · ${model.rejected.toLocaleString()} rejected · ${model.undecided.toLocaleString()} undecided`
          : "";
      if (text === renderedProgressText) return;
      renderedProgressText = text;
      gridProgress.textContent = text;
      gridProgress.hidden = text === "";
    },
    setControls(model) {
      if (!alive) return;
      // A disabled cell cannot hold focus. The Grid keeps its keyboard
      // position on the viewport instead of losing it to the page while a
      // write settles or a source changes readiness, and takes the cell back
      // when the Grid becomes interactive again.
      const focused = document.activeElement;
      const heldCell = Boolean(
        focused instanceof HTMLElement &&
          gridLayer.contains(focused) &&
          focused.matches("[data-photo-index]"),
      );
      const becameInteractive = !gridInteractionEnabled && model.gridEnabled;
      gridInteractionEnabled = model.gridEnabled;
      for (const cell of Array.from(
        gridLayer.querySelectorAll<HTMLButtonElement>(
          ".photo-cell[data-photo-index]",
        ),
      ))
        cell.disabled = !gridInteractionEnabled;
      if (heldCell && !gridInteractionEnabled) gridViewport.focus();
      else if (becameInteractive) restoreGridKeyboardFocus();
      decisionInteractionEnabled = model.decisionEnabled;
      for (const button of [
        select,
        reject,
        ...Array.from(ratings.querySelectorAll<HTMLButtonElement>("button")),
      ])
        button.disabled = !model.decisionEnabled;
      clear.disabled = !model.clearEnabled;
      back.disabled = !model.backEnabled;
      refresh.disabled = !model.refreshEnabled;
      retry.disabled = !model.recoveryEnabled;
      retryPhoto.disabled = !model.recoveryEnabled;
      previous.disabled = !model.previousEnabled;
      next.disabled = !model.nextEnabled;
      undo.disabled = !model.undoEnabled;
      const wasStripInteractive = filmstripInteractive;
      filmstripInteractive = model.filmstripEnabled;
      const heldElement = document.activeElement as HTMLElement | null;
      const holdsStripFocus =
        wasStripInteractive &&
        !model.filmstripEnabled &&
        heldElement?.dataset.filmstripIndex !== undefined;
      applyFilmstripInteractivity();
      if (holdsStripFocus) {
        heldStripIndex = Number(heldElement.dataset.filmstripIndex);
        photoView.focus();
      } else if (!wasStripInteractive && model.filmstripEnabled) {
        restoreHeldStripFocus();
      }
      syncZoomControls();
    },
    renderMembership,
    prepareSourceOpen(name) {
      if (!alive) return;
      const returnFocus = browser.classList.contains("sources-open");
      cancelGridRender();
      stage.replaceChildren();
      resetZoomForImage();
      gridView.hidden = false;
      photoView.hidden = true;
      clearGridCells();
      clearFilmstripCells();
      closeSources(false);
      if (returnFocus) gridViewport.focus();
      gridTitle.textContent = name;
      folderAlbumControls.hidden = true;
      folderAlbumSelect.replaceChildren();
      folderAlbumStatus.textContent = "";
      gridStatus.textContent = "Preparing Library order…";
      gridEmpty.hidden = true;
      gridEmptyMessage.textContent = "";
      gridEmptyAction.hidden = true;
      currentPhotoId = undefined;
      photoSurface = {};
      gridKeyboardIndex = undefined;
      // A new source starts with no multi-selection: the bar presents nothing
      // until the page model marks Photos again.
      resetGridMultiSelection();
    },
    renderGrid,
    scheduleGridRender,
    cancelGridRender,
    clearGridCells,
    rebindDetachedGridCells,
    resetGridMultiSelection,
    gridVisible: () => alive && !gridView.hidden,
    scrollToGridIndex(index) {
      if (alive)
        gridViewport.scrollTop = Math.floor(index / columns()) * rowPitch();
    },
    focusGridIndex(index) {
      if (!alive) return;
      const count = columns();
      const target = Math.max(0, Math.min(Math.max(gridTotal - 1, 0), index));
      gridKeyboardIndex = target;
      gridViewport.scrollTop = Math.floor(target / count) * rowPitch();
      scheduleGridRender();
    },
    showGrid(index) {
      if (!alive) return;
      resetZoomForImage();
      photoView.hidden = true;
      gridView.hidden = false;
      // Photo View detached the owner's Grid images, so the visible Grid
      // rebuilds its cells and re-attaches every thumbnail it still shows.
      clearGridCells();
      clearFilmstripCells();
      closeSources(false);
      gridViewport.focus();
      // Returning from Photo View returns the Grid keyboard to that Photo
      // cell; the merged render focuses it once it is rendered.
      gridKeyboardIndex = index;
      if (index !== undefined)
        gridViewport.scrollTop = Math.floor(index / columns()) * rowPitch();
      scheduleGridRender();
    },
    enterPhoto() {
      if (!alive) return;
      gridView.hidden = true;
      photoView.hidden = false;
      photoView.scrollTop = 0;
      syncSourcePanel();
      photoView.focus();
      resetZoomForImage();
      photoSurface = {};
    },
    renderFilmstrip,
    renderPhotoFacts,
    renderPhotoMetadata,
    renderPhotoShell,
    presentReviewImage,
    reviewImageMatches(url) {
      if (!alive) return false;
      const image = stage.querySelector<HTMLImageElement>("img");
      return Boolean(
        image && image.src === new URL(url, window.location.href).href,
      );
    },
    showPreviewUnavailable(text) {
      if (alive && !stage.querySelector("img")) {
        stage.replaceChildren(paragraph(text));
        resetZoomForImage();
      }
    },
    setPreviewFacts(value, isLimited) {
      if (!alive) return;
      applyPreviewFact(value, isLimited);
    },
    setAlbumFormMessage(formId, message) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      albumForm.message = message;
      albumForm.pending = false;
      if (sourceModel) renderSources(sourceModel);
    },
    setAlbumFormPending(formId, pending, name) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      albumForm.pending = pending;
      if (name !== undefined) albumForm.name = name;
      delete albumForm.message;
      if (sourceModel) renderSources(sourceModel);
    },
    dismissAlbumForm(formId) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      albumFocusRequest = {
        kind: "return",
        focusKey: albumForm.returnFocusKey,
      };
      albumForm = undefined;
      if (sourceModel) renderSources(sourceModel);
    },
    dispose() {
      if (!alive) return;
      alive = false;
      resetGestures();
      stageObserver.disconnect();
      clearFilmstripCells();
      preview.removeEventListener("wheel", wheelZoom);
      cancelGridRender();
      compactSources.removeEventListener("change", onSourceViewportChange);
      shortViewport.removeEventListener("change", onShortViewportChange);
      gridViewport.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onResize);
      window.removeEventListener("keydown", keydown);
    },
  };
}

function required<T extends Element>(root: ParentNode, selector: string): T {
  const value = root.querySelector<T>(selector);
  if (!value) throw new Error(`Missing ${selector}`);
  return value;
}

function paragraph(text: string): HTMLParagraphElement {
  const value = document.createElement("p");
  value.textContent = text;
  return value;
}

function selectionLabel(value?: ViewSelectionState): string {
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
  if (photo.ambiguous) facts.push("Ambiguous pairing");
  if (photo.preview.state === "unavailable") facts.push("Preview unavailable");
  if (photo.preview.state === "failed") facts.push("Preview failed");
  if (deliveryFailed) facts.push("Thumbnail delivery failed");
  return facts;
}

/// Everything one rendered cell presents at its position. Two renders with the
/// same signature leave the cell's button and thumbnail image untouched.
function gridCellSignature(
  index: number,
  photo: GridPhotoViewModel,
  deliveryFailed: boolean,
): string {
  return [
    String(index),
    photo.id,
    photo.originalFilename ?? "",
    photo.available ? "available" : "unavailable",
    photo.ambiguous ? "ambiguous" : "paired",
    photo.selectionState,
    String(photo.rating),
    photo.preview.state,
    photo.preview.thumbnailUrl ?? "",
    deliveryFailed ? "delivery-failed" : "delivered",
  ].join("|");
}

function filmstripCellSignature(
  cell: FilmstripCellViewModel,
  total: number,
  deliveryFailed: boolean,
): string {
  const photo = cell.photo;
  return [
    String(cell.index),
    String(total),
    cell.current ? "current" : "neighbor",
    photo ? gridCellSignature(cell.index, photo, deliveryFailed) : "loading",
  ].join("|");
}
function gridThumbnailTarget(
  image: HTMLImageElement,
  setDeliveryFailed: (failed: boolean) => void,
): GridThumbnailTarget {
  return {
    get complete() {
      return image.complete;
    },
    get isConnected() {
      return image.isConnected;
    },
    get src() {
      return image.src;
    },
    set src(value) {
      image.src = value;
    },
    get onload() {
      return image.onload;
    },
    set onload(value) {
      image.onload = value;
    },
    get onerror() {
      return image.onerror;
    },
    set onerror(value) {
      image.onerror = value;
    },
    removeAttribute(name) {
      image.removeAttribute(name);
    },
    setDeliveryFailed,
  };
}

function sourceLabel(source?: ViewPreviewSource): string {
  return source === "matching-jpeg"
    ? "JPEG"
    : source === "embedded-raw-jpeg"
      ? "RAW embedded JPEG"
      : "—";
}

function clamp(value: number, low: number, high: number): number {
  return Math.max(low, Math.min(high, value));
}

/// Keys a focused range input handles itself: arrows, Home, End, and Page.
function rangeAdjustmentKey(key: string): boolean {
  return (
    key.startsWith("Arrow") ||
    key.startsWith("Page") ||
    key === "Home" ||
    key === "End"
  );
}
