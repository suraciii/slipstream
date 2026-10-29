/// The Library browser's removal review, Trash listing, and record-cleanup review.
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
  GridThumbnailBinding,
  GridThumbnailTarget,
} from "./grid-cell-presenter.js";
import type { LibraryBrowserIntent } from "./library-browser-view.js";
import type {
  RemovalReviewViewModel,
  RemovedPanelViewModel,
  TrashReviewViewModel,
} from "../model/removed-listing-contract.js";

/// Intents emitted by the removal, Trash listing, and record-cleanup surfaces.
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
  | "removed-cleanup"
  | "removed-copy-locations"
  | "removed-restore-selected"
  | "removed-restore"
  | "trash-review-confirm"
  | "trash-review-cancel"
  | "trash-check-result";

export type RemovedPanelIntent = Extract<
  LibraryBrowserIntent,
  { kind: RemovedPanelIntentKind }
>;

// Presentation contracts are owned by the model; this UI controller only
// renders them and emits the page intent union.

/// The Removed Photos surfaces' controls, looked up by the view that owns the
/// markup: the removal review, the listing, the Trash outcome, and cleanup review.
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
  trashLibraryFolder: HTMLElement;
  trashCopyLocations: HTMLButtonElement;
  removedDelete: HTMLButtonElement;
  removedRestoreSelection: HTMLButtonElement;
  removedUndo: HTMLButtonElement;
  trashOutcome: HTMLElement;
  trashOutcomeTitle: HTMLElement;
  trashOutcomeCounts: HTMLElement;
  trashOutcomeItems: HTMLElement;
  trashOutcomeActions: HTMLElement;
  trashOutcomeCheck: HTMLButtonElement;
  trashReviewDialog: HTMLDialogElement;
  trashReviewSummary: HTMLElement;
  trashReviewAlbums: HTMLElement;
  trashReviewItems: HTMLElement;
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
    setThumbnailState?: (state: "unavailable" | "failed") => void,
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
  /// Opens the record-cleanup review above the Trash listing.
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
    trashLibraryFolder,
    trashCopyLocations,
    removedDelete,
    removedRestoreSelection,
    removedUndo,
    trashOutcome,
    trashOutcomeTitle,
    trashOutcomeCounts,
    trashOutcomeItems,
    trashOutcomeActions,
    trashOutcomeCheck,
    trashReviewDialog,
    trashReviewSummary,
    trashReviewAlbums,
    trashReviewItems,
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
    send({ kind: "removed-cleanup" }),
  );
  trashCopyLocations.addEventListener("click", () =>
    send({ kind: "removed-copy-locations" }),
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
  // A close the controller did not start — a native close request, an Escape
  // from a surface this one yielded to, a destination change — releases the
  // listing's thumbnails the same way an explicit Close does.
  removedPanel.addEventListener("close", () => {
    if (removedPanel.open) return;
    releaseRemovedRows();
    if (!trashReviewDialog.open) send({ kind: "removed-list-close" });
  });
  trashReviewDialog.addEventListener("close", () => {
    if (!trashReviewDialog.open) send({ kind: "trash-review-cancel" });
  });

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
  const presentTrashOutcome = (model: RemovedPanelViewModel) => {
    const outcome = model.outcome;
    const wasHidden = trashOutcome.hidden;
    trashOutcomeCheck.disabled =
      model.outcomeBusy !== undefined || model.pending || model.cleaning;
    trashOutcome.hidden = outcome === undefined;
    if (!outcome) return;
    trashOutcomeTitle.textContent = outcome.unconfirmed
      ? "The cleanup result could not be confirmed."
      : "Cleanup outcome";
    trashOutcomeActions.hidden = !outcome.unconfirmed;
    trashOutcomeCounts.hidden = outcome.unconfirmed === true;
    if (outcome.unconfirmed) {
      trashOutcomeItems.replaceChildren();
      if (wasHidden) trashOutcome.scrollIntoView({ block: "nearest" });
      return;
    }
    const counts = outcome.counts;
    trashOutcomeCounts.textContent = `Cleaned ${counts.cleaned.toLocaleString()} · Changed ${counts.changed.toLocaleString()} · Present ${counts.present.toLocaleString()} · Blocked ${counts.blocked.toLocaleString()} · Unavailable ${counts.unavailable.toLocaleString()} · Inspection failed ${counts.inspectionFailed.toLocaleString()}`;
    trashOutcomeItems.replaceChildren(
      ...outcome.items.map((item) => {
        const row = document.createElement("li");
        row.className = "trash-outcome-item";
        row.dataset.trashOutcomeItem = "";
        const labels = {
          cleaned: "Cleaned",
          changed: "Changed",
          present: "Original present",
          blocked: "Blocked",
          unavailable: "Record unavailable",
          "inspection-failed": "Inspection failed",
        };
        row.textContent = `${labels[item.state]} · ${item.photoId} · ${item.originalKind.toUpperCase()} · ${item.originalLocation}${item.message ? ` · ${item.message}` : ""}${item.blockers.map((blocker) => ` · ${blocker.kind}: ${blocker.name} — ${blocker.action}`).join("")}`;
        return row;
      }),
    );
    if (wasHidden) trashOutcome.scrollIntoView({ block: "nearest" });
  };
  const presentTrashReview = (model: TrashReviewViewModel) => {
    trashReviewSummary.textContent = `${formatPhotoCount(model.itemCount)} selected for record cleanup.`;
    if (!model.canConfirm && !model.cleaning) {
      trashReviewSummary.textContent +=
        " Confirmation requires readable Album membership for every selected Photo.";
    }
    trashReviewAlbums.textContent = model.albumNames.length
      ? `Affected Albums: ${model.albumNames.join(", ")}.`
      : "Album membership is shown for each Photo below.";
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
        kind.textContent = `${item.originalKind.toUpperCase()} · ${item.photoId}`;
        const size = document.createElement("span");
        size.className = "trash-review-size";
        size.dataset.trashReviewSize = "";
        size.textContent =
          item.originalSize === null
            ? "Size unavailable"
            : `${item.originalSize.toLocaleString()} bytes`;
        const evidence = document.createElement("span");
        const blockerText = item.blockers
          .map((blocker) => `${blocker.kind}: ${blocker.name}`)
          .join(", ");
        evidence.textContent = [
          item.previewState === "changed"
            ? "Changed since capture; cleanup will be refused."
            : item.previewState === "unavailable"
              ? "Photo record unavailable; cleanup will be refused."
              : item.absence === "absent"
                ? item.blockers.length
                  ? "Original absent, but retained work blocks cleanup."
                  : "Original absent; eligible for cleanup."
                : item.absence === "present"
                  ? "Original present; cleanup will be refused."
                  : item.absence === "uninspectable"
                    ? "Original could not be inspected; cleanup will be refused."
                    : "Absence evidence unavailable.",
          item.message ?? "",
          blockerText ? `Blockers: ${blockerText}` : "",
        ]
          .filter(Boolean)
          .join(" ");
        const albums = document.createElement("span");
        albums.className = "trash-review-albums";
        albums.dataset.trashReviewItemAlbums = "";
        albums.textContent =
          item.albumNames === null
            ? "Album membership unavailable"
            : item.albumNames.length
              ? item.albumNames.join(", ")
              : "No Albums";
        row.append(location, kind, size, evidence, albums);
        return row;
      }),
    );
    trashReviewConfirm.hidden = !model.canConfirm;
    trashReviewConfirm.disabled = model.cleaning;
    trashReviewConfirm.textContent = model.cleaning
      ? "Cleaning up records…"
      : model.confirmLabel;
    trashReviewCancel.disabled = model.cleaning;
    if (
      model.cleaning &&
      (document.activeElement === trashReviewConfirm ||
        document.activeElement === document.body)
    ) {
      trashReviewClose.focus();
    }
  };
  const presentRemovedPanel = (model: RemovedPanelViewModel) => {
    releaseRemovedRows();
    const ready = model.status === "ready";
    const shown = Math.min(model.total, model.start + model.items.length);
    removedStatus.textContent =
      model.status === "loading"
        ? "Loading Trash…"
        : model.status === "failed"
          ? "Trash could not be loaded."
          : model.total === 0
            ? "Trash is empty."
            : model.items.length === 0
              ? `${formatPhotoCount(model.total)} in Trash.`
              : `${formatPhotoCount(model.total)} in Trash. Showing ${(
                  model.start + 1
                ).toLocaleString()}–${shown.toLocaleString()}.`;
    removedList.setAttribute("aria-busy", String(model.status === "loading"));
    const singleRestore = model.restoringPhotoId !== undefined;
    const rows = (ready ? model.items : []).map((item) => {
      const row = document.createElement("li");
      row.className = "removed-item";
      if (item.excluded) row.dataset.trashExcludedItem = "true";
      const select = document.createElement("input");
      select.type = "checkbox";
      select.checked = item.selected;
      select.ariaLabel = `Select ${item.filename}`;
      select.disabled =
        model.pending ||
        model.cleaning ||
        model.restoringSelection ||
        singleRestore;
      select.addEventListener("change", () =>
        send({
          kind: "removed-toggle",
          photoId: item.photoId,
          selected: select.checked,
        }),
      );
      const thumbnail = document.createElement("div");
      thumbnail.className = "removed-thumbnail";
      const image = document.createElement("img");
      image.className = "removed-thumb";
      image.alt = "";
      image.hidden = true;
      const placeholder = document.createElement("span");
      placeholder.className = "removed-thumbnail-placeholder";
      placeholder.dataset.removedThumbnailPlaceholder = "";
      placeholder.textContent =
        item.preview.state === "unavailable"
          ? "Thumbnail unavailable"
          : item.preview.state === "failed"
            ? "Thumbnail failed"
            : "Thumbnail loading";
      thumbnail.append(image, placeholder);
      const facts = document.createElement("div");
      facts.className = "removed-facts";
      const name = document.createElement("p");
      name.className = "removed-name";
      name.textContent = item.filename;
      const location = document.createElement("p");
      location.className = "removed-location";
      location.textContent = item.originalLocation;
      const when = document.createElement("p");
      when.className = "removed-when";
      when.textContent = `Removed ${removalTimestamp(item.removedAtMs)}`;
      const availability = document.createElement("p");
      availability.className = "removed-when";
      availability.textContent = `${item.originalKind.toUpperCase()} · Last known Original: ${item.originalAvailable ? "available" : "unavailable"}${item.state === "changed" ? " · Changed since capture; inspect again before Restore or cleanup." : item.state === "unavailable" ? " · Photo record unavailable since capture." : ""}`;
      const details = document.createElement("details");
      details.className = "removed-details";
      const detailsLabel = document.createElement("summary");
      detailsLabel.textContent = "Details";
      const technicalFacts = document.createElement("p");
      technicalFacts.textContent = `Photo ID: ${item.photoId} · Original ID: ${item.originalId} · ${item.originalSize === null ? "Size unavailable" : `${item.originalSize.toLocaleString()} bytes`} · Removal marker: ${item.removedAtMs}`;
      details.append(detailsLabel, technicalFacts);
      facts.append(name, when, location, availability, details);
      const restore = document.createElement("button");
      restore.type = "button";
      restore.className = "removed-restore";
      const restoring = model.restoringPhotoId === item.photoId;
      restore.textContent = restoring ? "Restoring…" : "Restore";
      restore.disabled =
        model.pending ||
        model.cleaning ||
        model.restoringSelection ||
        singleRestore ||
        restoring ||
        item.state !== "captured";
      restore.addEventListener("click", () =>
        send({
          kind: "removed-restore",
          photoId: item.photoId,
          removedAtMs: item.removedAtMs,
        }),
      );
      row.append(select, thumbnail, facts, restore);
      if (item.excluded) {
        // Excluded records stay visible without joining the captured selection.
        const excluded = document.createElement("p");
        excluded.className = "removed-excluded";
        excluded.dataset.trashExcluded = "";
        excluded.textContent =
          "Excluded — this item is not part of the selection and will not be cleaned up.";
        row.append(excluded);
      }
      const binding: GridThumbnailBinding = {
        photoId: item.photoId,
        preview: item.preview,
        target: gridThumbnailTarget(
          image,
          (failed) => {
            placeholder.hidden = !failed && !image.hidden;
            if (failed) placeholder.textContent = "Thumbnail failed";
          },
          (state) => {
            placeholder.hidden = false;
            placeholder.textContent =
              state === "unavailable"
                ? "Thumbnail unavailable"
                : "Thumbnail failed";
          },
        ),
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
    removedSelectionCount.textContent = !ready
      ? ""
      : model.allSelected
        ? `${model.selectionCount.toLocaleString()} captured items selected, except exclusions.`
        : model.selectionCount === 0
          ? "Nothing selected."
          : `${model.selectionCount.toLocaleString()} selected.`;
    const hasRows = ready && model.total > 0;
    removedSelectAll.hidden = !hasRows;
    removedClearSelection.hidden =
      model.selectionCount === 0 && !model.allSelected;
    removedRestoreSelection.hidden = model.selectionCount === 0;
    removedDelete.hidden = model.selectionCount === 0;
    trashCopyLocations.hidden = !hasRows;
    removedSelectionCount.hidden = !hasRows && model.selectionCount === 0;
    removedSelectAll.disabled =
      !ready ||
      model.pending ||
      model.cleaning ||
      model.restoringSelection ||
      singleRestore ||
      model.total === 0;
    removedClearSelection.disabled =
      !ready ||
      model.pending ||
      model.cleaning ||
      model.restoringSelection ||
      singleRestore ||
      (model.selectionCount === 0 && !model.allSelected);
    removedRestoreSelection.disabled =
      !ready ||
      model.pending ||
      model.cleaning ||
      model.restoringSelection ||
      singleRestore ||
      !model.canRestore;
    removedRestoreSelection.textContent = model.restoringSelection
      ? "Restoring selected…"
      : "Restore selected";
    removedDelete.disabled =
      !ready ||
      model.pending ||
      model.cleaning ||
      model.restoringSelection ||
      singleRestore ||
      !model.canCleanup;
    removedDelete.textContent = "Clean up records";
    trashCopyLocations.disabled = !ready || !model.canCopy;
    trashLibraryFolder.textContent = hasRows
      ? `Library Folder: ${model.libraryRoot} · Server/container filesystem namespace, not this device. Map this location before using filesystem tools. Captured ${model.evaluatedAt}${model.expiresAt ? ` · Capture continuation expires ${model.expiresAt}` : ""}`
      : "";
    removedPrevious.hidden = !ready || model.total === 0;
    removedNext.hidden = !ready || model.total === 0;
    removedPrevious.disabled =
      !ready || model.pending || singleRestore || !model.hasPrevious;
    removedNext.disabled =
      !ready || model.pending || singleRestore || !model.hasNext;
    removedRetry.hidden = model.status !== "failed" || !model.canRetry;
    removedRetry.disabled = model.pending;
    removedUndo.hidden =
      !ready || model.undo === undefined || model.undo.removed === 0;
    removedUndo.disabled =
      !ready ||
      model.pending ||
      model.cleaning ||
      model.restoringSelection ||
      singleRestore;
    removedUndo.textContent =
      model.undo === undefined
        ? "Undo the last removal"
        : `Undo the last removal (${model.undo.removed.toLocaleString()})`;
    removedPage.textContent =
      !ready || model.total === 0
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
      if (!alive || !removedPanel.open) return;
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
      if (!alive || !trashReviewDialog.open) return;
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

export const REMOVED_PANELS_TEMPLATE = `
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
            <p class="removed-status" data-removed-status role="status"></p><p class="removed-status" data-trash-library-folder></p>
            <div class="removed-selection" data-removed-selection>
              <button type="button" class="quiet" data-removed-select-all>Select all Trash</button>
              <button type="button" class="quiet" data-removed-clear-selection>Clear selection</button>
              <span data-removed-selection-count></span>
              <button type="button" class="removed-restore-selection" data-removed-restore-selection>Restore selected</button>
              <button type="button" class="quiet" data-trash-copy-locations>Copy file locations</button><button type="button" class="quiet removed-delete-review" data-removed-delete>Clean up records</button>
            </div>
            <div class="removed-scroll">
            <ul class="removed-list" data-removed-list></ul>
            <section class="trash-outcome" data-trash-outcome hidden>
              <p class="trash-outcome-title" data-trash-outcome-title role="status"></p>
              <p class="trash-outcome-counts" data-trash-outcome-counts hidden></p>
              <ul class="trash-outcome-items" data-trash-outcome-items></ul>
              <div class="trash-outcome-actions" data-trash-outcome-actions hidden><button type="button" class="quiet" data-trash-check-result>Check result</button></div>
            </section>
            </div>
            <p class="removed-message" data-removed-message role="alert" hidden></p>
            <footer class="removed-pager"><button type="button" class="quiet" data-removed-previous>Previous</button><span data-removed-page></span><button type="button" class="quiet" data-removed-next>Next</button><button type="button" data-removed-retry hidden>Retry</button></footer><footer class="removed-actions"><button type="button" class="quiet" data-removed-undo hidden>Undo the last removal</button></footer>
          </div>
        </dialog>
        <dialog class="removal-dialog trash-review-dialog" data-trash-review aria-labelledby="trash-review-title">
          <div class="removal-sheet">
            <header class="removal-header"><h3 id="trash-review-title">Clean up Trash records</h3><button type="button" class="quiet" data-trash-review-close>Close</button></header>
            <div class="removal-body">
              <p class="removal-summary" data-trash-review-summary></p>
              <p class="removal-message trash-review-warning">Cleanup discards Photo records, Library decisions, and Album memberships. Slipstream cannot undo it. No files are deleted. Absence at the remembered Location does not prove that all copies were deleted or that space was reclaimed.</p>
              <p class="removal-summary" data-trash-review-albums></p>
              <ul class="trash-review-items" data-trash-review-items></ul>
            </div>
            <footer class="removal-actions"><button type="button" class="trash-confirm-danger" data-trash-confirm hidden></button><button type="button" class="quiet" data-trash-cancel>Cancel</button></footer>
          </div>
        </dialog>
`;
