/// Browser addresses and native history for Library Browser destinations.
///
/// This is the page-local navigation owner described by
/// [Browser Navigation](../../../../design/browser-navigation.md). It decodes
/// and encodes addresses, owns the current browser entry, and reports a
/// traversal intent to the page controller. It issues no HTTP calls, holds no
/// Photo facts, and mirrors no history stack: the browser owns the stack and
/// each entry carries only constant-size restoration metadata under one
/// versioned `slipstream` namespace in history.state.

/// The three Library sources an address may name. Omission means `library`.
export type NavigationSourceKind = "library" | "folder" | "album";

/// An explicit view order. `album-order` is valid only for an Album and is
/// that source's default, so the encoder omits it.
export type NavigationViewOrder =
  | "capture-time-asc"
  | "capture-time-desc"
  | "album-order";

/// The Selection State filter an address may request. Omission means `all`.
export type NavigationSelectionFilter =
  | "all"
  | "undecided"
  | "selected"
  | "rejected";

/// One validated destination: a source reference, view order, Selection State
/// filter, and an optional stable Photo identity. Absence of `photoId` means
/// Grid.
export type NavigationDestination = Readonly<{
  source: NavigationSourceKind;
  folderPath?: string;
  albumId?: string;
  photoId?: string;
  order?: NavigationViewOrder;
  selection: NavigationSelectionFilter;
}>;

/// The recognized parameters in their canonical address order.
const PARAMETER_ORDER = [
  "source",
  "folderPath",
  "albumId",
  "photoId",
  "order",
  "selection",
] as const;

const SELECTION_FILTERS: ReadonlyArray<NavigationSelectionFilter> = [
  "all",
  "undecided",
  "selected",
  "rejected",
];

/// Mirrors the server's identifier validator: 36 to 64 bytes of lowercase
/// hexadecimal or `-`.
const validPhotoId = (value: string): boolean =>
  value.length >= 36 && value.length <= 64 && /^[0-9a-f-]+$/.test(value);

/// Mirrors the server's Folder Location validator: relative, component-valid,
/// bounded. An empty value is the Library Folder root.
const MAXIMUM_FOLDER_LOCATION_BYTES = 1024;
const MAXIMUM_FOLDER_COMPONENTS = 32;
const validFolderLocation = (value: string): boolean => {
  if (value.length === 0) return true;
  if (
    new TextEncoder().encode(value).length > MAXIMUM_FOLDER_LOCATION_BYTES ||
    value.startsWith("/")
  )
    return false;
  const components = value.split("/");
  return (
    components.length <= MAXIMUM_FOLDER_COMPONENTS &&
    components.every(
      (component) =>
        component.length > 0 &&
        component !== "." &&
        component !== ".." &&
        !component.includes("\0"),
    )
  );
};

const validOrder = (
  value: string,
  source: NavigationSourceKind,
): value is NavigationViewOrder =>
  value === "capture-time-asc" ||
  value === "capture-time-desc" ||
  (value === "album-order" && source === "album");

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

/// The address reading: either a validated destination or an invalid address.
/// Unknown query keys never make an address invalid; they are dropped on
/// canonicalization.
export type AddressDecoding =
  | Readonly<{ kind: "destination"; destination: NavigationDestination }>
  | Readonly<{ kind: "invalid" }>;

