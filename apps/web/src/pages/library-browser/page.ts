import {
  RecoveryGate,
  type RecoveryClaim,
  type RecoveryTransition,
} from "./model/async-ownership.js";
import type {
  AlbumSummary,
  FolderChild,
  SelectionFilter,
  SelectionState,
} from "./api/contracts.js";
import { fetchPhotoAlbums, fetchPhotoMetadata } from "./api/photo.js";
import {
  applyRelocations,
  fetchUnavailableOriginals,
  proposeRelocations,
  proposeSingleRelocation,
  type RecoveryApplyItem,
  type RecoveryProposal,
} from "./api/recovery.js";
import {
  createFileLocationOwner,
  type FileLocationAuthority,
  type FileLocationFailure,
  type FileLocationOutcome,
  type FileLocationWindow,
} from "./model/file-location-owner.js";
import {
  createApplicationOwner,
  type ApplicationCoordination,
  type ApplicationEvent,
  type ApplicationPresentation,
  type ApplicationRecovery,
  type ApplicationSummaryAction,
  type FileLocationPresentation,
} from "./model/application-owner.js";
import {
  createSourceGridOwner,
  type SourceAuthority,
  type SourceGridSource,
  type SourceWindowOperation,
} from "./model/source-grid-owner.js";
import { releaseBrowse, type SourceViewOrder } from "./api/source-grid.js";
import {
  fetchRemovedPhotos,
  type RemovedPhotoItem,
  type RemovalResult,
  type RestorationResult,
} from "./api/removal.js";
import { createRemovalOwner } from "./model/removal-owner.js";
import {
  createAlbumActionOwner,
  type AlbumActionAdmission,
  type AlbumActionContext,
  type AlbumFormAuthority,
} from "./model/album-action-owner.js";
import { createPhotoOwner, type PhotoAuthority } from "./model/photo-owner.js";
import { createSavedPositionOwner } from "./model/saved-position-owner.js";
import {
  allPhotosDestination,
  createNavigationOwner,
  gridDestination,
  sameSourceView,
  type NavigationDestination,
  type NavigationGridRestoration,
  type NavigationTraversal,
} from "./model/browser-navigation.js";
import {
  createLibraryBrowserView,
  type AlbumFormReference,
  type FolderViewModel,
  type LibraryBrowserIntent,
  type LibraryBrowserView,
  type SourceListViewModel,
} from "./ui/library-browser-view.js";
import { formatPhotoCount } from "./ui/photo-count.js";
import { mountAccessBoundary } from "./access-boundary.js";
import type { BrowserFetch } from "./model/access-session.js";

type GridRangeRetry = Readonly<{
  sourceAuthority: SourceAuthority;
  operationKind: "source" | "grid";
  anchorIndex: number;
  start: number;
  quiet: boolean;
  priority: "high" | "low";
  /// The exact failure presentation this range owns, so a later range status
  /// report never hides the Retry message it shows.
  message: string;
}>;
type BrowseRangeFailure = Readonly<{
  claim: RecoveryClaim;
  ownerScope: "source" | "photo";
  retry?: GridRangeRetry;
}>;
type AlbumRecoveryRecord = Readonly<{
  claim: RecoveryClaim;
  sourceAuthority: SourceAuthority;
}>;
/// How establishing a source destination reports back to its caller.
type SourceEstablishment =
  /// The destination is committed and its first window is admitted.
  | Readonly<{ kind: "established" }>
  /// A newer destination superseded this one, so nothing was committed.
  | Readonly<{ kind: "superseded" }>
  /// The request failed; the destination stays retryable.
  | Readonly<{ kind: "failed" }>
  /// The server confirmed the requested Album or Folder is gone.
  | Readonly<{ kind: "missing" }>;

/// What a destination supplies to the source establishment it asks for.
type SourceEstablishmentOptions = Readonly<{
  /// The Grid anchor and focus to restore once the source is established.
  restoration?: NavigationGridRestoration;
  /// The Folder publication the requesting entry was established under.
  folderPublication?: string;
  /// An explained note presented after the destination commits.
  explanation?: string;
  /// How the committed destination is recorded in browser history.
  address?: "push" | "replace" | "none";
}>;

const SOURCE_ESTABLISHED: SourceEstablishment = { kind: "established" };
const SOURCE_SUPERSEDED: SourceEstablishment = { kind: "superseded" };
const SOURCE_FAILED: SourceEstablishment = { kind: "failed" };
const SOURCE_MISSING: SourceEstablishment = { kind: "missing" };
export function mountLibraryBrowser(
  root: HTMLElement,
  fetcher: BrowserFetch = fetch,
): () => void {
  return mountAccessBoundary(
    root,
    fetcher,
    (privateRoot, privateFetcher, signOut, cleanupFetcher) =>
      mountPrivateLibraryBrowser(
        privateRoot,
        privateFetcher,
        signOut,
        (token) => releaseBrowse(cleanupFetcher, token),
      ),
  );
}

