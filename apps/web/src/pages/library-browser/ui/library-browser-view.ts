import "./library-browser.css";

export type ViewSelectionState = "undecided" | "selected" | "rejected";
export type ViewPreviewSource = "matching-jpeg" | "embedded-raw-jpeg";

const GRID_CELL_HEIGHT = 178;
const GRID_CELL_WIDTH = 150;
const SWIPE_PENDING_PIXELS = 24;
const SWIPE_COMMIT_PIXELS = 72;
const SWIPE_COMMIT_VELOCITY = 0.5;

export type SourceReference =
  | Readonly<{ kind: "library" }>
  | Readonly<{ kind: "album"; id: string }>
  | Readonly<{ kind: "folder"; location: string; name: string }>;

export type AlbumFormReference = Readonly<{
  formId: string;
  kind: "create" | "rename" | "delete";
  albumId?: string;
  name: string;
}>;

export type LibraryBrowserIntent =
  | Readonly<{ kind: "summary-action"; presentationId: number }>
  | Readonly<{ kind: "source-open"; source: SourceReference }>
  | Readonly<{ kind: "file-location-retry"; key: string }>
  | Readonly<{ kind: "folder-toggle"; location: string; expanded: boolean }>
  | Readonly<{ kind: "folder-page"; location: string; direction: -1 | 1 }>
  | Readonly<{ kind: "album-form-open"; form: AlbumFormReference }>
  | Readonly<{ kind: "album-form-close"; formId: string }>
  | Readonly<{
      kind: "album-form-submit";
      formId: string;
      name?: string;
    }>
  | Readonly<{ kind: "grid-render" | "grid-resize" }>
  | Readonly<{ kind: "grid-window"; index: number }>
  | Readonly<{ kind: "open-photo"; index: number }>
  | Readonly<{
      kind:
        | "show-grid"
        | "refresh"
        | "retry-source"
        | "retry-photo"
        | "previous"
        | "next"
        | "undo";
    }>
  | Readonly<{
      kind: "photo-mutation";
      field: "selectionState" | "rating";
      value: ViewSelectionState | number;
      advance: boolean;
    }>
  | Readonly<{ kind: "membership-add"; albumId: string }>
  | Readonly<{ kind: "membership-remove" }>;

export type GridThumbnailBinding = Readonly<{
  photoId: string;
  preview: GridPhotoViewModel["preview"];
  target: GridThumbnailTarget;
}>;

export interface GridThumbnailTarget {
  alt: string;
  complete: boolean;
  isConnected: boolean;
  src: string;
  onload: GlobalEventHandlers["onload"];
  onerror: GlobalEventHandlers["onerror"];
  removeAttribute(name: string): void;
}

export interface ReviewImageTarget {
  readonly connected: boolean;
  readonly source: string;
  setHandlers(onLoad: () => void, onError: () => void): void;
  clearHandlers(): void;
  setSource(resolvedUrl: string): void;
  clearSource(): void;
}

export type ReviewImagePresentation = Readonly<{
  target: ReviewImageTarget;
  resolvedUrl: string;
  surface: object;
}>;

export type FolderViewModel = Readonly<{
  location: string;
  name: string;
  photoCount: number;
  hasDescendantFolders: boolean;
  expanded: boolean;
  enabled: boolean;
  active: boolean;
  children: ReadonlyArray<FolderViewModel>;
  pager?: Readonly<{
    page: number;
    pages: number;
    hasPrevious: boolean;
    hasNext: boolean;
  }>;
}>;

export type SourceListViewModel = Readonly<{
  libraryCount: number;
  libraryActive: boolean;
  fileLocationsEnabled: boolean;
  fileLocationFailures: ReadonlyArray<Readonly<{ key: string; range: string }>>;
  rootExpanded: boolean;
  rootActive: boolean;
  rootChildren: ReadonlyArray<FolderViewModel>;
  rootPager?: FolderViewModel["pager"];
  albums: ReadonlyArray<
    Readonly<{
      id: string;
      name: string;
      photoCount: number;
      hasSavedPosition: boolean;
      active: boolean;
    }>
  >;
}>;

export type GridPhotoViewModel = Readonly<{
  id: string;
  selectionState: ViewSelectionState;
  rating: number;
  preview: Readonly<{
    state: "inspection-pending" | "ready" | "unavailable" | "failed";
    thumbnailUrl?: string;
  }>;
}>;

export type GridViewModel = Readonly<{
  total: number;
  photoAt(index: number): GridPhotoViewModel | undefined;
}>;

export type PhotoFactsViewModel = Readonly<{
  index: number;
  total: number;
  selectionState?: ViewSelectionState | undefined;
  rating?: number | undefined;
}>;

export type PhotoShellViewModel = PhotoFactsViewModel &
  Readonly<{
    sourceName: string;
    photoId?: string | undefined;
    available?: boolean | undefined;
    previewSource?: ViewPreviewSource | undefined;
    limitedDetail?: boolean | undefined;
    previewUrl?: string | undefined;
  }>;

