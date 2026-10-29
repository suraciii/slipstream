import type { EditorIntent } from "../model/editor-intent-contract.js";
import type { RecoveryApplyMapping } from "../model/recovery-review.js";
import type { GridPhotoViewModel } from "./grid-cell-presenter.js";

/// The pure view contract of the Library Browser: the intent union and the
/// view-model shapes the page model and the UI presenters exchange. It owns no
/// runtime code and no DOM, so every presenter and the page model can depend
/// on these types without a cycle back into the view.

export type ViewSelectionState = "unflagged" | "picked" | "rejected";

export type SourceReference =
  | Readonly<{ kind: "library" }>
  | Readonly<{ kind: "album"; id: string }>
  | Readonly<{ kind: "folder"; location: string; name: string }>;

export type AlbumFormReference = Readonly<{
  formId: string;
  kind: "create" | "rename" | "delete";
  albumId?: string;
  name: string;
  contextual?: boolean;
}>;

export type ViewSourceOrder =
  | "source-default"
  | "capture-time-asc"
  | "capture-time-desc";

/// One Selection State filter for the open source. `all` keeps every Photo of
/// the source order.
export type ViewSelectionFilter = "all" | ViewSelectionState;

/// The Original File kind View options may request.
export type ViewOriginalKind = "raw" | "jpeg";

/// The defined Photo fact conditions View options stages until Apply. Every
/// field is optional, an omitted field applies no condition, and different
/// fields combine with AND. They carry the same meaning as the CLI query
/// contract, so a Web link never names a condition the CLI cannot express.
export type ViewFilterConditions = Readonly<{
  ratingMinimum?: number;
  ratingMaximum?: number;
  kind?: ViewOriginalKind;
  available?: boolean;
  capturedFrom?: string;
  capturedBefore?: string;
}>;

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
  /// The committed fact conditions of the open source.
  conditions: ViewFilterConditions;
}>;

/// Decision progress for the open source: the per-state counts the server
/// reports for the complete source, plus the number of Photos in the currently
/// visible filtered sequence.
export type GridProgressViewModel = Readonly<{
  visible: boolean;
  visibleTotal: number;
  sourceTotal: number;
  picked: number;
  rejected: number;
  unflagged: number;
}>;

export type LibraryBrowserIntent =
  | EditorIntent
  | Readonly<{ kind: "summary-action"; presentationId: number }>
  | Readonly<{ kind: "sign-out" }>
  /// Commits the View options draft once. A size-only change never reaches the
  /// page model: it is Grid presentation and keeps the open Snapshot and its
  /// anchor.
  | Readonly<{
      kind: "view-options-apply";
      order: ViewSourceOrder;
      selection: ViewSelectionFilter;
      conditions: ViewFilterConditions;
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
  | Readonly<{ kind: "album-overview-retry" }>
  | Readonly<{ kind: "album-form-open"; form: AlbumFormReference }>
  | Readonly<{ kind: "album-form-close"; formId: string }>
  | Readonly<{
      kind: "album-form-submit";
      formId: string;
      name?: string;
    }>
  /// Opens one random Photo of the current Browse Snapshot: the source, the
  /// committed order, and the committed Selection State filter of the open
  /// view, sampled through its bounded windows. It carries no position, seed,
  /// or source of its own, so the page model owns the whole decision.
  | Readonly<{ kind: "random-photo" }>
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
  | Readonly<{ kind: "removed-cleanup" }>
  | Readonly<{ kind: "removed-copy-locations" }>
  | Readonly<{ kind: "removed-restore-selected" }>
  | Readonly<{
      kind: "removed-restore";
      photoId: string;
      removedAtMs: number;
    }>
  /// Confirm or cancel the explicit record-only cleanup review.
  | Readonly<{ kind: "trash-review-confirm" }>
  | Readonly<{ kind: "trash-review-cancel" }>
  /// Read the retained cleanup outcome without repeating the mutation.
  | Readonly<{ kind: "trash-check-result" }>
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

export type FolderViewModel = Readonly<{
  location: string;
  name: string;
  photoCount: number;
  hasDescendantFolders: boolean;
  expanded: boolean;
  loading: boolean;
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
  fileLocationFailures: ReadonlyArray<
    Readonly<{ key: string; range: string; parent: string; loading: boolean }>
  >;
  rootExpanded: boolean;
  rootLoading: boolean;
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

export type GridViewModel = Readonly<{
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

type MembershipAlbumViewModel = Readonly<{ id: string; name: string }>;

export type MembershipViewModel = Readonly<{
  photoId?: string;
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
