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
  const summaryStatus = {
    get textContent() {
      return summaryStatusElement.textContent;
    },
    set textContent(value: string) {
      summaryStatusElement.textContent = value;
    },
  };
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
  // Photo-status writes are epoch-sequenced so a late Album settlement can
  // never overwrite a newer Photo status from any writer.
  let photoStatusEpoch = 0;
  const status = {
    get textContent() {
      return statusElement.textContent;
    },
    set textContent(value: string) {
      photoStatusEpoch += 1;
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

  let overview: LibraryOverviewResponse | undefined;
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

  // Album mutations report on the surface that initiated them. Every write to
  // a status surface bumps that surface's epoch, so a late Album settlement
  // can never overwrite a newer status from any writer — Album action, Photo
  // decision, or scan label alike.
  let albumActionSequence = 0;
  let summaryNoticeAction = 0;
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
  let windowRequests = new Map<
    number,
    { promise: Promise<void>; signal: AbortSignal }
  >();
  let thumbnailUrls = new Map<string, string>();
  let thumbnailRequests = new Map<string, Promise<string | undefined>>();
  let thumbnailFailures = new Set<string>();
  let renderedThumbnailImages = new Map<string, HTMLImageElement>();
  let lastCurrentPhotoId: string | undefined;
  let renderedColumns = 0;
  let renderedViewportHeight = 0;
  let connected = false;
  let busy = false;
  let openingPhoto = false;
  let currentPhotoMode = false;
  let undo: SessionUndo | undefined;
  let requestGeneration = 0;
  let sourceGeneration = 0;
  let sourceAbortController = new AbortController();
  let gridAbortController = new AbortController();
  let photoAbortController = new AbortController();
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
  const setConnected = (value: boolean, message?: string) => {
    if (!value) clearPointer();
    connected = value;
    connection.textContent = value ? "Connected" : "Disconnected";
    connection.classList.toggle("offline", !value);
    retry.hidden = value || currentPhotoMode;
    retryPhoto.hidden = value || !currentPhotoMode;
    if (message) status.textContent = message;
    updateControls();
  };
  const updateControls = () => {
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
  let folderFailure: { parent: string; page: number } | undefined;
  // Only one unbound root binder may own the bind at a time, identified by
  // its globally unique request token. A reset invalidates the owner so a
  // newer recovery can take over instead of deadlocking against a discarded
  // in-flight bind.
  let rootBindOwner = 0;
  // Per-parent request tokens drawn from one globally monotonic counter:
  // only the newest request for one parent may commit its window, delayed
  // duplicates cannot regress the current page, and tokens are never reused,
  // so collapse/eviction cleanup can never let a stale in-flight request
  // impersonate a newer one.
  const folderRequestSequence = new Map<string, number>();
  let folderRequestCounter = 0;

  const resetFileLocations = () => {
    fileLocationGeneration += 1;
    fileLocationPublication = undefined;
    folderWindows.clear();
    expandedFolders.clear();
    folderFailure = undefined;
    folderRequestSequence.clear();
    rootBindOwner = 0;
    releaseRootBindingWaiters();
    // Re-render at once: retained Folder cards must not stay clickable with
    // an unbound publication, and a cleared failure must remove its Retry
    // control before any handler can dereference it.
    renderSources();
  };

  /// Awaits a bound File Location publication. Folder-source requests must
  /// never be sent publicationless: retry and reconnect paths wait for (or
  /// re-establish) the root binding first and report failure truthfully.
  let rootBindingWaiters: Array<() => void> = [];
  const releaseRootBindingWaiters = () => {
    const waiters = rootBindingWaiters;
    rootBindingWaiters = [];
    for (const waiter of waiters) waiter();
  };
  const awaitRootBinding = async () => {
    if (fileLocationPublication) return true;
    if (rootBindOwner !== 0) {
      // A root bind is already in flight: await its settlement instead of
      // reporting no publication while one is on the way.
      await new Promise<void>((resolve) => {
        rootBindingWaiters.push(resolve);
      });
      return fileLocationPublication !== undefined;
    }
    await loadFolderWindow("", 0, false);
    return fileLocationPublication !== undefined;
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
  ): Promise<{ ok: boolean; announce: (text: string) => void }> => {
    const action = ++albumActionSequence;
    const photoEpoch = photoStatusEpoch;
    const stillNewest = () => action === albumActionSequence;
    // The photo surface only accepts the newest action's notice, for the
    // Photo generation that initiated it, while Photo View is open and no
    // newer status of any kind has been written since initiation.
    const ownsPhotoSurface = () =>
      stillNewest() &&
      surface === "photo" &&
      photoGeneration === requestGeneration &&
      currentPhotoMode &&
      photoEpoch === photoStatusEpoch;
    const reportToSummary = (text: string) => {
      // A failure that no longer owns the Photo surface still matters:
      // surface it in the Library summary unless a newer Album notice
      // already owns that channel.
      if (action > summaryNoticeAction) {
        summaryNoticeAction = action;
        summaryStatus.textContent = text;
      }
    };
    const report = (text: string) => {
      if (ownsPhotoSurface()) status.textContent = text;
      else reportToSummary(text);
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
          // Only transport-class failures change global connectivity: a
          // duplicate name (409) or unknown Album (404) is a normal,
          // answered request, not a disconnection — and a superseded
          // action's failure must not disconnect a newer success.
          if (response.status >= 500 && stillNewest()) setConnected(false);
          return { ok: false, announce: () => {} };
        }
        // A stale settlement refreshes the bounded data but leaves the
        // current connection state to whichever action owns the UI now.
        try {
          await refreshOverviewState({
            connectivity: stillNewest(),
            action,
          });
        } catch {
          // The mutation itself persisted; the summary refresh failing must
          // not silently hide that success. A superseded action must not
          // disconnect the UI a newer action already restored.
          if (!stillNewest()) return { ok: true, announce: () => {} };
          setConnected(false);
          if (action > summaryNoticeAction) {
            summaryNoticeAction = action;
            summaryStatus.textContent =
              "The Album was saved but the Library summary could not be refreshed.";
          }
        }
        return {
          ok: true,
          announce: (text: string) => {
            if (ownsPhotoSurface()) status.textContent = text;
          },
        };
      } catch {
        // A transport failure of the newest action is a real connectivity
        // loss; a superseded action's failure still surfaces as a notice
        // but must not disconnect UI a newer action already restored.
        report(failure(0));
        if (stillNewest()) setConnected(false);
        return { ok: false, announce: () => {} };
      }
    })();
  };

  const describeRange = (parent: string, page: number) =>
    `${parent || "Library Folder"} items ${(page * FOLDER_PAGE_SIZE + 1).toLocaleString()}–${((page + 1) * FOLDER_PAGE_SIZE).toLocaleString()}`;

  const loadFolderWindow = async (
    parent: string,
    page: number,
    expand = true,
  ) => {
    const unboundRoot = parent === "" && !fileLocationPublication;
    if (unboundRoot && rootBindOwner !== 0) return;
    const generation = fileLocationGeneration;
    const sequence = (folderRequestCounter += 1);
    folderRequestSequence.set(parent, sequence);
    if (unboundRoot) rootBindOwner = sequence;
    const parameters = new URLSearchParams({
      start: String(page * FOLDER_PAGE_SIZE),
      limit: String(FOLDER_PAGE_SIZE),
    });
    if (parent) parameters.set("parent", parent);
    const boundPublication = fileLocationPublication;
    if (boundPublication) parameters.set("publication", boundPublication);
    try {
      const response = await fetcher(`/api/file-locations?${parameters}`);
      // A response from a superseded navigation generation or an outdated
      // same-parent request is discarded, whatever its outcome.
      if (generation !== fileLocationGeneration) return;
      if (folderRequestSequence.get(parent) !== sequence) return;
      if (response.status === 409) {
        resetFileLocations();
        await refreshOverviewState().catch(() => {});
        await loadFolderWindow("", 0);
        // Only claim a coherent reload when the new root actually bound; a
        // failed rebind keeps its own failure message and retry control.
        if (fileLocationPublication) {
          // Deliberate retake: this recovery message outranks a standing
          // Album notice because it asks the user to act now.
          summaryNoticeAction = 0;
          summaryStatus.textContent =
            "Scan results changed File Locations. Reloaded the current Folders.";
        }
        return;
      }
      if (!response.ok) throw new Error("file locations failed");
      const window = (await response.json()) as FileLocationsResponse;
      if (generation !== fileLocationGeneration) return;
      if (boundPublication && window.publication !== boundPublication) {
        // A delayed response bound to a different publication: discard it,
        // for the root exactly as for descendant windows.
        return;
      }
      if (folderRequestSequence.get(parent) !== sequence) {
        // A superseded request for this parent already committed a newer
        // window; a delayed duplicate must not regress the current page.
        return;
      }
      fileLocationPublication = window.publication;
      folderWindows.set(parent, {
        page,
        children: [...window.children],
        total: window.total,
      });
      // Startup binding publishes the window without visually expanding it;
      // user navigation expands retained state instead of refetching.
      if (expand) {
        expandedFolders.add(parent);
        enforceExpandedCap(parent);
      }
      if (folderFailure) {
        if (folderFailure.parent === parent && folderFailure.page === page) {
          // Only the failed range's own success clears its retry state; an
          // unrelated range loading must not hide a still-failed range.
          folderFailure = undefined;
          // Follow the Library summary's notice rule: a standing Album
          // notice is not erased by this background status write.
          if (summaryNoticeAction === 0)
            summaryStatus.textContent = overview
              ? scanLabel(overview.scan)
              : "Library ready";
        }
        // Any successful load restores the connection a failed range marked
        // offline; the global connection banner must not stay stuck.
        setConnected(true);
      }
      renderSources();
    } catch {
      if (generation !== fileLocationGeneration) return;
      // A superseded request's delayed failure must not overwrite the
      // failure state or connection of a newer committed request.
      if (folderRequestSequence.get(parent) !== sequence) return;
      // No window committed for this parent, so its sequencing metadata is
      // dropped: repeated failed exploration cannot grow retained state.
      folderRequestSequence.delete(parent);
      folderFailure = { parent, page };
      // Deliberate retake: the failed range's retry control must surface.
      summaryNoticeAction = 0;
      summaryStatus.textContent = `Could not load File Locations (${describeRange(parent, page)}). Retry to continue.`;
      setConnected(false);
      renderSources();
    } finally {
      // Clear ownership only when this request still owns it: a reset that
      // invalidated the bind must not let a discarded request clear a newer
      // owner's flag.
      if (unboundRoot && rootBindOwner === sequence) {
        rootBindOwner = 0;
        releaseRootBindingWaiters();
      }
    }
  };

  const enforceExpandedCap = (newest: string) => {
    while (expandedFolders.size > MAXIMUM_EXPANDED_FOLDERS) {
      const oldest = [...expandedFolders].find(
        (location) => location !== newest && location !== "",
      );
      if (oldest === undefined) break;
      expandedFolders.delete(oldest);
      folderWindows.delete(oldest);
      folderRequestSequence.delete(oldest);
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
        folderRequestSequence.delete(child.location);
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
        folderRequestSequence.delete("");
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
  let statusPoll = 0;
  const pollUntilPublished = async () => {
    const generation = ++statusPoll;
    while (generation === statusPoll && overview && !overview.published) {
      await new Promise((resolve) => setTimeout(resolve, 1_000));
      if (generation !== statusPoll) return;
      try {
        const response = await fetcher("/api/status");
        if (!response.ok) continue;
        const scan = (await response.json()) as LibraryOverviewResponse["scan"];
        // Publication polling is background status: it must not erase a
        // standing Album notice from the Library summary.
        if (summaryNoticeAction === 0)
          summaryStatus.textContent = scanLabel(scan);
        if (scan.state === "idle") await loadOverview();
      } catch {
        /* keep polling; the connection status reflects hard failures */
      }
    }
  };
  let overviewRequestSequence = 0;
  let overviewCommitted = 0;
  const refreshOverviewState = async (options?: {
    connectivity?: boolean;
    action?: number;
  }) => {
    // Overview responses commit shared state in commit order: a response may
    // land only if it is newer than the last COMMITTED overview, so a late
    // older response cannot revert Albums, counts, titles, or retry state,
    // while a newer request that FAILS never discards an older success.
    const request = ++overviewRequestSequence;
    const response = await fetcher("/api/overview");
    if (!response.ok) throw new Error("overview failed");
    const body = (await response.json()) as LibraryOverviewResponse;
    // Re-check after the body resolves: a slow parse must not let an
    // obsolete overview revert newer committed state either.
    if (request <= overviewCommitted) return;
    overviewCommitted = request;
    overview = body;
    sets = overview.albums;
    // The Library summary keeps a standing Album notice until a newer
    // Album action settles or the user deliberately reloads: background
    // refreshes (folder recovery, scan polling) must not erase it.
    const owner = options?.action;
    const mayTakeSummary =
      owner !== undefined
        ? owner >= summaryNoticeAction
        : summaryNoticeAction === 0;
    if (mayTakeSummary) {
      summaryStatus.textContent = scanLabel(overview.scan);
      // A successfully settled action supersedes any prior notice: clear
      // the marker so background refreshes may take the summary again.
      // (A failing action keeps its marker until a newer action settles.)
      if (owner !== undefined) summaryNoticeAction = 0;
    }
    if (options?.connectivity !== false) setConnected(true);
    // A refreshed Album list must not leave stale presentation behind: the
    // open Album keeps its (possibly renamed) name on every heading, the
    // remembered retry source follows the rename, and the current-Photo
    // membership controls follow the refreshed Album list.
    if (sourceKind === "album" && sourceSetId) {
      const open = sets.find((candidate) => candidate.id === sourceSetId);
      if (open) {
        sourceSetName = open.name;
        gridTitle.textContent = sourceSetName;
        photoTitle.textContent = sourceSetName;
        if (lastSource?.kind === "album" && lastSource.set?.id === open.id) {
          lastSource = { ...lastSource, set: open };
        }
      }
    }
    renderMembershipControls();
    renderSources();
  };

  let overviewLoadSequence = 0;
  const loadOverview = async () => {
    // A deliberate reload or retry retakes the Library summary channel.
    // Sequence loads so an older failed load cannot mark a newer successful
    // load disconnected: only the newest load owns the failure surface.
    const load = ++overviewLoadSequence;
    summaryNoticeAction = 0;
    summaryStatus.textContent = "Loading Library summary…";
    try {
      await refreshOverviewState();
      const current = overview;
      if (!current) throw new Error("overview missing");
      if (!fileLocationPublication && current.published) {
        // A remembered Folder source must reopen only after the publication
        // is bound; otherwise the request would be rejected as invalid.
        if (lastSource?.kind === "folder" && !token) {
          await awaitRootBinding();
        } else {
          void loadFolderWindow("", 0, false);
        }
      }
      if (!token && current.published) {
        const source = lastSource ?? {
          kind: "library" as const,
          set: undefined,
          folder: undefined,
        };
        const bindable =
          source.kind !== "folder" || fileLocationPublication !== undefined;
        if (bindable) {
          await openSource(source.kind, source.set, undefined, source.folder);
        } else {
          gridStatus.textContent =
            "Could not load this source. Retry to continue.";
        }
      } else if (!current.published) void pollUntilPublished();
    } catch {
      if (load !== overviewLoadSequence) return;
      summaryStatus.textContent =
        "Could not reach Slipstream. Check the server and retry.";
      setConnected(false);
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
    const sourceDrawerWasOpen = browser.classList.contains("sources-open");
    busy = true;
    cancelScheduledGridRender();
    sourceGeneration += 1;
    requestGeneration += 1;
    const generation = sourceGeneration;
    openingPhoto = false;
    cancelPendingImageLoads(gridLayer, true);
    gridLayer.replaceChildren();
    cancelPendingImageLoads(stage, true);
    stage.replaceChildren();
    sourceAbortController.abort();
    sourceAbortController = new AbortController();
    gridAbortController.abort();
    gridAbortController = new AbortController();
    photoAbortController.abort();
    photoAbortController = new AbortController();
    const signal = sourceAbortController.signal;
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
    windowRequests = new Map();
    thumbnailUrls = new Map();
    thumbnailRequests = new Map();
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
        resetFileLocations();
        await refreshOverviewState().catch(() => {});
        await loadFolderWindow("", 0);
        if (generation !== sourceGeneration) return;
        if (fileLocationPublication) {
          // Deliberate retake: actionable recovery outranks a standing notice.
          summaryNoticeAction = 0;
          summaryStatus.textContent =
            "Scan results changed File Locations. Reopen the current Folder.";
        }
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
      renderGrid();
      gridStatus.textContent = total
        ? `Ready · ${total.toLocaleString()} Photos`
        : "No Photos in this source";
    } catch {
      if (generation !== sourceGeneration) return;
      token = "";
      if (priorToken) void closeBrowse(priorToken);
      gridStatus.textContent = "Could not load this source. Retry to continue.";
      setConnected(false);
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
    photoAbortController.abort();
    photoAbortController = new AbortController();
    const photoSignal = photoAbortController.signal;
    cancelPendingImageLoads(stage);
    openingPhoto = false;
    const oldToken = token;
    const anchorId =
      loaded.get(anchorIndex)?.id ?? lastCurrentPhotoId ?? currentPhoto()?.id;
    sourceAbortController.abort();
    sourceAbortController = new AbortController();
    gridAbortController.abort();
    gridAbortController = new AbortController();
    cancelScheduledGridRender();
    const generation = ++sourceGeneration;
    cancelPendingImageLoads(gridLayer);
    const signal = sourceAbortController.signal;
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
        setConnected(false);
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
          resetFileLocations();
          await refreshOverviewState().catch(() => {});
          await loadFolderWindow("", 0);
          if (generation === sourceGeneration && fileLocationPublication) {
            // Deliberate retake: actionable recovery outranks a standing notice.
            summaryNoticeAction = 0;
            summaryStatus.textContent =
              "Scan results changed File Locations. Reopen the current Folder.";
          }
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
      windowRequests = new Map();
      thumbnailUrls = new Map();
      thumbnailRequests = new Map();
      thumbnailFailures = new Set();
      if (oldToken && oldToken !== token) void closeBrowse(oldToken);
      await loadWindow(currentIndex, generation);
      if (generation !== sourceGeneration) return;
      gridViewport.scrollTop =
        Math.floor(currentIndex / columns()) * GRID_CELL_HEIGHT;
      renderGrid();
      gridStatus.textContent =
        "Source reopened using the latest published Library order.";
      setConnected(true);
      if (resumePhoto && photoGeneration === requestGeneration) {
        gridView.hidden = true;
        photoView.hidden = false;
        syncSourcePanel();
        renderPhotoShell(photoGeneration);
        void showPreview(photoGeneration, photoSignal).then((refreshed) => {
          if (refreshed && photoGeneration === requestGeneration)
            void persistPosition();
        });
      }
    } catch {
      if (generation !== sourceGeneration) return;
      browseTokenGeneration = generation;
      const failure =
        "This source expired and could not be reopened. Retry the connection.";
      gridStatus.textContent = failure;
      status.textContent = failure;
      setConnected(false);
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
    signal = sourceAbortController.signal,
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
    const existing = windowRequests.get(start);
    if (existing && !existing.signal.aborted) return existing.promise;
    if (existing) windowRequests.delete(start);
    if (windowLoaded(start)) return Promise.resolve();
    const browseToken = token;
    if (!quiet)
      gridStatus.textContent = `Loading Photos ${start + 1}–${Math.min(total, start + WINDOW_SIZE)} of ${total.toLocaleString()}…`;
    const request = (async () => {
      try {
        let response: Response;
        try {
          response = await fetcher(
            `/api/browse/${encodeURIComponent(browseToken)}?start=${start}&limit=${WINDOW_SIZE}`,
            { signal, priority },
          );
        } catch {
          if (!signal.aborted && generation === sourceGeneration)
            setConnected(
              false,
              "Connection lost. Retry to refresh this range.",
            );
          return;
        }
        if (response.status === 404) {
          if (!signal.aborted && generation === sourceGeneration)
            await reopenExpired(index, generation);
          return;
        }
        if (!response.ok) throw new Error("window failed");
        const result = (await response.json()) as BrowseWindowResponse;
        if (generation !== sourceGeneration || signal.aborted) return;
        for (const [offset, photo] of result.photos.entries())
          loaded.set(result.start + offset, photo);
        trimLoaded(index);
        renderGrid();
        if (!quiet)
          gridStatus.textContent = `Ready · ${total.toLocaleString()} Photos`;
      } catch {
        if (!signal.aborted && generation === sourceGeneration)
          gridStatus.textContent =
            "Some Photos could not load. Scroll or retry this range.";
      }
    })();
    const entry = {
      signal,
      promise: request.finally(() => {
        if (windowRequests.get(start) === entry) windowRequests.delete(start);
        if (
          signal.aborted &&
          generation === sourceGeneration &&
          !gridView.hidden
        )
          scheduleGridRender();
        updateControls();
      }),
    };
    windowRequests.set(start, entry);
    return entry.promise;
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
    image.onerror = () => markThumbnailUnavailable(photoId, image, generation);
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
    const pending = thumbnailRequests.get(photoId);
    if (pending) {
      void pending.then((url) =>
        attachThumbnail(photoId, image, url, false, generation),
      );
      return;
    }
    const signal = gridAbortController.signal;
    const request = (async () => {
      try {
        const response = await fetcher(`/api/photos/${photoId}/thumbnail`, {
          signal,
          priority: "low",
        });
        if (!response.ok) return undefined;
        const result = (await response.json()) as PreviewResponse;
        return result.url ?? undefined;
      } catch {
        return undefined;
      }
    })();
    thumbnailRequests.set(photoId, request);
    void request.then((url) => {
      if (
        generation !== sourceGeneration ||
        thumbnailRequests.get(photoId) !== request
      )
        return;
      thumbnailRequests.delete(photoId);
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
    photoAbortController.abort();
    photoAbortController = new AbortController();
    const signal = photoAbortController.signal;
    cancelPendingImageLoads(stage, true);
    renderedThumbnailImages = new Map();
    gridAbortController.abort();
    gridAbortController = new AbortController();
    thumbnailRequests = new Map();
    cancelPendingImageLoads(gridLayer);
    currentPhotoMode = true;
    gridView.hidden = true;
    photoView.hidden = false;
    syncSourcePanel();
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
    const previewRequest = showPreview(generation, signal);
    if (!hasKnownPreview) await previewRequest;
    // A superseded open must not persist or touch controls afterwards: the
    // newer navigation persists its own position.
    if (generation !== requestGeneration) return;
    await persistPosition();
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
    const image = document.createElement("img");
    image.alt = `Photo ${currentIndex + 1} of ${total}`;
    image.draggable = false;
    image.fetchPriority = "high";
    image.decoding = "async";
    image.src = url;
    image.addEventListener(
      "error",
      () => {
        if (generation !== requestGeneration || !image.isConnected) return;
        status.textContent =
          "Preview could not be loaded. You can continue browsing.";
      },
      { once: true },
    );
    stage.replaceChildren(image);
  };
  // Membership state that must survive re-renders: in-flight operations are
  // keyed by action, Album, and Photo (an Add to one Album never blocks a
  // Remove from another), and Photos already removed from the open Album
  // stay non-removable in the retained open snapshot.
  const membershipInFlight = new Set<string>();
  const membershipKey = (
    verb: "add" | "remove",
    albumId: string,
    photoId: string,
  ) => `${verb}:${albumId}:${photoId}`;
  const removedFromCurrentAlbum = new Set<string>();

  /// Populates the current-Photo Album membership controls.
  const renderMembershipControls = () => {
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
          membershipInFlight.has(membershipKey("add", set.id, photo.id)),
        ),
    );
    const removing =
      Boolean(photo && sourceSetId) &&
      membershipInFlight.has(membershipKey("remove", sourceSetId!, photo!.id));
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
    if (membershipInFlight.has(key)) return;
    const generation = requestGeneration;
    membershipInFlight.add(key);
    void (async () => {
      addToAlbum.disabled = true;
      const { ok: added, announce } = await mutateAlbum(
        `/api/albums/${albumId}/members`,
        { photoIds: [photoId] },
        () => "The Photo could not be added to the Album.",
        "photo",
        generation,
      );
      membershipInFlight.delete(key);
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
    if (membershipInFlight.has(key)) return;
    const generation = requestGeneration;
    const snapshotGeneration = sourceGeneration;
    membershipInFlight.add(key);
    void (async () => {
      removeFromAlbum.disabled = true;
      const { ok: removed, announce } = await mutateAlbum(
        `/api/albums/${albumId}/members/remove`,
        { photoId },
        () => "The Photo could not be removed from the Album.",
        "photo",
        generation,
      );
      membershipInFlight.delete(key);
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
  ): Promise<boolean> => {
    if (generation !== requestGeneration || signal.aborted) return false;
    const photo = currentPhoto();
    if (!photo) return false;
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
      setConnected(false, "Connection lost. Retry to refresh this Photo.");
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
    photoAbortController.abort();
    photoAbortController = new AbortController();
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
        if (generation === sourceGeneration && sourceSetId === albumId)
          setConnected(
            false,
            "Album position could not be saved. Retry before making more decisions.",
          );
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
        if (response.status === 409) setConnected(false);
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
      if (generation !== sourceGeneration) return;
      undo = undefined;
      setConnected(
        false,
        "Connection lost before the change was confirmed. Retry to refresh.",
      );
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
          setConnected(
            false,
            "Undo is no longer available because the Photo changed elsewhere. Retry to refresh its current state.",
          );
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
      syncSourcePanel();
      photoView.focus();
      resetTransform();
      const previewGeneration = ++requestGeneration;
      photoAbortController.abort();
      photoAbortController = new AbortController();
      const signal = photoAbortController.signal;
      cancelPendingImageLoads(stage, true);
      renderPhotoShell(previewGeneration);
      busy = false;
      updateControls();
      await showPreview(previewGeneration, signal);
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
      )
        status.textContent = "Last change undone.";
    } catch {
      if (generation === sourceGeneration)
        setConnected(false, "Connection lost before Undo was confirmed.");
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
      photoAbortController.abort();
      photoAbortController = new AbortController();
      const signal = photoAbortController.signal;
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
        const refreshed = await showPreview(generation, signal);
        const persisted = refreshed && (await persistPosition());
        if (
          refreshed &&
          persisted &&
          sourceOwner === sourceGeneration &&
          generation === requestGeneration
        ) {
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
    window.removeEventListener("keydown", keydown);
    window.removeEventListener("resize", onResize);
    compactSources.removeEventListener("change", onSourceViewportChange);
    if (token) void closeBrowse(token);
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
  if (root) renderApp(root);
}