export type MembershipViewModel = Readonly<{
  albums: ReadonlyArray<Readonly<{ id: string; name: string }>>;
  photoPresent: boolean;
  addingAlbumIds: ReadonlyArray<string>;
  inOpenAlbum: boolean;
  removing: boolean;
}>;

export type ControlsViewModel = Readonly<{
  decisionEnabled: boolean;
  clearEnabled: boolean;
  backEnabled: boolean;
  refreshEnabled: boolean;
  previousEnabled: boolean;
  nextEnabled: boolean;
  undoEnabled: boolean;
}>;

export interface LibraryBrowserView {
  readonly photoStatusSurface: object;
  isPhotoStatusSurfaceCurrent(surface: object): boolean;
  setPhotoStatus(text: string): void;
  presentSummary(
    text: string,
    action?: Readonly<{
      kind: "retry-library-check" | "refresh-current-source";
      presentationId: number;
    }>,
  ): void;
  setConnection(
    connected: boolean,
    sourceRetryVisible: boolean,
    photoRetryVisible: boolean,
  ): void;
  setSourceTitle(name: string): void;
  setGridStatus(text: string): void;
  renderSources(model: SourceListViewModel): void;
  setControls(model: ControlsViewModel): void;
  renderMembership(model: MembershipViewModel): void;
  prepareSourceOpen(name: string): void;
  renderGrid(model: GridViewModel): void;
  scheduleGridRender(): void;
  cancelGridRender(): void;
  gridVisible(): boolean;
  scrollToGridIndex(index: number): void;
  showGrid(index?: number): void;
  enterPhoto(): void;
  renderPhotoFacts(model: PhotoFactsViewModel): void;
  renderPhotoShell(
    model: PhotoShellViewModel,
  ): ReviewImagePresentation | undefined;
  presentReviewImage(
    url: string,
    index: number,
    total: number,
  ): ReviewImagePresentation | undefined;
  reviewImageMatches(url: string): boolean;
  showPreviewUnavailable(text: string): void;
  setPreviewFacts(
    source: ViewPreviewSource | undefined,
    limited: boolean,
  ): void;
  setAlbumFormMessage(formId: string, message: string): void;
  setAlbumFormPending(formId: string, pending: boolean, name?: string): void;
  dismissAlbumForm(formId: string): void;
  dispose(): void;
}

type AlbumFormState = {
  kind: "create" | "rename" | "delete";
  formId: string;
  albumId?: string;
  name: string;
  pending: boolean;
  message?: string;
};