/// Decodes one query string with URL and URLSearchParams, so every value is
/// decoded exactly once. A duplicate recognized parameter, an empty value for
/// an optional parameter, a source-incompatible option, or a value the API
/// validators reject makes the whole address invalid.
export function decodeAddress(search: string): AddressDecoding {
  const params = new URLSearchParams(search);
  const present = new Map<string, string>();
  for (const key of PARAMETER_ORDER) {
    const found = params.getAll(key);
    // A repeated recognized parameter is ambiguous, so it is invalid rather
    // than first-wins.
    if (found.length > 1) return { kind: "invalid" };
    if (found.length === 1) present.set(key, found[0]!);
  }
  const source = present.get("source");
  if (source !== undefined && source.length === 0) return { kind: "invalid" };
  const kind: NavigationSourceKind | "invalid" =
    source === undefined
      ? "library"
      : source === "library" || source === "folder" || source === "album"
        ? source
        : "invalid";
  if (kind === "invalid") return { kind: "invalid" };
  const folderPath = present.get("folderPath");
  const albumId = present.get("albumId");
  if (folderPath !== undefined && albumId !== undefined)
    return { kind: "invalid" };
  if (kind === "folder") {
    // The Library Folder is an empty Folder Location, so the parameter is
    // required even when its value is empty.
    if (folderPath === undefined || !validFolderLocation(folderPath))
      return { kind: "invalid" };
  } else if (folderPath !== undefined) return { kind: "invalid" };
  if (kind === "album") {
    if (albumId === undefined || !validPhotoId(albumId))
      return { kind: "invalid" };
  } else if (albumId !== undefined) return { kind: "invalid" };
  const photoId = present.get("photoId");
  if (photoId !== undefined && !validPhotoId(photoId))
    return { kind: "invalid" };
  const order = present.get("order");
  if (order !== undefined && !validOrder(order, kind))
    return { kind: "invalid" };
  const selection = present.get("selection");
  if (
    selection !== undefined &&
    !SELECTION_FILTERS.includes(selection as NavigationSelectionFilter)
  )
    return { kind: "invalid" };
  const filter = (selection as NavigationSelectionFilter | undefined) ?? "all";
  const photo = photoId !== undefined ? { photoId } : {};
  const viewOrder = order !== undefined ? { order } : {};
  return {
    kind: "destination",
    destination:
      kind === "folder"
        ? {
            source: "folder",
            folderPath: folderPath!,
            ...photo,
            ...viewOrder,
            selection: filter,
          }
        : kind === "album"
          ? {
              source: "album",
              albumId: albumId!,
              ...photo,
              ...viewOrder,
              selection: filter,
            }
          : { source: "library", ...photo, ...viewOrder, selection: filter },
  };
}

/// True when an address omits the order its source already defaults to.
const omitsDefaultOrder = (destination: NavigationDestination): boolean =>
  destination.order === undefined ||
  (destination.source === "album" && destination.order === "album-order");

/// Encodes one destination, omitting defaults and emitting the recognized
/// parameters in canonical order. Folder Locations keep their encoded
/// separators, so an encoded slash round-trips as a separator and an encoded
/// reserved character round-trips without a second decode.
export function encodeAddress(destination: NavigationDestination): string {
  const params = new URLSearchParams();
  if (destination.source !== "library")
    params.set("source", destination.source);
  if (destination.source === "folder")
    params.set("folderPath", destination.folderPath ?? "");
  if (destination.source === "album")
    params.set("albumId", destination.albumId ?? "");
  if (destination.photoId) params.set("photoId", destination.photoId);
  if (!omitsDefaultOrder(destination)) params.set("order", destination.order!);
  if (destination.selection !== "all")
    params.set("selection", destination.selection);
  return params.toString();
}

/// The canonical address of one destination on the application path. Unknown
/// keys are dropped, because the encoder emits only recognized parameters.
export function addressFor(
  destination: NavigationDestination,
  path = "/",
): string {
  const query = encodeAddress(destination);
  return query ? `${path}?${query}` : path;
}

/// The All Photos Grid with default options: the bare application address.
export const allPhotosDestination: NavigationDestination = Object.freeze({
  source: "library",
  selection: "all",
});

/// The Grid destination of one source: no Photo identity, so the address
/// names the Grid rather than one Photo.
export const gridDestination = (
  destination: NavigationDestination,
): NavigationDestination => {
  const grid: NavigationDestination = {
    source: destination.source,
    ...(destination.source === "folder"
      ? { folderPath: destination.folderPath }
      : {}),
    ...(destination.source === "album" ? { albumId: destination.albumId } : {}),
    ...(destination.order ? { order: destination.order } : {}),
    selection: destination.selection,
  };
  return grid;
};

/// True when two destinations name the same source, order, and filter, so a
/// traversal can reuse the live Snapshot instead of reopening the source.
export const sameSourceView = (
  left: NavigationDestination,
  right: NavigationDestination,
): boolean =>
  left.source === right.source &&
  left.folderPath === right.folderPath &&
  left.albumId === right.albumId &&
  left.order === right.order &&
  left.selection === right.selection;

