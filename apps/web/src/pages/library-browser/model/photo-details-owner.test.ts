import { describe, expect, test } from "bun:test";
import type { PhotoMetadataResponse } from "../api/contracts.js";
import { createPhotoDetailsOwner } from "./photo-details-owner.js";
import type { PhotoAuthority } from "./photo-owner.js";

const authority = (): PhotoAuthority => ({}) as PhotoAuthority;

const json = (body: unknown): Response => Response.json(body);

describe("Photo details owner", () => {
  test("unknown membership offers no false choices and rejects toggles until Retry confirms facts", async () => {
    const current = authority();
    const album = {
      id: "album",
      name: "Picks",
      photoCount: 1,
      hasSavedPosition: false,
    };
    const presented: Array<{
      loading: boolean;
      failed: boolean;
      options: ReadonlyArray<{ id: string; member: boolean }>;
    }> = [];
    let succeed = false;
    let mutations = 0;
    const owner = createPhotoDetailsOwner(
      () =>
        Promise.resolve(
          succeed
            ? json({ albums: [{ id: album.id, name: album.name }] })
            : new Response("{}", { status: 503 }),
        ),
      {
        isAlive: () => true,
        currentPhoto: () => ({ id: "photo" }),
        isCurrent: (candidate) => candidate === current,
        authority: () => current,
        albums: () => [album],
        isMembershipAdmitted: () => false,
        mutateAlbum: () => {
          mutations++;
          return Promise.resolve({ ok: false, announce: () => {} });
        },
        addMembership: () => undefined,
        removeMembership: () => undefined,
        sourceAlbumId: () => undefined,
        renderMembership: (model) => presented.push(model),
        renderMetadata: () => {},
      },
    );
    owner.renderMembership();
    expect(presented.at(-1)).toMatchObject({ loading: true, options: [] });
    owner.toggleMembership(album.id, true, current);
    await owner.loadAlbums(current, "photo");
    expect(presented.at(-1)).toMatchObject({ failed: true, options: [] });
    owner.toggleMembership(album.id, false, current);
    expect(mutations).toBe(0);
    succeed = true;
    await owner.loadAlbums(current, "photo", { force: true });
    expect(presented.at(-1)).toMatchObject({
      loading: false,
      failed: false,
      options: [{ id: album.id, member: true }],
    });
    succeed = false;
    await owner.loadAlbums(current, "photo", { force: true, keepFacts: true });
    expect(presented.at(-1)).toMatchObject({ failed: true, options: [] });
    owner.dispose();
  });

  test("a late membership read cannot replace a newer Photo's Albums", async () => {
    let settleFirst!: (response: Response) => void;
    let photoId = "first";
    let current = authority();
    const presented: string[][] = [];
    const owner = createPhotoDetailsOwner(
      (path) =>
        typeof path === "string" && path.includes("first")
          ? new Promise<Response>((resolve) => {
              settleFirst = resolve;
            })
          : Promise.resolve(
              json({ albums: [{ id: "second-album", name: "New" }] }),
            ),
      {
        isAlive: () => true,
        currentPhoto: () => ({ id: photoId }),
        isCurrent: (candidate) => candidate === current,
        authority: () => current,
        albums: () => [],
        isMembershipAdmitted: () => false,
        mutateAlbum: () => Promise.resolve({ ok: false, announce: () => {} }),
        addMembership: () => undefined,
        removeMembership: () => undefined,
        sourceAlbumId: () => undefined,
        renderMembership: (model) =>
          presented.push(model.containing.map((album) => album.id)),
        renderMetadata: () => {},
      },
    );
    const oldRead = owner.loadAlbums(current, photoId);
    photoId = "second";
    current = authority();
    await owner.loadAlbums(current, photoId);
    settleFirst(json({ albums: [{ id: "first-album", name: "Old" }] }));
    await oldRead;
    expect(presented.at(-1)).toEqual(["second-album"]);
    owner.dispose();
  });

  test("a superseded metadata read cannot paint over the current Photo", async () => {
    let settleFirst!: (response: Response) => void;
    let photoId = "first";
    let current = authority();
    const presented: Array<PhotoMetadataResponse | undefined> = [];
    const owner = createPhotoDetailsOwner(
      (path) =>
        typeof path === "string" && path.includes("first")
          ? new Promise<Response>((resolve) => {
              settleFirst = resolve;
            })
          : Promise.resolve(json({ iso: 200 })),
      {
        isAlive: () => true,
        currentPhoto: () => ({ id: photoId }),
        isCurrent: (candidate) => candidate === current,
        authority: () => current,
        albums: () => [],
        isMembershipAdmitted: () => false,
        mutateAlbum: () => Promise.resolve({ ok: false, announce: () => {} }),
        addMembership: () => undefined,
        removeMembership: () => undefined,
        sourceAlbumId: () => undefined,
        renderMembership: () => {},
        renderMetadata: (metadata) => presented.push(metadata),
      },
    );
    const oldRead = owner.loadMetadata(current, photoId);
    photoId = "second";
    current = authority();
    await owner.loadMetadata(current, photoId);
    settleFirst(json({ iso: 100 }));
    await oldRead;
    expect(presented.at(-1)).toEqual({ iso: 200 });
    owner.dispose();
  });
});
