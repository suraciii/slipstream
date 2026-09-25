import "./library-browser.css";

import { createModalSurfaces } from "./modal-surface.js";
import {
  addressFor,
  type NavigationGridRestoration,
} from "../model/browser-navigation.js";
import { formatCaptureTime } from "./capture-time.js";
import { formatPhotoCount } from "./photo-count.js";
import type {
  EditorWhiteBalance,
  EditorWhiteBalancePresentation,
} from "../model/photo-editor.js";

/// One white-balance intent in the workspace's words. A stored mode the
/// deployment does not admit still reads as retained intent, never as as-shot.
const describeWhiteBalance = (intent: EditorWhiteBalance): string =>
  intent.mode === "as-shot"
    ? "As shot"
    : `Temperature ${intent.temperatureKelvin} K, tint ${intent.tintMilli}`;

/// The white-balance presentation before any facts arrive: as-shot intent, no
/// admitted adjustable mode, and controls that take no input.
const loadingWhiteBalance = (): EditorWhiteBalancePresentation =>
  Object.freeze({
    intent: Object.freeze({ mode: "as-shot" }),
    modes: Object.freeze(["as-shot"]),
    adjustable: false,
    note: "",
    temperatureKelvin: null,
    tintMilli: null,
    resettable: false,
  });

type ViewSelectionState = "undecided" | "selected" | "rejected";
type ViewPreviewSource = "jpeg-original" | "raw-embedded-jpeg";

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
const RATING_WHEEL_HOLD_MS = 450;
const RATING_WHEEL_MOVE_PIXELS = 12;
const RATING_WHEEL_RADIUS = 78;
const RATING_WHEEL_OPTION_COUNT = 6;

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
/// reports for the complete source, plus the number of Photos in the currently
/// visible filtered sequence.
export type GridProgressViewModel = Readonly<{
  visible: boolean;
  visibleTotal: number;
  sourceTotal: number;
  selected: number;
  rejected: number;
  undecided: number;
}>;

/// The closed editing stages. Camera shows the camera-produced Preview,
/// Develop the Edit Preview of the Development Result, and Film the Edit
/// Preview of the Film Result once that capability is enabled.
export type EditorStage = "camera" | "develop" | "film";

export type EditorExportViewModel = Readonly<{
  state:
    | "idle"
    | "submitting"
    | "queued"
    | "running"
    | "succeeded"
    | "failed"
    | "cancelled";
  note: string;
  artifact: Readonly<{
    byteLength: number;
    width: number;
    height: number;
    expiresAt: string;
  }> | null;
  canSubmit: boolean;
  canCancel: boolean;
  canRetry: boolean;
  canDownload: boolean;
}>;

export type EditorViewModel = Readonly<{
  photoId: string;
  loading: boolean;
  stage: EditorStage;
  /// What the presented image actually is, in provenance language.
  stageNote: string;
  /// Why the Film stage cannot be presented, when it cannot.
  filmReason: string;
  sourceSupport: "supported" | "unsupported" | "unavailable" | "unknown";
  processingAvailable: boolean;
  /// Why the deployment cannot execute development work, when it cannot. An
  /// unavailable deployment is explained instead of attempted.
  capabilityNote: string;
  exposureEv: number;
  savedExposureEv: number;
  baselineExposureEv: number;
  exposureMinimumEv: number;
  exposureMaximumEv: number;
  exposureStepEv: number;
  /// The white-balance intent in force, the modes a Photographer may select,
  /// and why an adjustable mode is not offered.
  whiteBalance: EditorWhiteBalancePresentation;
  canEdit: boolean;
  canPreview: boolean;
  previewing: boolean;
  /// What the presented Edit Preview is, or why none is presented.
  previewNote: string;
  /// True while the retained image is older than the current settings.
  previewStale: boolean;
  saving: boolean;
  dirty: boolean;
  canUndo: boolean;
  canRedo: boolean;
  comparing: boolean;
  conflict: Readonly<{ message: string }> | null;
  draftNote: string;
  export: EditorExportViewModel;
  status: string;
}>;

export type LibraryBrowserIntent =
  | Readonly<{ kind: "editor-open" | "editor-refresh"; photoId: string }>
  | Readonly<{ kind: "editor-exposure"; photoId: string; exposureEv: number }>
  | Readonly<{
      kind: "editor-white-balance-mode";
      photoId: string;
      mode: string;
    }>
  | Readonly<{
      kind: "editor-temperature";
      photoId: string;
      temperatureKelvin: number;
    }>
  | Readonly<{ kind: "editor-tint"; photoId: string; tintMilli: number }>
  | Readonly<{ kind: "editor-stage"; photoId: string; stage: EditorStage }>
  | Readonly<{ kind: "editor-compare"; photoId: string; pressed: boolean }>
  | Readonly<{
      kind:
        | "editor-undo"
        | "editor-redo"
        | "editor-reset"
        | "editor-reset-exposure"
        | "editor-reset-white-balance"
        | "editor-preview"
        | "editor-rebind"
        | "editor-use-saved"
        | "editor-reapply"
        | "editor-discard-draft"
        | "editor-export-submit"
        | "editor-export-cancel"
        | "editor-export-retry"
        | "editor-export-download";
      photoId: string;
    }>
  | Readonly<{ kind: "summary-action"; presentationId: number }>
  | Readonly<{ kind: "sign-out" }>
  /// Commits the View options draft once. A size-only change never reaches the
  /// page model: it is Grid presentation and keeps the open Snapshot and its
  /// anchor.
  | Readonly<{
      kind: "view-options-apply";
      order: ViewSourceOrder;
      selection: ViewSelectionFilter;
    }>
  | Readonly<{ kind: "source-open"; source: SourceReference }>
  /// Resolves an Album's saved position under the existing saved-position
  /// rules and opens Photo View. Its destination requires resolution, so it
  /// stays a button rather than a destination anchor.
  | Readonly<{ kind: "album-resume"; albumId: string }>
  /// The one explicit action an explained Grid state offers, such as opening
  /// the current Folder after its publication changed.
  | Readonly<{ kind: "explained-action" }>
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
  | Readonly<{ kind: "grid-batch-album-remove" }>
  | Readonly<{ kind: "grid-batch-review" }>
  /// Opens the review of the current `Rejected` result. The review is the only
  /// surface that removes Photos, and it removes exactly the result it named.
  | Readonly<{ kind: "removal-review-open" }>
  | Readonly<{ kind: "removal-review-close" }>
  | Readonly<{ kind: "removal-confirm" }>
  /// Undo restores one confirmed operation. It is offered beside the removal
  /// that confirmed it and in the listing a Photographer returns to, so it
  /// names the surface that reports the outcome.
  | Readonly<{ kind: "removal-undo"; surface: "review" | "listing" }>
  /// Opens the bounded Removed Photos listing: the recovery path for every
  /// confirmed removal, and the only ordinary surface a removed Photo has.
  | Readonly<{ kind: "removed-list-open" }>
  | Readonly<{ kind: "removed-list-close" }>
  | Readonly<{ kind: "removed-page"; direction: -1 | 1 }>
  | Readonly<{ kind: "removed-retry" }>
  | Readonly<{
      kind: "removed-restore";
      photoId: string;
      /// The removal marker the rendered row presented, so the restore names
      /// the removal the Photographer saw.
      removedAtMs: number;
    }>
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
  | Readonly<{ kind: "membership-retry" }>
  | Readonly<{ kind: "recovery-entry" | "recovery-close" }>
  | Readonly<{
      kind: "recovery-propose";
      oldPrefix: string;
      newPrefix: string;
    }>
  | Readonly<{
      kind: "recovery-propose-single";
      originalId: string;
      newLocation: string;
    }>
  | Readonly<{
      kind: "recovery-apply";
      items: ReadonlyArray<
        Readonly<{
          originalId: string;
          newLocation: string;
          retireDestination: boolean;
        }>
      >;
    }>;

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
  original: Readonly<{ kind: "raw" | "jpeg"; available: boolean }>;
  originalFilename?: string;
  selectionState: ViewSelectionState;
  rating: number;
  hasSavedEdits: boolean;
  preview: Readonly<{
    state: "inspection-pending" | "ready" | "unavailable" | "failed";
    thumbnailUrl?: string;
  }>;
}>;

/// One unavailable Original the bounded recovery review entry lists.
export type RecoveryEntryViewModel = Readonly<{
  originalId: string;
  location: string;
  kind: "raw" | "jpeg";
  rating: number;
  selectionState: ViewSelectionState;
  albumCount: number;
  fingerprintEnrolled: boolean;
}>;

/// One inspectable proposed mapping for an unavailable Original.
export type RecoveryProposalViewModel = Readonly<{
  originalId: string;
  fromLocation: string;
  toLocation: string;
  kind: "raw" | "jpeg";
  outcome:
    | "matched"
    | "content-mismatch"
    | "missing"
    | "kind-mismatch"
    | "unreadable"
    | "occupied"
    | "colliding";
  verified: boolean;
  retire: Readonly<{ photoId: string; location: string }> | null;
}>;

type GridBatchResultViewModel = Readonly<{
  tone: "success" | "warning" | "failure";
  message: string;
  review?: Readonly<{ label: string }>;
  compensation?: Readonly<{ label: string }>;
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
    result?: GridBatchResultViewModel | undefined;
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
  /// Whether the current source is a `Rejected` result a removal could be
  /// reviewed against. The review is offered only where it can be admitted.
  removalEnabled: boolean;
}>;

/// The batch tray's Album choices. `pending` covers one Add to Album settling
/// and the outcome is reported beside the control.
export type BatchAlbumsViewModel = Readonly<{
  albums: ReadonlyArray<Readonly<{ id: string; name: string }>>;
  pending: boolean;
}>;

/// The reviewed removal: the count one confirmation would take out of the
/// Library, the outcome of the last attempt, and the Undo record a successful
/// removal leaves. `canConfirm` is false once the reviewed result is consumed,
/// so the surface never offers a confirmation it cannot admit.
export type RemovalReviewViewModel = Readonly<{
  reviewed: number;
  pending: boolean;
  canConfirm: boolean;
  tone?: "success" | "warning" | "failure";
  message?: string;
  undo?: Readonly<{ removed: number }>;
}>;

