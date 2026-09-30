import type { AlbumSummary, SelectionFilter } from "../api/contracts.js";
import type { SourceViewOrder } from "../api/source-grid.js";
import type { LibraryBrowserView } from "../ui/library-browser-view.js";
import type { RecoveryGate } from "./async-ownership.js";
import {
  allPhotosDestination,
  gridDestination,
  type NavigationDestination,
  type NavigationGridRestoration,
  type NavigationOwner,
  type NavigationTraversal,
} from "./browser-navigation.js";
import {
  destinationOrder,
  folderNameFor,
  resolveRestorationIndex,
  restorationGeometry,
  reusableForTraversal,
} from "./destination-policy.js";
import type { FileLocationOwner } from "./file-location-owner.js";
import type {
  DestinationEstablishment,
  NavigationSession,
} from "./navigation-session.js";
import type { SourceAuthority, SourceGridOwner } from "./source-grid-owner.js";

export type SourceEstablishment = Readonly<{
  kind: "established" | "superseded" | "failed" | "missing";
}>;

export type SourceReestablishment =
  | Readonly<{ kind: "established"; authority: SourceAuthority }>
  | Readonly<{ kind: "superseded" }>
  | Readonly<{ kind: "failed" }>
  | Readonly<{ kind: "missing" }>;

export type SourceEstablishmentOptions = Readonly<{
  restoration?: NavigationGridRestoration;
  folderPublication?: string;
  explanation?: string;
  address?: "push" | "replace" | "none";
  intent?: DestinationEstablishment;
}>;

export type OpenSourceOptions = Readonly<{
  kind: "library" | "album" | "folder";
  album?: AlbumSummary;
  preferredPhotoId?: string;
  folder?: { location: string; name: string };
  order?: SourceViewOrder;
  selection?: SelectionFilter;
  establishment?: SourceEstablishmentOptions;
}>;

type EstablishOptions = SourceEstablishmentOptions &
  Readonly<{
    addressed?: boolean;
    reopen?: boolean;
  }>;

type Dependencies = Readonly<{
  source: Pick<
    SourceGridOwner,
    | "source"
    | "order"
    | "selection"
    | "authority"
    | "generation"
    | "token"
    | "total"
    | "name"
    | "kind"
    | "albumId"
    | "isCurrent"
    | "isReady"
    | "photoAt"
    | "findPhotoIndex"
    | "resolvePhotoPosition"
    | "readGridPosition"
  >;
  session: NavigationSession;
  navigation: Pick<NavigationOwner, "replaceGrid">;
  locations: Pick<FileLocationOwner, "publication" | "window">;
  view: Pick<
    LibraryBrowserView,
    | "prepareSourceOpen"
    | "setGridExplanation"
    | "showGrid"
    | "restoreGridAnchor"
    | "closeTransientSurfaces"
  >;
  gate: RecoveryGate;
  albums: () => ReadonlyArray<AlbumSummary>;
  canResume: () => boolean;
  openSource: (options: OpenSourceOptions) => Promise<SourceEstablishment>;
  reopen: (
    photoId: string,
    intent: DestinationEstablishment,
  ) => Promise<SourceReestablishment>;
  openPhoto: (
    index: number,
    address: "push" | "none",
    intent: DestinationEstablishment,
  ) => Promise<boolean>;
  leavePhoto: () => void;
  bindRoot: () => Promise<boolean>;
  loadWindow: (index: number, authority: SourceAuthority) => Promise<boolean>;
  renderGrid: (index: number) => void;
  presentRangeStatus: () => void;
  setGridStatus: (message: string) => void;
  syncConnection: () => void;
  updateControls: () => void;
}>;

