import {
  RecoveryGate,
  type RecoveryClaim,
  type RecoveryTransition,
} from "./model/async-ownership.js";
import type {
  AlbumSummary,
  FolderChild,
  PhotoSummary,
  PreviewSource,
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
import "./ui/library-browser.css";

type GridRangeRetry = Readonly<{
  sourceAuthority: SourceAuthority;
  operationKind: "source" | "grid";
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
const GRID_CELL_HEIGHT = 178;
const GRID_CELL_WIDTH = 150;
const swipePendingPixels = 24;
const swipeCommitPixels = 72;
const swipeCommitVelocity = 0.5;

export function mountLibraryBrowser(
  root: HTMLElement,
  fetcher: typeof fetch = fetch,
): () => void {
  let applicationAlive = true;
  const recoveryGate = new RecoveryGate();
  const sourceGrid = createSourceGridOwner(fetcher);

  root.innerHTML = `
    <main class="app-shell">
      <header class="app-header"><h1>Slipstream</h1><p data-connection role="status">Connecting…</p></header>
      <section class="browser" data-browser aria-labelledby="browser-title">
        <div class="source-scrim" data-source-scrim aria-hidden="true"></div>
        <aside class="source-panel" id="source-panel" data-library-screen aria-label="Library sources">
          <header class="source-header"><h2 id="browser-title">Sources</h2><button type="button" class="quiet source-close" data-source-close>Close</button></header>
          <p data-summary-status role="status">Loading Library…</p>
          <div class="source-list" data-source-list></div>
          <footer class="source-footer"><button type="button" data-refresh>Refresh Source</button><button type="button" data-retry hidden>Retry connection</button></footer>
        </aside>
        <section class="grid-view" data-grid-view aria-labelledby="grid-title">
          <header class="grid-header"><button type="button" class="quiet source-toggle" data-source-toggle aria-controls="source-panel" aria-expanded="false">Sources</button><div><h2 id="grid-title" data-grid-title>All Photos</h2><p data-grid-status role="status"></p></div></header>
          <div class="grid-viewport" data-grid-viewport tabindex="0" aria-label="Photo Library Grid"><div class="grid-canvas" data-grid-canvas></div><div class="grid-layer" data-grid-layer></div></div>
        </section>
        <section class="photo-view" data-review data-photo-view hidden tabindex="-1" aria-labelledby="photo-title">
          <header class="photo-header"><button type="button" class="quiet" data-back>Back to Grid</button><div><h2 id="photo-title" data-photo-title>Photo</h2><p data-position>0 / 0</p></div><div class="photo-header-actions"><button type="button" class="quiet photo-source-toggle" data-photo-source-toggle aria-controls="source-panel" aria-expanded="false">Sources</button><button type="button" class="quiet" data-retry-photo hidden>Retry</button></div></header>
          <section class="preview" data-preview aria-label="Photo Preview">
            <div class="swipe-feedback reject" data-reject-feedback>Reject</div>
            <div class="image-stage" data-stage><p>Loading Preview…</p></div>
            <div class="swipe-feedback select" data-select-feedback>Select</div>
          </section>
          <section class="review-bar" aria-label="Photo review">
            <div class="review-state"><dl class="facts"><div><dt>Selection</dt><dd data-selection>Undecided</dd></div><div><dt>Rating</dt><dd data-rating>0 stars</dd></div><div><dt>Preview</dt><dd data-source>—</dd></div><div data-limited hidden><dt>Detail</dt><dd>Limited by camera Preview resolution</dd></div></dl><p class="status" data-status role="status" aria-live="polite"></p></div>
            <div class="decision-controls" aria-label="Selection controls"><button type="button" class="reject-button" data-reject>Reject <span aria-hidden="true">X</span></button><button type="button" class="quiet" data-clear>Clear <span aria-hidden="true">U</span></button><button type="button" class="select-button" data-select>Select <span aria-hidden="true">P</span></button></div>
          </section>
          <section class="review-tools" aria-label="Review tools">
            <fieldset class="rating-controls"><legend>Rating</legend><div data-ratings></div></fieldset>
            <div class="membership-controls" aria-label="Album membership"><label for="album-select">Album</label><select id="album-select" data-album-select></select><button type="button" data-add-to-album>Add to Album</button><button type="button" data-remove-from-album hidden>Remove from this Album</button></div>
            <div class="photo-controls"><button type="button" class="quiet" data-previous>Previous</button><button type="button" class="quiet" data-detail aria-pressed="false">Detail Review</button><button type="button" class="quiet" data-undo disabled>Undo</button><button type="button" class="quiet" data-next>Next</button></div>
          </section>
        </section>
      </section>
    </main>`;

  const browser = required<HTMLElement>(root, "[data-browser]");
  const sourcePanel = required<HTMLElement>(root, "#source-panel");
  const sourceToggle = required<HTMLButtonElement>(
    root,
    "[data-source-toggle]",
  );
  const photoSourceToggle = required<HTMLButtonElement>(
    root,
    "[data-photo-source-toggle]",
  );
  const sourceClose = required<HTMLButtonElement>(root, "[data-source-close]");
  const sourceScrim = required<HTMLElement>(root, "[data-source-scrim]");
  const connection = required<HTMLElement>(root, "[data-connection]");
  const summaryStatusElement = required<HTMLElement>(
    root,
    "[data-summary-status]",
  );
  const sourceList = required<HTMLElement>(root, "[data-source-list]");
  const retry = required<HTMLButtonElement>(root, "[data-retry]");
  const refresh = required<HTMLButtonElement>(root, "[data-refresh]");
  const gridView = required<HTMLElement>(root, "[data-grid-view]");
  const gridTitle = required<HTMLElement>(root, "[data-grid-title]");
  const gridStatus = required<HTMLElement>(root, "[data-grid-status]");
  const gridViewport = required<HTMLElement>(root, "[data-grid-viewport]");
  const gridCanvas = required<HTMLElement>(root, "[data-grid-canvas]");
  const gridLayer = required<HTMLElement>(root, "[data-grid-layer]");
  const photoView = required<HTMLElement>(root, "[data-photo-view]");
  const photoTitle = required<HTMLElement>(root, "[data-photo-title]");
  const position = required<HTMLElement>(root, "[data-position]");
  const stage = required<HTMLElement>(root, "[data-stage]");
  const preview = required<HTMLElement>(root, "[data-preview]");
  const selection = required<HTMLElement>(root, "[data-selection]");
  const rating = required<HTMLElement>(root, "[data-rating]");
  const source = required<HTMLElement>(root, "[data-source]");
  const limited = required<HTMLElement>(root, "[data-limited]");
  const statusElement = required<HTMLElement>(root, "[data-status]");
  // A status write replaces the Photo surface's identity. Album settlement
  // handles capture that identity instead of maintaining a second sequence.
  let photoStatusOwner: object = {};
  const status = {
    get textContent() {
      return statusElement.textContent;
    },
    set textContent(value: string) {
      if (statusElement.textContent === value) return;
      photoStatusOwner = {};
      statusElement.textContent = value;
    },
  };
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
          event.surface !== photoStatusOwner
        )
          return;
        status.textContent =
          "Preview could not be loaded. You can continue browsing.";
      },
    },
  );
  photoOwner.bindSource({
    sourceAuthority: sourceGrid.authority,
    total: sourceGrid.total,
    index: 0,
  });
  const retryPhoto = required<HTMLButtonElement>(root, "[data-retry-photo]");
  const back = required<HTMLButtonElement>(root, "[data-back]");
  const previous = required<HTMLButtonElement>(root, "[data-previous]");
  const next = required<HTMLButtonElement>(root, "[data-next]");
  const select = required<HTMLButtonElement>(root, "[data-select]");
  const albumSelect = required<HTMLSelectElement>(root, "[data-album-select]");
  const addToAlbum = required<HTMLButtonElement>(root, "[data-add-to-album]");
  const removeFromAlbum = required<HTMLButtonElement>(
    root,
    "[data-remove-from-album]",
  );
  const reject = required<HTMLButtonElement>(root, "[data-reject]");
  const clear = required<HTMLButtonElement>(root, "[data-clear]");
  const undoButton = required<HTMLButtonElement>(root, "[data-undo]");
  const detail = required<HTMLButtonElement>(root, "[data-detail]");
  const ratings = required<HTMLElement>(root, "[data-ratings]");
  const selectFeedback = required<HTMLElement>(root, "[data-select-feedback]");
  const rejectFeedback = required<HTMLElement>(root, "[data-reject-feedback]");

  const compactSources = window.matchMedia("(max-width: 760px)");
  let sourceReturn = sourceToggle;
  const syncSourcePanel = () => {
    const drawerMode = compactSources.matches || !photoView.hidden;
    const drawerOpen = drawerMode && browser.classList.contains("sources-open");
    const concealed = drawerMode && !drawerOpen;
    sourcePanel.inert = concealed;
    sourcePanel.setAttribute("aria-hidden", String(concealed));
    gridView.inert = drawerOpen;
    photoView.inert = drawerOpen;
  };
  const setSourcesExpanded = (expanded: boolean) => {
    sourceToggle.setAttribute("aria-expanded", String(expanded));
    photoSourceToggle.setAttribute("aria-expanded", String(expanded));
  };
  const openSources = (returnTo: HTMLButtonElement) => {
    sourceReturn = returnTo;
    browser.classList.add("sources-open");
    setSourcesExpanded(true);
    syncSourcePanel();
    sourceClose.focus();
  };
  const closeSources = (restoreFocus = true) => {
    browser.classList.remove("sources-open");
    setSourcesExpanded(false);
    syncSourcePanel();
    if (restoreFocus) sourceReturn.focus();
  };
  const onSourceViewportChange = () => {
    browser.classList.remove("sources-open");
    setSourcesExpanded(false);
    syncSourcePanel();
  };
  compactSources.addEventListener("change", onSourceViewportChange);
  sourceToggle.addEventListener("click", () => openSources(sourceToggle));
  photoSourceToggle.addEventListener("click", () =>
    openSources(photoSourceToggle),
  );
  sourceClose.addEventListener("click", () => closeSources());
  sourceScrim.addEventListener("click", () => closeSources());
  syncSourcePanel();

  for (let value = 0; value <= 5; value += 1) {
    const button = document.createElement("button");
    button.type = "button";
    button.dataset.ratingValue = String(value);
    button.setAttribute(
      "aria-label",
      value === 0 ? "Clear Rating" : `Rate ${value} stars`,
    );
    button.textContent = value === 0 ? "0" : `${value}★`;
    ratings.append(button);
  }

  const applicationRecoveries = new Map<ApplicationRecovery, RecoveryClaim>();

  const presentApplication = (presentation: ApplicationPresentation): void => {
    if (!applicationAlive) return;
    if (presentation.kind === "summary") {
      summaryStatusElement.replaceChildren(
        document.createTextNode(presentation.summary.text),
      );
      if (!presentation.summary.action) return;
      const action = document.createElement("button");
      action.type = "button";
      action.className = "summary-action";
      action.textContent =
        presentation.summary.action.kind === "retry-library-check"
          ? "Retry Library Check"
          : "Refresh Current Source";
      const owner = presentation.summary.action;
      action.addEventListener("click", () => {
        const intent = application.activateSummaryAction(owner);
        if (intent?.kind === "refresh-current-source") refresh.click();
      });
      summaryStatusElement.append(" ", action);
      return;
    }
    if (sourceGrid.kind === "album" && sourceGrid.albumId) {
      const open = presentation.albums.find(
        (candidate) => candidate.id === sourceGrid.albumId,
      );
      if (open) {
        sourceGrid.updateAlbum(open);
        gridTitle.textContent = sourceGrid.name;
        photoTitle.textContent = sourceGrid.name;
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
        gridStatus.textContent =
          "Could not load this source. Retry to continue.";
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
  // In-progress Album management form: create, rename, or a two-step delete
  // confirmation. Only one form exists at a time and it lives in the Albums
  // section of the source list. The current model object is the operation's
  // identity: an asynchronous continuation only clears or updates the form it
  // still owns, so a newer form is never clobbered by an older settle.
  type AlbumFormModel =
    | {
        kind: "create";
        formId: string;
        authority: AlbumFormAuthority;
        name: string;
        pending?: boolean;
        message?: string;
      }
    | {
        kind: "rename";
        formId: string;
        authority: AlbumFormAuthority;
        id: string;
        name: string;
        pending?: boolean;
        message?: string;
      }
    | {
        kind: "delete";
        formId: string;
        authority: AlbumFormAuthority;
        id: string;
        name: string;
        pending?: boolean;
      };
  let albumFormCounter = 0;
  const nextAlbumFormId = () => `album-form-${++albumFormCounter}`;
  let albumForm: AlbumFormModel | undefined;
  const dismissAlbumForm = (model: AlbumFormModel): boolean => {
    if (!albumActions.isFormCurrent(model.authority)) return false;
    albumActions.closeForm(model.authority);
    albumForm = undefined;
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
  let progressQueue: Promise<void> = Promise.resolve();
  let renderedColumns = 0;
  let renderedViewportHeight = 0;
  let gridRenderFrame: number | undefined;
  let zoomed = false;
  let panX = 0;
  let panY = 0;
  let pointer:
    | {
        id: number;
        startX: number;
        startY: number;
        lastX: number;
        lastY: number;
        startedAt: number;
        vertical: boolean;
        authority: PhotoAuthority;
        photoId: string;
      }
    | undefined;

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
    if (!connected) clearPointer();
    connection.textContent = connected ? "Connected" : "Disconnected";
    connection.classList.toggle("offline", !connected);
    retry.hidden = connected || photoOwner.active;
    retryPhoto.hidden = connected || !photoOwner.active;
    if (message) status.textContent = message;
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
            transportLost: true,
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
    if (!recoveryGate.fail(claim, { transportLost: true }))
      recoveryGate.discard(claim);
    syncConnection();
  };
  const updateControls = () => {
    if (!applicationAlive) return;
    const photo = currentPhoto();
    const interactionBusy = pageBusy || photoOwner.busy;
    const enabled = Boolean(photo) && connected && !interactionBusy;
    for (const button of [
      select,
      reject,
      ...Array.from(ratings.querySelectorAll<HTMLButtonElement>("button")),
    ])
      button.disabled = !enabled;
    clear.disabled = !enabled || photo?.selectionState === "undecided";
    back.disabled = interactionBusy;
    refresh.disabled = interactionBusy;
    previous.disabled =
      interactionBusy || photoOwner.opening || photoOwner.currentIndex <= 0;
    next.disabled =
      interactionBusy ||
      photoOwner.opening ||
      sourceGrid.total === 0 ||
      photoOwner.currentIndex >= sourceGrid.total - 1;
    undoButton.disabled =
      !connected ||
      interactionBusy ||
      photoOwner.opening ||
      !photoOwner.canUndo;
    detail.disabled = !stage.querySelector("img");
  };
  const resetTransform = () => {
    zoomed = false;
    panX = 0;
    panY = 0;
    detail.setAttribute("aria-pressed", "false");
    detail.textContent = "Detail Review";
    applyTransform();
  };
  const applyTransform = () => {
    const image = stage.querySelector<HTMLImageElement>("img");
    if (!image) return;
    image.style.transform = zoomed
      ? `translate(${panX}px, ${panY}px) scale(2)`
      : "translate(0, 0) scale(1)";
    preview.classList.toggle("detail", zoomed);
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
  }> => {
    const capturedPhotoStatus = photoStatusOwner;
    const sourceOwner = sourceGrid.authority;
    const ownsPhotoSurface = () =>
      surface === "photo" &&
      photoOwner.isCurrent(photoOwnerAuthority) &&
      photoOwner.active &&
      capturedPhotoStatus === photoStatusOwner;
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
          if (presentOnPhoto) status.textContent = outcome.failureMessage;
          else
            application.presentAlbumSummary(
              summaryPresentation,
              outcome.failureMessage,
            );
          if (presentOnPhoto)
            application.releaseAlbumSummary(summaryPresentation);
          if (
            outcome.connectivity === "lost-if-latest" &&
            albumActions.isLatest(outcome.mutation)
          )
            disconnect(outcome.sourceAuthority);
          return { admitted: true, ok: false, announce: () => {} };
        }

        let disconnectAfterRefresh = false;
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
              status.textContent = text;
          },
          ...(outcome.removedFromCurrentAlbum
            ? { removedFromCurrentAlbum: outcome.removedFromCurrentAlbum }
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

  const folderPager = (parent: string, retained: FileLocationWindow) => {
    const depth = parent ? parent.split("/").length : 0;
    const pages = Math.max(
      1,
      Math.ceil(retained.total / fileLocations.pageSize),
    );
    const controls = document.createElement("div");
    controls.className = "folder-pager";
    controls.style.marginLeft = `${Math.min(depth, 6) * 12}px`;
    const previous = document.createElement("button");
    previous.type = "button";
    previous.className = "folder-page-button";
    previous.textContent = "Previous Folders";
    previous.disabled = retained.page === 0;
    previous.addEventListener("click", () => {
      const current = fileLocations.window(parent);
      if (current && current.page > 0)
        void loadFolderWindow(parent, current.page - 1);
    });
    const label = document.createElement("span");
    label.className = "folder-page-label";
    label.textContent = `${retained.page + 1} / ${pages}`;
    const next = document.createElement("button");
    next.type = "button";
    next.className = "folder-page-button";
    next.textContent = "More Folders";
    next.disabled =
      (retained.page + 1) * fileLocations.pageSize >= retained.total;
    next.addEventListener("click", () => {
      // Read the current page at click time: the retained entry is replaced
      // by each response, so a stale closure would replay the same page.
      const current = fileLocations.window(parent);
      if (current) void loadFolderWindow(parent, current.page + 1);
    });
    controls.append(previous, label, next);
    return controls;
  };

  const folderCard = (child: FolderChild) => {
    const depth = child.location.split("/").length;
    const row = document.createElement("div");
    row.className = "folder-row folder-child";
    row.style.marginLeft = `${Math.min(depth - 1, 6) * 12}px`;
    const expand = document.createElement("button");
    expand.type = "button";
    expand.className = "folder-expand";
    expand.setAttribute(
      "aria-expanded",
      fileLocations.isExpanded(child.location) ? "true" : "false",
    );
    expand.textContent = fileLocations.isExpanded(child.location) ? "▾" : "▸";
    expand.setAttribute("aria-label", `Toggle ${child.name} subfolders`);
    expand.addEventListener("click", () => {
      if (fileLocations.isExpanded(child.location)) {
        if (fileLocations.collapse(child.location)) renderSources();
      } else {
        // Expanding revalidates against the current publication instead of
        // trusting retained state from a possibly superseded generation.
        void loadFolderWindow(child.location, 0);
      }
    });
    const button = sourceButton(
      `${child.name}${child.hasDescendantFolders ? " · Subfolders" : ""}`,
      child.photoCount,
      sourceGrid.kind === "folder" &&
        sourceGrid.folder?.location === child.location,
      false,
      false,
    );
    // Folder sources cannot open before a publication is bound.
    button.disabled = !fileLocations.publication;
    button.addEventListener(
      "click",
      () =>
        void openSource("folder", undefined, undefined, {
          location: child.location,
          name: child.name,
        }),
    );
    if (!child.hasDescendantFolders) expand.disabled = true;
    row.append(expand, button);
    if (!fileLocations.isExpanded(child.location)) return row;
    const fragment = document.createDocumentFragment();
    fragment.append(row, folderChildrenFragment(child.location));
    return fragment;
  };

  const folderChildrenFragment = (parent: string) => {
    const retained = fileLocations.window(parent);
    if (!retained || !fileLocations.isExpanded(parent))
      return document.createDocumentFragment();
    const fragment = document.createDocumentFragment();
    for (const child of retained.children) fragment.append(folderCard(child));
    if (retained.total > fileLocations.pageSize)
      fragment.append(folderPager(parent, retained));
    return fragment;
  };

  const renderSources = () => {
    if (!applicationAlive) return;
    // Background refreshes rebuild the source list; an Album form input that
    // held focus keeps its focus and caret through the rebuild.
    const focused = document.activeElement;
    const focusedFormId =
      focused instanceof HTMLInputElement
        ? focused.dataset.albumFormId
        : undefined;
    const focusedSelection =
      focused instanceof HTMLInputElement
        ? [focused.selectionStart, focused.selectionEnd]
        : undefined;
    sourceList.replaceChildren();
    const libraryButton = sourceButton(
      "All Photos",
      application.overview?.photoCount ?? 0,
      sourceGrid.kind === "library",
    );
    libraryButton.addEventListener("click", () => void openSource("library"));
    sourceList.append(libraryButton);

    const fileHeading = document.createElement("h3");
    fileHeading.textContent = "File Locations";
    sourceList.append(fileHeading);
    for (const failure of fileLocations.failures()) {
      const retryFolders = document.createElement("button");
      retryFolders.type = "button";
      retryFolders.className = "folder-more";
      retryFolders.textContent = `Retry File Locations (${failure.range})`;
      retryFolders.addEventListener("click", () => {
        void (async () => {
          await handleFileLocationOutcome(await fileLocations.retry(failure));
        })();
      });
      sourceList.append(retryFolders);
    }
    const rootCard = sourceButton(
      "Library Folder",
      application.overview?.photoCount ?? 0,
      sourceGrid.kind === "folder" && sourceGrid.folder?.location === "",
      false,
      false,
    );
    rootCard.addEventListener(
      "click",
      () =>
        void openSource("folder", undefined, undefined, {
          location: "",
          name: "Library Folder",
        }),
    );
    // The root source cannot open before a publication is bound.
    rootCard.disabled = !fileLocations.publication;
    const rootRow = document.createElement("div");
    rootRow.className = "folder-row folder-root";
    const rootExpand = document.createElement("button");
    rootExpand.type = "button";
    rootExpand.className = "folder-expand";
    rootExpand.setAttribute(
      "aria-expanded",
      fileLocations.isExpanded("") ? "true" : "false",
    );
    rootExpand.textContent = fileLocations.isExpanded("") ? "▾" : "▸";
    rootExpand.setAttribute("aria-label", "Toggle Library Folder subfolders");
    rootExpand.addEventListener("click", () => {
      if (fileLocations.isExpanded("")) {
        if (fileLocations.collapse("")) renderSources();
      } else {
        void loadFolderWindow("", 0);
      }
    });
    rootRow.append(rootExpand, rootCard);
    sourceList.append(rootRow, folderChildrenFragment(""));

    const albumHeadingRow = document.createElement("div");
    albumHeadingRow.className = "album-heading";
    const albumHeading = document.createElement("h3");
    albumHeading.textContent = "Albums";
    const newAlbum = document.createElement("button");
    newAlbum.type = "button";
    newAlbum.className = "album-new";
    newAlbum.textContent = "New Album";
    newAlbum.addEventListener("click", () => {
      const formId = nextAlbumFormId();
      albumForm = {
        kind: "create",
        formId,
        authority: albumActions.openForm(formId),
        name: "",
      };
      renderSources();
    });
    albumHeadingRow.append(albumHeading, newAlbum);
    sourceList.append(albumHeadingRow);

    if (albumForm?.kind === "create") {
      sourceList.append(albumCreateForm());
    }
    for (const set of application.albums) {
      const button = sourceButton(
        set.name,
        set.photoCount,
        sourceGrid.kind === "album" && sourceGrid.albumId === set.id,
        set.hasSavedPosition,
        false,
      );
      button.addEventListener("click", () => void openSource("album", set));
      const row = document.createElement("div");
      row.className = "album-row";
      row.append(button, albumTools(set));
      sourceList.append(row);
    }
    if (focusedFormId) {
      const restored = sourceList.querySelector<HTMLInputElement>(
        `input[data-album-form-id="${focusedFormId}"]`,
      );
      if (restored) {
        const end = restored.value.length;
        const start = Math.min(focusedSelection?.[0] ?? end, end);
        const finish = Math.min(focusedSelection?.[1] ?? end, end);
        restored.focus();
        restored.setSelectionRange(start, finish);
      }
    }
  };

  const albumNameInput = (
    model: Extract<AlbumFormModel, { kind: "create" | "rename" }>,
    initial: string,
  ) => {
    const input = document.createElement("input");
    input.type = "text";
    input.name = "name";
    input.dataset.albumFormId = model.formId;
    input.setAttribute("aria-label", "Album name");
    // Keep the model's draft current so a re-render during editing preserves
    // exactly what the user typed, and validate by Unicode code points like
    // the server instead of native UTF-16 code-unit maxlength.
    const draft = model;
    input.addEventListener("input", () => {
      if (albumActions.isFormCurrent(draft.authority)) {
        draft.name = input.value;
        if (draft.message) delete draft.message;
      }
    });
    input.value = initial;
    return input;
  };

  const albumFormMessage = () => {
    const message = document.createElement("p");
    message.className = "album-form-message";
    message.setAttribute("role", "alert");
    return message;
  };

  const albumCreateForm = () => {
    const model = albumForm as Extract<AlbumFormModel, { kind: "create" }>;
    const form = document.createElement("form");
    form.className = "album-form";
    form.setAttribute("aria-label", "Create Album");
    const input = albumNameInput(model, model.name);
    const message = albumFormMessage();
    message.textContent = model.message ?? "";
    const save = document.createElement("button");
    save.type = "submit";
    save.textContent = "Create Album";
    save.disabled = Boolean(model.pending);
    const abort = document.createElement("button");
    abort.type = "button";
    abort.textContent = "Cancel";
    abort.addEventListener("click", () => {
      dismissAlbumForm(model);
      renderSources();
    });
    form.append(input, save, abort, message);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = input.value.trim();
      const invalid = albumNameError(name);
      if (invalid) {
        // Stored in the model so a background re-render cannot erase it.
        model.message = invalid;
        message.textContent = invalid;
        return;
      }
      delete model.message;
      message.textContent = "";
      model.name = name;
      model.pending = true;
      renderSources();
      void (async () => {
        const { ok: created } = await mutateAlbum(
          (context) => albumActions.create(name, context),
          "summary",
          photoOwner.authority,
          model.authority,
        );
        if (albumActions.isFormCurrent(model.authority)) {
          if (created) dismissAlbumForm(model);
          else model.pending = false;
        }
        renderSources();
      })();
    });
    return form;
  };

  const albumTools = (set: AlbumSummary) => {
    const tools = document.createElement("div");
    tools.className = "album-tools";
    if (albumForm?.kind === "rename" && albumForm.id === set.id) {
      const model = albumForm;
      const form = document.createElement("form");
      form.className = "album-form";
      form.setAttribute("aria-label", "Rename Album");
      const input = albumNameInput(model, model.name);
      const message = albumFormMessage();
      message.textContent = model.message ?? "";
      const save = document.createElement("button");
      save.type = "submit";
      save.textContent = "Save Name";
      save.disabled = Boolean(model.pending);
      const abort = document.createElement("button");
      abort.type = "button";
      abort.textContent = "Cancel";
      abort.addEventListener("click", () => {
        dismissAlbumForm(model);
        renderSources();
      });
      form.append(input, save, abort, message);
      form.addEventListener("submit", (event) => {
        event.preventDefault();
        const name = input.value.trim();
        if (!name || name === set.name) {
          dismissAlbumForm(model);
          renderSources();
          return;
        }
        const invalid = albumNameError(name);
        if (invalid) {
          // Stored in the model so a background re-render cannot erase it.
          model.message = invalid;
          message.textContent = invalid;
          return;
        }
        delete model.message;
        message.textContent = "";
        model.name = name;
        model.pending = true;
        renderSources();
        void (async () => {
          const { ok: renamed } = await mutateAlbum(
            (context) => albumActions.rename(set.id, name, context),
            "summary",
            photoOwner.authority,
            model.authority,
          );
          if (albumActions.isFormCurrent(model.authority)) {
            if (renamed) dismissAlbumForm(model);
            else model.pending = false;
          }
          renderSources();
        })();
      });
      tools.append(form);
      return tools;
    }
    if (albumForm?.kind === "delete" && albumForm.id === set.id) {
      const model: AlbumFormModel = albumForm;
      const confirmBox = document.createElement("div");
      confirmBox.className = "album-confirm";
      confirmBox.setAttribute("role", "alert");
      const text = document.createElement("p");
      text.textContent = "Photos and Original Files remain unchanged.";
      const confirmButton = document.createElement("button");
      confirmButton.type = "button";
      confirmButton.textContent = "Delete Album";
      confirmButton.disabled = Boolean(model.pending);
      const abort = document.createElement("button");
      abort.type = "button";
      abort.textContent = "Cancel";
      abort.addEventListener("click", () => {
        dismissAlbumForm(model);
        renderSources();
      });
      confirmButton.addEventListener("click", () => {
        void (async () => {
          confirmButton.disabled = true;
          model.pending = true;
          const { ok: deleted } = await mutateAlbum(
            (context) => albumActions.delete(set.id, context),
            "summary",
            photoOwner.authority,
            model.authority,
          );
          if (albumActions.isFormCurrent(model.authority))
            dismissAlbumForm(model);
          renderSources();
          if (
            deleted &&
            sourceGrid.kind === "album" &&
            sourceGrid.albumId === set.id
          ) {
            // The open source object is gone; return to the system source.
            await openSource("library");
          }
        })();
      });
      confirmBox.append(text, confirmButton, abort);
      tools.append(confirmBox);
      return tools;
    }
    const rename = document.createElement("button");
    rename.type = "button";
    rename.className = "album-tool";
    rename.textContent = "Rename";
    rename.setAttribute("aria-label", `Rename ${set.name}`);
    rename.addEventListener("click", () => {
      const formId = nextAlbumFormId();
      albumForm = {
        kind: "rename",
        formId,
        authority: albumActions.openForm(formId),
        id: set.id,
        name: set.name,
      };
      renderSources();
    });
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "album-tool";
    remove.textContent = "Delete";
    remove.setAttribute("aria-label", `Delete ${set.name}`);
    remove.addEventListener("click", () => {
      const formId = nextAlbumFormId();
      albumForm = {
        kind: "delete",
        formId,
        authority: albumActions.openForm(formId),
        id: set.id,
        name: set.name,
      };
      renderSources();
    });
    tools.append(rename, remove);
    return tools;
  };
  const sourceButton = (
    name: string,
    count: number,
    active: boolean,
    saved = false,
    disableWhenEmpty = true,
  ) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `source-card${active ? " active" : ""}`;
    button.disabled = disableWhenEmpty && count === 0;
    button.innerHTML = "<strong></strong><span></span>";
    button.querySelector("strong")!.textContent = name;
    button.querySelector("span")!.textContent =
      `${count} Photos${saved ? " · Resume" : ""}`;
    return button;
  };

  const cancelScheduledGridRender = () => {
    if (gridRenderFrame === undefined) return;
    cancelAnimationFrame(gridRenderFrame);
    gridRenderFrame = undefined;
  };

  const openSource = async (
    kind: "library" | "album" | "folder",
    set?: AlbumSummary,
    preferredPhotoId?: string,
    folder?: { location: string; name: string },
  ) => {
    const descriptor: SourceGridSource =
      kind === "library"
        ? { kind: "library" }
        : kind === "album"
          ? {
              kind: "album",
              album: { id: set!.id, name: set!.name },
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
    const sourceDrawerWasOpen = browser.classList.contains("sources-open");
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
    gridLayer.replaceChildren();
    stage.replaceChildren();
    gridView.hidden = false;
    photoView.hidden = true;
    closeSources(false);
    if (sourceDrawerWasOpen) gridViewport.focus();
    // A new open snapshot ends the previous source's removal memory.
    removedFromCurrentAlbum.clear();
    gridTitle.textContent = sourceGrid.name;
    gridStatus.textContent = "Preparing Library order…";
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
      gridViewport.scrollTop =
        Math.floor(gridPosition / columns()) * GRID_CELL_HEIGHT;
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
      gridStatus.textContent = sourceGrid.total
        ? `Ready · ${sourceGrid.total.toLocaleString()} Photos`
        : "No Photos in this source";
    } catch {
      if (!sourceGrid.isCurrent(authority)) return;
      gridStatus.textContent = "Could not load this source. Retry to continue.";
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

  const columns = () =>
    Math.max(
      1,
      Math.floor(Math.max(320, gridViewport.clientWidth) / GRID_CELL_WIDTH),
    );
  const effectiveViewportHeight = () =>
    Math.max(360, Math.min(gridViewport.clientHeight, window.innerHeight));
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
        gridStatus.textContent =
          "Could not load this source. Retry to continue.";
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
    gridStatus.textContent = notice;
    status.textContent = notice;
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
      gridViewport.scrollTop =
        Math.floor(gridPosition / columns()) * GRID_CELL_HEIGHT;
      renderGrid();
      gridStatus.textContent =
        "Source reopened using the latest published Library order.";
      if (sourceGrid.kind === "folder") releasePublicationLocationRecovery();
      recoveryGate.succeedTransition(sourceTransition);
      sourceGrid.establish(authority);
      setConnected(true);
      if (resumePhoto && photoOwner.isCurrent(photoAuthority)) {
        gridView.hidden = true;
        photoView.hidden = false;
        syncSourcePanel();
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
      gridStatus.textContent = failure;
      status.textContent = failure;
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
      gridStatus.textContent = `Loading ${range} of ${sourceGrid.total.toLocaleString()}…`;
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
        if (ownerScope === "photo") status.textContent = message;
        else gridStatus.textContent = message;
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
        gridStatus.textContent = `Ready · ${sourceGrid.total.toLocaleString()} Photos`;
      return true;
    } finally {
      if (
        outcome.kind === "detached" &&
        operation.kind === "grid" &&
        sourceGrid.isCurrent(operation.authority) &&
        !gridView.hidden
      )
        scheduleGridRender();
      updateControls();
    }
  };
  const syncGridHeight = (count: number) => {
    const height = `${Math.ceil(sourceGrid.total / count) * GRID_CELL_HEIGHT}px`;
    gridCanvas.style.height = height;
    gridLayer.style.height = height;
  };
  const renderGrid = () => {
    if (!applicationAlive) return;
    const count = columns();
    const viewportHeight = effectiveViewportHeight();
    renderedColumns = count;
    renderedViewportHeight = viewportHeight;
    syncGridHeight(count);
    const firstRow = Math.max(
      0,
      Math.floor(gridViewport.scrollTop / GRID_CELL_HEIGHT) - 2,
    );
    const visibleRows = Math.ceil(viewportHeight / GRID_CELL_HEIGHT) + 4;
    const start = firstRow * count;
    const end = Math.min(sourceGrid.total, start + visibleRows * count);
    sourceGrid.beginGridRender();
    gridLayer.replaceChildren();
    for (let index = start; index < end; index += 1) {
      const cell = document.createElement("button");
      cell.type = "button";
      cell.className = "photo-cell";
      cell.style.left = `${(index % count) * GRID_CELL_WIDTH}px`;
      cell.style.top = `${Math.floor(index / count) * GRID_CELL_HEIGHT}px`;
      const photo = sourceGrid.photoAt(index);
      if (!photo) {
        cell.disabled = true;
        cell.textContent = "Loading…";
        void loadWindow(index);
      } else {
        cell.dataset.photoIndex = String(index);
        renderCell(cell, photo, index);
        cell.addEventListener("click", () => void openPhoto(index));
      }
      gridLayer.append(cell);
    }
    updateControls();
  };
  const renderCell = (
    cell: HTMLButtonElement,
    photo: PhotoSummary,
    index: number,
  ) => {
    const image = document.createElement("img");
    image.alt = `Photo ${index + 1} of ${sourceGrid.total}`;
    image.loading = "lazy";
    image.fetchPriority = "low";
    image.decoding = "async";
    image.draggable = false;
    image.className = "thumbnail";
    cell.append(image);
    const badge = document.createElement("span");
    badge.className = `cell-state ${photo.selectionState}`;
    badge.textContent =
      photo.selectionState === "undecided"
        ? ""
        : photo.selectionState === "selected"
          ? "✓"
          : "×";
    cell.append(badge);
    const caption = document.createElement("span");
    caption.className = "cell-caption";
    caption.textContent = photo.rating
      ? `${index + 1} · ${photo.rating}★`
      : String(index + 1);
    cell.append(caption);
    if (photo.preview.state === "unavailable") {
      sourceGrid.markThumbnailUnavailable(photo.id, image);
    } else if (photo.preview.thumbnailUrl) {
      sourceGrid.presentThumbnail(
        photo.id,
        image,
        photo.preview.thumbnailUrl,
        true,
      );
    } else {
      void sourceGrid.loadThumbnail(photo.id, image);
    }
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
    gridView.hidden = true;
    photoView.hidden = false;
    syncSourcePanel();
    photoView.focus();
    resetTransform();
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
    position.textContent = `${photoOwner.currentIndex + 1} / ${sourceGrid.total}`;
    selection.textContent = selectionLabel(photo?.selectionState);
    rating.textContent = `${photo?.rating ?? 0} ${(photo?.rating ?? 0) === 1 ? "star" : "stars"}`;
    updateControls();
  };
  const renderReviewImage = (url: string, authority = photoOwner.authority) => {
    const capturedStatus = photoStatusOwner;
    const image = document.createElement("img");
    image.alt = `Photo ${photoOwner.currentIndex + 1} of ${sourceGrid.total}`;
    image.draggable = false;
    image.fetchPriority = "high";
    image.decoding = "async";
    stage.replaceChildren(image);
    const resolvedUrl = new URL(url, window.location.href).href;
    photoOwner.attachReviewImage(
      authority,
      {
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
        setSource(next) {
          image.src = next;
        },
        clearSource() {
          image.removeAttribute("src");
        },
      },
      resolvedUrl,
      capturedStatus,
    );
  };
  // Album action ownership suppresses duplicate membership admissions. The
  // open snapshot separately remembers members removed until reopen.
  const removedFromCurrentAlbum = new Set<string>();
  let selectedAlbumId = "";

  /// Populates the current-Photo Album membership controls.
  const renderMembershipControls = () => {
    if (!applicationAlive) return;
    albumSelect.replaceChildren();
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = application.albums.length
      ? "Choose Album…"
      : "No Albums yet";
    placeholder.disabled = true;
    if (!application.albums.some((set) => set.id === selectedAlbumId))
      selectedAlbumId = "";
    placeholder.selected = selectedAlbumId === "";
    albumSelect.append(placeholder);
    for (const set of application.albums) {
      const option = document.createElement("option");
      option.value = set.id;
      option.textContent = set.name;
      option.selected = set.id === selectedAlbumId;
      albumSelect.append(option);
    }
    const photo = currentPhoto();
    const adding = Boolean(
      photo &&
        selectedAlbumId &&
        albumActions.isMembershipAdmitted("add", selectedAlbumId, photo.id),
    );
    const removing =
      Boolean(photo && sourceGrid.albumId) &&
      albumActions.isMembershipAdmitted(
        "remove",
        sourceGrid.albumId!,
        photo!.id,
      );
    const inOpenAlbum =
      sourceGrid.kind === "album" &&
      Boolean(sourceGrid.albumId) &&
      Boolean(photo) &&
      !removedFromCurrentAlbum.has(photo!.id);
    albumSelect.disabled = !application.albums.length;
    addToAlbum.disabled =
      !application.albums.length || !photo || !selectedAlbumId || adding;
    removeFromAlbum.hidden = !inOpenAlbum;
    removeFromAlbum.disabled = !inOpenAlbum || removing;
  };

  albumSelect.addEventListener("change", () => {
    selectedAlbumId = albumSelect.value;
    renderMembershipControls();
  });

  addToAlbum.addEventListener("click", () => {
    const photo = currentPhoto();
    const albumId = albumSelect.value;
    if (!photo || !albumId) return;
    const photoId = photo.id;
    if (albumActions.isMembershipAdmitted("add", albumId, photoId)) return;
    const photoAuthority = photoOwner.authority;
    const snapshotAuthority = sourceGrid.authority;
    void (async () => {
      addToAlbum.disabled = true;
      const { ok: added, announce } = await mutateAlbum(
        (context) => albumActions.addMembership(albumId, photoId, context),
        "photo",
        photoAuthority,
      );
      // Re-adding to the open Album clears the retained snapshot's removal
      // mark: the Photo is a member again and may be removed once more.
      if (
        added &&
        sourceGrid.isCurrent(snapshotAuthority) &&
        albumId === sourceGrid.albumId
      ) {
        removedFromCurrentAlbum.delete(photoId);
      }
      // Admitted persistence is never aborted by a source change; the
      // continuation only touches this Photo's controls while it is still
      // the current Photo of the initiating generation.
      if (
        photoOwner.isCurrent(photoAuthority) &&
        currentPhoto()?.id === photoId
      ) {
        renderMembershipControls();
        if (added) announce("Added to the Album.");
      } else {
        renderMembershipControls();
      }
    })();
  });

  removeFromAlbum.addEventListener("click", () => {
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
      removeFromAlbum.disabled = true;
      const {
        ok: removed,
        announce,
        removedFromCurrentAlbum: removedFact,
      } = await mutateAlbum(
        (context) => albumActions.removeMembership(albumId, photoId, context),
        "photo",
        photoAuthority,
      );
      if (
        removed &&
        removedFact !== undefined &&
        sourceGrid.isCurrent(removedFact.sourceAuthority) &&
        removedFact.albumId === sourceGrid.albumId
      ) {
        // The member is gone from the Album; within this open snapshot it
        // must not be removable again. A removal settling after the source
        // changed must not mark the Photo in a different Album's snapshot.
        removedFromCurrentAlbum.add(photoId);
      }
      // Recomputing the controls re-enables removal after a failure so the
      // failed action stays retryable.
      if (
        photoOwner.isCurrent(photoAuthority) &&
        currentPhoto()?.id === photoId
      ) {
        renderMembershipControls();
        if (removed)
          announce(
            "Removed from the Album. It stays in this open view until reopened.",
          );
      } else {
        renderMembershipControls();
      }
    })();
  });

  const renderPhotoShell = (authority = photoOwner.authority): boolean => {
    const photo = currentPhoto();
    photoTitle.textContent = sourceGrid.name;
    renderMembershipControls();
    renderPhotoFacts();
    source.textContent = photo?.preview.source
      ? sourceLabel(photo.preview.source)
      : "—";
    limited.hidden = !photo?.preview.limitedDetail;
    const knownUrl = photo?.preview.url;
    if (knownUrl) {
      renderReviewImage(knownUrl, authority);
    } else {
      stage.replaceChildren(
        paragraph(photo ? "Loading Preview…" : "Photo unavailable"),
      );
    }
    status.textContent =
      photo && !photo.available
        ? "Original File is unavailable. Decisions remain available."
        : "";
    updateControls();
    return Boolean(knownUrl);
  };
  const showPreview = async (
    authority: PhotoAuthority,
    transition?: RecoveryTransition,
  ): Promise<boolean> => {
    const capturedStatus = photoStatusOwner;
    const outcome = await photoOwner.loadCurrentPreview(authority);
    if (outcome.kind === "detached") return false;
    if (outcome.kind === "failed") {
      if (capturedStatus === photoStatusOwner)
        status.textContent = "Connection lost. Retry to refresh this Photo.";
      failPhotoRecovery(authority, "preview", transition);
      return false;
    }
    if (!photoOwner.isCurrent(authority)) return false;
    const existingImage = stage.querySelector<HTMLImageElement>("img");
    if (outcome.kind === "not-ready") {
      if (capturedStatus === photoStatusOwner)
        status.textContent = outcome.preview.message ?? "Preview unavailable";
      if (!existingImage)
        stage.replaceChildren(paragraph("Preview unavailable"));
      return true;
    }
    const result = outcome.preview;
    const resolvedUrl = new URL(result.url, window.location.href).href;
    if (!existingImage || existingImage.src !== resolvedUrl)
      renderReviewImage(result.url, authority);
    source.textContent = sourceLabel(result.source);
    limited.hidden = !result.limitedDetail;
    if (capturedStatus === photoStatusOwner)
      status.textContent = result.stale
        ? (result.message ?? "Showing a stale Preview.")
        : "";
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
    photoView.hidden = true;
    gridView.hidden = false;
    closeSources(false);
    renderGrid();
    cancelScheduledGridRender();
    const gridAuthority = sourceGrid.authority;
    scheduleGridRender(() => {
      const gridPosition = sourceGrid.readGridPosition(gridAuthority);
      if (gridPosition === undefined) return;
      gridViewport.scrollTop =
        Math.floor(gridPosition / columns()) * GRID_CELL_HEIGHT;
      renderGrid();
    });
    updateControls();
  };
  const persistPosition = (
    photoAuthority = photoOwner.authority,
  ): Promise<boolean> => {
    if (sourceGrid.kind !== "album" || !sourceGrid.albumId || !currentPhoto())
      return Promise.resolve(true);
    const albumId = sourceGrid.albumId;
    const photoId = currentPhoto()!.id;
    const sourceAuthority = sourceGrid.authority;
    const stillCurrent = () =>
      applicationAlive &&
      sourceGrid.isCurrent(sourceAuthority) &&
      photoOwner.isCurrent(photoAuthority) &&
      sourceGrid.kind === "album" &&
      sourceGrid.albumId === albumId &&
      currentPhoto()?.id === photoId;
    const task = progressQueue.then(async () => {
      if (!stillCurrent()) return false;
      try {
        const response = await fetcher(`/api/albums/${albumId}/progress`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ photoId }),
        });
        if (response.status === 404 || response.status === 409) {
          // The saved-position member is gone or contested: an answered,
          // expected stale write — not a connectivity loss.
          return false;
        }
        if (!response.ok) throw new Error("position rejected");
        if (!stillCurrent()) return false;
        application.confirmSavedPosition(albumId);
        renderSources();
        return true;
      } catch {
        if (stillCurrent()) {
          status.textContent =
            "Album position could not be saved. Retry before making more decisions.";
          failPhotoRecovery(photoAuthority, "saved-position");
        }
        return false;
      }
    });
    progressQueue = task.then(() => undefined);
    return task;
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
    status.textContent = `Saving ${field === "rating" ? "Rating" : "Selection State"}…`;
    updateControls();
    const outcome = await admission.settlement;
    if (outcome.kind === "detached") return;
    if (outcome.kind === "failed") {
      if (outcome.failure === "answered") {
        status.textContent =
          outcome.status === 409
            ? "The Photo changed elsewhere. Retry to refresh its current state."
            : "The change could not be saved.";
      } else {
        status.textContent =
          "Connection lost before the change was confirmed. Retry to refresh.";
      }
      if (outcome.connectivity === "lost")
        failPhotoRecovery(outcome.authority, "photo-write");
      updateControls();
      return;
    }
    if (!outcome.applied) return;
    status.textContent = `${field === "rating" ? "Rating" : "Selection"} saved.`;
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
      status.textContent = "Loading Photo for Undo…";
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
        status.textContent = "Connection lost before Undo was confirmed.";
      } else if (outcome.status === 409) {
        status.textContent =
          "Undo is no longer available because the Photo changed elsewhere. Retry to refresh its current state.";
      } else {
        status.textContent = "Undo could not be saved. Try Undo again.";
      }
      if (outcome.connectivity === "lost")
        failPhotoRecovery(outcome.authority, "undo");
      updateControls();
      return;
    }
    gridView.hidden = true;
    photoView.hidden = false;
    syncSourcePanel();
    photoView.focus();
    resetTransform();
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
      status.textContent = "Last change undone.";
    }
    updateControls();
  };

  const pointerDown = (event: PointerEvent) => {
    const photo = currentPhoto();
    if (
      pointer ||
      !event.isPrimary ||
      pageBusy ||
      photoOwner.busy ||
      !connected ||
      !photo
    )
      return;
    pointer = {
      id: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      startedAt: event.timeStamp,
      vertical: false,
      authority: photoOwner.authority,
      photoId: photo.id,
    };
    preview.setPointerCapture(event.pointerId);
  };
  const clearPointer = () => {
    const id = pointer?.id;
    pointer = undefined;
    if (id !== undefined && preview.hasPointerCapture(id))
      preview.releasePointerCapture(id);
    stage.style.transform = "";
    selectFeedback.classList.remove("pending");
    rejectFeedback.classList.remove("pending");
  };
  const pointerMove = (event: PointerEvent) => {
    if (!pointer || pointer.id !== event.pointerId) return;
    const dx = event.clientX - pointer.startX;
    const dy = event.clientY - pointer.startY;
    const stepX = event.clientX - pointer.lastX;
    const stepY = event.clientY - pointer.lastY;
    pointer.lastX = event.clientX;
    pointer.lastY = event.clientY;
    if (zoomed) {
      panX = clamp(panX + stepX, -stage.clientWidth / 2, stage.clientWidth / 2);
      panY = clamp(
        panY + stepY,
        -stage.clientHeight / 2,
        stage.clientHeight / 2,
      );
      applyTransform();
      return;
    }
    if (Math.abs(dy) > Math.abs(dx) && Math.abs(dy) > 12)
      pointer.vertical = true;
    if (pointer.vertical) return;
    stage.style.transform = `translateX(${clamp(dx, -140, 140)}px)`;
    selectFeedback.classList.toggle("pending", dx > swipePendingPixels);
    rejectFeedback.classList.toggle("pending", dx < -swipePendingPixels);
  };
  const finishPointer = (event: PointerEvent, cancelled = false) => {
    if (!pointer || pointer.id !== event.pointerId) return;
    const active = pointer;
    clearPointer();
    if (
      zoomed ||
      cancelled ||
      active.vertical ||
      !connected ||
      !photoOwner.isCurrent(active.authority) ||
      currentPhoto()?.id !== active.photoId
    )
      return;
    const dx = event.clientX - active.startX;
    const elapsed = Math.max(1, event.timeStamp - active.startedAt);
    const velocity = Math.abs(dx) / elapsed;
    if (
      Math.abs(dx) >= swipeCommitPixels ||
      (Math.abs(dx) >= 48 && velocity >= swipeCommitVelocity)
    )
      void mutate("selectionState", dx > 0 ? "selected" : "rejected", true);
  };
  const keydown = (event: KeyboardEvent) => {
    const target = event.target as HTMLElement | null;
    if (
      event.isComposing ||
      target?.matches("input, textarea, select, [contenteditable=true]") ||
      target?.isContentEditable ||
      event.altKey
    )
      return;
    if (
      event.key === "Escape" &&
      (compactSources.matches || !photoView.hidden) &&
      browser.classList.contains("sources-open")
    ) {
      event.preventDefault();
      closeSources();
      return;
    }
    const modifier = event.ctrlKey || event.metaKey;
    if (modifier && !event.shiftKey && event.key.toLowerCase() === "z") {
      event.preventDefault();
      void performUndo();
      return;
    }
    if (modifier || event.shiftKey || !photoOwner.active) return;
    if (event.key === "ArrowLeft") void moveTo(photoOwner.currentIndex - 1);
    else if (event.key === "ArrowRight")
      void moveTo(photoOwner.currentIndex + 1);
    else if (event.key.toLowerCase() === "p")
      void mutate("selectionState", "selected", true);
    else if (event.key.toLowerCase() === "x")
      void mutate("selectionState", "rejected", true);
    else if (
      event.key.toLowerCase() === "u" &&
      currentPhoto()?.selectionState !== "undecided"
    )
      void mutate("selectionState", "undecided", false);
    else if (/^[0-5]$/.test(event.key))
      void mutate("rating", Number(event.key), false);
  };

  const scheduleGridRender = (render = renderGrid) => {
    if (gridRenderFrame !== undefined) return;
    const authority = sourceGrid.authority;
    gridRenderFrame = requestAnimationFrame(() => {
      gridRenderFrame = undefined;
      if (sourceGrid.isCurrent(authority)) render();
    });
  };
  gridViewport.addEventListener("scroll", () => {
    scheduleGridRender();
    const count = columns();
    void loadWindow(
      Math.floor(gridViewport.scrollTop / GRID_CELL_HEIGHT) * count,
    );
  });
  const onResize = () => {
    const gridAuthority = sourceGrid.authority;
    requestAnimationFrame(() => {
      const gridPosition = sourceGrid.readGridPosition(gridAuthority);
      if (gridPosition === undefined) return;
      if (gridView.hidden) return;
      const nextColumns = columns();
      const nextViewportHeight = effectiveViewportHeight();
      if (
        nextColumns === renderedColumns &&
        nextViewportHeight === renderedViewportHeight
      )
        return;
      if (nextColumns !== renderedColumns) {
        syncGridHeight(nextColumns);
        gridViewport.scrollTop =
          Math.floor(gridPosition / nextColumns) * GRID_CELL_HEIGHT;
      }
      renderGrid();
    });
  };
  window.addEventListener("resize", onResize);
  back.addEventListener("click", showGrid);
  refresh.addEventListener("click", () => {
    void (async () => {
      // A Folder reopen needs the File Location binding: never send a
      // publicationless browse (it can only fail as expired/invalid).
      if (sourceGrid.kind === "folder" && !fileLocations.publication) {
        await awaitRootBinding();
        if (!fileLocations.publication) {
          gridStatus.textContent =
            "Could not load this source. Retry to continue.";
          return;
        }
      }
      if (sourceGrid.kind === "album") {
        const album = application.albums.find(
          (set) => set.id === sourceGrid.albumId,
        );
        if (album) await openSource("album", album);
        return;
      }
      await openSourceDescriptor(sourceGrid.source);
    })();
  });
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
        range.start,
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
  retry.addEventListener("click", () => {
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
          gridStatus.textContent =
            "Could not load this source. Retry to continue.";
          return;
        }
      }
      await openSourceDescriptor(remembered);
    })();
  });
  retryPhoto.addEventListener("click", () => {
    void (async () => {
      const retry = photoOwner.beginRetry();
      if (!retry) return;
      status.textContent = "Reconnecting…";
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
          !photoOwner.retryPhotoIsCurrent(retry)
        )
          return;
        // An exact Source retry just refreshed these facts. Other Photo
        // failures still invalidate and reload the current window before
        // trusting Preview or saved-position recovery.
        if (!sourceRangeRecovered) sourceGrid.invalidateWindow(retry.index);
        const windowReady = await loadWindow(
          start,
          {
            kind: "photo",
            authority: retry.windowAuthority,
          },
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
        const persisted = refreshed && (await persistPosition(retry.authority));
        if (
          refreshed &&
          persisted &&
          sourceGrid.isCurrent(retry.sourceAuthority) &&
          photoOwner.retryPhotoIsCurrent(retry)
        ) {
          recoveryGate.succeedTransition(photoTransition);
          setConnected(true);
          status.textContent = "Connected. Current state refreshed.";
        }
      } finally {
        photoOwner.finishRetry(retry);
        updateControls();
      }
    })();
  });
  previous.addEventListener(
    "click",
    () => void moveTo(photoOwner.currentIndex - 1),
  );
  next.addEventListener(
    "click",
    () => void moveTo(photoOwner.currentIndex + 1),
  );
  undoButton.addEventListener("click", () => void performUndo());
  detail.addEventListener("click", () => {
    if (!stage.querySelector("img")) return;
    zoomed = !zoomed;
    panX = 0;
    panY = 0;
    detail.setAttribute("aria-pressed", String(zoomed));
    detail.textContent = zoomed ? "Exit Detail" : "Detail Review";
    applyTransform();
  });
  stage.addEventListener("dblclick", () => detail.click());
  select.addEventListener(
    "click",
    () => void mutate("selectionState", "selected", true),
  );
  reject.addEventListener(
    "click",
    () => void mutate("selectionState", "rejected", true),
  );
  clear.addEventListener(
    "click",
    () => void mutate("selectionState", "undecided", false),
  );
  ratings.addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
      "[data-rating-value]",
    );
    if (button)
      void mutate("rating", Number(button.dataset.ratingValue), false);
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
  window.addEventListener("keydown", keydown);
  void application.loadOverview();
  return () => {
    if (!applicationAlive) return;
    applicationAlive = false;
    cancelScheduledGridRender();
    albumRecovery = undefined;
    albumActions.dispose();
    application.dispose();
    fileLocations.dispose();
    photoOwner.dispose();
    sourceGrid.dispose();
    recoveryGate.close();
    window.removeEventListener("keydown", keydown);
    window.removeEventListener("resize", onResize);
    compactSources.removeEventListener("change", onSourceViewportChange);
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
function selectionLabel(value?: SelectionState): string {
  return value === "selected"
    ? "Selected"
    : value === "rejected"
      ? "Rejected"
      : "Undecided";
}
function sourceLabel(source?: PreviewSource): string {
  return source === "matching-jpeg"
    ? "JPEG"
    : source === "embedded-raw-jpeg"
      ? "RAW embedded JPEG"
      : "—";
}
function clamp(value: number, low: number, high: number): number {
  return Math.max(low, Math.min(high, value));
}
