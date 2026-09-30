import {
  RecoveryGate,
  type RecoveryClaim,
  type RecoveryTransition,
} from "./model/async-ownership.js";
import {
  createBrowseRangeRecoveryOwner,
  type BrowseRangeRecoveryOwner,
  type GridRangeRetry,
} from "./model/browse-range-recovery-owner.js";
import type {
  AlbumSummary,
  SelectionFilter,
  SelectionState,
} from "./api/contracts.js";
import { createEditorController } from "./model/editor-controller.js";
import { createPhotoDetailsOwner } from "./model/photo-details-owner.js";
import { createMetadataPanel } from "./ui/external-metadata-panel.js";
import { createRecoveryReviewOwner } from "./model/recovery-review-owner.js";
import { createFileLocationController } from "./model/file-location-controller.js";
import {
  createApplicationOwner,
  type ApplicationCoordination,
  type ApplicationEvent,
  type ApplicationPresentation,
} from "./model/application-owner.js";
import { createApplicationPresentationController } from "./model/application-presentation-controller.js";
import {
  createSourceGridOwner,
  type SourceGridSource,
  type SourceWindowOperation,
} from "./model/source-grid-owner.js";
import {
  createSourceOpenOwner,
  type SourceLifecycleOwner,
} from "./model/source-open-owner.js";
import { releaseBrowse, type SourceViewOrder } from "./api/source-grid.js";
import { createRemovalOwner } from "./model/removal-owner.js";
import { createRemovedListingOwner } from "./model/removed-listing-owner.js";
import { createGridMultiSelectionOwner } from "./model/grid-multi-selection-owner.js";
import { createAlbumActionOwner } from "./model/album-action-owner.js";
import { createAlbumManagementController } from "./model/album-management-controller.js";
import {
  createRemovalReviewController,
  restorationOutcomeMessage,
} from "./model/removal-review-controller.js";
import { createAlbumMutationController } from "./model/album-mutation-controller.js";
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
import { createNavigationSession } from "./model/navigation-session.js";
import {
  destinationOrder,
  folderNameFor,
  liveDestination,
  resolveRestorationIndex,
  restorationGeometry,
  reusableForTraversal,
} from "./model/destination-policy.js";
import {
  createLibraryBrowserView,
  type LibraryBrowserIntent,
  type LibraryBrowserView,
  type SourceListViewModel,
} from "./ui/library-browser-view.js";
import { formatPhotoCount } from "./ui/photo-count.js";
import { mountAccessBoundary } from "./ui/access-boundary.js";
import type { BrowserFetch } from "./model/access-session.js";

type SourceEstablishment =
  | Readonly<{ kind: "established" }>
  | Readonly<{ kind: "superseded" }>
  | Readonly<{ kind: "failed" }>
  | Readonly<{ kind: "missing" }>;

type SourceEstablishmentOptions = Readonly<{
  restoration?: NavigationGridRestoration;
  folderPublication?: string;
  explanation?: string;
  address?: "push" | "replace" | "none";
}>;