/// The namespace that holds one entry's bounded restoration metadata.
export const NAVIGATION_STATE_NAMESPACE = "slipstream";
export const NAVIGATION_STATE_VERSION = 1;

/// One Grid anchor: a stable Photo identity, the index hint it was seen at,
/// and its CSS-pixel offset inside its row.
export type NavigationGridAnchor = Readonly<{
  photoId: string;
  indexHint: number;
  offset: number;
}>;

/// What the Grid keyboard owned when the Photographer left the Grid.
export type NavigationFocusTarget =
  | Readonly<{ kind: "photo"; photoId: string }>
  | Readonly<{ kind: "grid" }>;

/// The restoration facts one Grid entry carries. Constant size: no Photo
/// facts, thumbnails, tokens, request objects, pending writes, multi-selection,
/// or Undo descriptions.
export type NavigationGridRestoration = Readonly<{
  anchor: NavigationGridAnchor;
  focus: NavigationFocusTarget;
}>;

/// One browser entry's metadata under the versioned namespace.
export type NavigationEntryState = Readonly<{
  version: number;
  entryId: string;
  anchor?: NavigationGridAnchor;
  focus?: NavigationFocusTarget;
  parentGridEntryId?: string;
  folderPublication?: string;
}>;

const MALFORMED = Symbol("malformed");

type FieldReading<T> = T | typeof MALFORMED | undefined;

const readAnchor = (value: unknown): FieldReading<NavigationGridAnchor> => {
  if (value === undefined) return undefined;
  if (!isRecord(value)) return MALFORMED;
  if (
    typeof value.photoId !== "string" ||
    !validPhotoId(value.photoId) ||
    !Number.isInteger(value.indexHint) ||
    Number(value.indexHint) < 0 ||
    !Number.isInteger(value.offset) ||
    Number(value.offset) < 0
  )
    return MALFORMED;
  return {
    photoId: value.photoId,
    indexHint: Number(value.indexHint),
    offset: Number(value.offset),
  };
};

const readFocus = (value: unknown): FieldReading<NavigationFocusTarget> => {
  if (value === undefined) return undefined;
  if (!isRecord(value)) return MALFORMED;
  if (value.kind === "grid") return { kind: "grid" };
  if (value.kind === "photo" && typeof value.photoId === "string")
    return { kind: "photo", photoId: value.photoId };
  return MALFORMED;
};

const readOptionalString = (value: unknown): FieldReading<string> => {
  if (value === undefined) return undefined;
  return typeof value === "string" && value.length > 0 ? value : MALFORMED;
};

/// Reads one entry's metadata. Absent, malformed, or unknown-version state is
/// reported as a direct entry rather than guessed at.
export function readEntryState(
  state: unknown,
): NavigationEntryState | undefined {
  if (!isRecord(state)) return undefined;
  const namespace = state[NAVIGATION_STATE_NAMESPACE];
  if (!isRecord(namespace)) return undefined;
  if (namespace.version !== NAVIGATION_STATE_VERSION) return undefined;
  const entryId = readOptionalString(namespace.entryId);
  const anchor = readAnchor(namespace.anchor);
  const focus = readFocus(namespace.focus);
  const parentGridEntryId = readOptionalString(namespace.parentGridEntryId);
  const folderPublication = readOptionalString(namespace.folderPublication);
  if (
    entryId === MALFORMED ||
    anchor === MALFORMED ||
    focus === MALFORMED ||
    parentGridEntryId === MALFORMED ||
    folderPublication === MALFORMED
  )
    return undefined;
  return {
    version: NAVIGATION_STATE_VERSION,
    entryId: entryId!,
    ...(anchor ? { anchor } : {}),
    ...(focus ? { focus } : {}),
    ...(parentGridEntryId ? { parentGridEntryId } : {}),
    ...(folderPublication ? { folderPublication } : {}),
  };
}

/// Merges one entry's metadata into the existing history state without
/// overwriting fields another owner may hold.
export function writeEntryState(
  existing: unknown,
  entry: NavigationEntryState,
): Record<string, unknown> {
  return {
    ...(isRecord(existing) ? existing : {}),
    [NAVIGATION_STATE_NAMESPACE]: entry,
  };
}

