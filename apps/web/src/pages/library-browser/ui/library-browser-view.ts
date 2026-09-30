import "./library-browser.css";

import { createModalSurfaces } from "./modal-surface.js";
import { gridThumbnailTarget } from "./grid-thumbnail-target.js";
import {
  createRemovedPanels,
  type RemovedPanelViewModel,
  type RemovalReviewViewModel,
  type TrashReviewViewModel,
} from "./removed-panels.js";
import {
  createRecoveryPanel,
  type RecoveryReviewView,
  RECOVERY_PANEL_TEMPLATE,
  recoveryPanelElements,
} from "./recovery-panel.js";
import type {
  RecoveryEntryViewModel,
  RecoveryMappingViewModel,
  RecoveryPagingViewModel,
} from "./recovery-view-models.js";
import {
  createMembershipPanel,
  type MembershipPanelElements,
} from "./membership-panel.js";
import { createViewOptions, type ViewOptionsElements } from "./view-options.js";
import { createRatingControls } from "./rating-controls.js";
import { createPhotoToolsController } from "./photo-tools.js";
import { createPhotoZoomController } from "./photo-zoom.js";
import { createPhotoGestures } from "./photo-gestures.js";
import { createPhotoEditorSurfaceController } from "./photo-editor-surface.js";
import type { EditorProxyViewModel } from "./editor-proxy-view-model.js";
import { createSourceSurfaceController } from "./source-surface.js";
import { createGridPresenter } from "./grid-presenter.js";
import type {
  GridPhotoViewModel,
  GridThumbnailBinding,
} from "./grid-cell-presenter.js";
import {
  createFilmstripPresenter,
  type FilmstripPresenter,
  type FilmstripViewModel,
} from "./filmstrip-presenter.js";
import { createAlbumForm } from "./album-form.js";
import {
  addressFor,
  type NavigationGridRestoration,
} from "../model/browser-navigation.js";
import { formatCaptureTime } from "./capture-time.js";
import { formatPhotoCount } from "./photo-count.js";
import type {
  EditSourceKind,
  EditSourceReadiness,
  EditorWhiteBalancePresentation,
} from "../model/photo-editor.js";
import type { EditorExportViewModel } from "./photo-editor-surface.js";
import type { RecoveryApplyMapping } from "../model/recovery-review.js";
export type {
  GridPhotoPreview,
  GridThumbnailBinding,
  GridThumbnailTarget,
} from "./grid-cell-presenter.js";

export type {
  RemovedPanelViewModel,
  RemovalReviewViewModel,
  TrashOutcomeViewModel,
  TrashReviewViewModel,
} from "./removed-panels.js";
export type {
  RecoveryEntryViewModel,
  RecoveryMappingViewModel,
  RecoveryPagingViewModel,
} from "./recovery-view-models.js";

export type ViewSelectionState = "undecided" | "selected" | "rejected";
type ViewPreviewSource = "jpeg-original" | "raw-embedded-jpeg";

/** Full wording behind the compact limited-detail marker in the Preview fact. */
const LIMITED_PREVIEW_DETAIL = "Limited by camera Preview resolution";

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

export type GridSortViewModel = Readonly<{
  kind: ViewSourceKind;
  value: ViewSourceOrder;
  enabled: boolean;
}>;

/// The Grid's Selection State filter for the open source. The filter is a view
/// option of every source kind, so no option list depends on the kind.

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

/// One Edit workspace presents the current edit, Film, or Original reference.
/// These are presentation states, not stage tabs.
export type EditorStage = "camera" | "develop" | "film";

export type { EditorExportViewModel } from "./photo-editor-surface.js";

export type EditorViewModel = Readonly<{
  photoId: string;
  loading: boolean;
  stage: EditorStage;
  /// What the presented image actually is, in plain language.
  stageNote: string;
  /// Why Film is unavailable, when it is.
  filmReason: string;
  editSourceReadiness: EditSourceReadiness;
  editSourceKind: EditSourceKind;
  /// The Source support fact line: the readiness word, the Library's scan
  /// phase while the source is being checked, and proxy provenance.
  sourceFactNote: string;
  /// The Processing axis: the deployment's engine capability, separately
  /// from this Photo's source.
  processingReadiness: "checking" | "ready" | "waiting" | "unavailable";
  /// The Edit Preview axis for the chosen stage, or `null` when no Edit
  /// Preview is described on that stage.
  previewState: "pending" | "ready" | "stale" | "failed" | null;
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
  proxy?: EditorProxyViewModel;
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
  /// The session's own detailed wording behind the compact status, presented
  /// only under the optional Details affordance.
  statusDetail: string;
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
        | "editor-proxy-create"
        | "editor-proxy-remove"
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
  | Readonly<{ kind: "removed-select-all" }>
  | Readonly<{ kind: "removed-clear-selection" }>
  | Readonly<{ kind: "removed-toggle"; photoId: string; selected: boolean }>
  | Readonly<{ kind: "removed-delete" }>
  | Readonly<{ kind: "removed-restore-selected" }>
  | Readonly<{
      kind: "removed-restore";
      photoId: string;
      removedAtMs: number;
    }>
  /// Confirms the open permanent-deletion review. The review, not the
  /// listing, is the only surface that deletes Original Files.
  | Readonly<{ kind: "trash-review-confirm" }>
  /// Closes the permanent-deletion review. Cancel deletes nothing: no
  /// delete route is called.
  | Readonly<{ kind: "trash-review-cancel" }>
  /// Fetches the retained result of the Trash deletion operation whose
  /// response was lost.
  | Readonly<{ kind: "trash-check-result" }>
  /// Re-POSTs the retained Trash deletion operation: the server repeats only
  /// its unresolved items and never widens the reviewed set.
  | Readonly<{ kind: "trash-retry-delete" }>
  /// Fetches the retained result of one listing item's pending
  /// permanent-deletion operation.
  | Readonly<{ kind: "trash-row-check"; photoId: string }>
  /// Re-POSTs one listing item's pending permanent-deletion operation.
  | Readonly<{ kind: "trash-row-resume"; photoId: string }>
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
  | Readonly<{ kind: "recovery-more" | "recovery-mappings-more" }>
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
      items: ReadonlyArray<RecoveryApplyMapping>;
    }>;

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

