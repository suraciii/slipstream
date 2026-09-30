import { formatPhotoCount } from "./photo-count.js";
import type {
  FolderAlbumViewModel,
  FolderViewModel,
  SourceListViewModel,
} from "./library-browser-view.js";

type SourceReference =
  | Readonly<{ kind: "library" }>
  | Readonly<{ kind: "album"; id: string }>
  | Readonly<{ kind: "folder"; location: string; name: string }>;

type SourceIntent =
  | Readonly<{ kind: "folder-page"; location: string; direction: -1 | 1 }>
  | Readonly<{ kind: "folder-toggle"; location: string; expanded: boolean }>
  | Readonly<{ kind: "source-open"; source: SourceReference }>
  | Readonly<{ kind: "file-location-retry"; key: string }>
  | Readonly<{ kind: "folder-album-add"; albumId: string }>;

type SourceElement = HTMLAnchorElement | HTMLButtonElement;

export type SourceListPresenter = Readonly<{
  render(model: SourceListViewModel): void;
  renderFolderAlbum(model: FolderAlbumViewModel): void;
  sourceModel(): SourceListViewModel | undefined;
  folderAlbumSelection(): string;
  dispose(): void;
}>;

export type SourceListPresenterOptions = Readonly<{
  sourceList: HTMLElement;
  albumResume: HTMLButtonElement;
  folderAlbumControls: HTMLElement;
  folderAlbumSelect: HTMLSelectElement;
  addFolderToAlbum: HTMLButtonElement;
  folderAlbumStatus: HTMLElement;
  alive: () => boolean;
  send: (intent: SourceIntent) => void;
  sourceAddress: (source: SourceReference) => string;
  openAlbumForm: (
    kind: "create" | "rename" | "delete",
    albumId?: string,
    name?: string,
  ) => void;
  createAlbumTools: (
    album: SourceListViewModel["albums"][number],
  ) => HTMLElement;
  actionFocusKey: (kind: "create") => string;
  restoreSourceFocus: (
    target: (key: string) => HTMLElement | undefined,
  ) => boolean;
}>;

