/// The Removed Photos listing's view models, owned by the model that computes
/// them: the listing owner builds the panel projection and the
/// permanent-deletion review the removed panels present. The UI imports
/// these contracts rather than the model importing the UI.

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

/// The permanent-deletion review: the reviewed files, their Locations,
/// kinds, sizes, and affected Albums, and the rejections the server refused.
/// Opening the surface deletes nothing; only the confirmation does, and
/// Slipstream cannot undo that confirmation.
export type TrashReviewViewModel = Readonly<{
  itemCount: number;
  /// The summed logical size of the reviewed Originals.
  totalBytes: number;
  albumCount: number;
  albumNames: ReadonlyArray<string>;
  items: ReadonlyArray<
    Readonly<{
      photoId: string;
      originalLocation: string;
      originalKind: "raw" | "jpeg";
      size: number;
      albumNames: ReadonlyArray<string>;
    }>
  >;
  rejected: ReadonlyArray<Readonly<{ photoId: string; reasonLabel: string }>>;
  /// False when every selected item was rejected: the rejections are shown
  /// and the surface offers no delete action.
  canConfirm: boolean;
  /// The exact final action label the contract names.
  confirmLabel: string;
  deleting: boolean;
}>;

/// The outcome of one Trash deletion operation, or the state of an operation
/// whose response was lost. A lost response is never presented as proof of
/// failure or success: it names only the retained operation id.
export type TrashOutcomeViewModel =
  | Readonly<{
      /// The delete response was lost: the surface offers Check result and
      /// Retry instead of counts it does not have.
      unconfirmed: true;
    }>
  | Readonly<{
      unconfirmed?: false;
      deleted: number;
      changed: number;
      missing: number;
      failed: number;
      /// The items whose outcome is not settled yet: `pending`, `deleting`,
      /// and `uncertain` states.
      pendingVerification: number;
      /// Logical bytes the server credited to confirmed deletions only.
      logicalBytesDeleted: number;
      items: ReadonlyArray<
        Readonly<{
          photoId: string;
          state:
            | "pending"
            | "deleting"
            | "missing"
            | "changed"
            | "failed"
            | "uncertain";
          originalLocation: string;
          originalKind: "raw" | "jpeg";
          size: number | null;
          message?: string;
        }>
      >;
    }>;

/// One bounded page of removed Photos, newest removal first. Each item carries
/// the facts the Grid and Photo View already present, so the Photographer
/// recognizes what is recoverable before restoring it.
export type RemovedPanelViewModel = Readonly<{
  status: "loading" | "ready" | "failed";
  start: number;
  total: number;
  limit: number;
  pending: boolean;
  deleting: boolean;
  selectionCount: number;
  /// True when review will resolve all current Trash minus exclusions at
  /// admission; the count reflects the most recent complete scan.
  allSelected: boolean;
  canDelete: boolean;
  canRestore: boolean;
  /// Whether the last read of the listing failed, so the surface offers the
  /// one control that repeats it.
  canRetry: boolean;
  restoringPhotoId?: string;
  restoringSelection: boolean;
  /// A prior all-mode Restore has no confirmed response and must be retried
  /// under its retained operation identity before another selection or write.
  restoreUnconfirmed?: boolean;
  message?: string;
  /// The last confirmed operation, while Undo can still restore it. The
  /// listing is the surface a Photographer returns to, so the operation-level
  /// recovery is offered here and not only beside the confirmation.
  undo?: Readonly<{ removed: number }>;
  /// The outcome of the last Trash deletion operation this panel confirmed
  /// or recovered. It stays visible until the panel is reloaded or dismissed.
  outcome?: TrashOutcomeViewModel;
  /// The retained-operation action in flight, so its control cannot be
  /// re-entered while it settles.
  outcomeBusy?: "check" | "resume";
  items: ReadonlyArray<
    Readonly<{
      photoId: string;
      filename: string;
      originalLocation: string;
      originalKind: "raw" | "jpeg";
      originalSize: number | null;
      removedAtMs: number;
      /// Non-null while the item's permanent-deletion outcome is not settled:
      /// the item cannot be selected or restored, and its row offers the
      /// recovery that names the retained operation id.
      pendingVerificationOperationId: string | null;
      selected: boolean;
      /// True when the Photographer excluded this item from a Select all
      /// selection; excluded items are not submitted for review.
      excluded: boolean;
      preview: Readonly<{
        state: "inspection-pending" | "ready" | "unavailable" | "failed";
        thumbnailUrl?: string;
      }>;
    }>
  >;
}>;