function mountPrivateLibraryBrowser(
  root: HTMLElement,
  fetcher: BrowserFetch,
  signOut: () => void,
  releaseLease: (token: string) => Promise<void>,
): () => void {
  let applicationAlive = true;
  const recoveryGate = new RecoveryGate();
  const sourceGrid = createSourceGridOwner(fetcher, releaseLease);
  let photoMetadataAbort: AbortController | undefined;
  // The page-local navigation owner. Its traversal callback is bound once the
  // page controller's coordination functions exist, so the owner can be
  // created before them and still report every popstate.
  let applyNavigationTraversal: (
    traversal: NavigationTraversal,
  ) => void = () => {};
  // The Grid anchor is captured before Photo View takes the layout over, so a
  // Photo entry can replace the Grid entry it came from with the position the
  // Photographer left.
  let pendingGridRestoration: NavigationGridRestoration | undefined;
  const navigation = createNavigationOwner(
    { captureGridRestoration: () => pendingGridRestoration },
    (traversal) => applyNavigationTraversal(traversal),
  );
  // Startup elects exactly one source bootstrap from the committed Overview:
  // the address this document was loaded with. It replaces the unconditional
  // All Photos bootstrap and never races a second source open.
  const startup = navigation.start();
  // An invalid address is explained and replaced once with All Photos before
  // any request for the invalid source is made.
  let startupDestination: NavigationDestination =
    startup.kind === "destination" ? startup.destination : allPhotosDestination;
  let startupConsumed = false;
  /// The destination whose establishment is in flight, so a traversal that
  /// arrives while a newer intent is establishing a different destination is
  /// superseded instead of repainting it.
  let pendingDestination: NavigationDestination | undefined;
  /// The destination a failed traversal asked for, so the source Retry
  /// re-establishes it instead of reloading the Overview.
  let retryableTraversal: NavigationDestination | undefined;
  let startupExplanation: string | undefined =
    startup.kind === "invalid"
      ? "That link is not a valid Library Browser address. Showing All Photos."
      : undefined;
  // A reloaded Grid entry keeps the anchor and focus target it recorded, so
  // the destination it re-establishes restores where the Photographer left.
  let startupRestoration: NavigationGridRestoration | undefined =
    startup.kind === "destination" &&
    startup.entry.anchor &&
    startup.entry.focus
      ? { anchor: startup.entry.anchor, focus: startup.entry.focus }
      : undefined;
  const view: LibraryBrowserView = createLibraryBrowserView(
    root,
    handleViewIntent,
    (binding) => {
      if (binding.preview.state === "unavailable") return;
      if (binding.preview.thumbnailUrl) {
        sourceGrid.presentThumbnail(
          binding.photoId,
          binding.target,
          binding.preview.thumbnailUrl,
          true,
        );
        return;
      }
      void sourceGrid.loadThumbnail(binding.photoId, binding.target);
    },
    (binding) => sourceGrid.releaseThumbnail(binding.photoId, binding.target),
  );
  const photoOwner = createPhotoOwner(
    fetcher,
    {
      isSourceCurrent: (authority) => sourceGrid.isCurrent(authority),
      renewPhotoWindow: (authority) =>
        sourceGrid.isCurrent(authority)
          ? sourceGrid.renewPhotoWindow()
          : undefined,
      photoAt: (authority, index) =>
        sourceGrid.isCurrent(authority) ? sourceGrid.photoAt(index) : undefined,
      findPhotoIndex: (photoId) => sourceGrid.findPhotoIndex(photoId),
      movePosition: (authority, index) =>
        sourceGrid.moveGridPosition(authority, index),
      patchPreview: (authority, index, photoId, preview) =>
        sourceGrid.setPhotoPreview(authority, index, photoId, preview),
      patchSelection: (authority, index, photoId, selectionState) =>
        sourceGrid.setPhotoSelection(authority, index, photoId, selectionState),
      applyBatchSelection: (authority, photoId, priorValue, selectionState) =>
        sourceGrid.applyBatchSelection(
          authority,
          photoId,
          priorValue,
          selectionState,
        ),
      patchRating: (authority, index, photoId, rating) =>
        sourceGrid.setPhotoRating(authority, index, photoId, rating),
      noteCommittedDecision: (authority, photoId, field, value) =>
        sourceGrid.noteCommittedDecision(authority, photoId, field, value),
      trimFacts: (authority, anchor) => {
        if (sourceGrid.isCurrent(authority)) sourceGrid.trimFacts(anchor);
      },
    },
    {
      emit: (event) => {
        if (
          !photoOwner.isCurrent(event.authority) ||
          !view.isPhotoStatusSurfaceCurrent(event.surface)
        )
          return;
        view.setPhotoStatus(
          "Preview could not be loaded. You can continue browsing.",
        );
      },
    },
  );
  photoOwner.bindSource({
    sourceAuthority: sourceGrid.authority,
    total: sourceGrid.total,
    index: 0,
  });
  const savedPositions = createSavedPositionOwner(fetcher, {
    isSourceCurrent: (authority, albumId) =>
      sourceGrid.isCurrent(authority) &&
      sourceGrid.kind === "album" &&
      sourceGrid.albumId === albumId,
    isPhotoCurrent: (authority, photoId) =>
      photoOwner.isCurrent(authority) && photoOwner.current?.id === photoId,
  });
  const applicationRecoveries = new Map<ApplicationRecovery, RecoveryClaim>();
  let nextSummaryPresentationId = 0;
  let summaryAction:
    | Readonly<{
        presentationId: number;
        action: ApplicationSummaryAction;
      }>
    | undefined;

  const presentApplication = (presentation: ApplicationPresentation): void => {
    if (!applicationAlive) return;
    if (presentation.kind === "summary") {
      const presentationId = ++nextSummaryPresentationId;
      summaryAction = presentation.summary.action
        ? { presentationId, action: presentation.summary.action }
        : undefined;
      view.presentSummary(
        presentation.summary.text,
        presentation.summary.action
          ? { kind: presentation.summary.action.kind, presentationId }
          : undefined,
        presentation.summary.libraryCheckState,
      );
      return;
    }
    if (sourceGrid.kind === "album" && sourceGrid.albumId) {
      const open = presentation.albums.find(
        (candidate) => candidate.id === sourceGrid.albumId,
      );
      if (open) {
        sourceGrid.updateAlbum(open);
        view.setSourceTitle(sourceGrid.name);
      }
    }
    presentRecoveryNotice(presentation.overview.scan.lastRecovery);
    renderMembershipControls();
    refreshMembershipFacts();
    renderSources();
  };

  const coordinateApplication = async (
    coordination: ApplicationCoordination,
  ): Promise<void> => {
    if (!applicationAlive) return;
    if (coordination.kind === "mark-reachable") {
      // The probe answers on every poll, so an already established connection
      // under a reachable transport has nothing to restore and stays untouched.
      if (connectionEstablished && recoveryGate.transportReachable) return;
      setConnected(true);
      return;
    }
    if (coordination.kind === "transport-lost") {
      // Reachability is owned here, so a probe that repeats a loss the page
      // already applied changes nothing.
      if (!recoveryGate.transportReachable) return;
      recoveryGate.markTransportLost();
      syncConnection();
      return;
    }
    if (coordination.kind === "fail-application-recovery") {
      let claim = applicationRecoveries.get(coordination.recovery);
      if (!claim) {
        claim = recoveryGate.issue(
          coordination.slot,
          coordination.slot === "overview-reload" ? "overview" : "library",
        );
        applicationRecoveries.set(coordination.recovery, claim);
      }
      if (!recoveryGate.fail(claim, { transportLost: true }))
        recoveryGate.discard(claim);
      syncConnection();
      return;
    }
    if (coordination.kind === "recover") {
      const claim = applicationRecoveries.get(coordination.recovery);
      if (claim) recoveryGate.recover(claim);
      applicationRecoveries.delete(coordination.recovery);
      syncConnection();
      return;
    }
    if (coordination.kind === "reset-file-locations") {
      resetFileLocations();
      return;
    }
    if (coordination.kind === "load-file-location-root") {
      await loadFolderWindow("", 0, false);
      return;
    }

    if (!fileLocations.publication && coordination.overview.published) {
      if (sourceGrid.lastSource?.kind === "folder" && !sourceGrid.token) {
        await awaitRootBinding();
        if (!coordination.isCurrent()) return;
      } else {
        void loadFolderWindow("", 0, false);
      }
    }
    if (!coordination.isCurrent()) return;
    if (!sourceGrid.token && coordination.overview.published) {
      if (!startupConsumed) {
        // Startup elects exactly one source bootstrap: the destination this
        // document was loaded with. It replaces the unconditional All Photos
        // bootstrap and never races a second source open.
        startupConsumed = true;
        const destination = startupDestination;
        const explanation = startupExplanation;
        const restoration = startupRestoration;
        startupDestination = allPhotosDestination;
        startupExplanation = undefined;
        startupRestoration = undefined;
        const established = await establishDestination(destination, {
          addressed: true,
          ...(restoration ? { restoration } : {}),
          ...(explanation ? { explanation } : {}),
        });
        // A directly loaded Folder destination binds to the current Published
        // Library, so the entry it replaced records that publication. Without
        // the provenance a later traversal after a rescan would reopen the
        // Folder silently instead of requiring the explicit confirmation.
        if (established && sourceGrid.kind === "folder" && sourceGrid.token)
          navigation.replaceGrid(
            liveDestination(),
            undefined,
            fileLocations.publication,
          );
        return;
      }
      const remembered =
        sourceGrid.lastSource ?? ({ kind: "library" } as const);
      const bindable =
        remembered.kind !== "folder" || fileLocations.publication !== undefined;
      if (bindable) {
        await openSourceDescriptor(
          remembered,
          undefined,
          sourceGrid.order,
          sourceGrid.selection,
        );
      } else if (coordination.isCurrent()) {
        setGridStatusText("Could not load this source. Retry to continue.");
      }
    }
  };

  const handleApplicationEvent = (
    event: ApplicationEvent,
  ): void | Promise<void> =>
    event.kind === "summary" || event.kind === "overview"
      ? presentApplication(event)
      : coordinateApplication(event);

  const application = createApplicationOwner(fetcher, {
    emit: handleApplicationEvent,
  });
  const albumActions = createAlbumActionOwner(fetcher);
  const removal = createRemovalOwner(fetcher);
  /// One bounded page of removed Photos. The listing is not a source and
  /// creates no second browsing model: it presents the recovery path for the
  /// removals the Library has committed.
  const REMOVED_PAGE_LIMIT = 50;
  let removalReviewed = 0;
  let removalReviewOpen = false;
  let removalResult:
    | Readonly<{
        tone: "success" | "warning" | "failure";
        message: string;
      }>
    | undefined;
  let removedPage:
    | Readonly<{
        start: number;
        total: number;
        items: ReadonlyArray<RemovedPhotoItem>;
      }>
    | undefined;
  let removedLoadFailed = false;
  let removedPanelOpen = false;
  let removedAbort: AbortController | undefined;
  let removedPending = false;
  let removedRestoringId: string | undefined;
  let removedMessage: string | undefined;
  type AlbumFormRecord = Readonly<{
    formId: string;
    kind: AlbumFormReference["kind"];
    authority: AlbumFormAuthority;
    albumId?: string;
    initialName: string;
  }>;
  let albumForm: AlbumFormRecord | undefined;
  const dismissAlbumForm = (record: AlbumFormRecord): boolean => {
    if (!albumActions.isFormCurrent(record.authority)) return false;
    albumActions.closeForm(record.authority);
    if (albumForm === record) albumForm = undefined;
    view.dismissAlbumForm(record.formId);
    return true;
  };

  const ALBUM_NAME_MAXIMUM = 120;
  const albumNameError = (name: string): string | undefined => {
    const trimmed = name.trim();
    if (!trimmed) return "Enter an Album name.";
    if (Array.from(trimmed).length > ALBUM_NAME_MAXIMUM)
      return `Album names are at most ${ALBUM_NAME_MAXIMUM} characters.`;
    return undefined;
  };

  let connected = false;
  let connectionEstablished = false;
  let pageBusy = false;
  let photoRetryPending = false;
  // The Grid's multi-selection is session state of the open source: the
  // stable Photo identities the Photographer marked, the anchor a range
  // extends from, and whether Select mode makes every cell activation toggle
  // its Photo instead of opening it. Opening or reopening a source clears it.
  // One batch addresses at most MULTI_SELECTION_LIMIT Photos: the Grid refuses
  // to grow the selection past the bound the server enforces, so an invalid
  // batch can never be built.
  const MULTI_SELECTION_LIMIT = 100;
  type GridBatchResult = Readonly<{
    tone: "success" | "warning" | "failure";
    message: string;
    review?: Readonly<{ label: string }>;
    compensation?: Readonly<{ label: string }>;
  }>;
  type BatchAlbumCompensation = Readonly<{
    albumId: string;
    albumName: string;
    photoIds: ReadonlyArray<string>;
    sourceAuthority: SourceAuthority;
    hadSavedPosition: boolean;
  }>;
  let multiSelection = new Set<string>();
  // A settled batch can report that a retained Photo is no longer in the
  // current Library. It stays visible in the tray, but must not be sent in a
  // later batch request until the Photographer clears the selection.
  let multiMissingIds = new Set<string>();
  let multiChangedIds = new Set<string>();
  let multiExpectedSelection = new Map<string, SelectionState>();
  let multiAnchorId: string | undefined;
  let selectMode = false;
  let gridBatchResult: GridBatchResult | undefined;
  let batchAlbumCompensation: BatchAlbumCompensation | undefined;
  // One batch Add to Album in flight. Membership stays outside the Undo
  // contract, so it never touches the pending Undo description.
  let batchAlbumPending = false;
  const canOpenGridPhoto = () =>
    sourceGrid.isReady(sourceGrid.authority) &&
    !pageBusy &&
    !photoOwner.busy &&
    !photoOwner.opening;
  const browseRangeFailures = new Map<string, BrowseRangeFailure>();
  // Grid status text has one owner at a time. The range status rewrites the
  // line only when its own text changes, so merged window completions never
  // churn it, and every other status takes the line over until the range
  // reports again.
  let rangeStatusText: string | undefined;
  const setGridStatusText = (text: string) => {
    rangeStatusText = undefined;
    view.setGridStatus(text);
  };
  const setRangeStatusText = (text: string) => {
    if (text === rangeStatusText) return;
    rangeStatusText = text;
    view.setGridStatus(text);
  };
  /// Decision, Undo, and retry messages have one visible surface: the Photo
  /// View status while it is open, and the Grid status line otherwise, so a
  /// Grid keyboard decision reports its failure where the Photographer is.
  const setDecisionStatus = (text: string) => {
    if (view.gridVisible()) setGridStatusText(text);
    else view.setPhotoStatus(text);
  };
  // The range the Grid last reported for admission, with the source it was
  // reported for: window settlements present status only for that source.
  let admittedRange:
    | Readonly<{ start: number; end: number; authority: SourceAuthority }>
    | undefined;
  const firstMissingGridIndex = (
    range: Readonly<{ start: number; end: number }>,
  ): number | undefined => {
    for (let index = range.start; index < range.end; index += 1)
      if (sourceGrid.photoAt(index) === undefined) return index;
    return undefined;
  };
  /// The exact Retry status of the failed window that owns the first Photo the
  /// range is still missing, so a range report cannot hide an answered
  /// failure behind a fresh loading line.
  const rangeFailureStatus = (missing: number): string | undefined => {
    for (const failure of browseRangeFailures.values()) {
      if (failure.ownerScope !== "source" || !failure.retry) continue;
      if (!recoveryGate.isActive(failure.claim)) continue;
      if (!sourceGrid.isCurrent(failure.retry.sourceAuthority)) continue;
      if (sourceGrid.alignedStart(missing) !== failure.retry.start) continue;
      return failure.retry.message;
    }
    return undefined;
  };
  /// One truthful status for the reported range: the exact failure blocking
  /// it, the aligned window still loading for it, or Ready once every Photo
  /// in the range is present.
  const presentRangeStatus = () => {
    const range = admittedRange;
    if (!range || !sourceGrid.isCurrent(range.authority)) return;
    if (range.end <= range.start) return;
    const missing = firstMissingGridIndex(range);
    if (missing === undefined) {
      setRangeStatusText(`Ready · ${formatPhotoCount(sourceGrid.total)}`);
      return;
    }
    const failure = rangeFailureStatus(missing);
    if (failure !== undefined) {
      setRangeStatusText(failure);
      return;
    }
    const window = sourceGrid.describeWindow(missing);
    setRangeStatusText(
      `Loading ${window.range} of ${sourceGrid.total.toLocaleString()}…`,
    );
  };
  let albumRecovery: AlbumRecoveryRecord | undefined;
  const photoRecoveryKeys = new WeakMap<object, string>();
  let nextPhotoRecoveryKey = 0;
  const photoRecoveryKey = (authority: PhotoAuthority): string => {
    const known = photoRecoveryKeys.get(authority);
    if (known) return known;
    const key = String(++nextPhotoRecoveryKey);
    photoRecoveryKeys.set(authority, key);
    return key;
  };

  const currentPhoto = () => photoOwner.current;
  const currentAlbumRecovery = (): AlbumRecoveryRecord | undefined => {
    if (
      albumRecovery &&
      (!sourceGrid.isCurrent(albumRecovery.sourceAuthority) ||
        !recoveryGate.isActive(albumRecovery.claim))
    )
      albumRecovery = undefined;
    return albumRecovery;
  };
  const syncConnection = (message?: string) => {
    if (!applicationAlive) return;
    currentAlbumRecovery();
    for (const [key, failure] of browseRangeFailures)
      if (!recoveryGate.isActive(failure.claim))
        browseRangeFailures.delete(key);
    connected = connectionEstablished && recoveryGate.decisionReady;
    view.setConnection(
      connected,
      !connected && !photoOwner.active,
      !connected && photoOwner.active,
    );
    if (message) view.setPhotoStatus(message);
    updateControls();
  };
  const setConnected = (value: boolean, message?: string) => {
    if (!applicationAlive) return;
    connectionEstablished = value;
    if (value) recoveryGate.markReachable();
    syncConnection(message);
  };
  const windowFailureMessage = (
    outcome: Readonly<{
      range: string;
      transportLost: boolean;
      status?: number;
      malformed?: true;
    }>,
  ): string =>
    outcome.malformed === true
      ? `${outcome.range} returned an invalid response. Retry this range.`
      : outcome.transportLost
        ? `Connection lost while loading ${outcome.range}. Retry this range.`
        : `${outcome.range} could not be loaded (HTTP ${outcome.status}). Retry this range.`;
  const failBrowseRange = (
    ownerScope: "source" | "photo",
    generation: string,
    start: number,
    transportLost: boolean,
    retryRange?: GridRangeRetry,
    transition?: RecoveryTransition,
  ): void => {
    const key = `${ownerScope}:${generation}:${start}`;
    const active = browseRangeFailures.get(key);
    if (active && recoveryGate.isActive(active.claim)) {
      syncConnection();
      return;
    }
    if (active) browseRangeFailures.delete(key);
    const owner = { scope: ownerScope, generation };
    let claim: RecoveryClaim | undefined;
    if (transition) {
      try {
        const replacement = recoveryGate.issue("browse-window", key, {
          owner,
          transition,
        });
        if (
          recoveryGate.failTransition(transition, replacement, {
            transportLost,
          })
        )
          claim = replacement;
        else recoveryGate.discard(replacement);
      } catch {
        /* superseded transitions cannot affect the current range */
      }
    } else {
      const candidate = recoveryGate.issue("browse-window", key, { owner });
      if (recoveryGate.fail(candidate, { transportLost })) claim = candidate;
      else recoveryGate.discard(candidate);
    }
    if (claim)
      browseRangeFailures.set(key, {
        claim,
        ownerScope,
        ...(retryRange ? { retry: retryRange } : {}),
      });
    syncConnection();
  };
  const recoverBrowseRange = (
    ownerScope: "source" | "photo",
    generation: string,
    start: number,
  ): void => {
    const key = `${ownerScope}:${generation}:${start}`;
    const failure = browseRangeFailures.get(key);
    if (!failure) return;
    browseRangeFailures.delete(key);
    if (recoveryGate.recover(failure.claim)) setConnected(true);
  };
  const failPhotoRecovery = (
    authority: PhotoAuthority,
    kind: string,
    transition?: RecoveryTransition,
    transportLost = true,
  ): void => {
    if (!photoOwner.isCurrent(authority)) return;
    const generation = photoRecoveryKey(authority);
    const recoveryOwner = { scope: "photo" as const, generation };
    if (transition) {
      try {
        const replacement = recoveryGate.issue(kind, String(generation), {
          owner: recoveryOwner,
          transition,
        });
        if (
          recoveryGate.failTransition(transition, replacement, {
            transportLost,
          })
        ) {
          syncConnection();
          return;
        }
        recoveryGate.discard(replacement);
      } catch {
        /* the transition was already superseded or settled */
      }
    }
    const claim = recoveryGate.issue(kind, String(generation), {
      owner: recoveryOwner,
    });
    if (!recoveryGate.fail(claim, { transportLost }))
      recoveryGate.discard(claim);
    syncConnection();
  };
  /// The open source's order is view state: the select shows the order the
  /// open snapshot was built with, and stays disabled while an open is
  /// already busy so a second order cannot race the first.
  const renderSortControl = () => {
    if (!applicationAlive) return;
    const interactionBusy = pageBusy || photoRetryPending || photoOwner.busy;
    view.renderSort({
      kind: sourceGrid.kind,
      value: sourceGrid.order,
      enabled: !interactionBusy && !photoOwner.opening,
    });
  };

  /// The Selection State filter is a view option of the open source, so the
  /// control shows the filter that snapshot was built with and stays disabled
  /// while an open is already busy.
  const renderFilterControl = () => {
    if (!applicationAlive) return;
    const interactionBusy = pageBusy || photoRetryPending || photoOwner.busy;
    view.renderFilter({
      value: sourceGrid.selection,
      enabled: !interactionBusy && !photoOwner.opening,
    });
  };

  /// Decision progress for the open source. The counts come from the server
  /// when the source opens and follow confirmed decisions and Undo
  /// afterwards; the loaded Grid windows are never their source.
  const renderProgress = () => {
    if (!applicationAlive) return;
    const counts = sourceGrid.selectionCounts;
    view.renderProgress({
      // The counts belong to the complete source, while `total` belongs to
      // the currently open filtered Snapshot. A source replacement clears the
      // token, so the line never presents another source's counts.
      visible: sourceGrid.token !== "" || sourceGrid.total > 0,
      visibleTotal: sourceGrid.total,
      sourceTotal: counts.selected + counts.rejected + counts.undecided,
      selected: counts.selected,
      rejected: counts.rejected,
      undecided: counts.undecided,
    });
  };

  const updateControls = () => {
    if (!applicationAlive) return;
    const photo = currentPhoto();
    const gridEnabled = canOpenGridPhoto();
    // The strip's entries open Photos through the same path as the Grid's
    // cells, so one admission fact gates both: an activation that would be
    // refused silently is never presented as an enabled control.
    const filmstripEnabled = gridEnabled;
    const interactionBusy =
      pageBusy || photoRetryPending || photoOwner.busy || removal.busy;
    const recoveryEnabled =
      !pageBusy &&
      !photoRetryPending &&
      !photoOwner.busy &&
      !photoOwner.opening;
    const enabled = Boolean(photo) && connected && !interactionBusy;
    view.setControls({
      gridEnabled,
      filmstripEnabled,
      decisionEnabled: enabled,
      clearEnabled: enabled && photo?.selectionState !== "undecided",
      backEnabled: !interactionBusy,
      refreshEnabled: !interactionBusy,
      recoveryEnabled,
      removalEnabled:
        connected &&
        !interactionBusy &&
        !photoOwner.opening &&
        sourceGrid.token !== "" &&
        sourceGrid.selection === "rejected" &&
        sourceGrid.total > 0,
      previousEnabled:
        !interactionBusy && !photoOwner.opening && photoOwner.currentIndex > 0,
      nextEnabled:
        !interactionBusy &&
        !photoOwner.opening &&
        sourceGrid.total > 0 &&
        photoOwner.currentIndex < sourceGrid.total - 1,
      undoEnabled:
        connected &&
        !interactionBusy &&
        !photoOwner.opening &&
        photoOwner.canUndo,
    });
    renderSortControl();
    renderFilterControl();
    renderProgress();
    // Every state that moves the presented result passes here, so a review
    // that no longer covers it is withdrawn with the same update.
    reconcileRemovalReview();
  };

  /// Sends one admitted Album mutation and reports truthful outcomes.
  /// Admitted persistence is never aborted by a source or Photo change; the
  /// response always refreshes the bounded Album list, while notices stay
  /// owned by the initiating action, surface, generation, and epoch.
  const mutateAlbum = (
    start: (context: AlbumActionContext) => AlbumActionAdmission | undefined,
    surface: "photo" | "summary",
    photoOwnerAuthority = photoOwner.authority,
    form?: AlbumFormAuthority,
  ): Promise<{
    admitted: boolean;
    ok: boolean;
    latest: boolean;
    announce: (text: string) => void;
    removedFromCurrentAlbum?: Readonly<{
      albumId: string;
      photoId: string;
      sourceAuthority: SourceAuthority;
    }>;
    createdAlbum?: AlbumSummary;
    folderAdd?: Readonly<{
      matchedCount: number;
      addedCount: number;
      alreadyMemberCount: number;
    }>;
    membershipAdd?: Readonly<{
      albumId: string;
      addedPhotoIds: ReadonlyArray<string>;
      alreadyMemberPhotoIds: ReadonlyArray<string>;
      albums: ReadonlyArray<AlbumSummary>;
    }>;
    membershipRemove?: Readonly<{
      albumId: string;
      removedPhotoIds: ReadonlyArray<string>;
      alreadyAbsentPhotoIds: ReadonlyArray<string>;
      albums: ReadonlyArray<AlbumSummary>;
    }>;
  }> => {
    const capturedPhotoStatus = view.photoStatusSurface;
    const sourceOwner = sourceGrid.authority;
    const ownsPhotoSurface = () =>
      surface === "photo" &&
      photoOwner.isCurrent(photoOwnerAuthority) &&
      photoOwner.active &&
      view.isPhotoStatusSurfaceCurrent(capturedPhotoStatus);
    const action = start({
      sourceAuthority: sourceOwner,
      surface:
        surface === "photo"
          ? { kind: "photo", isCurrent: ownsPhotoSurface }
          : { kind: "summary" },
      ...(form ? { form } : {}),
    });
    if (!action)
      return Promise.resolve({
        admitted: false,
        ok: false,
        latest: false,
        announce: () => {},
      });
    const summaryPresentation = application.claimAlbumSummary(action.noticeKey);
    const disconnect = (authority: SourceAuthority) => {
      if (!sourceGrid.isCurrent(authority)) return;
      const active = currentAlbumRecovery();
      if (active?.sourceAuthority === authority) {
        recoveryGate.fail(active.claim, { transportLost: true });
        syncConnection();
        return;
      }
      const claim = recoveryGate.issue("album", action.noticeKey, {
        owner: {
          scope: "source",
          generation: String(sourceGrid.generation),
        },
      });
      if (recoveryGate.fail(claim, { transportLost: true }))
        albumRecovery = Object.freeze({ claim, sourceAuthority: authority });
      else recoveryGate.discard(claim);
      syncConnection();
    };
    const recoverConnection = (authority: SourceAuthority): void => {
      const active = currentAlbumRecovery();
      if (active?.sourceAuthority !== authority) return;
      if (
        recoveryGate.recover(active.claim) ||
        !recoveryGate.isActive(active.claim)
      )
        albumRecovery = undefined;
    };
    return (async () => {
      try {
        const outcome = await action.settlement;
        if (!applicationAlive)
          return {
            admitted: true,
            ok: outcome.kind === "persisted",
            latest: albumActions.isLatest(outcome.mutation),
            announce: () => {},
          };

        if (outcome.kind === "failed") {
          const presentOnPhoto = albumActions.canPresent(outcome.surface);
          if (presentOnPhoto) view.setPhotoStatus(outcome.failureMessage);
          else
            application.presentAlbumSummary(
              summaryPresentation,
              outcome.failureMessage,
            );
          if (presentOnPhoto)
            application.releaseAlbumSummary(summaryPresentation);
          if (outcome.connectivity === "lost-if-latest") {
            // Persistence is ambiguous even when a newer, unrelated Album
            // action owns presentation, so always invalidate the exact
            // position authority. Only the latest action may fence its
            // successor Overview or change connectivity.
            if (action.invalidatesSavedPositionFor)
              application.invalidateSavedPositionAuthority(
                action.invalidatesSavedPositionFor,
              );
            if (albumActions.isLatest(outcome.mutation)) {
              application.advanceAlbumMutationFloor();
              disconnect(outcome.sourceAuthority);
            }
          }
          return {
            admitted: true,
            ok: false,
            latest: albumActions.isLatest(outcome.mutation),
            announce: () => {},
          };
        }

        let disconnectAfterRefresh = false;
        if (action.invalidatesSavedPositionFor)
          application.invalidateSavedPositionAuthority(
            action.invalidatesSavedPositionFor,
          );
        if (application.advanceAlbumMutationFloor()) {
          try {
            const committed = await application.refreshOverview();
            if (
              committed &&
              albumActions.isLatest(outcome.mutation) &&
              sourceGrid.isCurrent(outcome.sourceAuthority)
            ) {
              recoverConnection(outcome.sourceAuthority);
              setConnected(true);
            }
            application.resolveAlbumSummary(summaryPresentation);
          } catch {
            if (applicationAlive && albumActions.isLatest(outcome.mutation)) {
              disconnectAfterRefresh = true;
              application.presentAlbumSummary(
                summaryPresentation,
                "The Album was saved but the Library summary could not be refreshed.",
              );
            } else application.releaseAlbumSummary(summaryPresentation);
          }
        }
        const presentOnSurface = albumActions.canPresent(outcome.surface);
        if (disconnectAfterRefresh) disconnect(outcome.sourceAuthority);
        return {
          admitted: true,
          ok: true,
          latest: albumActions.isLatest(outcome.mutation),
          announce: (text: string) => {
            if (presentOnSurface && ownsPhotoSurface())
              view.setPhotoStatus(text);
          },
          ...(outcome.removedFromCurrentAlbum
            ? { removedFromCurrentAlbum: outcome.removedFromCurrentAlbum }
            : {}),
          ...(outcome.createdAlbum
            ? { createdAlbum: outcome.createdAlbum }
            : {}),
          ...(outcome.folderAdd
            ? {
                folderAdd: {
                  matchedCount: outcome.folderAdd.matchedCount,
                  addedCount: outcome.folderAdd.addedCount,
                  alreadyMemberCount: outcome.folderAdd.alreadyMemberCount,
                },
              }
            : {}),
          ...(outcome.membershipAdd
            ? { membershipAdd: outcome.membershipAdd }
            : {}),
          ...(outcome.membershipRemove
            ? { membershipRemove: outcome.membershipRemove }
            : {}),
        };
      } finally {
        albumActions.finish(action.mutation);
      }
    })();
  };

  // File Location owns navigation lifetime, publication binding, retained
  // windows, and exact failed ranges. Application owns shared Overview and
  // Summary state; this page maps failures to exact global Recovery claims.
  const fileLocations = createFileLocationOwner(fetcher);
  type FileLocationPresentationRecord = Readonly<{
    summary: FileLocationPresentation;
    recovery: RecoveryClaim;
  }>;
  const fileLocationPresentations = new Map<
    FileLocationFailure,
    FileLocationPresentationRecord
  >();
  const fileLocationOutcomeSettlements = new WeakMap<object, Promise<void>>();
  let publicationLocationPresentation:
    | FileLocationPresentationRecord
    | undefined;

  const releaseFileLocationPresentation = (
    presentation: FileLocationPresentationRecord,
  ): void => {
    application.releaseFileLocation(presentation.summary);
    recoveryGate.recover(presentation.recovery);
  };

  const claimFileLocationPresentation = (
    key: string,
    message: string,
    transportLost: boolean,
  ): FileLocationPresentationRecord => {
    const summary = application.claimFileLocation(key, message);
    const recovery = recoveryGate.issue("file-location", key);
    recoveryGate.fail(recovery, {
      ...(transportLost ? { transportLost } : {}),
    });
    return { summary, recovery };
  };

  const releasePublicationLocationRecovery = (): void => {
    if (publicationLocationPresentation)
      releaseFileLocationPresentation(publicationLocationPresentation);
    publicationLocationPresentation = undefined;
    syncConnection();
  };

  const claimPublicationLocationNotice = (
    key: string,
    message: string,
  ): void => {
    releasePublicationLocationRecovery();
    publicationLocationPresentation = claimFileLocationPresentation(
      key,
      message,
      false,
    );
    syncConnection();
  };

  const resetFileLocations = (): FileLocationAuthority => {
    const authority = fileLocations.reset();
    if (publicationLocationPresentation)
      releaseFileLocationPresentation(publicationLocationPresentation);
    publicationLocationPresentation = undefined;
    for (const presentation of fileLocationPresentations.values())
      releaseFileLocationPresentation(presentation);
    fileLocationPresentations.clear();
    syncConnection();
    renderSources();
    return authority;
  };

  const rebindFileLocations = async (): Promise<FileLocationAuthority> => {
    application.notePublicationConflict();
    const authority = resetFileLocations();
    await application.refreshOverview().catch(() => {});
    await loadFolderWindow("", 0);
    return authority;
  };

  async function applyFileLocationOutcome(
    outcome: FileLocationOutcome,
  ): Promise<void> {
    if (!fileLocations.accept(outcome)) return;
    if (outcome.kind === "detached" || outcome.kind === "bound") return;
    if (outcome.kind === "publication-conflict") {
      const reboundAuthority = await rebindFileLocations();
      if (
        fileLocations.isCurrent(reboundAuthority) &&
        fileLocations.publication
      )
        claimPublicationLocationNotice(
          `publication:${fileLocations.publication}`,
          "Library changed. Reloaded folders.",
        );
      return;
    }
    if (outcome.kind === "failed") {
      if (outcome.replaced) {
        const replaced = fileLocationPresentations.get(outcome.replaced);
        if (replaced) releaseFileLocationPresentation(replaced);
        fileLocationPresentations.delete(outcome.replaced);
      }
      const presentation = claimFileLocationPresentation(
        `range:${outcome.generation}:${outcome.parent}:${outcome.page}`,
        outcome.failure.message,
        true,
      );
      fileLocationPresentations.set(outcome.failure, presentation);
      syncConnection();
      renderSources();
      return;
    }
    if (outcome.recovered) {
      const recovered = fileLocationPresentations.get(outcome.recovered);
      if (recovered) releaseFileLocationPresentation(recovered);
      fileLocationPresentations.delete(outcome.recovered);
    }
    if (outcome.remainingNewest) {
      const remaining = fileLocationPresentations.get(outcome.remainingNewest);
      if (remaining)
        application.presentFileLocation(
          remaining.summary,
          outcome.remainingNewest.message,
        );
    }
    if (outcome.markTransportReachable) setConnected(true);
    renderSources();
  }

  function handleFileLocationOutcome(
    outcome: FileLocationOutcome,
  ): Promise<void> {
    const pending = fileLocationOutcomeSettlements.get(outcome);
    if (pending) return pending;
    const settlement = Promise.resolve().then(() =>
      applyFileLocationOutcome(outcome),
    );
    fileLocationOutcomeSettlements.set(outcome, settlement);
    return settlement;
  }

  async function loadFolderWindow(
    parent: string,
    page: number,
    expand = true,
  ): Promise<void> {
    await handleFileLocationOutcome(
      await fileLocations.loadWindow(parent, page, expand),
    );
  }

  const awaitRootBinding = async (): Promise<boolean> => {
    const outcome = await fileLocations.awaitRootBinding();
    const boundByThisOutcome =
      outcome.kind === "bound" || outcome.kind === "loaded";
    await handleFileLocationOutcome(outcome);
    return boundByThisOutcome && Boolean(fileLocations.publication);
  };

  const fileLocationFailuresByKey = new Map<string, FileLocationFailure>();

  type FolderAlbumOperation = Readonly<{
    sourceAuthority: SourceAuthority;
    albumId: string;
    folderPath: string;
    publication: string;
    pending: boolean;
    status?: string;
  }>;
  let folderAlbumOperation: FolderAlbumOperation | undefined;
  let selectedFolderAlbumId = "";

  const folderPagerModel = (
    retained: FileLocationWindow | undefined,
  ): FolderViewModel["pager"] => {
    if (!retained || retained.total <= fileLocations.pageSize) return undefined;
    return {
      page: retained.page,
      pages: Math.max(1, Math.ceil(retained.total / fileLocations.pageSize)),
      hasPrevious: retained.page > 0,
      hasNext: (retained.page + 1) * fileLocations.pageSize < retained.total,
    };
  };

  const folderViewModel = (child: FolderChild): FolderViewModel => {
    const expanded = fileLocations.isExpanded(child.location);
    const retained = expanded
      ? fileLocations.window(child.location)
      : undefined;
    const pager = folderPagerModel(retained);
    return {
      location: child.location,
      name: child.name,
      photoCount: child.photoCount,
      hasDescendantFolders: child.hasDescendantFolders,
      expanded,
      enabled: Boolean(fileLocations.publication),
      active:
        sourceGrid.kind === "folder" &&
        sourceGrid.folder?.location === child.location,
      children: retained?.children.map(folderViewModel) ?? [],
      ...(pager ? { pager } : {}),
    };
  };

  const renderSources = () => {
    if (!applicationAlive) return;
    fileLocationFailuresByKey.clear();
    const failures = fileLocations.failures().map((failure) => {
      const key = `${failure.generation}:${failure.parent}:${failure.page}`;
      fileLocationFailuresByKey.set(key, failure);
      return { key, range: failure.range };
    });
    const rootWindow = fileLocations.window("");
    const rootPager = folderPagerModel(rootWindow);
    const model: SourceListViewModel = {
      libraryCount: application.overview?.photoCount ?? 0,
      libraryActive: sourceGrid.kind === "library",
      fileLocationsEnabled: Boolean(fileLocations.publication),
      fileLocationFailures: failures,
      rootExpanded: fileLocations.isExpanded(""),
      rootActive:
        sourceGrid.kind === "folder" && sourceGrid.folder?.location === "",
      rootChildren:
        fileLocations.isExpanded("") && rootWindow
          ? rootWindow.children.map(folderViewModel)
          : [],
      ...(rootPager ? { rootPager } : {}),
      albums: application.albums.map((album) => ({
        id: album.id,
        name: album.name,
        photoCount: album.photoCount,
        hasSavedPosition: album.hasSavedPosition,
        active: sourceGrid.kind === "album" && sourceGrid.albumId === album.id,
      })),
    };
    view.renderSources(model);
    const folder = sourceGrid.kind === "folder" ? sourceGrid.folder : undefined;
    const folderPublication =
      sourceGrid.kind === "folder" ? fileLocations.publication : undefined;
    const operation =
      folder &&
      folderPublication &&
      folderAlbumOperation?.sourceAuthority === sourceGrid.authority &&
      folderAlbumOperation.folderPath === folder.location &&
      folderAlbumOperation.publication === folderPublication
        ? folderAlbumOperation
        : undefined;
    if (!folder || !folderPublication || application.albums.length === 0) {
      view.renderFolderAlbum({
        visible: false,
        folderPath: "",
        albums: [],
        selectedAlbumId: "",
        pending: false,
      });
    } else {
      const firstAlbum = application.albums[0];
      if (
        firstAlbum &&
        !application.albums.some((album) => album.id === selectedFolderAlbumId)
      )
        selectedFolderAlbumId = firstAlbum.id;
      view.renderFolderAlbum({
        visible: true,
        folderPath: folder.location,
        albums: application.albums.map(({ id, name }) => ({ id, name })),
        selectedAlbumId: selectedFolderAlbumId,
        pending: operation?.pending ?? false,
        ...(operation?.status ? { status: operation.status } : {}),
      });
    }
    renderBatchAlbums();
  };

  const openAlbumForm = (form: AlbumFormReference): void => {
    albumForm = {
      formId: form.formId,
      kind: form.kind,
      authority: albumActions.openForm(form.formId),
      ...(form.albumId ? { albumId: form.albumId } : {}),
      initialName: form.name,
    };
  };

  const closeAlbumForm = (formId: string): void => {
    const record = albumForm;
    if (!record || record.formId !== formId) return;
    dismissAlbumForm(record);
  };

  const submitAlbumForm = async (
    formId: string,
    draft?: string,
  ): Promise<void> => {
    const record = albumForm;
    if (
      !record ||
      record.formId !== formId ||
      !albumActions.isFormCurrent(record.authority)
    )
      return;
    if (record.kind === "delete") {
      const albumId = record.albumId!;
      view.setAlbumFormPending(formId, true);
      const { ok: deleted } = await mutateAlbum(
        (context) => albumActions.delete(albumId, context),
        "summary",
        photoOwner.authority,
        record.authority,
      );
      if (albumActions.isFormCurrent(record.authority))
        dismissAlbumForm(record);
      if (deleted && batchAlbumCompensation?.albumId === albumId) {
        expireBatchAlbumCompensation();
        renderGrid();
      }
      renderSources();
      if (
        deleted &&
        sourceGrid.kind === "album" &&
        sourceGrid.albumId === albumId
      )
        // The open Album's destination became invalid, so the current entry is
        // replaced with All Photos rather than left naming a deleted Album.
        await openSource(
          "library",
          undefined,
          undefined,
          undefined,
          "source-default",
          "all",
          {
            address: "replace",
          },
        );
      return;
    }

    const name = (draft ?? "").trim();
    if (record.kind === "rename" && (!name || name === record.initialName)) {
      dismissAlbumForm(record);
      renderSources();
      return;
    }
    const invalid = albumNameError(name);
    if (invalid) {
      view.setAlbumFormMessage(formId, invalid);
      return;
    }
    view.setAlbumFormPending(formId, true, name);
    const sourceAuthority = sourceGrid.authority;
    const photoAuthority = photoOwner.authority;
    const result = await mutateAlbum(
      (context) =>
        record.kind === "create"
          ? albumActions.create(name, context)
          : albumActions.rename(record.albumId!, name, context),
      "summary",
      photoAuthority,
      record.authority,
    );
    const formIsCurrent = albumActions.isFormCurrent(record.authority);
    const createdAlbum =
      record.kind === "create" ? result.createdAlbum : undefined;
    if (formIsCurrent) {
      if (result.ok && (record.kind === "rename" || createdAlbum))
        dismissAlbumForm(record);
      else view.setAlbumFormPending(formId, false);
    }
    renderSources();
    if (
      formIsCurrent &&
      sourceGrid.isCurrent(sourceAuthority) &&
      photoOwner.isCurrent(photoAuthority) &&
      result.ok &&
      createdAlbum
    )
      // Creating an Album from the Sources panel chooses a new destination, so
      // it creates one Grid entry exactly like choosing any other source.
      await openSource(
        "album",
        createdAlbum,
        undefined,
        undefined,
        "source-default",
        "all",
        {
          address: "push",
        },
      );
  };

  const addFolderToAlbum = (albumId: string): void => {
    expireBatchAlbumCompensation();
    const folder = sourceGrid.kind === "folder" ? sourceGrid.folder : undefined;
    const publication =
      sourceGrid.kind === "folder" ? fileLocations.publication : undefined;
    if (
      !folder ||
      !publication ||
      !application.albums.some((album) => album.id === albumId)
    )
      return;
    if (
      albumActions.isFolderMembersAdmitted(
        albumId,
        folder.location,
        publication,
      )
    )
      return;
    selectedFolderAlbumId = albumId;
    const sourceAuthority = sourceGrid.authority;
    const folderPath = folder.location;
    folderAlbumOperation = {
      sourceAuthority,
      albumId,
      folderPath,
      publication,
      pending: true,
    };
    renderSources();
    void (async () => {
      const result = await mutateAlbum(
        (context) =>
          albumActions.addFolderMembers(
            albumId,
            folderPath,
            publication,
            context,
          ),
        "summary",
      );
      if (
        !sourceGrid.isCurrent(sourceAuthority) ||
        sourceGrid.kind !== "folder" ||
        sourceGrid.folder?.location !== folderPath ||
        fileLocations.publication !== publication
      )
        return;
      const status = result.ok
        ? result.folderAdd
          ? `Added ${result.folderAdd.addedCount.toLocaleString()} Photos. ${result.folderAdd.alreadyMemberCount.toLocaleString()} already in the Album.`
          : "Folder added to the Album."
        : "The Folder could not be added to the Album. Try again.";
      folderAlbumOperation = {
        sourceAuthority,
        albumId,
        folderPath,
        publication,
        pending: false,
        status,
      };
      renderSources();
    })();
  };

  const cancelScheduledGridRender = () => {
    view.cancelGridRender();
  };

  const openSource = async (
    kind: "library" | "album" | "folder",
    album?: AlbumSummary,
    preferredPhotoId?: string,
    folder?: { location: string; name: string },
    order: SourceViewOrder = "source-default",
    selection: SelectionFilter = "all",
    establishment: SourceEstablishmentOptions = {},
  ) => {
    const descriptor: SourceGridSource =
      kind === "library"
        ? { kind: "library" }
        : kind === "album"
          ? {
              kind: "album",
              album: { id: album!.id, name: album!.name },
            }
          : {
              kind: "folder",
              folder: folder!,
              publication: fileLocations.publication!,
            };
    return openSourceDescriptor(
      descriptor,
      preferredPhotoId,
      order,
      selection,
      establishment,
    );
  };

  async function openSourceDescriptor(
    requested: SourceGridSource,
    preferredPhotoId?: string,
    order: SourceViewOrder = "source-default",
    selection: SelectionFilter = "all",
    establishment: SourceEstablishmentOptions = {},
  ): Promise<SourceEstablishment> {
    const descriptor: SourceGridSource =
      requested.kind === "folder" && fileLocations.publication
        ? { ...requested, publication: fileLocations.publication }
        : requested;
    pageBusy = true;
    updateControls();
    cancelScheduledGridRender();
    photoMetadataAbort?.abort();
    photoMetadataAbort = undefined;
    retryableTraversal = undefined;
    pendingDestination = {
      source: requested.kind,
      ...(requested.kind === "folder"
        ? { folderPath: requested.folder.location }
        : {}),
      ...(requested.kind === "album" ? { albumId: requested.album.id } : {}),
      ...(order !== "source-default" ? { order } : {}),
      selection,
    };
    const pendingOpen = sourceGrid.open(descriptor, {
      ...(preferredPhotoId ? { preferredPhotoId } : {}),
      order,
      selection,
    });
    const authority = sourceGrid.authority;
    const generation = sourceGrid.generation;
    const photoAuthority = photoOwner.bindSource({
      sourceAuthority: authority,
      total: sourceGrid.total,
      index: 0,
      ...(sourceGrid.albumId ? { albumId: sourceGrid.albumId } : {}),
      ...(preferredPhotoId ? { preferredPhotoId } : {}),
    });
    const sourceTransition = recoveryGate.beginTransition(
      "source",
      String(generation),
    );
    const photoTransition = recoveryGate.beginTransition(
      "photo",
      photoRecoveryKey(photoAuthority),
    );
    recoveryGate.succeedTransition(photoTransition);
    syncConnection();
    view.prepareSourceOpen(sourceGrid.name);
    // The multi-selection names Photos of the source that was open: a new
    // source starts empty, and its tray presents nothing until the
    // Photographer marks Photos again.
    clearMultiSelection();
    renderSortControl();
    try {
      const opened = await pendingOpen;
      if (opened.kind === "detached") return SOURCE_SUPERSEDED;
      if (opened.kind === "publication-conflict") {
        // Only the current source's handler may reset and reload File
        // Locations: a superseded open doing the same would discard the
        // newer recovery and leave the tree unbound.
        if (!sourceGrid.isCurrent(authority)) return SOURCE_SUPERSEDED;
        const reboundAuthority = await rebindFileLocations();
        if (!sourceGrid.isCurrent(authority)) return SOURCE_SUPERSEDED;
        if (
          fileLocations.isCurrent(reboundAuthority) &&
          fileLocations.publication
        )
          claimPublicationLocationNotice(
            `publication:${fileLocations.publication}`,
            "Library changed. Reopen this folder.",
          );
        throw new Error("source open failed");
      }
      if (opened.kind === "failed") {
        // A 404 is the server confirming that the requested Album or Folder
        // is gone, which is the only answer that licenses an explained
        // fallback. Every other answer keeps its retryable failure.
        if (opened.status === 404) return SOURCE_MISSING;
        throw new Error("source open failed");
      }
      const gridPosition = sourceGrid.readGridPosition(authority);
      if (gridPosition === undefined) return SOURCE_SUPERSEDED;
      photoOwner.updateSource({
        sourceAuthority: authority,
        total: sourceGrid.total,
        index: gridPosition,
        ...(sourceGrid.albumId ? { albumId: sourceGrid.albumId } : {}),
        ...(preferredPhotoId ? { preferredPhotoId } : {}),
      });
      view.scrollToGridIndex(gridPosition);
      renderSources();
      // A restoration resolves its anchor against the established Snapshot
      // before the covering window is admitted, so the restored row is the
      // row that is loaded.
      const restoration = establishment.restoration;
      const restoreIndex = restoration
        ? await resolveRestorationIndex(authority, gridPosition, restoration)
        : gridPosition;
      if (restoreIndex === undefined) return SOURCE_SUPERSEDED;
      const windowReady = await loadWindow(
        restoreIndex,
        { kind: "source", authority },
        false,
        "high",
        sourceTransition,
      );
      if (!sourceGrid.isCurrent(authority) || !windowReady)
        return SOURCE_SUPERSEDED;
      if (sourceGrid.kind === "folder") releasePublicationLocationRecovery();
      recoveryGate.succeedTransition(sourceTransition);
      sourceGrid.establish(authority);
      setConnected(true);
      // A source replacement empties the snapshot while its open is in
      // flight, and any render during that window clamps the Grid to the
      // top. Position the reopened Grid through the render that restores the
      // scroll height, so a clamped scroll cannot survive it.
      renderGrid(restoreIndex);
      if (restoration)
        view.restoreGridAnchor(restorationGeometry(restoreIndex, restoration));
      if (sourceGrid.total) {
        presentRangeStatus();
        // An explained fallback names the confirmed invalid or missing target
        // after the ordinary status has been presented.
        if (establishment.explanation)
          setGridStatusText(establishment.explanation);
      } else if (sourceGrid.selection === "all") {
        setGridStatusText(formatPhotoCount(0));
        view.setGridEmpty(emptySourceStatus(), sourceGrid.kind !== "album");
      } else {
        // A filtered view that matches nothing leaves its source intact, so
        // it must not report an empty source or offer a Library check.
        setGridStatusText(formatPhotoCount(0));
        view.setGridEmpty("No Photos match this filter.");
      }
      // The committed destination is recorded once, so the navigation owner's
      // current entry always names the source the page presents.
      if (establishment.address === "push")
        navigation.openGrid(liveDestination(), fileLocations.publication);
      else if (establishment.address === "replace")
        navigation.replaceGrid(
          liveDestination(),
          undefined,
          fileLocations.publication,
        );
      return SOURCE_ESTABLISHED;
    } catch {
      if (!sourceGrid.isCurrent(authority)) return SOURCE_SUPERSEDED;
      setGridStatusText("Could not load this source. Retry to continue.");
      const claim = recoveryGate.issue("source-open", String(generation), {
        owner: { scope: "source", generation: String(generation) },
        transition: sourceTransition,
      });
      recoveryGate.failTransition(sourceTransition, claim, {
        transportLost: true,
      });
      syncConnection();
      return SOURCE_FAILED;
    } finally {
      pendingDestination = undefined;
      if (sourceGrid.isCurrent(authority)) {
        pageBusy = false;
        updateControls();
      }
    }
  }

  /// Commits one View options draft. A thumbnail-size-only change never
  /// reaches here: it is Grid presentation and keeps the open Snapshot and its
  /// anchor. Order and filter commit together, so a combined change opens one
  /// view with both choices and the existing identity-anchor rules.
  const applyViewOptions = async (
    order: SourceViewOrder,
    selection: SelectionFilter,
  ): Promise<void> => {
    if (!applicationAlive || pageBusy || photoOwner.busy) return;
    const orderChanged = order !== sourceGrid.order;
    const filterChanged = selection !== sourceGrid.selection;
    if (!orderChanged && !filterChanged) return;
    // A Folder reopen needs the current File Location binding: without it a
    // committed change can only send a stale publication and fail as a false
    // disconnection. Match the refresh/reopen precondition.
    if (sourceGrid.kind === "folder" && !fileLocations.publication) {
      const bound = await awaitRootBinding();
      if (!applicationAlive || !bound) {
        if (applicationAlive)
          setGridStatusText("Could not load this source. Retry to continue.");
        return;
      }
    }
    await openSourceDescriptor(
      sourceGrid.source,
      photoOwner.lastCurrentPhotoId,
      order,
      selection,
      { address: "replace" },
    );
  };

  const emptySourceStatus = (): string => {
    if (sourceGrid.kind === "album")
      return "This Album contains no Photos. Add Photos from another source's Photo View.";
    return "No supported Photos found. Check the Library Folder or add supported files, then run Check Library.";
  };

  /// Reopens the current source with a fresh Snapshot of the same order and
  /// filter, keeping the anchor. `reason` names why, because an expired Library
  /// order is not the only cause: a committed removal or restore also leaves
  /// the open Snapshot stale, and the status line must not blame the order.
  const reopenExpired = async (
    anchorIndex: number,
    expectedGeneration = sourceGrid.generation,
    preferredPhotoId?: string,
    reason: Readonly<{ progress: string; settled: string }> = {
      progress:
        "Library order expired. Reopening this source from the latest Library…",
      settled: "Source reopened using the latest published Library order.",
    },
  ) => {
    if (expectedGeneration !== sourceGrid.generation) return;
    pageBusy = true;
    // A reopen builds a new Snapshot of the same source, so the
    // multi-selection starts empty here too; the render after the reopen
    // clears the markers on the retained cells.
    clearMultiSelection();
    updateControls();
    const resumePhoto = photoOwner.active;
    const resumeIndex = photoOwner.currentIndex;
    const currentSourceAuthority = photoOwner.sourceAuthority;
    if (currentSourceAuthority)
      photoOwner.rebindSource({
        sourceAuthority: currentSourceAuthority,
        total: photoOwner.total,
        index: resumeIndex,
        ...(sourceGrid.albumId ? { albumId: sourceGrid.albumId } : {}),
        ...(photoOwner.lastCurrentPhotoId
          ? { preferredPhotoId: photoOwner.lastCurrentPhotoId }
          : {}),
      });
    // A traversal names the Photo it wants resolved, so it wins over the
    // anchor the Grid or the current Photo would supply.
    const anchorId =
      preferredPhotoId ??
      sourceGrid.photoAt(anchorIndex)?.id ??
      photoOwner.lastCurrentPhotoId ??
      currentPhoto()?.id;
    cancelScheduledGridRender();
    sourceGrid.clearRenderedThumbnails();
    let boundPublication = fileLocations.publication;
    if (sourceGrid.kind === "folder" && !boundPublication) {
      // A Folder source must never be reopened publicationless; wait for
      // the root binding and fail truthfully if it cannot be established.
      boundPublication = (await awaitRootBinding())
        ? fileLocations.publication
        : undefined;
      if (expectedGeneration !== sourceGrid.generation) return;
      if (!boundPublication) {
        // Fail truthfully instead of sending a publicationless request.
        setGridStatusText("Could not load this source. Retry to continue.");
        const claim = recoveryGate.issue(
          "source-reopen",
          String(expectedGeneration),
          {
            owner: {
              scope: "source",
              generation: String(expectedGeneration),
            },
          },
        );
        recoveryGate.fail(claim, { transportLost: true });
        syncConnection();
        pageBusy = false;
        updateControls();
        return;
      }
    }
    const descriptor: SourceGridSource =
      sourceGrid.source.kind === "folder"
        ? {
            ...sourceGrid.source,
            publication: boundPublication!,
          }
        : sourceGrid.source;
    const pendingOpen = sourceGrid.open(descriptor, {
      mode: "reopen",
      order: sourceGrid.order,
      selection: sourceGrid.selection,
      ...(anchorId ? { preferredPhotoId: anchorId } : {}),
    });
    // The reopen detaches the images the Grid had in flight and keeps its
    // retained cells. Binding those thumbnails again right here - from the URL
    // the owner still holds - restores them without a render, so the retained
    // range and the status line stay exactly as the reopen found them.
    view.rebindDetachedGridCells({
      total: sourceGrid.total,
      photoAt: (index) => sourceGrid.photoAt(index),
    });
    const authority = sourceGrid.authority;
    const generation = sourceGrid.generation;
    const photoAuthority = photoOwner.rebindSource({
      sourceAuthority: authority,
      total: sourceGrid.total,
      index: resumeIndex,
      ...(sourceGrid.albumId ? { albumId: sourceGrid.albumId } : {}),
      ...(anchorId ? { preferredPhotoId: anchorId } : {}),
    });
    const photoTransition = recoveryGate.beginTransition(
      "photo",
      photoRecoveryKey(photoAuthority),
    );
    if (!resumePhoto) recoveryGate.succeedTransition(photoTransition);
    const sourceTransition = recoveryGate.beginTransition(
      "source",
      String(generation),
    );
    syncConnection();
    const notice = reason.progress;
    setGridStatusText(notice);
    view.setPhotoStatus(notice);
    try {
      const opened = await pendingOpen;
      if (opened.kind === "detached") return;
      if (opened.kind === "publication-conflict") {
        // Generation-gated exactly like openSource: only the current
        // source's recovery may reset and rebind File Locations.
        if (sourceGrid.isCurrent(authority)) {
          const reboundAuthority = await rebindFileLocations();
          if (
            sourceGrid.isCurrent(authority) &&
            fileLocations.isCurrent(reboundAuthority) &&
            fileLocations.publication
          )
            claimPublicationLocationNotice(
              `publication:${fileLocations.publication}`,
              "Library changed. Reopen this folder.",
            );
        }
        throw new Error("browse reopen failed");
      }
      if (opened.kind === "failed") throw new Error("browse reopen failed");
      const gridPosition = sourceGrid.readGridPosition(authority);
      if (gridPosition === undefined) return;
      photoOwner.updateSource({
        sourceAuthority: authority,
        total: sourceGrid.total,
        index: gridPosition,
        ...(sourceGrid.albumId ? { albumId: sourceGrid.albumId } : {}),
        ...(anchorId ? { preferredPhotoId: anchorId } : {}),
      });
      const windowReady = await loadWindow(
        gridPosition,
        { kind: "source", authority },
        false,
        "high",
        sourceTransition,
      );
      if (!sourceGrid.isCurrent(authority) || !windowReady) return;
      // A hidden Grid keeps its retained cells: the next visible render
      // rebuilds them, and only the visible Grid may touch its DOM.
      if (view.gridVisible()) view.clearGridCells();
      renderGrid(gridPosition);
      // A reopen that leaves the source empty presents the same explained
      // state the open path presents. Without it an emptied Grid would be
      // blank, and a blank Grid cannot be told from a broken one.
      if (sourceGrid.total === 0) {
        setGridStatusText(formatPhotoCount(0));
        view.setGridEmpty(
          sourceGrid.selection === "all"
            ? emptySourceStatus()
            : "No Photos match this filter.",
          sourceGrid.selection === "all" && sourceGrid.kind !== "album",
        );
      } else setGridStatusText(reason.settled);
      if (sourceGrid.kind === "folder") releasePublicationLocationRecovery();
      recoveryGate.succeedTransition(sourceTransition);
      sourceGrid.establish(authority);
      setConnected(true);
      if (resumePhoto && photoOwner.isCurrent(photoAuthority)) {
        view.enterPhoto();
        renderPhotoShell(photoAuthority);
        void showPreview(photoAuthority, photoTransition).then(
          async (refreshed) => {
            if (!refreshed || !photoOwner.isCurrent(photoAuthority)) return;
            const persisted = await persistPosition(photoAuthority);
            if (persisted && photoOwner.isCurrent(photoAuthority)) {
              recoveryGate.succeedTransition(photoTransition);
              setConnected(true);
            }
          },
        );
      }
    } catch {
      if (!sourceGrid.isCurrent(authority)) return;
      const failure =
        "This source expired and could not be reopened. Retry the connection.";
      setGridStatusText(failure);
      view.setPhotoStatus(failure);
      const claim = recoveryGate.issue("source-reopen", String(generation), {
        owner: { scope: "source", generation: String(generation) },
        transition: sourceTransition,
      });
      recoveryGate.failTransition(sourceTransition, claim, {
        transportLost: true,
      });
      syncConnection();
    } finally {
      if (sourceGrid.isCurrent(authority)) {
        pageBusy = false;
        updateControls();
      }
    }
  };
  const loadWindow = async (
    index: number,
    operation: SourceWindowOperation = {
      kind: "grid",
      authority: sourceGrid.authority,
    },
    quiet = false,
    priority: "high" | "low" = quiet ? "low" : "high",
    transition?: RecoveryTransition,
    photoOwnerAuthority = photoOwner.authority,
  ): Promise<boolean> => {
    if (sourceGrid.total === 0) return true;
    const sourceAuthority =
      operation.kind === "photo" ? sourceGrid.authority : operation.authority;
    const windowAuthority =
      operation.kind === "photo" ? operation.authority : undefined;
    const ownerScope = operation.kind === "photo" ? "photo" : "source";
    const ownerGeneration =
      operation.kind === "photo"
        ? photoRecoveryKey(photoOwnerAuthority)
        : String(sourceGrid.generation);
    const { range } = sourceGrid.describeWindow(index);
    if (!quiet)
      setGridStatusText(
        `Loading ${range} of ${sourceGrid.total.toLocaleString()}…`,
      );
    const outcome = await sourceGrid.loadWindow(index, operation, {
      quiet,
      priority,
    });
    try {
      const exactOwner =
        outcome.authority === sourceAuthority &&
        sourceGrid.isCurrent(sourceAuthority) &&
        (operation.kind === "photo"
          ? outcome.owner.scope === "photo" &&
            outcome.owner.authority === windowAuthority &&
            photoOwner.ownsWindow(photoOwnerAuthority, outcome.owner.authority)
          : outcome.owner.scope === "source" &&
            String(outcome.owner.generation) === ownerGeneration);
      if (!exactOwner) return false;
      if (outcome.kind === "detached") return false;
      if (outcome.kind === "expired") {
        await reopenExpired(outcome.index, sourceGrid.generation);
        return false;
      }
      if (outcome.kind === "failed") {
        const message = windowFailureMessage(outcome);
        if (ownerScope === "photo") view.setPhotoStatus(message);
        else setGridStatusText(message);
        failBrowseRange(
          ownerScope,
          ownerGeneration,
          outcome.start,
          outcome.transportLost,
          operation.kind === "photo"
            ? undefined
            : {
                sourceAuthority,
                operationKind: operation.kind,
                anchorIndex: index,
                start: outcome.start,
                quiet,
                priority,
                message,
              },
          transition,
        );
        return false;
      }
      recoverBrowseRange(ownerScope, ownerGeneration, outcome.start);
      // A window this caller awaited changes what the Grid presents, and the
      // merged notification does not report it, so the caller owns the render
      // request too.
      if (outcome.changed && view.gridVisible()) view.scheduleGridRender();
      if (!quiet) {
        if (admittedRange && sourceGrid.isCurrent(admittedRange.authority))
          presentRangeStatus();
        else setGridStatusText(`Ready · ${formatPhotoCount(sourceGrid.total)}`);
      }
      return true;
    } finally {
      updateControls();
    }
  };
  /// One notification per completed Grid window, however many consumers joined
  /// it. Range admission is fire-and-forget, so this is where a window the Grid
  /// itself admitted presents its outcome and asks for the merged render; a
  /// window a control-flow caller awaits still settles through that caller.
  const unsubscribeWindowSettled = sourceGrid.onWindowSettled((outcome) => {
    if (!applicationAlive) return;
    // A window request captured before a source change never commits here.
    if (!sourceGrid.isCurrent(outcome.authority)) return;
    // A fire-and-forget Grid or range window settles here and can hand the
    // strip neighbor facts from either scope; a window an awaited caller
    // loaded re-renders the strip through that caller instead.
    if (outcome.kind === "loaded") renderFilmstrip();
    // Photo windows present through the Photo surface that awaits them.
    if (outcome.owner.scope !== "source") return;
    const generation = String(outcome.owner.generation);
    switch (outcome.kind) {
      case "loaded":
        recoverBrowseRange("source", generation, outcome.start);
        if (outcome.changed && view.gridVisible()) view.scheduleGridRender();
        presentRangeStatus();
        return;
      case "failed": {
        const message = windowFailureMessage(outcome);
        setGridStatusText(message);
        failBrowseRange(
          "source",
          generation,
          outcome.start,
          outcome.transportLost,
          {
            sourceAuthority: outcome.authority,
            // A source whose first window has not established current facts
            // retries as that window, exactly like the awaited path does.
            operationKind: sourceGrid.isReady(outcome.authority)
              ? "grid"
              : "source",
            anchorIndex: windowAnchorIndex(outcome.start),
            start: outcome.start,
            quiet: false,
            priority: "high",
            message,
          },
        );
        updateControls();
        return;
      }
      case "expired":
        // Concurrent expired windows share one reopen: the first call
        // supersedes the source generation the others still hold.
        void reopenExpired(outcome.start, sourceGrid.generation);
        return;
      case "detached":
        if (view.gridVisible()) view.scheduleGridRender();
        return;
    }
  });
  const renderGrid = (position?: number) => {
    if (!applicationAlive) return;
    view.renderGrid(
      {
        total: sourceGrid.total,
        multi: {
          mode: selectMode,
          count: multiSelection.size,
          limit: MULTI_SELECTION_LIMIT,
          // A batch action is presented only while it would be admitted: the
          // same source readiness, connection, and idle owner a Grid decision
          // needs, so an activation is never refused silently.
          enabled: connected && canOpenGridPhoto(),
          result: gridBatchResult,
          selected: (index) => {
            const photo = sourceGrid.photoAt(index);
            return photo !== undefined && multiSelection.has(photo.id);
          },
        },
        photoAt: (index) => sourceGrid.photoAt(index),
      },
      position,
    );
    updateControls();
  };

  /// Presents the batch tray's Album choices from the bounded Album summary.
  const renderBatchAlbums = () => {
    if (!applicationAlive) return;
    view.renderBatchAlbums({
      albums: application.albums.map(({ id, name }) => ({ id, name })),
      pending: batchAlbumPending,
    });
  };

  /// One Photo count with its noun, so every batch message reads correctly
  /// for a single Photo and for many.
  const photoCountText = (count: number): string =>
    `${count.toLocaleString()} ${count === 1 ? "Photo" : "Photos"}`;

  const expireBatchAlbumCompensation = () => {
    batchAlbumCompensation = undefined;
    if (!gridBatchResult?.compensation) return;
    gridBatchResult = {
      tone: gridBatchResult.tone,
      message: gridBatchResult.message,
      ...(gridBatchResult.review ? { review: gridBatchResult.review } : {}),
    };
  };

  /// Empties the Grid's multi-selection and leaves Select mode. The caller
  /// owns the render, so a source open that clears it presents the cleared
  /// Grid in its own render; the view is told here as well, because an open
  /// that fails leaves the Grid its retained cells and must never keep a tray
  /// or a marker for Photos that open no longer presents.
  const clearMultiSelection = () => {
    multiSelection = new Set();
    multiMissingIds = new Set();
    multiChangedIds = new Set();
    multiExpectedSelection = new Map();
    multiAnchorId = undefined;
    selectMode = false;
    gridBatchResult = undefined;
    batchAlbumCompensation = undefined;
    view.resetGridMultiSelection();
  };

  /// The shared refusal for a multi-selection that would pass the batch
  /// bound: the bound is named, and the selection is left exactly as it was.
  const refuseBeyondBatchBound = () => {
    setDecisionStatus(
      "Selection limit reached. Remove a Photo to extend the range.",
    );
  };

  /// Toggles one Photo's membership of the multi-selection and moves the
  /// anchor there, so a following shift-click extends from the last mark.
  const toggleMultiSelection = (photoId: string) => {
    if (multiSelection.delete(photoId)) {
      multiExpectedSelection.delete(photoId);
      multiMissingIds.delete(photoId);
      multiChangedIds.delete(photoId);
    } else {
      if (multiSelection.size >= MULTI_SELECTION_LIMIT) {
        refuseBeyondBatchBound();
        return;
      }
      const photoIndex = sourceGrid.findPhotoIndex(photoId);
      const photo =
        photoIndex === undefined ? undefined : sourceGrid.photoAt(photoIndex);
      if (!photo) return;
      multiSelection.add(photoId);
      multiMissingIds.delete(photoId);
      multiExpectedSelection.set(photoId, photo.selectionState);
    }
    gridBatchResult = undefined;
    multiAnchorId = photoId;
    renderGrid();
  };

  /// Extends the multi-selection over the loaded Photos between the anchor and
  /// the clicked Photo. A position the Grid has not loaded cannot join, and an
  /// anchor the Grid no longer holds makes the clicked Photo the new anchor.
  /// An extension that would pass the batch bound is refused whole, so the
  /// Grid never presents a selection its own batch would be refused for.
  const extendMultiSelection = (index: number, photoId: string) => {
    const anchorIndex =
      multiAnchorId === undefined
        ? undefined
        : sourceGrid.findPhotoIndex(multiAnchorId);
    if (anchorIndex === undefined) {
      if (multiSelection.size >= MULTI_SELECTION_LIMIT) {
        refuseBeyondBatchBound();
        return;
      }
      const photo = sourceGrid.photoAt(index);
      if (!photo) return;
      multiSelection.add(photoId);
      multiMissingIds.delete(photoId);
      multiChangedIds.delete(photoId);
      multiExpectedSelection.set(photoId, photo.selectionState);
      gridBatchResult = undefined;
      multiAnchorId = photoId;
      renderGrid();
      return;
    }
    const joined: string[] = [];
    for (
      let position = Math.min(anchorIndex, index);
      position <= Math.max(anchorIndex, index);
      position += 1
    ) {
      const photo = sourceGrid.photoAt(position);
      if (photo && !multiSelection.has(photo.id)) joined.push(photo.id);
    }
    if (multiSelection.size + joined.length > MULTI_SELECTION_LIMIT) {
      refuseBeyondBatchBound();
      return;
    }
    for (const photoIdToAdd of joined) {
      const indexToAdd = sourceGrid.findPhotoIndex(photoIdToAdd);
      const photo =
        indexToAdd === undefined ? undefined : sourceGrid.photoAt(indexToAdd);
      if (!photo) continue;
      multiSelection.add(photoIdToAdd);
      multiMissingIds.delete(photoIdToAdd);
      multiChangedIds.delete(photoIdToAdd);
      multiExpectedSelection.set(photoIdToAdd, photo.selectionState);
    }
    gridBatchResult = undefined;
    multiAnchorId = photoId;
    renderGrid();
  };

  const openPhoto = async (
    index: number,
    address: "push" | "replace" | "none" = "push",
  ): Promise<boolean> => {
    if (!canOpenGridPhoto()) return false;
    // The anchor is read from the Grid before anything re-keys its focus or
    // hides its layout, so the Grid entry records where the Photographer left.
    pendingGridRestoration = view.captureGridRestoration();
    const navigation = photoOwner.beginOpen(index);
    if (!navigation) return false;
    const photoTransition = recoveryGate.beginTransition(
      "photo",
      photoRecoveryKey(navigation.authority),
    );
    syncConnection();
    sourceGrid.stopGridWork();
    view.enterPhoto();
    let windowReady = true;
    try {
      if (!sourceGrid.photoAt(index))
        windowReady = await loadWindow(
          index,
          {
            kind: "photo",
            authority: navigation.windowAuthority,
          },
          true,
          "high",
          photoTransition,
          navigation.authority,
        );
      if (!photoOwner.isCurrent(navigation.authority) || !windowReady)
        return false;
      const current = photoOwner.commitOpen(navigation);
      if (!current) return false;
      // The Photo address is committed only after the Photo owner commits its
      // bounded facts, so a failed step creates no new Photo address and keeps
      // the existing Photo Retry target.
      recordPhotoAddress(current.id, address);
      const hasKnownPreview = renderPhotoShell(navigation.authority);
      updateControls();
      const previewRequest = showPreview(navigation.authority, photoTransition);
      const previewReady = hasKnownPreview || (await previewRequest);
      // A superseded open must not persist or touch controls afterwards: the
      // newer navigation persists its own position.
      if (!photoOwner.isCurrent(navigation.authority)) return false;
      const positionReady = await persistPosition(navigation.authority);
      if (
        previewReady &&
        positionReady &&
        photoOwner.isCurrent(navigation.authority)
      ) {
        recoveryGate.succeedTransition(photoTransition);
        setConnected(true);
      }
    } finally {
      // Back to Grid or a superseding view may end this request while an
      // unloaded boundary window is still loading. A committed navigation has
      // already cleared this gate; every other path abandons its pending target.
      photoOwner.cancelOpen(navigation.authority);
      pendingGridRestoration = undefined;
      updateControls();
    }
    return true;
  };
  const renderPhotoFacts = () => {
    const photo = currentPhoto();
    view.renderPhotoFacts({
      index: photoOwner.currentIndex,
      total: sourceGrid.total,
      originalFilename: photo?.originalFilename,
      selectionState: photo?.selectionState,
      rating: photo?.rating,
    });
    renderFilmstrip();
    updateControls();
  };

  const renderReviewImage = (url: string, authority = photoOwner.authority) => {
    const image = view.presentReviewImage(
      url,
      photoOwner.currentIndex,
      sourceGrid.total,
    );
    if (!image) return;
    photoOwner.attachReviewImage(
      authority,
      image.target,
      image.resolvedUrl,
      image.surface,
    );
  };

  /// The bounded neighbor radius of the filmstrip. The strip stays this
  /// bounded however large the source is, and its entries follow the open
  /// source's own order, so an Album order or a Selection State filter shows
  /// the neighbors the Photographer actually moves through.
  ///
  /// The strip presents the facts the current Photo's loaded window already
  /// holds and admits nothing itself: Photo View owns the UI while it is
  /// open, so it starts no window work for a hidden surface, and a neighbor
  /// outside the loaded window stays a placeholder until navigating to it
  /// loads that window through the normal Photo path.
  const FILMSTRIP_RADIUS = 2;
  const filmstripRange = (
    index: number,
  ): Readonly<{ first: number; last: number }> => ({
    first: Math.max(0, index - FILMSTRIP_RADIUS),
    last: Math.min(sourceGrid.total - 1, index + FILMSTRIP_RADIUS),
  });
  const renderFilmstrip = () => {
    if (!applicationAlive || view.gridVisible()) return;
    const index = photoOwner.currentIndex;
    if (!photoOwner.current || index < 0 || sourceGrid.total === 0) return;
    const { first, last } = filmstripRange(index);
    const cells = [];
    for (let position = first; position <= last; position += 1)
      cells.push({
        index: position,
        current: position === index,
        photo: sourceGrid.photoAt(position),
      });
    view.renderFilmstrip({
      total: sourceGrid.total,
      interactive: canOpenGridPhoto(),
      cells,
    });
  };

  type MembershipFacts =
    | Readonly<{ kind: "loading" }>
    | Readonly<{
        kind: "ready";
        albums: ReadonlyArray<Readonly<{ id: string; name: string }>>;
      }>
    | Readonly<{ kind: "failed" }>;
  let membershipFacts: MembershipFacts = { kind: "loading" };
  let membershipPhotoId: string | undefined;
  let membershipMessage: string | undefined;
  let membershipAbort: AbortController | undefined;
  let membershipRevision = 0;
  const membershipAlbumName = (albumId: string): string =>
    application.albums.find((album) => album.id === albumId)?.name ?? "Album";

  const renderMembershipControls = () => {
    if (!applicationAlive) return;
    const photo = currentPhoto();
    const photoId = photo?.id;
    const facts: MembershipFacts =
      photoId !== undefined && membershipPhotoId === photoId
        ? membershipFacts
        : { kind: "loading" };
    const containing = facts.kind === "ready" ? facts.albums : [];
    const memberIds = new Set(containing.map((album) => album.id));
    const pending = photoId
      ? application.albums
          .filter(
            (album) =>
              albumActions.isMembershipAdmitted("add", album.id, photoId) ||
              albumActions.isMembershipAdmitted("remove", album.id, photoId),
          )
          .map((album) => album.id)
      : [];
    view.renderMembership({
      photoPresent: Boolean(photo),
      loading: Boolean(photo) && facts.kind === "loading",
      failed: Boolean(photo) && facts.kind === "failed",
      ...(membershipMessage ? { message: membershipMessage } : {}),
      containing,
      options: application.albums.map((album) => ({
        id: album.id,
        name: album.name,
        member: memberIds.has(album.id),
      })),
      pendingAlbumIds: pending,
    });
  };

  /// Loads the current Photo's Album membership. The read is fenced to the
  /// Photo, to the generation, and to any membership toggle admitted while it
  /// runs: a response that arrives after its Photo stopped being current, or
  /// after a toggle took over the panel, is discarded. A revalidation keeps
  /// the stated facts while it runs so the panel does not flicker.
  const loadPhotoAlbums = async (
    authority: PhotoAuthority,
    photoId: string | undefined,
    options: Readonly<{ force?: boolean; keepFacts?: boolean }> = {},
  ): Promise<void> => {
    if (
      !options.force &&
      photoId !== undefined &&
      photoId === membershipPhotoId &&
      membershipFacts.kind !== "failed"
    )
      return;
    const showLoading = !options.keepFacts || photoId !== membershipPhotoId;
    membershipAbort?.abort();
    membershipAbort = undefined;
    membershipPhotoId = photoId;
    membershipMessage = undefined;
    membershipRevision += 1;
    const revision = membershipRevision;
    if (!photoId) {
      membershipFacts = { kind: "loading" };
      renderMembershipControls();
      return;
    }
    if (showLoading) {
      membershipFacts = { kind: "loading" };
      renderMembershipControls();
    }
    const controller = new AbortController();
    membershipAbort = controller;
    const result = await fetchPhotoAlbums(fetcher, photoId, controller.signal);
    if (
      controller.signal.aborted ||
      membershipRevision !== revision ||
      !photoOwner.isCurrent(authority) ||
      currentPhoto()?.id !== photoId
    )
      return;
    membershipFacts =
      result.kind === "ok"
        ? { kind: "ready", albums: result.value.albums }
        : { kind: "failed" };
    renderMembershipControls();
  };

  const refreshMembershipFacts = (): void => {
    if (!applicationAlive) return;
    const photo = currentPhoto();
    if (!photo) return;
    void loadPhotoAlbums(photoOwner.authority, photo.id, {
      force: true,
      keepFacts: true,
    });
  };

  /// Sends one admitted membership toggle for the current Photo. The
  /// checkbox shows the intended state while the mutation is in flight, and
  /// a failed mutation keeps the panel truthful and names the action.
  const toggleMembership = (albumId: string, member: boolean): void => {
    expireBatchAlbumCompensation();
    const photo = currentPhoto();
    if (!photo || !albumId) return;
    const photoId = photo.id;
    const kind = member ? "add" : "remove";
    if (albumActions.isMembershipAdmitted(kind, albumId, photoId)) return;
    const photoAuthority = photoOwner.authority;
    const revision = ++membershipRevision;
    const prior = membershipFacts;
    if (membershipFacts.kind === "ready") {
      const others = membershipFacts.albums.filter(
        (album) => album.id !== albumId,
      );
      membershipFacts = {
        kind: "ready",
        albums: member
          ? [...others, { id: albumId, name: membershipAlbumName(albumId) }]
          : others,
      };
    }
    membershipMessage = undefined;
    renderMembershipControls();
    void (async () => {
      const settlement = member
        ? mutateAlbum(
            (context) => albumActions.addMembership(albumId, photoId, context),
            "photo",
            photoAuthority,
          )
        : mutateAlbum(
            (context) =>
              albumActions.removeMembership(albumId, photoId, context),
            "photo",
            photoAuthority,
          );
      // The admission is registered now: show the checkbox as in flight.
      renderMembershipControls();
      const { ok, announce } = await settlement;
      const stillCurrent =
        photoOwner.isCurrent(photoAuthority) && currentPhoto()?.id === photoId;
      if (ok) {
        if (stillCurrent) {
          announce(
            member
              ? "Added to the Album."
              : albumId === sourceGrid.albumId
                ? "Removed from the Album. It stays in this open view until reopened."
                : "Removed from the Album.",
          );
          void loadPhotoAlbums(photoAuthority, photoId, {
            force: true,
            keepFacts: true,
          });
        }
        return;
      }
      // A failed toggle restores the prior true state. Facts that were still
      // loading are not a true state: they become a load failure that offers
      // the membership retry instead of stranding the panel on loading.
      if (stillCurrent && membershipRevision === revision)
        membershipFacts = prior.kind === "ready" ? prior : { kind: "failed" };
      if (!stillCurrent) return;
      membershipMessage = member
        ? `Could not add this Photo to “${membershipAlbumName(albumId)}”.`
        : `Could not remove this Photo from “${membershipAlbumName(albumId)}”.`;
      renderMembershipControls();
    })();
  };

  const loadPhotoMetadata = async (
    authority: PhotoAuthority,
    photoId: string | undefined,
  ): Promise<void> => {
    photoMetadataAbort?.abort();
    photoMetadataAbort = undefined;
    view.renderPhotoMetadata();
    if (!photoId) return;
    const controller = new AbortController();
    photoMetadataAbort = controller;
    const result = await fetchPhotoMetadata(
      fetcher,
      photoId,
      controller.signal,
    );
    if (
      controller.signal.aborted ||
      !photoOwner.isCurrent(authority) ||
      currentPhoto()?.id !== photoId
    )
      return;
    view.renderPhotoMetadata(result.kind === "ok" ? result.value : undefined);
  };

  const renderPhotoShell = (authority = photoOwner.authority): boolean => {
    const photo = currentPhoto();
    renderMembershipControls();
    const image = view.renderPhotoShell({
      sourceName: sourceGrid.name,
      index: photoOwner.currentIndex,
      total: sourceGrid.total,
      photoId: photo?.id,
      available: photo?.available,
      originalFilename: photo?.originalFilename,
      selectionState: photo?.selectionState,
      rating: photo?.rating,
      previewSource: photo?.preview.source,
      limitedDetail: photo?.preview.limitedDetail,
      previewUrl: photo?.preview.url,
    });
    void loadPhotoMetadata(authority, photo?.id);
    void loadPhotoAlbums(authority, photo?.id);
    if (image)
      photoOwner.attachReviewImage(
        authority,
        image.target,
        image.resolvedUrl,
        image.surface,
      );
    renderFilmstrip();
    updateControls();
    return Boolean(image);
  };

  const showPreview = async (
    authority: PhotoAuthority,
    transition?: RecoveryTransition,
  ): Promise<boolean> => {
    const capturedStatus = view.photoStatusSurface;
    const outcome = await photoOwner.loadCurrentPreview(authority);
    if (outcome.kind === "detached") return false;
    if (outcome.kind === "failed") {
      if (view.isPhotoStatusSurfaceCurrent(capturedStatus))
        view.setPhotoStatus("Connection lost. Retry to refresh this Photo.");
      failPhotoRecovery(authority, "preview", transition);
      return false;
    }
    if (!photoOwner.isCurrent(authority)) return false;
    if (outcome.kind === "not-ready") {
      if (view.isPhotoStatusSurfaceCurrent(capturedStatus))
        view.setPhotoStatus(outcome.preview.message ?? "Preview unavailable");
      view.showPreviewUnavailable("Preview unavailable");
      return true;
    }
    const result = outcome.preview;
    if (!view.reviewImageMatches(result.url))
      renderReviewImage(result.url, authority);
    view.setPreviewFacts(result.source, Boolean(result.limitedDetail));
    if (view.isPhotoStatusSurfaceCurrent(capturedStatus))
      view.setPhotoStatus(
        result.stale ? (result.message ?? "Showing a stale Preview.") : "",
      );
    updateControls();
    void prefetchAdjacent(photoOwner.currentIndex - 1, authority);
    void prefetchAdjacent(photoOwner.currentIndex + 1, authority);
    return true;
  };
  const prefetchAdjacent = async (index: number, authority: PhotoAuthority) => {
    if (!photoOwner.isCurrent(authority)) return;
    if (index < 0 || index >= sourceGrid.total) return;
    let photo = sourceGrid.photoAt(index);
    if (!photo) {
      const windowAuthority = photoOwner.windowAuthority;
      if (!windowAuthority) return;
      await loadWindow(
        index,
        {
          kind: "photo",
          authority: windowAuthority,
        },
        true,
        "low",
        undefined,
        authority,
      );
      if (!photoOwner.isCurrent(authority)) return;
      // A window an awaited caller loaded settles through that caller, so the
      // strip asks for its own render here instead of waiting for a
      // window-settled notification awaited windows never raise: the neighbor
      // it just loaded stops being a placeholder now.
      renderFilmstrip();
      photo = sourceGrid.photoAt(index);
    }
    if (!photo || !photo.available) return;
    await photoOwner.prefetchAdjacent(authority, index);
  };
  /// Releases Photo View's ownership of the current Photo and re-keys the
  /// recovery gate exactly as returning to the Grid does, so a pending
  /// transfer cannot paint a Photo the page has left.
  const leavePhotoView = () => {
    photoMetadataAbort?.abort();
    photoMetadataAbort = undefined;
    const authority = photoOwner.leave();
    const photoTransition = recoveryGate.beginTransition(
      "photo",
      photoRecoveryKey(authority),
    );
    recoveryGate.succeedTransition(photoTransition);
    setConnected(true);
  };
  const showGrid = () => {
    leavePhotoView();
    cancelScheduledGridRender();
    const gridAuthority = sourceGrid.authority;
    const gridPosition = sourceGrid.readGridPosition(gridAuthority);
    view.showGrid(gridPosition);
    renderGrid(undefined);
    presentRangeStatus();
    updateControls();
  };
  const persistPosition = (
    photoAuthority = photoOwner.authority,
  ): Promise<boolean> => {
    if (sourceGrid.kind !== "album" || !sourceGrid.albumId || !currentPhoto())
      return Promise.resolve(true);
    const albumId = sourceGrid.albumId;
    const albumSummaryAuthority = application.albumSummaryAuthority(albumId);
    const photoId = currentPhoto()!.id;
    const sourceAuthority = sourceGrid.authority;
    const admission = savedPositions.save({
      sourceAuthority,
      photoAuthority,
      albumId,
      photoId,
    });
    if (!admission) return Promise.resolve(false);
    return admission.settlement.then((outcome) => {
      if (
        outcome.kind === "skipped" ||
        outcome.kind === "detached" ||
        outcome.kind === "stale" ||
        !applicationAlive ||
        !savedPositions.isCurrent(outcome.target)
      )
        return false;
      if (outcome.kind === "failed") {
        view.setPhotoStatus(
          "Album position could not be saved. Retry before making more decisions.",
        );
        failPhotoRecovery(
          outcome.target.photoAuthority,
          "saved-position",
          undefined,
          outcome.transportLost,
        );
        return false;
      }
      if (
        application.confirmSavedPosition(
          outcome.target.albumId,
          albumSummaryAuthority,
        )
      )
        renderSources();
      return true;
    });
  };
  const moveTo = async (target: number) => {
    if (
      pageBusy ||
      photoOwner.busy ||
      photoOwner.opening ||
      target < 0 ||
      target >= sourceGrid.total
    )
      return;
    await openPhoto(target, "replace");
  };
  const mutate = async (
    field: "selectionState" | "rating",
    value: SelectionState | number,
    advance: boolean,
  ) => {
    if (!connected || pageBusy) return;
    const admission = photoOwner.mutate(field, value, advance);
    if (!admission) return;
    view.setPhotoStatus(
      `Saving ${field === "rating" ? "Rating" : "Selection State"}…`,
    );
    updateControls();
    const outcome = await admission.settlement;
    if (outcome.kind === "detached") return;
    if (outcome.kind === "failed") {
      if (outcome.failure === "answered") {
        view.setPhotoStatus(
          outcome.status === 409
            ? "The Photo changed elsewhere. Retry to refresh its current state."
            : "The change could not be saved.",
        );
      } else {
        view.setPhotoStatus(
          "Connection lost before the change was confirmed. Retry to refresh.",
        );
      }
      if (outcome.connectivity === "lost")
        failPhotoRecovery(outcome.authority, "photo-write");
      updateControls();
      return;
    }
    if (!outcome.applied) return;
    if (
      field === "selectionState" &&
      outcome.photoId &&
      multiSelection.has(outcome.photoId)
    )
      multiExpectedSelection.set(outcome.photoId, value as SelectionState);
    view.setPhotoStatus(
      `${field === "rating" ? "Rating" : "Selection"} saved.`,
    );
    if (outcome.advance) await moveTo(outcome.index + 1);
    else renderPhotoFacts();
    updateControls();
  };
  /// Applies one Grid keyboard decision or Rating to the focused Photo
  /// without opening Photo View. The write shares the Photo View admission,
  /// the one-level Undo, and every failure rule; nothing advances, so the
  /// Photographer keeps the focused cell and moves it with the arrow keys.
  const mutateGridPhoto = async (
    index: number,
    field: "selectionState" | "rating",
    value: SelectionState | number,
  ) => {
    if (!connected || pageBusy || !view.gridVisible() || !canOpenGridPhoto())
      return;
    const admission = photoOwner.mutateAt(index, field, value);
    if (!admission) return;
    updateControls();
    const outcome = await admission.settlement;
    // The write settled, so the Grid is interactive again whatever the
    // outcome; the merged render re-enables the cells, rebuilds the decided
    // cell in place, and returns focus to it. A detached write stays silent.
    renderGrid();
    if (outcome.kind === "detached") return;
    if (outcome.kind === "persisted" && field === "selectionState") {
      const photoId = sourceGrid.photoAt(index)?.id;
      if (photoId && multiSelection.has(photoId))
        multiExpectedSelection.set(photoId, value as SelectionState);
    }
    if (outcome.kind === "failed") {
      if (outcome.failure === "answered") {
        setDecisionStatus(
          outcome.status === 409
            ? "The Photo changed elsewhere. Open it to confirm its current state."
            : "The change could not be saved.",
        );
      } else {
        setDecisionStatus("Connection lost before the change was confirmed.");
      }
      if (outcome.connectivity === "lost")
        failPhotoRecovery(outcome.authority, "photo-write");
      updateControls();
    }
  };
  /// Applies one Selection State to every multi-selected Photo as one bounded
  /// change. The write shares the Photo View and Grid admission, the
  /// one-level Undo, and every failure rule; the multi-selection stays, so the
  /// Photographer can decide again or add the same Photos to an Album.
  const mutateGridBatch = async (value: SelectionState) => {
    if (!connected || pageBusy || !view.gridVisible() || !canOpenGridPhoto())
      return;
    const photoIds = [...multiSelection].filter(
      (photoId) => !multiMissingIds.has(photoId),
    );
    if (photoIds.length === 0) {
      const message =
        "No selected Photos remain in this Library. Clear the selection to continue.";
      gridBatchResult = { tone: "warning", message };
      setDecisionStatus(message);
      renderGrid();
      return;
    }
    const photos = photoIds.flatMap((photoId) => {
      const expectedCurrent = multiExpectedSelection.get(photoId);
      return expectedCurrent === undefined
        ? []
        : [{ photoId, expectedCurrent }];
    });
    if (photos.length !== photoIds.length) {
      const message =
        "The selected Photos need a refresh before this batch can be retried.";
      gridBatchResult = { tone: "failure", message };
      setDecisionStatus(message);
      renderGrid();
      return;
    }
    const admission = photoOwner.mutateBatch(photos, value);
    if (!admission) return;
    gridBatchResult = undefined;
    renderGrid();
    setDecisionStatus(`Saving ${photoCountText(photoIds.length)}…`);
    const outcome = await admission.settlement;
    // The write settled, so the Grid is interactive again whatever the
    // outcome; the merged render re-enables the tray and rebuilds the decided
    // cells in place. A detached write stays silent.
    renderGrid();
    if (outcome.kind === "detached") return;
    if (outcome.kind === "failed") {
      if (outcome.failure === "answered") {
        setDecisionStatus(
          // Only an over-limit batch answers 400; the client caps the
          // selection, so the bound clause stays off every other answered
          // failure it cannot have caused.
          outcome.status === 400
            ? `The change could not be saved. A batch holds up to ${MULTI_SELECTION_LIMIT} Photos.`
            : "The change could not be saved.",
        );
      } else {
        setDecisionStatus(
          "Connection lost before the change was confirmed. Retry to refresh.",
        );
      }
      if (outcome.connectivity === "lost")
        failPhotoRecovery(photoOwner.authority, "photo-write");
      gridBatchResult = {
        tone: "failure",
        message:
          outcome.failure === "transport"
            ? "Connection lost before the batch was confirmed. Retry to refresh."
            : "The batch could not be saved. Retry to refresh the selected Photos.",
      };
      renderGrid();
      updateControls();
      return;
    }
    const applied = outcome.applied.length;
    const changed = outcome.changedElsewhere.length;
    const missing = outcome.missing.length;
    multiChangedIds = new Set(
      outcome.changedElsewhere.map((entry) => entry.photoId),
    );
    for (const entry of outcome.missing) {
      multiMissingIds.add(entry.photoId);
      multiExpectedSelection.delete(entry.photoId);
    }
    const decision = value === "selected" ? "selected" : "rejected";
    const resumeMessage =
      sourceGrid.kind === "album" ? " Album resume point unchanged." : "";
    for (const entry of outcome.applied)
      multiExpectedSelection.set(entry.photoId, value);
    const parts = [`${photoCountText(applied)} ${decision}.`];
    if (changed > 0)
      parts.push(
        `${photoCountText(changed)} changed elsewhere. Review them before retrying.`,
      );
    if (missing > 0)
      parts.push(`${photoCountText(missing)} no longer in this Library.`);
    const message = `${parts.join(" ")}${resumeMessage}`;
    gridBatchResult = {
      tone: changed > 0 || missing > 0 ? "warning" : "success",
      message,
      ...(changed > 0
        ? { review: { label: `Review ${photoCountText(changed)}` } }
        : {}),
    };
    setDecisionStatus(message);
    renderGrid();
    updateControls();
  };
  /// Refreshes the bounded facts for Photos that the server reported as
  /// changed elsewhere. The open Browse Snapshot and the page-owned selection
  /// stay in place; only the windows containing the reviewed identities are
  /// reloaded. Missing identities become non-retryable retained selections.
  const reviewChangedPhotos = async () => {
    if (
      !applicationAlive ||
      !connected ||
      pageBusy ||
      !view.gridVisible() ||
      multiChangedIds.size === 0
    )
      return;
    const reviewIds = [...multiChangedIds].filter(
      (photoId) => multiSelection.has(photoId) && !multiMissingIds.has(photoId),
    );
    if (reviewIds.length === 0) return;
    const sourceAuthority = sourceGrid.authority;
    const loadedWindows = new Set<number>();
    const reviewed = new Set<string>();
    const becameMissing = new Set<string>();
    let firstReviewedIndex: number | undefined;
    let failed = false;
    pageBusy = true;
    renderGrid();
    setDecisionStatus(
      `Reviewing ${photoCountText(reviewIds.length)} changed elsewhere…`,
    );
    try {
      for (const photoId of reviewIds) {
        if (!sourceGrid.isCurrent(sourceAuthority)) return;
        let index = sourceGrid.findPhotoIndex(photoId);
        if (index === undefined) {
          const position = await sourceGrid.resolvePhotoPosition(
            sourceAuthority,
            photoId,
          );
          if (!sourceGrid.isCurrent(sourceAuthority)) return;
          if (position.kind === "missing") {
            becameMissing.add(photoId);
            continue;
          }
          if (position.kind === "expired") {
            await reopenExpired(
              sourceGrid.readGridPosition(sourceAuthority) ?? 0,
              sourceGrid.generation,
            );
            return;
          }
          if (position.kind !== "resolved") {
            failed = true;
            break;
          }
          index = position.position;
        }
        const believedState = multiExpectedSelection.get(photoId);
        const { start } = sourceGrid.describeWindow(index);
        const refreshWindow = async () => {
          sourceGrid.invalidateWindow(index);
          const loaded = await loadWindow(
            index,
            { kind: "grid", authority: sourceAuthority },
            true,
            "high",
          );
          if (!loaded || !sourceGrid.isCurrent(sourceAuthority)) return false;
          loadedWindows.add(start);
          return true;
        };
        if (!loadedWindows.has(start) && !(await refreshWindow())) {
          failed = true;
          break;
        }
        let refreshedIndex = sourceGrid.findPhotoIndex(photoId);
        if (refreshedIndex === undefined && !(await refreshWindow())) {
          failed = true;
          break;
        }
        refreshedIndex = sourceGrid.findPhotoIndex(photoId);
        let photo =
          refreshedIndex === undefined
            ? undefined
            : sourceGrid.photoAt(refreshedIndex);
        if (!photo) {
          // A refreshed window can lose a retained fact to bounded-cache
          // pressure. Ask the position authority before deciding that the
          // Photo left the Library; only its `missing` answer is definitive.
          const currentPosition = await sourceGrid.resolvePhotoPosition(
            sourceAuthority,
            photoId,
          );
          if (!sourceGrid.isCurrent(sourceAuthority)) return;
          if (currentPosition.kind === "missing") {
            becameMissing.add(photoId);
            continue;
          }
          if (currentPosition.kind === "expired") {
            await reopenExpired(
              sourceGrid.readGridPosition(sourceAuthority) ?? 0,
              sourceGrid.generation,
            );
            return;
          }
          if (currentPosition.kind !== "resolved") {
            failed = true;
            break;
          }
          if (!(await refreshWindow())) {
            failed = true;
            break;
          }
          refreshedIndex = sourceGrid.findPhotoIndex(photoId);
          photo =
            refreshedIndex === undefined
              ? undefined
              : sourceGrid.photoAt(refreshedIndex);
          // The position route proved that the Photo still exists. If the
          // second refresh cannot retain it, keep it retryable rather than
          // presenting a false deletion.
          if (!photo) {
            failed = true;
            break;
          }
        }
        if (
          believedState !== undefined &&
          !sourceGrid.reconcilePhotoSelection(
            sourceAuthority,
            refreshedIndex!,
            photoId,
            believedState,
            photo.selectionState,
          )
        ) {
          failed = true;
          break;
        }
        multiExpectedSelection.set(photoId, photo.selectionState);
        reviewed.add(photoId);
        firstReviewedIndex ??= refreshedIndex;
      }
    } finally {
      if (sourceGrid.isCurrent(sourceAuthority)) {
        pageBusy = false;
        updateControls();
      }
    }
    if (!applicationAlive || !sourceGrid.isCurrent(sourceAuthority)) return;
    for (const photoId of becameMissing) {
      multiMissingIds.add(photoId);
      multiExpectedSelection.delete(photoId);
      multiChangedIds.delete(photoId);
    }
    for (const photoId of reviewed) multiChangedIds.delete(photoId);
    const remaining = [...multiChangedIds].filter(
      (photoId) => multiSelection.has(photoId) && !multiMissingIds.has(photoId),
    );
    if (failed) {
      const missingMessage =
        becameMissing.size > 0
          ? ` ${photoCountText(becameMissing.size)} no longer in this Library.`
          : "";
      const message = `Some changed Photos could not be refreshed.${missingMessage} Retry Review to continue.`;
      gridBatchResult = {
        tone: "warning",
        message,
        ...(remaining.length > 0
          ? {
              review: {
                label: `Review ${photoCountText(remaining.length)}`,
              },
            }
          : {}),
      };
      setDecisionStatus(message);
    } else {
      const reviewedCount = reviewed.size;
      const missingMessage =
        becameMissing.size > 0
          ? ` ${photoCountText(becameMissing.size)} no longer in this Library.`
          : "";
      const message = `${photoCountText(reviewedCount)} reviewed.${missingMessage} Retry the batch when ready.`;
      gridBatchResult = {
        tone: becameMissing.size > 0 ? "warning" : "success",
        message,
      };
      setDecisionStatus(message);
    }
    renderGrid(firstReviewedIndex);
    if (firstReviewedIndex !== undefined)
      view.focusGridIndex(firstReviewedIndex);
    updateControls();
  };

  /// Adds every multi-selected Photo to one Album through the bounded
  /// membership route. Membership stays outside the Undo contract, and the
  /// multi-selection stays so the same Photos can join another Album.
  const batchAddToAlbum = async (albumId: string) => {
    expireBatchAlbumCompensation();
    if (!applicationAlive || batchAlbumPending || !albumId) return;
    if (multiSelection.size === 0) return;
    if (!application.albums.some((album) => album.id === albumId)) return;
    const photoIds = [...multiSelection].filter(
      (photoId) => !multiMissingIds.has(photoId),
    );
    if (photoIds.length === 0) {
      const message =
        "No selected Photos remain in this Library. Clear the selection to continue.";
      gridBatchResult = { tone: "warning", message };
      setDecisionStatus(message);
      renderGrid();
      return;
    }
    const sourceAuthority = sourceGrid.authority;
    const albumBefore = application.albums.find(
      (album) => album.id === albumId,
    );
    const name = membershipAlbumName(albumId);
    batchAlbumPending = true;
    gridBatchResult = undefined;
    renderGrid();
    renderBatchAlbums();
    setDecisionStatus(
      `Adding ${photoCountText(photoIds.length)} to “${name}”…`,
    );
    const result = await mutateAlbum(
      (context) => albumActions.addMemberships(albumId, photoIds, context),
      "summary",
    );
    batchAlbumPending = false;
    if (!applicationAlive) return;
    renderBatchAlbums();
    // A superseded or already admitted batch reports nothing: the action that
    // owns the settlement presents its own outcome.
    if (
      !result.admitted ||
      !result.latest ||
      !sourceGrid.isCurrent(sourceAuthority)
    )
      return;
    let message: string;
    let compensation: GridBatchResult["compensation"];
    if (result.ok && result.membershipAdd) {
      const added = result.membershipAdd.addedPhotoIds.length;
      const alreadyMember = result.membershipAdd.alreadyMemberPhotoIds.length;
      const parts = [
        ...(added > 0 ? [`${photoCountText(added)} added to “${name}”.`] : []),
        ...(alreadyMember > 0
          ? [`${photoCountText(alreadyMember)} already in “${name}”.`]
          : []),
      ];
      if (sourceGrid.kind === "album")
        parts.push("Album resume point unchanged.");
      message = parts.join(" ");
      if (added > 0) {
        batchAlbumCompensation = Object.freeze({
          albumId,
          albumName: name,
          photoIds: Object.freeze([...result.membershipAdd.addedPhotoIds]),
          sourceAuthority,
          hadSavedPosition: albumBefore?.hasSavedPosition ?? false,
        });
        compensation = { label: "Remove added Photos" };
      } else batchAlbumCompensation = undefined;
    } else {
      batchAlbumCompensation = undefined;
      message = result.ok
        ? `${photoCountText(photoIds.length)} added to “${name}”.${
            sourceGrid.kind === "album" ? " Album resume point unchanged." : ""
          }`
        : `Could not add the selected Photos to “${name}”.`;
    }
    gridBatchResult = {
      tone: result.ok ? "success" : "failure",
      message,
      ...(compensation ? { compensation } : {}),
    };
    setDecisionStatus(message);
    renderGrid();
  };
  /// Removes only the identities returned as newly added by the current batch
  /// Album operation. This is a scoped compensation, not the global decision
  /// Undo, and a failed settlement keeps the exact bounded record retryable.
  const removeAddedPhotosFromAlbum = async () => {
    const compensation = batchAlbumCompensation;
    if (
      !compensation ||
      batchAlbumPending ||
      !applicationAlive ||
      !sourceGrid.isCurrent(compensation.sourceAuthority)
    )
      return;
    const savedPositionBefore =
      application.albums.find((album) => album.id === compensation.albumId)
        ?.hasSavedPosition ?? compensation.hadSavedPosition;
    batchAlbumPending = true;
    renderGrid();
    renderBatchAlbums();
    setDecisionStatus(
      `Removing ${photoCountText(compensation.photoIds.length)} added Photos from “${compensation.albumName}”…`,
    );
    const result = await mutateAlbum(
      (context) =>
        albumActions.removeAddedMemberships(
          compensation.albumId,
          compensation.photoIds,
          context,
        ),
      "summary",
    );
    batchAlbumPending = false;
    if (!applicationAlive) return;
    renderBatchAlbums();
    if (
      !result.admitted ||
      !result.latest ||
      batchAlbumCompensation !== compensation ||
      !sourceGrid.isCurrent(compensation.sourceAuthority)
    ) {
      renderGrid();
      return;
    }
    if (!result.ok || !result.membershipRemove) {
      const message = `Could not remove the added Photos from “${compensation.albumName}”. Retry to continue.`;
      gridBatchResult = {
        tone: "failure",
        message,
        compensation: { label: "Remove added Photos" },
      };
      setDecisionStatus(message);
      renderGrid();
      return;
    }
    const removed = result.membershipRemove.removedPhotoIds.length;
    const absent = result.membershipRemove.alreadyAbsentPhotoIds.length;
    const currentAlbum = application.albums.find(
      (album) => album.id === compensation.albumId,
    );
    const savedPositionMessage =
      savedPositionBefore && !currentAlbum?.hasSavedPosition
        ? " Album resume point cleared."
        : savedPositionBefore && currentAlbum?.hasSavedPosition
          ? " Album resume point remains."
          : "";
    const message = [
      `${photoCountText(removed)} removed from “${compensation.albumName}”.`,
      ...(absent > 0
        ? [
            `${photoCountText(absent)} already absent from “${compensation.albumName}”.`,
          ]
        : []),
      savedPositionMessage.trim(),
    ]
      .filter(Boolean)
      .join(" ");
    batchAlbumCompensation = undefined;
    gridBatchResult = {
      tone: absent > 0 ? "warning" : "success",
      message,
    };
    setDecisionStatus(message);
    renderGrid();
  };

  /// Restores every Photo one batch Selection State change confirmed. The
  /// writes are the same compare-and-set writes a single Undo sends, one
  /// Photo at a time, and the Grid stays where it is: a batch never opened a
  /// Photo, so nothing navigates.
  const performBatchUndo = async () => {
    const preparation = photoOwner.prepareBatchUndo();
    if (!preparation) return;
    updateControls();
    setDecisionStatus(`Restoring ${photoCountText(preparation.count)}…`);
    const outcome = await photoOwner.performBatchUndo(preparation);
    renderGrid();
    updateControls();
    if (outcome.kind === "detached") return;
    if (outcome.connectivity === "lost")
      failPhotoRecovery(photoOwner.authority, "undo");
    const restored = outcome.restored.length;
    const conflicts = outcome.conflicts.length;
    const failed = outcome.failed.length;
    for (const entry of outcome.restoredValues) {
      if (multiSelection.has(entry.photoId))
        multiExpectedSelection.set(entry.photoId, entry.value);
    }
    const message =
      failed > 0
        ? `${photoCountText(restored)} restored. ${photoCountText(failed)} not restored; Undo again to retry.`
        : conflicts > 0
          ? `${photoCountText(restored)} restored. ${photoCountText(conflicts)} could not be restored because ${conflicts === 1 ? "it changed" : "they changed"} elsewhere.`
          : `${photoCountText(restored)} restored.`;
    gridBatchResult = {
      tone: failed > 0 || conflicts > 0 ? "warning" : "success",
      message,
    };
    setDecisionStatus(message);
    renderGrid();
  };
  const performUndo = async () => {
    if (!connected || pageBusy) return;
    // One Undo control covers the one-level change: a pending batch restores
    // in place, and a pending single decision keeps its own path.
    if (photoOwner.undoBatch) {
      await performBatchUndo();
      return;
    }
    const targetPhotoId = photoOwner.undoPhotoId;
    if (!targetPhotoId) return;
    const sourceAuthority = sourceGrid.authority;
    let targetIndex = sourceGrid.findPhotoIndex(targetPhotoId);
    if (targetIndex === undefined) {
      pageBusy = true;
      updateControls();
      const resolution = await sourceGrid.resolvePhotoPosition(
        sourceAuthority,
        targetPhotoId,
      );
      if (sourceGrid.isCurrent(sourceAuthority)) pageBusy = false;
      if (
        !sourceGrid.isCurrent(sourceAuthority) ||
        photoOwner.sourceAuthority !== sourceAuthority
      ) {
        updateControls();
        return;
      }
      if (resolution.kind === "detached") {
        updateControls();
        return;
      }
      if (resolution.kind === "expired") {
        // A position lookup is bound to the opaque Snapshot. Reuse the
        // existing source-reopen recovery so the replacement Snapshot can be
        // opened with the current Photo as its anchor while retaining the
        // stable-identity Undo description.
        await reopenExpired(photoOwner.currentIndex, sourceGrid.generation);
        return;
      }
      if (resolution.kind === "missing") {
        photoOwner.discardUndo();
        setDecisionStatus(
          "Undo is no longer available because that Photo is no longer in this source.",
        );
        updateControls();
        return;
      }
      if (resolution.kind === "failed") {
        setDecisionStatus(
          resolution.transportLost
            ? "Connection lost while locating the Photo for Undo. Retry to refresh."
            : resolution.malformed
              ? "Undo target lookup returned an invalid response. Try Undo again."
              : `Undo target could not be located (HTTP ${resolution.status}). Try Undo again.`,
        );
        if (resolution.transportLost)
          failPhotoRecovery(photoOwner.authority, "undo-target");
        updateControls();
        return;
      }
      targetIndex = resolution.position;
    }
    const preparation = photoOwner.prepareUndo(targetIndex);
    if (!preparation) return;
    // A Grid decision never advanced, so Undo restores that cell in place and
    // returns the Grid keyboard to the affected Photo instead of opening it.
    const gridUndo = view.gridVisible() && !photoOwner.undoAdvanced;
    updateControls();
    if (preparation.needsWindow) {
      setDecisionStatus("Loading Photo for Undo…");
      const windowReady = await loadWindow(
        preparation.index,
        { kind: "photo", authority: preparation.windowAuthority },
        true,
        "high",
        undefined,
        preparation.authority,
      );
      if (!windowReady) {
        photoOwner.cancelUndo(preparation);
        updateControls();
        return;
      }
    }
    const outcome = await photoOwner.performUndo(preparation);
    if (outcome.kind === "detached") return;
    if (outcome.kind === "persisted" && outcome.photoId) {
      const restored = sourceGrid.photoAt(outcome.index);
      if (restored && multiSelection.has(outcome.photoId))
        multiExpectedSelection.set(outcome.photoId, restored.selectionState);
    }
    if (outcome.kind === "failed") {
      if (outcome.failure === "transport") {
        setDecisionStatus("Connection lost before Undo was confirmed.");
      } else if (outcome.status === 409) {
        setDecisionStatus(
          "Undo is no longer available because the Photo changed elsewhere. Retry to refresh its current state.",
        );
      } else {
        setDecisionStatus("Undo could not be saved. Try Undo again.");
      }
      if (outcome.connectivity === "lost")
        failPhotoRecovery(outcome.authority, "undo");
      updateControls();
      return;
    }
    if (gridUndo) {
      // The Grid owns this surface: release Photo View's ownership and re-key
      // the recovery gate exactly as returning to the Grid does, so recovery
      // routing and a source reopen keep the Grid.
      const gridAuthority = photoOwner.leave();
      const gridTransition = recoveryGate.beginTransition(
        "photo",
        photoRecoveryKey(gridAuthority),
      );
      recoveryGate.succeedTransition(gridTransition);
      syncConnection();
      updateControls();
      renderGrid();
      view.focusGridIndex(outcome.index);
      setGridStatusText("Last change undone.");
      return;
    }
    view.enterPhoto();
    const photoTransition = recoveryGate.beginTransition(
      "photo",
      photoRecoveryKey(outcome.authority),
    );
    syncConnection();
    renderPhotoShell(outcome.authority);
    // Undo-driven Photo return replaces the current Photo destination, keeping
    // its parent Grid relationship.
    if (outcome.photoId) recordPhotoAddress(outcome.photoId, "replace");
    updateControls();
    const refreshed = await showPreview(outcome.authority, photoTransition);
    if (
      !refreshed ||
      !photoOwner.isCurrent(outcome.authority) ||
      currentPhoto()?.id !== outcome.photoId
    )
      return;
    const persisted = await persistPosition(outcome.authority);
    if (
      persisted &&
      photoOwner.isCurrent(outcome.authority) &&
      currentPhoto()?.id === outcome.photoId
    ) {
      recoveryGate.succeedTransition(photoTransition);
      setConnected(true);
      view.setPhotoStatus("Last change undone.");
    }
    updateControls();
  };

  const refreshSource = async (): Promise<void> => {
    // A Folder reopen needs the File Location binding: never send a
    // publicationless browse (it can only fail as expired/invalid).
    if (sourceGrid.kind === "folder" && !fileLocations.publication) {
      await awaitRootBinding();
      if (!fileLocations.publication) {
        setGridStatusText("Could not load this source. Retry to continue.");
        return;
      }
    }
    if (sourceGrid.kind === "album") {
      const album = application.albums.find(
        (candidate) => candidate.id === sourceGrid.albumId,
      );
      if (album)
        await openSource(
          "album",
          album,
          undefined,
          undefined,
          sourceGrid.order,
          sourceGrid.selection,
          { address: "replace" },
        );
      return;
    }
    await openSourceDescriptor(
      sourceGrid.source,
      undefined,
      sourceGrid.order,
      sourceGrid.selection,
      { address: "replace" },
    );
  };

  /// The Grid index a retry replays a failed window with. `loadWindow` and
  /// `invalidateWindow` align an index back to its window, and the clamped tail
  /// window starts before the first index that aligns to it (a 70-Photo range
  /// requests [10, 70) while index 10 still aligns to [0, 60)), so the anchor
  /// is the first index the Grid can report for that window.
  const windowAnchorIndex = (windowStart: number): number => {
    if (sourceGrid.alignedStart(windowStart) === windowStart)
      return windowStart;
    for (let index = windowStart + 1; index < sourceGrid.total; index += 1)
      if (sourceGrid.alignedStart(index) === windowStart) return index;
    return windowStart;
  };
  const currentSourceRangeRetries = (
    alignedStart?: number,
  ): Array<Readonly<{ claim: RecoveryClaim; retry: GridRangeRetry }>> => {
    const retries: Array<
      Readonly<{ claim: RecoveryClaim; retry: GridRangeRetry }>
    > = [];
    for (const failure of browseRangeFailures.values()) {
      if (
        failure.ownerScope !== "source" ||
        !failure.retry ||
        !recoveryGate.isActive(failure.claim) ||
        !sourceGrid.isCurrent(failure.retry.sourceAuthority) ||
        (alignedStart !== undefined && failure.retry.start !== alignedStart)
      )
        continue;
      retries.push({ claim: failure.claim, retry: failure.retry });
    }
    return retries;
  };
  const retryCurrentSourceRanges = async (
    rangeRetries: ReadonlyArray<
      Readonly<{ claim: RecoveryClaim; retry: GridRangeRetry }>
    >,
  ): Promise<boolean> => {
    let recovered = true;
    for (const { claim, retry: range } of rangeRetries) {
      if (
        !sourceGrid.isCurrent(range.sourceAuthority) ||
        !recoveryGate.isActive(claim)
      ) {
        recovered = false;
        continue;
      }
      sourceGrid.invalidateWindow(range.anchorIndex);
      const loaded = await loadWindow(
        range.anchorIndex,
        {
          kind: range.operationKind,
          authority: range.sourceAuthority,
        },
        range.quiet,
        range.priority,
      );
      if (!loaded || recoveryGate.isActive(claim)) recovered = false;
    }
    return recovered;
  };
  const retrySource = (): void => {
    const rangeRetries = currentSourceRangeRetries();
    if (rangeRetries.length > 0) {
      const retryAuthority = sourceGrid.authority;
      void (async () => {
        pageBusy = true;
        updateControls();
        try {
          await retryCurrentSourceRanges(rangeRetries);
        } finally {
          if (sourceGrid.isCurrent(retryAuthority)) {
            pageBusy = false;
            updateControls();
          }
        }
      })();
      return;
    }
    // A traversal whose bounded lookup failed keeps its destination retryable,
    // so Retry re-establishes that destination rather than reloading the
    // Overview.
    const traversal = retryableTraversal;
    if (traversal) {
      retryableTraversal = undefined;
      void establishDestination(traversal, { addressed: true });
      return;
    }
    if (!sourceGrid.retryRequired) {
      void application.loadOverview();
      return;
    }
    const remembered = sourceGrid.lastSource ?? ({ kind: "library" } as const);
    void (async () => {
      if (remembered.kind === "folder") {
        resetFileLocations();
        const bound = await awaitRootBinding();
        if (!bound) {
          setGridStatusText("Could not load this source. Retry to continue.");
          return;
        }
      }
      await openSourceDescriptor(
        remembered,
        undefined,
        sourceGrid.order,
        sourceGrid.selection,
        { address: "replace" },
      );
    })();
  };

  const retryCurrentPhoto = (): void => {
    if (pageBusy || photoRetryPending || photoOwner.busy || photoOwner.opening)
      return;
    void (async () => {
      const retry = photoOwner.beginRetry();
      if (!retry) return;
      photoRetryPending = true;
      view.setPhotoStatus("Reconnecting…");
      let retryStatusSurface = view.photoStatusSurface;
      const photoTransition = recoveryGate.beginTransition(
        "photo",
        photoRecoveryKey(retry.authority),
      );
      syncConnection();
      updateControls();
      try {
        const start = sourceGrid.alignedStart(retry.index);
        const sourceRangeRetries = currentSourceRangeRetries(start);
        const sourceRangeRecovered = sourceRangeRetries.length > 0;
        if (
          sourceRangeRecovered &&
          !(await retryCurrentSourceRanges(sourceRangeRetries))
        )
          return;
        if (
          !sourceGrid.isCurrent(retry.sourceAuthority) ||
          !photoOwner.retryIsCurrent(retry)
        )
          return;
        if (!sourceRangeRecovered) sourceGrid.invalidateWindow(retry.index);
        const windowReady = await loadWindow(
          retry.index,
          { kind: "photo", authority: retry.windowAuthority },
          true,
          "high",
          photoTransition,
          retry.authority,
        );
        if (
          !windowReady ||
          !sourceGrid.isCurrent(retry.sourceAuthority) ||
          !photoOwner.retryPhotoIsCurrent(retry)
        )
          return;
        const refreshed = await showPreview(retry.authority, photoTransition);
        if (refreshed && view.photoStatusEmpty)
          retryStatusSurface = view.photoStatusSurface;
        const persisted = refreshed && (await persistPosition(retry.authority));
        if (
          refreshed &&
          persisted &&
          sourceGrid.isCurrent(retry.sourceAuthority) &&
          photoOwner.retryPhotoIsCurrent(retry)
        ) {
          recoveryGate.succeedTransition(photoTransition);
          setConnected(true);
          view.setPhotoStatus("Connected. Current state refreshed.");
        }
      } finally {
        const retryIsCurrent = photoOwner.retryIsCurrent(retry);
        photoOwner.finishRetry(retry);
        photoRetryPending = false;
        if (
          retryIsCurrent &&
          view.isPhotoStatusSurfaceCurrent(retryStatusSurface)
        )
          view.setPhotoStatus(
            "Could not refresh this Photo. Retry to continue.",
          );
        updateControls();
      }
    })();
  };

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

  const mapRecoveryProposal = (proposal: RecoveryProposal) => ({
    originalId: proposal.originalId,
    fromLocation: proposal.fromLocation,
    toLocation: proposal.toLocation,
    kind: proposal.kind,
    outcome: proposal.outcome,
    verified: proposal.verified,
    retire: proposal.retire
      ? { photoId: proposal.retire.photoId, location: proposal.retire.location }
      : null,
  });

  const openRecoveryReview = async (): Promise<void> => {
    view.setRecoveryPending(true);
    const result = await fetchUnavailableOriginals(
      fetcher,
      new AbortController().signal,
    );
    view.setRecoveryPending(false);
    if (!applicationAlive) return;
    if (result.kind === "ok") {
      view.openRecoveryPanel(
        result.unavailable.map((entry) => ({
          originalId: entry.originalId,
          location: entry.location,
          kind: entry.kind,
          rating: entry.rating,
          selectionState: entry.selectionState,
          albumCount: entry.albumCount,
          fingerprintEnrolled: entry.fingerprintEnrolled,
        })),
      );
      view.setRecoveryMessage();
      return;
    }
    view.setRecoveryMessage("Could not load unavailable originals. Retry.");
  };

  const proposeRecoveryBatch = async (
    oldPrefix: string,
    newPrefix: string,
  ): Promise<void> => {
    if (!oldPrefix || !newPrefix) {
      view.setRecoveryMessage("Enter both folder prefixes.");
      return;
    }
    view.setRecoveryPending(true);
    const result = await proposeRelocations(
      fetcher,
      oldPrefix,
      newPrefix,
      new AbortController().signal,
    );
    view.setRecoveryPending(false);
    if (!applicationAlive) return;
    if (result.kind === "ok") {
      view.renderRecoveryProposals(result.proposals.map(mapRecoveryProposal));
      view.setRecoveryMessage(
        result.proposals.length === 0
          ? "No unavailable originals under that folder prefix."
          : undefined,
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
    if (!applicationAlive) return;
    if (result.kind === "ok") {
      view.renderRecoveryProposals(result.proposals.map(mapRecoveryProposal));
      view.setRecoveryMessage();
      return;
    }
    view.setRecoveryMessage(
      "Could not propose that mapping. Check the location and retry.",
    );
  };

  const applyRecovery = async (
    items: ReadonlyArray<RecoveryApplyItem>,
  ): Promise<void> => {
    view.setRecoveryPending(true);
    const result = await applyRelocations(
      fetcher,
      items,
      new AbortController().signal,
    );
    view.setRecoveryPending(false);
    if (!applicationAlive) return;
    if (result.kind === "applied") {
      view.closeRecoveryPanel();
      // The committed counts are the freshest recovery truth until the next
      // scan reports its own.
      recoveryNoticeShown = `${result.relocatedPhotos}:${result.unavailablePhotos}`;
      view.setRecoveryNotice({
        relocatedPhotos: result.relocatedPhotos,
        unavailablePhotos: result.unavailablePhotos,
      });
      void refreshSource();
      return;
    }
    if (result.kind === "rejected" && result.status === 409) {
      const reasons = result.rejections
        .map((rejection) => rejection.reason)
        .join(", ");
      view.setRecoveryMessage(
        `${result.message ?? "Recovery batch rejected without changes."}${reasons ? ` (${reasons})` : ""}`,
      );
      return;
    }
    view.setRecoveryMessage("Could not apply the mappings. Retry.");
  };

  function handleViewIntent(intent: LibraryBrowserIntent): void {
    if (!applicationAlive) return;
    switch (intent.kind) {
      case "summary-action": {
        const current = summaryAction;
        if (!current || current.presentationId !== intent.presentationId)
          return;
        const outcome = application.activateSummaryAction(current.action);
        if (outcome?.kind === "refresh-current-source") void refreshSource();
        return;
      }
      case "sign-out":
        signOut();
        return;
      case "view-options-apply":
        void applyViewOptions(intent.order, intent.selection);
        return;
      case "source-open": {
        // Choosing a source creates one Grid destination with that source's
        // default order and All filter. It never implicitly replaces the Grid
        // with Photo View.
        const source = intent.source;
        if (source.kind === "library") {
          void openSource(
            "library",
            undefined,
            undefined,
            undefined,
            "source-default",
            "all",
            {
              address: "push",
            },
          );
        } else if (source.kind === "album") {
          const album = application.albums.find(
            (candidate) => candidate.id === source.id,
          );
          if (album)
            void openSource(
              "album",
              album,
              undefined,
              undefined,
              "source-default",
              "all",
              {
                address: "push",
              },
            );
        } else if (fileLocations.publication) {
          void openSource(
            "folder",
            undefined,
            undefined,
            source,
            "source-default",
            "all",
            {
              address: "push",
            },
          );
        }
        return;
      }
      case "album-resume":
        void resumeAlbum(intent.albumId);
        return;
      case "explained-action":
        void openCurrentFolder();
        return;
      case "file-location-retry": {
        const failure = fileLocationFailuresByKey.get(intent.key);
        if (failure)
          void fileLocations.retry(failure).then(handleFileLocationOutcome);
        return;
      }
      case "folder-toggle":
        if (intent.expanded) {
          if (fileLocations.collapse(intent.location)) renderSources();
        } else {
          void loadFolderWindow(intent.location, 0);
        }
        return;
      case "folder-page": {
        const retained = fileLocations.window(intent.location);
        if (!retained) return;
        const page = retained.page + intent.direction;
        if (page >= 0) void loadFolderWindow(intent.location, page);
        return;
      }
      case "folder-album-add":
        addFolderToAlbum(intent.albumId);
        return;
      case "album-form-open":
        openAlbumForm(intent.form);
        return;
      case "album-form-close":
        closeAlbumForm(intent.formId);
        return;
      case "album-form-submit":
        void submitAlbumForm(intent.formId, intent.name);
        return;
      case "grid-render":
        renderGrid();
        return;
      case "grid-range":
        // Report only: the owner owns alignment, coalescing, and admission.
        admittedRange = {
          start: intent.start,
          end: intent.end,
          authority: sourceGrid.authority,
        };
        sourceGrid.ensureRange(intent.start, intent.end, {
          // A source whose establishing window failed has an active claim and
          // placeholders still presenting its first required window. The
          // range re-admission of that window must establish readiness — the
          // same operation kind the recorded range Retry would use — or a
          // successful reload recovers the claim while every cell stays
          // disabled and no visible Retry remains.
          kind: sourceGrid.isReady(admittedRange.authority) ? "grid" : "source",
          authority: admittedRange.authority,
        });
        presentRangeStatus();
        return;
      case "filmstrip-resize": {
        // A short viewport hides the strip and releases its entries; when the
        // space returns, the strip rebuilds from the loaded facts.
        if (!view.gridVisible()) renderFilmstrip();
        return;
      }
      case "grid-resize": {
        const authority = sourceGrid.authority;
        const position = sourceGrid.readGridPosition(authority);
        if (position === undefined || !view.gridVisible()) return;
        if (sourceGrid.isCurrent(authority)) renderGrid(position);
        return;
      }
      case "open-photo": {
        const photo = sourceGrid.photoAt(intent.index);
        if (!photo) return;
        // A modifier always wins: a shift-click extends a range even in
        // Select mode, where a plain activation toggles its Photo.
        if (intent.range) {
          extendMultiSelection(intent.index, photo.id);
          return;
        }
        if (selectMode || intent.toggle) {
          toggleMultiSelection(photo.id);
          return;
        }
        void openPhoto(intent.index);
        return;
      }
      case "grid-select-mode":
        if (selectMode === intent.mode) return;
        selectMode = intent.mode;
        renderGrid();
        return;
      case "grid-multi-clear":
        if (multiSelection.size === 0 && !selectMode) return;
        clearMultiSelection();
        renderGrid();
        return;
      case "grid-batch-mutation":
        void mutateGridBatch(intent.value);
        return;
      case "grid-batch-album-add":
        void batchAddToAlbum(intent.albumId);
        return;
      case "grid-batch-album-remove":
        void removeAddedPhotosFromAlbum();
        return;
      case "grid-batch-review":
        void reviewChangedPhotos();
        return;
      case "removal-review-open":
        openRemovalReview();
        return;
      case "removal-review-close":
        removalReviewOpen = false;
        view.closeRemovalReview();
        return;
      case "removal-confirm":
        void confirmRemoval();
        return;
      case "removal-undo":
        void undoRemoval(intent.surface);
        return;
      case "removed-list-open":
        openRemovedPanel();
        return;
      case "removed-list-close":
        closeRemovedPanel();
        return;
      case "removed-page": {
        if (removedPending || removedPage === undefined) return;
        const start = removedPage.start + intent.direction * REMOVED_PAGE_LIMIT;
        if (start < 0 || start >= removedPage.total) return;
        void loadRemovedPage(start);
        return;
      }
      case "removed-retry":
        void loadRemovedPage(removedPage?.start ?? 0);
        return;
      case "removed-restore":
        void restoreRemovedPhoto(intent.photoId, intent.removedAtMs);
        return;
      case "show-grid":
        returnToSourceGrid();
        return;
      case "library-check":
        application.requestLibraryCheck();
        return;
      case "refresh":
        void refreshSource();
        return;
      case "retry-source":
        retrySource();
        return;
      case "retry-photo":
        retryCurrentPhoto();
        return;
      case "previous":
        void moveTo(photoOwner.currentIndex - 1);
        return;
      case "next":
        void moveTo(photoOwner.currentIndex + 1);
        return;
      case "undo":
        void performUndo();
        return;
      case "photo-mutation":
        void mutate(intent.field, intent.value, intent.advance);
        return;
      case "grid-photo-mutation":
        void mutateGridPhoto(intent.index, intent.field, intent.value);
        return;
      case "membership-toggle":
        toggleMembership(intent.albumId, intent.member);
        return;
      case "recovery-entry":
        void openRecoveryReview();
        return;
      case "recovery-close":
        view.closeRecoveryPanel();
        return;
      case "recovery-propose":
        void proposeRecoveryBatch(intent.oldPrefix, intent.newPrefix);
        return;
      case "recovery-propose-single":
        void proposeRecoverySingle(intent.originalId, intent.newLocation);
        return;
      case "recovery-apply":
        void applyRecovery(intent.items);
        return;
      case "membership-retry":
        refreshMembershipFacts();
    }
  }

  /// The status one reopen presents when a removal or a restore is why the
  /// current source is read again. The order-expiry texts would describe the
  /// wrong cause.
  const REMOVAL_REOPEN = Object.freeze({
    progress: "Reopening this source after the removal…",
    settled: "Source reopened after the removal.",
  });
  const RESTORE_REOPEN = Object.freeze({
    progress: "Reopening this source after the restore…",
    settled: "Source reopened after the restore.",
  });
  /// A review the server refused as gone cannot be repeated, so the source is
  /// read again and the Photographer reviews the current rejected result.
  const REVIEW_EXPIRED_REOPEN = Object.freeze({
    progress: "Reopening this source with the current rejected result…",
    settled: "Source reopened with the current rejected result.",
  });

  /// Reloads every Folder window the Sources surface presents, so the Folder
  /// Photo counts it claims are the counts the Library now holds. A window the
  /// tree does not present is left alone, and reloading never changes which
  /// Folders are expanded.
  const refreshFolderCounts = async (parent = ""): Promise<void> => {
    const retained = fileLocations.window(parent);
    if (!retained) return;
    const children = [...retained.children];
    await loadFolderWindow(parent, retained.page, false);
    for (const child of children)
      if (fileLocations.isExpanded(child.location))
        await refreshFolderCounts(child.location);
  };

  /// Brings every surface a Library-visibility change touched back to the
  /// committed Library: the Overview count and Album summaries, the Folder
  /// Photo counts, and the open source Snapshot. A source that is not loaded
  /// is not reopened, so an explained state is never replaced by one.
  const refreshAfterLibraryChange = async (
    reason: Readonly<{ progress: string; settled: string }>,
  ): Promise<void> => {
    await application.refreshOverview().catch(() => {});
    await refreshFolderCounts();
    if (sourceGrid.token === "" || !sourceGrid.isReady(sourceGrid.authority))
      return;
    const anchor =
      sourceGrid.readGridPosition(sourceGrid.authority) ??
      photoOwner.currentIndex;
    await reopenExpired(anchor, sourceGrid.generation, undefined, reason);
  };

  const removalOutcomeMessage = (counts: RemovalResult["counts"]): string => {
    const parts = [
      counts.removed > 0
        ? `${photoCountText(counts.removed)} removed from the Library. Their Original Files are unchanged.`
        : "No Photos were removed.",
    ];
    if (counts.changedElsewhere > 0)
      parts.push(
        `${photoCountText(counts.changedElsewhere)} no longer rejected, so they stayed in the Library.`,
      );
    if (counts.alreadyRemoved > 0)
      parts.push(
        `${photoCountText(counts.alreadyRemoved)} already removed by another operation.`,
      );
    if (counts.missing > 0)
      parts.push(`${photoCountText(counts.missing)} no longer in the Library.`);
    return parts.join(" ");
  };

  const restorationOutcomeMessage = (
    counts: RestorationResult["counts"],
  ): string => {
    const parts = [
      counts.restored > 0
        ? `${photoCountText(counts.restored)} restored to the Library.`
        : "Nothing was restored.",
    ];
    if (counts.changedElsewhere > 0)
      parts.push(
        `${photoCountText(counts.changedElsewhere)} already in the Library.`,
      );
    if (counts.missing > 0)
      parts.push(`${photoCountText(counts.missing)} no longer in the Library.`);
    return parts.join(" ");
  };

  /// Whether the presented result is still the one a review covers. A review
  /// names the Snapshot token it was opened on, so a source that was reopened
  /// or filtered since then cannot be confirmed against a result the
  /// Photographer no longer sees.
  const reviewCoversPresentedResult = (): boolean => {
    const review = removal.review;
    return (
      review !== undefined &&
      sourceGrid.token !== "" &&
      sourceGrid.token === review.token &&
      sourceGrid.total === review.reviewed &&
      sourceGrid.authority === review.sourceAuthority &&
      sourceGrid.selection === "rejected"
    );
  };

  const removalReviewModel = (): Parameters<
    LibraryBrowserView["renderRemovalReview"]
  >[0] => {
    const review = removal.review;
    return {
      reviewed: review?.reviewed ?? removalReviewed,
      pending: removal.busy,
      canConfirm: reviewCoversPresentedResult(),
      ...(removalResult
        ? { tone: removalResult.tone, message: removalResult.message }
        : {}),
      ...(removal.operation
        ? { undo: { removed: removal.operation.removed } }
        : {}),
    };
  };

  const renderRemoval = () => {
    if (!applicationAlive || !removalReviewOpen) return;
    view.renderRemovalReview(removalReviewModel());
  };

  /// Withdraws a review whose result is no longer presented, so the dialog
  /// never offers a confirmation the server would refuse. The Photographer
  /// reviews the result that is presented instead.
  const reconcileRemovalReview = () => {
    if (!applicationAlive || !removalReviewOpen) return;
    if (removal.review === undefined || reviewCoversPresentedResult()) return;
    removal.discardReview();
    removalResult = {
      tone: "warning",
      message:
        "The reviewed result changed. Review the current rejected result again.",
    };
    renderRemoval();
  };

  /// Opens the review of the current `Rejected` result. The review names the
  /// count it covers and the operation it would use, and removes nothing.
  const openRemovalReview = () => {
    if (
      !connected ||
      sourceGrid.selection !== "rejected" ||
      sourceGrid.token === "" ||
      sourceGrid.total === 0 ||
      removal.busy
    )
      return;
    const review = removal.openReview(
      sourceGrid.token,
      sourceGrid.total,
      sourceGrid.authority,
    );
    if (!review) return;
    removalReviewed = review.reviewed;
    removalResult = undefined;
    removalReviewOpen = true;
    view.openRemovalReview(removalReviewModel());
  };

  const confirmRemoval = async (): Promise<void> => {
    const admission = removal.confirm();
    if (!admission) return;
    renderRemoval();
    updateControls();
    const outcome = await admission.settlement;
    if (outcome.kind === "detached") return;
    if (outcome.kind === "failed") {
      // A reviewed Snapshot that is gone cannot be reviewed again: the
      // Photographer reviews the current rejected result instead of retrying
      // a request the server would refuse a second time.
      if (outcome.status === 404) {
        removal.discardReview();
        removalResult = {
          tone: "warning",
          message:
            "The reviewed result is no longer available. Review the current rejected result again.",
        };
        await refreshAfterLibraryChange(REVIEW_EXPIRED_REOPEN);
        renderRemoval();
        updateControls();
        return;
      }
      removalResult = {
        tone: "failure",
        message: "The removal could not be confirmed. Retry to continue.",
      };
      renderRemoval();
      updateControls();
      return;
    }
    removalResult = {
      tone: outcome.result.counts.removed > 0 ? "success" : "warning",
      message: removalOutcomeMessage(outcome.result.counts),
    };
    renderRemoval();
    await refreshAfterLibraryChange(REMOVAL_REOPEN);
    renderRemoval();
    updateControls();
  };

  /// Restores one confirmed operation. The surface that asked for it reports
  /// the outcome: the review dialog keeps Undo beside the removal it confirmed,
  /// and the listing offers it to a Photographer who came back to recover.
  const undoRemoval = async (surface: "review" | "listing"): Promise<void> => {
    const admission = removal.undo();
    if (!admission) return;
    if (surface === "review") renderRemoval();
    else renderRemovedPanel();
    updateControls();
    const outcome = await admission.settlement;
    if (outcome.kind === "detached") return;
    if (outcome.kind === "failed") {
      const message = "The removal could not be undone. Retry to continue.";
      if (surface === "review") {
        removalResult = { tone: "failure", message };
        renderRemoval();
      } else {
        removedMessage = message;
        renderRemovedPanel();
      }
      updateControls();
      return;
    }
    removal.forgetOperation(admission.operationId);
    const message = restorationOutcomeMessage(outcome.result.counts);
    const tone = outcome.result.counts.restored > 0 ? "success" : "warning";
    await application.refreshOverview().catch(() => {});
    await refreshFolderCounts();
    if (surface === "review") {
      removalResult = { tone, message };
      renderRemoval();
    } else if (removedPanelOpen) {
      // The listing reads the Library again before it reports the restore, so
      // no row outlives the removal it presents.
      await loadRemovedPage(removedPage?.start ?? 0, message);
    }
    if (sourceGrid.token !== "" && sourceGrid.isReady(sourceGrid.authority)) {
      const anchor =
        sourceGrid.readGridPosition(sourceGrid.authority) ??
        photoOwner.currentIndex;
      await reopenExpired(
        anchor,
        sourceGrid.generation,
        undefined,
        RESTORE_REOPEN,
      );
    }
    updateControls();
  };

  const renderRemovedPanel = () => {
    if (!applicationAlive) return;
    view.renderRemovedPanel({
      start: removedPage?.start ?? 0,
      total: removedPage?.total ?? 0,
      limit: REMOVED_PAGE_LIMIT,
      pending: removedPending,
      canRetry: removedLoadFailed,
      ...(removal.operation
        ? { undo: { removed: removal.operation.removed } }
        : {}),
      ...(removedRestoringId ? { restoringPhotoId: removedRestoringId } : {}),
      ...(removedMessage ? { message: removedMessage } : {}),
      items: (removedPage?.items ?? []).map((item) => ({
        photoId: item.photo.id,
        filename: item.photo.originalFilename ?? item.photo.id,
        removedAtMs: item.removedAtMs,
        preview: item.photo.preview,
      })),
    });
  };

  const loadRemovedPage = async (
    start: number,
    successMessage?: string,
  ): Promise<void> => {
    removedAbort?.abort();
    const controller = new AbortController();
    removedAbort = controller;
    removedPending = true;
    removedLoadFailed = false;
    removedMessage = undefined;
    renderRemovedPanel();
    updateControls();
    const result = await fetchRemovedPhotos(fetcher, {
      start,
      limit: REMOVED_PAGE_LIMIT,
      signal: controller.signal,
    });
    if (controller.signal.aborted || removedAbort !== controller) return;
    removedAbort = undefined;
    removedPending = false;
    if (result.kind === "ok") {
      removedPage = {
        start: result.start,
        total: result.total,
        items: result.photos,
      };
      removedMessage = successMessage;
    } else {
      removedLoadFailed = true;
      removedMessage =
        "The removed Photos could not be loaded. Retry to continue.";
    }
    renderRemovedPanel();
    updateControls();
  };

  const openRemovedPanel = (): void => {
    removedPanelOpen = true;
    removedPage = undefined;
    removedLoadFailed = false;
    removedRestoringId = undefined;
    removedMessage = undefined;
    view.openRemovedPanel({
      start: 0,
      total: 0,
      limit: REMOVED_PAGE_LIMIT,
      pending: true,
      canRetry: false,
      items: [],
    });
    void loadRemovedPage(0);
  };

  const closeRemovedPanel = (): void => {
    removedPanelOpen = false;
    removedAbort?.abort();
    removedAbort = undefined;
    removedPending = false;
    removedRestoringId = undefined;
    view.closeRemovedPanel();
  };

  const restoreRemovedPhoto = async (
    photoId: string,
    removedAtMs: number,
  ): Promise<void> => {
    const admission = removal.restorePhotos([{ photoId, removedAtMs }]);
    if (!admission) return;
    removedRestoringId = photoId;
    removedMessage = undefined;
    renderRemovedPanel();
    updateControls();
    const outcome = await admission.settlement;
    removedRestoringId = undefined;
    if (outcome.kind === "detached") return;
    if (outcome.kind === "failed") {
      removedMessage =
        outcome.status === 404
          ? "That Photo is no longer removed from the Library. Reload this listing."
          : "The Photo could not be restored. Retry to continue.";
      renderRemovedPanel();
      updateControls();
      return;
    }
    // The listing reads the Library again before it reports the restore, so a
    // row never outlives the state it presents.
    await application.refreshOverview().catch(() => {});
    await refreshFolderCounts();
    await loadRemovedPage(
      removedPage?.start ?? 0,
      restorationOutcomeMessage(outcome.result.counts),
    );
    if (sourceGrid.token !== "" && sourceGrid.isReady(sourceGrid.authority)) {
      const anchor =
        sourceGrid.readGridPosition(sourceGrid.authority) ??
        photoOwner.currentIndex;
      await reopenExpired(
        anchor,
        sourceGrid.generation,
        undefined,
        RESTORE_REOPEN,
      );
    }
    updateControls();
  };

  /// The destination the live Snapshot presents, derived from the committed
  /// source, order, and filter. An address is never derived from a retained
  /// projection of Photo facts.
  const liveDestination = (photoId?: string): NavigationDestination => {
    const source = sourceGrid.source;
    const order =
      sourceGrid.order === "source-default" ? undefined : sourceGrid.order;
    return {
      source: source.kind,
      ...(source.kind === "folder"
        ? { folderPath: source.folder.location }
        : {}),
      ...(source.kind === "album" ? { albumId: source.album.id } : {}),
      ...(photoId ? { photoId } : {}),
      ...(order ? { order } : {}),
      selection: sourceGrid.selection,
    };
  };

  /// The wire order one address requests. An omitted order and the Album's own
  /// order both leave the order to the server.
  const destinationOrder = (
    destination: NavigationDestination,
  ): SourceViewOrder =>
    destination.order === undefined || destination.order === "album-order"
      ? "source-default"
      : destination.order;

  /// The display name of a Folder Location. The File Location tree names the
  /// Folders it has loaded; a Location outside a loaded window falls back to
  /// its last component, and the server answers authoritatively.
  const folderNameFor = (location: string): string => {
    if (location === "") return "Library Folder";
    const separator = location.lastIndexOf("/");
    const parent = separator === -1 ? "" : location.slice(0, separator);
    const known = fileLocations
      .window(parent)
      ?.children.find((child) => child.location === location);
    return known?.name ?? location.slice(separator + 1);
  };

  /// True when the live Snapshot can serve a destination without reopening the
  /// source: the same source, order, and filter, and a Folder whose
  /// publication still matches the entry's provenance.
  const reusableForTraversal = (
    destination: NavigationDestination,
    folderPublication?: string,
  ): boolean => {
    if (!sourceGrid.token || !sourceGrid.isReady(sourceGrid.authority))
      return false;
    if (!sameSourceView(liveDestination(), destination)) return false;
    return !(
      destination.source === "folder" &&
      folderPublication !== undefined &&
      folderPublication !== fileLocations.publication
    );
  };

  /// Resolves one captured Grid anchor against the established Snapshot. The
  /// stable identity is confirmed before the index hint is trusted; an absent
  /// anchor clamps the prior index hint to the current source. Undefined means
  /// a newer destination superseded this one.
  const resolveRestorationIndex = async (
    authority: SourceAuthority,
    fallbackIndex: number,
    restoration: NavigationGridRestoration,
  ): Promise<number | undefined> => {
    if (!sourceGrid.isCurrent(authority)) return undefined;
    const anchor = restoration.anchor;
    const retained = sourceGrid.findPhotoIndex(anchor.photoId);
    if (
      retained !== undefined &&
      sourceGrid.photoAt(retained)?.id === anchor.photoId
    )
      return retained;
    const resolved = await sourceGrid.resolvePhotoPosition(
      authority,
      anchor.photoId,
    );
    if (!sourceGrid.isCurrent(authority)) return undefined;
    if (resolved.kind === "resolved") return resolved.position;
    if (resolved.kind === "missing")
      return Math.min(anchor.indexHint, Math.max(0, sourceGrid.total - 1));
    // A failed lookup is retryable, not evidence that the Photo disappeared,
    // so the bounded position the open already resolved stays usable.
    return fallbackIndex;
  };

  /// The geometry one restoration applies at a resolved index: the anchor's
  /// row and offset, and the cell the focus target names when the Snapshot
  /// still holds that Photo identity. An anchor the Snapshot no longer holds
  /// falls back to the clamped position, which restores geometry but leaves
  /// the Grid itself as the focus target.
  const restorationGeometry = (
    index: number,
    restoration: NavigationGridRestoration,
  ): Readonly<{ index: number; offset: number; focusIndex?: number }> => {
    const anchorHeld =
      sourceGrid.photoAt(index)?.id === restoration.anchor.photoId;
    const focusIndex =
      anchorHeld && restoration.focus.kind === "photo"
        ? sourceGrid.findPhotoIndex(restoration.focus.photoId)
        : undefined;
    return {
      index,
      offset: restoration.anchor.offset,
      ...(focusIndex !== undefined ? { focusIndex } : {}),
    };
  };

  /// An explained fallback: the confirmed invalid or missing target is named,
  /// the current entry is replaced once with All Photos, and no request is
  /// made for the invalid source.
  const fallbackToAllPhotos = async (message: string): Promise<boolean> => {
    navigation.replaceGrid(allPhotosDestination);
    const outcome = await openSource(
      "library",
      undefined,
      undefined,
      undefined,
      "source-default",
      "all",
      {
        explanation: message,
      },
    );
    return outcome.kind === "established";
  };

  /// A Photo no longer in the requested source or current filter returns to
  /// that source's Grid, preserving the requested filter, with an explanation.
  const fallbackToSourceGrid = async (
    destination: NavigationDestination,
    message: string,
  ): Promise<boolean> => {
    const grid = gridDestination(destination);
    navigation.replaceGrid(
      grid,
      undefined,
      destination.source === "folder" ? fileLocations.publication : undefined,
    );
    return establishDestination(grid, { explanation: message });
  };

  /// A Folder entry whose publication changed keeps its Location but must not
  /// silently reinterpret it: the Photographer is told the Folder changed and
  /// must explicitly open the current Folder.
  const presentFolderPublicationChange = (
    destination: NavigationDestination,
  ): void => {
    pendingCurrentFolder = destination.folderPath ?? "";
    // The destination shell is the Grid: Photo View must not keep presenting
    // the Photo the traversal left.
    leavePhotoView();
    view.prepareSourceOpen(sourceGrid.name);
    view.setGridExplanation(
      "This Folder changed with a newer Library publication. Open the current Folder to browse it.",
      { label: "Open current Folder" },
    );
  };

  let pendingCurrentFolder: string | undefined;

  /// Opens the Photo a destination names against the live Snapshot. Undefined
  /// means a newer destination superseded this one.
  const openTraversedPhoto = async (
    destination: NavigationDestination,
    photoId: string,
  ): Promise<boolean | undefined> => {
    const retained = sourceGrid.findPhotoIndex(photoId);
    if (retained !== undefined && sourceGrid.photoAt(retained)?.id === photoId)
      return openPhoto(retained, "none");
    const resolved = await sourceGrid.resolvePhotoPosition(
      sourceGrid.authority,
      photoId,
    );
    if (!sourceGrid.isCurrent(sourceGrid.authority)) return undefined;
    if (resolved.kind === "expired")
      // The Browse Snapshot this destination was resolved against has been
      // released, so the source is reopened exactly as an expired window is
      // and the Photo is resolved again against the fresh Snapshot.
      return reopenForTraversedPhoto(destination, photoId);
    if (resolved.kind === "missing")
      // A null position is the server confirming the Photo is absent from
      // this source and filter, which is the explained source-Grid fallback.
      return fallbackToSourceGrid(
        destination,
        "This Photo is no longer in this view. Showing the source Grid.",
      );
    if (resolved.kind === "resolved")
      return openPhoto(resolved.position, "none");
    // A failed or detached lookup never silently replaces the destination:
    // only a superseded one stops here, and a failed one keeps the target
    // URL with a retryable destination state.
    return resolved.kind === "failed"
      ? presentRetryableTraversal(destination)
      : undefined;
  };

  /// Reopens the source an expired traversal was resolved against and
  /// resolves its Photo again. The established expired-snapshot path owns the
  /// reopen, so a released Browse token is answered by a fresh Snapshot
  /// rather than by an explanation that claims the Photo is gone.
  const reopenForTraversedPhoto = async (
    destination: NavigationDestination,
    photoId: string,
  ): Promise<boolean | undefined> => {
    await reopenExpired(
      photoOwner.currentIndex,
      sourceGrid.generation,
      photoId,
    );
    if (!applicationAlive || !sourceGrid.token) return undefined;
    if (!sourceGrid.isCurrent(sourceGrid.authority)) return undefined;
    const reopened = sourceGrid.readGridPosition(sourceGrid.authority);
    if (reopened !== undefined && sourceGrid.photoAt(reopened)?.id === photoId)
      return openPhoto(reopened, "none");
    const resolved = await sourceGrid.resolvePhotoPosition(
      sourceGrid.authority,
      photoId,
    );
    if (!sourceGrid.isCurrent(sourceGrid.authority)) return undefined;
    if (resolved.kind === "resolved")
      return openPhoto(resolved.position, "none");
    if (resolved.kind === "missing")
      return fallbackToSourceGrid(
        destination,
        "This Photo is no longer in this view. Showing the source Grid.",
      );
    // A second failure is retryable, not evidence that the Photo disappeared.
    return resolved.kind === "failed"
      ? presentRetryableTraversal(destination)
      : undefined;
  };

  /// The retryable destination state a failed traversal keeps: the target URL
  /// stays as the browser left it, the previous content is replaced by a
  /// truthful shell, and the source Retry the failed-open path already owns
  /// re-establishes this destination.
  const presentRetryableTraversal = (
    destination: NavigationDestination,
  ): boolean => {
    retryableTraversal = destination;
    leavePhotoView();
    view.prepareSourceOpen(sourceGrid.name);
    setGridStatusText("Could not load this source. Retry to continue.");
    const generation = String(sourceGrid.generation);
    const claim = recoveryGate.issue("source-position", generation, {
      owner: { scope: "source", generation },
    });
    recoveryGate.fail(claim, { transportLost: true });
    syncConnection();
    updateControls();
    return false;
  };

  /// Renders a Grid destination the live Snapshot already serves and restores
  /// its anchor. No source is reopened, so frozen membership survives.
  const showTraversedGrid = async (
    restoration?: NavigationGridRestoration,
    explanation?: string,
  ): Promise<boolean> => {
    const authority = sourceGrid.authority;
    const gridPosition = sourceGrid.readGridPosition(authority);
    if (gridPosition === undefined) return false;
    const restoreIndex = restoration
      ? await resolveRestorationIndex(authority, gridPosition, restoration)
      : gridPosition;
    if (restoreIndex === undefined) return false;
    leavePhotoView();
    view.showGrid();
    if (restoration) {
      // Only the bounded window covering the restored row is admitted, and the
      // anchor's stable identity is confirmed against the live Snapshot before
      // its index hint is trusted.
      const windowReady = await loadWindow(
        restoreIndex,
        { kind: "source", authority },
        false,
        "high",
      );
      if (!sourceGrid.isCurrent(authority) || !windowReady) return false;
    }
    renderGrid(restoreIndex);
    if (restoration)
      view.restoreGridAnchor(restorationGeometry(restoreIndex, restoration));
    presentRangeStatus();
    // An explained fallback names the confirmed missing target after the
    // ordinary status has been presented.
    if (explanation) setGridStatusText(explanation);
    updateControls();
    return true;
  };

  /// Establishes one destination: reusing the live Snapshot when its source,
  /// order, and filter match, and otherwise opening the requested source.
  const establishDestination = async (
    destination: NavigationDestination,
    options: SourceEstablishmentOptions &
      Readonly<{
        addressed?: boolean;
        /// Bind the current Published Library instead of reusing a Snapshot
        /// opened under a superseded publication.
        reopen?: boolean;
      }> = {},
  ): Promise<boolean> => {
    if (!applicationAlive) return false;
    // A newer intent is already establishing a different destination, so this
    // traversal is superseded rather than repainting what the page left.
    if (pendingDestination && !sameSourceView(pendingDestination, destination))
      return false;
    const photoId = destination.photoId;
    if (
      !options.reopen &&
      reusableForTraversal(destination, options.folderPublication)
    ) {
      if (photoId)
        return Boolean(await openTraversedPhoto(destination, photoId));
      return showTraversedGrid(options.restoration, options.explanation);
    }
    if (
      destination.source === "folder" &&
      options.folderPublication !== undefined &&
      fileLocations.publication !== undefined &&
      options.folderPublication !== fileLocations.publication
    ) {
      presentFolderPublicationChange(destination);
      return false;
    }
    if (destination.source === "library") {
      const outcome = await openSource(
        "library",
        undefined,
        photoId,
        undefined,
        destinationOrder(destination),
        destination.selection,
        options,
      );
      return commitDestinationPhoto(destination, photoId, outcome, () =>
        fallbackToAllPhotos("This source is no longer available."),
      );
    }
    if (destination.source === "album") {
      const album = application.albums.find(
        (candidate) => candidate.id === destination.albumId,
      );
      if (!album)
        return fallbackToAllPhotos("This Album is no longer available.");
      const outcome = await openSource(
        "album",
        album,
        photoId,
        undefined,
        destinationOrder(destination),
        destination.selection,
        options,
      );
      return commitDestinationPhoto(destination, photoId, outcome, () =>
        fallbackToAllPhotos("This Album is no longer available."),
      );
    }
    // A Folder destination binds to the current Published Library and opens
    // only a valid current relative Location.
    if (!fileLocations.publication) {
      const bound = await awaitRootBinding();
      if (!applicationAlive) return false;
      if (!bound) {
        setGridStatusText("Could not load this source. Retry to continue.");
        return false;
      }
    }
    const outcome = await openSource(
      "folder",
      undefined,
      photoId,
      {
        location: destination.folderPath ?? "",
        name: folderNameFor(destination.folderPath ?? ""),
      },
      destinationOrder(destination),
      destination.selection,
      options,
    );
    return commitDestinationPhoto(destination, photoId, outcome, () =>
      fallbackToAllPhotos(
        "This Folder is no longer part of the Library. Showing All Photos.",
      ),
    );
  };

  /// Finishes a destination whose source has just been established: a Grid
  /// destination is committed, and a Photo destination confirms the stable
  /// identity before presenting it. A preferred Photo is only a hint to source
  /// opening, so a fallback position is never proof that the Photo exists.
  const commitDestinationPhoto = async (
    destination: NavigationDestination,
    photoId: string | undefined,
    outcome: SourceEstablishment,
    missing: () => Promise<boolean>,
  ): Promise<boolean> => {
    if (outcome.kind === "missing") return missing();
    if (outcome.kind !== "established") return false;
    if (!photoId) return true;
    return Boolean(await openTraversedPhoto(destination, photoId));
  };

  /// Applies one browser traversal. The address has already been chosen by the
  /// browser, so the page renders a destination shell while resolving it and
  /// never undoes the traversal.
  applyNavigationTraversal = (traversal: NavigationTraversal) => {
    void applyTraversal(traversal);
  };

  const applyTraversal = async (traversal: NavigationTraversal) => {
    if (!applicationAlive) return;
    retryableTraversal = undefined;
    view.closeTransientSurfaces();
    const entry = traversal.entry;
    await establishDestination(traversal.destination, {
      addressed: true,
      ...(entry?.anchor && entry.focus
        ? { restoration: { anchor: entry.anchor, focus: entry.focus } }
        : {}),
      ...(entry?.folderPublication
        ? { folderPublication: entry.folderPublication }
        : {}),
    });
  };

  /// Records the Photo destination this navigation committed. A repeated
  /// activation of the current destination records nothing, so duplicate
  /// activation creates no duplicate history entry.
  const recordPhotoAddress = (
    photoId: string,
    mode: "push" | "replace" | "none",
  ): void => {
    if (mode === "none") return;
    const destination = liveDestination(photoId);
    if (mode === "replace") {
      navigation.replacePhoto(destination);
      return;
    }
    if (
      navigation.destination.photoId === photoId &&
      sameSourceView(navigation.destination, destination)
    )
      return;
    navigation.openPhoto(destination);
  };

  /// Album Resume resolves the saved position under the existing
  /// saved-position rules and opens Photo View. From another source the Album
  /// Grid entry is committed first, so returning from the Photo has a
  /// meaningful source destination; from the current Grid only the Photo entry
  /// is added.
  const resumeAlbum = async (albumId: string): Promise<void> => {
    if (!applicationAlive || pageBusy || photoOwner.busy) return;
    const album = application.albums.find(
      (candidate) => candidate.id === albumId,
    );
    if (!album) return;
    const alreadyCurrent =
      sourceGrid.kind === "album" &&
      sourceGrid.albumId === albumId &&
      sourceGrid.isReady(sourceGrid.authority);
    // The Album is opened without a preferred Photo, so the server resumes at
    // the durable saved position in its Browse response.
    const outcome = await openSource(
      "album",
      album,
      undefined,
      undefined,
      "source-default",
      "all",
      { address: alreadyCurrent ? "replace" : "push" },
    );
    if (outcome.kind !== "established") return;
    const authority = sourceGrid.authority;
    const position = sourceGrid.readGridPosition(authority);
    const savedPhoto =
      position === undefined ? undefined : sourceGrid.photoAt(position);
    if (position === undefined || !savedPhoto) {
      setGridStatusText(
        "Resume is unavailable: this Album has no saved position.",
      );
      return;
    }
    await openPhoto(position, "push");
  };

  /// The in-app source return from Photo View. It traverses one entry only
  /// when the current Photo has a known parent Grid entry; a directly loaded
  /// Photo replaces its entry with its source Grid instead of risking
  /// navigation to another site.
  const returnToSourceGrid = () => {
    if (navigation.returnToSourceGrid(liveDestination()) === "traversed")
      return;
    showGrid();
  };

  /// Opens the current Folder after an entry's publication changed. The action
  /// replaces the entry's provenance and adds no history loop.
  const openCurrentFolder = async (): Promise<void> => {
    const location = pendingCurrentFolder ?? "";
    pendingCurrentFolder = undefined;
    const destination: NavigationDestination = {
      source: "folder",
      folderPath: location,
      selection: "all",
    };
    navigation.replaceGrid(destination, undefined, fileLocations.publication);
    // The action replaces the entry's provenance with the current publication,
    // so the Folder is opened against it rather than reusing a stale Snapshot.
    await establishDestination(destination, { addressed: true, reopen: true });
  };

  void application.loadOverview();
  return () => {
    if (!applicationAlive) return;
    applicationAlive = false;
    photoMetadataAbort?.abort();
    membershipAbort?.abort();
    removedAbort?.abort();
    removal.dispose();
    view.dispose();
    cancelScheduledGridRender();
    unsubscribeWindowSettled();
    navigation.dispose();
    albumRecovery = undefined;
    batchAlbumCompensation = undefined;
    albumActions.dispose();
    savedPositions.dispose();
    application.dispose();
    fileLocations.dispose();
    photoOwner.dispose();
    sourceGrid.dispose();
    recoveryGate.close();
  };
}
