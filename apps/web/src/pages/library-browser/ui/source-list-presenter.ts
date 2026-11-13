import { formatPhotoCount } from "./photo-count.js";
import type {
  FolderAlbumViewModel,
  FolderViewModel,
  SourceListViewModel,
  SourceReference,
} from "./library-browser-view.js";

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
  sourceFilter: HTMLInputElement;
  sourceFilterEmpty: HTMLElement;
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
  let sourceQuery = "";
  let applySourceFilter = (): void => {};
  let disposed = false;
  const {
    sourceList,
    sourceFilter,
    sourceFilterEmpty,
    albumResume,
    folderAlbumControls,
    folderAlbumSelect,
    addFolderToAlbum,
    folderAlbumStatus,
  } = options;
  const sourcePanel = sourceList.closest<HTMLElement>(".source-panel");
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
    prior.dataset.focusKey = `folder:previous:${location}`;
    prior.dataset.focusFallback = `folder:next:${location}`;
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
    more.dataset.focusKey = `folder:next:${location}`;
    more.dataset.focusFallback = `folder:previous:${location}`;
    more.disabled = !pager.hasNext;
    more.addEventListener("click", () =>
      options.send({ kind: "folder-page", location, direction: 1 }),
    );
    controls.append(prior, label, more);
    return controls;
  };
  const appendLoading = (parent: ParentNode, name: string) => {
    const status = document.createElement("span");
    status.className = "folder-loading folder-page-label";
    status.setAttribute("role", "status");
    status.textContent = `Loading subfolders of ${name}…`;
    parent.append(status);
  };
  let folderControlId = 0;
  const appendFolder = (
    parent: HTMLUListElement,
    folder: FolderViewModel,
  ): HTMLLIElement => {
    const item = document.createElement("li");
    item.className = "folder-item";
    const row = document.createElement("div");
    row.className = "folder-row folder-child";
    let childList: HTMLUListElement | undefined;
    if (folder.hasDescendantFolders || folder.children.length) {
      childList = document.createElement("ul");
      childList.className = "folder-children";
      childList.id = `folder-children-${++folderControlId}`;
      childList.dataset.folderExpanded = String(folder.expanded);
      childList.hidden = !folder.expanded;
      const expand = document.createElement("button");
      expand.type = "button";
      expand.className = "folder-expand";
      expand.dataset.focusKey = `folder:expand:${folder.location}`;
      expand.setAttribute("aria-expanded", String(folder.expanded));
      expand.setAttribute(
        "aria-disabled",
        String(folder.loading && !folder.expanded),
      );
      expand.setAttribute("aria-controls", childList.id);
      expand.textContent = folder.expanded ? "▾" : "▸";
      expand.setAttribute("aria-label", `Toggle ${folder.name} subfolders`);
      expand.addEventListener("click", () => {
        if (folder.loading && !folder.expanded) return;
        options.send({
          kind: "folder-toggle",
          location: folder.location,
          expanded: folder.expanded,
        });
      });
      row.append(expand);
    }
    const source = {
      kind: "folder" as const,
      location: folder.location,
      name: folder.name,
    };
    const button = createSourceButton(
      folder.name,
      folder.photoCount,
      folder.active,
      options.sourceAddress(source),
      folder.enabled,
    );
    button.dataset.focusKey = `source:folder:${folder.location}`;
    interceptDestination(button, () =>
      options.send({ kind: "source-open", source }),
    );
    row.append(button);
    addFolderKeyboardNavigation(row);
    item.append(row);
    if (folder.loading) appendLoading(item, folder.name);
    if (childList) {
      for (const child of folder.children) appendFolder(childList, child);
      item.append(childList);
    }
    if (folder.pager) {
      const pager = createFolderPager(folder.location, folder.pager);
      item.append(pager);
    }
    parent.append(item);
    return item;
  };
  const addFolderKeyboardNavigation = (row: HTMLElement): void => {
    row.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      if (sourceQuery) return;
      const expand = row.querySelector<HTMLButtonElement>(".folder-expand");
      if (event.key === "ArrowRight") {
        if (!expand || expand.disabled) return;
        event.preventDefault();
        if (expand.getAttribute("aria-expanded") !== "true") {
          expand.click();
          return;
        }
        const childList = row.parentElement?.querySelector<HTMLElement>(
          ":scope > .folder-children",
        );
        const first = Array.from(
          childList?.querySelectorAll<HTMLElement>(
            ":scope > .folder-item > .folder-row .source-card",
          ) ?? [],
        ).find(
          (card) => !card.closest("[hidden]") && !card.matches(":disabled"),
        );
        first?.focus();
        return;
      }
      if (expand?.getAttribute("aria-expanded") === "true") {
        event.preventDefault();
        expand.click();
        return;
      }
      const parentCard = row
        .closest(".folder-item")
        ?.parentElement?.closest(".folder-item")
        ?.querySelector<HTMLElement>(":scope > .folder-row .source-card");
      if (!parentCard) return;
      event.preventDefault();
      parentCard.focus();
    });
  };
  const focusTarget = (key: string) =>
    Array.from(
      sourceList.querySelectorAll<HTMLElement>("[data-focus-key]"),
    ).find(
      (candidate) =>
        candidate.dataset.focusKey === key &&
        !candidate.closest("[hidden]") &&
        !candidate.matches(":disabled"),
    );
  const render = (next: SourceListViewModel) => {
    if (disposed || !options.alive()) return;
    model = next;
    folderControlId = 0;
    const focused = document.activeElement;
    const focusedKey =
      focused instanceof HTMLElement ? focused.dataset.focusKey : undefined;
    const focusedFallback =
      focused instanceof HTMLElement
        ? focused.dataset.focusFallback
        : undefined;
    const focusedRetryParent =
      focused instanceof HTMLElement ? focused.dataset.retryParent : undefined;
    sourceList.replaceChildren();
    const allPhotosSection = document.createElement("section");
    allPhotosSection.className = "source-group";
    allPhotosSection.setAttribute("aria-label", "All Photos");
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
    allPhotosSection.append(library);
    sourceList.append(allPhotosSection);

    const foldersSection = document.createElement("section");
    foldersSection.className = "source-group";
    foldersSection.setAttribute("aria-labelledby", "folders-heading");
    const folderHeading = document.createElement("h3");
    folderHeading.id = "folders-heading";
    folderHeading.textContent = "Folders";
    foldersSection.append(folderHeading);

    const folderFailures = document.createElement("div");
    folderFailures.className = "source-failures";
    for (const failure of next.fileLocationFailures) {
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "folder-more";
      retry.dataset.focusKey = `folder:retry:${failure.key}`;
      retry.dataset.retryParent = failure.parent;
      retry.setAttribute("aria-disabled", String(failure.loading));
      retry.textContent = `Retry Folders (${failure.range})`;
      retry.addEventListener("click", () => {
        if (!failure.loading)
          options.send({ kind: "file-location-retry", key: failure.key });
      });
      folderFailures.append(retry);
      if (failure.loading) {
        const status = document.createElement("span");
        status.className = "folder-loading folder-page-label";
        status.setAttribute("role", "status");
        status.textContent = `Retrying Folders (${failure.range})…`;
        folderFailures.append(status);
      }
    }
    if (next.fileLocationFailures.length) foldersSection.append(folderFailures);

    const folderTree = document.createElement("ul");
    folderTree.className = "folder-tree";
    folderTree.setAttribute("aria-labelledby", "folders-heading");
    const rootItem = document.createElement("li");
    rootItem.className = "folder-item folder-root-item";
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
    rootExpand.dataset.focusKey = "folder:expand:";
    rootExpand.setAttribute("aria-expanded", String(next.rootExpanded));
    rootExpand.setAttribute(
      "aria-disabled",
      String(next.rootLoading && !next.rootExpanded),
    );
    const rootChildList = document.createElement("ul");
    rootChildList.className = "folder-children";
    rootChildList.id = "folder-children-root";
    rootChildList.dataset.folderExpanded = String(next.rootExpanded);
    rootChildList.hidden = !next.rootExpanded;
    rootExpand.setAttribute("aria-controls", rootChildList.id);
    rootExpand.textContent = next.rootExpanded ? "▾" : "▸";
    rootExpand.setAttribute("aria-label", "Toggle Library Folder subfolders");
    rootExpand.addEventListener("click", () => {
      if (next.rootLoading && !next.rootExpanded) return;
      options.send({
        kind: "folder-toggle",
        location: "",
        expanded: next.rootExpanded,
      });
    });
    if (next.rootChildren.length) rootRow.append(rootExpand);
    rootRow.append(rootCard);
    addFolderKeyboardNavigation(rootRow);
    rootItem.append(rootRow);
    if (next.rootChildren.length) {
      for (const folder of next.rootChildren)
        appendFolder(rootChildList, folder);
      rootItem.append(rootChildList);
    }
    if (next.rootLoading) appendLoading(rootItem, "Library Folder");
    if (next.rootExpanded && next.rootPager) {
      const pager = createFolderPager("", next.rootPager);
      rootItem.append(pager);
    }
    folderTree.append(rootItem);
    foldersSection.append(folderTree);
    sourceList.append(foldersSection);

    const albumsSection = document.createElement("section");
    albumsSection.className = "source-group";
    albumsSection.setAttribute("aria-labelledby", "albums-heading");
    const albumHeadingRow = document.createElement("div");
    albumHeadingRow.className = "album-heading";
    const albumHeading = document.createElement("h3");
    albumHeading.id = "albums-heading";
    albumHeading.textContent = "Albums";
    const newAlbum = document.createElement("button");
    newAlbum.type = "button";
    newAlbum.className = "album-new";
    newAlbum.textContent = "New Album";
    newAlbum.dataset.focusKey = options.actionFocusKey("create");
    newAlbum.addEventListener("click", () => options.openAlbumForm("create"));
    albumHeadingRow.append(albumHeading, newAlbum);
    albumsSection.append(albumHeadingRow);
    const albumList = document.createElement("ul");
    albumList.className = "album-list";
    albumList.setAttribute("aria-labelledby", "albums-heading");
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
      const row = document.createElement("li");
      row.className = "album-row";
      row.append(button, options.createAlbumTools(album));
      albumList.append(row);
    }
    albumsSection.append(albumList);
    sourceList.append(albumsSection);

    const restoreFolderExpansion = () => {
      folderTree
        .querySelectorAll<HTMLElement>(".folder-children")
        .forEach((children) => {
          children.hidden = children.dataset.folderExpanded !== "true";
        });
    };
    const filterFolder = (item: HTMLLIElement): boolean => {
      const name =
        item.querySelector<HTMLElement>(
          ":scope > .folder-row .source-card strong",
        )?.textContent ?? "";
      const childList = item.querySelector<HTMLElement>(
        ":scope > .folder-children",
      );
      const children = Array.from(
        item.querySelectorAll<HTMLLIElement>(
          ":scope > .folder-children > .folder-item",
        ),
      );
      // Visit every child so a matching sibling cannot leave later siblings
      // visible. `.some` would stop the recursion at the first match.
      const childMatch = children.map(filterFolder).includes(true);
      const match =
        name.toLocaleLowerCase().includes(sourceQuery) || childMatch;
      item.hidden = !match;
      if (childMatch && childList) childList.hidden = false;
      return match;
    };
    const filterAlbums = (): boolean => {
      let any = false;
      for (const row of Array.from(
        albumList.querySelectorAll<HTMLElement>(":scope > .album-row"),
      )) {
        const name =
          row.querySelector<HTMLElement>(".source-card strong")?.textContent ??
          "";
        const match = name.toLocaleLowerCase().includes(sourceQuery);
        row.hidden = !match;
        any ||= match;
      }
      return any;
    };
    applySourceFilter = () => {
      sourceFilterEmpty.hidden = true;
      sourcePanel?.classList.toggle("is-filtering", Boolean(sourceQuery));
      restoreFolderExpansion();
      if (!sourceQuery) {
        foldersSection.hidden = false;
        albumsSection.hidden = false;
        folderTree
          .querySelectorAll<HTMLElement>(".folder-item")
          .forEach((item) => {
            item.hidden = false;
          });
        albumList.querySelectorAll<HTMLElement>(".album-row").forEach((row) => {
          row.hidden = false;
        });
        return;
      }
      const folderMatch = filterFolder(rootItem);
      const albumMatch = filterAlbums();
      foldersSection.hidden = !folderMatch;
      albumsSection.hidden = !albumMatch;
      sourceFilterEmpty.hidden = folderMatch || albumMatch;
    };
    applySourceFilter();
    if (options.restoreSourceFocus(focusTarget)) return;
    const restored = focusedKey ? focusTarget(focusedKey) : undefined;
    let retryFallback: HTMLElement | undefined;
    if (!restored && focusedRetryParent !== undefined) {
      let parent = focusedRetryParent;
      while (true) {
        retryFallback = focusTarget(`folder:expand:${parent}`);
        if (retryFallback || !parent) break;
        const slash = parent.lastIndexOf("/");
        parent = slash < 0 ? "" : parent.slice(0, slash);
      }
    }
    const target =
      restored && !restored.matches(":disabled")
        ? restored
        : (retryFallback ??
          (focusedFallback ? focusTarget(focusedFallback) : undefined));
    if (target && !target.matches(":disabled"))
      target.focus({ preventScroll: true });
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
  const onSourceFilter = () => {
    sourceQuery = sourceFilter.value.trim().toLocaleLowerCase();
    applySourceFilter();
  };
  sourceFilter.addEventListener("input", onSourceFilter);
  folderAlbumSelect.addEventListener("change", onFolderChange);
  addFolderToAlbum.addEventListener("click", onFolderAdd);
  return {
    render,
    renderFolderAlbum,
    sourceModel: () => model,
    folderAlbumSelection: () => folderAlbumSelection,
    dispose: () => {
      disposed = true;
      sourceFilter.removeEventListener("input", onSourceFilter);
      folderAlbumSelect.removeEventListener("change", onFolderChange);
      addFolderToAlbum.removeEventListener("click", onFolderAdd);
    },
  };
};
