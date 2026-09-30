import type { AlbumSummary, PhotoMetadataResponse } from "../api/contracts.js";
import { fetchPhotoAlbums, fetchPhotoMetadata } from "../api/photo.js";
import type { BrowserFetch } from "./access-session.js";
import type { PhotoAuthority } from "./photo-owner.js";
import type {
  AlbumActionContext,
  AlbumActionAdmission,
} from "./album-action-owner.js";
type MembershipViewModel = Readonly<{
  photoPresent: boolean;
  loading: boolean;
  failed: boolean;
  message?: string;
  containing: ReadonlyArray<Readonly<{ id: string; name: string }>>;
  options: ReadonlyArray<
    Readonly<{ id: string; name: string; member: boolean }>
  >;
  pendingAlbumIds: ReadonlyArray<string>;
}>;

type MembershipFacts =
  | Readonly<{ kind: "loading" }>
  | Readonly<{
      kind: "ready";
      albums: ReadonlyArray<Readonly<{ id: string; name: string }>>;
    }>
  | Readonly<{ kind: "failed" }>;

type PhotoDetailsDeps = Readonly<{
  isAlive: () => boolean;
  currentPhoto: () => Readonly<{ id: string }> | undefined;
  isCurrent: (authority: PhotoAuthority) => boolean;
  authority: () => PhotoAuthority;
  albums: () => ReadonlyArray<AlbumSummary>;
  isMembershipAdmitted: (
    kind: "add" | "remove",
    albumId: string,
    photoId: string,
  ) => boolean;
  mutateAlbum: (
    start: (context: AlbumActionContext) => AlbumActionAdmission | undefined,
    authority: PhotoAuthority,
  ) => Promise<{ ok: boolean; announce: (text: string) => void }>;
  addMembership: (
    albumId: string,
    photoId: string,
    context: AlbumActionContext,
  ) => AlbumActionAdmission | undefined;
  removeMembership: (
    albumId: string,
    photoId: string,
    context: AlbumActionContext,
  ) => AlbumActionAdmission | undefined;
  sourceAlbumId: () => string | undefined;
  renderMembership: (model: MembershipViewModel) => void;
  renderMetadata: (metadata?: PhotoMetadataResponse) => void;
}>;