export const createSourceListPresenter = (
  options: SourceListPresenterOptions,
): SourceListPresenter => {
  let model: SourceListViewModel | undefined;
  let folderAlbumSelection = "";
  let disposed = false;
  const {
    sourceList,
    albumResume,
    folderAlbumControls,
    folderAlbumSelect,
    addFolderToAlbum,
    folderAlbumStatus,
  } = options;
  const interceptDestination = (
    element: SourceElement,
    activate: () => void,
  ) => {
    element.addEventListener("click", (event) => {
      const pointer = event as MouseEvent;
      if (
        pointer.defaultPrevented ||
        pointer.button !== 0 ||
        pointer.metaKey ||
        pointer.ctrlKey ||
        pointer.shiftKey ||
        pointer.altKey
      )
        return;
      pointer.preventDefault();
      activate();
    });
  };
  const createSourceButton = (
    name: string,
    count: number,
    active: boolean,
    address: string,
    openable: boolean,
  ): SourceElement => {
    const element: SourceElement = openable
      ? document.createElement("a")
      : document.createElement("button");
    if (openable) (element as HTMLAnchorElement).href = address;
    else {
      const button = element as HTMLButtonElement;
      button.type = "button";
      button.disabled = true;
    }
    element.className = `source-card${active ? " active" : ""}`;
    if (active) element.setAttribute("aria-current", "true");
    element.title = name;
    element.innerHTML = "<strong></strong><span></span>";
    (element.querySelector("strong") as HTMLElement).textContent = name;
    (element.querySelector("span") as HTMLElement).textContent =
      formatPhotoCount(count);
    return element;
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
      options.send({ kind: "folder-page", location, direction: -1 }),
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
      options.send({ kind: "folder-page", location, direction: 1 }),
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
    if (folder.hasDescendantFolders) {
      const expand = document.createElement("button");
      expand.type = "button";
      expand.className = "folder-expand";
      expand.setAttribute("aria-expanded", String(folder.expanded));
      expand.textContent = folder.expanded ? "▾" : "▸";
      expand.setAttribute("aria-label", `Toggle ${folder.name} subfolders`);
      expand.addEventListener("click", () =>
        options.send({
          kind: "folder-toggle",
          location: folder.location,
          expanded: folder.expanded,
        }),
      );
      row.append(expand);
    }
    const source = {
      kind: "folder" as const,
      location: folder.location,
      name: folder.name,
    };
    const button = createSourceButton(
      `${folder.name}${folder.hasDescendantFolders ? " · Subfolders" : ""}`,
      folder.photoCount,
      folder.active,
      options.sourceAddress(source),
      folder.enabled,
    );
    interceptDestination(button, () =>
      options.send({ kind: "source-open", source }),
    );
    row.append(button);
    fragment.append(row);
    if (!folder.expanded) return;
    for (const child of folder.children) appendFolder(fragment, child);
    if (folder.pager)
      fragment.append(createFolderPager(folder.location, folder.pager));
  };
  const focusTarget = (key: string) =>
    Array.from(
      sourceList.querySelectorAll<HTMLElement>("[data-focus-key]"),
    ).find((candidate) => candidate.dataset.focusKey === key);
  const render = (next: SourceListViewModel) => {
    if (disposed || !options.alive()) return;
    model = next;
    const focused = document.activeElement;
    const focusedKey =
      focused instanceof HTMLElement ? focused.dataset.focusKey : undefined;
    sourceList.replaceChildren();
    const library = createSourceButton(
      "All Photos",
      next.libraryCount,
      next.libraryActive,
      options.sourceAddress({ kind: "library" }),
      true,
    );
    library.dataset.focusKey = "source:library";
    interceptDestination(library, () =>
      options.send({ kind: "source-open", source: { kind: "library" } }),
    );
    sourceList.append(library);
    const heading = document.createElement("h3");
    heading.textContent = "Folders";
    sourceList.append(heading);
    for (const failure of next.fileLocationFailures) {
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "folder-more";
      retry.textContent = `Retry Folders (${failure.range})`;
      retry.addEventListener("click", () =>
        options.send({ kind: "file-location-retry", key: failure.key }),
      );
      sourceList.append(retry);
    }
    const rootSource = {
      kind: "folder" as const,
      location: "",
      name: "Library Folder",
    };
    const rootCard = createSourceButton(
      "Library Folder",
      next.libraryCount,
      next.rootActive,
      options.sourceAddress(rootSource),
      next.fileLocationsEnabled,
    );
    rootCard.dataset.focusKey = "source:folder:";
    interceptDestination(rootCard, () =>
      options.send({ kind: "source-open", source: rootSource }),
    );
    const rootRow = document.createElement("div");
    rootRow.className = "folder-row folder-root";
    const rootExpand = document.createElement("button");
    rootExpand.type = "button";
    rootExpand.className = "folder-expand";
    rootExpand.setAttribute("aria-expanded", String(next.rootExpanded));
    rootExpand.textContent = next.rootExpanded ? "▾" : "▸";
    rootExpand.setAttribute("aria-label", "Toggle Library Folder subfolders");
    rootExpand.addEventListener("click", () =>
      options.send({
        kind: "folder-toggle",
        location: "",
        expanded: next.rootExpanded,
      }),
    );
    rootRow.append(rootExpand, rootCard);
    const folders = document.createDocumentFragment();
    folders.append(rootRow);
    if (next.rootExpanded) {
      for (const folder of next.rootChildren) appendFolder(folders, folder);
      if (next.rootPager) folders.append(createFolderPager("", next.rootPager));
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
    newAlbum.dataset.focusKey = options.actionFocusKey("create");
    newAlbum.addEventListener("click", () => options.openAlbumForm("create"));
    albumHeadingRow.append(albumHeading, newAlbum);
    sourceList.append(albumHeadingRow);
    const activeAlbum = next.albums.find((album) => album.active);
    albumResume.hidden = activeAlbum?.hasSavedPosition !== true;
    for (const album of next.albums) {
      const source = { kind: "album" as const, id: album.id };
      const button = createSourceButton(
        album.name,
        album.photoCount,
        album.active,
        options.sourceAddress(source),
        true,
      );
      button.dataset.focusKey = `source:album:${album.id}`;
      interceptDestination(button, () =>
        options.send({ kind: "source-open", source }),
      );
      const row = document.createElement("div");
      row.className = "album-row";
      row.append(button, options.createAlbumTools(album));
      sourceList.append(row);
    }
    if (options.restoreSourceFocus(focusTarget)) return;
    const restored = focusedKey ? focusTarget(focusedKey) : undefined;
    if (restored && !restored.matches(":disabled")) restored.focus();
  };
  const renderFolderAlbum = (next: FolderAlbumViewModel) => {
    if (disposed || !options.alive()) return;
    folderAlbumControls.hidden = !next.visible;
    if (!next.visible) {
      folderAlbumSelect.replaceChildren();
      folderAlbumStatus.textContent = "";
      folderAlbumSelection = "";
      return;
    }
    if (!next.albums.some((album) => album.id === folderAlbumSelection))
      folderAlbumSelection = next.selectedAlbumId || next.albums[0]?.id || "";
    folderAlbumSelect.replaceChildren(
      ...next.albums.map((album) => {
        const option = document.createElement("option");
        option.value = album.id;
        option.textContent = album.name;
        option.selected = album.id === folderAlbumSelection;
        return option;
      }),
    );
    folderAlbumSelect.disabled = next.pending || !next.albums.length;
    addFolderToAlbum.disabled =
      next.pending || !next.albums.length || !folderAlbumSelection;
    folderAlbumStatus.textContent = next.status ?? "";
  };
  const onFolderChange = () => {
    if (!options.alive()) return;
    folderAlbumSelection = folderAlbumSelect.value;
    addFolderToAlbum.disabled = !folderAlbumSelection;
  };
  const onFolderAdd = () => {
    if (folderAlbumSelection)
      options.send({ kind: "folder-album-add", albumId: folderAlbumSelection });
  };
  folderAlbumSelect.addEventListener("change", onFolderChange);
  addFolderToAlbum.addEventListener("click", onFolderAdd);
  return {
    render,
    renderFolderAlbum,
    sourceModel: () => model,
    folderAlbumSelection: () => folderAlbumSelection,
    dispose: () => {
      disposed = true;
      folderAlbumSelect.removeEventListener("change", onFolderChange);
      addFolderToAlbum.removeEventListener("click", onFolderAdd);
    },
  };
};
