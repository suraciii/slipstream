import type {
  AlbumSummary,
  LibraryOverviewResponse,
} from "../api/contracts.js";
import type { FileLocationController } from "./file-location-controller.js";
import type { LibraryBrowserView } from "../ui/library-browser-view.js";
import { folderPendingViewModel } from "./folder-pending-view.js";

type SurfaceInput = Readonly<{
  folders: Pick<FileLocationController, "tree" | "pending">;
  source: Readonly<{
    kind: string;
    folder?: Readonly<{ location: string }> | undefined;
    albumId?: string | undefined;
  }>;
  photoCount: () => number;
  albums: () => ReadonlyArray<AlbumSummary>;
  view: Pick<LibraryBrowserView, "renderSources" | "renderFolderPending">;
  albumManagement: () => Readonly<{
    setAlbums(albums: ReadonlyArray<AlbumSummary>): void;
  }>;
  gridMulti: () => Readonly<{ renderBatchAlbums(): void }>;
  photoActive: () => boolean;
  scan: () => LibraryOverviewResponse["scan"] | undefined;
  alive: () => boolean;
}>;

export const createSourceSurfaces = (input: SurfaceInput) => {
  const renderSources = (): void => {
    if (!input.alive()) return;
    const tree = input.folders.tree(
      (location) =>
        input.source.kind === "folder" &&
        input.source.folder?.location === location,
    );
    const albums = input.albums();
    input.view.renderSources({
      libraryCount: input.photoCount(),
      libraryActive: input.source.kind === "library",
      fileLocationsEnabled: tree.enabled,
      fileLocationFailures: tree.failures,
      rootExpanded: tree.rootExpanded,
      rootLoading: tree.rootLoading,
      rootActive: input.source.folder?.location === "",
      rootChildren: tree.rootChildren,
      rootPendingFiles: tree.rootPendingFiles,
      rootHasPendingDescendants: tree.rootHasPendingDescendants,
      ...(tree.rootPager ? { rootPager: tree.rootPager } : {}),
      albums: albums.map((album) => ({
        id: album.id,
        name: album.name,
        photoCount: album.photoCount,
        hasSavedPosition: album.hasSavedPosition,
        active:
          input.source.kind === "album" && input.source.albumId === album.id,
      })),
    });
    input.albumManagement().setAlbums(albums);
    input.gridMulti().renderBatchAlbums();
  };
  const renderFolderPending = (): void => {
    if (!input.alive()) return;
    const location =
      input.source.kind === "folder"
        ? input.source.folder?.location
        : undefined;
    input.view.renderFolderPending(
      folderPendingViewModel({
        location,
        photoActive: input.photoActive(),
        pending:
          location === undefined ? undefined : input.folders.pending(location),
        scan: input.scan(),
      }),
    );
  };
  return { renderSources, renderFolderPending };
};