type OpenSourceOptions = Readonly<{
  kind: "library" | "album" | "folder";
  album?: AlbumSummary;
  preferredPhotoId?: string | undefined;
  folder?: { location: string; name: string };
  order?: SourceViewOrder;
  selection?: SelectionFilter;
  establishment?: SourceEstablishmentOptions;
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
  const navigation = createNavigationOwner(
    { captureGridRestoration: () => navigationSession.gridRestoration },
    (traversal) => {
      void applyTraversal(traversal);
    },
  );
  const navigationSession = createNavigationSession(navigation.start());
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
  const metadataHost = root.querySelector<HTMLElement>(
    "[data-external-metadata]",
  );
  if (!metadataHost) throw new Error("External Metadata surface is missing.");
  const metadataPanel = createMetadataPanel(
    metadataHost,
    fetcher,
    (message) => {
      view.presentSummary(message);
    },
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
  const editor = createEditorController(fetcher, view, {
    isAlive: () => applicationAlive,
    isCurrentPhoto: (photoId) =>
      photoOwner.isCurrent(photoOwner.authority) &&
      photoOwner.current?.id === photoId,
    currentPhoto: () => photoOwner.current,
    libraryPhase: () => applicationPresentation.scanPhase,
  });
  const {
    open: openEditor,
    refresh: refreshEditor,
    commitExposure: commitEditorExposure,
    commitWhiteBalance: commitEditorWhiteBalance,
    stepHistory: stepEditorHistory,
    requestPreview: requestEditorPreview,
    applyStage: applyEditorStage,
    setComparison: setEditorComparison,
    useSaved: useSavedRecipe,
    reapplyLocal: reapplyLocalSettings,
    discardDraft: discardEditorDraft,
    rebind: rebindEditor,
    createProxy: createEditorProxy,
    removeProxy: removeEditorProxy,
    leave: leaveEditor,
  } = editor;
  const savedPositions = createSavedPositionOwner(fetcher, {
    isSourceCurrent: (authority, albumId) =>
      sourceGrid.isCurrent(authority) &&
      sourceGrid.kind === "album" &&
      sourceGrid.albumId === albumId,
    isPhotoCurrent: (authority, photoId) =>
      photoOwner.isCurrent(authority) && photoOwner.current?.id === photoId,
  });

  const presentApplication = (presentation: ApplicationPresentation): void => {
    if (!applicationAlive) return;
    if (presentation.kind === "summary") {
      applicationPresentation.presentSummary(presentation.summary);
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
    recoveryReview.presentRecoveryNotice(
      presentation.overview.scan.lastRecovery,
    );
    renderMembershipControls();
    refreshMembershipFacts();
    renderSources();
  };

  const coordinateApplication = async (
    coordination: ApplicationCoordination,
  ): Promise<void> => {
    if (!applicationAlive) return;
    if (
      coordination.kind === "mark-reachable" ||
      coordination.kind === "transport-lost" ||
      coordination.kind === "fail-application-recovery" ||
      coordination.kind === "recover"
    ) {
      applicationPresentation.coordinate(coordination);
      return;
    }
    if (coordination.kind === "publication-advanced") {
      // A completed scan published new Library source facts. The open
      // Photo's Edit surface re-reads its bounded edit facts now, so a
      // source whose read was pending while the Library recovered becomes
      // editable without a reload or a reopened Photo. The read joins one
      // already under way, and an Edit surface that is not presented stays
      // untouched: opening it later reads the facts fresh anyway.
      const photoId = photoOwner.current?.id;
      if (photoId && view.editorVisible()) void refreshEditor(photoId);
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
      const startup = navigationSession.takeStartup();
      if (startup) {
        const { destination, explanation, restoration } = startup;
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
            liveDestination(sourceGrid),
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
  const applicationPresentation = createApplicationPresentationController(
    application,
    recoveryGate,
    {
      isAlive: () => applicationAlive,
      isConnectionEstablished: () => connectionEstablished,
      presentSummary: (text, action, state) =>
        view.presentSummary(
          text,
          action
            ? {
                kind: action.action.kind,
                presentationId: action.presentationId,
              }
            : undefined,
          state,
        ),
      setConnected: () => setConnected(true),
      syncConnection: () => syncConnection(),
    },
  );
  const albumActions = createAlbumActionOwner(fetcher);
  const removal = createRemovalOwner(fetcher);
  // Trash owns listing state; the page coordinates owners touched by writes.
  const removedListing = createRemovedListingOwner(fetcher, {
    removal,
    present: {
      openPanel: (model) => view.openRemovedPanel(model),
      renderPanel: (model) => {
        if (!applicationAlive) return;
        view.renderRemovedPanel(model);
      },
      closePanel: () => view.closeRemovedPanel(),
      openReview: (model) => view.openTrashReview(model),
      renderReview: (model) => {
        if (!applicationAlive) return;
        view.renderTrashReview(model);
      },
      closeReview: () => view.closeTrashReview(),
      controlsChanged: () => updateControls(),
    },
    coordinate: {
      refreshLibrary: async () => {
        await application.refreshOverview().catch(() => {});
        await folders.refreshCounts();
      },
      reopenSourceAfterRestore: async () => {
        if (
          sourceGrid.token !== "" &&
          sourceGrid.isReady(sourceGrid.authority)
        ) {
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
      },
      restorationMessage: (counts) => restorationOutcomeMessage(counts),
    },
  });
  const gridMulti = createGridMultiSelectionOwner({
    present: {
      renderGrid: (position) => renderGrid(position),
      renderBatchAlbums: (model) => view.renderBatchAlbums(model),
      resetGridMultiSelection: () => view.resetGridMultiSelection(),
      setDecisionStatus: (text) => setDecisionStatus(text),
      focusGridIndex: (index) => view.focusGridIndex(index),
      updateControls: () => updateControls(),
    },
    coordinate: {
      alive: () => applicationAlive,
      connected: () => connected,
      pageBusy: () => pageBusy,
      setPageBusy: (value) => {
        pageBusy = value;
      },
      gridVisible: () => view.gridVisible(),
      canOpenGridPhoto: () => canOpenGridPhoto(),
      sourceGrid,
      photoOwner,
      albumActions,
      albums: () => application.albums,
      mutateAlbum: (start) => mutateAlbum(start, "summary"),
      membershipAlbumName: (albumId) =>
        application.albums.find((album) => album.id === albumId)?.name ??
        "Album",
      loadWindow: (index, operation, quiet, priority) =>
        loadWindow(index, operation, quiet, priority),
      reopenExpired: (anchorIndex, generation) =>
        reopenExpired(anchorIndex, generation),
      failPhotoRecovery: (authority, kind) =>
        failPhotoRecovery(authority, kind),
    },
  });

  let connected = false;
  let connectionEstablished = false;
  let pageBusy = false;
  let photoRetryPending = false;
  const canOpenGridPhoto = () =>
    sourceGrid.isReady(sourceGrid.authority) &&
    !pageBusy &&
    !photoOwner.busy &&
    !photoOwner.opening;
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
  const syncConnection = (message?: string) => {
    if (!applicationAlive) return;
    albumMutations.clearInactive();
    rangeRecovery?.clearInactive();
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
  const rangeRecovery: BrowseRangeRecoveryOwner =
    createBrowseRangeRecoveryOwner({
      gate: recoveryGate,
      source: {
        isCurrent: (authority) => sourceGrid.isCurrent(authority),
        photoAt: (index) => sourceGrid.photoAt(index),
        alignedStart: (index) => sourceGrid.alignedStart(index),
        describeWindow: (index) => sourceGrid.describeWindow(index),
        total: () => sourceGrid.total,
      },
      setRangeStatus: setRangeStatusText,
      formatPhotoCount,
      onFailure: syncConnection,
      onRecovered: () => setConnected(true),
    });
  const presentRangeStatus = (): void => rangeRecovery?.presentStatus();
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
  ): void =>
    rangeRecovery?.fail(
      ownerScope,
      generation,
      start,
      transportLost,
      retryRange,
      transition,
    );
  const recoverBrowseRange = (
    ownerScope: "source" | "photo",
    generation: string,
    start: number,
  ): void => rangeRecovery?.recover(ownerScope, generation, start);
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
  const renderSortControl = () => {
    if (!applicationAlive) return;
    const interactionBusy = pageBusy || photoRetryPending || photoOwner.busy;
    view.renderSort({
      kind: sourceGrid.kind,
      value: sourceGrid.order,
      enabled: !interactionBusy && !photoOwner.opening,
    });
  };

  const renderFilterControl = () => {
    if (!applicationAlive) return;
    const interactionBusy = pageBusy || photoRetryPending || photoOwner.busy;
    view.renderFilter({
      value: sourceGrid.selection,
      enabled: !interactionBusy && !photoOwner.opening,
    });
  };

  // Counts cover the complete source, not just loaded or filtered windows.
  const renderProgress = () => {
    if (!applicationAlive) return;
    const counts = sourceGrid.selectionCounts;
    view.renderProgress({
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
    removalReview.reconcile();
  };

  const albumMutations = createAlbumMutationController({
    actions: albumActions,
    application,
    source: sourceGrid,
    gate: recoveryGate,
    photo: photoOwner,
    presentation: {
      get surface() {
        return view.photoStatusSurface;
      },
      isCurrent: (surface) => view.isPhotoStatusSurfaceCurrent(surface),
      status: (message) => view.setPhotoStatus(message),
    },
    connectionChanged: syncConnection,
    reachable: () => setConnected(true),
  });
  const mutateAlbum: typeof albumMutations.mutate = (...args) =>
    albumMutations.mutate(...args);

  const folders = createFileLocationController({
    fetcher,
    application,
    recoveryGate,
    isAlive: () => applicationAlive,
    onChanged: () => {
      syncConnection();
      renderSources();
    },
    onReachable: () => setConnected(true),
  });
  const fileLocations = folders.owner;
  const resetFileLocations = () => folders.reset();
  const rebindFileLocations = () => folders.rebind();
  const loadFolderWindow: typeof folders.load = (...args) =>
    folders.load(...args);
  const awaitRootBinding = () => folders.awaitRootBinding();
  const releasePublicationLocationRecovery = () =>
    folders.releasePublicationNotice();
  const claimPublicationLocationNotice: typeof folders.claimPublicationNotice =
    (...args) => folders.claimPublicationNotice(...args);
  const sourceLifecycle: SourceLifecycleOwner = createSourceOpenOwner({
    sourceGrid,
    fileLocations,
    rebindFileLocations,
    onPublicationConflict: (publication) =>
      claimPublicationLocationNotice(
        `publication:${publication}`,
        "Library changed. Reopen this folder.",
      ),
  });

  const renderSources = () => {
    if (!applicationAlive) return;
    const tree = folders.tree(
      (location) =>
        sourceGrid.kind === "folder" &&
        sourceGrid.folder?.location === location,
    );
    const model: SourceListViewModel = {
      libraryCount: application.overview?.photoCount ?? 0,
      libraryActive: sourceGrid.kind === "library",
      fileLocationsEnabled: tree.enabled,
      fileLocationFailures: tree.failures,
      rootExpanded: tree.rootExpanded,
      rootActive:
        sourceGrid.kind === "folder" && sourceGrid.folder?.location === "",
      rootChildren: tree.rootChildren,
      ...(tree.rootPager ? { rootPager: tree.rootPager } : {}),
      albums: application.albums.map((album) => ({
        id: album.id,
        name: album.name,
        photoCount: album.photoCount,
        hasSavedPosition: album.hasSavedPosition,
        active: sourceGrid.kind === "album" && sourceGrid.albumId === album.id,
      })),
    };
    view.renderSources(model);
    albumManagement.setAlbums(application.albums);
    gridMulti.renderBatchAlbums();
  };

  const albumManagement = createAlbumManagementController({
    actions: albumActions,
    albums: () => application.albums,
    source: {
      get authority() {
        return sourceGrid.authority;
      },
      current: () =>
        sourceGrid.kind === "folder" &&
        sourceGrid.folder &&
        fileLocations.publication
          ? {
              authority: sourceGrid.authority,
              path: sourceGrid.folder.location,
              publication: fileLocations.publication,
            }
          : undefined,
      isCurrent: (authority) => sourceGrid.isCurrent(authority),
    },
    photo: photoOwner,
    mutate: mutateAlbum,
    present: {
      setFormPending: (id, pending, name) =>
        view.setAlbumFormPending(id, pending, name),
      setFormMessage: (id, message) => view.setAlbumFormMessage(id, message),
      dismissForm: (id) => view.dismissAlbumForm(id),
      renderFolder: (model) => view.renderFolderAlbum(model),
    },
    onCreatedAlbum: async (album) => {
      await openSource({
        kind: "album",
        album,
        establishment: { address: "push" },
      });
    },
    onDeletedAlbum: async (albumId) => {
      if (gridMulti.expireCompensationForAlbum(albumId)) renderGrid();
      renderSources();
      if (sourceGrid.kind === "album" && sourceGrid.albumId === albumId)
        await openSource({
          kind: "library",
          establishment: { address: "replace" },
        });
    },
    onFolderChanged: renderSources,
  });

  const cancelScheduledGridRender = () => {
    view.cancelGridRender();
  };

  const openSource = async ({
    kind,
    album,
    preferredPhotoId,
    folder,
    order,
    selection,
    establishment,
  }: OpenSourceOptions) => {
    const descriptor: SourceGridSource =
      kind === "library"
        ? { kind: "library" }
        : kind === "album"
          ? { kind: "album", album: { id: album!.id, name: album!.name } }
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
    pageBusy = true;
    updateControls();
    cancelScheduledGridRender();
    metadataPanel.show(undefined);
    photoDetails.clearMetadata();
    const destinationEstablishment = navigationSession.begin({
      source: requested.kind,
      ...(requested.kind === "folder"
        ? { folderPath: requested.folder.location }
        : {}),
      ...(requested.kind === "album" ? { albumId: requested.album.id } : {}),
      ...(order !== "source-default" ? { order } : {}),
      selection,
    });
    const lifecycleOpen = sourceLifecycle.beginOpen(requested, {
      ...(preferredPhotoId ? { preferredPhotoId } : {}),
      order,
      selection,
    });
    const authority = lifecycleOpen.authority;
    const generation = lifecycleOpen.generation;
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
    gridMulti.clear();
    renderSortControl();
    try {
      const opened = await lifecycleOpen.outcome;
      if (!sourceGrid.isCurrent(authority)) return SOURCE_SUPERSEDED;
      if (opened.kind === "superseded") return SOURCE_SUPERSEDED;
      if (opened.kind === "missing") return SOURCE_MISSING;
      if (opened.kind === "failed") throw new Error("source open failed");
      const gridPosition = opened.position;
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
        ? await resolveRestorationIndex(
            sourceGrid,
            authority,
            gridPosition,
            restoration,
          )
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
        view.restoreGridAnchor(
          restorationGeometry(sourceGrid, restoreIndex, restoration),
        );
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
        navigation.openGrid(
          liveDestination(sourceGrid),
          fileLocations.publication,
        );
      else if (establishment.address === "replace")
        navigation.replaceGrid(
          liveDestination(sourceGrid),
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
      navigationSession.finish(destinationEstablishment);
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
    gridMulti.clear();
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
    const lifecycleOpen = sourceLifecycle.beginOpen(descriptor, {
      mode: "reopen",
      order: sourceGrid.order,
      selection: sourceGrid.selection,
      ...(anchorId ? { preferredPhotoId: anchorId } : {}),
    });
    const authority = lifecycleOpen.authority;
    const generation = lifecycleOpen.generation;
    // The reopen detaches the images the Grid had in flight and keeps its
    // retained cells. Binding those thumbnails again right here - from the URL
    // the owner still holds - restores them without a render, so the retained
    // range and the status line stay exactly as the reopen found them.
    view.rebindDetachedGridCells({
      total: sourceGrid.total,
      photoAt: (index) => sourceGrid.photoAt(index),
    });
    // The lifecycle owner establishes the new authority before the request
    // settles, so Photo and recovery state bind to the same generation.
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
      const opened = await lifecycleOpen.outcome;
      if (!sourceGrid.isCurrent(authority)) return;
      if (opened.kind === "superseded") return;
      if (opened.kind === "failed" || opened.kind === "missing")
        throw new Error("browse reopen failed");
      const gridPosition = opened.position;
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
        if (
          rangeRecovery?.admittedRange &&
          sourceGrid.isCurrent(rangeRecovery.admittedRange.authority)
        )
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
        multi: gridMulti.model(),
        photoAt: (index) => sourceGrid.photoAt(index),
      },
      position,
    );
    updateControls();
  };

  const openPhoto = async (
    index: number,
    address: "push" | "replace" | "none" = "push",
  ): Promise<boolean> => {
    if (!canOpenGridPhoto()) return false;
    // The anchor is read from the Grid before anything re-keys its focus or
    // hides its layout, so the Grid entry records where the Photographer left.
    navigationSession.captureGrid(view.captureGridRestoration());
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
      navigationSession.captureGrid(undefined);
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

  // Filmstrip reads loaded neighbors only. Navigating a placeholder admits
  // its window through the Photo owner; a hidden strip starts no work.
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

  const photoDetails = createPhotoDetailsOwner(fetcher, {
    isAlive: () => applicationAlive,
    currentPhoto,
    authority: () => photoOwner.authority,
    isCurrent: (authority) => photoOwner.isCurrent(authority),
    albums: () => application.albums,
    isMembershipAdmitted: (kind, albumId, photoId) =>
      albumActions.isMembershipAdmitted(kind, albumId, photoId),
    mutateAlbum: (start, authority) => mutateAlbum(start, "photo", authority),
    addMembership: (albumId, photoId, context) =>
      albumActions.addMembership(albumId, photoId, context),
    removeMembership: (albumId, photoId, context) =>
      albumActions.removeMembership(albumId, photoId, context),
    sourceAlbumId: () => sourceGrid.albumId,
    renderMembership: (model) => view.renderMembership(model),
    renderMetadata: (metadata) => view.renderPhotoMetadata(metadata),
  });
  const renderMembershipControls = (): void => photoDetails.renderMembership();
  const loadPhotoAlbums = photoDetails.loadAlbums;
  const refreshMembershipFacts = photoDetails.refreshAlbums;
  const toggleMembership = (albumId: string, member: boolean): void => {
    gridMulti.expireCompensation();
    photoDetails.toggleMembership(albumId, member, photoOwner.authority);
  };
  const loadPhotoMetadata = photoDetails.loadMetadata;

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
    metadataPanel.show(photo?.id);
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
    metadataPanel.show(undefined);
    photoDetails.clearMetadata();
    leaveEditor();
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
    if (field === "selectionState" && outcome.photoId)
      gridMulti.noteExpectedSelection(outcome.photoId, value as SelectionState);
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
      if (photoId)
        gridMulti.noteExpectedSelection(photoId, value as SelectionState);
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
  const performUndo = async () => {
    if (!connected || pageBusy) return;
    // One Undo control covers the one-level change: a pending batch restores
    // in place, and a pending single decision keeps its own path.
    if (photoOwner.undoBatch) {
      await gridMulti.performBatchUndo();
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
      if (restored)
        gridMulti.noteExpectedSelection(
          outcome.photoId,
          restored.selectionState,
        );
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
        await openSource({
          kind: "album",
          album,
          order: sourceGrid.order,
          selection: sourceGrid.selection,
          establishment: { address: "replace" },
        });
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
  const windowAnchorIndex = (windowStart: number): number =>
    rangeRecovery?.windowAnchorIndex(windowStart) ?? windowStart;
  const currentSourceRangeRetries = (
    alignedStart?: number,
  ): Array<Readonly<{ claim: RecoveryClaim; retry: GridRangeRetry }>> =>
    rangeRecovery?.currentSourceRetries(alignedStart) ?? [];
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
    const traversal = navigationSession.takeRetry();
    if (traversal) {
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

  const recoveryReview = createRecoveryReviewOwner(fetcher, view, {
    isAlive: () => applicationAlive,
    setGridStatusText,
    refreshSource,
    refreshRecoveryOverview: async () => {
      await application.refreshOverview().catch(() => {});
    },
  });

  function handleViewIntent(intent: LibraryBrowserIntent): void {
    if (!applicationAlive) return;
    switch (intent.kind) {
      // The Edit workspace owns one Photo at a time. Every editor intent
      // re-checks the owner, so a gesture or response for a Photo the
      // Photographer already left can never write or publish for it.
      case "editor-open":
        openEditor(intent.photoId);
        return;
      case "editor-refresh":
        refreshEditor(intent.photoId);
        return;
      case "editor-exposure":
        commitEditorExposure(intent.photoId, intent.exposureEv);
        return;
      case "editor-white-balance-mode":
        commitEditorWhiteBalance(intent.photoId, {
          kind: "mode",
          mode: intent.mode,
        });
        return;
      case "editor-temperature":
        commitEditorWhiteBalance(intent.photoId, {
          kind: "temperature",
          temperatureKelvin: intent.temperatureKelvin,
        });
        return;
      case "editor-tint":
        commitEditorWhiteBalance(intent.photoId, {
          kind: "tint",
          tintMilli: intent.tintMilli,
        });
        return;
      case "editor-undo":
        stepEditorHistory(intent.photoId, "undo");
        return;
      case "editor-redo":
        stepEditorHistory(intent.photoId, "redo");
        return;
      case "editor-reset":
        stepEditorHistory(intent.photoId, "reset");
        return;
      case "editor-reset-exposure":
        stepEditorHistory(intent.photoId, "resetExposure");
        return;
      case "editor-reset-white-balance":
        stepEditorHistory(intent.photoId, "resetWhiteBalance");
        return;
      case "editor-preview":
        void requestEditorPreview(intent.photoId);
        return;
      case "editor-stage":
        applyEditorStage(intent.photoId, intent.stage);
        return;
      case "editor-compare":
        setEditorComparison(intent.photoId, intent.pressed);
        return;
      case "editor-use-saved":
        void useSavedRecipe(intent.photoId);
        return;
      case "editor-reapply":
        reapplyLocalSettings(intent.photoId);
        return;
      case "editor-discard-draft":
        discardEditorDraft(intent.photoId);
        return;
      case "editor-proxy-create":
        createEditorProxy(intent.photoId);
        return;
      case "editor-proxy-remove":
        removeEditorProxy(intent.photoId);
        return;
      case "editor-rebind":
        void rebindEditor(intent.photoId);
        return;
      case "editor-export-submit":
        editor.submitExport(intent.photoId, intent.target);
        return;
      case "editor-export-cancel":
        editor.cancelExport(intent.photoId, intent.target);
        return;
      case "editor-export-retry":
        editor.retryExport(intent.photoId, intent.target);
        return;
      case "editor-export-download":
        editor.downloadExport(intent.photoId, intent.target);
        return;
      case "editor-xmp-submit":
        editor.submitXmp(intent.photoId);
        return;
      case "editor-xmp-download":
        editor.downloadXmp(intent.photoId);
        return;
      case "summary-action": {
        const outcome = applicationPresentation.activateAction(
          intent.presentationId,
        );
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
          void openSource({
            kind: "library",
            establishment: { address: "push" },
          });
        } else if (source.kind === "album") {
          const album = application.albums.find(
            (candidate) => candidate.id === source.id,
          );
          if (album)
            void openSource({
              kind: "album",
              album,
              establishment: { address: "push" },
            });
        } else if (fileLocations.publication) {
          void openSource({
            kind: "folder",
            folder: source,
            establishment: { address: "push" },
          });
        }
        return;
      }
      case "album-resume":
        void resumeAlbum(intent.albumId);
        return;
      case "explained-action":
        void openCurrentFolder();
        return;
      case "file-location-retry":
        void folders.retryKey(intent.key);
        return;
      case "folder-toggle":
        void folders.toggle(intent.location, intent.expanded);
        return;
      case "folder-page":
        void folders.page(intent.location, intent.direction);
        return;
      case "folder-album-add":
        gridMulti.expireCompensation();
        void albumManagement.addFolderToAlbum(intent.albumId);
        return;
      case "album-form-open":
        albumManagement.openForm(intent.form);
        return;
      case "album-form-close":
        albumManagement.closeForm(intent.formId);
        return;
      case "album-form-submit":
        void albumManagement.submitForm(intent.formId, intent.name);
        return;
      case "grid-render":
        renderGrid();
        return;
      case "grid-range":
        // Report only: the owner owns alignment, coalescing, and admission.
        rangeRecovery?.setAdmittedRange({
          start: intent.start,
          end: intent.end,
          authority: sourceGrid.authority,
        });
        sourceGrid.ensureRange(intent.start, intent.end, {
          // A source whose establishing window failed has an active claim and
          // placeholders still presenting its first required window. The
          // range re-admission of that window must establish readiness — the
          // same operation kind the recorded range Retry would use — or a
          // successful reload recovers the claim while every cell stays
          // disabled and no visible Retry remains.
          kind: sourceGrid.isReady(sourceGrid.authority) ? "grid" : "source",
          authority: sourceGrid.authority,
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
          gridMulti.extend(intent.index, photo.id);
          return;
        }
        if (gridMulti.model().mode || intent.toggle) {
          gridMulti.toggle(photo.id);
          return;
        }
        void openPhoto(intent.index);
        return;
      }
      case "grid-select-mode":
        gridMulti.setMode(intent.mode);
        return;
      case "grid-multi-clear": {
        const model = gridMulti.model();
        if (model.count === 0 && !model.mode) return;
        gridMulti.clear();
        renderGrid();
        return;
      }
      case "grid-batch-mutation":
        void gridMulti.mutate(intent.value);
        return;
      case "grid-batch-album-add":
        void gridMulti.addToAlbum(intent.albumId);
        return;
      case "grid-batch-album-remove":
        void gridMulti.removeAddedPhotos();
        return;
      case "grid-batch-review":
        void gridMulti.reviewChanged();
        return;
      case "removal-review-open":
        removalReview.open();
        return;
      case "removal-review-close":
        removalReview.close();
        return;
      case "removal-confirm":
        void removalReview.confirm();
        return;
      case "removal-undo":
        void removalReview.undo(intent.surface);
        return;
      case "removed-list-open":
        removedListing.openPanel();
        return;
      case "removed-list-close":
        removedListing.closePanel();
        return;
      case "removed-page":
        removedListing.turnPage(intent.direction);
        return;
      case "removed-retry":
        removedListing.retry();
        return;
      case "removed-select-all":
        void removedListing.selectAll();
        return;
      case "removed-clear-selection":
        removedListing.clearSelection();
        return;
      case "removed-toggle":
        removedListing.toggle(intent.photoId, intent.selected);
        return;
      case "removed-delete":
        void removedListing.deleteSelection();
        return;
      case "removed-restore-selected":
        void removedListing.restoreSelection();
        return;
      case "removed-restore":
        removedListing.restorePhoto(intent.photoId, intent.removedAtMs);
        return;
      case "trash-review-confirm":
        void removedListing.confirmReview();
        return;
      case "trash-review-cancel":
        removedListing.cancelReview();
        return;
      case "trash-check-result":
        void removedListing.checkOutcome();
        return;
      case "trash-retry-delete":
        void removedListing.retryDelete();
        return;
      case "trash-row-check":
        void removedListing.checkRow(intent.photoId);
        return;
      case "trash-row-resume":
        void removedListing.resumeRow(intent.photoId);
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
        void recoveryReview.openRecoveryReview();
        return;
      case "recovery-close":
        view.closeRecoveryPanel();
        return;
      case "recovery-more":
        void recoveryReview.loadMoreRecoveryEntries();
        return;
      case "recovery-mappings-more":
        void recoveryReview.loadMoreRecoveryMappings();
        return;
      case "recovery-propose":
        void recoveryReview.proposeRecoveryBatch(
          intent.oldPrefix,
          intent.newPrefix,
        );
        return;
      case "recovery-propose-single":
        void recoveryReview.proposeRecoverySingle(
          intent.originalId,
          intent.newLocation,
        );
        return;
      case "recovery-apply":
        void recoveryReview.applyRecovery(intent.items);
        return;
      case "membership-retry":
        refreshMembershipFacts();
    }
  }

  const RESTORE_REOPEN = Object.freeze({
    progress: "Reopening this source after the restore…",
    settled: "Source reopened after the restore.",
  });

  // Refresh shared facts; an explained destination without a Snapshot stays put.
  const refreshAfterLibraryChange = async (
    reason: Readonly<{ progress: string; settled: string }>,
  ): Promise<void> => {
    await application.refreshOverview().catch(() => {});
    await folders.refreshCounts();
    if (sourceGrid.token === "" || !sourceGrid.isReady(sourceGrid.authority))
      return;
    const anchor =
      sourceGrid.readGridPosition(sourceGrid.authority) ??
      photoOwner.currentIndex;
    await reopenExpired(anchor, sourceGrid.generation, undefined, reason);
  };

  const removalReview = createRemovalReviewController({
    removal,
    current: () => ({
      token: sourceGrid.token,
      total: sourceGrid.total,
      authority: sourceGrid.authority,
      selection: sourceGrid.selection === "rejected" ? "rejected" : "other",
    }),
    connected: () => connected,
    presentation: {
      render: (model) => view.renderRemovalReview(model),
      open: (model) => view.openRemovalReview(model),
      close: () => view.closeRemovalReview(),
      renderListingPending: () => removedListing.render(),
      reloadListingAfterUndo: (message) =>
        removedListing.reloadAfterUndo(message),
      presentListingMessage: (message) =>
        removedListing.presentMessage(message),
    },
    refreshAfterChange: refreshAfterLibraryChange,
    refreshOverview: async () => {
      await application.refreshOverview();
    },
    refreshFolders: () => folders.refreshCounts(),
    reopenAfterRestore: async () => {
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
    },
    updateControls,
  });

  /// An explained fallback: the confirmed invalid or missing target is named,
  /// the current entry is replaced once with All Photos, and no request is
  /// made for the invalid source.
  const fallbackToAllPhotos = async (message: string): Promise<boolean> => {
    navigation.replaceGrid(allPhotosDestination);
    const outcome = await openSource({
      kind: "library",
      establishment: { explanation: message },
    });
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
    navigationSession.requireCurrentFolder(destination.folderPath ?? "");
    // The destination shell is the Grid: Photo View must not keep presenting
    // the Photo the traversal left.
    leavePhotoView();
    view.prepareSourceOpen(sourceGrid.name);
    view.setGridExplanation(
      "This Folder changed with a newer Library publication. Open the current Folder to browse it.",
      { label: "Open current Folder" },
    );
  };

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
    navigationSession.fail(destination);
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
      ? await resolveRestorationIndex(
          sourceGrid,
          authority,
          gridPosition,
          restoration,
        )
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
      view.restoreGridAnchor(
        restorationGeometry(sourceGrid, restoreIndex, restoration),
      );
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
    if (!navigationSession.allows(destination)) return false;
    const photoId = destination.photoId;
    if (
      !options.reopen &&
      reusableForTraversal(
        sourceGrid,
        fileLocations.publication,
        destination,
        options.folderPublication,
      )
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
      const outcome = await openSource({
        kind: "library",
        preferredPhotoId: photoId,
        order: destinationOrder(destination),
        selection: destination.selection,
        establishment: options,
      });
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
      const outcome = await openSource({
        kind: "album",
        album,
        preferredPhotoId: photoId,
        order: destinationOrder(destination),
        selection: destination.selection,
        establishment: options,
      });
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
    const outcome = await openSource({
      kind: "folder",
      folder: {
        location: destination.folderPath ?? "",
        name: folderNameFor(fileLocations, destination.folderPath ?? ""),
      },
      preferredPhotoId: photoId,
      order: destinationOrder(destination),
      selection: destination.selection,
      establishment: options,
    });
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

  const applyTraversal = async (traversal: NavigationTraversal) => {
    if (!applicationAlive) return;
    navigationSession.clearRetry();
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
    const destination = liveDestination(sourceGrid, photoId);
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
    const outcome = await openSource({
      kind: "album",
      album,
      establishment: { address: alreadyCurrent ? "replace" : "push" },
    });
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
    if (
      navigation.returnToSourceGrid(liveDestination(sourceGrid)) === "traversed"
    )
      return;
    showGrid();
  };

  /// Opens the current Folder after an entry's publication changed. The action
  /// replaces the entry's provenance and adds no history loop.
  const openCurrentFolder = async (): Promise<void> => {
    const location = navigationSession.takeCurrentFolder() ?? "";
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
    photoDetails.dispose();
    metadataPanel.dispose();
    removedListing.dispose();
    removalReview.dispose();
    albumManagement.dispose();
    removal.dispose();
    view.dispose();
    cancelScheduledGridRender();
    unsubscribeWindowSettled();
    navigation.dispose();
    navigationSession.dispose();
    albumMutations.dispose();
    gridMulti.dispose();
    albumActions.dispose();
    savedPositions.dispose();
    application.dispose();
    applicationPresentation.dispose();
    folders.dispose();
    photoOwner.dispose();
    sourceGrid.dispose();
    recoveryGate.close();
  };
}
