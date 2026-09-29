import { describe, expect, test } from "bun:test";
import type { PhotoMetadataResponse } from "../api/contracts.js";
import { createPhotoDetailsOwner } from "./photo-details-owner.js";
import type { PhotoAuthority } from "./photo-owner.js";

const authority = (): PhotoAuthority => ({}) as PhotoAuthority;

const json = (body: unknown): Response => Response.json(body);

describe("Photo details owner", () => {
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
