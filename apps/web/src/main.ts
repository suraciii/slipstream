import {
  RecoveryGate,
  SettlementFamily,
  SummaryNoticeChannel,
  TaskScope,
  type NoticeHandle,
  type RecoveryClaim,
  type NoticeUpdate,
  type RecoveryTransition,
} from "./async-ownership.js";
import type {
  AlbumSummary,
  BrowseOpenResponse,
  BrowseWindowResponse,
  FileLocationsResponse,
  FolderChild,
  LibraryOverviewResponse,
  PhotoSummary,
  PreviewResponse,
  PreviewSource,
  SelectionState,
  UndoDescription,
} from "./protocol.js";
import "./style.css";

type SessionUndo = UndoDescription & Readonly<{ advanced: boolean }>;
type MutationResponse = Readonly<{ undo: UndoDescription }>;
type SummaryMessage = Readonly<{
  text: string;
  action?: Readonly<{ label: string; run: () => void | Promise<void> }>;
}>;

const WINDOW_SIZE = 60;
const MAX_RETAINED_FACTS = WINDOW_SIZE * 3;
const MAX_RETAINED_THUMBNAILS = WINDOW_SIZE * 4;
const GRID_CELL_HEIGHT = 178;
const GRID_CELL_WIDTH = 150;
const swipePendingPixels = 24;
const swipeCommitPixels = 72;
const swipeCommitVelocity = 0.5;

