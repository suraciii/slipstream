import type {
  BrowseOpenResponse,
  BrowseWindowResponse,
  LibraryOverviewResponse,
  PhotoSetSummary,
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
        <aside class="source-panel" data-set-screen aria-label="Library sources">
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
          <fieldset class="rating-controls"><legend>Rating</legend><div data-ratings></div></fieldset>
          <div class="photo-controls"><button type="button" data-previous>Previous</button><button type="button" data-detail aria-pressed="false">Detail Review</button><button type="button" data-undo disabled>Undo</button><button type="button" data-next>Next</button></div>
        </section>
      </section>
    </main>`;

  const connection = required<HTMLElement>(root, "[data-connection]");
  const summaryStatus = required<HTMLElement>(root, "[data-summary-status]");
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
  const status = required<HTMLElement>(root, "[data-status]");
  const retryPhoto = required<HTMLButtonElement>(root, "[data-retry-photo]");
  const back = required<HTMLButtonElement>(root, "[data-back]");
  const previous = required<HTMLButtonElement>(root, "[data-previous]");
  const next = required<HTMLButtonElement>(root, "[data-next]");
  const select = required<HTMLButtonElement>(root, "[data-select]");
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
  let sets: ReadonlyArray<PhotoSetSummary> = [];
  let token = "";
  let total = 0;
  let currentIndex = 0;
  let sourceKind: "library" | "photo-set" = "library";
  let sourceSetId: string | undefined;
  let sourceSetName = "All Photos";
  let loaded = new Map<number, PhotoSummary>();
  let loadingRanges = new Set<number>();
  let thumbnailUrls = new Map<string, string>();
  let thumbnailRequests = new Map<string, Promise<string | undefined>>();
  let thumbnailFailures = new Set<string>();
  let lastCurrentPhotoId: string | undefined;
  let renderedColumns = 0;
  let connected = false;
  let busy = false;
  let openingPhoto = false;
  let currentPhotoMode = false;
  let undo: SessionUndo | undefined;
  let requestGeneration = 0;
  let sourceGeneration = 0;
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

  const renderSources = () => {
    sourceList.replaceChildren();
    const libraryButton = sourceButton(
      "All Photos",
      overview?.photoCount ?? 0,
      sourceKind === "library",
    );
    libraryButton.addEventListener("click", () => void openSource("library"));
    sourceList.append(libraryButton);
    for (const set of sets) {
      const button = sourceButton(
        set.name,
        set.photoCount,
        sourceKind === "photo-set" && sourceSetId === set.id,
        set.hasSavedPosition,
      );
      button.addEventListener("click", () => void openSource("photo-set", set));
      sourceList.append(button);
    }
  };
  const sourceButton = (
    name: string,
    count: number,
    active: boolean,
    saved = false,
  ) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `source-card${active ? " active" : ""}`;
    button.disabled = count === 0;
    button.innerHTML = "<strong></strong><span></span>";
    button.querySelector("strong")!.textContent = name;
    button.querySelector("span")!.textContent =
      `${count} Photos${saved ? " · Resume" : ""}`;
    return button;
  };

  const loadOverview = async () => {
    summaryStatus.textContent = "Loading Library summary…";
    try {
      const response = await fetcher("/api/overview");
      if (!response.ok) throw new Error("overview failed");
      overview = (await response.json()) as LibraryOverviewResponse;
      sets = overview.photoSets;
      summaryStatus.textContent =
        overview.scan.state === "idle"
          ? "Library ready"
          : `Library ${overview.scan.state}`;
      setConnected(true);
      renderSources();
      if (!token) await openSource("library");
    } catch {
      summaryStatus.textContent =
        "Could not reach Slipstream. Check the server and retry.";
      setConnected(false);
    }
  };

  const openSource = async (
    kind: "library" | "photo-set",
    set?: PhotoSetSummary,
    preferredPhotoId?: string,
  ) => {
    busy = true;
    sourceGeneration += 1;
    requestGeneration += 1;
    const generation = sourceGeneration;
    currentPhotoMode = false;
    gridView.hidden = false;
    photoView.hidden = true;
    sourceKind = kind;
    sourceSetId = set?.id;
    sourceSetName = set?.name ?? "All Photos";
    gridTitle.textContent = sourceSetName;
    gridStatus.textContent = "Preparing Library order…";
    undo = undefined;
    loaded = new Map();
    loadingRanges = new Set();
    thumbnailUrls = new Map();
    thumbnailRequests = new Map();
    thumbnailFailures = new Set();
    lastCurrentPhotoId = preferredPhotoId;
    const priorToken = token;
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
            : {
                source: "photo-set",
                photoSetId: set!.id,
                ...(preferredPhotoId ? { photoId: preferredPhotoId } : {}),
              },
        ),
      });
      if (!response.ok) throw new Error("browse open failed");
      const opened = (await response.json()) as BrowseOpenResponse;
      if (generation !== sourceGeneration) return;
      token = opened.token;
      if (priorToken && priorToken !== token) void closeBrowse(priorToken);
      total = opened.total;
      currentIndex = Math.min(opened.position, Math.max(0, total - 1));
      gridViewport.scrollTop =
        Math.floor(currentIndex / columns()) * GRID_CELL_HEIGHT;
      renderSources();
      await loadWindow(currentIndex, generation);
      renderGrid();
      gridStatus.textContent = total
        ? `Ready · ${total.toLocaleString()} Photos`
        : "No Photos in this source";
    } catch {
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
  const reopenExpired = async (anchorIndex: number) => {
    const oldToken = token;
    const anchorId =
      loaded.get(anchorIndex)?.id ?? lastCurrentPhotoId ?? currentPhoto()?.id;
    const generation = ++sourceGeneration;
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
            : {
                source: "photo-set",
                photoSetId: sourceSetId,
                ...(anchorId ? { photoId: anchorId } : {}),
              },
        ),
      });
      if (!response.ok) throw new Error("browse reopen failed");
      const opened = (await response.json()) as BrowseOpenResponse;
      if (generation !== sourceGeneration) return;
      token = opened.token;
      total = opened.total;
      currentIndex = Math.min(opened.position, Math.max(0, total - 1));
      loaded = new Map();
      loadingRanges = new Set();
      thumbnailUrls = new Map();
      thumbnailRequests = new Map();
      thumbnailFailures = new Set();
      if (oldToken && oldToken !== token) void closeBrowse(oldToken);
      await loadWindow(currentIndex, generation);
      gridViewport.scrollTop =
        Math.floor(currentIndex / columns()) * GRID_CELL_HEIGHT;
      renderGrid();
      gridStatus.textContent =
        "Source reopened using the latest published Library order.";
      setConnected(true);
    } catch {
      const failure =
        "This source expired and could not be reopened. Retry the connection.";
      gridStatus.textContent = failure;
      status.textContent = failure;
      setConnected(false);
    }
  };
  const windowLoaded = (start: number) => {
    const end = Math.min(total, start + WINDOW_SIZE);
    for (let index = start; index < end; index += 1)
      if (!loaded.has(index)) return false;
    return start < end;
  };
  const loadWindow = async (
    index: number,
    generation = sourceGeneration,
    quiet = false,
  ) => {
    if (!token || total === 0) return;
    const start = alignedStart(index);
    if (loadingRanges.has(start) || windowLoaded(start)) return;
    loadingRanges.add(start);
    if (!quiet)
      gridStatus.textContent = `Loading Photos ${start + 1}–${Math.min(total, start + WINDOW_SIZE)} of ${total.toLocaleString()}…`;
    try {
      let response: Response;
      try {
        response = await fetcher(
          `/api/browse/${encodeURIComponent(token)}?start=${start}&limit=${WINDOW_SIZE}`,
        );
      } catch {
        if (generation === sourceGeneration)
          setConnected(false, "Connection lost. Retry to refresh this range.");
        return;
      }
      if (response.status === 404) {
        await reopenExpired(index);
        return;
      }
      if (!response.ok) throw new Error("window failed");
      const result = (await response.json()) as BrowseWindowResponse;
      if (generation !== sourceGeneration) return;
      for (const [offset, photo] of result.photos.entries())
        loaded.set(result.start + offset, photo);
      trimLoaded(index);
      renderGrid();
      if (!quiet)
        gridStatus.textContent = `Ready · ${total.toLocaleString()} Photos`;
    } catch {
      if (generation === sourceGeneration)
        gridStatus.textContent =
          "Some Photos could not load. Scroll or retry this range.";
    } finally {
      loadingRanges.delete(start);
      updateControls();
    }
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
  const renderGrid = () => {
    const count = columns();
    renderedColumns = count;
    const rows = Math.ceil(total / count);
    gridCanvas.style.height = `${rows * GRID_CELL_HEIGHT}px`;
    const firstRow = Math.max(
      0,
      Math.floor(gridViewport.scrollTop / GRID_CELL_HEIGHT) - 2,
    );
    const visibleRows =
      Math.ceil(Math.max(360, gridViewport.clientHeight) / GRID_CELL_HEIGHT) +
      4;
    const start = firstRow * count;
    const end = Math.min(total, start + visibleRows * count);
    gridLayer.replaceChildren();
    gridLayer.style.height = `${rows * GRID_CELL_HEIGHT}px`;
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
    const image = document.createElement("img");
    image.alt = `Photo ${index + 1} of ${total}`;
    image.loading = "lazy";
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
    caption.textContent = `${index + 1} · ${photo.rating ? `${photo.rating}★` : ""}`;
    cell.append(caption);
    void loadThumbnail(photo.id, image);
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
  const loadThumbnail = (photoId: string, image: HTMLImageElement) => {
    const cached = thumbnailUrls.get(photoId);
    if (cached) {
      image.src = cached;
      return;
    }
    if (thumbnailFailures.has(photoId)) {
      image.alt = "Thumbnail unavailable";
      return;
    }
    const attach = (url?: string) => {
      if (url && image.isConnected) image.src = url;
    };
    const pending = thumbnailRequests.get(photoId);
    if (pending) {
      void pending.then(attach);
      return;
    }
    const request = (async () => {
      try {
        const response = await fetcher(`/api/photos/${photoId}/thumbnail`);
        if (!response.ok) return undefined;
        const result = (await response.json()) as PreviewResponse;
        return result.url ?? undefined;
      } catch {
        return undefined;
      }
    })();
    thumbnailRequests.set(photoId, request);
    void request.then((url) => {
      thumbnailRequests.delete(photoId);
      if (url) {
        rememberThumbnail(photoId, url);
        thumbnailFailures.delete(photoId);
      } else {
        thumbnailFailures.add(photoId);
        if (image.isConnected) image.alt = "Thumbnail unavailable";
      }
      attach(url);
    });
  };

  const openPhoto = async (index: number) => {
    if (busy || openingPhoto || index < 0 || index >= total) return;
    openingPhoto = true;
    currentIndex = index;
    const generation = ++requestGeneration;
    currentPhotoMode = true;
    gridView.hidden = true;
    photoView.hidden = false;
    photoView.focus();
    resetTransform();
    try {
      if (!loaded.has(index)) await loadWindow(index, sourceGeneration, true);
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
    renderPhotoShell();
    updateControls();
    await showPreview(generation);
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
  const renderPhotoShell = () => {
    const photo = currentPhoto();
    photoTitle.textContent = sourceSetName;
    renderPhotoFacts();
    source.textContent = "—";
    limited.hidden = true;
    stage.replaceChildren(
      paragraph(photo ? "Loading Preview…" : "Photo unavailable"),
    );
    status.textContent =
      photo && !photo.available
        ? "Original File is unavailable. Decisions remain available."
        : "";
    updateControls();
  };
  const showPreview = async (generation: number): Promise<boolean> => {
    const photo = currentPhoto();
    if (!photo) return false;
    let result: PreviewResponse;
    try {
      // Always contact the server, even for an unavailable Photo: a restored
      // Original must surface its Preview without a full reload, and Retry
      // must prove the server is reachable before reporting Connected.
      const response = await fetcher(`/api/photos/${photo.id}/preview`);
      result = (await response.json()) as PreviewResponse;
    } catch {
      if (generation === requestGeneration)
        setConnected(false, "Connection lost. Retry to refresh this Photo.");
      return false;
    }
    if (
      generation !== requestGeneration ||
      !currentPhoto() ||
      currentPhoto()!.id !== photo.id
    )
      return false;
    if (result.state !== "ready" || !result.url) {
      status.textContent = result.message ?? "Preview unavailable";
      stage.replaceChildren(paragraph("Preview unavailable"));
      return true;
    }
    const image = document.createElement("img");
    image.alt = `Photo ${currentIndex + 1} of ${total}`;
    image.draggable = false;
    image.src = result.url;
    image.addEventListener(
      "error",
      () => {
        status.textContent =
          "Preview could not be loaded. You can continue browsing.";
      },
      { once: true },
    );
    stage.replaceChildren(image);
    source.textContent = sourceLabel(result.source);
    limited.hidden = !result.limitedDetail;
    status.textContent = result.stale
      ? (result.message ?? "Showing a stale Preview.")
      : "";
    updateControls();
    void prefetchAdjacent(currentIndex - 1, generation);
    void prefetchAdjacent(currentIndex + 1, generation);
    return true;
  };
  const prefetchAdjacent = async (index: number, generation: number) => {
    if (generation !== requestGeneration) return;
    if (index < 0 || index >= total) return;
    let photo = loaded.get(index);
    if (!photo) {
      await loadWindow(index, sourceGeneration, true);
      if (generation !== requestGeneration) return;
      photo = loaded.get(index);
    }
    if (!photo || !photo.available) return;
    try {
      await fetcher(`/api/photos/${photo.id}/preview?priority=adjacent`);
    } catch {
      /* adjacent work is best effort */
    }
  };
  const showGrid = () => {
    currentPhotoMode = false;
    requestGeneration += 1;
    photoView.hidden = true;
    gridView.hidden = false;
    renderGrid();
    requestAnimationFrame(() => {
      gridViewport.scrollTop =
        Math.floor(currentIndex / columns()) * GRID_CELL_HEIGHT;
      renderGrid();
    });
    updateControls();
  };
  const persistPosition = (): Promise<boolean> => {
    if (sourceKind !== "photo-set" || !sourceSetId || !currentPhoto())
      return Promise.resolve(true);
    const photoId = currentPhoto()!.id;
    const task = progressQueue.then(async () => {
      try {
        const response = await fetcher(
          `/api/photo-sets/${sourceSetId}/progress`,
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ photoId }),
          },
        );
        if (!response.ok) throw new Error("position rejected");
        sets = sets.map((set) =>
          set.id === sourceSetId ? { ...set, hasSavedPosition: true } : set,
        );
        renderSources();
        return true;
      } catch {
        setConnected(
          false,
          "Photo Set position could not be saved. Retry before making more decisions.",
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
          ...(sourceSetId ? { photoSetId: sourceSetId } : {}),
        }),
      });
      if (!response.ok) {
        undo = prior;
        status.textContent =
          response.status === 409
            ? "The Photo changed elsewhere. Retry to refresh its current state."
            : "The change could not be saved.";
        if (response.status === 409) setConnected(false);
        return;
      }
      const result = (await response.json()) as MutationResponse;
      const updated = {
        ...photo,
        ...(field === "selectionState"
          ? { selectionState: value as SelectionState }
          : { rating: value as number }),
      };
      loaded.set(currentIndex, updated);
      undo = { ...result.undo, advanced: advance && currentIndex < total - 1 };
      status.textContent = `${field === "rating" ? "Rating" : "Selection"} saved.`;
      if (undo.advanced) {
        busy = false;
        await moveTo(currentIndex + 1);
      } else renderPhotoFacts();
    } catch {
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
          ...(sourceSetId ? { photoSetId: sourceSetId } : {}),
        }),
      });
      if (!response.ok) {
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
      renderPhotoShell();
      busy = false;
      updateControls();
      await showPreview(requestGeneration);
      await persistPosition();
      status.textContent = "Last change undone.";
    } catch {
      setConnected(false, "Connection lost before Undo was confirmed.");
    } finally {
      busy = false;
      updateControls();
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

  gridViewport.addEventListener("scroll", () => {
    renderGrid();
    const count = columns();
    void loadWindow(
      Math.floor(gridViewport.scrollTop / GRID_CELL_HEIGHT) * count,
    );
  });
  const onResize = () => {
    requestAnimationFrame(() => {
      if (gridView.hidden || columns() === renderedColumns) return;
      gridViewport.scrollTop =
        Math.floor(currentIndex / columns()) * GRID_CELL_HEIGHT;
      renderGrid();
    });
  };
  window.addEventListener("resize", onResize);
  back.addEventListener("click", showGrid);
  refresh.addEventListener(
    "click",
    () =>
      void openSource(
        sourceKind,
        sourceKind === "photo-set"
          ? sets.find((set) => set.id === sourceSetId)
          : undefined,
      ),
  );
  retry.addEventListener("click", () => void loadOverview());
  retryPhoto.addEventListener("click", () => {
    void (async () => {
      busy = true;
      status.textContent = "Reconnecting…";
      updateControls();
      const start = alignedStart(currentIndex);
      const end = Math.min(total, start + WINDOW_SIZE);
      for (let index = start; index < end; index += 1) loaded.delete(index);
      await loadWindow(start, sourceGeneration, true);
      const refreshed = await showPreview(requestGeneration);
      const persisted = refreshed && (await persistPosition());
      if (refreshed && persisted) {
        setConnected(true);
        status.textContent = "Connected. Current state refreshed.";
      }
      busy = false;
      updateControls();
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
