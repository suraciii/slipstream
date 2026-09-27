/// The Library browser page's Removed Photos projections: the removal
/// review, the bounded Removed Photos listing (the Trash), and the
/// permanent-deletion review above it.
///
/// The controller owns presentation only. It binds the surfaces' controls,
/// presents the view models the page model builds, and emits the intents the
/// page model already names. It owns no routes, no removal state, and no
/// image delivery: Thumbnails travel through the view's one bind/release
/// path, and the shared modal-surface controller owns which surface is
/// active, where focus returns, and the single cleanup path every close
/// converges on.

import type { ModalSurfaces } from "./modal-surface.js";
import { formatPhotoCount } from "./photo-count.js";
import type {
  GridPhotoPreview,
  GridThumbnailBinding,
  GridThumbnailTarget,
  LibraryBrowserIntent,
} from "./library-browser-view.js";

/// The intents the Removed Photos surfaces emit, as the page model names
/// them: the review, listing, and permanent-deletion actions of the removal
/// path and nothing else.
type RemovedPanelIntentKind =
  | "removal-review-open"
  | "removal-review-close"
  | "removal-confirm"
  | "removal-undo"
  | "removed-list-open"
  | "removed-list-close"
  | "removed-page"
  | "removed-retry"
  | "removed-select-all"
  | "removed-clear-selection"
  | "removed-toggle"
  | "removed-delete"
  | "removed-restore-selected"
  | "removed-restore"
  | "trash-review-confirm"
  | "trash-review-cancel"
  | "trash-check-result"
  | "trash-retry-delete"
  | "trash-row-check"
  | "trash-row-resume";

export type RemovedPanelIntent = Extract<
  LibraryBrowserIntent,
  { kind: RemovedPanelIntentKind }
>;

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
  start: number;
  total: number;
  limit: number;
  pending: boolean;
  deleting: boolean;
  selectionCount: number;
  canDelete: boolean;
  canRestore: boolean;
  /// Whether the last read of the listing failed, so the surface offers the
  /// one control that repeats it.
  canRetry: boolean;
  restoringPhotoId?: string;
  restoringSelection: boolean;
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
      preview: GridPhotoPreview;
    }>
  >;
}>;

/// The Removed Photos surfaces' controls, looked up by the view that owns the
/// markup: the removal review, the listing, the Trash outcome, and the
/// permanent-deletion review.
export type RemovedPanelsElements = Readonly<{
  removalOpen: HTMLButtonElement;
  removalDialog: HTMLDialogElement;
  removalSummary: HTMLElement;
  removalMessage: HTMLElement;
  removalConfirm: HTMLButtonElement;
  removalUndo: HTMLButtonElement;
  removalClose: HTMLButtonElement;
  removedOpen: HTMLButtonElement;
  removedPanel: HTMLDialogElement;
  removedStatus: HTMLElement;
  removedList: HTMLElement;
  removedMessage: HTMLElement;
  removedPrevious: HTMLButtonElement;
  removedNext: HTMLButtonElement;
  removedPage: HTMLElement;
  removedRetry: HTMLButtonElement;
  removedClose: HTMLButtonElement;
  removedSelectAll: HTMLButtonElement;
  removedClearSelection: HTMLButtonElement;
  removedSelectionCount: HTMLElement;
  removedDelete: HTMLButtonElement;
  removedRestoreSelection: HTMLButtonElement;
  removedUndo: HTMLButtonElement;
  trashOutcome: HTMLElement;
  trashOutcomeTitle: HTMLElement;
  trashOutcomeCounts: HTMLElement;
  trashOutcomeDeleted: HTMLElement;
  trashOutcomeChanged: HTMLElement;
  trashOutcomeMissing: HTMLElement;
  trashOutcomeFailed: HTMLElement;
  trashOutcomePending: HTMLElement;
  trashOutcomeBytes: HTMLElement;
  trashOutcomeItems: HTMLElement;
  trashOutcomeActions: HTMLElement;
  trashOutcomeCheck: HTMLButtonElement;
  trashOutcomeRetry: HTMLButtonElement;
  trashReviewDialog: HTMLDialogElement;
  trashReviewSummary: HTMLElement;
  trashReviewAlbums: HTMLElement;
  trashReviewItems: HTMLElement;
  trashReviewRejected: HTMLElement;
  trashReviewRejectedHeading: HTMLElement;
  trashReviewRejectedList: HTMLElement;
  trashReviewConfirm: HTMLButtonElement;
  trashReviewCancel: HTMLButtonElement;
  trashReviewClose: HTMLButtonElement;
}>;

