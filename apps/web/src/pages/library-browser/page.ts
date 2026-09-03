import {
  RecoveryGate,
  type RecoveryClaim,
  type RecoveryTransition,
} from "./model/async-ownership.js";
import type {
  AlbumSummary,
  FolderChild,
  SelectionState,
} from "./api/contracts.js";
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
import {
  createAlbumActionOwner,
  type AlbumActionAdmission,
  type AlbumActionContext,
  type AlbumFormAuthority,
} from "./model/album-action-owner.js";
import { createPhotoOwner, type PhotoAuthority } from "./model/photo-owner.js";
import { createSavedPositionOwner } from "./model/saved-position-owner.js";
import {
  createLibraryBrowserView,
  type AlbumFormReference,
  type FolderViewModel,
  type LibraryBrowserIntent,
  type LibraryBrowserView,
  type SourceListViewModel,
} from "./ui/library-browser-view.js";
import { formatPhotoCount } from "./ui/photo-count.js";

type GridRangeRetry = Readonly<{
  sourceAuthority: SourceAuthority;
  operationKind: "source" | "grid";
  anchorIndex: number;
  start: number;
  quiet: boolean;
  priority: "high" | "low";
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
export function mountLibraryBrowser(
  root: HTMLElement,
  fetcher: typeof fetch = fetch,
): () => void {
  let applicationAlive = true;
  const recoveryGate = new RecoveryGate();
  const sourceGrid = createSourceGridOwner(fetcher);
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
      movePosition: (authority, index) =>
        sourceGrid.moveGridPosition(authority, index),
      patchPreview: (authority, index, photoId, preview) =>
        sourceGrid.setPhotoPreview(authority, index, photoId, preview),
      patchSelection: (authority, index, photoId, selectionState) =>
        sourceGrid.setPhotoSelection(authority, index, photoId, selectionState),
      patchRating: (authority, index, photoId, rating) =>
        sourceGrid.setPhotoRating(authority, index, photoId, rating),
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
    renderMembershipControls();
    renderSources();
  };

  const coordinateApplication = async (
    coordination: ApplicationCoordination,
  ): Promise<void> => {
    if (!applicationAlive) return;
    if (coordination.kind === "mark-reachable") {
      setConnected(true);
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
      const remembered =
        sourceGrid.lastSource ?? ({ kind: "library" } as const);
      const bindable =
        remembered.kind !== "folder" || fileLocations.publication !== undefined;
      if (bindable) {
        await openSourceDescriptor(remembered);
      } else if (coordination.isCurrent()) {
        view.setGridStatus("Could not load this source. Retry to continue.");
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
  const browseRangeFailures = new Map<string, BrowseRangeFailure>();
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
  const updateControls = () => {
    if (!applicationAlive) return;
    const photo = currentPhoto();
    const interactionBusy = pageBusy || photoRetryPending || photoOwner.busy;
    const recoveryEnabled =
      !pageBusy &&
      !photoRetryPending &&
      !photoOwner.busy &&
      !photoOwner.opening;
    const enabled = Boolean(photo) && connected && !interactionBusy;
    view.setControls({
      decisionEnabled: enabled,
      clearEnabled: enabled && photo?.selectionState !== "undecided",
      backEnabled: !interactionBusy,
      refreshEnabled: !interactionBusy,
      recoveryEnabled,
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
    announce: (text: string) => void;
    removedFromCurrentAlbum?: Readonly<{
      albumId: string;
      photoId: string;
      sourceAuthority: SourceAuthority;
    }>;
    createdAlbum?: AlbumSummary;
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
          return { admitted: true, ok: false, announce: () => {} };
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
          "Scan results changed File Locations. Reloaded the current Folders.",
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
      renderSources();
      if (
        deleted &&
        sourceGrid.kind === "album" &&
        sourceGrid.albumId === albumId
      )
        await openSource("library");
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
      await openSource("album", createdAlbum);
  };

  const cancelScheduledGridRender = () => {
    view.cancelGridRender();
  };

  const openSource = async (
    kind: "library" | "album" | "folder",
    album?: AlbumSummary,
    preferredPhotoId?: string,
    folder?: { location: string; name: string },
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
    return openSourceDescriptor(descriptor, preferredPhotoId);
  };

  async function openSourceDescriptor(
    requested: SourceGridSource,
    preferredPhotoId?: string,
  ): Promise<void> {
    const descriptor: SourceGridSource =
      requested.kind === "folder" && fileLocations.publication
        ? { ...requested, publication: fileLocations.publication }
        : requested;
    pageBusy = true;
    cancelScheduledGridRender();
    const pendingOpen = sourceGrid.open(descriptor, {
      ...(preferredPhotoId ? { preferredPhotoId } : {}),
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
    // A new open snapshot ends the previous source's removal memory.
    removedFromCurrentAlbum.clear();
    try {
      const opened = await pendingOpen;
      if (opened.kind === "detached") return;
      if (opened.kind === "publication-conflict") {
        // Only the current source's handler may reset and reload File
        // Locations: a superseded open doing the same would discard the
        // newer recovery and leave the tree unbound.
        if (!sourceGrid.isCurrent(authority)) return;
        const reboundAuthority = await rebindFileLocations();
        if (!sourceGrid.isCurrent(authority)) return;
        if (
          fileLocations.isCurrent(reboundAuthority) &&
          fileLocations.publication
        )
          claimPublicationLocationNotice(
            `publication:${fileLocations.publication}`,
            "Scan results changed File Locations. Reopen the current Folder.",
          );
        throw new Error("source open failed");
      }
      if (opened.kind === "failed") throw new Error("source open failed");
      const gridPosition = sourceGrid.readGridPosition(authority);
      if (gridPosition === undefined) return;
      photoOwner.updateSource({
        sourceAuthority: authority,
        total: sourceGrid.total,
        index: gridPosition,
        ...(sourceGrid.albumId ? { albumId: sourceGrid.albumId } : {}),
        ...(preferredPhotoId ? { preferredPhotoId } : {}),
      });
      view.scrollToGridIndex(gridPosition);
      renderSources();
      const windowReady = await loadWindow(
        gridPosition,
        { kind: "source", authority },
        false,
        "high",
        sourceTransition,
      );
      if (!sourceGrid.isCurrent(authority) || !windowReady) return;
      if (sourceGrid.kind === "folder") releasePublicationLocationRecovery();
      recoveryGate.succeedTransition(sourceTransition);
      sourceGrid.establish(authority);
      setConnected(true);
      renderGrid();
      if (sourceGrid.total) {
        view.setGridStatus(`Ready · ${formatPhotoCount(sourceGrid.total)}`);
      } else {
        view.setGridStatus(formatPhotoCount(0));
        view.setGridEmpty(emptySourceStatus(), sourceGrid.kind !== "album");
      }
    } catch {
      if (!sourceGrid.isCurrent(authority)) return;
      view.setGridStatus("Could not load this source. Retry to continue.");
      const claim = recoveryGate.issue("source-open", String(generation), {
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
  }

  const emptySourceStatus = (): string => {
    if (sourceGrid.kind === "album")
      return "This Album contains no Photos. Add Photos from another source's Photo View.";
    return "No supported Photos found. Check the Library Folder or add supported files, then run Check Library.";
  };

  const reopenExpired = async (
    anchorIndex: number,
    expectedGeneration = sourceGrid.generation,
  ) => {
    if (expectedGeneration !== sourceGrid.generation) return;
    pageBusy = true;
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
    const anchorId =
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
        view.setGridStatus("Could not load this source. Retry to continue.");
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
      ...(anchorId ? { preferredPhotoId: anchorId } : {}),
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
    const notice =
      "Library order expired. Reopening this source from the latest Library…";
    view.setGridStatus(notice);
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
              "Scan results changed File Locations. Reopen the current Folder.",
            );
        }
        throw new Error("browse reopen failed");
      }
      if (opened.kind === "failed") throw new Error("browse reopen failed");
      const gridPosition = sourceGrid.readGridPosition(authority);
      if (gridPosition === undefined) return;
      // The replacement Snapshot is now authoritative. Retain this memory when
      // reopen fails so the old recoverable view remains truthful.
      removedFromCurrentAlbum.clear();
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
      view.scrollToGridIndex(gridPosition);
      renderGrid();
      view.setGridStatus(
        "Source reopened using the latest published Library order.",
      );
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
      view.setGridStatus(failure);
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
      view.setGridStatus(
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
        const message =
          outcome.malformed === true
            ? `${outcome.range} returned an invalid response. Retry this range.`
            : outcome.transportLost
              ? `Connection lost while loading ${outcome.range}. Retry this range.`
              : `${outcome.range} could not be loaded (HTTP ${outcome.status}). Retry this range.`;
        if (ownerScope === "photo") view.setPhotoStatus(message);
        else view.setGridStatus(message);
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
              },
          transition,
        );
        return false;
      }
      recoverBrowseRange(ownerScope, ownerGeneration, outcome.start);
      if (outcome.changed) renderGrid();
      if (!quiet)
        view.setGridStatus(`Ready · ${formatPhotoCount(sourceGrid.total)}`);
      return true;
    } finally {
      if (
        outcome.kind === "detached" &&
        operation.kind === "grid" &&
        sourceGrid.isCurrent(operation.authority) &&
        view.gridVisible()
      )
        scheduleGridRender();
      updateControls();
    }
  };
  const renderGrid = (position?: number) => {
    if (!applicationAlive) return;
    sourceGrid.beginGridRender();
    view.renderGrid(
      {
        total: sourceGrid.total,
        photoAt: (index) => sourceGrid.photoAt(index),
      },
      position,
    );
    updateControls();
  };

  const openPhoto = async (index: number) => {
    if (pageBusy || photoOwner.busy || photoOwner.opening) return;
    const navigation = photoOwner.beginOpen(index);
    if (!navigation) return;
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
    } finally {
      // Back to Grid or a superseding view may end this request while an
      // unloaded boundary window is still loading. Release the open gate so
      // the interface can never remain wedged by an abandoned load.
      photoOwner.finishOpen(navigation.authority);
      updateControls();
    }
    if (!photoOwner.isCurrent(navigation.authority) || !windowReady) return;
    const current = currentPhoto();
    if (!current) return;
    const hasKnownPreview = renderPhotoShell(navigation.authority);
    updateControls();
    const previewRequest = showPreview(navigation.authority, photoTransition);
    const previewReady = hasKnownPreview || (await previewRequest);
    // A superseded open must not persist or touch controls afterwards: the
    // newer navigation persists its own position.
    if (!photoOwner.isCurrent(navigation.authority)) return;
    const positionReady = await persistPosition(navigation.authority);
    if (
      previewReady &&
      positionReady &&
      photoOwner.isCurrent(navigation.authority)
    ) {
      recoveryGate.succeedTransition(photoTransition);
      setConnected(true);
    }
    updateControls();
  };
  const renderPhotoFacts = () => {
    const photo = currentPhoto();
    view.renderPhotoFacts({
      index: photoOwner.currentIndex,
      total: sourceGrid.total,
      selectionState: photo?.selectionState,
      rating: photo?.rating,
    });
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

  // Album action ownership suppresses duplicate membership admissions. The
  // open snapshot separately remembers members removed until reopen.
  const removedFromCurrentAlbum = new Set<string>();

  const renderMembershipControls = () => {
    if (!applicationAlive) return;
    const photo = currentPhoto();
    view.renderMembership({
      albums: application.albums.map(({ id, name }) => ({ id, name })),
      photoPresent: Boolean(photo),
      addingAlbumIds: photo
        ? application.albums
            .filter((album) =>
              albumActions.isMembershipAdmitted("add", album.id, photo.id),
            )
            .map((album) => album.id)
        : [],
      inOpenAlbum:
        sourceGrid.kind === "album" &&
        Boolean(sourceGrid.albumId) &&
        Boolean(photo) &&
        !removedFromCurrentAlbum.has(photo!.id),
      removing:
        Boolean(photo && sourceGrid.albumId) &&
        albumActions.isMembershipAdmitted(
          "remove",
          sourceGrid.albumId!,
          photo!.id,
        ),
    });
  };

  const addMembership = (albumId: string): void => {
    const photo = currentPhoto();
    if (!photo || !albumId) return;
    const photoId = photo.id;
    if (albumActions.isMembershipAdmitted("add", albumId, photoId)) return;
    const photoAuthority = photoOwner.authority;
    const snapshotAuthority = sourceGrid.authority;
    void (async () => {
      const settlement = mutateAlbum(
        (context) => albumActions.addMembership(albumId, photoId, context),
        "photo",
        photoAuthority,
      );
      renderMembershipControls();
      const { ok: added, announce } = await settlement;
      if (
        added &&
        sourceGrid.isCurrent(snapshotAuthority) &&
        albumId === sourceGrid.albumId
      )
        removedFromCurrentAlbum.delete(photoId);
      renderMembershipControls();
      if (
        added &&
        photoOwner.isCurrent(photoAuthority) &&
        currentPhoto()?.id === photoId
      )
        announce("Added to the Album.");
    })();
  };

  const removeMembership = (): void => {
    const photo = currentPhoto();
    if (
      !photo ||
      sourceGrid.kind !== "album" ||
      !sourceGrid.albumId ||
      removedFromCurrentAlbum.has(photo.id)
    )
      return;
    const albumId = sourceGrid.albumId;
    const photoId = photo.id;
    if (albumActions.isMembershipAdmitted("remove", albumId, photoId)) return;
    const photoAuthority = photoOwner.authority;
    void (async () => {
      const settlement = mutateAlbum(
        (context) => albumActions.removeMembership(albumId, photoId, context),
        "photo",
        photoAuthority,
      );
      renderMembershipControls();
      const {
        ok: removed,
        announce,
        removedFromCurrentAlbum: removedFact,
      } = await settlement;
      if (
        removed &&
        removedFact !== undefined &&
        sourceGrid.isCurrent(removedFact.sourceAuthority) &&
        removedFact.albumId === sourceGrid.albumId
      )
        removedFromCurrentAlbum.add(photoId);
      renderMembershipControls();
      if (
        removed &&
        photoOwner.isCurrent(photoAuthority) &&
        currentPhoto()?.id === photoId
      )
        announce(
          "Removed from the Album. It stays in this open view until reopened.",
        );
    })();
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
      selectionState: photo?.selectionState,
      rating: photo?.rating,
      previewSource: photo?.preview.source,
      limitedDetail: photo?.preview.limitedDetail,
      previewUrl: photo?.preview.url,
    });
    if (image)
      photoOwner.attachReviewImage(
        authority,
        image.target,
        image.resolvedUrl,
        image.surface,
      );
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
      photo = sourceGrid.photoAt(index);
    }
    if (!photo || !photo.available) return;
    await photoOwner.prefetchAdjacent(authority, index);
  };
  const showGrid = () => {
    const authority = photoOwner.leave();
    const photoTransition = recoveryGate.beginTransition(
      "photo",
      photoRecoveryKey(authority),
    );
    recoveryGate.succeedTransition(photoTransition);
    setConnected(true);
    renderGrid();
    cancelScheduledGridRender();
    const gridAuthority = sourceGrid.authority;
    const gridPosition = sourceGrid.readGridPosition(gridAuthority);
    view.showGrid(gridPosition);
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
    await openPhoto(target);
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
    view.setPhotoStatus(
      `${field === "rating" ? "Rating" : "Selection"} saved.`,
    );
    if (outcome.advance) await moveTo(outcome.index + 1);
    else renderPhotoFacts();
    updateControls();
  };
  const performUndo = async () => {
    if (!connected || pageBusy) return;
    const preparation = photoOwner.prepareUndo();
    if (!preparation) return;
    updateControls();
    if (preparation.needsWindow) {
      view.setPhotoStatus("Loading Photo for Undo…");
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
    if (outcome.kind === "failed") {
      if (outcome.failure === "transport") {
        view.setPhotoStatus("Connection lost before Undo was confirmed.");
      } else if (outcome.status === 409) {
        view.setPhotoStatus(
          "Undo is no longer available because the Photo changed elsewhere. Retry to refresh its current state.",
        );
      } else {
        view.setPhotoStatus("Undo could not be saved. Try Undo again.");
      }
      if (outcome.connectivity === "lost")
        failPhotoRecovery(outcome.authority, "undo");
      updateControls();
      return;
    }
    view.enterPhoto();
    const photoTransition = recoveryGate.beginTransition(
      "photo",
      photoRecoveryKey(outcome.authority),
    );
    syncConnection();
    renderPhotoShell(outcome.authority);
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

  const scheduleGridRender = () => {
    view.scheduleGridRender();
  };

  const refreshSource = async (): Promise<void> => {
    // A Folder reopen needs the File Location binding: never send a
    // publicationless browse (it can only fail as expired/invalid).
    if (sourceGrid.kind === "folder" && !fileLocations.publication) {
      await awaitRootBinding();
      if (!fileLocations.publication) {
        view.setGridStatus("Could not load this source. Retry to continue.");
        return;
      }
    }
    if (sourceGrid.kind === "album") {
      const album = application.albums.find(
        (candidate) => candidate.id === sourceGrid.albumId,
      );
      if (album) await openSource("album", album);
      return;
    }
    await openSourceDescriptor(sourceGrid.source);
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
          view.setGridStatus("Could not load this source. Retry to continue.");
          return;
        }
      }
      await openSourceDescriptor(remembered);
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
      case "source-open": {
        const source = intent.source;
        if (source.kind === "library") {
          void openSource("library");
        } else if (source.kind === "album") {
          const album = application.albums.find(
            (candidate) => candidate.id === source.id,
          );
          if (album) void openSource("album", album);
        } else if (fileLocations.publication) {
          void openSource("folder", undefined, undefined, source);
        }
        return;
      }
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
      case "grid-resize": {
        const authority = sourceGrid.authority;
        const position = sourceGrid.readGridPosition(authority);
        if (position === undefined || !view.gridVisible()) return;
        if (sourceGrid.isCurrent(authority)) renderGrid(position);
        return;
      }
      case "grid-window":
        void loadWindow(intent.index);
        return;
      case "open-photo":
        void openPhoto(intent.index);
        return;
      case "show-grid":
        showGrid();
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
      case "membership-add":
        addMembership(intent.albumId);
        return;
      case "membership-remove":
        removeMembership();
    }
  }

  void application.loadOverview();
  return () => {
    if (!applicationAlive) return;
    applicationAlive = false;
    view.dispose();
    cancelScheduledGridRender();
    albumRecovery = undefined;
    albumActions.dispose();
    savedPositions.dispose();
    application.dispose();
    fileLocations.dispose();
    photoOwner.dispose();
    sourceGrid.dispose();
    recoveryGate.close();
  };
}