export type MembershipViewModel = Readonly<{
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

export interface LibraryBrowserView extends RecoveryReviewView {
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
  /// Opens the permanent-deletion review above the Trash listing. Opening it
  /// deletes nothing; only its confirmation reaches the delete route.
  openTrashReview(model: TrashReviewViewModel): void;
  /// Updates the open review without reopening it, so a settling
  /// confirmation reaches the surface the Photographer is looking at.
  renderTrashReview(model: TrashReviewViewModel): void;
  closeTrashReview(): void;
  /// Presents the committed recovery counts of the last scan and, while
  /// Originals remain unavailable, the one bounded review entry.
  setRecoveryNotice(
    model: Readonly<{
      relocatedPhotos: number;
      unavailablePhotos: number;
    }>,
  ): void;
  /// Opens the recovery review with the first page of its bounded listing:
  /// entries are the remembered facts, paging names how much of the total is
  /// loaded and whether a continuation page remains.
  openRecoveryPanel(
    entries: ReadonlyArray<RecoveryEntryViewModel>,
    paging: RecoveryPagingViewModel,
  ): void;
  /// Re-renders the entries of the open review, including its paging.
  renderRecoveryEntries(
    entries: ReadonlyArray<RecoveryEntryViewModel>,
    paging: RecoveryPagingViewModel,
  ): void;
  /// Presents the reviewed mappings and their paging; each occupied
  /// destination that an explicit retire-and-bind may replace carries its
  /// checkbox.
  renderRecoveryProposals(
    mappings: ReadonlyArray<RecoveryMappingViewModel>,
    paging: RecoveryPagingViewModel,
  ): void;
  clearRecoveryProposals(): void;
  markRecoveryProposalsUnusable(): void;
  resetRecoveryProposalChoices(): void;
  setRecoveryPending(pending: boolean): void;
  setRecoveryMessage(text?: string): void;
  closeRecoveryPanel(): void;
  dispose(): void;
}

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
          <header class="grid-header"><div class="grid-header-row"><button type="button" class="quiet source-toggle" data-source-toggle aria-controls="source-panel" aria-expanded="false"><span class="source-toggle-name" data-grid-compact-title>All Photos</span><span class="source-toggle-indicator" aria-hidden="true">▾</span></button><div class="grid-heading"><h2 id="grid-title" data-grid-title>All Photos</h2><p data-grid-status role="status"></p></div><p class="grid-connection" data-grid-connection role="status" hidden></p><div class="grid-tools" data-grid-tools><button type="button" class="quiet" data-grid-view-options>Options</button><span class="options-flag" data-view-options-flag hidden></span><button type="button" class="quiet" data-grid-select-mode aria-pressed="false">Select mode</button><button type="button" class="quiet" data-removal-open hidden>Remove rejected Photos</button><button type="button" class="quiet" data-removed-open>Trash</button></div><div class="grid-selection" data-grid-selection hidden><p class="grid-selection-count" data-batch-count></p><button type="button" class="quiet" data-grid-multi-done>Done</button></div></div><p class="grid-summary" data-grid-summary role="status" aria-live="polite"></p></header>
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
                  <div class="photo-editor-controls" aria-label="Photo edit">
                    <p class="photo-editor-status" data-photo-editor-status role="status" aria-live="polite"></p>
                    <img class="photo-editor-preview-image" data-photo-editor-preview-image alt="Current edit preview" hidden>
                    <p class="photo-editor-preview-note" data-photo-editor-preview-note hidden></p>
                    <div class="photo-editor-view-actions" role="group" aria-label="Edit view"><button type="button" data-photo-editor-stage="film" aria-pressed="false" disabled>Film</button><button type="button" class="quiet" data-photo-editor-stage="camera" aria-pressed="false">Original reference</button></div>
                    <p class="photo-editor-stage-note" id="photo-editor-stage-note" data-photo-editor-stage-note role="status" hidden></p>
                    <div class="photo-editor-field"><label for="photo-editor-exposure">Exposure</label><output data-photo-editor-exposure-value for="photo-editor-exposure">0.000 EV</output><input id="photo-editor-exposure" data-photo-editor-exposure type="range" min="0" max="1" step="0.001" value="0" aria-label="Exposure" disabled><button type="button" class="quiet" data-photo-editor-reset-exposure disabled>Reset exposure</button></div>
                    <div class="photo-editor-field photo-editor-white-balance">
                      <label for="photo-editor-white-balance-mode">White balance</label>
                      <select id="photo-editor-white-balance-mode" data-photo-editor-white-balance-mode disabled><option value="as-shot">As shot</option><option value="temperature-tint" disabled>Temperature &amp; tint</option></select>
                      <p class="photo-editor-note" data-photo-editor-white-balance-note hidden></p>
                      <div class="photo-editor-field"><label for="photo-editor-temperature">Temperature</label><output data-photo-editor-temperature-value for="photo-editor-temperature">—</output><input id="photo-editor-temperature" data-photo-editor-temperature type="range" min="1000" max="40000" step="1" value="6500" aria-label="Temperature in Kelvin" disabled></div>
                      <div class="photo-editor-field"><label for="photo-editor-tint">Tint</label><output data-photo-editor-tint-value for="photo-editor-tint">—</output><input id="photo-editor-tint" data-photo-editor-tint type="range" min="-150000" max="150000" step="1" value="0" aria-label="Tint" disabled></div>
                      <button type="button" class="quiet" data-photo-editor-reset-white-balance disabled>Reset white balance</button>
                    </div>
                    <div class="photo-editor-actions"><button type="button" class="quiet" data-photo-editor-undo disabled>Undo</button><button type="button" class="quiet" data-photo-editor-redo disabled>Redo</button><button type="button" class="quiet" data-photo-editor-reset disabled>Reset</button><button type="button" class="quiet" data-photo-editor-compare aria-pressed="false" title="Press to compare the current settings with the unadjusted rendering" disabled>Baseline comparison</button><button type="button" data-photo-editor-preview disabled>Refresh preview</button><button type="button" class="quiet" data-photo-editor-rebind hidden disabled>Keep edit for the current file</button><button type="button" class="quiet" data-photo-editor-refresh>Reload edit</button></div>
                    <p class="photo-editor-draft" data-photo-editor-draft hidden role="status"></p>
                    <div class="photo-editor-conflict" data-photo-editor-conflict hidden><p data-photo-editor-conflict-message role="alert"></p><div class="photo-editor-actions"><button type="button" data-photo-editor-use-saved>Use saved edit</button><button type="button" class="quiet" data-photo-editor-reapply>Reapply my changes</button><button type="button" class="quiet" data-photo-editor-discard-draft>Discard draft</button></div></div>
                    <div class="photo-editor-export" aria-label="Export"><p class="photo-editor-export-heading">Export <span data-photo-editor-export-target>Development TIFF</span></p><p class="photo-editor-export-state" data-photo-editor-export-state role="status"></p><div class="photo-editor-actions"><button type="button" data-photo-editor-export-submit>Export Development TIFF</button><button type="button" class="quiet" data-photo-editor-export-cancel hidden>Cancel</button><button type="button" class="quiet" data-photo-editor-export-retry hidden>Retry</button><button type="button" class="quiet" data-photo-editor-export-download hidden>Download</button></div></div>
                    <details class="photo-editor-details"><summary>Details</summary><p data-photo-editor-provenance></p><p data-photo-editor-detail hidden></p><p data-photo-editor-capability hidden></p><p class="photo-editor-fact"><span>White balance</span><span data-photo-editor-white-balance>As shot</span></p><p class="photo-editor-fact"><span>Edit source</span><span data-photo-editor-support>Checking…</span></p><p class="photo-editor-fact"><span>Development Proxy</span><span data-photo-editor-proxy-state>Checking…</span></p><div class="photo-editor-actions"><button type="button" class="quiet" data-photo-editor-proxy-create disabled>Create Development Proxy</button><button type="button" class="quiet" data-photo-editor-proxy-remove hidden disabled>Remove Development Proxy</button></div><p class="photo-editor-fact"><span>Processing</span><span data-photo-editor-processing>Checking…</span></p><p class="photo-editor-fact"><span>Edit Preview</span><span data-photo-editor-preview-state>Checking…</span></p></details>
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
                  <div class="external-metadata" data-external-metadata aria-label="External Metadata"></div>
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
${RECOVERY_PANEL_TEMPLATE}
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
            <header class="removed-header"><h3 id="removed-title">Trash</h3><button type="button" class="quiet" data-removed-close>Close</button></header>
            <p class="removed-status" data-removed-status role="status"></p>
            <div class="removed-selection" data-removed-selection>
              <button type="button" class="quiet" data-removed-select-all>Select all Trash</button>
              <button type="button" class="quiet" data-removed-clear-selection>Clear selection</button>
              <span data-removed-selection-count></span>
              <button type="button" class="quiet" data-removed-restore-selection>Restore selected</button>
              <button type="button" data-removed-delete>Permanently delete selected Originals</button>
            </div>
            <ul class="removed-list" data-removed-list></ul>
            <section class="trash-outcome" data-trash-outcome hidden>
              <p class="trash-outcome-title" data-trash-outcome-title role="status"></p>
              <p class="trash-outcome-counts" data-trash-outcome-counts hidden><span data-trash-outcome-deleted></span> <span data-trash-outcome-changed></span> <span data-trash-outcome-missing></span> <span data-trash-outcome-failed></span> <span data-trash-outcome-pending></span></p>
              <p class="trash-outcome-bytes" data-trash-outcome-bytes hidden></p>
              <ul class="trash-outcome-items" data-trash-outcome-items></ul>
              <div class="trash-outcome-actions" data-trash-outcome-actions hidden><button type="button" class="quiet" data-trash-check-result>Check result</button><button type="button" class="quiet" data-trash-retry-delete>Retry</button></div>
            </section>
            <p class="removed-message" data-removed-message role="alert" hidden></p>
            <footer class="removed-pager"><button type="button" class="quiet" data-removed-previous>Previous</button><span data-removed-page></span><button type="button" class="quiet" data-removed-next>Next</button><button type="button" data-removed-retry hidden>Retry</button></footer><footer class="removed-actions"><button type="button" class="quiet" data-removed-undo hidden>Undo the last removal</button></footer>
          </div>
        </dialog>
        <dialog class="removal-dialog trash-review-dialog" data-trash-review aria-labelledby="trash-review-title">
          <div class="removal-sheet">
            <header class="removal-header"><h3 id="trash-review-title">Permanently delete from Trash</h3><button type="button" class="quiet" data-trash-review-close>Close</button></header>
            <div class="removal-body">
              <p class="removal-summary" data-trash-review-summary></p>
              <p class="removal-message trash-review-warning">Deletion removes each Photo from every Album that contains it. Slipstream cannot undo it.</p>
              <p class="removal-summary" data-trash-review-albums></p>
              <ul class="trash-review-items" data-trash-review-items></ul>
              <div class="trash-review-rejected" data-trash-review-rejected hidden>
                <p class="removal-summary" data-trash-review-rejected-heading></p>
                <ul class="trash-review-rejected-list" data-trash-review-rejected-list></ul>
              </div>
            </div>
            <footer class="removal-actions"><button type="button" data-trash-confirm hidden></button><button type="button" class="quiet" data-trash-cancel>Cancel</button></footer>
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
  const albumFormDialog = required<HTMLDialogElement>(
    root,
    "[data-album-form-dialog]",
  );
  const albumFormBody = required<HTMLElement>(root, "[data-album-form-body]");
  const albumResume = required<HTMLButtonElement>(root, "[data-album-resume]");
  const summaryStatus = required<HTMLElement>(root, "[data-summary-status]");
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
  const removedSelectAll = required<HTMLButtonElement>(
    root,
    "[data-removed-select-all]",
  );
  const removedClearSelection = required<HTMLButtonElement>(
    root,
    "[data-removed-clear-selection]",
  );
  const removedSelectionCount = required<HTMLElement>(
    root,
    "[data-removed-selection-count]",
  );
  const removedDelete = required<HTMLButtonElement>(
    root,
    "[data-removed-delete]",
  );
  const removedRestoreSelection = required<HTMLButtonElement>(
    root,
    "[data-removed-restore-selection]",
  );
  const removedUndo = required<HTMLButtonElement>(root, "[data-removed-undo]");
  const trashOutcome = required<HTMLElement>(root, "[data-trash-outcome]");
  const trashOutcomeTitle = required<HTMLElement>(
    root,
    "[data-trash-outcome-title]",
  );
  const trashOutcomeCounts = required<HTMLElement>(
    root,
    "[data-trash-outcome-counts]",
  );
  const trashOutcomeDeleted = required<HTMLElement>(
    root,
    "[data-trash-outcome-deleted]",
  );
  const trashOutcomeChanged = required<HTMLElement>(
    root,
    "[data-trash-outcome-changed]",
  );
  const trashOutcomeMissing = required<HTMLElement>(
    root,
    "[data-trash-outcome-missing]",
  );
  const trashOutcomeFailed = required<HTMLElement>(
    root,
    "[data-trash-outcome-failed]",
  );
  const trashOutcomePending = required<HTMLElement>(
    root,
    "[data-trash-outcome-pending]",
  );
  const trashOutcomeBytes = required<HTMLElement>(
    root,
    "[data-trash-outcome-bytes]",
  );
  const trashOutcomeItems = required<HTMLElement>(
    root,
    "[data-trash-outcome-items]",
  );
  const trashOutcomeActions = required<HTMLElement>(
    root,
    "[data-trash-outcome-actions]",
  );
  const trashOutcomeCheck = required<HTMLButtonElement>(
    root,
    "[data-trash-check-result]",
  );
  const trashOutcomeRetry = required<HTMLButtonElement>(
    root,
    "[data-trash-retry-delete]",
  );
  const trashReviewDialog = required<HTMLDialogElement>(
    root,
    "[data-trash-review]",
  );
  const trashReviewSummary = required<HTMLElement>(
    root,
    "[data-trash-review-summary]",
  );
  const trashReviewAlbums = required<HTMLElement>(
    root,
    "[data-trash-review-albums]",
  );
  const trashReviewItems = required<HTMLElement>(
    root,
    "[data-trash-review-items]",
  );
  const trashReviewRejected = required<HTMLElement>(
    root,
    "[data-trash-review-rejected]",
  );
  const trashReviewRejectedHeading = required<HTMLElement>(
    root,
    "[data-trash-review-rejected-heading]",
  );
  const trashReviewRejectedList = required<HTMLElement>(
    root,
    "[data-trash-review-rejected-list]",
  );
  const trashReviewConfirm = required<HTMLButtonElement>(
    root,
    "[data-trash-confirm]",
  );
  const trashReviewCancel = required<HTMLButtonElement>(
    root,
    "[data-trash-cancel]",
  );
  const trashReviewClose = required<HTMLButtonElement>(
    root,
    "[data-trash-review-close]",
  );
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
  const ratingDialog = required<HTMLDialogElement>(
    root,
    "[data-rating-choices]",
  );
  const ratingChoicesClose = required<HTMLButtonElement>(
    root,
    "[data-rating-choices-close]",
  );
  const ratings = required<HTMLElement>(root, "[data-ratings]");
  const selectFeedback = required<HTMLElement>(root, "[data-select-feedback]");
  const rejectFeedback = required<HTMLElement>(root, "[data-reject-feedback]");

  const send = (intent: LibraryBrowserIntent): void => {
    if (alive) emit(intent);
  };
  const membershipPanelController = createMembershipPanel({
    elements: {
      membershipStatus: required<HTMLElement>(root, "[data-membership-status]"),
      membershipList: required<HTMLElement>(root, "[data-membership-list]"),
      membershipMessage: required<HTMLElement>(
        root,
        "[data-membership-message]",
      ),
      membershipManage: required<HTMLButtonElement>(
        root,
        "[data-membership-manage]",
      ),
      membershipRetry: required<HTMLButtonElement>(
        root,
        "[data-membership-retry]",
      ),
      membershipPanel: required<HTMLElement>(root, "[data-membership-panel]"),
      membershipOptions: required<HTMLElement>(
        root,
        "[data-membership-options]",
      ),
    } satisfies MembershipPanelElements,
    send,
  });

  /// The one page-UI controller for the shared native-modal lifecycle. Every
  /// supporting surface registers its dialog here, so at most one is active
  /// and every dismissal converges on one cleanup path.
  const surfaces = createModalSurfaces();
  /// The disclosure state of the two Photo View entries mirrors the active
  /// surface, so every supporting-surface transition updates both controls.
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
  const recoveryPanelController = createRecoveryPanel({
    elements: recoveryPanelElements(root),
    send,
    surfaces,
    selectionLabel,
  });
  /// The removal review, the Removed Photos listing, and the
  /// permanent-deletion review present through their own controller, which
  /// registers its surfaces here so the one active-surface order the page
  /// keeps is unchanged.
  const removedPanels = createRemovedPanels({
    elements: {
      removalOpen,
      removalDialog,
      removalSummary,
      removalMessage,
      removalConfirm,
      removalUndo,
      removalClose,
      removedOpen,
      removedPanel,
      removedStatus,
      removedList,
      removedMessage,
      removedPrevious,
      removedNext,
      removedPage,
      removedRetry,
      removedClose,
      removedSelectAll,
      removedClearSelection,
      removedSelectionCount,
      removedDelete,
      removedRestoreSelection,
      removedUndo,
      trashOutcome,
      trashOutcomeTitle,
      trashOutcomeCounts,
      trashOutcomeDeleted,
      trashOutcomeChanged,
      trashOutcomeMissing,
      trashOutcomeFailed,
      trashOutcomePending,
      trashOutcomeBytes,
      trashOutcomeItems,
      trashOutcomeActions,
      trashOutcomeCheck,
      trashOutcomeRetry,
      trashReviewDialog,
      trashReviewSummary,
      trashReviewAlbums,
      trashReviewItems,
      trashReviewRejected,
      trashReviewRejectedHeading,
      trashReviewRejectedList,
      trashReviewConfirm,
      trashReviewCancel,
      trashReviewClose,
    },
    send,
    surfaces,
    bindThumbnail,
    releaseThumbnail,
    gridThumbnailTarget,
  });

  let photoStatusSurface: object = {};
  let sourceModel: SourceListViewModel | undefined;
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
  let photoSurface: object = {};
  let currentPhotoId: string | undefined;
  let currentSelection: ViewSelectionState = "undecided";
  let decisionInteractionEnabled = false;

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
  const stripInTools = () => photoToolsController.isNearbyOpen();
  const filmstripPresenter: FilmstripPresenter = createFilmstripPresenter({
    elements: { photoView, filmstrip, filmstripHost, filmstripTools },
    layout: {
      isDisclosed: stripIsDisclosed,
      isNearbyOpen: stripInTools,
      requestResize: () => send({ kind: "filmstrip-resize" }),
    },
    thumbnails: {
      target: gridThumbnailTarget,
      bind: bindThumbnail,
      release: releaseThumbnail,
    },
    openPhoto: (index) => send({ kind: "open-photo", index }),
  });
  const gridPresenter = createGridPresenter({
    browser,
    gridView,
    viewport: gridViewport,
    canvas: gridCanvas,
    layer: gridLayer,
    sizeSelect,
    gridTools,
    gridSelection,
    gridBatch,
    compact: compactSources,
    isAlive: () => alive,
    multiMode: () => gridMultiMode,
    multiCount: () => gridMultiCount,
    multiSelected: (index) => gridMultiSelected(index),
    renderBatch: () => renderBatch(),
    send,
    bindThumbnail,
    releaseThumbnail,
  });
  /// Opens the explicit Rating choices. Only this surface or the Rating entry
  /// owns explicit Rating interaction at one time; the Rating Wheel stays the
  /// touch accelerator on the Preview.
  const openRatingChoices = () => {
    if (!alive || photoView.hidden) return;
    resetGestures();
    ratingControls.openChoices();
  };
  const closeRatingChoices = (restoreFocus = true) => {
    if (!alive) return;
    ratingControls.closeChoices(restoreFocus);
  };
  const setGridThumbnailSize = (size: "small" | "medium" | "large") =>
    gridPresenter.setSize(size);

  const resetGestures = () => photoGestures?.reset();
  const photoToolsController = createPhotoToolsController({
    elements: {
      photoToolsDialog,
      photoToolsClose,
      photoToolsTitle,
      photoToolsViews,
      photoToolsEntries,
    },
    surfaces,
    isPhotoVisible: () => !photoView.hidden,
    currentPhotoId: () => currentPhotoId,
    resetGestures,
    syncFilmstripHost: filmstripPresenter.syncHost,
    syncSecondarySurface,
    openSources: () => sourceController.open(),
    openEditor: (photoId) => editorController.open(photoId),
  });
  const editorController = createPhotoEditorSurfaceController({
    root,
    send,
    isPhotoVisible: () => !photoView.hidden,
    isEditorSurfaceVisible: () =>
      surfaces.isActive("photo-tools") &&
      photoToolsController.view() === "edit",
    openEditorSurface: () => photoToolsController.open("edit"),
  });
  const ratingControls = createRatingControls({
    elements: {
      ratingDialog,
      ratingChoicesClose,
      ratings,
      ratingWheel,
      ratingWheelInstructions,
      ratingWheelStatus,
      ratingWheelOptions,
      preview,
      rating,
      dockRating,
    },
    surfaces,
    send,
    onSurfaceChange: syncSecondarySurface,
  });
  const sourceController = createSourceSurfaceController({
    elements: {
      browser,
      sourceDialog,
      sourceResizer,
      sourceToggle,
      photoSourceToggle,
      sourceClose,
    },
    surfaces,
    isModal: () => compactSources.matches || !photoView.hidden,
    resetGestures,
    closePhotoTools: (restoreFocus) => photoToolsController.close(restoreFocus),
    closeRatingChoices: (restoreFocus) =>
      ratingControls.closeChoices(restoreFocus),
  });
  const onSourceViewportChange = () => {
    if (!alive) return;
    sourceController.syncLayout();
    presentConnection();
    syncSecondarySurface();
    filmstripPresenter.syncHost();
  };
  const viewOptionsController = createViewOptions({
    elements: {
      viewOptionsDialog: required<HTMLDialogElement>(
        root,
        "[data-view-options]",
      ),
      viewOptionsOpen: required<HTMLButtonElement>(
        root,
        "[data-grid-view-options]",
      ),
      viewOptionsFlag: required<HTMLElement>(root, "[data-view-options-flag]"),
      viewOptionsClose: required<HTMLButtonElement>(
        root,
        "[data-view-options-close]",
      ),
      viewOptionsApply: required<HTMLButtonElement>(
        root,
        "[data-view-options-apply]",
      ),
      viewOptionsCancel: required<HTMLButtonElement>(
        root,
        "[data-view-options-cancel]",
      ),
      albumResume: required<HTMLButtonElement>(root, "[data-album-resume]"),
      sortSelect: required<HTMLSelectElement>(root, "[data-sort-select]"),
      optionsProgress: required<HTMLElement>(
        root,
        "[data-grid-source-progress]",
      ),
      optionsVisibleResults: required<HTMLElement>(
        root,
        "[data-grid-visible-results]",
      ),
      filterSelect: required<HTMLSelectElement>(root, "[data-filter-select]"),
      sizeSelect,
    } satisfies ViewOptionsElements,
    surfaces,
    send,
    resetGestures,
    closePhotoTools: (restoreFocus) => photoToolsController.close(restoreFocus),
    closeRatingChoices,
    activeAlbumId: () =>
      sourceModel?.albums.find((candidate) => candidate.active)?.id,
    onSizeChange: setGridThumbnailSize,
  });
  const albumFormController = createAlbumForm({
    elements: { albumFormDialog, albumFormBody },
    surfaces,
    send,
    resetGestures,
    findFocusTarget: (focusKey) =>
      Array.from(
        sourceList.querySelectorAll<HTMLElement>("[data-focus-key]"),
      ).find((candidate) => candidate.dataset.focusKey === focusKey),
  });
  const zoomController = createPhotoZoomController({
    preview,
    stage,
    controls: zoomControls,
    fit: zoomFit,
    out: zoomOut,
    inButton: zoomIn,
    hundred: zoom100,
    slider: zoomSlider,
    level: zoomLevel,
    isAlive: () => alive,
  });
  const photoGestures = createPhotoGestures({
    preview,
    stage,
    selectFeedback,
    rejectFeedback,
    zoom: zoomController,
    rating: ratingControls,
    isAlive: () => alive,
    currentPhotoId: () => currentPhotoId,
    currentSurface: () => photoSurface,
    decisionEnabled: () => decisionInteractionEnabled,
    send,
  });

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
    newAlbum.dataset.focusKey = albumFormController.actionFocusKey("create");
    newAlbum.addEventListener("click", () =>
      albumFormController.open("create"),
    );
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
      row.append(button, albumFormController.createAlbumTools(album));
      sourceList.append(row);
    }
    const focusTarget = (focusKey: string) =>
      Array.from(
        sourceList.querySelectorAll<HTMLElement>("[data-focus-key]"),
      ).find((candidate) => candidate.dataset.focusKey === focusKey);
    if (albumFormController.restoreSourceFocus(focusTarget)) return;
    const restored = focusedKey ? focusTarget(focusedKey) : undefined;
    if (restored && !restored.matches(":disabled")) {
      restored.focus();
      return;
    }
  };

  const scheduleGridRender = gridPresenter.schedule;
  const cancelGridRender = gridPresenter.cancel;
  const clearGridCells = gridPresenter.clear;
  const rebindDetachedGridCells = gridPresenter.rebindDetached;
  const applyGridMultiSelection = gridPresenter.applyMulti;
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
  const renderGrid = (model: GridViewModel, position?: number) => {
    gridMultiMode = model.multi.mode;
    gridMultiCount = model.multi.count;
    gridMultiLimit = model.multi.limit;
    gridMultiEnabled = model.multi.enabled;
    gridMultiResult = model.multi.result;
    gridMultiSelected = model.multi.selected;
    gridPresenter.render(model, position);
  };

  const renderPhotoFacts = (model: PhotoFactsViewModel) => {
    if (!alive) return;
    position.textContent = `${model.index + 1} / ${model.total}`;
    photoFilename.textContent = model.originalFilename ?? "—";
    photoFilename.title = model.originalFilename ?? "";
    currentSelection = model.selectionState ?? "undecided";
    selection.textContent = selectionLabel(currentSelection);
    ratingControls.render(model.rating ?? 0);
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
    zoomController?.resetForImage();
    // Fit depends on the Preview's natural pixels, so the geometry is
    // applied when the bytes arrive.
    image.addEventListener("load", () => {
      if (!alive || !image.isConnected) return;
      zoomController?.applyZoom();
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
      zoomController?.resetForImage();
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
      if (!modifier && !event.shiftKey) gridPresenter.handleKey(event);
      return;
    }
    if (modifier) return;
    const zoom = zoomController;
    if (!zoom) return;
    if (event.key === "+" || event.key === "=") {
      if (!zoom.hasMeasurableImage()) return;
      event.preventDefault();
      zoom.zoomIn();
      return;
    }
    if (event.key === "-" || event.key === "_") {
      if (!zoom.hasMeasurableImage()) return;
      event.preventDefault();
      zoom.zoomOut();
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
      zoom.applyFit();
    } else if (event.key.toLowerCase() === "d") {
      event.preventDefault();
      zoom.toggleDetail();
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
    filmstripPresenter.syncHost();
    send({ kind: "filmstrip-resize" });
  };
  shortViewport.addEventListener("change", onShortViewportChange);
  window.addEventListener("keydown", keydown);
  const stageObserver = new ResizeObserver(() => {
    if (!alive) return;
    // Fit is recomputed and a manual percentage keeps its value, while a
    // shrinking stage re-clamps how far the Preview may be panned.
    zoomController?.clampPan();
    zoomController?.applyZoom();
  });
  stageObserver.observe(stage);
  back.addEventListener("click", () => send({ kind: "show-grid" }));
  refresh.addEventListener("click", () => send({ kind: "refresh" }));
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
  dockMore.addEventListener("click", () => photoToolsController.open());
  for (const button of photoToolsReturnButtons)
    button.addEventListener("click", () =>
      photoToolsController.returnToTools(),
    );
  photoToolsClear.addEventListener("click", () =>
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: "undecided",
      advance: false,
    }),
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
  sourceController.syncLayout();
  syncSecondarySurface();
  // The strip's home is only placed once a Photo can present it: the markup
  // already holds it beside the Preview, which is where a wide layout shows
  // it, and a compact layout moves it into Photo tools when that disclosure
  // opens. Placing it here would emit an intent while the page model that
  // owns the strip's facts is still being constructed.

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
      filmstripPresenter.clear();
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
      viewOptionsController.renderSort(model);
    },
    renderFilter(model) {
      if (!alive) return;
      viewOptionsController.renderFilter(model);
    },
    renderProgress(model) {
      if (!alive) return;
      viewOptionsController.renderProgress(model);
    },
    setControls(model) {
      if (!alive) return;
      // A disabled cell cannot hold focus. The Grid keeps its keyboard
      // position on the viewport instead of losing it to the page while a
      // write settles or a source changes readiness, and takes the cell back
      // when the Grid becomes interactive again.
      gridPresenter.setInteractive(model.gridEnabled);
      decisionInteractionEnabled = model.decisionEnabled;
      if (!model.decisionEnabled) photoGestures?.cancelUnavailableDecision();
      ratingControls.setDecisionEnabled(model.decisionEnabled);
      dockSelect.disabled = !model.decisionEnabled;
      dockReject.disabled = !model.decisionEnabled;
      photoToolsClear.disabled = !model.clearEnabled;
      back.disabled = !model.backEnabled;
      refresh.disabled = !model.refreshEnabled;
      retry.disabled = !model.recoveryEnabled;
      retryPhoto.disabled = !model.recoveryEnabled;
      dockPrevious.disabled = !model.previousEnabled;
      dockNext.disabled = !model.nextEnabled;
      photoToolsUndo.disabled = !model.undoEnabled;
      removedPanels.setRemovalEnabled(model.removalEnabled);
      filmstripPresenter.setInteractive(model.filmstripEnabled);
      zoomController?.applyZoom();
    },
    renderMembership(model) {
      if (!alive) return;
      membershipPanelController.render(model);
    },
    prepareSourceOpen(name) {
      if (!alive) return;
      const returnFocus = surfaces.isActive("sources");
      cancelGridRender();
      resetGestures();
      photoToolsController.close(false);
      closeRatingChoices(false);
      stage.replaceChildren();
      zoomController?.resetForImage();
      gridView.hidden = false;
      photoView.hidden = true;
      clearGridCells();
      filmstripPresenter.clear();
      sourceController.close(false);
      sourceController.syncLayout();
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
      gridPresenter.resetKeyboard();
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
      gridPresenter.scrollTo(index);
    },
    captureGridRestoration() {
      return gridPresenter.captureRestoration();
    },
    restoreGridAnchor(model) {
      gridPresenter.restoreAnchor(model);
    },
    closeTransientSurfaces() {
      if (!alive) return;
      resetGestures();
      photoToolsController.close(false);
      closeRatingChoices(false);
      // A destination change supersedes every supporting surface: the Album
      // form's draft is discarded and the surfaces close without returning
      // focus, because the destination render owns focus next.
      if (albumFormController.discard()) {
        surfaces.closeAll();
        if (!gridView.hidden) gridViewport.focus();
        else photoView.focus();
        return;
      }
      surfaces.closeAll();
    },
    focusGridIndex(index) {
      gridPresenter.focusIndex(index);
    },
    showGrid(index) {
      if (!alive) return;
      resetGestures();
      photoToolsController.close(false);
      closeRatingChoices(false);
      zoomController?.resetForImage();
      photoView.hidden = true;
      gridView.hidden = false;
      // Photo View detached the owner's Grid images, so the visible Grid
      // rebuilds its cells and re-attaches every thumbnail it still shows.
      clearGridCells();
      filmstripPresenter.clear();
      sourceController.close(false);
      sourceController.syncLayout();
      presentConnection();
      gridViewport.focus();
      // Returning from Photo View returns the Grid keyboard to that Photo
      // cell; the merged render focuses it once it is rendered.
      gridPresenter.returnFromPhoto(index);
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
      sourceController.syncLayout();
      filmstripPresenter.syncHost();
      presentConnection();
      photoView.focus();
      zoomController?.resetForImage();
      photoSurface = {};
    },
    renderFilmstrip: filmstripPresenter.render,
    renderPhotoFacts,
    renderPhotoMetadata,
    renderPhotoShell,
    editorVisible: () => editorController.visible(),
    renderEditor: (model) => editorController.render(model),
    presentEditorPreview: (url) => editorController.presentPreview(url),
    clearEditorPreview: () => editorController.clearPreview(),
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
        zoomController?.resetForImage();
      }
    },
    setPreviewFacts(value, isLimited) {
      if (!alive) return;
      applyPreviewFact(value, isLimited);
    },
    setAlbumFormMessage(formId, message) {
      if (!alive) return;
      albumFormController.setMessage(formId, message);
    },
    setAlbumFormPending(formId, pending, name) {
      if (!alive) return;
      albumFormController.setPending(formId, pending, name);
    },
    dismissAlbumForm(formId) {
      if (!alive) return;
      albumFormController.dismiss(formId);
    },
    openRemovalReview(model) {
      if (!alive) return;
      removedPanels.openRemovalReview(model);
    },
    renderRemovalReview(model) {
      if (!alive) return;
      removedPanels.renderRemovalReview(model);
    },
    closeRemovalReview() {
      if (!alive) return;
      removedPanels.closeRemovalReview();
    },
    openRemovedPanel(model) {
      if (!alive) return;
      removedPanels.openRemovedPanel(model);
    },
    renderRemovedPanel(model) {
      if (!alive) return;
      removedPanels.renderRemovedPanel(model);
    },
    closeRemovedPanel() {
      if (!alive) return;
      removedPanels.closeRemovedPanel();
    },
    openTrashReview(model) {
      if (!alive) return;
      removedPanels.openTrashReview(model);
    },
    renderTrashReview(model) {
      if (!alive) return;
      removedPanels.renderTrashReview(model);
    },
    closeTrashReview() {
      if (!alive) return;
      removedPanels.closeTrashReview();
    },
    setRecoveryNotice(model) {
      if (!alive) return;
      recoveryPanelController.setRecoveryNotice(model);
    },
    openRecoveryPanel(entries, paging) {
      if (!alive) return;
      recoveryPanelController.openRecoveryPanel(entries, paging);
    },
    renderRecoveryEntries(entries, paging) {
      if (!alive) return;
      recoveryPanelController.renderRecoveryEntries(entries, paging);
    },
    renderRecoveryProposals(mappings, paging) {
      if (!alive) return;
      recoveryPanelController.renderRecoveryProposals(mappings, paging);
    },
    clearRecoveryProposals() {
      if (!alive) return;
      recoveryPanelController.clearRecoveryProposals();
    },
    markRecoveryProposalsUnusable() {
      if (!alive) return;
      recoveryPanelController.markRecoveryProposalsUnusable();
    },
    resetRecoveryProposalChoices() {
      if (!alive) return;
      recoveryPanelController.resetRecoveryProposalChoices();
    },
    setRecoveryPending(pending) {
      if (!alive) return;
      recoveryPanelController.setRecoveryPending(pending);
    },
    setRecoveryMessage(text) {
      if (!alive) return;
      recoveryPanelController.setRecoveryMessage(text);
    },
    closeRecoveryPanel() {
      if (!alive) return;
      recoveryPanelController.closeRecoveryPanel();
    },
    dispose() {
      if (!alive) return;
      alive = false;
      resetGestures();
      removedPanels.dispose();
      recoveryPanelController.dispose();
      membershipPanelController.dispose();
      albumFormController.dispose();
      photoToolsController.dispose();
      editorController.dispose();
      ratingControls.dispose();
      viewOptionsController.dispose();
      stageObserver.disconnect();
      filmstripPresenter.dispose();
      zoomController?.dispose();
      photoGestures?.dispose();
      sourceController.dispose();
      gridPresenter.dispose();
      compactSources.removeEventListener("change", onSourceViewportChange);
      mobileActionHierarchy.removeEventListener(
        "change",
        onSourceViewportChange,
      );
      shortViewport.removeEventListener("change", onShortViewportChange);
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

function sourceLabel(source?: ViewPreviewSource): string {
  return source === "jpeg-original"
    ? "JPEG"
    : source === "raw-embedded-jpeg"
      ? "RAW embedded JPEG"
      : "—";
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
