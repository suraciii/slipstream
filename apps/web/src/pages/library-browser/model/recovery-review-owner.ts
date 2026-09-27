import {
  applyRecoveryMappings,
  continueRelocationProposals,
  continueUnavailableReview,
  openUnavailableReview,
  proposeRelocationBatch,
  proposeSingleRelocation,
  RECOVERY_PAGE_LIMIT,
  type RecoveryApplyMapping,
  type RecoveryItem,
  type RecoveryMapping,
} from "../api/recovery.js";
import type { PhotoFetch } from "../api/photo.js";
import type {
  RecoveryEntryViewModel,
  RecoveryMappingViewModel,
  RecoveryPagingViewModel,
} from "../ui/library-browser-view.js";

export type {
  RecoveryApplyMapping,
  RecoveryItem,
  RecoveryMapping,
} from "../api/recovery.js";

/// The recovery-panel surface the review owner drives: exactly the view
/// controls the reviewed recovery protocol presents through.
export interface RecoveryReviewSurface {
  setRecoveryNotice(
    model: Readonly<{
      relocatedPhotos: number;
      unavailablePhotos: number;
    }>,
  ): void;
  openRecoveryPanel(
    entries: ReadonlyArray<RecoveryEntryViewModel>,
    paging: RecoveryPagingViewModel,
  ): void;
  renderRecoveryEntries(
    entries: ReadonlyArray<RecoveryEntryViewModel>,
    paging: RecoveryPagingViewModel,
  ): void;
  renderRecoveryProposals(
    mappings: ReadonlyArray<RecoveryMappingViewModel>,
    paging: RecoveryPagingViewModel,
  ): void;
  clearRecoveryProposals(): void;
  resetRecoveryProposalChoices(): void;
  setRecoveryPending(pending: boolean): void;
  setRecoveryMessage(text?: string): void;
  closeRecoveryPanel(): void;
}

/// The page ports the review owner reads: the mounted-page check every
/// settled answer re-reads, the Grid status line a failed open names its
/// failure on, and the source reload a committed apply triggers.
export type RecoveryReviewPage = Readonly<{
  isAlive(): boolean;
  setGridStatusText(text: string): void;
  refreshSource(): Promise<void>;
}>;

export interface RecoveryReviewOwner {
  presentRecoveryNotice(
    recovery:
      | Readonly<{
          relocatedPhotos: number;
          fingerprintedOriginals: number;
          unavailablePhotos: number;
        }>
      | undefined,
  ): void;
  openRecoveryReview(): Promise<void>;
  loadMoreRecoveryEntries(): Promise<void>;
  loadMoreRecoveryMappings(): Promise<void>;
  proposeRecoveryBatch(
    oldPrefix: string | undefined,
    newPrefix: string | undefined,
  ): Promise<void>;
  proposeRecoverySingle(originalId: string, newLocation: string): Promise<void>;
  applyRecovery(mappings: ReadonlyArray<RecoveryApplyMapping>): Promise<void>;
}