export function createPhotoDetailsOwner(
  fetcher: BrowserFetch,
  deps: PhotoDetailsDeps,
) {
  let membershipFacts: MembershipFacts = { kind: "loading" };
  let membershipPhotoId: string | undefined;
  let membershipMessage: string | undefined;
  let membershipAbort: AbortController | undefined;
  let membershipRevision = 0;
  let metadataAbort: AbortController | undefined;

  const albumName = (id: string): string =>
    deps.albums().find((album) => album.id === id)?.name ?? "Album";

  const renderMembership = (): void => {
    if (!deps.isAlive()) return;
    const photo = deps.currentPhoto();
    const photoId = photo?.id;
    const facts =
      photoId !== undefined && membershipPhotoId === photoId
        ? membershipFacts
        : { kind: "loading" as const };
    const containing = facts.kind === "ready" ? facts.albums : [];
    const memberIds = new Set(containing.map((album) => album.id));
    const pending = photoId
      ? deps
          .albums()
          .filter(
            (album) =>
              deps.isMembershipAdmitted("add", album.id, photoId) ||
              deps.isMembershipAdmitted("remove", album.id, photoId),
          )
          .map((album) => album.id)
      : [];
    deps.renderMembership({
      photoPresent: Boolean(photo),
      loading: Boolean(photo) && facts.kind === "loading",
      failed: Boolean(photo) && facts.kind === "failed",
      ...(membershipMessage ? { message: membershipMessage } : {}),
      containing,
      options: deps.albums().map((album) => ({
        id: album.id,
        name: album.name,
        member: memberIds.has(album.id),
      })),
      pendingAlbumIds: pending,
    });
  };

  const loadAlbums = async (
    authority: PhotoAuthority,
    photoId: string | undefined,
    options: Readonly<{ force?: boolean; keepFacts?: boolean }> = {},
  ): Promise<void> => {
    if (
      !options.force &&
      photoId !== undefined &&
      photoId === membershipPhotoId &&
      membershipFacts.kind !== "failed"
    )
      return;
    const showLoading = !options.keepFacts || photoId !== membershipPhotoId;
    membershipAbort?.abort();
    membershipAbort = undefined;
    membershipPhotoId = photoId;
    membershipMessage = undefined;
    const revision = ++membershipRevision;
    if (!photoId) {
      membershipFacts = { kind: "loading" };
      renderMembership();
      return;
    }
    if (showLoading) {
      membershipFacts = { kind: "loading" };
      renderMembership();
    }
    const controller = new AbortController();
    membershipAbort = controller;
    const result = await fetchPhotoAlbums(fetcher, photoId, controller.signal);
    if (
      controller.signal.aborted ||
      membershipRevision !== revision ||
      !deps.isCurrent(authority) ||
      deps.currentPhoto()?.id !== photoId
    )
      return;
    membershipFacts =
      result.kind === "ok"
        ? { kind: "ready", albums: result.value.albums }
        : { kind: "failed" };
    renderMembership();
  };

  const refreshAlbums = (): void => {
    if (!deps.isAlive()) return;
    const photo = deps.currentPhoto();
    if (photo)
      void loadAlbums(deps.authority(), photo.id, {
        force: true,
        keepFacts: true,
      });
  };

  const toggleMembership = (
    albumId: string,
    member: boolean,
    authority: PhotoAuthority,
  ): void => {
    const photo = deps.currentPhoto();
    if (
      !photo ||
      !albumId ||
      deps.isMembershipAdmitted(member ? "add" : "remove", albumId, photo.id)
    )
      return;
    const photoId = photo.id;
    const revision = ++membershipRevision;
    const prior = membershipFacts;
    if (prior.kind === "ready") {
      const others = prior.albums.filter((album) => album.id !== albumId);
      membershipFacts = {
        kind: "ready",
        albums: member
          ? [...others, { id: albumId, name: albumName(albumId) }]
          : others,
      };
    }
    membershipMessage = undefined;
    renderMembership();
    void (async () => {
      const settlement = member
        ? deps.mutateAlbum(
            (context) => deps.addMembership(albumId, photoId, context),
            authority,
          )
        : deps.mutateAlbum(
            (context) => deps.removeMembership(albumId, photoId, context),
            authority,
          );
      renderMembership();
      const { ok, announce } = await settlement;
      const stillCurrent =
        deps.isCurrent(authority) && deps.currentPhoto()?.id === photoId;
      if (ok) {
        if (stillCurrent) {
          announce(
            member
              ? "Added to the Album."
              : albumId === deps.sourceAlbumId()
                ? "Removed from the Album. It stays in this open view until reopened."
                : "Removed from the Album.",
          );
          void loadAlbums(authority, photoId, { force: true, keepFacts: true });
        }
        return;
      }
      if (stillCurrent && membershipRevision === revision)
        membershipFacts = prior.kind === "ready" ? prior : { kind: "failed" };
      if (!stillCurrent) return;
      membershipMessage = member
        ? `Could not add this Photo to “${albumName(albumId)}”.`
        : `Could not remove this Photo from “${albumName(albumId)}”.`;
      renderMembership();
    })();
  };

  const clearMetadata = (): void => {
    metadataAbort?.abort();
    metadataAbort = undefined;
    deps.renderMetadata();
  };

  const loadMetadata = async (
    authority: PhotoAuthority,
    photoId: string | undefined,
  ): Promise<void> => {
    clearMetadata();
    if (!photoId) return;
    const controller = new AbortController();
    metadataAbort = controller;
    const result = await fetchPhotoMetadata(
      fetcher,
      photoId,
      controller.signal,
    );
    if (
      controller.signal.aborted ||
      !deps.isCurrent(authority) ||
      deps.currentPhoto()?.id !== photoId
    )
      return;
    deps.renderMetadata(result.kind === "ok" ? result.value : undefined);
  };

  return {
    loadAlbums,
    refreshAlbums,
    renderMembership,
    toggleMembership,
    loadMetadata,
    clearMetadata,
    dispose: () => {
      membershipAbort?.abort();
      metadataAbort?.abort();
    },
  };
}