/// What a browser traversal asks the page controller to render.
export type NavigationTraversal = Readonly<{
  destination: NavigationDestination;
  entry: NavigationEntryState | undefined;
}>;

/// What startup reports after the current entry has been canonicalized.
export type NavigationStartup =
  | Readonly<{
      kind: "destination";
      destination: NavigationDestination;
      entry: NavigationEntryState;
    }>
  | Readonly<{ kind: "invalid" }>;

export interface NavigationOwnerHost {
  /// The Grid restoration facts the UI holds right now, or undefined when the
  /// Grid is not presenting a restorable position.
  captureGridRestoration(): NavigationGridRestoration | undefined;
}

export interface NavigationOwner {
  /// The destination the page currently presents.
  readonly destination: NavigationDestination;
  /// The identifier of the browser entry the page currently presents.
  readonly entryId: string;
  /// Canonicalizes the current entry with replaceState and reports the
  /// startup destination. It never pushes a duplicate entry.
  start(): NavigationStartup;
  /// Records a source selection as one new Grid entry.
  openGrid(
    destination: NavigationDestination,
    folderPublication?: string,
  ): void;
  /// Replaces the current entry's address, keeping its entry identity. Used
  /// for an applied filter or order change and for an explained fallback.
  replaceGrid(
    destination: NavigationDestination,
    restoration?: NavigationGridRestoration,
    folderPublication?: string,
  ): void;
  /// Records a Photo opened from the Grid as one new entry linked to the Grid
  /// entry that opened it.
  openPhoto(destination: NavigationDestination): void;
  /// Replaces the current Photo entry's address while preserving its parent
  /// relationship. Used for Previous, Next, neighbor navigation,
  /// decision-driven advancement, and Undo-driven return.
  replacePhoto(destination: NavigationDestination): void;
  /// The in-app source return: traverse one entry when the current Photo has
  /// a known parent Grid entry, otherwise replace the Photo entry with its
  /// source Grid. Never traverses from unknown history.
  returnToSourceGrid(
    destination: NavigationDestination,
  ): "traversed" | "replaced";
  /// Removes the popstate subscription and restores the previous scroll
  /// restoration setting.
  dispose(): void;
}

let entrySequence = 0;
const nextEntryId = (): string =>
  `e${Date.now().toString(36)}${(++entrySequence).toString(36)}${Math.random()
    .toString(36)
    .slice(2, 8)}`;

type CurrentEntry = Readonly<{
  destination: NavigationDestination;
  entryId: string;
  parentGridEntryId?: string;
  folderPublication?: string;
}>;

