/// Destination policy: how one Library Browser address maps onto the live
/// source and back, and which captured Grid restoration still applies.
///
/// This is page-local policy, not an owner. It retains no state and issues no
/// HTTP; it reads the shape of the open source, the open Snapshot's readiness,
/// and the File Location binding through the narrow queries below. The page
/// controller remains the only coordinator that commits what this module
/// answers.
import {
  sameSourceView,
  type NavigationDestination,
  type NavigationGridRestoration,
} from "./browser-navigation.js";
import type { FileLocationWindow } from "./file-location-owner.js";
import type {
  SourceAuthority,
  SourceGridSource,
  SourcePositionOutcome,
} from "./source-grid-owner.js";
import type { SelectionFilter } from "../api/contracts.js";
import type { SourceViewOrder } from "../api/source-grid.js";

/// The open source's identity as an address names it.
export interface LiveSourceView {
  readonly source: SourceGridSource;
  readonly order: SourceViewOrder;
  readonly selection: SelectionFilter;
}

/// What the traversal reuse rule reads: the open source's identity plus the
/// open Snapshot's token and readiness.
export interface SnapshotFacts extends LiveSourceView {
  readonly token: string;
  readonly authority: SourceAuthority;
  isReady(authority: SourceAuthority): boolean;
}

/// What one restoration resolution reads from the live Snapshot.
export interface RestorationSource {
  readonly total: number;
  isCurrent(authority: SourceAuthority): boolean;
  photoAt(index: number): Readonly<{ id: string }> | undefined;
  findPhotoIndex(photoId: string): number | undefined;
  resolvePhotoPosition(
    authority: SourceAuthority,
    photoId: string,
  ): Promise<SourcePositionOutcome>;
}

/// The destination the live Snapshot presents, derived from the committed
/// source, order, and filter. An address is never derived from a retained
/// projection of Photo facts.
export const liveDestination = (
  live: LiveSourceView,
  photoId?: string,
): NavigationDestination => {
  const source = live.source;
  const order = live.order === "source-default" ? undefined : live.order;
  return {
    source: source.kind,
    ...(source.kind === "folder" ? { folderPath: source.folder.location } : {}),
    ...(source.kind === "album" ? { albumId: source.album.id } : {}),
    ...(photoId ? { photoId } : {}),
    ...(order ? { order } : {}),
    selection: live.selection,
  };
};

/// The wire order one address requests. An omitted order and the Album's own
/// order both leave the order to the server.
export const destinationOrder = (
  destination: NavigationDestination,
): SourceViewOrder =>
  destination.order === undefined || destination.order === "album-order"
    ? "source-default"
    : destination.order;

/// True when the live Snapshot can serve a destination without reopening the
/// source: the same source, order, and filter, and a Folder whose publication
/// still matches the entry's provenance.
export const reusableForTraversal = (
  snapshot: SnapshotFacts,
  publication: string | undefined,
  destination: NavigationDestination,
  folderPublication?: string,
): boolean => {
  if (!snapshot.token || !snapshot.isReady(snapshot.authority)) return false;
  if (!sameSourceView(liveDestination(snapshot), destination)) return false;
  return !(
    destination.source === "folder" &&
    folderPublication !== undefined &&
    folderPublication !== publication
  );
};

/// The display name of a Folder Location. The File Location tree names the
/// Folders it has loaded; a Location outside a loaded window falls back to its
/// last component, and the server answers authoritatively.
export const folderNameFor = (
  fileLocations: Readonly<{
    window(parent: string): FileLocationWindow | undefined;
  }>,
  location: string,
): string => {
  if (location === "") return "Library Folder";
  const separator = location.lastIndexOf("/");
  const parent = separator === -1 ? "" : location.slice(0, separator);
  const known = fileLocations
    .window(parent)
    ?.children.find((child) => child.location === location);
  return known?.name ?? location.slice(separator + 1);
};

/// Resolves one captured Grid anchor against the established Snapshot. The
/// stable identity is confirmed before the index hint is trusted; an absent
/// anchor clamps the prior index hint to the current source. Undefined means
/// a newer destination superseded this one.
export const resolveRestorationIndex = async (
  source: RestorationSource,
  authority: SourceAuthority,
  fallbackIndex: number,
  restoration: NavigationGridRestoration,
): Promise<number | undefined> => {
  if (!source.isCurrent(authority)) return undefined;
  const anchor = restoration.anchor;
  const retained = source.findPhotoIndex(anchor.photoId);
  if (retained !== undefined && source.photoAt(retained)?.id === anchor.photoId)
    return retained;
  const resolved = await source.resolvePhotoPosition(authority, anchor.photoId);
  if (!source.isCurrent(authority)) return undefined;
  if (resolved.kind === "resolved") return resolved.position;
  if (resolved.kind === "missing")
    return Math.min(anchor.indexHint, Math.max(0, source.total - 1));
  // A failed lookup is retryable, not evidence that the Photo disappeared,
  // so the bounded position the open already resolved stays usable.
  return fallbackIndex;
};

/// The geometry one restoration applies at a resolved index: the anchor's row
/// and offset, and the cell the focus target names when the Snapshot still
/// holds that Photo identity. An anchor the Snapshot no longer holds falls
/// back to the clamped position, which restores geometry but leaves the Grid
/// itself as the focus target.
export const restorationGeometry = (
  source: RestorationSource,
  index: number,
  restoration: NavigationGridRestoration,
): Readonly<{ index: number; offset: number; focusIndex?: number }> => {
  const anchorHeld = source.photoAt(index)?.id === restoration.anchor.photoId;
  const focusIndex =
    anchorHeld && restoration.focus.kind === "photo"
      ? source.findPhotoIndex(restoration.focus.photoId)
      : undefined;
  return {
    index,
    offset: restoration.anchor.offset,
    ...(focusIndex !== undefined ? { focusIndex } : {}),
  };
};