/// One bounded page of removed Photos, newest removal first. Each item carries
/// the facts the Grid and Photo View already present, so the Photographer
/// recognizes what is recoverable before restoring it.
export type RemovedPanelViewModel = Readonly<{
  start: number;
  total: number;
  limit: number;
  pending: boolean;
  /// Whether the last read of the listing failed, so the surface offers the
  /// one control that repeats it.
  canRetry: boolean;
  restoringPhotoId?: string;
  message?: string;
  /// The last confirmed operation, while Undo can still restore it. The
  /// listing is the surface a Photographer returns to, so the operation-level
  /// recovery is offered here and not only beside the confirmation.
  undo?: Readonly<{ removed: number }>;
  items: ReadonlyArray<
    Readonly<{
      photoId: string;
      filename: string;
      removedAtMs: number;
      preview: GridPhotoViewModel["preview"];
    }>
  >;
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
  /// Presents an explained state that is not a source Grid: the Photographer
  /// is told why the requested destination is not shown and given the one
  /// explicit action that establishes it. It never claims a source loaded.
  setGridExplanation(
    message: string,
    action?: Readonly<{ label: string }>,
  ): void;
  renderSources(model: SourceListViewModel): void;
  /// Presents the Folder source's Add-to-Album action, which View options
  /// holds for the open Original Folder.
  renderFolderAlbum(model: FolderAlbumViewModel): void;
  /// Presents the committed source order. View options holds the draft until
  /// an explicit Apply commits it.
  renderSort(model: GridSortViewModel): void;
  /// Presents the committed Selection State filter. View options holds the
  /// draft until an explicit Apply commits it.
  renderFilter(model: GridFilterViewModel): void;
  /// Presents the complete source decision counts and the filtered result
  /// count, which View options names separately.
  renderProgress(model: GridProgressViewModel): void;
  setControls(model: ControlsViewModel): void;
  renderMembership(model: MembershipViewModel): void;
  prepareSourceOpen(name: string): void;
  renderGrid(model: GridViewModel, position?: number): void;
  /// Presents the multi-selection the page model has just emptied: the batch
  /// tray hides and the retained cells drop their markers. A failed source open
  /// or reopen calls it instead of a render, so a tray can never name Photos
  /// the Grid no longer holds.
  resetGridMultiSelection(): void;
  /// Presents the batch tray's Album list and its pending state. The list is
  /// the bounded Album summary the Sources surface already presents.
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
  /// The Grid's current restoration facts: the top visible Photo, its index,
  /// its CSS-pixel offset inside its row, and what the Grid keyboard owns.
  /// Undefined while the Grid presents no Photo.
  captureGridRestoration(): NavigationGridRestoration | undefined;
  /// Restores one captured Grid anchor after the current size step has
  /// determined the column count, then returns cell focus to `focusIndex`, or
  /// focuses the Grid when no cell is named. The offset is applied once, so the
  /// restore never fights later user scrolling.
  restoreGridAnchor(
    model: Readonly<{
      index: number;
      offset: number;
      focusIndex?: number;
    }>,
  ): void;
  /// Closes the transient surfaces a destination change supersedes: the
  /// supporting sheets, the Sources surface, the Album form, and the recovery
  /// review. It adds no history entry.
  closeTransientSurfaces(): void;
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
  editorVisible(): boolean;
  renderEditor(model: EditorViewModel): void;
  presentEditorPreview(url: string): void;
  clearEditorPreview(): void;
  presentReviewImage(
    url: string,
    index: number,
    total: number,
  ): ReviewImagePresentation | undefined;
  /// True while the presented camera Preview is the given URL.
  reviewImageMatches(url: string): boolean;
  showPreviewUnavailable(text: string): void;
  setPreviewFacts(
    source: ViewPreviewSource | undefined,
    limited: boolean,
  ): void;
  setAlbumFormMessage(formId: string, message: string): void;
  setAlbumFormPending(formId: string, pending: boolean, name?: string): void;
  dismissAlbumForm(formId: string): void;
  /// Presents the reviewed removal: the count that would leave the Library,
  /// the outcome of the last attempt, and the Undo record a successful removal
  /// leaves. Opening the surface removes nothing; only the confirmation does.
  openRemovalReview(model: RemovalReviewViewModel): void;
  /// Updates the open removal review without reopening it, so a settling
  /// confirmation, its outcome, and its Undo record all reach the surface the
  /// Photographer is looking at.
  renderRemovalReview(model: RemovalReviewViewModel): void;
  closeRemovalReview(): void;
  /// Opens the bounded Removed Photos listing and presents its first page.
  openRemovedPanel(model: RemovedPanelViewModel): void;
  renderRemovedPanel(model: RemovedPanelViewModel): void;
  closeRemovedPanel(): void;
  /// Presents the committed recovery counts of the last scan and, while
  /// Originals remain unavailable, the one bounded review entry.
  setRecoveryNotice(
    model: Readonly<{
      relocatedPhotos: number;
      unavailablePhotos: number;
    }>,
  ): void;
  /// Opens the recovery review with the remembered facts of every
  /// unavailable Original.
  openRecoveryPanel(entries: ReadonlyArray<RecoveryEntryViewModel>): void;
  /// Presents inspectable proposed mappings; each occupied destination that
  /// an explicit retire-and-bind may replace carries its checkbox.
  renderRecoveryProposals(
    proposals: ReadonlyArray<RecoveryProposalViewModel>,
  ): void;
  setRecoveryPending(pending: boolean): void;
  setRecoveryMessage(text?: string): void;
  closeRecoveryPanel(): void;
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
        <dialog class="source-dialog" data-source-dialog>
          <nav class="source-panel" id="source-panel" data-library-screen aria-label="Library sources">
            <header class="source-header"><h2 id="browser-title">Sources</h2><button type="button" class="quiet source-close" data-source-close>Close</button></header>
            <p data-summary-status role="status">Loading Library…</p>
            <p class="recovery-notice" data-recovery-notice hidden role="status"></p>
            <div class="source-list" data-source-list></div>
            <footer class="source-footer"><button type="button" data-retry hidden>Retry connection</button><button type="button" class="quiet" data-access-sign-out>Sign out</button></footer>
          </nav>
        </dialog>
        <div class="source-resizer" data-source-resizer role="separator" aria-label="Resize sources" aria-orientation="vertical" tabindex="0"></div>
        <section class="grid-view" data-grid-view aria-labelledby="grid-title">
          <header class="grid-header"><div class="grid-header-row"><button type="button" class="quiet source-toggle" data-source-toggle aria-controls="source-panel" aria-expanded="false"><span class="source-toggle-name" data-grid-compact-title>All Photos</span><span class="source-toggle-indicator" aria-hidden="true">▾</span></button><div class="grid-heading"><h2 id="grid-title" data-grid-title>All Photos</h2><p data-grid-status role="status"></p></div><p class="grid-connection" data-grid-connection role="status" hidden></p><div class="grid-tools" data-grid-tools><button type="button" class="quiet" data-grid-view-options>Options</button><span class="options-flag" data-view-options-flag hidden></span><button type="button" class="quiet" data-grid-select-mode aria-pressed="false">Select mode</button><button type="button" class="quiet" data-removal-open hidden>Remove rejected Photos</button><button type="button" class="quiet" data-removed-open>Removed Photos</button></div><div class="grid-selection" data-grid-selection hidden><p class="grid-selection-count" data-batch-count></p><button type="button" class="quiet" data-grid-multi-done>Done</button></div></div><p class="grid-summary" data-grid-summary role="status" aria-live="polite"></p></header>
          <div class="grid-viewport" data-grid-viewport tabindex="0" aria-label="Photo Library Grid"><div class="grid-canvas" data-grid-canvas></div><div class="grid-layer" data-grid-layer></div><div class="grid-empty" data-grid-empty hidden><p data-grid-empty-message role="status"></p><button type="button" data-grid-empty-action hidden>Check Library</button></div></div>
          <div class="grid-batch" data-grid-batch hidden><div class="grid-batch-result" data-grid-batch-result hidden><p class="grid-batch-retained" data-batch-retained hidden>Selection remains active</p><p data-grid-batch-result-text></p><button type="button" class="quiet" data-grid-batch-compensate hidden>Remove added Photos</button></div><div class="grid-batch-actions" data-batch-actions role="group" aria-label="Batch actions"><button type="button" data-batch-select>Select</button><button type="button" data-batch-reject>Reject</button><label for="batch-album-select">Add to</label><select id="batch-album-select" data-batch-album-select></select><button type="button" data-batch-album-add>Add to Album</button></div></div>
        </section>
        <section class="photo-view" data-review data-photo-view hidden tabindex="-1" aria-labelledby="photo-title">
          <header class="photo-header"><button type="button" class="quiet" data-back>Back to Grid</button><div class="photo-identity"><h2 id="photo-title" data-photo-title>Photo</h2><p class="photo-filename" data-photo-filename>—</p></div><div class="photo-header-actions"><button type="button" class="quiet photo-source-toggle" data-photo-source-toggle aria-controls="source-panel" aria-expanded="false">Sources</button><p class="photo-connection" data-photo-connection role="status" hidden></p><button type="button" class="quiet" data-retry-photo hidden>Retry</button></div></header>
          <section class="preview" data-preview aria-label="Photo Preview">
            <div class="swipe-feedback reject" data-reject-feedback>Reject</div>
            <div class="image-stage" data-stage><p>Loading Preview…</p></div>
            <div class="swipe-feedback select" data-select-feedback>Select</div>
            <div class="rating-wheel" data-rating-wheel hidden role="dialog" aria-label="Rating Wheel" aria-describedby="rating-wheel-instructions">
              <p class="rating-wheel-instructions" id="rating-wheel-instructions" data-rating-wheel-instructions>Move across a Rating and release to save.</p>
              <p class="rating-wheel-status" data-rating-wheel-status role="status" aria-live="polite"></p>
              <div class="rating-wheel-options" data-rating-wheel-options role="group" aria-label="Rating choices"></div>
            </div>
          </section>
          <div class="photo-navigation" data-photo-navigation role="group" aria-label="Photo navigation"><button type="button" class="quiet" data-dock-previous>Previous</button><p class="photo-position" data-position>0 / 0</p><button type="button" class="quiet" data-dock-next>Next</button></div>
          <div class="filmstrip-host" data-filmstrip-host><div class="filmstrip" data-filmstrip role="group" aria-label="Neighbor Photos" hidden></div></div>
          <section class="review-bar" aria-label="Photo review"><div class="review-state"><dl class="facts"><div><dt>Selection</dt><dd data-selection>Undecided</dd></div><div><dt>Rating</dt><dd data-rating>No rating</dd></div></dl><p class="status" data-status role="status" aria-live="polite"></p></div></section>
          <div class="quick-action-dock" data-quick-action-dock role="toolbar" aria-label="Quick Photo actions">
            <button type="button" class="reject-button" data-dock-reject>Reject</button>
            <button type="button" class="rating-dock-button" data-dock-rating aria-controls="rating-choices" aria-expanded="false">Rating</button>
            <button type="button" class="select-button" data-dock-select>Select</button>
            <button type="button" class="quiet" data-dock-more aria-controls="photo-tools-panel" aria-expanded="false">More</button>
          </div>
          <dialog class="photo-tools-dialog" data-photo-tools aria-labelledby="photo-tools-title">
            <div class="photo-tools-sheet" id="photo-tools-panel">
              <header class="photo-tools-header"><h2 id="photo-tools-title" data-photo-tools-title>Photo tools</h2><button type="button" class="quiet" data-photo-tools-close>Close</button></header>
              <div class="photo-tools-body">
                <div class="photo-tools-view" data-photo-tools-view="tools">
                  <div class="photo-tools-actions" aria-label="Photo actions"><button type="button" class="quiet" data-photo-tools-clear>Clear</button><button type="button" class="quiet" data-photo-tools-undo disabled>Undo</button></div>
                  <div class="photo-tools-entries" data-photo-tools-entries role="group" aria-label="Photo tools"><button type="button" data-photo-tools-entry="edit">Edit</button><button type="button" data-photo-tools-entry="albums">Albums</button><button type="button" data-photo-tools-entry="details">Details</button><button type="button" data-photo-tools-entry="zoom">Preview Zoom</button><button type="button" data-photo-tools-entry="nearby">Nearby Photos</button><button type="button" data-photo-tools-entry="sources">Sources</button></div>
                </div>
                <div class="photo-tools-view" id="photo-tools-view-edit" data-photo-tools-view="edit" hidden>
                  <header class="photo-tools-view-header"><h3>Edit</h3><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
                  <div class="photo-editor-controls" aria-label="Photo edit recipe">
                    <div class="photo-editor-stages" role="group" aria-label="Editing stage"><button type="button" data-photo-editor-stage="camera" aria-pressed="false">Camera</button><button type="button" data-photo-editor-stage="develop" aria-pressed="true">Develop</button><button type="button" data-photo-editor-stage="film" aria-pressed="false">Film</button></div>
                    <p class="photo-editor-stage-note" id="photo-editor-stage-note" data-photo-editor-stage-note role="status"></p>
                    <p class="photo-editor-provenance" data-photo-editor-provenance role="status"></p>
                    <img class="photo-editor-preview-image" data-photo-editor-preview-image alt="Current stage Edit Preview" hidden>
                    <p class="photo-editor-preview-note" data-photo-editor-preview-note hidden></p>
                    <div class="photo-editor-field"><label for="photo-editor-exposure">Exposure</label><output data-photo-editor-exposure-value for="photo-editor-exposure">0.000 EV</output><input id="photo-editor-exposure" data-photo-editor-exposure type="range" min="0" max="1" step="0.001" value="0" aria-label="Exposure" disabled><button type="button" class="quiet" data-photo-editor-reset-exposure disabled>Reset exposure</button></div>
                    <div class="photo-editor-field photo-editor-white-balance">
                      <label for="photo-editor-white-balance-mode">White balance</label>
                      <select id="photo-editor-white-balance-mode" data-photo-editor-white-balance-mode disabled><option value="as-shot">As shot</option><option value="temperature-tint" disabled>Temperature &amp; tint</option></select>
                      <p class="photo-editor-note" data-photo-editor-white-balance-note hidden></p>
                      <div class="photo-editor-field"><label for="photo-editor-temperature">Temperature</label><output data-photo-editor-temperature-value for="photo-editor-temperature">—</output><input id="photo-editor-temperature" data-photo-editor-temperature type="range" min="1000" max="40000" step="1" value="6500" aria-label="Temperature in Kelvin" disabled></div>
                      <div class="photo-editor-field"><label for="photo-editor-tint">Tint</label><output data-photo-editor-tint-value for="photo-editor-tint">—</output><input id="photo-editor-tint" data-photo-editor-tint type="range" min="-150000" max="150000" step="1" value="0" aria-label="Tint" disabled></div>
                      <button type="button" class="quiet" data-photo-editor-reset-white-balance disabled>Reset white balance</button>
                    </div>
                    <div class="photo-editor-actions"><button type="button" class="quiet" data-photo-editor-undo disabled>Undo</button><button type="button" class="quiet" data-photo-editor-redo disabled>Redo</button><button type="button" class="quiet" data-photo-editor-reset disabled>Reset all</button><button type="button" class="quiet" data-photo-editor-compare aria-pressed="false" title="Press to compare the current settings with the as-shot/baseline development of this stage" disabled>Baseline comparison</button><button type="button" data-photo-editor-preview disabled>Refresh preview</button><button type="button" class="quiet" data-photo-editor-rebind hidden disabled>Rebind source</button><button type="button" class="quiet" data-photo-editor-refresh>Reload recipe</button></div>
                    <p class="photo-editor-fact"><span>White balance</span><span data-photo-editor-white-balance>As shot</span></p>
                    <p class="photo-editor-fact"><span>Source support</span><span data-photo-editor-support>Checking…</span></p>
                    <p class="photo-editor-fact"><span>Processing</span><span data-photo-editor-processing>Checking…</span></p>
                    <p class="photo-editor-note" data-photo-editor-capability hidden></p>
                    <p class="photo-editor-draft" data-photo-editor-draft hidden role="status"></p>
                    <div class="photo-editor-conflict" data-photo-editor-conflict hidden><p data-photo-editor-conflict-message role="alert"></p><div class="photo-editor-actions"><button type="button" data-photo-editor-use-saved>Use saved recipe</button><button type="button" class="quiet" data-photo-editor-reapply>Reapply my settings</button><button type="button" class="quiet" data-photo-editor-discard-draft>Discard draft</button></div></div>
                    <div class="photo-editor-export" aria-label="Export"><p class="photo-editor-export-heading">Export <span>Development TIFF</span></p><p class="photo-editor-export-state" data-photo-editor-export-state role="status"></p><div class="photo-editor-actions"><button type="button" data-photo-editor-export-submit>Export TIFF</button><button type="button" class="quiet" data-photo-editor-export-cancel hidden>Cancel</button><button type="button" class="quiet" data-photo-editor-export-retry hidden>Retry</button><button type="button" class="quiet" data-photo-editor-export-download hidden>Download</button></div></div>
                    <p class="photo-editor-status" data-photo-editor-status role="status" aria-live="polite"></p>
                  </div>
                </div>
                <div class="photo-tools-view" id="photo-tools-view-albums" data-photo-tools-view="albums" hidden>
                  <header class="photo-tools-view-header"><h3>Albums</h3><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
                  <div class="membership" data-membership aria-label="Album membership"><div class="membership-facts"><p class="membership-heading">Albums</p><p class="membership-status" data-membership-status role="status">Loading Albums…</p><ul class="membership-list" data-membership-list hidden></ul><p class="membership-message" data-membership-message role="alert" hidden></p><div class="membership-actions"><button type="button" class="quiet" data-membership-manage aria-expanded="false" aria-controls="membership-panel">Manage</button><button type="button" data-membership-retry hidden>Retry Albums</button></div></div><div class="membership-panel" id="membership-panel" data-membership-panel hidden><div class="membership-options" data-membership-options></div></div></div>
                </div>
                <div class="photo-tools-view" id="photo-tools-view-details" data-photo-tools-view="details" hidden>
                  <header class="photo-tools-view-header"><h3>Details</h3><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
                  <div class="photo-tools-facts" data-metadata aria-label="Capture details"><strong>Capture Details</strong><dl><div><dt>Captured</dt><dd data-metadata-capture-time>—</dd></div><div><dt>Aperture</dt><dd data-metadata-aperture>—</dd></div><div><dt>ISO</dt><dd data-metadata-iso>—</dd></div><div><dt>Shutter</dt><dd data-metadata-shutter-speed>—</dd></div><div><dt>Focal Length</dt><dd data-metadata-focal-length>—</dd></div></dl></div>
                  <p class="photo-tools-preview"><span class="photo-tools-preview-label">Preview Source</span><span data-source>—</span><span class="photo-tools-detail-limit" data-detail-limit hidden>Limited by camera Preview resolution</span></p>
                </div>
                <div class="photo-tools-view" id="photo-tools-view-zoom" data-photo-tools-view="zoom" hidden>
                  <header class="photo-tools-view-header"><h3>Preview Zoom</h3><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
                  <div class="photo-tools-zoom" data-zoom-controls role="group" aria-label="Preview zoom">
                    <button type="button" class="zoom-control" data-zoom-fit aria-pressed="true" aria-label="Fit Window">Fit Window</button>
                    <button type="button" class="zoom-control zoom-step" data-zoom-out aria-label="Zoom out">−</button>
                    <input class="zoom-slider" type="range" data-zoom-slider min="10" max="800" step="1" value="100" aria-label="Zoom percentage" />
                    <button type="button" class="zoom-control zoom-step" data-zoom-in aria-label="Zoom in">+</button>
                    <span class="zoom-level" data-zoom-level>—</span>
                    <button type="button" class="zoom-control" data-zoom-100 aria-label="Zoom to 100 percent">100%</button>
                  </div>
                </div>
                <div class="photo-tools-view" id="photo-tools-view-nearby" data-photo-tools-view="nearby" hidden>
                  <header class="photo-tools-view-header"><h3>Nearby Photos</h3><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
                  <div class="filmstrip-tools" data-filmstrip-tools></div>
                </div>
              </div>
            </div>
          </dialog>
          <dialog class="rating-dialog" id="rating-choices" data-rating-choices aria-labelledby="rating-choices-title">
            <div class="rating-sheet">
              <header class="rating-header"><h2 id="rating-choices-title">Rating</h2><button type="button" class="quiet" data-rating-choices-close>Close</button></header>
              <fieldset class="rating-controls"><legend>Rating</legend><div data-ratings></div></fieldset>
            </div>
          </dialog>
        </section>
        <dialog class="options-dialog" data-view-options aria-labelledby="view-options-title">
          <div class="options-sheet">
            <header class="options-header"><h2 id="view-options-title">View options</h2><button type="button" class="quiet" data-view-options-close>Close</button></header>
            <div class="options-body">
              <p class="options-progress" data-grid-source-progress role="status"></p>
              <p class="options-progress" data-grid-visible-results role="status"></p>
              <div class="options-field"><label for="view-filter-select">Selection State</label><select id="view-filter-select" data-filter-select></select></div>
              <div class="options-field"><label for="view-sort-select">Source order</label><select id="view-sort-select" data-sort-select></select></div>
              <div class="options-field"><label for="view-size-select">Thumbnail size</label><select id="view-size-select" data-size-select></select></div>
              <div class="folder-album-controls" data-folder-album-controls hidden><label for="folder-album-select">Add Folder to</label><select id="folder-album-select" data-folder-album-select></select><button type="button" data-add-folder-to-album>Add Folder</button><p data-folder-album-status role="status" aria-live="polite"></p></div>
              <div class="options-source-actions" data-options-source-actions><button type="button" data-album-resume hidden>Resume</button><button type="button" data-refresh>Refresh Current Source</button></div>
            </div>
            <footer class="options-footer"><button type="button" data-view-options-apply>Apply</button><button type="button" class="quiet" data-view-options-cancel>Cancel</button></footer>
          </div>
        </dialog>
        <dialog class="album-dialog" data-album-form-dialog aria-labelledby="album-form-title">
          <div class="album-dialog-sheet" data-album-form-body></div>
        </dialog>
        <dialog class="recovery-dialog" data-recovery-panel aria-labelledby="recovery-title">
          <div class="recovery-sheet">
            <header class="recovery-header"><h3 id="recovery-title">Review unavailable originals</h3><button type="button" class="quiet" data-recovery-close>Close</button></header>
          <p class="recovery-summary" data-recovery-summary role="status"></p>
          <ul class="recovery-list" data-recovery-list></ul>
          <div class="recovery-forms">
            <div class="recovery-batch">
              <label>Old folder prefix<input data-recovery-old-prefix type="text" autocomplete="off" spellcheck="false" placeholder="2023/travel" /></label>
              <label>New folder prefix<input data-recovery-new-prefix type="text" autocomplete="off" spellcheck="false" placeholder="2024/travel" /></label>
              <button type="button" data-recovery-propose>Propose batch mappings</button>
            </div>
            <div class="recovery-single">
              <label>Single Original<select data-recovery-single-original></select></label>
              <label>New location<input data-recovery-single-location type="text" autocomplete="off" spellcheck="false" placeholder="2024/travel/renamed.ARW" /></label>
              <button type="button" data-recovery-propose-single>Propose this mapping</button>
            </div>
          </div>
          <p class="recovery-note" data-recovery-note hidden>Old content cannot be verified for Originals without a fingerprint. Review every mapping and confirm explicitly.</p>
          <ul class="recovery-proposals" data-recovery-proposals hidden></ul>
          <div class="recovery-actions"><button type="button" data-recovery-apply hidden>Apply mappings</button></div>
          <p class="recovery-message" data-recovery-message role="alert" hidden></p>
          </div>
        </dialog>
        <dialog class="removal-dialog" data-removal-review aria-labelledby="removal-title">
          <div class="removal-sheet">
            <header class="removal-header"><h3 id="removal-title">Remove rejected Photos</h3><button type="button" class="quiet" data-removal-close>Close</button></header>
            <div class="removal-body">
              <p class="removal-summary" data-removal-summary></p>
              <p class="removal-message" data-removal-message role="alert" hidden></p>
            </div>
            <footer class="removal-actions"><button type="button" data-removal-confirm>Remove from Library</button><button type="button" class="quiet" data-removal-undo hidden>Undo</button></footer>
          </div>
        </dialog>
        <dialog class="removed-dialog" data-removed-panel aria-labelledby="removed-title">
          <div class="removed-sheet">
            <header class="removed-header"><h3 id="removed-title">Removed Photos</h3><button type="button" class="quiet" data-removed-close>Close</button></header>
            <p class="removed-status" data-removed-status role="status"></p>
            <ul class="removed-list" data-removed-list></ul>
            <p class="removed-message" data-removed-message role="alert" hidden></p>
            <footer class="removed-pager"><button type="button" class="quiet" data-removed-previous>Previous</button><span data-removed-page></span><button type="button" class="quiet" data-removed-next>Next</button><button type="button" data-removed-retry hidden>Retry</button></footer><footer class="removed-actions"><button type="button" class="quiet" data-removed-undo hidden>Undo the last removal</button></footer>
          </div>
        </dialog>
      </section>
    </div>`;

  const browser = required<HTMLElement>(root, "[data-browser]");
  const connection = required<HTMLElement>(root, "[data-connection]");
  const sourceDialog = required<HTMLDialogElement>(
    root,
    "[data-source-dialog]",
  );
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
  const gridConnection = required<HTMLElement>(root, "[data-grid-connection]");
  const photoConnection = required<HTMLElement>(
    root,
    "[data-photo-connection]",
  );
  const compactTitle = required<HTMLElement>(root, "[data-grid-compact-title]");
  const viewOptionsDialog = required<HTMLDialogElement>(
    root,
    "[data-view-options]",
  );
  const viewOptionsOpen = required<HTMLButtonElement>(
    root,
    "[data-grid-view-options]",
  );
  const viewOptionsFlag = required<HTMLElement>(
    root,
    "[data-view-options-flag]",
  );
  const viewOptionsClose = required<HTMLButtonElement>(
    root,
    "[data-view-options-close]",
  );
  const viewOptionsApply = required<HTMLButtonElement>(
    root,
    "[data-view-options-apply]",
  );
  const viewOptionsCancel = required<HTMLButtonElement>(
    root,
    "[data-view-options-cancel]",
  );
  const albumFormDialog = required<HTMLDialogElement>(
    root,
    "[data-album-form-dialog]",
  );
  const albumFormBody = required<HTMLElement>(root, "[data-album-form-body]");
  const albumResume = required<HTMLButtonElement>(root, "[data-album-resume]");
  const summaryStatus = required<HTMLElement>(root, "[data-summary-status]");
  const recoveryNotice = required<HTMLElement>(root, "[data-recovery-notice]");
  const recoveryPanel = required<HTMLDialogElement>(
    root,
    "[data-recovery-panel]",
  );
  const recoverySummary = required<HTMLElement>(
    root,
    "[data-recovery-summary]",
  );
  const recoveryList = required<HTMLElement>(root, "[data-recovery-list]");
  const recoveryOldPrefix = required<HTMLInputElement>(
    root,
    "[data-recovery-old-prefix]",
  );
  const recoveryNewPrefix = required<HTMLInputElement>(
    root,
    "[data-recovery-new-prefix]",
  );
  const recoveryPropose = required<HTMLButtonElement>(
    root,
    "[data-recovery-propose]",
  );
  const recoverySingleOriginal = required<HTMLSelectElement>(
    root,
    "[data-recovery-single-original]",
  );
  const recoverySingleLocation = required<HTMLInputElement>(
    root,
    "[data-recovery-single-location]",
  );
  const recoveryProposeSingle = required<HTMLButtonElement>(
    root,
    "[data-recovery-propose-single]",
  );
  const recoveryNote = required<HTMLElement>(root, "[data-recovery-note]");
  const recoveryProposalList = required<HTMLElement>(
    root,
    "[data-recovery-proposals]",
  );
  const recoveryApply = required<HTMLButtonElement>(
    root,
    "[data-recovery-apply]",
  );
  const recoveryMessage = required<HTMLElement>(
    root,
    "[data-recovery-message]",
  );
  const recoveryClose = required<HTMLButtonElement>(
    root,
    "[data-recovery-close]",
  );
  const removalOpen = required<HTMLButtonElement>(root, "[data-removal-open]");
  const removalDialog = required<HTMLDialogElement>(
    root,
    "[data-removal-review]",
  );
  const removalSummary = required<HTMLElement>(root, "[data-removal-summary]");
  const removalMessage = required<HTMLElement>(root, "[data-removal-message]");
  const removalConfirm = required<HTMLButtonElement>(
    root,
    "[data-removal-confirm]",
  );
  const removalUndo = required<HTMLButtonElement>(root, "[data-removal-undo]");
  const removalClose = required<HTMLButtonElement>(
    root,
    "[data-removal-close]",
  );
  const removedOpen = required<HTMLButtonElement>(root, "[data-removed-open]");
  const removedPanel = required<HTMLDialogElement>(
    root,
    "[data-removed-panel]",
  );
  const removedStatus = required<HTMLElement>(root, "[data-removed-status]");
  const removedList = required<HTMLElement>(root, "[data-removed-list]");
  const removedMessage = required<HTMLElement>(root, "[data-removed-message]");
  const removedPrevious = required<HTMLButtonElement>(
    root,
    "[data-removed-previous]",
  );
  const removedNext = required<HTMLButtonElement>(root, "[data-removed-next]");
  const removedPage = required<HTMLElement>(root, "[data-removed-page]");
  const removedRetry = required<HTMLButtonElement>(
    root,
    "[data-removed-retry]",
  );
  const removedClose = required<HTMLButtonElement>(
    root,
    "[data-removed-close]",
  );
  const removedUndo = required<HTMLButtonElement>(root, "[data-removed-undo]");
  /// Thumbnails the Removed Photos listing attached. They are released when
  /// the listing re-renders or closes, so the page's image delivery holds only
  /// the rows that are actually presented.
  let removedListBindings: ReadonlyArray<GridThumbnailBinding> = [];
  const releaseRemovedRows = () => {
    for (const binding of removedListBindings) releaseThumbnail(binding);
    removedListBindings = [];
    removedList.replaceChildren();
  };
  let recoveryCurrentProposals: ReadonlyArray<RecoveryProposalViewModel> = [];
  const recoveryRetireSelection = new Map<string, boolean>();
  const recoveryOutcomeLabel = (
    outcome: RecoveryProposalViewModel["outcome"],
  ): string =>
    ({
      matched: "Ready to recover",
      "content-mismatch": "Content differs from the remembered fingerprint",
      missing: "No file at the destination",
      "kind-mismatch": "Destination format differs",
      unreadable: "Destination cannot be read",
      occupied: "Destination already holds another Photo",
      colliding: "Another mapping targets this destination",
    })[outcome];
  const updateRecoveryApply = (): void => {
    const applicable = recoveryCurrentProposals.filter(
      (proposal) =>
        proposal.outcome === "matched" ||
        (proposal.outcome === "occupied" &&
          proposal.retire &&
          recoveryRetireSelection.get(proposal.originalId)),
    );
    recoveryApply.textContent =
      applicable.length === 1
        ? "Apply 1 mapping"
        : `Apply ${applicable.length} mappings`;
    recoveryApply.hidden = applicable.length === 0;
  };
  const sourceList = required<HTMLElement>(root, "[data-source-list]");
  const retry = required<HTMLButtonElement>(root, "[data-retry]");
  const signOut = required<HTMLButtonElement>(root, "[data-access-sign-out]");
  const refresh = required<HTMLButtonElement>(root, "[data-refresh]");
  const gridView = required<HTMLElement>(root, "[data-grid-view]");
  const gridTitle = required<HTMLElement>(root, "[data-grid-title]");
  const gridStatus = required<HTMLElement>(root, "[data-grid-status]");
  const gridSelectMode = required<HTMLButtonElement>(
    root,
    "[data-grid-select-mode]",
  );
  const gridTools = required<HTMLElement>(root, "[data-grid-tools]");
  /// Presents the connection state. A wide layout keeps it in the application
  /// header. A narrow layout has no dedicated brand or connected-status row,
  /// so the normal state shows no permanent connection text at all and the
  /// open view's own header carries the state beside its primary actions only
  /// while it is a failure. Exactly one indicator holds the text at a time.
  let connectionState = true;
  const presentConnection = () => {
    const failed = !connectionState;
    const state = failed ? "Disconnected" : "Connected";
    gridConnection.classList.toggle("offline", failed);
    photoConnection.classList.toggle("offline", failed);
    // A compact layout has no dedicated brand or connected-status row. A short
    // viewport gives its whole height to the open Photo View, so the Photo
    // header carries the state there exactly as a narrow layout does.
    const compactChrome =
      compactSources.matches || (!photoView.hidden && shortViewport.matches);
    if (!compactChrome) {
      connection.textContent = state;
      connection.classList.toggle("offline", failed);
      gridConnection.textContent = "";
      gridConnection.hidden = true;
      photoConnection.textContent = "";
      photoConnection.hidden = true;
      return;
    }
    connection.textContent = "";
    // A narrow normal state presents no connection text: the compact header
    // keeps its row for the source, the count, and the two tool entries.
    const inPhoto = !photoView.hidden;
    gridConnection.textContent = inPhoto || !failed ? "" : state;
    gridConnection.hidden = inPhoto || !failed;
    photoConnection.textContent = !inPhoto || !failed ? "" : state;
    photoConnection.hidden = !inPhoto || !failed;
  };
  /// Presents the current source in both views. The narrow disclosure's
  /// accessible name identifies both Sources and the current source, so a
  /// screen reader names the destination the control opens, while the visible
  /// label stays the truncated source name.
  const presentSourceTitle = (name: string) => {
    gridTitle.textContent = name;
    photoTitle.textContent = name;
    compactTitle.textContent = name;
    sourceToggle.setAttribute("aria-label", `Sources — ${name}`);
  };
  const gridSelection = required<HTMLElement>(root, "[data-grid-selection]");
  const multiDone = required<HTMLButtonElement>(root, "[data-grid-multi-done]");
  const gridBatch = required<HTMLElement>(root, "[data-grid-batch]");
  const batchCount = required<HTMLElement>(root, "[data-batch-count]");
  const batchRetained = required<HTMLElement>(root, "[data-batch-retained]");
  const batchResult = required<HTMLElement>(root, "[data-grid-batch-result]");
  const batchResultText = required<HTMLElement>(
    root,
    "[data-grid-batch-result-text]",
  );
  const batchCompensate = required<HTMLButtonElement>(
    root,
    "[data-grid-batch-compensate]",
  );
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
  const sortSelect = required<HTMLSelectElement>(root, "[data-sort-select]");
  const optionsProgress = required<HTMLElement>(
    root,
    "[data-grid-source-progress]",
  );
  const optionsVisibleResults = required<HTMLElement>(
    root,
    "[data-grid-visible-results]",
  );
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
  const ratingWheel = required<HTMLElement>(root, "[data-rating-wheel]");
  const ratingWheelInstructions = required<HTMLElement>(
    root,
    "[data-rating-wheel-instructions]",
  );
  const ratingWheelStatus = required<HTMLElement>(
    root,
    "[data-rating-wheel-status]",
  );
  const ratingWheelOptions = required<HTMLElement>(
    root,
    "[data-rating-wheel-options]",
  );
  const zoomControls = required<HTMLElement>(root, "[data-zoom-controls]");
  const zoomFit = required<HTMLButtonElement>(root, "[data-zoom-fit]");
  const zoomOut = required<HTMLButtonElement>(root, "[data-zoom-out]");
  const zoomIn = required<HTMLButtonElement>(root, "[data-zoom-in]");
  const zoomSlider = required<HTMLInputElement>(root, "[data-zoom-slider]");
  const zoomLevel = required<HTMLElement>(root, "[data-zoom-level]");
  const zoom100 = required<HTMLButtonElement>(root, "[data-zoom-100]");
  const selection = required<HTMLElement>(root, "[data-selection]");
  const filmstrip = required<HTMLElement>(root, "[data-filmstrip]");
  /// The two homes of the bounded neighbor strip: the wide Photo View shows it
  /// beside the Preview, and a compact layout discloses it inside Photo tools.
  /// One strip node moves between them, so no layout holds a second copy.
  const filmstripHost = required<HTMLElement>(root, "[data-filmstrip-host]");
  const filmstripTools = required<HTMLElement>(root, "[data-filmstrip-tools]");
  const photoFilename = required<HTMLElement>(root, "[data-photo-filename]");
  const rating = required<HTMLElement>(root, "[data-rating]");
  const previewSource = required<HTMLElement>(root, "[data-source]");
  const detailLimit = required<HTMLElement>(root, "[data-detail-limit]");
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
  const dockPrevious = required<HTMLButtonElement>(
    root,
    "[data-dock-previous]",
  );
  const dockReject = required<HTMLButtonElement>(root, "[data-dock-reject]");
  const dockRating = required<HTMLButtonElement>(root, "[data-dock-rating]");
  const dockSelect = required<HTMLButtonElement>(root, "[data-dock-select]");
  const dockMore = required<HTMLButtonElement>(root, "[data-dock-more]");
  const dockNext = required<HTMLButtonElement>(root, "[data-dock-next]");
  const photoToolsDialog = required<HTMLDialogElement>(
    root,
    "[data-photo-tools]",
  );
  const photoToolsTitle = required<HTMLElement>(
    root,
    "[data-photo-tools-title]",
  );
  const photoToolsClose = required<HTMLButtonElement>(
    root,
    "[data-photo-tools-close]",
  );
  const photoToolsClear = required<HTMLButtonElement>(
    root,
    "[data-photo-tools-clear]",
  );
  const photoToolsUndo = required<HTMLButtonElement>(
    root,
    "[data-photo-tools-undo]",
  );
  const photoToolsEntries = required<HTMLElement>(
    root,
    "[data-photo-tools-entries]",
  );
  const photoToolsViews = Array.from(
    root.querySelectorAll<HTMLElement>("[data-photo-tools-view]"),
  );
  const photoToolsReturnButtons = Array.from(
    root.querySelectorAll<HTMLButtonElement>("[data-photo-tools-return]"),
  );
  const editorExposure = required<HTMLInputElement>(
    root,
    "[data-photo-editor-exposure]",
  );
  const editorExposureValue = required<HTMLOutputElement>(
    root,
    "[data-photo-editor-exposure-value]",
  );
  const editorWhiteBalance = required<HTMLElement>(
    root,
    "[data-photo-editor-white-balance]",
  );
  const editorWhiteBalanceMode = required<HTMLSelectElement>(
    root,
    "[data-photo-editor-white-balance-mode]",
  );
  const editorWhiteBalanceNote = required<HTMLElement>(
    root,
    "[data-photo-editor-white-balance-note]",
  );
  const editorTemperature = required<HTMLInputElement>(
    root,
    "[data-photo-editor-temperature]",
  );
  const editorTemperatureValue = required<HTMLOutputElement>(
    root,
    "[data-photo-editor-temperature-value]",
  );
  const editorTint = required<HTMLInputElement>(
    root,
    "[data-photo-editor-tint]",
  );
  const editorTintValue = required<HTMLOutputElement>(
    root,
    "[data-photo-editor-tint-value]",
  );
  const editorResetExposure = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-reset-exposure]",
  );
  const editorResetWhiteBalance = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-reset-white-balance]",
  );
  const editorStageNote = required<HTMLElement>(
    root,
    "[data-photo-editor-stage-note]",
  );
  const editorSupport = required<HTMLElement>(
    root,
    "[data-photo-editor-support]",
  );
  const editorProcessing = required<HTMLElement>(
    root,
    "[data-photo-editor-processing]",
  );
  const editorCapabilityNote = required<HTMLElement>(
    root,
    "[data-photo-editor-capability]",
  );
  const editorStages = Array.from(
    root.querySelectorAll<HTMLButtonElement>("[data-photo-editor-stage]"),
  );
  const editorProvenance = required<HTMLElement>(
    root,
    "[data-photo-editor-provenance]",
  );
  const editorPreview = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-preview]",
  );
  const editorReset = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-reset]",
  );
  const editorUndo = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-undo]",
  );
  const editorRedo = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-redo]",
  );
  const editorCompare = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-compare]",
  );
  const editorPreviewImage = required<HTMLImageElement>(
    root,
    "[data-photo-editor-preview-image]",
  );
  const editorPreviewNote = required<HTMLElement>(
    root,
    "[data-photo-editor-preview-note]",
  );
  const editorDraftNote = required<HTMLElement>(
    root,
    "[data-photo-editor-draft]",
  );
  const editorConflict = required<HTMLElement>(
    root,
    "[data-photo-editor-conflict]",
  );
  const editorConflictMessage = required<HTMLElement>(
    root,
    "[data-photo-editor-conflict-message]",
  );
  const editorUseSaved = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-use-saved]",
  );
  const editorReapply = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-reapply]",
  );
  const editorDiscardDraft = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-discard-draft]",
  );
  const editorExportState = required<HTMLElement>(
    root,
    "[data-photo-editor-export-state]",
  );
  const editorExportSubmit = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-export-submit]",
  );
  const editorExportCancel = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-export-cancel]",
  );
  const editorExportRetry = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-export-retry]",
  );
  const editorExportDownload = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-export-download]",
  );
  const editorStatus = required<HTMLElement>(
    root,
    "[data-photo-editor-status]",
  );
  const ratingDialog = required<HTMLDialogElement>(
    root,
    "[data-rating-choices]",
  );
  const ratingChoicesClose = required<HTMLButtonElement>(
    root,
    "[data-rating-choices-close]",
  );
  const editorRebind = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-rebind]",
  );
  const editorRefresh = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-refresh]",
  );
  const ratings = required<HTMLElement>(root, "[data-ratings]");
  const selectFeedback = required<HTMLElement>(root, "[data-select-feedback]");
  const rejectFeedback = required<HTMLElement>(root, "[data-reject-feedback]");

  const send = (intent: LibraryBrowserIntent): void => {
    if (alive) emit(intent);
  };

  /// The one page-UI controller for the shared native-modal lifecycle. Every
  /// supporting surface registers its dialog here, so at most one is active
  /// and every dismissal converges on one cleanup path.
  const surfaces = createModalSurfaces();
  /// True while the Sources surface presents as a modal rather than the wide
  /// resizable sidebar: a narrow Grid, or any Photo View.
  const sourcesAreModal = () => compactSources.matches || !photoView.hidden;
  /// The disclosure state of the two Photo View entries mirrors the surface
  /// itself, so a native close request, an explicit Close, a destination
  /// change, and a surface the controller reopens on an invoker's behalf all
  /// leave both entries in the same state.
  const syncSecondarySurface = () => {
    dockMore.setAttribute(
      "aria-expanded",
      String(surfaces.isActive("photo-tools")),
    );
    dockRating.setAttribute(
      "aria-expanded",
      String(surfaces.isActive("rating")),
    );
  };
  surfaces.register("sources", {
    dialog: sourceDialog,
    modal: sourcesAreModal,
  });
  surfaces.register("view-options", {
    dialog: viewOptionsDialog,
    modal: () => true,
  });
  surfaces.register("rating", {
    dialog: ratingDialog,
    modal: () => true,
    onOpen: syncSecondarySurface,
  });
  surfaces.register("photo-tools", {
    dialog: photoToolsDialog,
    modal: () => true,
    onOpen: syncSecondarySurface,
  });
  surfaces.register("album-form", {
    dialog: albumFormDialog,
    modal: () => true,
  });
  surfaces.register("recovery", {
    dialog: recoveryPanel,
    modal: () => true,
  });
  surfaces.register("removal-review", {
    dialog: removalDialog,
    modal: () => true,
  });
  surfaces.register("removed-panel", {
    dialog: removedPanel,
    modal: () => true,
  });

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

    const wheelButton = document.createElement("button");
    wheelButton.type = "button";
    wheelButton.className = "rating-wheel-option";
    wheelButton.dataset.ratingWheelValue = String(value);
    wheelButton.style.setProperty(
      "--wheel-angle",
      `${value * (360 / RATING_WHEEL_OPTION_COUNT)}deg`,
    );
    wheelButton.tabIndex = -1;
    wheelButton.setAttribute(
      "aria-label",
      value === 0
        ? "Clear Rating, 0 stars"
        : `${value} ${value === 1 ? "star" : "stars"}`,
    );
    wheelButton.setAttribute("aria-pressed", "false");
    wheelButton.textContent = value === 0 ? "0" : `${value}★`;
    ratingWheelOptions.append(wheelButton);
  }

  let photoStatusSurface: object = {};
  let sourceModel: SourceListViewModel | undefined;
  let membershipModel: MembershipViewModel | undefined;
  let albumFormCounter = 0;
  let albumForm: AlbumFormState | undefined;
  let albumFocusRequest: AlbumFocusRequest | undefined;
  let gridKeyboardIndex: number | undefined;
  let gridTotal = 0;
  let thumbnailSize: GridThumbnailSize = DEFAULT_GRID_THUMBNAIL_SIZE;
  /// The committed order and filter the page model reports, and the draft
  /// View options holds until an explicit Apply commits it. Closing without
  /// Apply discards the draft, so the committed pair is the only source of
  /// what the Grid presents.
  let committedOrder: ViewSourceOrder = "source-default";
  let committedFilter: ViewSelectionFilter = "all";
  let draftOrder: ViewSourceOrder = "source-default";
  let draftFilter: ViewSelectionFilter = "all";
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
  /// True while the empty-state action belongs to an explained destination
  /// state rather than to an empty source's Library check.
  let gridEmptyExplanation = false;
  let batchAlbumSelection = "";
  // The Grid's multi-selection presentation: the page model owns which Photos
  // are multi-selected, and these mirror the last rendered model so a cell
  // build, a keyboard key, and a merged render all read one state.
  let gridMultiMode = false;
  let gridMultiCount = 0;
  let gridMultiLimit = 0;
  let gridMultiEnabled = false;
  let gridMultiResult: GridBatchResultViewModel | undefined;
  let gridMultiSelected: (index: number) => boolean = () => false;
  /// The cell an explicit focus request names. `focusGridIndex` moves focus to
  /// a cell on purpose — Review focuses the Photos it refreshed — so the next
  /// render focuses it even when another control holds focus. A merged
  /// re-render without such a request only reclaims focus the Grid owns.
  let pendingGridCellFocus: number | undefined;
  /// The Grid's Photo facts for the current render. Restoration asks this for
  /// the stable identity of the top visible Photo, so the view never keeps
  /// Photo facts of its own.
  let gridPhotoLookup: (index: number) => GridPhotoViewModel | undefined = () =>
    undefined;
  /// Presents the multi-selection the page model has just emptied, or one
  /// whose bound the page model reports. A hidden tray clears the markers too,
  /// so a cell never keeps a marker the tray no longer names, and a hidden Grid
  /// keeps its retained DOM: only the visible Grid touches it.
  const resetGridMultiSelection = () => {
    if (!alive) return;
    gridMultiMode = false;
    gridMultiCount = 0;
    gridMultiEnabled = false;
    gridMultiResult = undefined;
    gridMultiSelected = () => false;
    if (gridView.hidden) return;
    renderBatch();
    applyGridMultiSelection();
  };
  // Whether one batch Add to Album is settling: its control stays disabled
  // until the outcome is presented.
  let batchAlbumsPending = false;
  /// The batch-tray control that held keyboard focus when a settling batch
  /// disabled the tray; parked and returned by renderBatch.
  let heldBatchControl: HTMLButtonElement | HTMLSelectElement | null = null;
  // The Album choices the batch tray presents. A hidden tray binds no options,
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
  let renderedSourceProgressText = "";
  let gridInteractionEnabled = false;
  let decisionInteractionEnabled = false;
  let currentRating = 0;
  let ratingWheelHoldTimer: number | undefined;
  let ratingWheelOpen = false;
  let ratingWheelCandidate: number | undefined;
  let ratingWheelCenter: Readonly<{ x: number; y: number }> | undefined;
  let ratingWheelRadius = RATING_WHEEL_RADIUS;
  let pointer:
    | {
        id: number;
        startX: number;
        startY: number;
        lastX: number;
        lastY: number;
        startedAt: number;
        vertical: boolean;
        ratingPending: boolean;
        ratingWheel: boolean;
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
  const mobileActionHierarchy = window.matchMedia(
    "(max-width: 760px), (max-height: 480px)",
  );
  /// True while the bounded neighbor strip is disclosed inside Photo tools
  /// rather than shown beside the Preview: a narrow or short layout moves it
  /// behind the Nearby Photos entry so it costs the Preview no space.
  const stripIsDisclosed = () => mobileActionHierarchy.matches;
  // The CSS yields the strip on a short viewport so the Preview and the
  // decision controls keep their space; the view mirrors that condition so a
  // hidden strip binds no thumbnails and rebuilds when the space returns.
  const shortViewport = window.matchMedia("(max-height: 480px)");
  /// Reflects which layout the Sources surface uses: the wide resizable
  /// sidebar keeps it in the Grid, and a narrow or Photo View layout opens it
  /// as one native modal surface.
  const syncSourceLayout = () => {
    const modal = sourcesAreModal();
    browser.classList.toggle("sources-drawer", modal);
    sourceToggle.hidden = !modal;
    photoSourceToggle.hidden = !modal;
    if (!modal) closeSources(false);
  };
  /// Which Photo tools content the modal presents. Every subview replaces the
  /// tools list and carries its own local return, so moving between them adds
  /// no browser history entry and never opens a second modal.
  type PhotoToolsView =
    | "tools"
    | "edit"
    | "albums"
    | "details"
    | "zoom"
    | "nearby";
  let photoToolsView: PhotoToolsView = "tools";
  /// The last neighbor facts the page model reported. A disclosure rebuilds
  /// the strip from them, so a closed strip holds no image demand at all.
  let filmstripModel: FilmstripViewModel | undefined;
  let editorPhotoId: string | undefined;
  let editorModel: EditorViewModel | undefined;
  /// The exposure a live gesture is showing, and the model value it is drawn
  /// over. The draft covers the window between a pointer drag and the write
  /// that follows it, and it is dropped as soon as the model reaches it or
  /// moves anywhere else: an undo, reset, or conflict adoption renders the
  /// settings it actually restored.
  let editorDraft: number | undefined;
  let editorDraftBase: number | undefined;
  const applyPhotoToolsView = () => {
    for (const view of photoToolsViews)
      view.hidden = view.dataset.photoToolsView !== photoToolsView;
    // A wide layout docks the Edit surface beside the image instead of
    // centring it over the Preview, so the controls and the image they change
    // are both visible.
    photoToolsDialog.dataset.photoToolsSurface = photoToolsView;
    photoToolsTitle.textContent =
      photoToolsView === "tools"
        ? "Photo tools"
        : photoToolsView === "nearby"
          ? "Nearby Photos"
          : photoToolsView === "zoom"
            ? "Preview Zoom"
            : photoToolsView === "albums"
              ? "Albums"
              : photoToolsView === "details"
                ? "Details"
                : "Edit";
  };
  const openEditor = (photoId: string) => {
    if (!photoId || photoView.hidden) return;
    editorPhotoId = photoId;
    editorDraft = undefined;
    editorDraftBase = undefined;
    editorModel = undefined;
    renderEditor({
      photoId,
      loading: true,
      stage: "develop",
      stageNote: "Develop: loading this Photo's edit facts.",
      filmReason: "",
      sourceSupport: "unknown",
      processingAvailable: false,
      capabilityNote: "",
      exposureEv: 0,
      savedExposureEv: 0,
      baselineExposureEv: 0,
      exposureMinimumEv: 0,
      exposureMaximumEv: 1,
      exposureStepEv: 0.001,
      whiteBalance: loadingWhiteBalance(),
      canEdit: false,
      canPreview: false,
      previewing: false,
      previewNote: "",
      previewStale: false,
      saving: false,
      dirty: false,
      canUndo: false,
      canRedo: false,
      comparing: false,
      conflict: null,
      draftNote: "",
      export: {
        state: "idle",
        note: "",
        artifact: null,
        canSubmit: false,
        canCancel: false,
        canRetry: false,
        canDownload: false,
      },
      status: "Loading edit recipe…",
    });
    send({ kind: "editor-open", photoId });
    openPhotoTools("edit");
  };
  const editorVisible = () =>
    alive &&
    !photoView.hidden &&
    surfaces.isActive("photo-tools") &&
    photoToolsView === "edit";
  const renderEditor = (model: EditorViewModel) => {
    if (
      !alive ||
      (editorPhotoId !== undefined && editorPhotoId !== model.photoId)
    )
      return;
    editorPhotoId = model.photoId;
    editorModel = model;
    const minimum = Number.isFinite(model.exposureMinimumEv)
      ? model.exposureMinimumEv
      : 0;
    const maximum = Number.isFinite(model.exposureMaximumEv)
      ? model.exposureMaximumEv
      : 1;
    const step =
      Number.isFinite(model.exposureStepEv) && model.exposureStepEv > 0
        ? model.exposureStepEv
        : 0.001;
    if (
      editorDraft !== undefined &&
      (model.exposureEv === editorDraft || model.exposureEv !== editorDraftBase)
    ) {
      editorDraft = undefined;
      editorDraftBase = undefined;
    }
    const exposure = editorDraft ?? model.exposureEv;
    editorExposure.min = String(minimum);
    editorExposure.max = String(maximum);
    editorExposure.step = String(step);
    editorExposure.value = String(exposure);
    const label = `${exposure.toFixed(3)} EV`;
    editorExposureValue.value = label;
    editorExposureValue.textContent = label;
    editorExposure.disabled = model.loading || !model.canEdit;
    editorWhiteBalance.textContent = describeWhiteBalance(
      model.whiteBalance.intent,
    );
    const whiteBalance = model.whiteBalance;
    const adjustableOption =
      editorWhiteBalanceMode.querySelector<HTMLOptionElement>(
        'option[value="temperature-tint"]',
      );
    if (adjustableOption) adjustableOption.disabled = !whiteBalance.adjustable;
    editorWhiteBalanceMode.value = whiteBalance.intent.mode;
    editorWhiteBalanceMode.disabled =
      model.loading || !model.canEdit || !whiteBalance.adjustable;
    editorWhiteBalanceNote.textContent = whiteBalance.note;
    editorWhiteBalanceNote.hidden = !whiteBalance.note;
    const temperature = whiteBalance.temperatureKelvin;
    if (temperature) {
      editorTemperature.min = String(temperature.minimum);
      editorTemperature.max = String(temperature.maximum);
      editorTemperature.value = String(temperature.value);
      editorTemperature.disabled = model.loading || !temperature.enabled;
      const label = `${temperature.value} K`;
      editorTemperatureValue.value = label;
      editorTemperatureValue.textContent = label;
    } else {
      editorTemperature.disabled = true;
      editorTemperatureValue.value = "—";
      editorTemperatureValue.textContent = "—";
    }
    const tint = whiteBalance.tintMilli;
    if (tint) {
      editorTint.min = String(tint.minimum);
      editorTint.max = String(tint.maximum);
      editorTint.value = String(tint.value);
      editorTint.disabled = model.loading || !tint.enabled;
      const label = `${tint.value}`;
      editorTintValue.value = label;
      editorTintValue.textContent = label;
    } else {
      editorTint.disabled = true;
      editorTintValue.value = "—";
      editorTintValue.textContent = "—";
    }
    editorResetExposure.disabled =
      model.loading ||
      !model.canEdit ||
      Math.abs(exposure - model.baselineExposureEv) < step / 2;
    editorResetWhiteBalance.disabled =
      model.loading || !model.canEdit || !whiteBalance.resettable;
    editorSupport.textContent = model.sourceSupport;
    editorProcessing.textContent = model.loading
      ? "Checking…"
      : model.processingAvailable
        ? "Available"
        : "Unavailable";
    editorCapabilityNote.textContent = model.capabilityNote;
    editorCapabilityNote.hidden = !model.capabilityNote;
    for (const button of editorStages) {
      const stage = button.dataset.photoEditorStage as EditorStage | undefined;
      button.setAttribute("aria-pressed", String(stage === model.stage));
      button.disabled = stage === "film" && Boolean(model.filmReason);
      if (stage === "film") {
        button.title = model.filmReason;
        if (model.filmReason)
          button.setAttribute("aria-describedby", editorStageNote.id);
        else button.removeAttribute("aria-describedby");
      }
    }
    editorStageNote.textContent = model.filmReason;
    editorStageNote.hidden = !model.filmReason;
    editorProvenance.textContent = model.stageNote;
    // Reset all restores the processing baseline of both controls, so it is
    // enabled exactly while the settings are away from that baseline. Local
    // changes are discarded by Undo, never by a disabled Reset all.
    const atBaseline =
      Math.abs(exposure - model.baselineExposureEv) < step / 2 &&
      !model.whiteBalance.resettable;
    editorUndo.disabled = model.loading || !model.canUndo;
    editorRedo.disabled = model.loading || !model.canRedo;
    editorReset.disabled = model.loading || model.saving || atBaseline;
    // The comparison compares a stage's baseline development with the current
    // settings; the Camera stage presents the camera Preview itself, so it
    // offers no comparison of its own.
    editorCompare.disabled =
      model.loading || !model.canPreview || model.stage === "camera";
    editorCompare.setAttribute("aria-pressed", String(model.comparing));
    editorPreview.disabled =
      model.loading || model.previewing || !model.canPreview;
    editorRebind.hidden = !model.conflict;
    editorRebind.disabled = model.loading || model.saving || !model.conflict;
    editorRefresh.disabled = model.loading || model.saving;
    editorDraftNote.textContent = model.draftNote;
    editorDraftNote.hidden = !model.draftNote;
    editorConflict.hidden = !model.conflict;
    editorConflictMessage.textContent = model.conflict?.message ?? "";
    editorUseSaved.disabled = model.saving;
    editorReapply.disabled = model.saving;
    editorDiscardDraft.disabled = model.saving;
    const exported = model.export;
    editorExportState.textContent = exported.note;
    editorExportSubmit.disabled =
      model.loading || !model.canEdit || !exported.canSubmit;
    editorExportCancel.hidden = !exported.canCancel;
    editorExportRetry.hidden = !exported.canRetry;
    editorExportDownload.hidden = !exported.canDownload;
    // The Edit Preview note describes the Develop or Film rendition. The
    // Camera stage presents the camera Preview, which is not an Edit Preview.
    const editPreviewStage = model.stage !== "camera";
    editorPreviewNote.textContent = editPreviewStage ? model.previewNote : "";
    editorPreviewNote.hidden = !editPreviewStage || !model.previewNote;
    editorPreviewNote.dataset.tone = model.previewStale ? "stale" : "";
    editorStatus.textContent = model.status;
    editorStatus.dataset.tone = model.status ? "notice" : "";
  };
  const presentEditorPreview = (url: string): void => {
    if (!alive || !editorVisible() || !editorPhotoId) return;
    editorPreviewImage.src = new URL(url, window.location.href).href;
    editorPreviewImage.hidden = false;
  };
  const clearEditorPreview = (): void => {
    if (!alive) return;
    editorPreviewImage.hidden = true;
    editorPreviewImage.removeAttribute("src");
  };
  /// True while the Nearby Photos subview is the presented content of Photo
  /// tools. The one strip node then lives inside that disclosure; otherwise it
  /// lives beside the Preview, which only a wide layout shows.
  const stripInTools = () =>
    photoToolsView === "nearby" && surfaces.isActive("photo-tools");
  /// Places the one strip node where it is presented, and releases it
  /// everywhere else: a strip that is not presented binds no thumbnail and
  /// admits no window of its own.
  const syncFilmstripHost = () => {
    const presented = !stripIsDisclosed() || stripInTools();
    const host = stripInTools() ? filmstripTools : filmstripHost;
    if (filmstrip.parentElement !== host) host.append(filmstrip);
    if (!presented) {
      clearFilmstripCells();
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
      renderFilmstrip(filmstripModel);
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
  /// Opens Photo tools on its list, or moves it to one subview. A pending
  /// gesture never competes with the surface that takes over the pointer and
  /// the keyboard.
  const openPhotoTools = (view: PhotoToolsView = "tools") => {
    if (!alive || photoView.hidden) return;
    resetGestures();
    photoToolsView = view;
    applyPhotoToolsView();
    // A move between subviews keeps the invoker that opened the surface, so
    // closing it still returns focus to the More entry rather than to a
    // control the modal itself holds.
    if (!surfaces.isActive("photo-tools"))
      surfaces.open(
        "photo-tools",
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : undefined,
      );
    syncFilmstripHost();
    syncSecondarySurface();
    photoToolsClose.focus();
  };
  /// Leaves a Photo tools subview for its list, which is a local return and
  /// not a browser history entry.
  const returnToPhotoTools = () => {
    if (!alive || !surfaces.isActive("photo-tools")) return;
    photoToolsView = "tools";
    applyPhotoToolsView();
    syncFilmstripHost();
    photoToolsClose.focus();
  };
  const closePhotoTools = (restoreFocus = true) => {
    if (!alive) return;
    photoToolsView = "tools";
    applyPhotoToolsView();
    surfaces.close("photo-tools", restoreFocus);
    syncFilmstripHost();
    syncSecondarySurface();
  };
  /// Opens the explicit Rating choices. Only this surface or the Rating entry
  /// owns explicit Rating interaction at one time; the Rating Wheel stays the
  /// touch accelerator on the Preview.
  const openRatingChoices = () => {
    if (!alive || photoView.hidden) return;
    resetGestures();
    surfaces.open(
      "rating",
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : undefined,
    );
    syncSecondarySurface();
    const current = ratings.querySelector<HTMLButtonElement>(
      `[data-rating-value="${currentRating}"]`,
    );
    (current ?? ratings.querySelector<HTMLButtonElement>("button"))?.focus();
  };
  const closeRatingChoices = (restoreFocus = true) => {
    if (!alive) return;
    surfaces.close("rating", restoreFocus);
    syncSecondarySurface();
  };
  /// The disclosure's expanded state always mirrors the surface itself, so a
  /// native close request and an explicit Close leave it in the same state.
  const syncSourcesExpanded = () => {
    const expanded = sourceDialog.open;
    sourceToggle.setAttribute("aria-expanded", String(expanded));
    photoSourceToggle.setAttribute("aria-expanded", String(expanded));
  };
  /// Opens the Sources surface. A wide Grid already shows it as the resizable
  /// sidebar, so only a narrow Grid or a Photo View opens it as a modal.
  const openSources = () => {
    if (!alive || !sourcesAreModal()) return;
    // The invoker is read before any surface closes. Closing the surface that
    // holds the activating control moves focus out of it, and the invoker is
    // what closing this surface returns focus to — reopening the surface that
    // holds it when that surface had to close for this one.
    const invoker =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : undefined;
    // A pending gesture must never compete with the surface that takes over
    // the pointer and the keyboard.
    resetGestures();
    closePhotoTools(false);
    closeRatingChoices(false);
    surfaces.open("sources", invoker);
    syncSourceLayout();
    syncSourcesExpanded();
    sourceClose.focus();
  };
  /// Closes the Sources surface, returning focus to the disclosure that opened
  /// it unless the caller names a different owner.
  const closeSources = (restoreFocus = true) => {
    if (!alive) return;
    surfaces.close("sources", restoreFocus);
    syncSourcesExpanded();
  };
  const onSourceViewportChange = () => {
    if (!alive) return;
    syncSourcesExpanded();
    syncSourceLayout();
    presentConnection();
    syncSecondarySurface();
    syncFilmstripHost();
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
  /// The normal header's nondefault indication. It names the active filter
  /// and order in text, so the state never depends on color alone and is
  /// never inferred from the loaded cells. The flag is part of the entry's
  /// accessible name, so a screen reader hears the committed choices with the
  /// control that opens them.
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
  /// Opens View options with the committed choices as its draft, so an
  /// unapplied change can be discarded without touching the open Grid.
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
  /// Commits the draft once. A size-only change keeps the Snapshot and its
  /// anchor, because it changes cell geometry alone; a combined order and
  /// filter opens one view with both choices.
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

  const ratingLabel = (value: number) =>
    value === 0 ? "Clear Rating" : `${value} ${value === 1 ? "star" : "stars"}`;
  const syncRatingWheelOptions = () => {
    for (const button of Array.from(
      ratingWheelOptions.querySelectorAll<HTMLButtonElement>(
        "[data-rating-wheel-value]",
      ),
    )) {
      const value = Number(button.dataset.ratingWheelValue);
      button.dataset.current = String(value === currentRating);
      button.setAttribute(
        "aria-current",
        value === currentRating ? "true" : "false",
      );
      button.setAttribute(
        "aria-pressed",
        String(value === ratingWheelCandidate),
      );
    }
  };
  const setRatingWheelCandidate = (value: number | undefined) => {
    ratingWheelCandidate = value;
    if (value === undefined) {
      ratingWheelStatus.textContent = `Current Rating: ${ratingLabel(currentRating)}. Move across a Rating and release to save.`;
      ratingWheel.dataset.ratingWheelCandidate = "";
    } else {
      ratingWheelStatus.textContent = `${ratingLabel(value)}. Release to save.`;
      ratingWheel.dataset.ratingWheelCandidate = String(value);
    }
    syncRatingWheelOptions();
  };
  const closeRatingWheel = () => {
    ratingWheelOpen = false;
    ratingWheelCandidate = undefined;
    ratingWheelCenter = undefined;
    ratingWheel.hidden = true;
    delete ratingWheel.dataset.ratingWheelCandidate;
    preview.classList.remove("rating-wheel-open");
    preview.style.removeProperty("touch-action");
    ratingWheelStatus.textContent = "";
    syncRatingWheelOptions();
  };
  const openRatingWheel = (clientX: number, clientY: number) => {
    const bounds = preview.getBoundingClientRect();
    const radius = Math.max(
      46,
      Math.min(
        RATING_WHEEL_RADIUS,
        (bounds.width - 96) / 2,
        (bounds.height - 96) / 2,
      ),
    );
    const extent = radius + 28;
    const centerX = clamp(
      clientX - bounds.left,
      Math.min(extent, bounds.width / 2),
      Math.max(bounds.width / 2, bounds.width - extent),
    );
    const centerY = clamp(
      clientY - bounds.top,
      Math.min(extent, bounds.height / 2),
      Math.max(bounds.height / 2, bounds.height - extent),
    );
    ratingWheelRadius = radius;
    ratingWheelCenter = { x: centerX, y: centerY };
    ratingWheel.style.left = `${centerX}px`;
    ratingWheel.style.top = `${centerY}px`;
    ratingWheel.style.setProperty("--rating-wheel-radius", `${radius}px`);
    ratingWheelOpen = true;
    ratingWheel.hidden = false;
    ratingWheelInstructions.textContent =
      "Rating Wheel open. Move across a Rating and release to save.";
    preview.classList.add("rating-wheel-open");
    preview.style.touchAction = "none";
    setRatingWheelCandidate(undefined);
  };
  const updateRatingWheel = (clientX: number, clientY: number) => {
    if (!ratingWheelOpen || !ratingWheelCenter) return;
    const dx =
      clientX - (preview.getBoundingClientRect().left + ratingWheelCenter.x);
    const dy =
      clientY - (preview.getBoundingClientRect().top + ratingWheelCenter.y);
    const distance = Math.hypot(dx, dy);
    const outer = ratingWheelRadius + 56;
    if (distance < 30 || distance > outer) {
      setRatingWheelCandidate(undefined);
      return;
    }
    const degrees = (Math.atan2(dy, dx) * (180 / Math.PI) + 90 + 360) % 360;
    const value =
      Math.floor(
        (degrees + 180 / RATING_WHEEL_OPTION_COUNT) /
          (360 / RATING_WHEEL_OPTION_COUNT),
      ) % RATING_WHEEL_OPTION_COUNT;
    setRatingWheelCandidate(value);
  };
  const cancelRatingHold = () => {
    if (ratingWheelHoldTimer !== undefined) {
      window.clearTimeout(ratingWheelHoldTimer);
      ratingWheelHoldTimer = undefined;
    }
  };
  const clearPointer = () => {
    cancelRatingHold();
    const id = pointer?.id;
    pointer = undefined;
    if (id !== undefined && preview.hasPointerCapture(id))
      preview.releasePointerCapture(id);
    stage.style.transform = "";
    selectFeedback.classList.remove("pending");
    rejectFeedback.classList.remove("pending");
  };
  const resetGestures = () => {
    cancelRatingHold();
    closeRatingWheel();
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
    for (const button of [zoomFit, zoomOut, zoomIn, zoom100])
      button.disabled = !enabled;
    zoomSlider.disabled = !enabled;
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
    for (const id of Array.from(activePointers.keys())) {
      try {
        preview.setPointerCapture(id);
      } catch {
        // Synthetic PointerEvents have no native active pointer to capture.
      }
    }
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
  const onPreviewContextMenu = (event: MouseEvent) => {
    if (pointer?.ratingPending || ratingWheelOpen) event.preventDefault();
  };

  /// The address one source destination resolves to. A source selection
  /// always uses that source's default order and the All filter, so the
  /// address is fully known before any request is made.
  const sourceAddress = (source: SourceReference): string =>
    addressFor(
      source.kind === "library"
        ? { source: "library", selection: "all" }
        : source.kind === "album"
          ? { source: "album", albumId: source.id, selection: "all" }
          : { source: "folder", folderPath: source.location, selection: "all" },
    );

  /// Intercepts only an unmodified primary activation of a destination
  /// anchor. A modified activation, a middle click, or a non-primary button
  /// keeps its native new-tab, copy-link, and download behavior.
  const interceptDestination = (
    element: HTMLAnchorElement | HTMLButtonElement,
    activate: () => void,
  ) => {
    element.addEventListener("click", (event) => {
      const pointer = event as MouseEvent;
      if (
        pointer.defaultPrevented ||
        pointer.button !== 0 ||
        pointer.metaKey ||
        pointer.ctrlKey ||
        pointer.shiftKey ||
        pointer.altKey
      )
        return;
      pointer.preventDefault();
      activate();
    });
  };

  /// One source destination. A destination the Library can open right now is a
  /// real same-origin anchor, so new-tab, copy-link, and download stay native.
  /// A destination that cannot be opened yet — an unpublished Library Folder
  /// root, or a Folder the current publication does not list — keeps button
  /// semantics and no href, so an unmodified activation can never navigate the
  /// document away from the application, and its native disabled state keeps
  /// it out of the hover highlight.
  const createSourceButton = (
    name: string,
    count: number,
    active: boolean,
    address: string,
    openable: boolean,
  ) => {
    const element: HTMLAnchorElement | HTMLButtonElement = openable
      ? document.createElement("a")
      : document.createElement("button");
    if (openable) (element as HTMLAnchorElement).href = address;
    else {
      const button = element as HTMLButtonElement;
      button.type = "button";
      button.disabled = true;
    }
    element.className = `source-card${active ? " active" : ""}`;
    if (active) element.setAttribute("aria-current", "true");
    // The name may be visually truncated; the title keeps the full name
    // available on hover without changing the accessible name.
    element.title = name;
    element.innerHTML = "<strong></strong><span></span>";
    required<HTMLElement>(element, "strong").textContent = name;
    required<HTMLElement>(element, "span").textContent =
      formatPhotoCount(count);
    return element;
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
    // A Folder the current publication does not list is not an openable
    // destination yet, so it keeps button semantics and no href.
    const button = createSourceButton(
      `${folder.name}${folder.hasDescendantFolders ? " · Subfolders" : ""}`,
      folder.photoCount,
      folder.active,
      sourceAddress({
        kind: "folder",
        location: folder.location,
        name: folder.name,
      }),
      folder.enabled,
    );
    interceptDestination(button, () =>
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
  /// The action that opened the Album form. Closing the form returns focus
  /// here, reopening the surface that holds it when that surface presents as
  /// a modal and had to close for the form.
  let albumFormInvoker: HTMLElement | undefined;
  const albumFormTitle = (form: AlbumFormState): string =>
    form.kind === "create"
      ? "Create Album"
      : form.kind === "rename"
        ? "Rename Album"
        : "Delete Album";
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
    return input;
  };
  /// The control the Album form owns: the name field, the delete confirmation,
  /// or Cancel while the write settles and the committing control is disabled.
  const albumFormControl = (
    form: AlbumFormState,
  ): HTMLInputElement | HTMLButtonElement | undefined => {
    const selector =
      form.kind === "delete"
        ? `[data-album-form-id="${form.formId}"][data-focus-key$=":confirm"]`
        : `input[data-album-form-id="${form.formId}"]`;
    const target = albumFormBody.querySelector<HTMLElement>(selector);
    if (target && !target.matches(":disabled")) {
      return target as HTMLInputElement | HTMLButtonElement;
    }
    return albumFormBody.querySelector<HTMLElement>(
      `[data-album-form-id="${form.formId}"][data-focus-key$=":cancel"]`,
    ) as HTMLButtonElement | undefined;
  };
  /// Rebuilds the Album form surface from its own state. Only a change to the
  /// form reaches it, so a background source-list re-render never touches the
  /// draft, and the caret and validation message survive their own rebuilds.
  const renderAlbumForm = () => {
    const form = albumForm;
    if (!form) {
      albumFormBody.replaceChildren();
      return;
    }
    const active = document.activeElement;
    const heldForm =
      active instanceof HTMLElement &&
      active.dataset.albumFormId === form.formId;
    const heldSelection =
      active instanceof HTMLInputElement
        ? [active.selectionStart, active.selectionEnd]
        : undefined;
    const header = document.createElement("header");
    header.className = "album-dialog-header";
    const title = document.createElement("h2");
    title.id = "album-form-title";
    title.textContent = albumFormTitle(form);
    header.append(title);
    albumFormBody.replaceChildren(header);
    if (form.kind === "delete") {
      const confirmBox = document.createElement("div");
      confirmBox.className = "album-confirm";
      confirmBox.setAttribute("role", "alert");
      confirmBox.append(
        paragraph("Photos and Original Files remain unchanged."),
      );
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
      confirmBox.append(confirm, cancel);
      albumFormBody.append(confirmBox);
    } else {
      const element = document.createElement("form");
      element.className = "album-form";
      element.setAttribute("aria-label", albumFormTitle(form));
      const input = albumNameInput(form);
      const message = albumFormMessage();
      message.textContent = form.message ?? "";
      const save = document.createElement("button");
      save.type = "submit";
      save.dataset.albumFormId = form.formId;
      save.dataset.focusKey = `album:form:${form.formId}:submit`;
      save.textContent = form.kind === "create" ? "Create Album" : "Save Name";
      save.disabled = form.pending;
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.dataset.albumFormId = form.formId;
      cancel.dataset.focusKey = `album:form:${form.formId}:cancel`;
      cancel.textContent = "Cancel";
      cancel.addEventListener("click", () => closeAlbumForm(form));
      input.addEventListener("input", () => {
        if (alive && albumForm === form) {
          form.name = input.value;
          delete form.message;
          // Editing clears the validation message where it is presented, so a
          // background refresh never restores a stale one.
          message.textContent = "";
        }
      });
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
      albumFormBody.append(element);
    }
    // A rebuild keeps the control the Photographer was using, including the
    // caret, so a validation message or a pending write never moves focus.
    const request = albumFocusRequest;
    const control = albumFormControl(form);
    if (!control) return;
    if (request?.kind === "form" && request.formId === form.formId) {
      albumFocusRequest = undefined;
      control.focus();
      if (control instanceof HTMLInputElement) control.select();
      return;
    }
    if (heldForm) {
      control.focus();
      if (control instanceof HTMLInputElement && heldSelection) {
        const end = control.value.length;
        control.setSelectionRange(
          Math.min(heldSelection[0] ?? end, end),
          Math.min(heldSelection[1] ?? end, end),
        );
      }
      return;
    }
    // Opening a modal moves focus into the surface.
    control.focus();
    if (control instanceof HTMLInputElement && form.name === "")
      control.select();
  };
  const openAlbumForm = (
    kind: AlbumFormState["kind"],
    albumId = "",
    name = "",
  ) => {
    if (!alive) return;
    albumFormInvoker =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : undefined;
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
    resetGestures();
    surfaces.open("album-form", albumFormInvoker);
    renderAlbumForm();
  };
  /// Closes the Album form and returns focus to the action that opened it, or
  /// to the nearest valid Album action when that row is gone.
  const dismissAlbumFormSurface = (form: AlbumFormState) => {
    const invoker = albumFormInvoker;
    albumFormInvoker = undefined;
    surfaces.close("album-form", false);
    if (invoker?.isConnected && !("disabled" in invoker && invoker.disabled)) {
      surfaces.focus(invoker);
      return;
    }
    const target =
      albumFocusTarget(form.returnFocusKey) ?? albumFocusTarget("album:create");
    if (target) surfaces.focus(target);
  };
  const closeAlbumForm = (form: AlbumFormState) => {
    if (!alive || albumForm !== form) return;
    albumFocusRequest = {
      kind: "return",
      focusKey: form.returnFocusKey,
    };
    albumForm = undefined;
    send({ kind: "album-form-close", formId: form.formId });
    dismissAlbumFormSurface(form);
  };
  const createAlbumTools = (album: SourceListViewModel["albums"][number]) => {
    const tools = document.createElement("div");
    tools.className = "album-tools";
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
    // Resume resolves the saved position under the existing saved-position
    // rules and opens Photo View, so its destination is not an address: it
    // stays an explicit button beside the Album's Grid destination, reachable
    // from any source. View options carries the same action for the open
    // Album; both emit the one album-resume intent.
    if (album.hasSavedPosition) {
      const resume = document.createElement("button");
      resume.type = "button";
      resume.className = "album-tool album-resume";
      resume.textContent = "Resume";
      resume.setAttribute("aria-label", `Resume ${album.name}`);
      resume.addEventListener("click", () =>
        send({ kind: "album-resume", albumId: album.id }),
      );
      tools.append(resume);
    }
    return tools;
  };

  const renderSources = (model: SourceListViewModel) => {
    if (!alive) return;
    sourceModel = model;
    const focused = document.activeElement;
    const focusedKey =
      focused instanceof HTMLElement ? focused.dataset.focusKey : undefined;
    sourceList.replaceChildren();
    const library = createSourceButton(
      "All Photos",
      model.libraryCount,
      model.libraryActive,
      sourceAddress({ kind: "library" }),
      true,
    );
    library.dataset.focusKey = "source:library";
    interceptDestination(library, () =>
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
    // The Library Folder root opens only while the Library is published.
    const rootCard = createSourceButton(
      "Library Folder",
      model.libraryCount,
      model.rootActive,
      sourceAddress({ kind: "folder", location: "", name: "Library Folder" }),
      model.fileLocationsEnabled,
    );
    rootCard.dataset.focusKey = "source:folder:";
    interceptDestination(rootCard, () =>
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
    // An Album with a saved position exposes Resume separately from opening
    // its Grid: it resolves that position under the existing saved-position
    // rules and opens Photo View, so it lives with the other source-specific
    // actions in View options rather than beside the destination anchor.
    const activeAlbum = model.albums.find((album) => album.active);
    albumResume.hidden = activeAlbum?.hasSavedPosition !== true;
    for (const album of model.albums) {
      const button = createSourceButton(
        album.name,
        album.photoCount,
        album.active,
        sourceAddress({ kind: "album", id: album.id }),
        true,
      );
      button.dataset.focusKey = `source:album:${album.id}`;
      interceptDestination(button, () =>
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
    const request = albumFocusRequest;
    if (request?.kind === "form" && albumForm?.formId === request.formId) {
      albumFocusRequest = undefined;
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
      return;
    }
  };
  /// The Album action a focus return names, searched in the current source
  /// list. Used when the action that opened a form no longer exists.
  const albumFocusTarget = (focusKey: string): HTMLElement | undefined =>
    Array.from(
      sourceList.querySelectorAll<HTMLElement>("[data-focus-key]"),
    ).find((candidate) => candidate.dataset.focusKey === focusKey);

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
    const visible = gridMultiMode || count > 0;
    gridBatch.hidden = !visible;
    // Select mode, and a desktop modifier selection, replace the normal
    // header tools with the source, the count, and Done. A tool the
    // Photographer was using leaves the layout, so focus moves to the control
    // that replaced it rather than to the body.
    gridSelectMode.setAttribute("aria-pressed", String(gridMultiMode));
    const heldToolsFocus = gridTools.contains(document.activeElement);
    const heldSelectionFocus = gridSelection.contains(document.activeElement);
    gridTools.hidden = visible;
    gridSelection.hidden = !visible;
    if (heldToolsFocus && visible) multiDone.focus();
    else if (heldSelectionFocus && !visible) gridSelectMode.focus();
    if (!visible) {
      batchCount.textContent = "";
      batchRetained.hidden = true;
      batchResult.hidden = true;
      batchResultText.textContent = "";
      batchCompensate.hidden = true;
      batchCompensate.disabled = true;
      batchAlbumSelect.replaceChildren();
      renderedBatchAlbumSignature = "";
      batchAlbumSelection = "";
      // A hidden tray names no Photo, so no cell keeps a multi-selection
      // marker beside it.
      applyGridMultiSelection();
      return;
    }

    const countText = `${count.toLocaleString()} / ${gridMultiLimit.toLocaleString()} Photos`;
    const retainedHidden = count === 0 || gridMultiResult === undefined;
    const resultHidden = gridMultiResult === undefined;
    const resultText = gridMultiResult?.message ?? "";
    const compensation = gridMultiResult?.compensation;
    const review = gridMultiResult?.review;
    const resultAction = compensation ?? review;
    const compensationHidden = resultHidden || resultAction === undefined;
    if (batchCount.textContent !== countText)
      batchCount.textContent = countText;
    if (batchRetained.hidden !== retainedHidden)
      batchRetained.hidden = retainedHidden;
    if (batchResult.hidden !== resultHidden) batchResult.hidden = resultHidden;
    if (batchResultText.textContent !== resultText)
      batchResultText.textContent = resultText;
    batchCompensate.hidden = compensationHidden;
    if (resultAction && batchCompensate.textContent !== resultAction.label)
      batchCompensate.textContent = resultAction.label;
    batchCompensate.dataset.action = compensation ? "compensate" : "review";
    if (gridMultiResult) batchResult.dataset.tone = gridMultiResult.tone;
    else batchResult.removeAttribute("data-tone");

    const enabled = gridMultiEnabled && !batchAlbumsPending && count > 0;
    if (count === 0) {
      batchAlbumSelect.replaceChildren();
      renderedBatchAlbumSignature = "";
      batchAlbumSelection = "";
      batchCompensate.hidden = true;
      applyGridMultiSelection();
    } else {
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
    }
    // Disabling a focused batch control would drop keyboard focus to the
    // body, so the tray parks focus on the selection header's Done while a
    // batch settles and returns it when interactivity resumes, mirroring the
    // Grid's held-cell hand-off.
    if (!enabled && heldBatchControl === null) {
      const active = document.activeElement;
      if (
        active === batchSelect ||
        active === batchReject ||
        active === batchAlbumSelect ||
        active === batchAlbumAdd ||
        active === batchCompensate
      ) {
        heldBatchControl = active as HTMLButtonElement | HTMLSelectElement;
        multiDone.focus();
      }
    }
    batchSelect.disabled = !enabled;
    batchReject.disabled = !enabled;
    const albums = batchAlbumSelect.options.length > 0;
    batchAlbumSelect.disabled = !enabled || !albums;
    batchAlbumAdd.disabled = !enabled || !albums || !batchAlbumSelection;
    batchCompensate.disabled = !enabled || resultAction === undefined;
    if (enabled && heldBatchControl) {
      const control = heldBatchControl;
      heldBatchControl = null;
      if (
        control.isConnected &&
        !control.disabled &&
        (document.activeElement === document.body ||
          document.activeElement === multiDone)
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
    filmstripModel = model;
    // A strip that is not presented — a source without neighbors, or a closed
    // Nearby Photos disclosure — binds no thumbnails either: presenting
    // entries for a hidden surface is the work the strip promises never to
    // start. A compact layout discloses the strip inside Photo tools; a wide
    // layout shows it beside the Preview.
    const presented =
      (!stripIsDisclosed() || stripInTools()) && model.cells.length > 1;
    if (!presented || model.cells.length <= 1) {
      clearFilmstripCells();
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
  /// True while the Grid owns keyboard focus, so Grid keys never act while
  /// another surface (the Sources surface, the Album form, the Photo View) has
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
    // An explicit focus request names the cell it wants. Otherwise only the
    // Grid itself owns the keyboard position: focus on the body, on nothing,
    // or inside the Grid viewport is reclaimable, while the header tools, the
    // selection header, and the batch tray own their own focus, so a control
    // the Photographer is using there is never pulled back into a cell while a
    // batch settles.
    const requested = pendingGridCellFocus !== undefined;
    pendingGridCellFocus = undefined;
    const owns =
      requested ||
      active === null ||
      active === document.body ||
      (gridViewport.contains(active) &&
        !gridTools.contains(active) &&
        !gridSelection.contains(active) &&
        !gridBatch.contains(active));
    if (!owns || index === undefined) return;
    const cell = gridLayer.querySelector<HTMLButtonElement>(
      `[data-photo-index="${index}"]`,
    );
    if (cell && !cell.disabled) {
      // preventScroll keeps a focus move from scrolling the Grid the restore
      // has just positioned, so the restored geometry survives it.
      if (active !== cell) cell.focus({ preventScroll: true });
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
    if (event.key === "Escape" && (gridMultiCount > 0 || gridMultiMode)) {
      // Escape takes the same exit as the tray's Done control: the
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
    gridMultiResult = model.multi.result;
    gridMultiSelected = model.multi.selected;
    gridPhotoLookup = model.photoAt;
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
    currentRating = value;
    rating.textContent =
      value === 0 ? "No rating" : `${value} ${value === 1 ? "star" : "stars"}`;
    dockRating.textContent = value === 0 ? "Rating" : `Rating ${value}★`;
    dockRating.setAttribute(
      "aria-label",
      value === 0
        ? "Rating, no Rating; open Rating controls"
        : `Rating ${value} stars; open Rating controls`,
    );
    syncRatingWheelOptions();
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
    // The Preview Source fact and the detail-limit explanation are one
    // statement: a limited derivative names the resolution it came from.
    detailLimit.hidden = !isLimited;
    if (isLimited) previewSource.title = LIMITED_PREVIEW_DETAIL;
    else previewSource.removeAttribute("title");
  };

  const renderPhotoShell = (model: PhotoShellViewModel) => {
    if (!alive) return undefined;
    resetGestures();
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
      cancelRatingHold();
      closeRatingWheel();
      beginPinch();
      return;
    }
    if (activePointers.size > 2 || pinch || pointer || !event.isPrimary) return;
    // Fit owns decision swipes; a manual zoom owns bounded panning. Neither
    // state ever records a decision from a drag.
    if (!zoomManual && !decisionInteractionEnabled) return;
    const ratingPending =
      event.pointerType === "touch" &&
      !zoomManual &&
      measurableImage() &&
      decisionInteractionEnabled;
    pointer = {
      id: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      startedAt: event.timeStamp,
      vertical: false,
      ratingPending,
      ratingWheel: false,
      pan: zoomManual,
      surface: photoSurface,
      photoId: currentPhotoId,
    };
    try {
      preview.setPointerCapture(event.pointerId);
    } catch {
      // Synthetic PointerEvents used by browser qualification have no native
      // active pointer to capture; the gesture state still remains testable.
    }
    if (ratingPending) {
      const id = event.pointerId;
      const surface = photoSurface;
      const photoId = currentPhotoId;
      ratingWheelHoldTimer = window.setTimeout(() => {
        ratingWheelHoldTimer = undefined;
        const active = pointer;
        if (
          !active ||
          active.id !== id ||
          !active.ratingPending ||
          active.surface !== surface ||
          active.photoId !== photoId ||
          active.vertical ||
          active.pan ||
          zoomManual ||
          !decisionInteractionEnabled ||
          !currentPhotoId
        )
          return;
        active.ratingPending = false;
        active.ratingWheel = true;
        stage.style.transform = "";
        selectFeedback.classList.remove("pending");
        rejectFeedback.classList.remove("pending");
        openRatingWheel(active.lastX, active.lastY);
        updateRatingWheel(active.lastX, active.lastY);
      }, RATING_WHEEL_HOLD_MS);
    }
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
    if (pointer.ratingWheel) {
      updateRatingWheel(event.clientX, event.clientY);
      return;
    }
    if (pointer.pan || zoomManual) {
      panX = clamp(panX + stepX, -panLimitX(), panLimitX());
      panY = clamp(panY + stepY, -panLimitY(), panLimitY());
      applyZoom();
      return;
    }
    if (
      pointer.ratingPending &&
      Math.hypot(dx, dy) > RATING_WHEEL_MOVE_PIXELS
    ) {
      pointer.ratingPending = false;
      cancelRatingHold();
      // The dominant axis owns the gesture. Equal movement yields to native
      // vertical scrolling instead of guessing a decision direction.
      if (Math.abs(dy) >= Math.abs(dx)) pointer.vertical = true;
    } else if (Math.abs(dy) > Math.abs(dx) && Math.abs(dy) > 12) {
      pointer.vertical = true;
    }
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
    const wheelCandidate = active.ratingWheel
      ? ratingWheelCandidate
      : undefined;
    if (active.ratingWheel) closeRatingWheel();
    clearPointer();
    if (active.ratingWheel) {
      if (
        cancelled ||
        wheelCandidate === undefined ||
        !decisionInteractionEnabled ||
        active.surface !== photoSurface ||
        active.photoId !== currentPhotoId
      )
        return;
      send({
        kind: "photo-mutation",
        field: "rating",
        value: wheelCandidate,
        advance: false,
      });
      return;
    }
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
    // A modal surface owns the keyboard: background shortcuts, Grid movement,
    // and Photo decisions must not act behind it. Escape on such a surface is
    // the native dialog's own close request, handled by the modal-surface
    // cancel listener, so no branch here re-implements it.
    if (surfaces.blocking()) return;
    const modifier = event.ctrlKey || event.metaKey;
    if (modifier && !event.shiftKey && event.key.toLowerCase() === "z") {
      event.preventDefault();
      send({ kind: "undo" });
      return;
    }
    if (photoView.hidden) {
      if (
        event.key === "Escape" &&
        (gridMultiCount > 0 || gridMultiMode) &&
        gridView.contains(target)
      ) {
        event.preventDefault();
        send({ kind: "grid-multi-clear" });
        return;
      }
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
  mobileActionHierarchy.addEventListener("change", onSourceViewportChange);
  const onShortViewportChange = () => {
    if (!alive) return;
    // Entering or leaving a short viewport only changes where the strip is
    // presented. The page model owns its facts, so it re-renders the strip in
    // either direction and the view places the single node accordingly.
    syncFilmstripHost();
    send({ kind: "filmstrip-resize" });
  };
  // A native close request reaches the surface before the controller's
  // cleanup, so the disclosure state is synced from both events.
  sourceDialog.addEventListener("cancel", syncSourcesExpanded);
  sourceDialog.addEventListener("close", syncSourcesExpanded);
  // A Photo tools dismissal — explicit Close, Escape, a scrim activation, or a
  // platform close request — returns the modal to its list and releases the
  // neighbor strip, and both entries report their closed state again.
  photoToolsDialog.addEventListener("close", () => {
    photoToolsView = "tools";
    applyPhotoToolsView();
    syncFilmstripHost();
    syncSecondarySurface();
  });
  ratingDialog.addEventListener("close", syncSecondarySurface);
  shortViewport.addEventListener("change", onShortViewportChange);
  sourceToggle.addEventListener("click", () => openSources());
  photoSourceToggle.addEventListener("click", () => openSources());
  sourceClose.addEventListener("click", () => closeSources());
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
  recoveryClose.addEventListener("click", () =>
    send({ kind: "recovery-close" }),
  );
  removalOpen.addEventListener("click", () =>
    send({ kind: "removal-review-open" }),
  );
  removalClose.addEventListener("click", () =>
    send({ kind: "removal-review-close" }),
  );
  removalConfirm.addEventListener("click", () =>
    send({ kind: "removal-confirm" }),
  );
  removalUndo.addEventListener("click", () =>
    send({ kind: "removal-undo", surface: "review" }),
  );
  removedUndo.addEventListener("click", () =>
    send({ kind: "removal-undo", surface: "listing" }),
  );
  removedOpen.addEventListener("click", () =>
    send({ kind: "removed-list-open" }),
  );
  removedClose.addEventListener("click", () =>
    send({ kind: "removed-list-close" }),
  );
  removedPrevious.addEventListener("click", () =>
    send({ kind: "removed-page", direction: -1 }),
  );
  removedNext.addEventListener("click", () =>
    send({ kind: "removed-page", direction: 1 }),
  );
  removedRetry.addEventListener("click", () => send({ kind: "removed-retry" }));
  // A close the controller did not start — a native close request, an Escape
  // from a surface this one yielded to, a destination change — releases the
  // listing's thumbnails the same way an explicit Close does.
  removedPanel.addEventListener("close", releaseRemovedRows);
  recoveryPropose.addEventListener("click", () =>
    send({
      kind: "recovery-propose",
      oldPrefix: recoveryOldPrefix.value.trim(),
      newPrefix: recoveryNewPrefix.value.trim(),
    }),
  );
  recoveryProposeSingle.addEventListener("click", () =>
    send({
      kind: "recovery-propose-single",
      originalId: recoverySingleOriginal.value,
      newLocation: recoverySingleLocation.value.trim(),
    }),
  );
  recoveryApply.addEventListener("click", () => {
    const items = recoveryCurrentProposals
      .filter(
        (proposal) =>
          proposal.outcome === "matched" ||
          (proposal.outcome === "occupied" &&
            proposal.retire &&
            recoveryRetireSelection.get(proposal.originalId)),
      )
      .map((proposal) => ({
        originalId: proposal.originalId,
        newLocation: proposal.toLocation,
        retireDestination: proposal.outcome === "occupied",
      }));
    if (items.length === 0) return;
    send({ kind: "recovery-apply", items });
  });
  // The empty-state action is explained by the state that shows it: the
  // Library check for an empty source, and the one explicit action an
  // explained destination state offers.
  gridEmptyAction.addEventListener("click", () =>
    send(
      gridEmptyExplanation
        ? { kind: "explained-action" }
        : { kind: "library-check" },
    ),
  );
  retry.addEventListener("click", () => send({ kind: "retry-source" }));
  signOut.addEventListener("click", () => send({ kind: "sign-out" }));
  retryPhoto.addEventListener("click", () => send({ kind: "retry-photo" }));
  dockPrevious.addEventListener("click", () => send({ kind: "previous" }));
  dockNext.addEventListener("click", () => send({ kind: "next" }));
  dockRating.addEventListener("click", () => openRatingChoices());
  dockMore.addEventListener("click", () => openPhotoTools("tools"));
  ratingChoicesClose.addEventListener("click", () => closeRatingChoices());
  photoToolsClose.addEventListener("click", () => closePhotoTools());
  photoToolsClear.addEventListener("click", () =>
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: "undecided",
      advance: false,
    }),
  );
  photoToolsUndo.addEventListener("click", () => send({ kind: "undo" }));
  photoToolsEntries.addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
      "[data-photo-tools-entry]",
    );
    if (!button) return;
    const photoId = currentPhotoId;
    if (!photoId) return;
    if (button.dataset.photoToolsEntry === "sources") {
      openSources();
      return;
    }
    if (button.dataset.photoToolsEntry === "edit") {
      openEditor(photoId);
      return;
    }
    openPhotoTools(button.dataset.photoToolsEntry as PhotoToolsView);
  });
  // A pointer drag reports every intermediate position, and only the settled
  // gesture is one edit action, so the label follows `input` while the save
  // follows `change`.
  editorExposure.addEventListener("input", () => {
    if (!editorPhotoId || !editorModel || editorExposure.disabled) return;
    editorDraftBase ??= editorModel.exposureEv;
    editorDraft = Number(editorExposure.value);
    renderEditor(editorModel);
  });
  editorExposure.addEventListener("change", () => {
    if (!editorPhotoId || !editorModel || editorExposure.disabled) return;
    const value = Number(editorExposure.value);
    editorDraftBase ??= editorModel.exposureEv;
    editorDraft = value;
    renderEditor(editorModel);
    send({
      kind: "editor-exposure",
      photoId: editorPhotoId,
      exposureEv: value,
    });
  });
  // The white-balance mode is one committed choice, so it saves on `change`.
  editorWhiteBalanceMode.addEventListener("change", () => {
    if (!editorPhotoId || !editorModel || editorWhiteBalanceMode.disabled)
      return;
    send({
      kind: "editor-white-balance-mode",
      photoId: editorPhotoId,
      mode: editorWhiteBalanceMode.value,
    });
  });
  editorTemperature.addEventListener("change", () => {
    if (!editorPhotoId || !editorModel || editorTemperature.disabled) return;
    send({
      kind: "editor-temperature",
      photoId: editorPhotoId,
      temperatureKelvin: Number(editorTemperature.value),
    });
  });
  editorTint.addEventListener("change", () => {
    if (!editorPhotoId || !editorModel || editorTint.disabled) return;
    send({
      kind: "editor-tint",
      photoId: editorPhotoId,
      tintMilli: Number(editorTint.value),
    });
  });
  editorResetExposure.addEventListener("click", () => {
    if (!editorPhotoId || !editorModel) return;
    send({ kind: "editor-reset-exposure", photoId: editorPhotoId });
  });
  editorResetWhiteBalance.addEventListener("click", () => {
    if (!editorPhotoId || !editorModel) return;
    send({ kind: "editor-reset-white-balance", photoId: editorPhotoId });
  });
  editorPreview.addEventListener("click", () => {
    if (editorPhotoId) send({ kind: "editor-preview", photoId: editorPhotoId });
  });
  editorUndo.addEventListener("click", () => {
    if (editorPhotoId) send({ kind: "editor-undo", photoId: editorPhotoId });
  });
  editorRedo.addEventListener("click", () => {
    if (editorPhotoId) send({ kind: "editor-redo", photoId: editorPhotoId });
  });
  editorCompare.addEventListener("click", () => {
    if (!editorPhotoId || !editorModel) return;
    send({
      kind: "editor-compare",
      photoId: editorPhotoId,
      pressed: !editorModel.comparing,
    });
  });
  editorReset.addEventListener("click", () => {
    if (!editorPhotoId || !editorModel) return;
    send({ kind: "editor-reset", photoId: editorPhotoId });
  });
  editorRefresh.addEventListener("click", () => {
    if (editorPhotoId) send({ kind: "editor-refresh", photoId: editorPhotoId });
  });
  editorUseSaved.addEventListener("click", () => {
    if (editorPhotoId)
      send({ kind: "editor-use-saved", photoId: editorPhotoId });
  });
  editorReapply.addEventListener("click", () => {
    if (editorPhotoId) send({ kind: "editor-reapply", photoId: editorPhotoId });
  });
  editorDiscardDraft.addEventListener("click", () => {
    if (editorPhotoId)
      send({ kind: "editor-discard-draft", photoId: editorPhotoId });
  });
  editorExportSubmit.addEventListener("click", () => {
    if (editorPhotoId)
      send({ kind: "editor-export-submit", photoId: editorPhotoId });
  });
  editorExportCancel.addEventListener("click", () => {
    if (editorPhotoId)
      send({ kind: "editor-export-cancel", photoId: editorPhotoId });
  });
  editorExportRetry.addEventListener("click", () => {
    if (editorPhotoId)
      send({ kind: "editor-export-retry", photoId: editorPhotoId });
  });
  editorExportDownload.addEventListener("click", () => {
    if (editorPhotoId)
      send({ kind: "editor-export-download", photoId: editorPhotoId });
  });
  for (const button of editorStages)
    button.addEventListener("click", () => {
      if (!editorPhotoId || button.disabled) return;
      send({
        kind: "editor-stage",
        photoId: editorPhotoId,
        stage: button.dataset.photoEditorStage as EditorStage,
      });
    });
  editorRebind.addEventListener("click", () => {
    if (!editorPhotoId || editorRebind.disabled) return;
    send({ kind: "editor-rebind", photoId: editorPhotoId });
  });
  for (const button of photoToolsReturnButtons)
    button.addEventListener("click", () => returnToPhotoTools());
  stage.addEventListener("dblclick", toggleDetail);
  zoomFit.addEventListener("click", applyFit);
  zoomOut.addEventListener("click", () => zoomBy(1 / ZOOM_STEP));
  zoomIn.addEventListener("click", () => zoomBy(ZOOM_STEP));
  zoom100.addEventListener("click", () => applyManualZoom(100));
  zoomSlider.addEventListener("input", () =>
    applyManualZoom(Number(zoomSlider.value)),
  );
  preview.addEventListener("wheel", wheelZoom, { passive: false });
  preview.addEventListener("contextmenu", onPreviewContextMenu);
  zoomControls.addEventListener("pointerdown", (event) =>
    event.stopPropagation(),
  );
  zoomControls.addEventListener("pointermove", (event) =>
    event.stopPropagation(),
  );
  zoomControls.addEventListener("pointerup", (event) =>
    event.stopPropagation(),
  );
  dockSelect.addEventListener("click", () =>
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: "selected",
      advance: true,
    }),
  );
  dockReject.addEventListener("click", () =>
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: "rejected",
      advance: true,
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
  // Done is the visible clear exit: it empties the multi-selection and leaves
  // Select mode without changing a Photo decision, exactly like Escape.
  multiDone.addEventListener("click", () => send({ kind: "grid-multi-clear" }));
  batchSelect.addEventListener("click", () => {
    if (!alive || batchSelect.disabled) return;
    send({ kind: "grid-batch-mutation", value: "selected" });
  });
  batchReject.addEventListener("click", () => {
    if (!alive || batchReject.disabled) return;
    send({ kind: "grid-batch-mutation", value: "rejected" });
  });
  batchAlbumSelect.addEventListener("change", () => {
    if (!alive) return;
    batchAlbumSelection = batchAlbumSelect.value;
    renderBatch();
  });
  batchAlbumAdd.addEventListener("click", () => {
    if (!alive || batchAlbumAdd.disabled || !batchAlbumSelection) return;
    send({ kind: "grid-batch-album-add", albumId: batchAlbumSelection });
  });
  batchCompensate.addEventListener("click", () => {
    if (!alive || batchCompensate.disabled) return;
    send(
      batchCompensate.dataset.action === "review"
        ? { kind: "grid-batch-review" }
        : { kind: "grid-batch-album-remove" },
    );
  });
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
    // The View options entry for the open Album. The Sources row carries the
    // same action for any Album, so one album-resume intent serves both.
    const album = sourceModel?.albums.find((candidate) => candidate.active);
    if (album) send({ kind: "album-resume", albumId: album.id });
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
  syncSourceLayout();
  syncSourcesExpanded();
  applyPhotoToolsView();
  syncSecondarySurface();
  presentViewOptionsFlag();
  // The strip's home is only placed once a Photo can present it: the markup
  // already holds it beside the Preview, which is where a wide layout shows
  // it, and a compact layout moves it into Photo tools when that disclosure
  // opens. Placing it here would emit an intent while the page model that
  // owns the strip's facts is still being constructed.

  const presentRemovalReview = (model: RemovalReviewViewModel) => {
    removalSummary.textContent = `${formatPhotoCount(
      model.reviewed,
    )} reviewed as Rejected. Removing them takes them out of the Library; they stay recoverable and every Original File stays where it is.`;
    removalConfirm.hidden = !model.canConfirm;
    removalConfirm.disabled = model.pending;
    removalConfirm.textContent = model.pending
      ? "Removing…"
      : "Remove from Library";
    removalUndo.hidden = model.undo === undefined;
    removalUndo.disabled = model.pending;
    if (model.message === undefined) {
      removalMessage.hidden = true;
      removalMessage.textContent = "";
      removalMessage.removeAttribute("data-tone");
      return;
    }
    removalMessage.textContent = model.message;
    removalMessage.hidden = false;
    if (model.tone) removalMessage.dataset.tone = model.tone;
    else removalMessage.removeAttribute("data-tone");
  };
  const presentRemovedPanel = (model: RemovedPanelViewModel) => {
    releaseRemovedRows();
    const shown = Math.min(model.total, model.start + model.items.length);
    removedStatus.textContent =
      model.total === 0
        ? "No Photos are removed from the Library."
        : `${formatPhotoCount(model.total)} removed from the Library. Showing ${(
            model.start + 1
          ).toLocaleString()}–${shown.toLocaleString()}.`;
    const rows = model.items.map((item) => {
      const row = document.createElement("li");
      row.className = "removed-item";
      const image = document.createElement("img");
      image.className = "removed-thumb";
      image.alt = "";
      const facts = document.createElement("div");
      facts.className = "removed-facts";
      const name = document.createElement("p");
      name.className = "removed-name";
      name.textContent = item.filename;
      const when = document.createElement("p");
      when.className = "removed-when";
      when.textContent = `Removed ${removalTimestamp(item.removedAtMs)}`;
      facts.append(name, when);
      const restore = document.createElement("button");
      restore.type = "button";
      restore.className = "quiet";
      const restoring = model.restoringPhotoId === item.photoId;
      restore.textContent = restoring ? "Restoring…" : "Restore";
      restore.disabled = model.pending || restoring;
      restore.addEventListener("click", () =>
        send({
          kind: "removed-restore",
          photoId: item.photoId,
          removedAtMs: item.removedAtMs,
        }),
      );
      row.append(image, facts, restore);
      const binding: GridThumbnailBinding = {
        photoId: item.photoId,
        preview: item.preview,
        target: gridThumbnailTarget(image, (failed) => {
          image.classList.toggle("delivery-failed", failed);
        }),
      };
      // The listing presents the same Photo facts the Grid does, so it uses
      // the page's one image-delivery path. A delivery the page drops because
      // the source moved on leaves the row without its Thumbnail until the
      // listing is presented again, which is the rule the Grid follows too.
      bindThumbnail(binding);
      return { row, binding };
    });
    removedListBindings = rows.map((entry) => entry.binding);
    removedList.replaceChildren(...rows.map((entry) => entry.row));
    const shownEnd = shown >= model.total;
    removedPrevious.disabled = model.pending || model.start === 0;
    removedNext.disabled = model.pending || shownEnd;
    removedRetry.hidden = !model.canRetry;
    removedRetry.disabled = model.pending;
    removedUndo.hidden = model.undo === undefined;
    removedUndo.disabled = model.pending;
    removedUndo.textContent =
      model.undo === undefined
        ? "Undo the last removal"
        : `Undo the last removal (${model.undo.removed.toLocaleString()})`;
    removedPage.textContent =
      model.total === 0
        ? ""
        : `Page ${(
            Math.floor(model.start / model.limit) + 1
          ).toLocaleString()} of ${Math.max(
            1,
            Math.ceil(model.total / model.limit),
          ).toLocaleString()}`;
    if (model.message === undefined) {
      removedMessage.hidden = true;
      removedMessage.textContent = "";
      return;
    }
    removedMessage.textContent = model.message;
    removedMessage.hidden = false;
  };

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
      connectionState = isConnected;
      presentConnection();
      retry.hidden = !sourceRetryVisible;
      retryPhoto.hidden = !photoRetryVisible;
    },
    setSourceTitle(name) {
      if (!alive) return;
      presentSourceTitle(name);
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
      gridEmptyExplanation = false;
    },
    setGridExplanation(message, action) {
      if (!alive) return;
      cancelGridRender();
      clearGridCells();
      clearFilmstripCells();
      gridEmpty.hidden = false;
      gridEmptyMessage.textContent = message;
      gridEmptyAction.hidden = !action;
      if (action) gridEmptyAction.textContent = action.label;
      gridStatus.textContent = message;
      gridEmptyExplanation = Boolean(action);
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
      if (
        !model.decisionEnabled &&
        (ratingWheelOpen || pointer?.ratingPending || pointer?.ratingWheel)
      )
        resetGestures();
      for (const button of [
        dockSelect,
        dockReject,
        ...Array.from(ratings.querySelectorAll<HTMLButtonElement>("button")),
      ])
        button.disabled = !model.decisionEnabled;
      photoToolsClear.disabled = !model.clearEnabled;
      dockRating.disabled = !model.decisionEnabled;
      back.disabled = !model.backEnabled;
      refresh.disabled = !model.refreshEnabled;
      retry.disabled = !model.recoveryEnabled;
      retryPhoto.disabled = !model.recoveryEnabled;
      dockPrevious.disabled = !model.previousEnabled;
      dockNext.disabled = !model.nextEnabled;
      photoToolsUndo.disabled = !model.undoEnabled;
      removalOpen.hidden = !model.removalEnabled;
      removalOpen.disabled = !model.removalEnabled;
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
        // A native modal makes the rest of the document inert, so the Photo
        // View cannot take a parked focus while the strip is disclosed inside
        // Photo tools: the surface that holds the strip does, and the restore
        // below recognizes it as the parked owner.
        const surface = stripSurface();
        if (surface) surface.focus();
        else photoView.focus();
      } else if (!wasStripInteractive && model.filmstripEnabled) {
        restoreHeldStripFocus();
      }
      syncZoomControls();
    },
    renderMembership,
    prepareSourceOpen(name) {
      if (!alive) return;
      const returnFocus = surfaces.isActive("sources");
      cancelGridRender();
      resetGestures();
      closePhotoTools(false);
      closeRatingChoices(false);
      stage.replaceChildren();
      resetZoomForImage();
      gridView.hidden = false;
      photoView.hidden = true;
      clearGridCells();
      clearFilmstripCells();
      closeSources(false);
      syncSourceLayout();
      if (returnFocus) gridViewport.focus();
      presentSourceTitle(name);
      folderAlbumControls.hidden = true;
      folderAlbumSelect.replaceChildren();
      folderAlbumStatus.textContent = "";
      gridStatus.textContent = "Preparing Library order…";
      gridEmpty.hidden = true;
      gridEmptyMessage.textContent = "";
      gridEmptyAction.hidden = true;
      gridEmptyExplanation = false;
      currentPhotoId = undefined;
      photoSurface = {};
      gridKeyboardIndex = undefined;
      // A new source starts with no multi-selection: the tray presents nothing
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
    captureGridRestoration() {
      if (!alive || gridView.hidden || gridTotal === 0) return undefined;
      const count = columns();
      const index = firstVisibleGridIndex(count);
      const photo = gridPhotoLookup(index);
      if (!photo) return undefined;
      const offset = gridViewport.scrollTop % rowPitch();
      const active = document.activeElement;
      const holdsKeyboard =
        active === gridViewport ||
        (active instanceof HTMLElement && gridLayer.contains(active));
      // The cell the keyboard owns, or the cell a pointer activation focused.
      const activeCellIndex =
        active instanceof HTMLElement && active.dataset.photoIndex !== undefined
          ? Number(active.dataset.photoIndex)
          : gridKeyboardIndex;
      const focusedId =
        activeCellIndex !== undefined &&
        Number.isInteger(activeCellIndex) &&
        activeCellIndex >= 0 &&
        activeCellIndex < gridTotal
          ? gridPhotoLookup(activeCellIndex)?.id
          : undefined;
      return {
        anchor: { photoId: photo.id, indexHint: index, offset },
        focus:
          holdsKeyboard && focusedId
            ? { kind: "photo", photoId: focusedId }
            : { kind: "grid" },
      };
    },
    restoreGridAnchor(model) {
      if (!alive || gridView.hidden) return;
      // The Grid itself owns focus when the restoration names no cell, and a
      // cell that no longer exists leaves the Grid focused. The establishment
      // path (a reload or a direct entry) has no activation that moved focus
      // into the Grid, so the viewport takes it here rather than leaving the
      // document body focused. preventScroll keeps the restored geometry.
      gridKeyboardIndex =
        model.focusIndex === undefined
          ? undefined
          : Math.max(0, Math.min(gridTotal - 1, model.focusIndex));
      if (
        gridKeyboardIndex === undefined &&
        !gridLayer.contains(document.activeElement)
      )
        gridViewport.focus({ preventScroll: true });
      if (gridTotal === 0) return;
      const count = columns();
      const target = Math.max(0, Math.min(gridTotal - 1, model.index));
      gridViewport.scrollTop =
        Math.floor(target / count) * rowPitch() + model.offset;
      scheduleGridRender();
    },
    closeTransientSurfaces() {
      if (!alive) return;
      resetGestures();
      closePhotoTools(false);
      closeRatingChoices(false);
      // A destination change supersedes every supporting surface: the Album
      // form's draft is discarded and the surfaces close without returning
      // focus, because the destination render owns focus next.
      if (albumForm) {
        albumForm = undefined;
        albumFormInvoker = undefined;
        albumFormBody.replaceChildren();
        if (albumFormDialog.contains(document.activeElement)) {
          surfaces.closeAll();
          if (!gridView.hidden) gridViewport.focus();
          else photoView.focus();
          return;
        }
      }
      surfaces.closeAll();
    },
    focusGridIndex(index) {
      if (!alive) return;
      const count = columns();
      const target = Math.max(0, Math.min(Math.max(gridTotal - 1, 0), index));
      gridKeyboardIndex = target;
      // This is a deliberate focus move, so it survives a control that holds
      // focus now, such as the tray's Review action.
      pendingGridCellFocus = target;
      gridViewport.scrollTop = Math.floor(target / count) * rowPitch();
      scheduleGridRender();
    },
    showGrid(index) {
      if (!alive) return;
      resetGestures();
      closePhotoTools(false);
      closeRatingChoices(false);
      resetZoomForImage();
      photoView.hidden = true;
      gridView.hidden = false;
      // Photo View detached the owner's Grid images, so the visible Grid
      // rebuilds its cells and re-attaches every thumbnail it still shows.
      clearGridCells();
      clearFilmstripCells();
      closeSources(false);
      syncSourceLayout();
      presentConnection();
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
      resetGestures();
      // Photo View's own surfaces stay open across a Photo change: leaving the
      // Grid, a source change, and a destination change close them, so a
      // re-render of the Photo the tools describe never dismisses them.
      gridView.hidden = true;
      photoView.hidden = false;
      photoView.scrollTop = 0;
      syncSourceLayout();
      syncFilmstripHost();
      presentConnection();
      photoView.focus();
      resetZoomForImage();
      photoSurface = {};
    },
    renderFilmstrip,
    renderPhotoFacts,
    renderPhotoMetadata,
    renderPhotoShell,
    editorVisible,
    renderEditor,
    presentEditorPreview,
    clearEditorPreview,
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
      renderAlbumForm();
    },
    setAlbumFormPending(formId, pending, name) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      albumForm.pending = pending;
      if (name !== undefined) albumForm.name = name;
      delete albumForm.message;
      renderAlbumForm();
    },
    dismissAlbumForm(formId) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      const form = albumForm;
      albumFocusRequest = {
        kind: "return",
        focusKey: form.returnFocusKey,
      };
      albumForm = undefined;
      send({ kind: "album-form-close", formId: form.formId });
      dismissAlbumFormSurface(form);
    },
    openRemovalReview(model) {
      if (!alive) return;
      presentRemovalReview(model);
      surfaces.open("removal-review", removalOpen);
      removalClose.focus();
    },
    renderRemovalReview(model) {
      if (!alive) return;
      presentRemovalReview(model);
    },
    closeRemovalReview() {
      if (!alive) return;
      surfaces.close("removal-review");
    },
    openRemovedPanel(model) {
      if (!alive) return;
      presentRemovedPanel(model);
      surfaces.open("removed-panel", removedOpen);
      removedClose.focus();
    },
    renderRemovedPanel(model) {
      if (!alive) return;
      presentRemovedPanel(model);
    },
    closeRemovedPanel() {
      if (!alive) return;
      surfaces.close("removed-panel");
    },
    setRecoveryNotice(model) {
      if (!alive) return;
      const parts: string[] = [];
      if (model.relocatedPhotos > 0)
        parts.push(
          `Updated locations for ${formatPhotoCount(model.relocatedPhotos)}.`,
        );
      if (model.unavailablePhotos > 0)
        parts.push(
          `${formatPhotoCount(model.unavailablePhotos)} still unavailable.`,
        );
      recoveryNotice.replaceChildren();
      if (parts.length === 0) {
        recoveryNotice.hidden = true;
        return;
      }
      recoveryNotice.append(document.createTextNode(parts.join(" ")));
      if (model.unavailablePhotos > 0) {
        const button = document.createElement("button");
        button.type = "button";
        button.className = "summary-action";
        button.textContent = "Review unavailable originals";
        button.addEventListener("click", () =>
          send({ kind: "recovery-entry" }),
        );
        recoveryNotice.append(" ", button);
      }
      recoveryNotice.hidden = false;
    },
    openRecoveryPanel(entries) {
      if (!alive) return;
      recoveryCurrentProposals = [];
      recoveryRetireSelection.clear();
      recoverySummary.textContent = `${formatPhotoCount(entries.length)} unavailable`;
      const rows = entries.slice(0, 100).map((entry) => {
        const item = document.createElement("li");
        const decisions = [
          selectionLabel(entry.selectionState),
          entry.rating > 0 ? `${entry.rating} stars` : null,
          entry.albumCount > 0
            ? `${entry.albumCount} album${entry.albumCount === 1 ? "" : "s"}`
            : null,
        ]
          .filter(Boolean)
          .join(" · ");
        const fingerprint = entry.fingerprintEnrolled
          ? "Fingerprint on file"
          : "No fingerprint";
        item.textContent = `${entry.location} — ${entry.kind.toUpperCase()} — ${decisions} — ${fingerprint}`;
        return item;
      });
      if (entries.length > 100) {
        const more = document.createElement("li");
        more.textContent = `…and ${entries.length - 100} more`;
        rows.push(more);
      }
      recoveryList.replaceChildren(...rows);
      recoverySingleOriginal.replaceChildren(
        ...entries.map((entry) =>
          Object.assign(document.createElement("option"), {
            value: entry.originalId,
            textContent: entry.location,
          }),
        ),
      );
      recoveryProposalList.replaceChildren();
      recoveryProposalList.hidden = true;
      recoveryNote.hidden = true;
      recoveryApply.hidden = true;
      recoveryMessage.hidden = true;
      recoverySingleLocation.value = "";
      surfaces.open("recovery");
      recoveryClose.focus();
    },
    renderRecoveryProposals(proposals) {
      if (!alive) return;
      recoveryCurrentProposals = proposals;
      recoveryNote.hidden = !proposals.some((proposal) => !proposal.verified);
      const rows = proposals.map((proposal) => {
        const item = document.createElement("li");
        const heading = document.createElement("p");
        heading.className = "recovery-proposal-path";
        heading.textContent = `${proposal.fromLocation} → ${proposal.toLocation}`;
        const facts = document.createElement("p");
        facts.className = "recovery-proposal-facts";
        facts.textContent = `${recoveryOutcomeLabel(proposal.outcome)} · ${
          proposal.verified
            ? "Content verified"
            : "Old content cannot be verified"
        }`;
        item.append(heading, facts);
        if (proposal.outcome === "occupied" && proposal.retire) {
          const retireLabel = document.createElement("label");
          retireLabel.className = "recovery-retire";
          const checkbox = document.createElement("input");
          checkbox.type = "checkbox";
          checkbox.addEventListener("change", () => {
            recoveryRetireSelection.set(proposal.originalId, checkbox.checked);
            updateRecoveryApply();
          });
          retireLabel.append(
            checkbox,
            document.createTextNode(
              `Replace the discovered Photo at ${proposal.retire.location}`,
            ),
          );
          item.append(retireLabel);
        }
        return item;
      });
      recoveryProposalList.replaceChildren(...rows);
      recoveryProposalList.hidden = proposals.length === 0;
      updateRecoveryApply();
    },
    setRecoveryPending(pending) {
      if (!alive) return;
      recoveryPropose.disabled = pending;
      recoveryProposeSingle.disabled = pending;
      recoveryApply.disabled = pending;
    },
    setRecoveryMessage(text) {
      if (!alive) return;
      if (!text) {
        recoveryMessage.hidden = true;
        return;
      }
      recoveryMessage.textContent = text;
      recoveryMessage.hidden = false;
    },
    closeRecoveryPanel() {
      if (!alive) return;
      surfaces.close("recovery");
    },
    dispose() {
      if (!alive) return;
      alive = false;
      resetGestures();
      releaseRemovedRows();
      stageObserver.disconnect();
      clearFilmstripCells();
      preview.removeEventListener("wheel", wheelZoom);
      preview.removeEventListener("contextmenu", onPreviewContextMenu);
      cancelGridRender();
      compactSources.removeEventListener("change", onSourceViewportChange);
      mobileActionHierarchy.removeEventListener(
        "change",
        onSourceViewportChange,
      );
      shortViewport.removeEventListener("change", onShortViewportChange);
      gridViewport.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onResize);
      window.removeEventListener("keydown", keydown);
      surfaces.dispose();
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

/// When a Photo was removed, in the Photographer's own locale. A timestamp the
/// platform cannot parse is presented verbatim rather than invented.
/// The removal marker as a local reading. The listing reports the millisecond
/// the removal was confirmed, so the row shows the same instant the restore
/// names and no timezone-less text is parsed as if it were local.
function removalTimestamp(removedAtMs: number): string {
  return new Date(removedAtMs).toLocaleString();
}

function gridPhotoFacts(
  photo: GridPhotoViewModel,
  deliveryFailed: boolean,
): string[] {
  const facts: string[] = [];
  if (!photo.available) facts.push("Photo unavailable");
  if (photo.original.kind === "raw") facts.push("RAW");
  if (photo.hasSavedEdits) facts.push("Edited");
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
    photo.original.kind,
    photo.selectionState,
    String(photo.rating),
    photo.hasSavedEdits ? "edited" : "unedited",
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
  return source === "jpeg-original"
    ? "JPEG"
    : source === "raw-embedded-jpeg"
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