export function createLibraryBrowserView(
  root: HTMLElement,
  emit: (intent: LibraryBrowserIntent) => void,
  bindThumbnail: (binding: GridThumbnailBinding) => void,
): LibraryBrowserView {
  let alive = true;
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
  const previewSource = required<HTMLElement>(root, "[data-source]");
  const limited = required<HTMLElement>(root, "[data-limited]");
  const status = required<HTMLElement>(root, "[data-status]");
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
  const undo = required<HTMLButtonElement>(root, "[data-undo]");
  const detail = required<HTMLButtonElement>(root, "[data-detail]");
  const ratings = required<HTMLElement>(root, "[data-ratings]");
  const selectFeedback = required<HTMLElement>(root, "[data-select-feedback]");
  const rejectFeedback = required<HTMLElement>(root, "[data-reject-feedback]");

  const send = (intent: LibraryBrowserIntent): void => {
    if (alive) emit(intent);
  };

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

  let photoStatusSurface: object = {};
  let sourceReturn: "grid" | "photo" = "grid";
  let sourceModel: SourceListViewModel | undefined;
  let membershipModel: MembershipViewModel | undefined;
  let albumFormCounter = 0;
  let albumForm: AlbumFormState | undefined;
  let renderedColumns = 0;
  let renderedViewportHeight = 0;
  let gridRenderFrame: number | undefined;
  let selectedAlbumId = "";
  let zoomed = false;
  let panX = 0;
  let panY = 0;
  let photoSurface: object = {};
  let currentPhotoId: string | undefined;
  let currentSelection: ViewSelectionState = "undecided";
  let interactionEnabled = false;
  let pointer:
    | {
        id: number;
        startX: number;
        startY: number;
        lastX: number;
        lastY: number;
        startedAt: number;
        vertical: boolean;
        surface: object;
        photoId: string;
      }
    | undefined;

  const compactSources = window.matchMedia("(max-width: 760px)");
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
  const openSources = (returnTo: "grid" | "photo") => {
    if (!alive) return;
    sourceReturn = returnTo;
    browser.classList.add("sources-open");
    setSourcesExpanded(true);
    syncSourcePanel();
    sourceClose.focus();
  };
  const closeSources = (restoreFocus = true) => {
    if (!alive) return;
    browser.classList.remove("sources-open");
    setSourcesExpanded(false);
    syncSourcePanel();
    if (restoreFocus)
      (sourceReturn === "grid" ? sourceToggle : photoSourceToggle).focus();
  };
  const onSourceViewportChange = () => {
    if (!alive) return;
    browser.classList.remove("sources-open");
    setSourcesExpanded(false);
    syncSourcePanel();
  };
  const columns = () =>
    Math.max(
      1,
      Math.floor(Math.max(320, gridViewport.clientWidth) / GRID_CELL_WIDTH),
    );
  const effectiveViewportHeight = () =>
    Math.max(360, Math.min(gridViewport.clientHeight, window.innerHeight));

  const clearPointer = () => {
    const id = pointer?.id;
    pointer = undefined;
    if (id !== undefined && preview.hasPointerCapture(id))
      preview.releasePointerCapture(id);
    stage.style.transform = "";
    selectFeedback.classList.remove("pending");
    rejectFeedback.classList.remove("pending");
  };
  const applyTransform = () => {
    const image = stage.querySelector<HTMLImageElement>("img");
    if (!image) return;
    image.style.transform = zoomed
      ? `translate(${panX}px, ${panY}px) scale(2)`
      : "translate(0, 0) scale(1)";
    preview.classList.toggle("detail", zoomed);
  };
  const resetTransform = () => {
    if (!alive) return;
    zoomed = false;
    panX = 0;
    panY = 0;
    detail.setAttribute("aria-pressed", "false");
    detail.textContent = "Detail Review";
    applyTransform();
  };

  const createSourceButton = (
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
    required<HTMLElement>(button, "strong").textContent = name;
    required<HTMLElement>(button, "span").textContent =
      `${count} Photos${saved ? " · Resume" : ""}`;
    return button;
  };

  const createFolderPager = (
    location: string,
    pager: NonNullable<FolderViewModel["pager"]>,
  ) => {
    const depth = location ? location.split("/").length : 0;
    const controls = document.createElement("div");
    controls.className = "folder-pager";
    controls.style.marginLeft = `${Math.min(depth, 6) * 12}px`;
    const prior = document.createElement("button");
    prior.type = "button";
    prior.className = "folder-page-button";
    prior.textContent = "Previous Folders";
    prior.disabled = !pager.hasPrevious;
    prior.addEventListener("click", () =>
      send({ kind: "folder-page", location, direction: -1 }),
    );
    const label = document.createElement("span");
    label.className = "folder-page-label";
    label.textContent = `${pager.page + 1} / ${pager.pages}`;
    const more = document.createElement("button");
    more.type = "button";
    more.className = "folder-page-button";
    more.textContent = "More Folders";
    more.disabled = !pager.hasNext;
    more.addEventListener("click", () =>
      send({ kind: "folder-page", location, direction: 1 }),
    );
    controls.append(prior, label, more);
    return controls;
  };

  const appendFolder = (
    fragment: DocumentFragment,
    folder: FolderViewModel,
  ) => {
    const depth = folder.location.split("/").length;
    const row = document.createElement("div");
    row.className = "folder-row folder-child";
    row.style.marginLeft = `${Math.min(depth - 1, 6) * 12}px`;
    const expand = document.createElement("button");
    expand.type = "button";
    expand.className = "folder-expand";
    expand.setAttribute("aria-expanded", String(folder.expanded));
    expand.textContent = folder.expanded ? "▾" : "▸";
    expand.setAttribute("aria-label", `Toggle ${folder.name} subfolders`);
    expand.disabled = !folder.hasDescendantFolders;
    expand.addEventListener("click", () =>
      send({
        kind: "folder-toggle",
        location: folder.location,
        expanded: folder.expanded,
      }),
    );
    const button = createSourceButton(
      `${folder.name}${folder.hasDescendantFolders ? " · Subfolders" : ""}`,
      folder.photoCount,
      folder.active,
      false,
      false,
    );
    button.disabled = !folder.enabled;
    button.addEventListener("click", () =>
      send({
        kind: "source-open",
        source: {
          kind: "folder",
          location: folder.location,
          name: folder.name,
        },
      }),
    );
    row.append(expand, button);
    fragment.append(row);
    if (!folder.expanded) return;
    for (const child of folder.children) appendFolder(fragment, child);
    if (folder.pager)
      fragment.append(createFolderPager(folder.location, folder.pager));
  };

  const nextAlbumFormId = () => `album-form-${++albumFormCounter}`;
  const openAlbumForm = (
    kind: AlbumFormState["kind"],
    albumId = "",
    name = "",
  ) => {
    if (!alive) return;
    albumForm = {
      kind,
      formId: nextAlbumFormId(),
      ...(albumId ? { albumId } : {}),
      name,
      pending: false,
    };
    send({ kind: "album-form-open", form: { ...albumForm } });
    if (sourceModel) renderSources(sourceModel);
  };
  const closeAlbumForm = (form: AlbumFormState) => {
    if (!alive || albumForm !== form) return;
    albumForm = undefined;
    send({ kind: "album-form-close", formId: form.formId });
    if (sourceModel) renderSources(sourceModel);
  };
  const albumFormMessage = () => {
    const message = document.createElement("p");
    message.className = "album-form-message";
    message.setAttribute("role", "alert");
    return message;
  };
  const albumNameInput = (form: AlbumFormState) => {
    const input = document.createElement("input");
    input.type = "text";
    input.name = "name";
    input.dataset.albumFormId = form.formId;
    input.setAttribute("aria-label", "Album name");
    input.value = form.name;
    input.addEventListener("input", () => {
      if (alive && albumForm === form) {
        form.name = input.value;
        delete form.message;
      }
    });
    return input;
  };
  const createAlbumEditForm = (
    form: AlbumFormState,
    label: string,
    saveText: string,
  ) => {
    const element = document.createElement("form");
    element.className = "album-form";
    element.setAttribute("aria-label", label);
    const input = albumNameInput(form);
    const message = albumFormMessage();
    message.textContent = form.message ?? "";
    const save = document.createElement("button");
    save.type = "submit";
    save.textContent = saveText;
    save.disabled = form.pending;
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.textContent = "Cancel";
    cancel.addEventListener("click", () => closeAlbumForm(form));
    element.append(input, save, cancel, message);
    element.addEventListener("submit", (event) => {
      event.preventDefault();
      if (albumForm !== form || form.pending) return;
      send({
        kind: "album-form-submit",
        formId: form.formId,
        name: input.value,
      });
    });
    return element;
  };
  const createAlbumTools = (album: SourceListViewModel["albums"][number]) => {
    const tools = document.createElement("div");
    tools.className = "album-tools";
    if (albumForm?.kind === "rename" && albumForm.albumId === album.id) {
      tools.append(createAlbumEditForm(albumForm, "Rename Album", "Save Name"));
      return tools;
    }
    if (albumForm?.kind === "delete" && albumForm.albumId === album.id) {
      const form = albumForm;
      const confirmBox = document.createElement("div");
      confirmBox.className = "album-confirm";
      confirmBox.setAttribute("role", "alert");
      const text = paragraph("Photos and Original Files remain unchanged.");
      const confirm = document.createElement("button");
      confirm.type = "button";
      confirm.textContent = "Delete Album";
      confirm.disabled = form.pending;
      confirm.addEventListener("click", () => {
        if (albumForm === form && !form.pending)
          send({ kind: "album-form-submit", formId: form.formId });
      });
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.textContent = "Cancel";
      cancel.addEventListener("click", () => closeAlbumForm(form));
      confirmBox.append(text, confirm, cancel);
      tools.append(confirmBox);
      return tools;
    }
    const rename = document.createElement("button");
    rename.type = "button";
    rename.className = "album-tool";
    rename.textContent = "Rename";
    rename.setAttribute("aria-label", `Rename ${album.name}`);
    rename.addEventListener("click", () =>
      openAlbumForm("rename", album.id, album.name),
    );
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "album-tool";
    remove.textContent = "Delete";
    remove.setAttribute("aria-label", `Delete ${album.name}`);
    remove.addEventListener("click", () =>
      openAlbumForm("delete", album.id, album.name),
    );
    tools.append(rename, remove);
    return tools;
  };

  const renderSources = (model: SourceListViewModel) => {
    if (!alive) return;
    sourceModel = model;
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
    const library = createSourceButton(
      "All Photos",
      model.libraryCount,
      model.libraryActive,
    );
    library.addEventListener("click", () =>
      send({ kind: "source-open", source: { kind: "library" } }),
    );
    sourceList.append(library);
    const fileHeading = document.createElement("h3");
    fileHeading.textContent = "File Locations";
    sourceList.append(fileHeading);
    for (const failure of model.fileLocationFailures) {
      const retryFolders = document.createElement("button");
      retryFolders.type = "button";
      retryFolders.className = "folder-more";
      retryFolders.textContent = `Retry File Locations (${failure.range})`;
      retryFolders.addEventListener("click", () =>
        send({ kind: "file-location-retry", key: failure.key }),
      );
      sourceList.append(retryFolders);
    }
    const rootCard = createSourceButton(
      "Library Folder",
      model.libraryCount,
      model.rootActive,
      false,
      false,
    );
    rootCard.disabled = !model.fileLocationsEnabled;
    rootCard.addEventListener("click", () =>
      send({
        kind: "source-open",
        source: { kind: "folder", location: "", name: "Library Folder" },
      }),
    );
    const rootRow = document.createElement("div");
    rootRow.className = "folder-row folder-root";
    const rootExpand = document.createElement("button");
    rootExpand.type = "button";
    rootExpand.className = "folder-expand";
    rootExpand.setAttribute("aria-expanded", String(model.rootExpanded));
    rootExpand.textContent = model.rootExpanded ? "▾" : "▸";
    rootExpand.setAttribute("aria-label", "Toggle Library Folder subfolders");
    rootExpand.addEventListener("click", () =>
      send({
        kind: "folder-toggle",
        location: "",
        expanded: model.rootExpanded,
      }),
    );
    rootRow.append(rootExpand, rootCard);
    const folders = document.createDocumentFragment();
    folders.append(rootRow);
    if (model.rootExpanded) {
      for (const folder of model.rootChildren) appendFolder(folders, folder);
      if (model.rootPager)
        folders.append(createFolderPager("", model.rootPager));
    }
    sourceList.append(folders);
    const albumHeadingRow = document.createElement("div");
    albumHeadingRow.className = "album-heading";
    const albumHeading = document.createElement("h3");
    albumHeading.textContent = "Albums";
    const newAlbum = document.createElement("button");
    newAlbum.type = "button";
    newAlbum.className = "album-new";
    newAlbum.textContent = "New Album";
    newAlbum.addEventListener("click", () => openAlbumForm("create"));
    albumHeadingRow.append(albumHeading, newAlbum);
    sourceList.append(albumHeadingRow);
    if (albumForm?.kind === "create")
      sourceList.append(
        createAlbumEditForm(albumForm, "Create Album", "Create Album"),
      );
    for (const album of model.albums) {
      const button = createSourceButton(
        album.name,
        album.photoCount,
        album.active,
        album.hasSavedPosition,
        false,
      );
      button.addEventListener("click", () =>
        send({ kind: "source-open", source: { kind: "album", id: album.id } }),
      );
      const row = document.createElement("div");
      row.className = "album-row";
      row.append(button, createAlbumTools(album));
      sourceList.append(row);
    }
    if (focusedFormId) {
      const restored = sourceList.querySelector<HTMLInputElement>(
        `input[data-album-form-id="${focusedFormId}"]`,
      );
      if (restored) {
        const end = restored.value.length;
        restored.focus();
        restored.setSelectionRange(
          Math.min(focusedSelection?.[0] ?? end, end),
          Math.min(focusedSelection?.[1] ?? end, end),
        );
      }
    }
  };

  const scheduleGridRender = () => {
    if (!alive || gridRenderFrame !== undefined) return;
    gridRenderFrame = requestAnimationFrame(() => {
      gridRenderFrame = undefined;
      send({ kind: "grid-render" });
    });
  };
  const cancelGridRender = () => {
    if (gridRenderFrame === undefined) return;
    cancelAnimationFrame(gridRenderFrame);
    gridRenderFrame = undefined;
  };
  const renderGrid = (model: GridViewModel) => {
    if (!alive) return;
    const count = columns();
    const viewportHeight = effectiveViewportHeight();
    renderedColumns = count;
    renderedViewportHeight = viewportHeight;
    const height = `${Math.ceil(model.total / count) * GRID_CELL_HEIGHT}px`;
    gridCanvas.style.height = height;
    gridLayer.style.height = height;
    const firstRow = Math.max(
      0,
      Math.floor(gridViewport.scrollTop / GRID_CELL_HEIGHT) - 2,
    );
    const visibleRows = Math.ceil(viewportHeight / GRID_CELL_HEIGHT) + 4;
    const start = firstRow * count;
    const end = Math.min(model.total, start + visibleRows * count);
    gridLayer.replaceChildren();
    for (let index = start; index < end; index += 1) {
      const cell = document.createElement("button");
      cell.type = "button";
      cell.className = "photo-cell";
      cell.style.left = `${(index % count) * GRID_CELL_WIDTH}px`;
      cell.style.top = `${Math.floor(index / count) * GRID_CELL_HEIGHT}px`;
      const photo = model.photoAt(index);
      if (!photo) {
        cell.disabled = true;
        cell.textContent = "Loading…";
        send({ kind: "grid-window", index });
      } else {
        cell.dataset.photoIndex = String(index);
        const image = document.createElement("img");
        image.alt = `Photo ${index + 1} of ${model.total}`;
        image.loading = "lazy";
        image.fetchPriority = "low";
        image.decoding = "async";
        image.draggable = false;
        image.className = "thumbnail";
        const badge = document.createElement("span");
        badge.className = `cell-state ${photo.selectionState}`;
        badge.textContent =
          photo.selectionState === "undecided"
            ? ""
            : photo.selectionState === "selected"
              ? "✓"
              : "×";
        const caption = document.createElement("span");
        caption.className = "cell-caption";
        caption.textContent = photo.rating
          ? `${index + 1} · ${photo.rating}★`
          : String(index + 1);
        cell.append(image, badge, caption);
        cell.addEventListener("click", () =>
          send({ kind: "open-photo", index }),
        );
        if (alive)
          bindThumbnail({
            photoId: photo.id,
            preview: photo.preview,
            target: image,
          });
      }
      gridLayer.append(cell);
    }
  };

  const renderPhotoFacts = (model: PhotoFactsViewModel) => {
    if (!alive) return;
    position.textContent = `${model.index + 1} / ${model.total}`;
    currentSelection = model.selectionState ?? "undecided";
    selection.textContent = selectionLabel(currentSelection);
    const value = model.rating ?? 0;
    rating.textContent = `${value} ${value === 1 ? "star" : "stars"}`;
  };
  const presentReviewImage = (
    url: string,
    index: number,
    total: number,
  ): ReviewImagePresentation | undefined => {
    if (!alive) return undefined;
    const surface = photoStatusSurface;
    const image = document.createElement("img");
    image.alt = `Photo ${index + 1} of ${total}`;
    image.draggable = false;
    image.fetchPriority = "high";
    image.decoding = "async";
    stage.replaceChildren(image);
    const target: ReviewImageTarget = {
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
      setSource(nextUrl) {
        image.src = nextUrl;
      },
      clearSource() {
        image.removeAttribute("src");
      },
    };
    return {
      target,
      resolvedUrl: new URL(url, window.location.href).href,
      surface,
    };
  };
  const renderPhotoShell = (model: PhotoShellViewModel) => {
    if (!alive) return undefined;
    photoTitle.textContent = model.sourceName;
    currentPhotoId = model.photoId;
    photoSurface = {};
    renderPhotoFacts(model);
    previewSource.textContent = sourceLabel(model.previewSource);
    limited.hidden = !model.limitedDetail;
    let image: ReviewImagePresentation | undefined;
    if (model.previewUrl)
      image = presentReviewImage(model.previewUrl, model.index, model.total);
    else
      stage.replaceChildren(
        paragraph(model.photoId ? "Loading Preview…" : "Photo unavailable"),
      );
    setPhotoStatus(
      model.photoId && model.available === false
        ? "Original File is unavailable. Decisions remain available."
        : "",
    );
    return image;
  };

  const setPhotoStatus = (text: string) => {
    if (!alive || status.textContent === text) return;
    photoStatusSurface = {};
    status.textContent = text;
  };

  const toggleDetail = () => {
    if (!alive || !stage.querySelector("img")) return;
    zoomed = !zoomed;
    panX = 0;
    panY = 0;
    detail.setAttribute("aria-pressed", String(zoomed));
    detail.textContent = zoomed ? "Exit Detail" : "Detail Review";
    applyTransform();
  };
  const pointerDown = (event: PointerEvent) => {
    if (
      !alive ||
      pointer ||
      !event.isPrimary ||
      !interactionEnabled ||
      !currentPhotoId
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
      surface: photoSurface,
      photoId: currentPhotoId,
    };
    preview.setPointerCapture(event.pointerId);
  };
  const pointerMove = (event: PointerEvent) => {
    if (!alive || !pointer || pointer.id !== event.pointerId) return;
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
    selectFeedback.classList.toggle("pending", dx > SWIPE_PENDING_PIXELS);
    rejectFeedback.classList.toggle("pending", dx < -SWIPE_PENDING_PIXELS);
  };
  const finishPointer = (event: PointerEvent, cancelled = false) => {
    if (!alive || !pointer || pointer.id !== event.pointerId) return;
    const active = pointer;
    clearPointer();
    if (
      zoomed ||
      cancelled ||
      active.vertical ||
      !interactionEnabled ||
      active.surface !== photoSurface ||
      active.photoId !== currentPhotoId
    )
      return;
    const dx = event.clientX - active.startX;
    const elapsed = Math.max(1, event.timeStamp - active.startedAt);
    const velocity = Math.abs(dx) / elapsed;
    if (
      Math.abs(dx) >= SWIPE_COMMIT_PIXELS ||
      (Math.abs(dx) >= 48 && velocity >= SWIPE_COMMIT_VELOCITY)
    )
      send({
        kind: "photo-mutation",
        field: "selectionState",
        value: dx > 0 ? "selected" : "rejected",
        advance: true,
      });
  };
  const keydown = (event: KeyboardEvent) => {
    if (!alive) return;
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
      send({ kind: "undo" });
      return;
    }
    if (modifier || event.shiftKey || photoView.hidden) return;
    if (event.key === "ArrowLeft") send({ kind: "previous" });
    else if (event.key === "ArrowRight") send({ kind: "next" });
    else if (event.key.toLowerCase() === "p")
      send({
        kind: "photo-mutation",
        field: "selectionState",
        value: "selected",
        advance: true,
      });
    else if (event.key.toLowerCase() === "x")
      send({
        kind: "photo-mutation",
        field: "selectionState",
        value: "rejected",
        advance: true,
      });
    else if (
      event.key.toLowerCase() === "u" &&
      currentSelection !== "undecided"
    )
      send({
        kind: "photo-mutation",
        field: "selectionState",
        value: "undecided",
        advance: false,
      });
    else if (/^[0-5]$/.test(event.key))
      send({
        kind: "photo-mutation",
        field: "rating",
        value: Number(event.key),
        advance: false,
      });
  };

  const onResize = () => {
    requestAnimationFrame(() => {
      if (!alive || gridView.hidden) return;
      if (
        columns() === renderedColumns &&
        effectiveViewportHeight() === renderedViewportHeight
      )
        return;
      send({ kind: "grid-resize" });
    });
  };
  const onScroll = () => {
    scheduleGridRender();
    send({
      kind: "grid-window",
      index: Math.floor(gridViewport.scrollTop / GRID_CELL_HEIGHT) * columns(),
    });
  };

  const renderMembership = (model: MembershipViewModel) => {
    if (!alive) return;
    membershipModel = model;
    albumSelect.replaceChildren();
    if (!model.albums.some((album) => album.id === selectedAlbumId))
      selectedAlbumId = "";
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = model.albums.length
      ? "Choose Album…"
      : "No Albums yet";
    placeholder.disabled = true;
    placeholder.selected = selectedAlbumId === "";
    albumSelect.append(placeholder);
    for (const album of model.albums) {
      const option = document.createElement("option");
      option.value = album.id;
      option.textContent = album.name;
      option.selected = album.id === selectedAlbumId;
      albumSelect.append(option);
    }
    albumSelect.disabled = !model.albums.length;
    addToAlbum.disabled =
      !model.albums.length ||
      !model.photoPresent ||
      !selectedAlbumId ||
      model.addingAlbumIds.includes(selectedAlbumId);
    removeFromAlbum.hidden = !model.inOpenAlbum;
    removeFromAlbum.disabled = !model.inOpenAlbum || model.removing;
  };

  compactSources.addEventListener("change", onSourceViewportChange);
  sourceToggle.addEventListener("click", () => openSources("grid"));
  photoSourceToggle.addEventListener("click", () => openSources("photo"));
  sourceClose.addEventListener("click", () => closeSources());
  sourceScrim.addEventListener("click", () => closeSources());
  gridViewport.addEventListener("scroll", onScroll);
  window.addEventListener("resize", onResize);
  window.addEventListener("keydown", keydown);
  back.addEventListener("click", () => send({ kind: "show-grid" }));
  refresh.addEventListener("click", () => send({ kind: "refresh" }));
  retry.addEventListener("click", () => send({ kind: "retry-source" }));
  retryPhoto.addEventListener("click", () => send({ kind: "retry-photo" }));
  previous.addEventListener("click", () => send({ kind: "previous" }));
  next.addEventListener("click", () => send({ kind: "next" }));
  undo.addEventListener("click", () => send({ kind: "undo" }));
  detail.addEventListener("click", toggleDetail);
  stage.addEventListener("dblclick", toggleDetail);
  select.addEventListener("click", () =>
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: "selected",
      advance: true,
    }),
  );
  reject.addEventListener("click", () =>
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: "rejected",
      advance: true,
    }),
  );
  clear.addEventListener("click", () =>
    send({
      kind: "photo-mutation",
      field: "selectionState",
      value: "undecided",
      advance: false,
    }),
  );
  ratings.addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
      "[data-rating-value]",
    );
    if (button)
      send({
        kind: "photo-mutation",
        field: "rating",
        value: Number(button.dataset.ratingValue),
        advance: false,
      });
  });
  albumSelect.addEventListener("change", () => {
    if (!alive) return;
    selectedAlbumId = albumSelect.value;
    if (membershipModel) renderMembership(membershipModel);
  });
  addToAlbum.addEventListener("click", () => {
    if (selectedAlbumId)
      send({ kind: "membership-add", albumId: selectedAlbumId });
  });
  removeFromAlbum.addEventListener("click", () =>
    send({ kind: "membership-remove" }),
  );
  preview.addEventListener("pointerdown", pointerDown);
  preview.addEventListener("pointermove", pointerMove);
  preview.addEventListener("pointerup", (event) => finishPointer(event));
  preview.addEventListener("pointercancel", (event) =>
    finishPointer(event, true),
  );
  preview.addEventListener("lostpointercapture", (event) =>
    finishPointer(event, true),
  );
  syncSourcePanel();

  return {
    get photoStatusSurface() {
      return photoStatusSurface;
    },
    isPhotoStatusSurfaceCurrent: (surface) =>
      alive && surface === photoStatusSurface,
    setPhotoStatus,
    presentSummary(text, action) {
      if (!alive) return;
      summaryStatus.replaceChildren(document.createTextNode(text));
      if (!action) return;
      const button = document.createElement("button");
      button.type = "button";
      button.className = "summary-action";
      button.textContent =
        action.kind === "retry-library-check"
          ? "Retry Library Check"
          : "Refresh Current Source";
      button.addEventListener("click", () =>
        send({
          kind: "summary-action",
          presentationId: action.presentationId,
        }),
      );
      summaryStatus.append(" ", button);
    },
    setConnection(isConnected, sourceRetryVisible, photoRetryVisible) {
      if (!alive) return;
      if (!isConnected) clearPointer();
      connection.textContent = isConnected ? "Connected" : "Disconnected";
      connection.classList.toggle("offline", !isConnected);
      retry.hidden = !sourceRetryVisible;
      retryPhoto.hidden = !photoRetryVisible;
    },
    setSourceTitle(name) {
      if (!alive) return;
      gridTitle.textContent = name;
      photoTitle.textContent = name;
    },
    setGridStatus(text) {
      if (alive) gridStatus.textContent = text;
    },
    renderSources,
    setControls(model) {
      if (!alive) return;
      interactionEnabled = model.decisionEnabled;
      for (const button of [
        select,
        reject,
        ...Array.from(ratings.querySelectorAll<HTMLButtonElement>("button")),
      ])
        button.disabled = !model.decisionEnabled;
      clear.disabled = !model.clearEnabled;
      back.disabled = !model.backEnabled;
      refresh.disabled = !model.refreshEnabled;
      previous.disabled = !model.previousEnabled;
      next.disabled = !model.nextEnabled;
      undo.disabled = !model.undoEnabled;
      detail.disabled = !stage.querySelector("img");
    },
    renderMembership,
    prepareSourceOpen(name) {
      if (!alive) return;
      const returnFocus = browser.classList.contains("sources-open");
      cancelGridRender();
      gridLayer.replaceChildren();
      stage.replaceChildren();
      gridView.hidden = false;
      photoView.hidden = true;
      closeSources(false);
      if (returnFocus) gridViewport.focus();
      gridTitle.textContent = name;
      gridStatus.textContent = "Preparing Library order…";
      currentPhotoId = undefined;
      photoSurface = {};
    },
    renderGrid,
    scheduleGridRender,
    cancelGridRender,
    gridVisible: () => alive && !gridView.hidden,
    scrollToGridIndex(index) {
      if (alive)
        gridViewport.scrollTop =
          Math.floor(index / columns()) * GRID_CELL_HEIGHT;
    },
    showGrid(index) {
      if (!alive) return;
      photoView.hidden = true;
      gridView.hidden = false;
      closeSources(false);
      if (index !== undefined)
        gridViewport.scrollTop =
          Math.floor(index / columns()) * GRID_CELL_HEIGHT;
      scheduleGridRender();
    },
    enterPhoto() {
      if (!alive) return;
      gridView.hidden = true;
      photoView.hidden = false;
      syncSourcePanel();
      photoView.focus();
      resetTransform();
      photoSurface = {};
    },
    renderPhotoFacts,
    renderPhotoShell,
    presentReviewImage,
    reviewImageMatches(url) {
      if (!alive) return false;
      const image = stage.querySelector<HTMLImageElement>("img");
      return Boolean(
        image && image.src === new URL(url, window.location.href).href,
      );
    },
    showPreviewUnavailable(text) {
      if (alive && !stage.querySelector("img"))
        stage.replaceChildren(paragraph(text));
    },
    setPreviewFacts(value, isLimited) {
      if (!alive) return;
      previewSource.textContent = sourceLabel(value);
      limited.hidden = !isLimited;
    },
    setAlbumFormMessage(formId, message) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      albumForm.message = message;
      albumForm.pending = false;
      if (sourceModel) renderSources(sourceModel);
    },
    setAlbumFormPending(formId, pending, name) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      albumForm.pending = pending;
      if (name !== undefined) albumForm.name = name;
      delete albumForm.message;
      if (sourceModel) renderSources(sourceModel);
    },
    dismissAlbumForm(formId) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      albumForm = undefined;
      if (sourceModel) renderSources(sourceModel);
    },
    dispose() {
      if (!alive) return;
      alive = false;
      clearPointer();
      cancelGridRender();
      compactSources.removeEventListener("change", onSourceViewportChange);
      gridViewport.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onResize);
      window.removeEventListener("keydown", keydown);
    },
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

function selectionLabel(value?: ViewSelectionState): string {
  return value === "selected"
    ? "Selected"
    : value === "rejected"
      ? "Rejected"
      : "Undecided";
}

function sourceLabel(source?: ViewPreviewSource): string {
  return source === "matching-jpeg"
    ? "JPEG"
    : source === "embedded-raw-jpeg"
      ? "RAW embedded JPEG"
      : "—";
}

function clamp(value: number, low: number, high: number): number {
  return Math.max(low, Math.min(high, value));
}
