import {
  RecoveryGate,
  TaskScope,
  type RecoveryClaim,
  type RecoveryTransition,
} from "./model/async-ownership.js";
import type {
  AlbumSummary,
  BrowseOpenResponse,
  BrowseWindowResponse,
  FolderChild,
  PhotoSummary,
  PreviewResponse,
  PreviewSource,
  SelectionState,
  UndoDescription,
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
  type AlbumMutationSettlement,
  type ApplicationCoordination,
  type ApplicationEvent,
  type ApplicationPresentation,
  type ApplicationRecovery,
  type FileLocationPresentation,
} from "./model/application-owner.js";
import "./ui/library-browser.css";

type SessionUndo = UndoDescription &
  Readonly<{ advanced: boolean; snapshotIndex: number }>;
type MutationResponse = Readonly<{ undo: UndoDescription }>;
const WINDOW_SIZE = 60;
const MAX_RETAINED_FACTS = WINDOW_SIZE * 3;
const MAX_RETAINED_THUMBNAILS = WINDOW_SIZE * 4;
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
    if (sourceKind === "album" && sourceSetId) {
      const open = presentation.albums.find(
        (candidate) => candidate.id === sourceSetId,
      );
      if (open) {
        sourceSetName = open.name;
        gridTitle.textContent = sourceSetName;
        photoTitle.textContent = sourceSetName;
        if (lastSource?.kind === "album" && lastSource.set?.id === open.id)
          lastSource = { ...lastSource, set: open };
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
      if (lastSource?.kind === "folder" && !token) {
        await awaitRootBinding();
        if (!coordination.isCurrent()) return;
      } else {
        void loadFolderWindow("", 0, false);
      }
    }
    if (!coordination.isCurrent()) return;
    if (!token && coordination.overview.published) {
      const remembered = lastSource ?? {
        kind: "library" as const,
        set: undefined,
        folder: undefined,
      };
      const bindable =
        remembered.kind !== "folder" || fileLocations.publication !== undefined;
      if (bindable) {
        await openSource(
          remembered.kind,
          remembered.set,
          undefined,
          remembered.folder,
        );
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
  let token = "";
  let total = 0;
  let currentIndex = 0;
  let sourceKind: "library" | "album" | "folder" = "library";
  let sourceSetId: string | undefined;
  let sourceSetName = "All Photos";
  let sourceFolder: { location: string; name: string } | undefined;
  // In-progress Album management form: create, rename, or a two-step delete
  // confirmation. Only one form exists at a time and it lives in the Albums
  // section of the source list. The current model object is the operation's
  // identity: an asynchronous continuation only clears or updates the form it
  // still owns, so a newer form is never clobbered by an older settle.
  type AlbumFormModel =
    | {
        kind: "create";
        formId: string;
        name: string;
        pending?: boolean;
        message?: string;
      }
    | {
        kind: "rename";
        formId: string;
        id: string;
        name: string;
        pending?: boolean;
        message?: string;
      }
    | {
        kind: "delete";
        formId: string;
        id: string;
        name: string;
        pending?: boolean;
      };
  let albumFormCounter = 0;
  const nextAlbumFormId = () => `album-form-${++albumFormCounter}`;
  let albumForm: AlbumFormModel | undefined;

  const ALBUM_NAME_MAXIMUM = 120;
  const albumNameError = (name: string): string | undefined => {
    const trimmed = name.trim();
    if (!trimmed) return "Enter an Album name.";
    if (Array.from(trimmed).length > ALBUM_NAME_MAXIMUM)
      return `Album names are at most ${ALBUM_NAME_MAXIMUM} characters.`;
    return undefined;
  };

  // The most recently attempted source, retained for truthful reconnection
  // instead of silently falling back to All Photos.
  let lastSource:
    | {
        kind: "library" | "album" | "folder";
        set: AlbumSummary | undefined;
        folder: { location: string; name: string } | undefined;
      }
    | undefined;
  let loaded = new Map<number, PhotoSummary>();
  let thumbnailUrls = new Map<string, string>();
  let thumbnailFailures = new Set<string>();
  let renderedThumbnailImages = new Map<string, HTMLImageElement>();
  let lastCurrentPhotoId: string | undefined;
  let renderedColumns = 0;
  let renderedViewportHeight = 0;
  let connected = false;
  let connectionEstablished = false;
  let busy = false;
  let openingPhoto = false;
  let currentPhotoMode = false;
  let retrySourceRequired = false;
  const browseRangeFailures = new Map<string, RecoveryClaim>();
  let undo: SessionUndo | undefined;
  let requestGeneration = 0;
  let sourceGeneration = 0;
  let sourceTasks = new TaskScope();
  let gridTasks = new TaskScope();
  let photoTasks = new TaskScope();
  let sourceSignal = sourceTasks.beginLatest("lifetime", {
    abortTransport: true,
  }).signal!;
  let gridSignal = gridTasks.beginLatest("lifetime", {
    abortTransport: true,
  }).signal!;
  let photoSignal = photoTasks.beginLatest("lifetime", {
    abortTransport: true,
  }).signal!;
  const renewSourceTasks = () => {
    sourceTasks.halt();
    sourceTasks = new TaskScope();
    sourceSignal = sourceTasks.beginLatest("lifetime", {
      abortTransport: true,
    }).signal!;
  };
  const renewGridTasks = () => {
    gridTasks.halt();
    gridTasks = new TaskScope();
    gridSignal = gridTasks.beginLatest("lifetime", {
      abortTransport: true,
    }).signal!;
  };
  const renewPhotoTasks = () => {
    photoTasks.halt();
    photoTasks = new TaskScope();
    photoSignal = photoTasks.beginLatest("lifetime", {
      abortTransport: true,
    }).signal!;
  };
  let browseTokenGeneration = 0;
  let gridRenderFrame: number | undefined;
  let progressQueue: Promise<void> = Promise.resolve();
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
        generation: number;
        photoId: string;
      }
    | undefined;

  const currentPhoto = () => loaded.get(currentIndex);
  const syncConnection = (message?: string) => {
    if (!applicationAlive) return;
    for (const [key, claim] of browseRangeFailures)
      if (!recoveryGate.isActive(claim)) browseRangeFailures.delete(key);
    connected = connectionEstablished && recoveryGate.decisionReady;
    if (!connected) clearPointer();
    connection.textContent = connected ? "Connected" : "Disconnected";
    connection.classList.toggle("offline", !connected);
    retry.hidden = connected || currentPhotoMode;
    retryPhoto.hidden = connected || !currentPhotoMode;
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
    generation: number,
    start: number,
    transportLost: boolean,
    transition?: RecoveryTransition,
  ): void => {
    const key = `${ownerScope}:${generation}:${start}`;
    const owner = { scope: ownerScope, generation: String(generation) };
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
    if (claim) browseRangeFailures.set(key, claim);
    if (ownerScope === "source") retrySourceRequired = true;
    syncConnection();
  };
  const recoverBrowseRange = (
    ownerScope: "source" | "photo",
    generation: number,
    start: number,
  ): void => {
    const key = `${ownerScope}:${generation}:${start}`;
    const claim = browseRangeFailures.get(key);
    if (!claim) return;
    browseRangeFailures.delete(key);
    if (recoveryGate.recover(claim)) setConnected(true);
  };
  const failPhotoRecovery = (
    generation: number,
    kind: string,
    transition?: RecoveryTransition,
  ): void => {
    const owner = { scope: "photo" as const, generation: String(generation) };
    if (transition) {
      try {
        const replacement = recoveryGate.issue(kind, String(generation), {
          owner,
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
    const claim = recoveryGate.issue(kind, String(generation), { owner });
    if (!recoveryGate.fail(claim, { transportLost: true }))
      recoveryGate.discard(claim);
    syncConnection();
  };
  const updateControls = () => {
    if (!applicationAlive) return;
    const photo = currentPhoto();
    const enabled = Boolean(photo) && connected && !busy;
    for (const button of [
      select,
      reject,
      ...Array.from(ratings.querySelectorAll<HTMLButtonElement>("button")),
    ])
      button.disabled = !enabled;
    clear.disabled = !enabled || photo?.selectionState === "undecided";
    back.disabled = busy;
    refresh.disabled = busy;
    previous.disabled = busy || openingPhoto || currentIndex <= 0;
    next.disabled =
      busy || openingPhoto || total === 0 || currentIndex >= total - 1;
    undoButton.disabled = !connected || busy || openingPhoto || !undo;
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
    path: string,
    body: unknown,
    failure: (httpStatus: number) => string,
    surface: "photo" | "summary",
    photoGeneration = requestGeneration,
    admissionKey?: string,
  ): Promise<{
    admitted: boolean;
    ok: boolean;
    announce: (text: string) => void;
  }> => {
    const capturedPhotoStatus = photoStatusOwner;
    const sourceOwner = sourceGeneration;
    const ownsPhotoSurface = () =>
      surface === "photo" &&
      photoGeneration === requestGeneration &&
      currentPhotoMode &&
      capturedPhotoStatus === photoStatusOwner;
    const mutation = application.beginAlbumMutation({
      noticeKey: admissionKey ?? path,
      ...(admissionKey ? { admissionKey } : {}),
      surface,
      ownsSurface: ownsPhotoSurface,
    });
    if (!mutation)
      return Promise.resolve({
        admitted: false,
        ok: false,
        announce: () => {},
      });
    const disconnect = () => {
      const claim = recoveryGate.issue("album", path, {
        owner: { scope: "source", generation: String(sourceOwner) },
      });
      if (!recoveryGate.fail(claim, { transportLost: true }))
        recoveryGate.discard(claim);
      syncConnection();
    };
    const settle = async (result: AlbumMutationSettlement) => {
      const outcome = await application.settleAlbumMutation(mutation, result);
      if (outcome.surfaceMessage) status.textContent = outcome.surfaceMessage;
      if (outcome.disconnect) disconnect();
      return outcome;
    };
    return (async () => {
      try {
        const response = await fetcher(path, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        });
        if (!response.ok) {
          await settle({
            kind: "failed",
            message: failure(response.status),
            transportLost: response.status >= 500,
          });
          return { admitted: true, ok: false, announce: () => {} };
        }
        const outcome = await settle({ kind: "persisted" });
        return {
          admitted: true,
          ok: true,
          announce: (text: string) => {
            if (outcome.presentOnSurface && ownsPhotoSurface())
              status.textContent = text;
          },
        };
      } catch {
        await settle({
          kind: "failed",
          message: failure(0),
          transportLost: true,
        });
        return { admitted: true, ok: false, announce: () => {} };
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
      sourceKind === "folder" && sourceFolder?.location === child.location,
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
      sourceKind === "library",
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
      sourceKind === "folder" && sourceFolder?.location === "",
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
      albumForm = { kind: "create", formId: nextAlbumFormId(), name: "" };
      renderSources();
    });
    albumHeadingRow.append(albumHeading, newAlbum);
    sourceList.append(albumHeadingRow);

    if (albumForm?.kind === "create") {
      sourceList.append(
        albumCreateForm(() => {
          albumForm = undefined;
          renderSources();
        }),
      );
    }
    for (const set of application.albums) {
      const button = sourceButton(
        set.name,
        set.photoCount,
        sourceKind === "album" && sourceSetId === set.id,
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
      if (albumForm === draft) {
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

  const albumCreateForm = (cancel: () => void) => {
    const model: AlbumFormModel = albumForm as {
      kind: "create";
      formId: string;
      name: string;
      pending?: boolean;
    };
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
    abort.addEventListener("click", cancel);
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
          "/api/albums",
          { name },
          (httpStatus) =>
            httpStatus === 409
              ? "An Album with this name already exists."
              : "The Album could not be created.",
          "summary",
        );
        // Ownership check: a newer form opened meanwhile keeps its own state.
        if (albumForm === model) {
          if (created) albumForm = undefined;
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
      const model: AlbumFormModel = albumForm as {
        kind: "rename";
        formId: string;
        id: string;
        name: string;
        pending?: boolean;
      };
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
        albumForm = undefined;
        renderSources();
      });
      form.append(input, save, abort, message);
      form.addEventListener("submit", (event) => {
        event.preventDefault();
        const name = input.value.trim();
        if (!name || name === set.name) {
          albumForm = undefined;
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
            `/api/albums/${set.id}/rename`,
            { name },
            (httpStatus) =>
              httpStatus === 409
                ? "An Album with this name already exists."
                : "The Album could not be renamed.",
            "summary",
          );
          // Ownership check: a newer form opened meanwhile keeps its own state.
          if (albumForm === model) {
            if (renamed) albumForm = undefined;
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
        albumForm = undefined;
        renderSources();
      });
      confirmButton.addEventListener("click", () => {
        void (async () => {
          confirmButton.disabled = true;
          model.pending = true;
          const { ok: deleted } = await mutateAlbum(
            `/api/albums/${set.id}/delete`,
            {},
            () => "The Album could not be deleted.",
            "summary",
          );
          // Ownership check: a newer form opened while the deletion was
          // pending keeps its own state and draft.
          if (albumForm === model) albumForm = undefined;
          renderSources();
          if (deleted && sourceKind === "album" && sourceSetId === set.id) {
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
      albumForm = {
        kind: "rename",
        formId: nextAlbumFormId(),
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
      albumForm = {
        kind: "delete",
        formId: nextAlbumFormId(),
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
  const cancelPendingImageLoads = (
    container: ParentNode,
    removeCompleted = false,
  ) => {
    for (const image of Array.from(
      container.querySelectorAll<HTMLImageElement>("img"),
    )) {
      if (!removeCompleted && image.complete) continue;
      image.onerror = null;
      image.removeAttribute("src");
    }
  };

  const openSource = async (
    kind: "library" | "album" | "folder",
    set?: AlbumSummary,
    preferredPhotoId?: string,
    folder?: { location: string; name: string },
  ) => {
    const sourceDrawerWasOpen = browser.classList.contains("sources-open");
    busy = true;
    cancelScheduledGridRender();
    sourceGeneration += 1;
    requestGeneration += 1;
    const generation = sourceGeneration;
    const sourceTransition = recoveryGate.beginTransition(
      "source",
      String(generation),
    );
    const photoTransition = recoveryGate.beginTransition(
      "photo",
      String(requestGeneration),
    );
    recoveryGate.succeedTransition(photoTransition);
    syncConnection();
    openingPhoto = false;
    cancelPendingImageLoads(gridLayer, true);
    gridLayer.replaceChildren();
    cancelPendingImageLoads(stage, true);
    stage.replaceChildren();
    renewSourceTasks();
    renewGridTasks();
    renewPhotoTasks();
    const signal = sourceSignal;
    currentPhotoMode = false;
    gridView.hidden = false;
    photoView.hidden = true;
    closeSources(false);
    if (sourceDrawerWasOpen) gridViewport.focus();
    sourceKind = kind;
    sourceSetId = set?.id;
    sourceFolder = folder;
    // A new open snapshot ends the previous source's removal memory.
    removedFromCurrentAlbum.clear();
    sourceSetName =
      set?.name ?? (folder ? `${folder.name} · Folder` : "All Photos");
    gridTitle.textContent = sourceSetName;
    gridStatus.textContent = "Preparing Library order…";
    undo = undefined;
    loaded = new Map();
    thumbnailUrls = new Map();
    thumbnailFailures = new Set();
    lastCurrentPhotoId = preferredPhotoId;
    const priorToken = token;
    lastSource = { kind, set, folder };
    try {
      const response = await fetcher("/api/browse", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(
          kind === "library"
            ? {
                source: "library",
                ...(preferredPhotoId ? { photoId: preferredPhotoId } : {}),
              }
            : kind === "folder"
              ? {
                  source: "folder",
                  folderPath: folder!.location,
                  ...(fileLocations.publication
                    ? { publication: fileLocations.publication }
                    : {}),
                  ...(preferredPhotoId ? { photoId: preferredPhotoId } : {}),
                }
              : {
                  source: "album",
                  albumId: set!.id,
                  ...(preferredPhotoId ? { photoId: preferredPhotoId } : {}),
                },
        ),
        signal,
        priority: "high",
      });
      if (response.status === 409 && kind === "folder") {
        // Only the current source's handler may reset and reload File
        // Locations: a superseded open doing the same would discard the
        // newer recovery and leave the tree unbound.
        if (generation !== sourceGeneration) return;
        const reboundAuthority = await rebindFileLocations();
        if (generation !== sourceGeneration) return;
        if (
          fileLocations.isCurrent(reboundAuthority) &&
          fileLocations.publication
        )
          claimPublicationLocationNotice(
            `publication:${fileLocations.publication}`,
            "Scan results changed File Locations. Reopen the current Folder.",
          );
        throw new Error("file locations expired");
      }
      if (!response.ok) throw new Error("browse open failed");
      const opened = (await response.json()) as BrowseOpenResponse;
      if (generation !== sourceGeneration) {
        void closeBrowse(opened.token);
        return;
      }
      token = opened.token;
      browseTokenGeneration = generation;
      if (priorToken && priorToken !== token) void closeBrowse(priorToken);
      total = opened.total;
      currentIndex = Math.min(opened.position, Math.max(0, total - 1));
      gridViewport.scrollTop =
        Math.floor(currentIndex / columns()) * GRID_CELL_HEIGHT;
      renderSources();
      const windowReady = await loadWindow(
        currentIndex,
        generation,
        false,
        sourceSignal,
        "high",
        sourceTransition,
      );
      if (generation !== sourceGeneration || !windowReady) return;
      if (kind === "folder") releasePublicationLocationRecovery();
      recoveryGate.succeedTransition(sourceTransition);
      retrySourceRequired = false;
      setConnected(true);
      renderGrid();
      gridStatus.textContent = total
        ? `Ready · ${total.toLocaleString()} Photos`
        : "No Photos in this source";
    } catch {
      if (generation !== sourceGeneration) return;
      token = "";
      if (priorToken) void closeBrowse(priorToken);
      gridStatus.textContent = "Could not load this source. Retry to continue.";
      retrySourceRequired = true;
      const claim = recoveryGate.issue("source-open", String(generation), {
        owner: { scope: "source", generation: String(generation) },
        transition: sourceTransition,
      });
      recoveryGate.failTransition(sourceTransition, claim, {
        transportLost: true,
      });
      syncConnection();
    } finally {
      if (generation === sourceGeneration) {
        busy = false;
        updateControls();
      }
    }
  };

  const columns = () =>
    Math.max(
      1,
      Math.floor(Math.max(320, gridViewport.clientWidth) / GRID_CELL_WIDTH),
    );
  const effectiveViewportHeight = () =>
    Math.max(360, Math.min(gridViewport.clientHeight, window.innerHeight));
  const alignedStart = (index: number) =>
    Math.max(
      0,
      Math.min(
        Math.floor(index / WINDOW_SIZE) * WINDOW_SIZE,
        Math.max(0, total - WINDOW_SIZE),
      ),
    );
  const closeBrowse = async (browseToken: string) => {
    try {
      await fetcher(`/api/browse/${encodeURIComponent(browseToken)}`, {
        method: "DELETE",
        keepalive: true,
      });
    } catch {
      /* bounded server expiry remains the fallback */
    }
  };
  const reopenExpired = async (
    anchorIndex: number,
    expectedGeneration = sourceGeneration,
  ) => {
    if (expectedGeneration !== sourceGeneration) return;
    busy = true;
    const resumePhoto = currentPhotoMode;
    const photoGeneration = ++requestGeneration;
    const photoTransition = resumePhoto
      ? recoveryGate.beginTransition("photo", String(photoGeneration))
      : undefined;
    if (!photoTransition) {
      const gridTransition = recoveryGate.beginTransition(
        "photo",
        String(photoGeneration),
      );
      recoveryGate.succeedTransition(gridTransition);
    }
    syncConnection();
    renewPhotoTasks();
    const currentPhotoSignal = photoSignal;
    cancelPendingImageLoads(stage);
    openingPhoto = false;
    const oldToken = token;
    const anchorId =
      loaded.get(anchorIndex)?.id ?? lastCurrentPhotoId ?? currentPhoto()?.id;
    renewSourceTasks();
    renewGridTasks();
    cancelScheduledGridRender();
    const generation = ++sourceGeneration;
    const sourceTransition = recoveryGate.beginTransition(
      "source",
      String(generation),
    );
    syncConnection();
    cancelPendingImageLoads(gridLayer);
    const signal = sourceSignal;
    let boundPublication = fileLocations.publication;
    if (sourceKind === "folder" && !boundPublication) {
      // A Folder source must never be reopened publicationless; wait for
      // the root binding and fail truthfully if it cannot be established.
      boundPublication = (await awaitRootBinding())
        ? fileLocations.publication
        : undefined;
      if (generation !== sourceGeneration) return;
      if (!boundPublication) {
        // Fail truthfully instead of sending a publicationless request.
        gridStatus.textContent =
          "Could not load this source. Retry to continue.";
        retrySourceRequired = true;
        const claim = recoveryGate.issue("source-reopen", String(generation), {
          owner: { scope: "source", generation: String(generation) },
          transition: sourceTransition,
        });
        recoveryGate.failTransition(sourceTransition, claim, {
          transportLost: true,
        });
        syncConnection();
        busy = false;
        updateControls();
        return;
      }
    }
    const notice =
      "Library order expired. Reopening this source from the latest Library…";
    gridStatus.textContent = notice;
    status.textContent = notice;
    try {
      const response = await fetcher("/api/browse", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(
          sourceKind === "library"
            ? { source: "library", ...(anchorId ? { photoId: anchorId } : {}) }
            : sourceKind === "folder"
              ? {
                  source: "folder",
                  folderPath: sourceFolder?.location ?? "",
                  ...(boundPublication
                    ? { publication: boundPublication }
                    : {}),
                  ...(anchorId ? { photoId: anchorId } : {}),
                }
              : {
                  source: "album",
                  albumId: sourceSetId,
                  ...(anchorId ? { photoId: anchorId } : {}),
                },
        ),
        signal,
        priority: "high",
      });
      if (response.status === 409 && sourceKind === "folder") {
        // Generation-gated exactly like openSource: only the current
        // source's recovery may reset and rebind File Locations.
        if (generation === sourceGeneration) {
          const reboundAuthority = await rebindFileLocations();
          if (
            generation === sourceGeneration &&
            fileLocations.isCurrent(reboundAuthority) &&
            fileLocations.publication
          )
            claimPublicationLocationNotice(
              `publication:${fileLocations.publication}`,
              "Scan results changed File Locations. Reopen the current Folder.",
            );
        }
      }
      if (!response.ok) throw new Error("browse reopen failed");
      const opened = (await response.json()) as BrowseOpenResponse;
      if (generation !== sourceGeneration) {
        void closeBrowse(opened.token);
        return;
      }
      token = opened.token;
      browseTokenGeneration = generation;
      total = opened.total;
      currentIndex = Math.min(opened.position, Math.max(0, total - 1));
      loaded = new Map();
      thumbnailUrls = new Map();
      thumbnailFailures = new Set();
      if (oldToken && oldToken !== token) void closeBrowse(oldToken);
      const windowReady = await loadWindow(
        currentIndex,
        generation,
        false,
        sourceSignal,
        "high",
        sourceTransition,
      );
      if (generation !== sourceGeneration || !windowReady) return;
      gridViewport.scrollTop =
        Math.floor(currentIndex / columns()) * GRID_CELL_HEIGHT;
      renderGrid();
      gridStatus.textContent =
        "Source reopened using the latest published Library order.";
      if (sourceKind === "folder") releasePublicationLocationRecovery();
      recoveryGate.succeedTransition(sourceTransition);
      retrySourceRequired = false;
      setConnected(true);
      if (resumePhoto && photoGeneration === requestGeneration) {
        gridView.hidden = true;
        photoView.hidden = false;
        syncSourcePanel();
        renderPhotoShell(photoGeneration);
        void showPreview(
          photoGeneration,
          currentPhotoSignal,
          photoTransition,
        ).then(async (refreshed) => {
          if (!refreshed || photoGeneration !== requestGeneration) return;
          const persisted = await persistPosition();
          if (
            persisted &&
            photoGeneration === requestGeneration &&
            photoTransition
          ) {
            recoveryGate.succeedTransition(photoTransition);
            setConnected(true);
          }
        });
      }
    } catch {
      if (generation !== sourceGeneration) return;
      browseTokenGeneration = generation;
      const failure =
        "This source expired and could not be reopened. Retry the connection.";
      gridStatus.textContent = failure;
      status.textContent = failure;
      retrySourceRequired = true;
      const claim = recoveryGate.issue("source-reopen", String(generation), {
        owner: { scope: "source", generation: String(generation) },
        transition: sourceTransition,
      });
      recoveryGate.failTransition(sourceTransition, claim, {
        transportLost: true,
      });
      syncConnection();
    } finally {
      if (generation === sourceGeneration) {
        busy = false;
        updateControls();
      }
    }
  };
  const windowLoaded = (start: number) => {
    const end = Math.min(total, start + WINDOW_SIZE);
    for (let index = start; index < end; index += 1)
      if (!loaded.has(index)) return false;
    return start < end;
  };
  const loadWindow = (
    index: number,
    generation = sourceGeneration,
    quiet = false,
    signal = gridSignal,
    priority: "high" | "low" = quiet ? "low" : "high",
    transition?: RecoveryTransition,
  ): Promise<boolean> => {
    if (
      generation !== sourceGeneration ||
      !token ||
      browseTokenGeneration !== sourceGeneration
    )
      return Promise.resolve(false);
    if (total === 0) return Promise.resolve(true);
    const start = alignedStart(index);
    if (windowLoaded(start)) return Promise.resolve(true);
    const browseToken = token;
    const expectedTotal = total;
    const photoGeneration = requestGeneration;
    const scope =
      signal === sourceSignal
        ? sourceTasks
        : signal === photoSignal
          ? photoTasks
          : gridTasks;
    const ownerScope = scope === photoTasks ? "photo" : "source";
    const ownerGeneration =
      ownerScope === "photo" ? photoGeneration : generation;
    const rangeMessage = `Photos ${start + 1}–${Math.min(total, start + WINDOW_SIZE)}`;
    if (!quiet)
      gridStatus.textContent = `Loading ${rangeMessage} of ${total.toLocaleString()}…`;
    const shared = scope.joinOrStart(
      `window:${start}`,
      { abortTransport: true, onCancel: () => false },
      async (requestSignal) => {
        const ownedSignal = requestSignal!;
        let response: Response;
        try {
          response = await fetcher(
            `/api/browse/${encodeURIComponent(browseToken)}?start=${start}&limit=${WINDOW_SIZE}`,
            { signal: ownedSignal, priority },
          );
        } catch {
          if (!ownedSignal.aborted && generation === sourceGeneration) {
            const message = `Connection lost while loading ${rangeMessage}. Retry this range.`;
            if (ownerScope === "photo") status.textContent = message;
            else gridStatus.textContent = message;
            failBrowseRange(
              ownerScope,
              ownerGeneration,
              start,
              true,
              transition,
            );
          }
          return false;
        }
        if (response.status === 404) {
          if (!ownedSignal.aborted && generation === sourceGeneration)
            await reopenExpired(index, generation);
          return false;
        }
        if (!response.ok) {
          if (!ownedSignal.aborted && generation === sourceGeneration) {
            const message = `${rangeMessage} could not be loaded (HTTP ${response.status}). Retry this range.`;
            if (ownerScope === "photo") status.textContent = message;
            else gridStatus.textContent = message;
            failBrowseRange(
              ownerScope,
              ownerGeneration,
              start,
              false,
              transition,
            );
          }
          return false;
        }
        let result: BrowseWindowResponse;
        try {
          const candidate: unknown = await response.json();
          const decoded = decodeBrowseWindow(candidate, start, expectedTotal);
          if (!decoded) throw new Error("malformed window");
          result = decoded;
        } catch {
          if (!ownedSignal.aborted && generation === sourceGeneration) {
            const message = `${rangeMessage} returned an invalid response. Retry this range.`;
            if (ownerScope === "photo") status.textContent = message;
            else gridStatus.textContent = message;
            failBrowseRange(
              ownerScope,
              ownerGeneration,
              start,
              false,
              transition,
            );
          }
          return false;
        }
        if (generation !== sourceGeneration || ownedSignal.aborted)
          return false;
        for (const [offset, photo] of result.photos.entries())
          loaded.set(result.start + offset, photo);
        recoverBrowseRange(ownerScope, ownerGeneration, start);
        trimLoaded(index);
        renderGrid();
        if (!quiet)
          gridStatus.textContent = `Ready · ${total.toLocaleString()} Photos`;
        return true;
      },
    );
    return shared.promise.finally(() => {
      if (
        shared.signal?.aborted &&
        generation === sourceGeneration &&
        !gridView.hidden
      )
        scheduleGridRender();
      updateControls();
    });
  };
  const trimLoaded = (anchor: number) => {
    if (loaded.size <= MAX_RETAINED_FACTS) return;
    for (const index of Array.from(loaded.keys()))
      if (
        Math.abs(index - anchor) > WINDOW_SIZE &&
        loaded.size > MAX_RETAINED_FACTS
      )
        loaded.delete(index);
  };
  const syncGridHeight = (count: number) => {
    const height = `${Math.ceil(total / count) * GRID_CELL_HEIGHT}px`;
    gridCanvas.style.height = height;
    gridLayer.style.height = height;
  };
  const renderGrid = () => {
    if (!applicationAlive) return;
    const count = columns();
    renderedColumns = count;
    renderedViewportHeight = effectiveViewportHeight();
    syncGridHeight(count);
    const firstRow = Math.max(
      0,
      Math.floor(gridViewport.scrollTop / GRID_CELL_HEIGHT) - 2,
    );
    const visibleRows =
      Math.ceil(renderedViewportHeight / GRID_CELL_HEIGHT) + 4;
    const start = firstRow * count;
    const end = Math.min(total, start + visibleRows * count);
    renderedThumbnailImages = new Map();
    cancelPendingImageLoads(gridLayer);
    gridLayer.replaceChildren();
    for (let index = start; index < end; index += 1) {
      const cell = document.createElement("button");
      cell.type = "button";
      cell.className = "photo-cell";
      cell.style.left = `${(index % count) * GRID_CELL_WIDTH}px`;
      cell.style.top = `${Math.floor(index / count) * GRID_CELL_HEIGHT}px`;
      const photo = loaded.get(index);
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
    const generation = sourceGeneration;
    const image = document.createElement("img");
    image.alt = `Photo ${index + 1} of ${total}`;
    image.loading = "lazy";
    image.fetchPriority = "low";
    image.decoding = "async";
    image.draggable = false;
    image.className = "thumbnail";
    renderedThumbnailImages.set(photo.id, image);
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
      markThumbnailUnavailable(photo.id, image, generation);
    } else if (photo.preview.thumbnailUrl) {
      attachThumbnail(
        photo.id,
        image,
        photo.preview.thumbnailUrl,
        true,
        generation,
      );
    } else {
      void loadThumbnail(photo.id, image, generation);
    }
  };
  const rememberThumbnail = (photoId: string, url: string) => {
    thumbnailUrls.delete(photoId);
    thumbnailUrls.set(photoId, url);
    while (thumbnailUrls.size > MAX_RETAINED_THUMBNAILS) {
      const oldest = thumbnailUrls.keys().next().value;
      if (oldest === undefined) break;
      thumbnailUrls.delete(oldest);
    }
  };
  const markThumbnailUnavailable = (
    photoId: string,
    image: HTMLImageElement,
    generation: number,
  ) => {
    if (
      generation !== sourceGeneration ||
      renderedThumbnailImages.get(photoId) !== image
    )
      return;
    thumbnailFailures.add(photoId);
    image.removeAttribute("src");
    if (!image.alt.includes("Thumbnail unavailable"))
      image.alt = `${image.alt} — Thumbnail unavailable`;
  };
  const attachThumbnail = (
    photoId: string,
    image: HTMLImageElement,
    url?: string,
    attachDisconnected = false,
    generation = sourceGeneration,
  ) => {
    if (
      !url ||
      generation !== sourceGeneration ||
      renderedThumbnailImages.get(photoId) !== image
    )
      return;
    if (thumbnailFailures.has(photoId)) {
      if (!image.alt.includes("Thumbnail unavailable"))
        image.alt = `${image.alt} — Thumbnail unavailable`;
      return;
    }
    const transfer = gridTasks.beginLatest(`image:${photoId}`, {
      abortTransport: false,
    });
    const expectedUrl = new URL(url, window.location.href).href;
    transfer.onCleanup(() => {
      image.onload = null;
      image.onerror = null;
      if (!image.complete && image.src === expectedUrl)
        image.removeAttribute("src");
    });
    image.onload = () => transfer.finish();
    image.onerror = () => {
      if (!transfer.isCurrent()) return;
      markThumbnailUnavailable(photoId, image, generation);
      transfer.finish();
    };
    if (attachDisconnected || image.isConnected) image.src = url;
  };
  const loadThumbnail = (
    photoId: string,
    image: HTMLImageElement,
    generation = sourceGeneration,
  ) => {
    if (
      generation !== sourceGeneration ||
      renderedThumbnailImages.get(photoId) !== image
    )
      return;
    const cached = thumbnailUrls.get(photoId);
    if (cached) {
      attachThumbnail(photoId, image, cached, true, generation);
      return;
    }
    if (thumbnailFailures.has(photoId)) {
      if (!image.alt.includes("Thumbnail unavailable"))
        image.alt = `${image.alt} — Thumbnail unavailable`;
      return;
    }
    const scope = gridTasks;
    const request = scope.joinOrStart(
      `thumbnail:${photoId}`,
      { abortTransport: true, onCancel: () => undefined },
      async (signal) => {
        try {
          const response = await fetcher(`/api/photos/${photoId}/thumbnail`, {
            ...(signal ? { signal } : {}),
            priority: "low",
          });
          if (!response.ok) return undefined;
          const result = (await response.json()) as PreviewResponse;
          return result.url ?? undefined;
        } catch {
          return undefined;
        }
      },
    );
    void request.promise.then((url) => {
      if (generation !== sourceGeneration || scope !== gridTasks) return;
      if (url) {
        rememberThumbnail(photoId, url);
        thumbnailFailures.delete(photoId);
        attachThumbnail(photoId, image, url, false, generation);
      } else {
        markThumbnailUnavailable(photoId, image, generation);
      }
    });
  };

  const openPhoto = async (index: number) => {
    if (busy || openingPhoto || index < 0 || index >= total) return;
    openingPhoto = true;
    currentIndex = index;
    const generation = ++requestGeneration;
    const photoTransition = recoveryGate.beginTransition(
      "photo",
      String(generation),
    );
    syncConnection();
    renewPhotoTasks();
    const signal = photoSignal;
    cancelPendingImageLoads(stage, true);
    renderedThumbnailImages = new Map();
    renewGridTasks();
    cancelPendingImageLoads(gridLayer);
    currentPhotoMode = true;
    gridView.hidden = true;
    photoView.hidden = false;
    syncSourcePanel();
    photoView.focus();
    resetTransform();
    let windowReady = true;
    try {
      if (!loaded.has(index))
        windowReady = await loadWindow(
          index,
          sourceGeneration,
          true,
          signal,
          "high",
          photoTransition,
        );
    } finally {
      // Back to Grid or a superseding view may end this request while an
      // unloaded boundary window is still loading. Release the open gate so
      // the interface can never remain wedged by an abandoned load.
      openingPhoto = false;
      updateControls();
    }
    if (generation !== requestGeneration || !windowReady) return;
    const current = loaded.get(index);
    if (current) lastCurrentPhotoId = current.id;
    const hasKnownPreview = renderPhotoShell(generation);
    updateControls();
    const previewRequest = showPreview(generation, signal, photoTransition);
    const previewReady = hasKnownPreview || (await previewRequest);
    // A superseded open must not persist or touch controls afterwards: the
    // newer navigation persists its own position.
    if (generation !== requestGeneration) return;
    const positionReady = await persistPosition();
    if (previewReady && positionReady && generation === requestGeneration) {
      recoveryGate.succeedTransition(photoTransition);
      setConnected(true);
    }
    updateControls();
  };
  const renderPhotoFacts = () => {
    const photo = currentPhoto();
    position.textContent = `${currentIndex + 1} / ${total}`;
    selection.textContent = selectionLabel(photo?.selectionState);
    rating.textContent = `${photo?.rating ?? 0} ${(photo?.rating ?? 0) === 1 ? "star" : "stars"}`;
    updateControls();
  };
  const renderReviewImage = (url: string, generation = requestGeneration) => {
    cancelPendingImageLoads(stage, true);
    const capturedStatus = photoStatusOwner;
    const transfer = photoTasks.beginLatest("review-image", {
      abortTransport: false,
    });
    const image = document.createElement("img");
    image.alt = `Photo ${currentIndex + 1} of ${total}`;
    image.draggable = false;
    image.fetchPriority = "high";
    image.decoding = "async";
    const expectedUrl = new URL(url, window.location.href).href;
    transfer.onCleanup(() => {
      image.onload = null;
      image.onerror = null;
      if (!image.complete && image.src === expectedUrl)
        image.removeAttribute("src");
    });
    image.onload = () => transfer.finish();
    image.onerror = () => {
      if (!transfer.isCurrent()) return;
      if (
        generation === requestGeneration &&
        image.isConnected &&
        capturedStatus === photoStatusOwner
      )
        status.textContent =
          "Preview could not be loaded. You can continue browsing.";
      image.removeAttribute("src");
      transfer.finish();
    };
    image.src = url;
    stage.replaceChildren(image);
  };
  // Duplicate membership admission is owned by the Album settlement family;
  // the open snapshot separately remembers members removed until reopen.
  const membershipKey = (
    verb: "add" | "remove",
    albumId: string,
    photoId: string,
  ) => `${verb}:${albumId}:${photoId}`;
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
        application.isAlbumMutationAdmitted(
          membershipKey("add", selectedAlbumId, photo.id),
        ),
    );
    const removing =
      Boolean(photo && sourceSetId) &&
      application.isAlbumMutationAdmitted(
        membershipKey("remove", sourceSetId!, photo!.id),
      );
    const inOpenAlbum =
      sourceKind === "album" &&
      Boolean(sourceSetId) &&
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
    const key = membershipKey("add", albumId, photoId);
    if (application.isAlbumMutationAdmitted(key)) return;
    const generation = requestGeneration;
    void (async () => {
      addToAlbum.disabled = true;
      const { ok: added, announce } = await mutateAlbum(
        `/api/albums/${albumId}/members`,
        { photoIds: [photoId] },
        () => "The Photo could not be added to the Album.",
        "photo",
        generation,
        key,
      );
      // Re-adding to the open Album clears the retained snapshot's removal
      // mark: the Photo is a member again and may be removed once more.
      if (added && albumId === sourceSetId) {
        removedFromCurrentAlbum.delete(photoId);
      }
      // Admitted persistence is never aborted by a source change; the
      // continuation only touches this Photo's controls while it is still
      // the current Photo of the initiating generation.
      if (generation === requestGeneration && currentPhoto()?.id === photoId) {
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
      sourceKind !== "album" ||
      !sourceSetId ||
      removedFromCurrentAlbum.has(photo.id)
    )
      return;
    const albumId = sourceSetId;
    const photoId = photo.id;
    const key = membershipKey("remove", albumId, photoId);
    if (application.isAlbumMutationAdmitted(key)) return;
    const generation = requestGeneration;
    const snapshotGeneration = sourceGeneration;
    void (async () => {
      removeFromAlbum.disabled = true;
      const { ok: removed, announce } = await mutateAlbum(
        `/api/albums/${albumId}/members/remove`,
        { photoId },
        () => "The Photo could not be removed from the Album.",
        "photo",
        generation,
        key,
      );
      if (removed && snapshotGeneration === sourceGeneration) {
        // The member is gone from the Album; within this open snapshot it
        // must not be removable again. A removal settling after the source
        // changed must not mark the Photo in a different Album's snapshot.
        removedFromCurrentAlbum.add(photoId);
      }
      // Recomputing the controls re-enables removal after a failure so the
      // failed action stays retryable.
      if (generation === requestGeneration && currentPhoto()?.id === photoId) {
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

  const renderPhotoShell = (generation = requestGeneration): boolean => {
    const photo = currentPhoto();
    photoTitle.textContent = sourceSetName;
    renderMembershipControls();
    renderPhotoFacts();
    source.textContent = photo?.preview.source
      ? sourceLabel(photo.preview.source)
      : "—";
    limited.hidden = !photo?.preview.limitedDetail;
    const knownUrl = photo?.preview.url;
    if (knownUrl) {
      renderReviewImage(knownUrl, generation);
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
    generation: number,
    signal: AbortSignal,
    transition?: RecoveryTransition,
  ): Promise<boolean> => {
    if (generation !== requestGeneration || signal.aborted) return false;
    const photo = currentPhoto();
    if (!photo) return false;
    const capturedStatus = photoStatusOwner;
    let result: PreviewResponse;
    try {
      // Always contact the server, even for an unavailable Photo: a restored
      // Original must surface its Preview without a full reload, and Retry
      // must prove the server is reachable before reporting Connected.
      const response = await fetcher(`/api/photos/${photo.id}/preview`, {
        signal,
        priority: "high",
      });
      const candidate = (await response.json()) as Partial<PreviewResponse>;
      const explicitState =
        candidate.state === "ready" ||
        candidate.state === "unavailable" ||
        candidate.state === "failed";
      if (!explicitState) throw new Error("malformed Preview response");
      // The protocol intentionally carries explicit non-ready Preview states
      // with HTTP 404. Other non-success statuses are service failures even
      // when their body happens to resemble a typed Preview response.
      const readyResponse =
        response.ok && candidate.state === "ready" && Boolean(candidate.url);
      const nonReadyResponse =
        response.status === 404 &&
        (candidate.state === "unavailable" || candidate.state === "failed");
      if (!readyResponse && !nonReadyResponse)
        throw new Error(`invalid Preview HTTP/state ${response.status}`);
      result = candidate as PreviewResponse;
    } catch {
      if (signal.aborted || generation !== requestGeneration) return false;
      if (capturedStatus === photoStatusOwner)
        status.textContent = "Connection lost. Retry to refresh this Photo.";
      failPhotoRecovery(generation, "preview", transition);
      return false;
    }
    if (
      generation !== requestGeneration ||
      !currentPhoto() ||
      currentPhoto()!.id !== photo.id
    )
      return false;
    const existingImage = stage.querySelector<HTMLImageElement>("img");
    if (result.state !== "ready" || !result.url) {
      if (capturedStatus === photoStatusOwner)
        status.textContent = result.message ?? "Preview unavailable";
      if (!existingImage)
        stage.replaceChildren(paragraph("Preview unavailable"));
      return true;
    }
    const resolvedUrl = new URL(result.url, window.location.href).href;
    if (!existingImage || existingImage.src !== resolvedUrl)
      renderReviewImage(result.url, generation);
    source.textContent = sourceLabel(result.source);
    limited.hidden = !result.limitedDetail;
    if (capturedStatus === photoStatusOwner)
      status.textContent = result.stale
        ? (result.message ?? "Showing a stale Preview.")
        : "";
    if (!result.stale) {
      const latest = currentPhoto();
      if (latest && latest.id === photo.id) {
        loaded.set(currentIndex, {
          ...latest,
          preview: {
            ...latest.preview,
            state: "ready",
            ...(result.source ? { source: result.source } : {}),
            ...(result.width !== undefined ? { width: result.width } : {}),
            ...(result.height !== undefined ? { height: result.height } : {}),
            ...(result.limitedDetail !== undefined
              ? { limitedDetail: result.limitedDetail }
              : {}),
            url: result.url,
          },
        });
      }
    }
    updateControls();
    void prefetchAdjacent(currentIndex - 1, generation, signal);
    void prefetchAdjacent(currentIndex + 1, generation, signal);
    return true;
  };
  const prefetchAdjacent = async (
    index: number,
    generation: number,
    signal: AbortSignal,
  ) => {
    if (generation !== requestGeneration) return;
    if (index < 0 || index >= total) return;
    let photo = loaded.get(index);
    if (!photo) {
      await loadWindow(index, sourceGeneration, true, signal);
      if (generation !== requestGeneration || signal.aborted) return;
      photo = loaded.get(index);
    }
    if (!photo || !photo.available) return;
    try {
      await fetcher(`/api/photos/${photo.id}/preview?priority=adjacent`, {
        signal,
        priority: "low",
      });
    } catch {
      /* adjacent work is best effort */
    }
  };
  const showGrid = () => {
    currentPhotoMode = false;
    openingPhoto = false;
    requestGeneration += 1;
    const photoTransition = recoveryGate.beginTransition(
      "photo",
      String(requestGeneration),
    );
    recoveryGate.succeedTransition(photoTransition);
    setConnected(true);
    renewPhotoTasks();
    cancelPendingImageLoads(stage, true);
    photoView.hidden = true;
    gridView.hidden = false;
    closeSources(false);
    renderGrid();
    cancelScheduledGridRender();
    scheduleGridRender(() => {
      gridViewport.scrollTop =
        Math.floor(currentIndex / columns()) * GRID_CELL_HEIGHT;
      renderGrid();
    });
    updateControls();
  };
  const persistPosition = (): Promise<boolean> => {
    if (sourceKind !== "album" || !sourceSetId || !currentPhoto())
      return Promise.resolve(true);
    const albumId = sourceSetId;
    const photoId = currentPhoto()!.id;
    const generation = sourceGeneration;
    const photoGeneration = requestGeneration;
    const stillCurrent = () =>
      applicationAlive &&
      generation === sourceGeneration &&
      photoGeneration === requestGeneration &&
      sourceKind === "album" &&
      sourceSetId === albumId &&
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
          failPhotoRecovery(photoGeneration, "saved-position");
        }
        return false;
      }
    });
    progressQueue = task.then(() => undefined);
    return task;
  };
  const moveTo = async (target: number) => {
    if (busy || openingPhoto || target < 0 || target >= total) return;
    await openPhoto(target);
  };
  const mutate = async (
    field: "selectionState" | "rating",
    value: SelectionState | number,
    advance: boolean,
  ) => {
    const photo = currentPhoto();
    if (!photo || !connected || busy) return;
    const generation = sourceGeneration;
    const photoGeneration = requestGeneration;
    const photoIndex = currentIndex;
    const albumId = sourceSetId;
    const prior = undo;
    undo = undefined;
    busy = true;
    status.textContent = `Saving ${field === "rating" ? "Rating" : "Selection State"}…`;
    updateControls();
    try {
      const response = await fetcher(`/api/photos/${photo.id}/state`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          field,
          value,
          ...(albumId ? { albumId } : {}),
        }),
      });
      if (!response.ok) {
        if (generation !== sourceGeneration) return;
        undo = prior;
        status.textContent =
          response.status === 409
            ? "The Photo changed elsewhere. Retry to refresh its current state."
            : "The change could not be saved.";
        if (
          response.status === 409 &&
          photoGeneration === requestGeneration &&
          currentPhoto()?.id === photo.id
        )
          failPhotoRecovery(photoGeneration, "photo-write");
        return;
      }
      const result = (await response.json()) as MutationResponse;
      if (
        generation !== sourceGeneration ||
        loaded.get(photoIndex)?.id !== photo.id
      )
        return;
      const updated = {
        ...photo,
        ...(field === "selectionState"
          ? { selectionState: value as SelectionState }
          : { rating: value as number }),
      };
      loaded.set(photoIndex, updated);
      undo = {
        ...result.undo,
        advanced: advance && photoIndex < total - 1,
        snapshotIndex: photoIndex,
      };
      status.textContent = `${field === "rating" ? "Rating" : "Selection"} saved.`;
      if (undo.advanced) {
        busy = false;
        await moveTo(currentIndex + 1);
      } else renderPhotoFacts();
    } catch {
      if (
        generation !== sourceGeneration ||
        photoGeneration !== requestGeneration ||
        currentPhoto()?.id !== photo.id
      )
        return;
      undo = undefined;
      status.textContent =
        "Connection lost before the change was confirmed. Retry to refresh.";
      failPhotoRecovery(photoGeneration, "photo-write");
    } finally {
      if (generation === sourceGeneration) {
        busy = false;
        updateControls();
      }
    }
  };
  const performUndo = async () => {
    const action = undo;
    if (!action || !connected || busy) return;
    const generation = sourceGeneration;
    const photoGeneration = requestGeneration;
    const albumId = sourceSetId;
    const affectedIndex = action.snapshotIndex;
    busy = true;
    updateControls();
    try {
      if (!loaded.has(affectedIndex)) {
        status.textContent = "Loading Photo for Undo…";
        const windowReady = await loadWindow(
          affectedIndex,
          generation,
          true,
          photoSignal,
          "high",
        );
        if (!windowReady) return;
      }
      if (
        generation !== sourceGeneration ||
        photoGeneration !== requestGeneration ||
        undo !== action
      )
        return;
      const photo = loaded.get(affectedIndex);
      if (!photo || photo.id !== action.photoId) return;
      undo = undefined;
      const response = await fetcher(`/api/photos/${action.photoId}/state`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          field: action.field,
          value: action.priorValue,
          expectedCurrent: action.expectedCurrent,
          ...(albumId ? { albumId } : {}),
        }),
      });
      if (!response.ok) {
        if (
          generation !== sourceGeneration ||
          photoGeneration !== requestGeneration
        )
          return;
        if (response.status === 409) {
          status.textContent =
            "Undo is no longer available because the Photo changed elsewhere. Retry to refresh its current state.";
          failPhotoRecovery(photoGeneration, "undo");
        } else {
          undo = action;
          status.textContent = "Undo could not be saved. Try Undo again.";
        }
        return;
      }
      if (
        generation !== sourceGeneration ||
        photoGeneration !== requestGeneration
      )
        return;
      const updated = {
        ...photo,
        ...(action.field === "selectionState"
          ? { selectionState: action.priorValue as SelectionState }
          : { rating: action.priorValue as number }),
      };
      loaded.set(affectedIndex, updated);
      trimLoaded(affectedIndex);
      currentIndex = affectedIndex;
      currentPhotoMode = true;
      gridView.hidden = true;
      photoView.hidden = false;
      syncSourcePanel();
      photoView.focus();
      resetTransform();
      const previewGeneration = ++requestGeneration;
      const photoTransition = recoveryGate.beginTransition(
        "photo",
        String(previewGeneration),
      );
      syncConnection();
      renewPhotoTasks();
      const signal = photoSignal;
      cancelPendingImageLoads(stage, true);
      renderPhotoShell(previewGeneration);
      busy = false;
      updateControls();
      await showPreview(previewGeneration, signal, photoTransition);
      if (
        generation !== sourceGeneration ||
        previewGeneration !== requestGeneration ||
        currentPhoto()?.id !== action.photoId
      )
        return;
      const persisted = await persistPosition();
      if (
        persisted &&
        generation === sourceGeneration &&
        previewGeneration === requestGeneration &&
        currentPhoto()?.id === action.photoId
      ) {
        recoveryGate.succeedTransition(photoTransition);
        setConnected(true);
        status.textContent = "Last change undone.";
      }
    } catch {
      if (
        generation === sourceGeneration &&
        photoGeneration === requestGeneration
      ) {
        status.textContent = "Connection lost before Undo was confirmed.";
        failPhotoRecovery(photoGeneration, "undo");
      }
    } finally {
      if (generation === sourceGeneration) {
        busy = false;
        updateControls();
      }
    }
  };

  const pointerDown = (event: PointerEvent) => {
    const photo = currentPhoto();
    if (pointer || !event.isPrimary || busy || !connected || !photo) return;
    pointer = {
      id: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      startedAt: event.timeStamp,
      vertical: false,
      generation: requestGeneration,
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
      active.generation !== requestGeneration ||
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
    if (modifier || event.shiftKey || !currentPhotoMode) return;
    if (event.key === "ArrowLeft") void moveTo(currentIndex - 1);
    else if (event.key === "ArrowRight") void moveTo(currentIndex + 1);
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
    const generation = sourceGeneration;
    gridRenderFrame = requestAnimationFrame(() => {
      gridRenderFrame = undefined;
      if (generation !== sourceGeneration) return;
      render();
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
    requestAnimationFrame(() => {
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
          Math.floor(currentIndex / nextColumns) * GRID_CELL_HEIGHT;
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
      if (sourceKind === "folder" && !fileLocations.publication) {
        await awaitRootBinding();
        if (!fileLocations.publication) {
          gridStatus.textContent =
            "Could not load this source. Retry to continue.";
          return;
        }
      }
      await openSource(
        sourceKind,
        sourceKind === "album"
          ? application.albums.find((set) => set.id === sourceSetId)
          : undefined,
        undefined,
        sourceKind === "folder" ? sourceFolder : undefined,
      );
    })();
  });
  retry.addEventListener("click", () => {
    if (!retrySourceRequired) {
      void application.loadOverview();
      return;
    }
    const remembered = lastSource ?? {
      kind: "library" as const,
      set: undefined,
      folder: undefined,
    };
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
      await openSource(
        remembered.kind,
        remembered.set,
        undefined,
        remembered.folder,
      );
    })();
  });
  retryPhoto.addEventListener("click", () => {
    void (async () => {
      busy = true;
      status.textContent = "Reconnecting…";
      const sourceOwner = sourceGeneration;
      const photoId = currentPhoto()?.id;
      const generation = ++requestGeneration;
      const photoTransition = recoveryGate.beginTransition(
        "photo",
        String(generation),
      );
      syncConnection();
      renewPhotoTasks();
      const signal = photoSignal;
      cancelPendingImageLoads(stage, true);
      updateControls();
      try {
        const start = alignedStart(currentIndex);
        const end = Math.min(total, start + WINDOW_SIZE);
        for (let index = start; index < end; index += 1) loaded.delete(index);
        const windowReady = await loadWindow(
          start,
          sourceOwner,
          true,
          signal,
          "high",
          photoTransition,
        );
        if (
          !windowReady ||
          sourceOwner !== sourceGeneration ||
          generation !== requestGeneration ||
          currentPhoto()?.id !== photoId
        )
          return;
        const refreshed = await showPreview(
          generation,
          signal,
          photoTransition,
        );
        const persisted = refreshed && (await persistPosition());
        if (
          refreshed &&
          persisted &&
          sourceOwner === sourceGeneration &&
          generation === requestGeneration
        ) {
          recoveryGate.succeedTransition(photoTransition);
          setConnected(true);
          status.textContent = "Connected. Current state refreshed.";
        }
      } finally {
        if (
          sourceOwner === sourceGeneration &&
          generation === requestGeneration
        ) {
          busy = false;
          updateControls();
        }
      }
    })();
  });
  previous.addEventListener("click", () => void moveTo(currentIndex - 1));
  next.addEventListener("click", () => void moveTo(currentIndex + 1));
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
    sourceGeneration += 1;
    requestGeneration += 1;
    const browseToken = token;
    token = "";
    cancelScheduledGridRender();
    cancelPendingImageLoads(gridLayer, true);
    cancelPendingImageLoads(stage, true);
    application.dispose();
    fileLocations.dispose();
    sourceTasks.halt();
    gridTasks.halt();
    photoTasks.halt();
    recoveryGate.close();
    window.removeEventListener("keydown", keydown);
    window.removeEventListener("resize", onResize);
    compactSources.removeEventListener("change", onSourceViewportChange);
    if (browseToken) void closeBrowse(browseToken);
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function validOptional(
  value: unknown,
  predicate: (candidate: unknown) => boolean,
): boolean {
  return value === undefined || predicate(value);
}

function validPhotoSummary(value: unknown): value is PhotoSummary {
  if (!isRecord(value) || !isRecord(value.preview)) return false;
  const preview = value.preview;
  const previewState = preview.state;
  if (
    previewState !== "inspection-pending" &&
    previewState !== "ready" &&
    previewState !== "failed" &&
    previewState !== "unavailable"
  )
    return false;
  if (
    !Array.isArray(value.originals) ||
    !value.originals.every(
      (original) =>
        isRecord(original) &&
        (original.kind === "raw" || original.kind === "jpeg") &&
        typeof original.available === "boolean",
    )
  )
    return false;
  return (
    typeof value.id === "string" &&
    value.id.length > 0 &&
    typeof value.available === "boolean" &&
    typeof value.ambiguous === "boolean" &&
    (value.selectionState === "undecided" ||
      value.selectionState === "selected" ||
      value.selectionState === "rejected") &&
    Number.isInteger(value.rating) &&
    Number(value.rating) >= 0 &&
    Number(value.rating) <= 5 &&
    validOptional(
      preview.source,
      (source) => source === "matching-jpeg" || source === "embedded-raw-jpeg",
    ) &&
    validOptional(preview.width, Number.isInteger) &&
    validOptional(preview.height, Number.isInteger) &&
    validOptional(preview.limitedDetail, (item) => typeof item === "boolean") &&
    validOptional(preview.url, (item) => typeof item === "string") &&
    validOptional(preview.thumbnailUrl, (item) => typeof item === "string") &&
    validOptional(preview.message, (item) => typeof item === "string")
  );
}

function decodeBrowseWindow(
  value: unknown,
  expectedStart: number,
  expectedTotal: number,
): BrowseWindowResponse | undefined {
  if (
    !isRecord(value) ||
    value.start !== expectedStart ||
    value.total !== expectedTotal ||
    !Array.isArray(value.photos) ||
    value.photos.length > WINDOW_SIZE ||
    expectedStart + value.photos.length > expectedTotal ||
    !value.photos.every(validPhotoSummary)
  )
    return undefined;
  return value as BrowseWindowResponse;
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