export function createDestinationController(deps: Dependencies) {
  const { source, session, navigation, locations, view } = deps;
  let alive = true;
  const current = (intent: DestinationEstablishment): boolean =>
    alive && session.isCurrent(intent);
  const ownsSource = (
    intent: DestinationEstablishment,
    authority: SourceAuthority,
  ): boolean => current(intent) && source.isCurrent(authority);

  const retryable = (
    destination: NavigationDestination,
    intent: DestinationEstablishment,
  ): boolean => {
    if (!current(intent)) return false;
    session.fail(destination);
    deps.leavePhoto();
    view.prepareSourceOpen(source.name);
    deps.setGridStatus("Could not load this source. Retry to continue.");
    const generation = String(source.generation);
    const claim = deps.gate.issue("source-position", generation, {
      owner: { scope: "source", generation },
    });
    deps.gate.fail(claim, { transportLost: true });
    deps.syncConnection();
    deps.updateControls();
    return false;
  };

  const fallbackAll = async (
    message: string,
    intent: DestinationEstablishment,
  ): Promise<boolean> => {
    if (!current(intent)) return false;
    navigation.replaceGrid(allPhotosDestination);
    const outcome = await deps.openSource({
      kind: "library",
      establishment: { explanation: message, intent },
    });
    return current(intent) && outcome.kind === "established";
  };

  const fallbackGrid = async (
    destination: NavigationDestination,
    intent: DestinationEstablishment,
  ): Promise<boolean> => {
    if (!current(intent)) return false;
    const grid = gridDestination(destination);
    navigation.replaceGrid(
      grid,
      undefined,
      destination.source === "folder" ? locations.publication : undefined,
    );
    return establishIntent(
      grid,
      {
        explanation:
          "This Photo is no longer in this view. Showing the source Grid.",
      },
      intent,
    );
  };

  const resolvePhoto = async (
    destination: NavigationDestination,
    photoId: string,
    intent: DestinationEstablishment,
    allowReopen = true,
  ): Promise<boolean> => {
    const authority = source.authority;
    if (!ownsSource(intent, authority)) return false;
    const retained = source.findPhotoIndex(photoId);
    if (retained !== undefined && source.photoAt(retained)?.id === photoId)
      return deps.openPhoto(retained, "none", intent);
    const resolved = await source.resolvePhotoPosition(authority, photoId);
    if (!ownsSource(intent, authority)) return false;
    if (resolved.kind === "resolved")
      return deps.openPhoto(resolved.position, "none", intent);
    if (resolved.kind === "missing") return fallbackGrid(destination, intent);
    if (resolved.kind === "expired" && allowReopen) {
      const reopened = await deps.reopen(photoId, intent);
      if (!current(intent) || reopened.kind === "superseded") return false;
      if (reopened.kind === "missing")
        return fallbackAll(
          "This source is no longer available. Showing All Photos.",
          intent,
        );
      if (reopened.kind === "failed") return retryable(destination, intent);
      if (!ownsSource(intent, reopened.authority)) return false;
      return resolvePhoto(destination, photoId, intent, false);
    }
    return resolved.kind === "failed" || resolved.kind === "expired"
      ? retryable(destination, intent)
      : false;
  };

  const showGrid = async (
    options: EstablishOptions,
    intent: DestinationEstablishment,
  ): Promise<boolean> => {
    const authority = source.authority;
    const position = source.readGridPosition(authority);
    if (position === undefined || !ownsSource(intent, authority)) return false;
    const restoration = options.restoration;
    const index = restoration
      ? await resolveRestorationIndex(source, authority, position, restoration)
      : position;
    if (index === undefined || !ownsSource(intent, authority)) return false;
    deps.leavePhoto();
    view.showGrid();
    if (restoration) {
      const ready = await deps.loadWindow(index, authority);
      if (!ready || !ownsSource(intent, authority)) return false;
    }
    deps.renderGrid(index);
    if (restoration)
      view.restoreGridAnchor(restorationGeometry(source, index, restoration));
    deps.presentRangeStatus();
    if (options.explanation) deps.setGridStatus(options.explanation);
    deps.updateControls();
    return true;
  };

  const establishIntent = async (
    destination: NavigationDestination,
    options: EstablishOptions,
    intent: DestinationEstablishment,
  ): Promise<boolean> => {
    if (!current(intent)) return false;
    if (
      !options.reopen &&
      reusableForTraversal(
        source,
        locations.publication,
        destination,
        options.folderPublication,
      )
    ) {
      return destination.photoId
        ? resolvePhoto(destination, destination.photoId, intent)
        : showGrid(options, intent);
    }
    if (
      destination.source === "folder" &&
      options.folderPublication !== undefined &&
      locations.publication !== undefined &&
      options.folderPublication !== locations.publication
    ) {
      session.requireCurrentFolder(destination.folderPath ?? "");
      deps.leavePhoto();
      view.prepareSourceOpen(source.name);
      view.setGridExplanation(
        "This Folder changed with a newer Library publication. Open the current Folder to browse it.",
        { label: "Open current Folder" },
      );
      return false;
    }
    let album: AlbumSummary | undefined;
    let folder: OpenSourceOptions["folder"];
    let missing = "This source is no longer available.";
    if (destination.source === "album") {
      album = deps
        .albums()
        .find((candidate) => candidate.id === destination.albumId);
      if (!album)
        return fallbackAll("This Album is no longer available.", intent);
      missing = "This Album is no longer available.";
    } else if (destination.source === "folder") {
      if (!locations.publication) {
        const bound = await deps.bindRoot();
        if (!current(intent)) return false;
        if (!bound) return retryable(destination, intent);
      }
      const location = destination.folderPath ?? "";
      folder = { location, name: folderNameFor(locations, location) };
      missing =
        "This Folder is no longer part of the Library. Showing All Photos.";
    }
    const outcome = await deps.openSource({
      kind: destination.source,
      ...(album ? { album } : {}),
      ...(folder ? { folder } : {}),
      ...(destination.photoId ? { preferredPhotoId: destination.photoId } : {}),
      order: destinationOrder(destination),
      selection: destination.selection,
      establishment: { ...options, intent },
    });
    if (!current(intent)) return false;
    if (outcome.kind === "missing") return fallbackAll(missing, intent);
    if (outcome.kind !== "established") return false;
    return destination.photoId
      ? resolvePhoto(destination, destination.photoId, intent)
      : true;
  };

  const establish = (
    destination: NavigationDestination,
    options: EstablishOptions = {},
  ): Promise<boolean> => {
    if (!alive) return Promise.resolve(false);
    const intent = session.begin(destination);
    return establishIntent(destination, options, intent);
  };

  return {
    establish,
    async applyTraversal(
      this: void,
      traversal: NavigationTraversal,
    ): Promise<void> {
      if (!alive) return;
      view.closeTransientSurfaces();
      const entry = traversal.entry;
      await establish(traversal.destination, {
        addressed: true,
        ...(entry?.anchor && entry.focus
          ? { restoration: { anchor: entry.anchor, focus: entry.focus } }
          : {}),
        ...(entry?.folderPublication
          ? { folderPublication: entry.folderPublication }
          : {}),
      });
    },
    async resumeAlbum(this: void, albumId: string): Promise<void> {
      if (!alive || !deps.canResume()) return;
      const album = deps.albums().find((candidate) => candidate.id === albumId);
      if (!album) return;
      const intent = session.begin({
        source: "album",
        albumId,
        selection: "all",
      });
      const alreadyCurrent =
        source.kind === "album" &&
        source.albumId === albumId &&
        source.isReady(source.authority);
      const outcome = await deps.openSource({
        kind: "album",
        album,
        establishment: { address: alreadyCurrent ? "replace" : "push", intent },
      });
      if (!current(intent) || outcome.kind !== "established") return;
      const position = source.readGridPosition(source.authority);
      if (position === undefined || !source.photoAt(position)) {
        deps.setGridStatus(
          "Resume is unavailable: this Album has no saved position.",
        );
        return;
      }
      await deps.openPhoto(position, "push", intent);
    },
    async openCurrentFolder(this: void): Promise<void> {
      if (!alive) return;
      const destination: NavigationDestination = {
        source: "folder",
        folderPath: session.takeCurrentFolder() ?? "",
        selection: "all",
      };
      navigation.replaceGrid(destination, undefined, locations.publication);
      await establish(destination, { addressed: true, reopen: true });
    },
    dispose(): void {
      alive = false;
    },
  };
}