export function renderApp(
  root: HTMLElement,
  fetcher: typeof fetch = fetch,
): () => void {
  let applicationAlive = true;
  const applicationTasks = new TaskScope();
  const recoveryGate = new RecoveryGate();
  const albumSettlements = new SettlementFamily();
  const scanSettlements = new SettlementFamily();
  const summaryNotices = new SummaryNoticeChannel<SummaryMessage>();
  let overviewDataFloor = 0;

  root.innerHTML = `
    <main class="app-shell">
      <header class="app-header"><h1>Slipstream</h1><p data-connection role="status">Connecting…</p></header>
      <section class="browser" data-browser aria-labelledby="browser-title">
        <aside class="source-panel" data-library-screen aria-label="Library sources">
          <h2 id="browser-title">Library</h2>
          <p data-summary-status role="status">Loading Library…</p>
          <div class="source-list" data-source-list></div>
          <button type="button" data-retry hidden>Retry connection</button>
        </aside>
        <section class="grid-view" data-grid-view aria-labelledby="grid-title">
          <header class="grid-header"><div><h2 id="grid-title" data-grid-title>All Photos</h2><p data-grid-status role="status"></p></div><button type="button" data-refresh>Refresh</button></header>
          <div class="grid-viewport" data-grid-viewport tabindex="0" aria-label="Photo Library Grid"><div class="grid-canvas" data-grid-canvas></div><div class="grid-layer" data-grid-layer></div></div>
        </section>
        <section class="photo-view" data-review data-photo-view hidden tabindex="-1" aria-labelledby="photo-title">
          <header class="photo-header"><button type="button" class="quiet" data-back>Back to Grid</button><div><h2 id="photo-title" data-photo-title>Photo</h2><p data-position>0 / 0</p></div><button type="button" class="quiet" data-retry-photo hidden>Retry</button></header>
          <section class="preview" data-preview aria-label="Photo Preview">
            <div class="swipe-feedback reject" data-reject-feedback>Reject</div>
            <div class="image-stage" data-stage><p>Loading Preview…</p></div>
            <div class="swipe-feedback select" data-select-feedback>Select</div>
          </section>
          <dl class="facts"><div><dt>Selection</dt><dd data-selection>Undecided</dd></div><div><dt>Rating</dt><dd data-rating>0 stars</dd></div><div><dt>Preview Source</dt><dd data-source>—</dd></div><div data-limited hidden><dt>Detail</dt><dd>Limited by camera Preview resolution</dd></div></dl>
          <p class="status" data-status role="status" aria-live="polite"></p>
          <div class="decision-controls" aria-label="Selection controls"><button type="button" class="reject-button" data-reject>Reject <span aria-hidden="true">X</span></button><button type="button" data-clear>Clear <span aria-hidden="true">U</span></button><button type="button" class="select-button" data-select>Select <span aria-hidden="true">P</span></button></div>
          <div class="membership-controls" aria-label="Album membership"><label for="album-select">Album</label><select id="album-select" data-album-select></select><button type="button" data-add-to-album>Add to Album</button><button type="button" data-remove-from-album hidden>Remove from this Album</button></div>
          <fieldset class="rating-controls"><legend>Rating</legend><div data-ratings></div></fieldset>
          <div class="photo-controls"><button type="button" data-previous>Previous</button><button type="button" data-detail aria-pressed="false">Detail Review</button><button type="button" data-undo disabled>Undo</button><button type="button" data-next>Next</button></div>
        </section>
      </section>
    </main>`;

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

  for (let value = 0; value <= 5; value += 1) {
    const button = document.createElement("button");
    button.type = "button";
    button.dataset.ratingValue = String(value);
    button.setAttribute(
      "aria-label",
      value === 0 ? "Clear Rating" : `Rate ${value} stars`,
    );
    button.textContent = value === 0 ? "0" : "★".repeat(value);
    ratings.append(button);
  }

  let overview: LibraryOverviewResponse | undefined;
  const summaryMessage = (
    text: string,
    action?: SummaryMessage["action"],
  ): SummaryMessage => ({ text, ...(action ? { action } : {}) });
  const applySummaryUpdate = (update: NoticeUpdate<SummaryMessage>): void => {
    if (!applicationAlive || !("visible" in update)) return;
    if (update.visible === null) {
      const epoch = summaryNotices.backgroundEpoch();
      applySummaryUpdate(
        summaryNotices.presentBackground(
          epoch,
          summaryMessage(
            overview ? scanLabel(overview.scan) : "Loading Library…",
          ),
        ),
      );
      return;
    }
    if (update.visible === undefined) return;
    summaryStatusElement.replaceChildren(
      document.createTextNode(update.visible.text),
    );
    if (update.visible.action) {
      const action = document.createElement("button");
      action.type = "button";
      action.className = "summary-action";
      action.textContent = update.visible.action.label;
      action.addEventListener("click", () => {
        void update.visible!.action!.run();
      });
      summaryStatusElement.append(" ", action);
    }
  };
  let sets: ReadonlyArray<AlbumSummary> = [];
  let token = "";
  let total = 0;
  let currentIndex = 0;
  let sourceKind: "library" | "album" | "folder" = "library";
  let sourceSetId: string | undefined;
  let sourceSetName = "All Photos";
  let sourceFolder: { location: string; name: string } | undefined;
  // File Location navigation retains bounded per-parent windows from one
  // publication. A newer publication clears them and reloads coherently.
  let fileLocationPublication: string | undefined;
  const folderWindows = new Map<
    string,
    { page: number; children: FolderChild[]; total: number }
  >();
  const expandedFolders = new Set<string>();
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

  const ALBUM_NOTICE_PRIORITY = 10;
  const ACTIONABLE_NOTICE_PRIORITY = 30;
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
  let overviewRecovery: RecoveryClaim | undefined;
  let busy = false;
  let openingPhoto = false;
  let currentPhotoMode = false;
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
  const failSourceRecovery = (generation: number, kind: string): void => {
    const claim = recoveryGate.issue(kind, String(generation), {
      owner: { scope: "source", generation: String(generation) },
    });
    if (!recoveryGate.fail(claim, { transportLost: true }))
      recoveryGate.discard(claim);
    syncConnection();
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
      clear,
      ...Array.from(ratings.querySelectorAll<HTMLButtonElement>("button")),
    ])
      button.disabled = !enabled;
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

  // ---- File Location navigation -------------------------------------
  // Retained Folder state is bounded by construction: each expanded parent
  // retains exactly one current direct-child page, and the expanded set is
  // capped with FIFO eviction. A navigation generation plus the retained
  // publication guard against delayed responses from superseded generations.
  const FOLDER_PAGE_SIZE = 60;
  const MAXIMUM_EXPANDED_FOLDERS = 32;
  type FolderPage = {
    page: number;
    children: FolderChild[];
    total: number;
  };
  let fileLocationGeneration = 0;
  let fileLocationTasks = new TaskScope();
  let fileLocationNotice: NoticeHandle | undefined;
  let fileLocationRecovery: RecoveryClaim | undefined;
  let fileLocationNoticeKind: "range" | "publication" | undefined;
  let folderFailure:
    | {
        parent: string;
        page: number;
        notice: NoticeHandle;
        recovery: RecoveryClaim;
      }
    | undefined;
  const resetFileLocations = () => {
    fileLocationGeneration += 1;
    fileLocationTasks.halt();
    fileLocationTasks = new TaskScope();
    fileLocationPublication = undefined;
    folderWindows.clear();
    expandedFolders.clear();
    if (fileLocationNotice)
      applySummaryUpdate(summaryNotices.release(fileLocationNotice));
    if (fileLocationRecovery) recoveryGate.recover(fileLocationRecovery);
    fileLocationNotice = undefined;
    fileLocationRecovery = undefined;
    fileLocationNoticeKind = undefined;
    folderFailure = undefined;
    syncConnection();
    // Re-render at once: retained Folder cards must not stay clickable with
    // an unbound publication, and a cleared failure must remove its Retry
    // control before any handler can dereference it.
    renderSources();
  };

  /// Awaits one shared root bind. Resetting the File Location scope detaches
  /// the operation and settles every joiner with an unbound result.
  const awaitRootBinding = async () => {
    if (fileLocationPublication) return true;
    const scope = fileLocationTasks;
    const shared = scope.joinOrStart(
      "root-binding",
      { abortTransport: false },
      async () => {
        await requestFolderWindow("", 0, false, scope);
        return scope === fileLocationTasks && Boolean(fileLocationPublication);
      },
    );
    try {
      return await shared.promise;
    } catch {
      return false;
    }
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
    const settlement = albumSettlements.begin({
      ...(admissionKey ? { admissionKey } : {}),
      ownsSurface: ownsPhotoSurface,
    });
    if (!settlement)
      return Promise.resolve({
        admitted: false,
        ok: false,
        announce: () => {},
      });
    const notice = summaryNotices.issue(
      "album",
      admissionKey ?? path,
      ALBUM_NOTICE_PRIORITY,
    );
    const disconnect = () => {
      const claim = recoveryGate.issue("album", path, {
        owner: { scope: "source", generation: String(sourceOwner) },
      });
      if (!recoveryGate.fail(claim, { transportLost: true }))
        recoveryGate.discard(claim);
      syncConnection();
    };
    const report = (text: string) => {
      if (settlement.canPresent() && surface === "photo")
        status.textContent = text;
      else
        applySummaryUpdate(
          summaryNotices.present(notice, summaryMessage(text), {
            fallback: true,
          }),
        );
    };
    return (async () => {
      try {
        const response = await fetcher(path, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        });
        if (!response.ok) {
          report(failure(response.status));
          if (response.status >= 500 && settlement.isNewest()) disconnect();
          settlement.finish();
          return { admitted: true, ok: false, announce: () => {} };
        }
        overviewDataFloor += 1;
        try {
          await refreshOverviewState({
            connectivity: () => settlement.isNewest(),
          });
          applySummaryUpdate(summaryNotices.releaseKind("album", notice));
        } catch {
          // Persistence is already durable. Only the globally newest Album
          // settlement owns connectivity and the refresh-failure notice.
          if (settlement.isNewest()) {
            disconnect();
            applySummaryUpdate(
              summaryNotices.present(
                notice,
                summaryMessage(
                  "The Album was saved but the Library summary could not be refreshed.",
                ),
                { fallback: true },
              ),
            );
          } else {
            applySummaryUpdate(summaryNotices.release(notice));
          }
        }
        const mayAnnounce = settlement.canPresent();
        settlement.finish();
        return {
          admitted: true,
          ok: true,
          announce: (text: string) => {
            if (mayAnnounce && ownsPhotoSurface()) status.textContent = text;
          },
        };
      } catch {
        report(failure(0));
        if (settlement.isNewest()) disconnect();
        settlement.finish();
        return { admitted: true, ok: false, announce: () => {} };
      }
    })();
  };

  const describeRange = (parent: string, page: number) =>
    `${parent || "Library Folder"} items ${(page * FOLDER_PAGE_SIZE + 1).toLocaleString()}–${((page + 1) * FOLDER_PAGE_SIZE).toLocaleString()}`;

  const releaseFileLocationRecovery = (): void => {
    if (fileLocationNotice)
      applySummaryUpdate(summaryNotices.release(fileLocationNotice));
    if (fileLocationRecovery) recoveryGate.recover(fileLocationRecovery);
    fileLocationNotice = undefined;
    fileLocationRecovery = undefined;
    fileLocationNoticeKind = undefined;
    syncConnection();
  };
  const claimFileLocationNotice = (
    kind: "range" | "publication",
    key: string,
    text: string,
    transportLost: boolean,
  ): { notice: NoticeHandle; recovery: RecoveryClaim } => {
    releaseFileLocationRecovery();
    const notice = summaryNotices.issue(
      "file-location",
      key,
      ACTIONABLE_NOTICE_PRIORITY,
    );
    const recovery = recoveryGate.issue("file-location", key);
    recoveryGate.fail(recovery, { transportLost });
    fileLocationNotice = notice;
    fileLocationRecovery = recovery;
    fileLocationNoticeKind = kind;
    applySummaryUpdate(summaryNotices.present(notice, summaryMessage(text)));
    syncConnection();
    return { notice, recovery };
  };

  async function requestFolderWindow(
    parent: string,
    page: number,
    expand: boolean,
    scope = fileLocationTasks,
  ): Promise<void> {
    if (scope.halted) return;
    const task = scope.beginLatest(`folder:${parent}`, {
      abortTransport: false,
    });
    const generation = fileLocationGeneration;
    const parameters = new URLSearchParams({
      start: String(page * FOLDER_PAGE_SIZE),
      limit: String(FOLDER_PAGE_SIZE),
    });
    if (parent) parameters.set("parent", parent);
    const boundPublication = fileLocationPublication;
    if (boundPublication) parameters.set("publication", boundPublication);
    try {
      const response = await fetcher(`/api/file-locations?${parameters}`);
      if (!task.isCurrent() || scope !== fileLocationTasks) return;
      if (response.status === 409) {
        overviewDataFloor += 1;
        resetFileLocations();
        await refreshOverviewState().catch(() => {});
        await loadFolderWindow("", 0);
        if (fileLocationPublication)
          claimFileLocationNotice(
            "publication",
            `publication:${fileLocationPublication}`,
            "Scan results changed File Locations. Reloaded the current Folders.",
            false,
          );
        return;
      }
      if (!response.ok) throw new Error("file locations failed");
      const window = (await response.json()) as FileLocationsResponse;
      if (!task.isCurrent() || scope !== fileLocationTasks) return;
      if (boundPublication && window.publication !== boundPublication) return;
      fileLocationPublication = window.publication;
      folderWindows.set(parent, {
        page,
        children: [...window.children],
        total: window.total,
      });
      if (expand) {
        expandedFolders.add(parent);
        enforceExpandedCap(parent);
      }
      if (folderFailure) {
        if (folderFailure.parent === parent && folderFailure.page === page) {
          const recovered = folderFailure;
          folderFailure = undefined;
          if (
            fileLocationNotice === recovered.notice &&
            fileLocationRecovery === recovered.recovery
          )
            releaseFileLocationRecovery();
          else {
            applySummaryUpdate(summaryNotices.release(recovered.notice));
            recoveryGate.recover(recovered.recovery);
          }
        }
        setConnected(true);
      }
      renderSources();
    } catch {
      if (!task.isCurrent() || scope !== fileLocationTasks) return;
      const { notice, recovery } = claimFileLocationNotice(
        "range",
        `range:${generation}:${parent}:${page}`,
        `Could not load File Locations (${describeRange(parent, page)}). Retry to continue.`,
        true,
      );
      folderFailure = { parent, page, notice, recovery };
      renderSources();
    } finally {
      task.finish();
    }
  }

  async function loadFolderWindow(
    parent: string,
    page: number,
    expand = true,
  ): Promise<void> {
    if (parent === "" && !fileLocationPublication) {
      const bound = await awaitRootBinding();
      if (bound && expand) {
        expandedFolders.add("");
        enforceExpandedCap("");
        renderSources();
      }
      return;
    }
    await requestFolderWindow(parent, page, expand);
  }

  const enforceExpandedCap = (newest: string) => {
    while (expandedFolders.size > MAXIMUM_EXPANDED_FOLDERS) {
      const oldest = [...expandedFolders].find(
        (location) => location !== newest && location !== "",
      );
      if (oldest === undefined) break;
      expandedFolders.delete(oldest);
      folderWindows.delete(oldest);
    }
  };

  const folderPager = (parent: string, retained: FolderPage) => {
    const depth = parent ? parent.split("/").length : 0;
    const pages = Math.max(1, Math.ceil(retained.total / FOLDER_PAGE_SIZE));
    const controls = document.createElement("div");
    controls.className = "folder-pager";
    controls.style.marginLeft = `${Math.min(depth, 6) * 12}px`;
    const previous = document.createElement("button");
    previous.type = "button";
    previous.className = "folder-page-button";
    previous.textContent = "Previous Folders";
    previous.disabled = retained.page === 0;
    previous.addEventListener("click", () => {
      const current = folderWindows.get(parent);
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
    next.disabled = (retained.page + 1) * FOLDER_PAGE_SIZE >= retained.total;
    next.addEventListener("click", () => {
      // Read the current page at click time: the retained entry is replaced
      // by each response, so a stale closure would replay the same page.
      const current = folderWindows.get(parent);
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
      expandedFolders.has(child.location) ? "true" : "false",
    );
    expand.textContent = expandedFolders.has(child.location) ? "▾" : "▸";
    expand.setAttribute("aria-label", `Toggle ${child.name} subfolders`);
    expand.addEventListener("click", () => {
      if (expandedFolders.has(child.location)) {
        expandedFolders.delete(child.location);
        folderWindows.delete(child.location);
        renderSources();
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
    button.disabled = !fileLocationPublication;
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
    if (!expandedFolders.has(child.location)) return row;
    const fragment = document.createDocumentFragment();
    fragment.append(row, folderChildrenFragment(child.location));
    return fragment;
  };

  const folderChildrenFragment = (parent: string) => {
    const retained = folderWindows.get(parent);
    if (!retained || !expandedFolders.has(parent))
      return document.createDocumentFragment();
    const fragment = document.createDocumentFragment();
    for (const child of retained.children) fragment.append(folderCard(child));
    if (retained.total > FOLDER_PAGE_SIZE)
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
      overview?.photoCount ?? 0,
      sourceKind === "library",
    );
    libraryButton.addEventListener("click", () => void openSource("library"));
    sourceList.append(libraryButton);

    const fileHeading = document.createElement("h3");
    fileHeading.textContent = "File Locations";
    sourceList.append(fileHeading);
    if (folderFailure) {
      const retryFolders = document.createElement("button");
      retryFolders.type = "button";
      retryFolders.className = "folder-more";
      retryFolders.textContent = `Retry File Locations (${describeRange(folderFailure.parent, folderFailure.page)})`;
      retryFolders.addEventListener("click", () => {
        // The control only renders while folderFailure is set, and reset
        // removes it on the same render that clears the failure.
        const failure = folderFailure;
        if (failure) void loadFolderWindow(failure.parent, failure.page);
      });
      sourceList.append(retryFolders);
    }
    const rootCard = sourceButton(
      "Library Folder",
      overview?.photoCount ?? 0,
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
    rootCard.disabled = !fileLocationPublication;
    const rootRow = document.createElement("div");
    rootRow.className = "folder-row folder-root";
    const rootExpand = document.createElement("button");
    rootExpand.type = "button";
    rootExpand.className = "folder-expand";
    rootExpand.setAttribute(
      "aria-expanded",
      expandedFolders.has("") ? "true" : "false",
    );
    rootExpand.textContent = expandedFolders.has("") ? "▾" : "▸";
    rootExpand.setAttribute("aria-label", "Toggle Library Folder subfolders");
    rootExpand.addEventListener("click", () => {
      if (expandedFolders.has("")) {
        expandedFolders.delete("");
        renderSources();
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
    for (const set of sets) {
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

  const scanLabel = (scan: LibraryOverviewResponse["scan"]): string => {
    const completed = scan.completed?.toLocaleString();
    switch (scan.state) {
      case "idle":
        return "Library ready";
      case "initializing":
        return "Preparing Photo Library…";
      case "discovering":
        return completed
          ? `Checking Library Folder… ${completed} found`
          : "Checking Library Folder…";
      case "inspecting":
        return scan.total
          ? `Inspecting Capture Time… ${completed ?? 0} / ${scan.total.toLocaleString()}`
          : "Inspecting Capture Time…";
      case "applying":
        return "Applying Library updates…";
      case "failed":
        return "Library check failed; the last complete Library remains available";
      default:
        return `Library ${scan.state}`;
    }
  };
  type ScanCycle = { consumed: boolean };
  let observedScanState: string | undefined;
  let scanFailureNotice: NoticeHandle | undefined;
  let scanCompletionNotice: NoticeHandle | undefined;
  let scanCommandRecovery: RecoveryClaim | undefined;
  let activeScanCycle: ScanCycle | undefined;

  const releaseScanFailure = () => {
    if (!scanFailureNotice) return;
    applySummaryUpdate(summaryNotices.release(scanFailureNotice));
    scanFailureNotice = undefined;
  };
  const claimScanFailure = () => {
    if (!scanFailureNotice)
      scanFailureNotice = summaryNotices.issue(
        "scan-failure",
        "library",
        ACTIONABLE_NOTICE_PRIORITY,
      );
    applySummaryUpdate(
      summaryNotices.present(
        scanFailureNotice,
        summaryMessage(
          "Library check failed; the last complete Library remains available.",
          { label: "Retry Library Check", run: retryLibraryCheck },
        ),
      ),
    );
  };
  const completeScan = (cycle = activeScanCycle): void => {
    if (cycle?.consumed) return;
    if (cycle) cycle.consumed = true;
    if (!applicationAlive) return;
    if (activeScanCycle === cycle) activeScanCycle = undefined;
    overviewDataFloor += 1;
    releaseScanFailure();
    if (scanCompletionNotice)
      applySummaryUpdate(summaryNotices.release(scanCompletionNotice));
    const notice = summaryNotices.issue(
      "scan-completion",
      "library",
      ACTIONABLE_NOTICE_PRIORITY,
    );
    scanCompletionNotice = notice;
    applySummaryUpdate(
      summaryNotices.present(
        notice,
        summaryMessage(
          "Library check complete. Open Browse Snapshots remain unchanged.",
          {
            label: "Refresh Current Source",
            run: () => {
              if (scanCompletionNotice === notice) {
                scanCompletionNotice = undefined;
                applySummaryUpdate(summaryNotices.release(notice));
              }
              refresh.click();
            },
          },
        ),
      ),
    );
    resetFileLocations();
    void refreshOverviewState()
      .then(() => loadFolderWindow("", 0, false))
      .catch(() => {});
  };
  const ensureStatusMonitor = (
    baseline?: LibraryOverviewResponse["scan"],
  ): void => {
    if (observedScanState === undefined && baseline)
      observedScanState = baseline.state;
    if (applicationTasks.current("publication-status")) return;
    const monitor = applicationTasks.beginLatest("publication-status", {
      abortTransport: false,
    });
    let timer: ReturnType<typeof setTimeout> | undefined;
    let wake: (() => void) | undefined;
    monitor.onCleanup(() => {
      if (timer) clearTimeout(timer);
      timer = undefined;
      wake?.();
      wake = undefined;
    });
    const pause = () =>
      new Promise<void>((resolve) => {
        wake = resolve;
        timer = setTimeout(
          () => {
            timer = undefined;
            wake = undefined;
            resolve();
          },
          observedScanState === "idle" ? 2_000 : 500,
        );
      });
    void (async () => {
      try {
        while (monitor.isCurrent()) {
          await pause();
          if (!monitor.isCurrent()) return;
          const background = summaryNotices.backgroundEpoch();
          let settled = false;
          try {
            const response = await fetcher("/api/status");
            if (!response.ok) continue;
            const scan =
              (await response.json()) as LibraryOverviewResponse["scan"];
            if (!monitor.isCurrent()) return;
            applySummaryUpdate(
              summaryNotices.presentBackground(
                background,
                summaryMessage(scanLabel(scan)),
              ),
            );
            settled = true;
            const prior = observedScanState;
            observedScanState = scan.state;
            if (scan.state === "failed") {
              claimScanFailure();
              return;
            }
            if (scan.state === "idle" && prior && prior !== "idle")
              completeScan();
          } catch {
            /* answered and transport status failures stay silent */
          } finally {
            if (!settled) summaryNotices.discardBackground(background);
          }
        }
      } finally {
        monitor.finish();
      }
    })();
  };
  const retryLibraryCheck = async (): Promise<void> => {
    const settlement = scanSettlements.begin({ admissionKey: "scan" });
    if (!settlement) return;
    const cycle = { consumed: false };
    activeScanCycle = cycle;
    observedScanState = "applying";
    ensureStatusMonitor({ state: "applying" });
    try {
      const response = await fetcher("/api/scan", { method: "POST" });
      if (!response.ok) {
        if (response.status >= 500) {
          if (
            !scanCommandRecovery ||
            !recoveryGate.isActive(scanCommandRecovery)
          ) {
            scanCommandRecovery = recoveryGate.issue("scan-command", "library");
            recoveryGate.fail(scanCommandRecovery, { transportLost: true });
          }
          syncConnection();
        }
        return;
      }
      const terminal =
        (await response.json()) as LibraryOverviewResponse["scan"];
      if (scanCommandRecovery) {
        recoveryGate.recover(scanCommandRecovery);
        scanCommandRecovery = undefined;
        setConnected(true);
      }
      observedScanState = terminal.state;
      if (terminal.state === "idle") completeScan(cycle);
      else if (terminal.state === "failed") claimScanFailure();
    } catch {
      if (!scanCommandRecovery || !recoveryGate.isActive(scanCommandRecovery)) {
        scanCommandRecovery = recoveryGate.issue("scan-command", "library");
        recoveryGate.fail(scanCommandRecovery, { transportLost: true });
      }
      syncConnection();
    } finally {
      settlement.finish();
    }
  };
  const electOverviewBootstrap = (committed: LibraryOverviewResponse): void => {
    const owner = applicationTasks.beginLatest("overview-bootstrap", {
      abortTransport: false,
    });
    void (async () => {
      try {
        if (!fileLocationPublication && committed.published) {
          // Root binding belongs to the independent File Location scope. A
          // superseded bootstrap awaits it but starts no later child.
          if (lastSource?.kind === "folder" && !token) {
            await awaitRootBinding();
            if (!owner.isCurrent()) return;
          } else {
            void loadFolderWindow("", 0, false);
          }
        }
        if (!owner.isCurrent()) return;
        if (!token && committed.published) {
          const remembered = lastSource ?? {
            kind: "library" as const,
            set: undefined,
            folder: undefined,
          };
          const bindable =
            remembered.kind !== "folder" ||
            fileLocationPublication !== undefined;
          if (bindable) {
            await openSource(
              remembered.kind,
              remembered.set,
              undefined,
              remembered.folder,
            );
          } else if (owner.isCurrent()) {
            gridStatus.textContent =
              "Could not load this source. Retry to continue.";
          }
        }
      } finally {
        owner.finish();
      }
    })();
  };

  const refreshOverviewState = async (options?: {
    connectivity?: () => boolean;
    bootstrap?: boolean;
  }): Promise<boolean> => {
    // The scope owns request sequencing and the latest committed sequence at
    // the current data floor. Requests remain concurrent: a newer failure
    // does not detach an older valid success.
    const task = applicationTasks.beginOrdered("overview", overviewDataFloor);
    const background = summaryNotices.backgroundEpoch();
    let backgroundSettled = false;
    try {
      const response = await fetcher("/api/overview");
      if (!response.ok) throw new Error("overview failed");
      const body = (await response.json()) as LibraryOverviewResponse;
      // Re-check both captured floor and commit order after body parsing.
      if (!task.commit(overviewDataFloor)) return false;
      overview = body;
      sets = body.albums;
      ensureStatusMonitor(body.scan);
      applySummaryUpdate(
        summaryNotices.presentBackground(
          background,
          summaryMessage(scanLabel(body.scan)),
        ),
      );
      backgroundSettled = true;
      if (options?.connectivity?.()) setConnected(true);
      if (sourceKind === "album" && sourceSetId) {
        const open = sets.find((candidate) => candidate.id === sourceSetId);
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
      if (options?.bootstrap) electOverviewBootstrap(body);
      return true;
    } finally {
      if (!backgroundSettled) summaryNotices.discardBackground(background);
      task.finish();
    }
  };

  const loadOverview = async () => {
    // Foreground reload ownership is latest-only, while its shared overview
    // child keeps commit-in-order ownership and may elect bootstrap even when
    // a newer foreground load fails.
    const load = applicationTasks.beginLatest("overview-load", {
      abortTransport: false,
    });
    const barrier = summaryNotices.beginBarrier(
      "reload",
      "overview",
      ACTIONABLE_NOTICE_PRIORITY,
      summaryMessage("Loading Library summary…"),
    );
    applySummaryUpdate(barrier.update);
    try {
      await refreshOverviewState({
        bootstrap: true,
        connectivity: () => load.isCurrent(),
      });
      if (load.isCurrent() && overviewRecovery) {
        recoveryGate.recover(overviewRecovery);
        overviewRecovery = undefined;
        setConnected(true);
      }
      applySummaryUpdate(summaryNotices.release(barrier.handle));
    } catch {
      if (!load.isCurrent()) return;
      applySummaryUpdate(
        summaryNotices.present(
          barrier.handle,
          summaryMessage(
            "Could not reach Slipstream. Check the server and retry.",
          ),
        ),
      );
      if (!overviewRecovery || !recoveryGate.isActive(overviewRecovery)) {
        overviewRecovery = recoveryGate.issue("overview-reload", "overview");
        recoveryGate.fail(overviewRecovery, { transportLost: true });
      }
      syncConnection();
    } finally {
      load.finish();
    }
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
                  ...(fileLocationPublication
                    ? { publication: fileLocationPublication }
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
        overviewDataFloor += 1;
        resetFileLocations();
        await refreshOverviewState().catch(() => {});
        await loadFolderWindow("", 0);
        if (generation !== sourceGeneration) return;
        if (fileLocationPublication)
          claimFileLocationNotice(
            "publication",
            `publication:${fileLocationPublication}`,
            "Scan results changed File Locations. Reopen the current Folder.",
            false,
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
      await loadWindow(currentIndex, generation);
      if (generation !== sourceGeneration) return;
      if (kind === "folder" && fileLocationNoticeKind === "publication")
        releaseFileLocationRecovery();
      recoveryGate.succeedTransition(sourceTransition);
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
    let boundPublication = fileLocationPublication;
    if (sourceKind === "folder" && !boundPublication) {
      // A Folder source must never be reopened publicationless; wait for
      // the root binding and fail truthfully if it cannot be established.
      boundPublication = (await awaitRootBinding())
        ? fileLocationPublication
        : undefined;
      if (generation !== sourceGeneration) return;
      if (!boundPublication) {
        // Fail truthfully instead of sending a publicationless request.
        gridStatus.textContent =
          "Could not load this source. Retry to continue.";
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
          overviewDataFloor += 1;
          resetFileLocations();
          await refreshOverviewState().catch(() => {});
          await loadFolderWindow("", 0);
          if (generation === sourceGeneration && fileLocationPublication)
            claimFileLocationNotice(
              "publication",
              `publication:${fileLocationPublication}`,
              "Scan results changed File Locations. Reopen the current Folder.",
              false,
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
      await loadWindow(currentIndex, generation);
      if (generation !== sourceGeneration) return;
      gridViewport.scrollTop =
        Math.floor(currentIndex / columns()) * GRID_CELL_HEIGHT;
      renderGrid();
      gridStatus.textContent =
        "Source reopened using the latest published Library order.";
      if (sourceKind === "folder" && fileLocationNoticeKind === "publication")
        releaseFileLocationRecovery();
      recoveryGate.succeedTransition(sourceTransition);
      setConnected(true);
      if (resumePhoto && photoGeneration === requestGeneration) {
        gridView.hidden = true;
        photoView.hidden = false;
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
  ): Promise<void> => {
    if (
      generation !== sourceGeneration ||
      !token ||
      browseTokenGeneration !== sourceGeneration ||
      total === 0
    )
      return Promise.resolve();
    const start = alignedStart(index);
    if (windowLoaded(start)) return Promise.resolve();
    const browseToken = token;
    const photoGeneration = requestGeneration;
    const scope =
      signal === sourceSignal
        ? sourceTasks
        : signal === photoSignal
          ? photoTasks
          : gridTasks;
    if (!quiet)
      gridStatus.textContent = `Loading Photos ${start + 1}–${Math.min(total, start + WINDOW_SIZE)} of ${total.toLocaleString()}…`;
    const shared = scope.joinOrStart(
      `window:${start}`,
      { abortTransport: true },
      async (requestSignal) => {
        const ownedSignal = requestSignal!;
        try {
          let response: Response;
          try {
            response = await fetcher(
              `/api/browse/${encodeURIComponent(browseToken)}?start=${start}&limit=${WINDOW_SIZE}`,
              { signal: ownedSignal, priority },
            );
          } catch {
            if (!ownedSignal.aborted && generation === sourceGeneration) {
              if (currentPhotoMode)
                status.textContent =
                  "Connection lost. Retry to refresh this range.";
              else
                gridStatus.textContent =
                  "Connection lost. Retry to refresh this range.";
              if (scope === photoTasks)
                failPhotoRecovery(photoGeneration, "browse-window");
              else failSourceRecovery(generation, "browse-window");
            }
            return;
          }
          if (response.status === 404) {
            if (!ownedSignal.aborted && generation === sourceGeneration)
              await reopenExpired(index, generation);
            return;
          }
          if (!response.ok) throw new Error("window failed");
          const result = (await response.json()) as BrowseWindowResponse;
          if (generation !== sourceGeneration || ownedSignal.aborted) return;
          for (const [offset, photo] of result.photos.entries())
            loaded.set(result.start + offset, photo);
          trimLoaded(index);
          renderGrid();
          if (!quiet)
            gridStatus.textContent = `Ready · ${total.toLocaleString()} Photos`;
        } catch {
          if (!ownedSignal.aborted && generation === sourceGeneration)
            gridStatus.textContent =
              "Some Photos could not load. Scroll or retry this range.";
        }
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
    caption.textContent = `${index + 1} · ${photo.rating ? `${photo.rating}★` : ""}`;
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
      { abortTransport: true },
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
    photoView.focus();
    resetTransform();
    try {
      if (!loaded.has(index))
        await loadWindow(index, sourceGeneration, true, signal, "high");
    } finally {
      // Back to Grid or a superseding view may end this request while an
      // unloaded boundary window is still loading. Release the open gate so
      // the interface can never remain wedged by an abandoned load.
      openingPhoto = false;
      updateControls();
    }
    if (generation !== requestGeneration) return;
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

  /// Populates the current-Photo Album membership controls.
  const renderMembershipControls = () => {
    if (!applicationAlive) return;
    albumSelect.replaceChildren();
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = sets.length ? "Choose Album…" : "No Albums yet";
    placeholder.disabled = true;
    placeholder.selected = true;
    albumSelect.append(placeholder);
    for (const set of sets) {
      const option = document.createElement("option");
      option.value = set.id;
      option.textContent = set.name;
      albumSelect.append(option);
    }
    const photo = currentPhoto();
    const adding = Boolean(
      photo &&
        sets.some((set) =>
          albumSettlements.isAdmitted(membershipKey("add", set.id, photo.id)),
        ),
    );
    const removing =
      Boolean(photo && sourceSetId) &&
      albumSettlements.isAdmitted(
        membershipKey("remove", sourceSetId!, photo!.id),
      );
    const inOpenAlbum =
      sourceKind === "album" &&
      Boolean(sourceSetId) &&
      Boolean(photo) &&
      !removedFromCurrentAlbum.has(photo!.id);
    albumSelect.disabled = !sets.length || adding;
    addToAlbum.disabled = !sets.length || !photo || adding;
    removeFromAlbum.hidden = !inOpenAlbum;
    removeFromAlbum.disabled = !inOpenAlbum || removing;
  };

  addToAlbum.addEventListener("click", () => {
    const photo = currentPhoto();
    const albumId = albumSelect.value;
    if (!photo || !albumId) return;
    const photoId = photo.id;
    const key = membershipKey("add", albumId, photoId);
    if (albumSettlements.isAdmitted(key)) return;
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
    if (albumSettlements.isAdmitted(key)) return;
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
      result = (await response.json()) as PreviewResponse;
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
    const task = progressQueue.then(async () => {
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
        sets = sets.map((set) =>
          set.id === albumId ? { ...set, hasSavedPosition: true } : set,
        );
        renderSources();
        return true;
      } catch {
        if (
          generation === sourceGeneration &&
          photoGeneration === requestGeneration &&
          sourceSetId === albumId &&
          currentPhoto()?.id === photoId
        ) {
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
      undo = { ...result.undo, advanced: advance && photoIndex < total - 1 };
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
    const affectedIndex = Array.from(loaded.entries()).find(
      ([, photo]) => photo.id === action.photoId,
    )?.[0];
    if (affectedIndex === undefined) {
      undo = undefined;
      status.textContent =
        "Undo is no longer available because that Photo left the loaded window.";
      updateControls();
      return;
    }
    const photo = loaded.get(affectedIndex)!;
    busy = true;
    undo = undefined;
    try {
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
        if (generation !== sourceGeneration) return;
        if (response.status === 409) {
          status.textContent =
            "Undo is no longer available because the Photo changed elsewhere. Retry to refresh its current state.";
          if (photoGeneration === requestGeneration)
            failPhotoRecovery(photoGeneration, "undo");
        } else {
          status.textContent =
            "Undo could not be saved. Try Undo again or Retry the connection.";
        }
        return;
      }
      if (
        generation !== sourceGeneration ||
        loaded.get(affectedIndex)?.id !== action.photoId
      )
        return;
      const updated = {
        ...photo,
        ...(action.field === "selectionState"
          ? { selectionState: action.priorValue as SelectionState }
          : { rating: action.priorValue as number }),
      };
      loaded.set(affectedIndex, updated);
      currentIndex = affectedIndex;
      currentPhotoMode = true;
      gridView.hidden = true;
      photoView.hidden = false;
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
    else if (event.key.toLowerCase() === "u")
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
      if (sourceKind === "folder" && !fileLocationPublication) {
        await awaitRootBinding();
        if (!fileLocationPublication) {
          gridStatus.textContent =
            "Could not load this source. Retry to continue.";
          return;
        }
      }
      await openSource(
        sourceKind,
        sourceKind === "album"
          ? sets.find((set) => set.id === sourceSetId)
          : undefined,
        undefined,
        sourceKind === "folder" ? sourceFolder : undefined,
      );
    })();
  });
  retry.addEventListener("click", () => void loadOverview());
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
        await loadWindow(start, sourceOwner, true, signal, "high");
        if (
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
  void loadOverview();
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
    applicationTasks.halt();
    fileLocationTasks.halt();
    sourceTasks.halt();
    gridTasks.halt();
    photoTasks.halt();
    recoveryGate.close();
    albumSettlements.closePresentation();
    scanSettlements.closePresentation();
    summaryNotices.close();
    window.removeEventListener("keydown", keydown);
    window.removeEventListener("resize", onResize);
    if (browseToken) void closeBrowse(browseToken);
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

if (typeof document !== "undefined") {
  const root = document.querySelector<HTMLElement>("#app");
  if (root) {
    const dispose = renderApp(root);
    window.addEventListener("pagehide", dispose, { once: true });
  }
}