export function createRecoveryReviewOwner(
  fetcher: PhotoFetch,
  view: RecoveryReviewSurface,
  page: RecoveryReviewPage,
): RecoveryReviewOwner {
  let recoveryNoticeShown: string | undefined;
  const presentRecoveryNotice = (
    recovery:
      | Readonly<{
          relocatedPhotos: number;
          fingerprintedOriginals: number;
          unavailablePhotos: number;
        }>
      | undefined,
  ): void => {
    const relocated = recovery?.relocatedPhotos ?? 0;
    const unavailable = recovery?.unavailablePhotos ?? 0;
    if (relocated === 0 && unavailable === 0) {
      recoveryNoticeShown = "none";
      view.setRecoveryNotice({ relocatedPhotos: 0, unavailablePhotos: 0 });
      return;
    }
    const signature = `${relocated}:${unavailable}`;
    if (signature === recoveryNoticeShown) return;
    recoveryNoticeShown = signature;
    view.setRecoveryNotice({
      relocatedPhotos: relocated,
      unavailablePhotos: unavailable,
    });
  };
  /// The reviewed recovery protocol: one bounded page per request, with the
  /// continuation cursor naming the review it continues. Neither list is
  /// ever presented as complete while a cursor remains.
  const mapRecoveryEntry = (entry: RecoveryItem) => ({
    state: entry.state,
    originalId: entry.originalId,
    location: entry.location,
    kind: entry.kind,
    rating: entry.rating,
    selectionState: entry.selectionState,
    albumCount: entry.albumCount,
    fingerprintEnrolled: entry.fingerprintEnrolled,
  });

  const mapRecoveryMapping = (mapping: RecoveryMapping) => ({
    mappingId: mapping.mappingId,
    originalId: mapping.originalId,
    fromLocation: mapping.fromLocation,
    toLocation: mapping.toLocation,
    kind: mapping.kind,
    outcome: mapping.outcome,
    verified: mapping.verified,
    blockedReason: mapping.blockedReason,
    retire: mapping.retire
      ? {
          photoId: mapping.retire.photoId,
          originalId: mapping.retire.originalId,
          location: mapping.retire.location,
        }
      : null,
  });

  let recoveryEntries: ReadonlyArray<RecoveryItem> = [];
  let recoveryEntriesTotal = 0;
  let recoveryEntriesCursor: string | null = null;
  let recoveryMappings: ReadonlyArray<RecoveryMapping> = [];
  let recoveryMappingsTotal = 0;
  let recoveryMappingsCursor: string | null = null;

  const presentRecoveryEntries = (): void => {
    view.renderRecoveryEntries(recoveryEntries.map(mapRecoveryEntry), {
      shown: recoveryEntries.length,
      total: recoveryEntriesTotal,
      more: recoveryEntriesCursor !== null,
    });
  };

  const presentRecoveryMappings = (): void => {
    view.renderRecoveryProposals(recoveryMappings.map(mapRecoveryMapping), {
      shown: recoveryMappings.length,
      total: recoveryMappingsTotal,
      more: recoveryMappingsCursor !== null,
    });
  };

  const openRecoveryReview = async (): Promise<void> => {
    view.setRecoveryPending(true);
    const result = await openUnavailableReview(
      fetcher,
      { limit: RECOVERY_PAGE_LIMIT },
      new AbortController().signal,
    );
    view.setRecoveryPending(false);
    if (!page.isAlive()) return;
    if (result.kind === "ok") {
      recoveryEntries = result.page.items;
      recoveryEntriesTotal = result.page.total;
      recoveryEntriesCursor = result.page.nextCursor;
      view.openRecoveryPanel(recoveryEntries.map(mapRecoveryEntry), {
        shown: recoveryEntries.length,
        total: recoveryEntriesTotal,
        more: recoveryEntriesCursor !== null,
      });
      view.setRecoveryMessage();
      return;
    }
    // The review never opened, so its dialog stays closed and nothing inside
    // it can present this failure. The Grid status line is the visible
    // surface the entry button sits beside; it names the failure without
    // claiming any review happened.
    page.setGridStatusText("Could not load unavailable originals. Retry.");
  };

  /// Appends the next page of the same review. An expired review cannot be
  /// continued: the loaded entries stay and the remainder is named, never
  /// silently dropped.
  const loadMoreRecoveryEntries = async (): Promise<void> => {
    const cursor = recoveryEntriesCursor;
    if (cursor === null) return;
    view.setRecoveryPending(true);
    const result = await continueUnavailableReview(
      fetcher,
      cursor,
      new AbortController().signal,
    );
    view.setRecoveryPending(false);
    if (!page.isAlive()) return;
    if (result.kind === "ok") {
      recoveryEntries = [...recoveryEntries, ...result.page.items];
      recoveryEntriesTotal = result.page.total;
      recoveryEntriesCursor = result.page.nextCursor;
      presentRecoveryEntries();
      view.setRecoveryMessage();
      return;
    }
    if (result.kind === "rejected" && result.status === 409) {
      recoveryEntriesCursor = null;
      presentRecoveryEntries();
      view.setRecoveryMessage(
        result.message ??
          "This review is no longer current. Close it and review the unavailable originals again.",
      );
      return;
    }
    view.setRecoveryMessage(
      "Could not load more unavailable originals. Retry.",
    );
  };

  const proposeRecoveryBatch = async (
    oldPrefix: string | undefined,
    newPrefix: string | undefined,
  ): Promise<void> => {
    // An empty prefix is the documented Library-root prefix, so only a
    // missing form value is refused; "" is proposed like any other prefix.
    if (oldPrefix === undefined || newPrefix === undefined) {
      view.setRecoveryMessage("Enter both folder prefixes.");
      return;
    }
    view.setRecoveryPending(true);
    const result = await proposeRelocationBatch(
      fetcher,
      oldPrefix,
      newPrefix,
      { limit: RECOVERY_PAGE_LIMIT },
      new AbortController().signal,
    );
    view.setRecoveryPending(false);
    if (!page.isAlive()) return;
    if (result.kind === "ok") {
      view.resetRecoveryProposalChoices();
      recoveryMappings = result.page.items;
      recoveryMappingsTotal = result.page.total;
      recoveryMappingsCursor = result.page.nextCursor;
      presentRecoveryMappings();
      view.setRecoveryMessage(
        recoveryMappings.length === 0
          ? "No unavailable originals under that folder prefix."
          : undefined,
      );
      return;
    }
    if (result.kind === "rejected" && result.status === 413) {
      view.setRecoveryMessage(
        result.message ??
          "That folder prefix covers more originals than one review admits. Narrow the prefix and propose again.",
      );
      return;
    }
    view.setRecoveryMessage(
      "Could not propose mappings. Check the folder prefixes and retry.",
    );
  };

  const proposeRecoverySingle = async (
    originalId: string,
    newLocation: string,
  ): Promise<void> => {
    if (!originalId || !newLocation) {
      view.setRecoveryMessage("Choose an Original and enter its new location.");
      return;
    }
    view.setRecoveryPending(true);
    const result = await proposeSingleRelocation(
      fetcher,
      originalId,
      newLocation,
      new AbortController().signal,
    );
    view.setRecoveryPending(false);
    if (!page.isAlive()) return;
    if (result.kind === "ok") {
      view.resetRecoveryProposalChoices();
      recoveryMappings = result.page.items;
      recoveryMappingsTotal = result.page.total;
      recoveryMappingsCursor = result.page.nextCursor;
      presentRecoveryMappings();
      view.setRecoveryMessage();
      return;
    }
    view.setRecoveryMessage(
      "Could not propose that mapping. Check the location and retry.",
    );
  };

  /// Appends the next page of the same proposal review. An expired review
  /// cannot be continued: the reviewed mappings stay and the remainder is
  /// named, never silently dropped.
  const loadMoreRecoveryMappings = async (): Promise<void> => {
    const cursor = recoveryMappingsCursor;
    if (cursor === null) return;
    view.setRecoveryPending(true);
    const result = await continueRelocationProposals(
      fetcher,
      cursor,
      new AbortController().signal,
    );
    view.setRecoveryPending(false);
    if (!page.isAlive()) return;
    if (result.kind === "ok") {
      recoveryMappings = [...recoveryMappings, ...result.page.items];
      recoveryMappingsTotal = result.page.total;
      recoveryMappingsCursor = result.page.nextCursor;
      presentRecoveryMappings();
      view.setRecoveryMessage();
      return;
    }
    if (result.kind === "rejected" && result.status === 409) {
      recoveryMappingsCursor = null;
      presentRecoveryMappings();
      view.setRecoveryMessage(
        result.message ??
          "This proposal review is no longer current. Propose the mappings again.",
      );
      return;
    }
    view.setRecoveryMessage("Could not load more mappings. Retry.");
  };

  /// Commits the reviewed mappings the panel chose. A refusal names every
  /// refused mapping and claims nothing as applied.
  const applyRecovery = async (
    mappings: ReadonlyArray<RecoveryApplyMapping>,
  ): Promise<void> => {
    if (mappings.length > 100) {
      view.setRecoveryMessage(
        "Select no more than 100 mappings per apply. Narrow the proposal and review again.",
      );
      return;
    }
    if (mappings.length === 0) return;
    view.setRecoveryPending(true);
    const result = await applyRecoveryMappings(
      fetcher,
      mappings,
      new AbortController().signal,
    );
    view.setRecoveryPending(false);
    if (!page.isAlive()) return;
    if (result.kind === "applied") {
      view.closeRecoveryPanel();
      // The committed counts are the freshest recovery truth until the next
      // scan reports its own.
      recoveryNoticeShown = `${result.appliedMappings}:${result.unavailablePhotos}`;
      view.setRecoveryNotice({
        relocatedPhotos: result.appliedMappings,
        unavailablePhotos: result.unavailablePhotos,
      });
      void page.refreshSource();
      return;
    }
    if (result.kind === "refused") {
      const reasons = result.rejections
        .map((rejection) => rejection.reason)
        .join(", ");
      view.clearRecoveryProposals();
      view.setRecoveryMessage(
        `${result.message}${reasons ? ` (${reasons})` : ""}`,
      );
      return;
    }
    if (result.kind === "unknown") {
      const ids = result.mappings
        .map((mapping) => mapping.originalId)
        .join(", ");
      view.clearRecoveryProposals();
      view.setRecoveryMessage(
        `Apply outcome is unknown. Inspect these Photo identities before any new proposal: ${ids}`,
      );
      return;
    }
    view.setRecoveryMessage("Could not apply the mappings. Retry.");
  };

  return {
    presentRecoveryNotice,
    openRecoveryReview,
    loadMoreRecoveryEntries,
    loadMoreRecoveryMappings,
    proposeRecoveryBatch,
    proposeRecoverySingle,
    applyRecovery,
  };
}