/// The controller's explicit dependencies: the view's element lookups, the
/// page's intent sink, the shared modal-surface controller, and the one
/// Thumbnail bind/release path the page already owns.
export type RemovedPanelsOptions = Readonly<{
  elements: RemovedPanelsElements;
  send: (intent: RemovedPanelIntent) => void;
  surfaces: ModalSurfaces;
  bindThumbnail: (binding: GridThumbnailBinding) => void;
  releaseThumbnail: (binding: GridThumbnailBinding) => void;
  gridThumbnailTarget: (
    image: HTMLImageElement,
    setDeliveryFailed: (failed: boolean) => void,
  ) => GridThumbnailTarget;
}>;

export interface RemovedPanels {
  /// Presents whether the Grid's toolbar offers the removal review of the
  /// current `Rejected` result. The review is offered only where it can be
  /// admitted.
  setRemovalEnabled(enabled: boolean): void;
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
  /// Updates the open review without reopening it, so a settling confirmation
  /// reaches the surface the Photographer is looking at.
  renderTrashReview(model: TrashReviewViewModel): void;
  closeTrashReview(): void;
  /// Stops presenting and releases the listing's Thumbnails, for the page
  /// teardown every surface shares.
  dispose(): void;
}

export function createRemovedPanels({
  elements,
  send,
  surfaces,
  bindThumbnail,
  releaseThumbnail,
  gridThumbnailTarget,
}: RemovedPanelsOptions): RemovedPanels {
  const {
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
  } = elements;
  let alive = true;
  /// Thumbnails the Removed Photos listing attached. They are released when
  /// the listing re-renders or closes, so the page's image delivery holds only
  /// the rows that are actually presented.
  let removedListBindings: ReadonlyArray<GridThumbnailBinding> = [];
  const releaseRemovedRows = () => {
    for (const binding of removedListBindings) releaseThumbnail(binding);
    removedListBindings = [];
    removedList.replaceChildren();
  };

  surfaces.register("removal-review", {
    dialog: removalDialog,
    modal: () => true,
  });
  surfaces.register("removed-panel", {
    dialog: removedPanel,
    modal: () => true,
  });
  surfaces.register("trash-review", {
    dialog: trashReviewDialog,
    modal: () => true,
  });

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
  removedSelectAll.addEventListener("click", () =>
    send({ kind: "removed-select-all" }),
  );
  removedClearSelection.addEventListener("click", () =>
    send({ kind: "removed-clear-selection" }),
  );
  removedRestoreSelection.addEventListener("click", () =>
    send({ kind: "removed-restore-selected" }),
  );
  removedDelete.addEventListener("click", () =>
    send({ kind: "removed-delete" }),
  );
  removedPrevious.addEventListener("click", () =>
    send({ kind: "removed-page", direction: -1 }),
  );
  removedNext.addEventListener("click", () =>
    send({ kind: "removed-page", direction: 1 }),
  );
  removedRetry.addEventListener("click", () => send({ kind: "removed-retry" }));
  trashReviewConfirm.addEventListener("click", () =>
    send({ kind: "trash-review-confirm" }),
  );
  trashReviewCancel.addEventListener("click", () =>
    send({ kind: "trash-review-cancel" }),
  );
  trashReviewClose.addEventListener("click", () =>
    send({ kind: "trash-review-cancel" }),
  );
  trashOutcomeCheck.addEventListener("click", () =>
    send({ kind: "trash-check-result" }),
  );
  trashOutcomeRetry.addEventListener("click", () =>
    send({ kind: "trash-retry-delete" }),
  );
  // A close the controller did not start — a native close request, an Escape
  // from a surface this one yielded to, a destination change — releases the
  // listing's thumbnails the same way an explicit Close does.
  removedPanel.addEventListener("close", releaseRemovedRows);

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
  /// The outcome label one non-deleted operation item presents. The states
  /// whose outcome is not settled yet all present as pending verification.
  const TRASH_OUTCOME_STATE_LABELS: Record<
    "pending" | "deleting" | "missing" | "changed" | "failed" | "uncertain",
    string
  > = {
    pending: "Pending verification",
    deleting: "Pending verification",
    uncertain: "Pending verification",
    changed: "Changed",
    missing: "Missing",
    failed: "Failed",
  };
  const presentTrashOutcome = (model: RemovedPanelViewModel) => {
    const outcome = model.outcome;
    const busy =
      model.outcomeBusy !== undefined || model.pending || model.deleting;
    trashOutcomeCheck.disabled = busy;
    trashOutcomeRetry.disabled = busy;
    if (outcome === undefined) {
      trashOutcome.hidden = true;
      trashOutcomeTitle.textContent = "";
      trashOutcomeCounts.hidden = true;
      trashOutcomeBytes.hidden = true;
      trashOutcomeItems.replaceChildren();
      trashOutcomeActions.hidden = true;
      return;
    }
    trashOutcome.hidden = false;
    trashOutcomeTitle.textContent = outcome.unconfirmed
      ? "The deletion result could not be confirmed."
      : "Deletion outcome";
    // Recovery stays offered while the response is lost or any item is still
    // pending verification: retry reconciles prior effects before it repeats
    // the unresolved items.
    trashOutcomeActions.hidden = !(
      outcome.unconfirmed || outcome.pendingVerification > 0
    );
    if (outcome.unconfirmed) {
      trashOutcomeCounts.hidden = true;
      trashOutcomeBytes.hidden = true;
      trashOutcomeItems.replaceChildren();
      return;
    }
    trashOutcomeCounts.hidden = false;
    trashOutcomeDeleted.textContent = `Deleted ${outcome.deleted.toLocaleString()}`;
    trashOutcomeChanged.textContent = `Changed ${outcome.changed.toLocaleString()}`;
    trashOutcomeMissing.textContent = `Missing ${outcome.missing.toLocaleString()}`;
    trashOutcomeFailed.textContent = `Failed ${outcome.failed.toLocaleString()}`;
    trashOutcomePending.textContent = `Pending verification ${outcome.pendingVerification.toLocaleString()}`;
    trashOutcomeBytes.hidden = false;
    trashOutcomeBytes.textContent = `Logical bytes deleted: ${outcome.logicalBytesDeleted.toLocaleString()}`;
    trashOutcomeItems.replaceChildren(
      ...outcome.items.map((item) => {
        const row = document.createElement("li");
        row.className = "trash-outcome-item";
        row.dataset.trashOutcomeItem = "";
        row.textContent = `${TRASH_OUTCOME_STATE_LABELS[item.state]} · ${item.originalKind.toUpperCase()} · ${item.originalLocation} · ${
          item.size === null
            ? "Size unavailable"
            : `${item.size.toLocaleString()} bytes`
        }${item.message ? ` · ${item.message}` : ""}`;
        return row;
      }),
    );
  };
  const presentTrashReview = (model: TrashReviewViewModel) => {
    trashReviewSummary.textContent =
      model.itemCount === 0
        ? "None of the selected Trash items can be deleted."
        : `${formatPhotoCount(model.itemCount)} selected for permanent deletion · ${model.totalBytes.toLocaleString()} logical bytes.`;
    trashReviewAlbums.textContent =
      model.albumCount === 0
        ? "No Album is affected."
        : `${model.albumCount.toLocaleString()} ${
            model.albumCount === 1 ? "Album" : "Albums"
          } affected: ${model.albumNames.join(", ")}.`;
    trashReviewItems.replaceChildren(
      ...model.items.map((item) => {
        const row = document.createElement("li");
        row.className = "trash-review-item";
        row.dataset.trashReviewItem = "";
        const location = document.createElement("span");
        location.className = "trash-review-location";
        location.dataset.trashReviewLocation = "";
        location.textContent = item.originalLocation;
        const kind = document.createElement("span");
        kind.className = "trash-review-kind";
        kind.dataset.trashReviewKind = "";
        kind.textContent = item.originalKind.toUpperCase();
        const size = document.createElement("span");
        size.className = "trash-review-size";
        size.dataset.trashReviewSize = "";
        size.textContent = `${item.size.toLocaleString()} bytes`;
        const albums = document.createElement("span");
        albums.className = "trash-review-albums";
        albums.dataset.trashReviewItemAlbums = "";
        albums.textContent =
          item.albumNames.length === 0
            ? "No Albums"
            : item.albumNames.join(", ");
        row.append(location, kind, size, albums);
        return row;
      }),
    );
    trashReviewRejected.hidden = model.rejected.length === 0;
    trashReviewRejectedHeading.textContent = `${model.rejected.length.toLocaleString()} ${
      model.rejected.length === 1 ? "item" : "items"
    } could not be reviewed:`;
    trashReviewRejectedList.replaceChildren(
      ...model.rejected.map((item) => {
        const row = document.createElement("li");
        row.className = "trash-review-rejected-item";
        row.dataset.trashReviewRejectedItem = "";
        const photo = document.createElement("span");
        photo.dataset.trashReviewRejectedPhoto = "";
        photo.textContent = item.photoId;
        const reason = document.createElement("span");
        reason.dataset.trashReviewRejectedReason = "";
        reason.textContent = item.reasonLabel;
        row.append(photo, reason);
        return row;
      }),
    );
    trashReviewConfirm.hidden = !model.canConfirm;
    trashReviewConfirm.disabled = model.deleting;
    trashReviewConfirm.textContent = model.deleting
      ? "Permanently deleting…"
      : model.confirmLabel;
    trashReviewCancel.disabled = model.deleting;
  };
  const presentRemovedPanel = (model: RemovedPanelViewModel) => {
    releaseRemovedRows();
    const shown = Math.min(model.total, model.start + model.items.length);
    removedStatus.textContent =
      model.total === 0
        ? "Trash is empty."
        : `${formatPhotoCount(model.total)} in Trash. Showing ${(
            model.start + 1
          ).toLocaleString()}–${shown.toLocaleString()}.`;
    const rows = model.items.map((item) => {
      const row = document.createElement("li");
      row.className = "removed-item";
      const pending = item.pendingVerificationOperationId !== null;
      if (pending) row.dataset.trashPendingItem = "true";
      const retainedBusy =
        model.pending || model.deleting || model.outcomeBusy !== undefined;
      const select = document.createElement("input");
      select.type = "checkbox";
      select.checked = item.selected;
      select.ariaLabel = `Select ${item.filename} for permanent deletion`;
      select.disabled = model.pending || model.deleting || pending;
      select.addEventListener("change", () =>
        send({
          kind: "removed-toggle",
          photoId: item.photoId,
          selected: select.checked,
        }),
      );
      const image = document.createElement("img");
      image.className = "removed-thumb";
      image.alt = "";
      const facts = document.createElement("div");
      facts.className = "removed-facts";
      const name = document.createElement("p");
      name.className = "removed-name";
      name.textContent = item.filename;
      const details = document.createElement("p");
      details.className = "removed-when";
      details.textContent = `${item.originalKind.toUpperCase()} · ${item.originalLocation} · ${
        item.originalSize === null
          ? "Size unavailable"
          : `${item.originalSize.toLocaleString()} bytes`
      }`;
      const when = document.createElement("p");
      when.className = "removed-when";
      when.textContent = `Removed ${removalTimestamp(item.removedAtMs)}`;
      facts.append(name, details, when);
      const restore = document.createElement("button");
      restore.type = "button";
      restore.className = "quiet";
      const restoring = model.restoringPhotoId === item.photoId;
      restore.textContent = restoring ? "Restoring…" : "Restore";
      restore.disabled =
        model.pending ||
        model.deleting ||
        model.restoringSelection ||
        restoring ||
        pending;
      restore.addEventListener("click", () =>
        send({
          kind: "removed-restore",
          photoId: item.photoId,
          removedAtMs: item.removedAtMs,
        }),
      );
      row.append(select, image, facts, restore);
      if (pending) {
        // The item's permanent-deletion outcome is not settled. Its row
        // names the pending state and offers the recovery that names the
        // retained operation id; Restore and selection stay unavailable.
        const note = document.createElement("div");
        note.className = "removed-pending";
        note.dataset.trashPending = "";
        const marker = document.createElement("p");
        marker.className = "removed-pending-marker";
        marker.textContent =
          "Pending verification — the deletion outcome is still being verified.";
        const check = document.createElement("button");
        check.type = "button";
        check.className = "quiet";
        check.dataset.trashRowCheck = "";
        check.textContent = "Check result";
        check.disabled = retainedBusy;
        check.addEventListener("click", () =>
          send({ kind: "trash-row-check", photoId: item.photoId }),
        );
        const resume = document.createElement("button");
        resume.type = "button";
        resume.className = "quiet";
        resume.dataset.trashRowResume = "";
        resume.textContent = "Resume";
        resume.disabled = retainedBusy;
        resume.addEventListener("click", () =>
          send({ kind: "trash-row-resume", photoId: item.photoId }),
        );
        note.append(marker, check, resume);
        row.append(note);
      }
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
    removedSelectionCount.textContent =
      model.selectionCount === 0
        ? "Nothing selected."
        : `${model.selectionCount.toLocaleString()} selected.`;
    removedSelectAll.disabled =
      model.pending || model.deleting || model.total === 0;
    removedClearSelection.disabled =
      model.pending || model.deleting || model.selectionCount === 0;
    removedRestoreSelection.disabled =
      model.pending ||
      model.deleting ||
      model.restoringSelection ||
      !model.canRestore;
    removedRestoreSelection.textContent = model.restoringSelection
      ? "Restoring selected…"
      : "Restore selected";
    removedDelete.disabled =
      model.pending ||
      model.deleting ||
      model.restoringSelection ||
      !model.canDelete;
    removedDelete.textContent = model.deleting
      ? "Permanently deleting…"
      : "Permanently delete selected Originals";
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
    presentTrashOutcome(model);
    if (model.message === undefined) {
      removedMessage.hidden = true;
      removedMessage.textContent = "";
      return;
    }
    removedMessage.textContent = model.message;
    removedMessage.hidden = false;
  };

  return {
    setRemovalEnabled(enabled) {
      if (!alive) return;
      removalOpen.hidden = !enabled;
      removalOpen.disabled = !enabled;
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
    openTrashReview(model) {
      if (!alive) return;
      presentTrashReview(model);
      surfaces.open("trash-review", removedDelete);
      trashReviewCancel.focus();
    },
    renderTrashReview(model) {
      if (!alive) return;
      presentTrashReview(model);
    },
    closeTrashReview() {
      if (!alive) return;
      surfaces.close("trash-review");
    },
    dispose() {
      if (!alive) return;
      alive = false;
      releaseRemovedRows();
    },
  };
}

/// When a Photo was removed, in the Photographer's own locale. A timestamp the
/// platform cannot parse is presented verbatim rather than invented.
/// The removal marker as a local reading. The listing reports the millisecond
/// the removal was confirmed, so the row shows the same instant the restore
/// names and no timezone-less text is parsed as if it were local.
function removalTimestamp(removedAtMs: number): string {
  return new Date(removedAtMs).toLocaleString();
}