export function createNavigationOwner(
  host: NavigationOwnerHost,
  applyTraversal: (traversal: NavigationTraversal) => void,
): NavigationOwner {
  const history = window.history;
  const location = window.location;
  const path = location.pathname;
  let current: CurrentEntry = {
    destination: allPhotosDestination,
    entryId: nextEntryId(),
  };
  let previousScrollRestoration: ScrollRestoration = "auto";
  let disposed = false;

  const commit = (
    destination: NavigationDestination,
    entry: NavigationEntryState,
    mode: "push" | "replace",
  ): void => {
    if (disposed) return;
    const state = writeEntryState(history.state, entry);
    const address = addressFor(destination, path);
    if (mode === "push") history.pushState(state, "", address);
    else history.replaceState(state, "", address);
    current = {
      destination,
      entryId: entry.entryId,
      ...(entry.parentGridEntryId
        ? { parentGridEntryId: entry.parentGridEntryId }
        : {}),
      ...(entry.folderPublication
        ? { folderPublication: entry.folderPublication }
        : {}),
    };
  };

  const freshEntry = (
    parentGridEntryId?: string,
    folderPublication?: string,
  ): NavigationEntryState => ({
    version: NAVIGATION_STATE_VERSION,
    entryId: nextEntryId(),
    ...(parentGridEntryId ? { parentGridEntryId } : {}),
    ...(folderPublication ? { folderPublication } : {}),
  });

  const onPopState = () => {
    if (disposed) return;
    const entry = readEntryState(history.state);
    const decoding = decodeAddress(location.search);
    const destination =
      decoding.kind === "destination"
        ? decoding.destination
        : allPhotosDestination;
    current = {
      destination,
      entryId: entry?.entryId ?? nextEntryId(),
      ...(entry?.parentGridEntryId
        ? { parentGridEntryId: entry.parentGridEntryId }
        : {}),
      ...(entry?.folderPublication
        ? { folderPublication: entry.folderPublication }
        : {}),
    };
    applyTraversal({ destination, entry });
  };

  const owner: NavigationOwner = {
    get destination() {
      return current.destination;
    },
    get entryId() {
      return current.entryId;
    },
    start() {
      previousScrollRestoration = history.scrollRestoration;
      // The mounted Library Browser owns its internal Grid scroller, so the
      // browser must not also scroll the document.
      history.scrollRestoration = "manual";
      const decoding = decodeAddress(location.search);
      if (decoding.kind === "invalid") {
        // A known invalid address is explained and replaced once, before any
        // request for the invalid source is made.
        const entry = freshEntry();
        commit(allPhotosDestination, entry, "replace");
        return { kind: "invalid" };
      }
      const destination = decoding.destination;
      const existing = readEntryState(history.state);
      // Startup re-establishes the destination from its address. The entry
      // identifier and the Grid anchor survive a reload; the parent
      // relationship does not, because a reloaded document is a direct entry.
      const entry: NavigationEntryState = {
        version: NAVIGATION_STATE_VERSION,
        entryId: existing?.entryId ?? nextEntryId(),
        ...(existing?.anchor ? { anchor: existing.anchor } : {}),
        ...(existing?.focus ? { focus: existing.focus } : {}),
      };
      commit(destination, entry, "replace");
      return { kind: "destination", destination, entry };
    },
    openGrid(destination, folderPublication) {
      commit(destination, freshEntry(undefined, folderPublication), "push");
    },
    replaceGrid(destination, restoration, folderPublication) {
      const publication = folderPublication ?? current.folderPublication;
      commit(
        destination,
        {
          version: NAVIGATION_STATE_VERSION,
          entryId: current.entryId,
          ...(restoration?.anchor ? { anchor: restoration.anchor } : {}),
          ...(restoration?.focus ? { focus: restoration.focus } : {}),
          ...(publication ? { folderPublication: publication } : {}),
        },
        "replace",
      );
    },
    openPhoto(destination) {
      const restoration = host.captureGridRestoration();
      // The Grid entry keeps its anchor and focus, replaced in place, so a
      // later return restores where the Photographer left it.
      if (restoration) {
        commit(
          gridDestination(current.destination),
          {
            version: NAVIGATION_STATE_VERSION,
            entryId: current.entryId,
            anchor: restoration.anchor,
            focus: restoration.focus,
            ...(current.folderPublication
              ? { folderPublication: current.folderPublication }
              : {}),
          },
          "replace",
        );
      }
      commit(
        destination,
        freshEntry(
          current.entryId,
          destination.source === "folder"
            ? current.folderPublication
            : undefined,
        ),
        "push",
      );
    },
    replacePhoto(destination) {
      commit(
        destination,
        {
          version: NAVIGATION_STATE_VERSION,
          entryId: current.entryId,
          ...(current.parentGridEntryId
            ? { parentGridEntryId: current.parentGridEntryId }
            : {}),
          ...(current.folderPublication
            ? { folderPublication: current.folderPublication }
            : {}),
        },
        "replace",
      );
    },
    returnToSourceGrid(destination) {
      // The known parent relationship is evidence that the immediately
      // preceding entry is the Grid that opened this Photo. Without it the
      // destination is established by replacing this entry, so the browser's
      // own Back keeps its ordinary ability to leave the site.
      if (current.parentGridEntryId) {
        history.back();
        return "traversed";
      }
      commit(
        destination,
        {
          version: NAVIGATION_STATE_VERSION,
          entryId: current.entryId,
        },
        "replace",
      );
      return "replaced";
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      window.removeEventListener("popstate", onPopState);
      history.scrollRestoration = previousScrollRestoration;
    },
  };

  window.addEventListener("popstate", onPopState);
  return owner;
}
