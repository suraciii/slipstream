import { expect, test } from "bun:test";
import {
  createGridMultiSelectionOwner,
  type GridMultiSelectionCoordination,
} from "./grid-multi-selection-owner.js";

function setup(waitForAdd?: Promise<void>) {
  const photos = ["a", "b", "c"].map((id) => ({
    id,
    selectionState: "unflagged",
  }));
  const authority = {};
  const requests: Array<{ albumId: string; photoIds: string[] }> = [];
  const focused: number[] = [];
  let busy = false;
  const sourceGrid = {
    authority,
    kind: "library",
    generation: 1,
    isCurrent: (value: object) => value === authority,
    findPhotoIndex: (id: string) => {
      const index = photos.findIndex((photo) => photo.id === id);
      return index < 0 ? undefined : index;
    },
    photoAt: (index: number) => photos[index],
    describeWindow: () => ({ start: 0 }),
    invalidateWindow: () => {},
    reconcilePhotoSelection: () => true,
  };
  const photoOwner = {
    mutateBatch: () => ({
      settlement: Promise.resolve({
        kind: "persisted",
        value: "picked",
        applied: [{ photoId: "a", priorValue: "unflagged" }],
        changedElsewhere: [{ photoId: "b", currentValue: "rejected" }],
        missing: [{ photoId: "c" }],
      }),
    }),
  };
  const albumActions = {
    addMemberships: (albumId: string, photoIds: string[]) => {
      requests.push({ albumId, photoIds: [...photoIds] });
      return {};
    },
  };
  const coordinate = {
    alive: () => true,
    connected: () => true,
    pageBusy: () => busy,
    setPageBusy: (value: boolean) => {
      busy = value;
    },
    gridVisible: () => true,
    canOpenGridPhoto: () => true,
    sourceGrid,
    photoOwner,
    albumActions,
    albums: () => [{ id: "album", name: "Picks", hasSavedPosition: false }],
    membershipAlbumName: () => "Picks",
    mutateAlbum: async (start: (context: object) => unknown) => {
      start({});
      await waitForAdd;
      return {
        admitted: true,
        latest: true,
        ok: true,
        membershipAdd: {
          albumId: "album",
          addedPhotoIds: ["a", "b"],
          alreadyMemberPhotoIds: [],
          albums: [],
        },
      };
    },
    loadWindow: () => Promise.resolve(true),
  } as unknown as GridMultiSelectionCoordination;
  const owner = createGridMultiSelectionOwner({
    coordinate,
    present: {
      renderGrid() {},
      renderBatchAlbums() {},
      resetGridMultiSelection() {},
      setDecisionStatus() {},
      focusGridIndex(index) {
        focused.push(index);
      },
      updateControls() {},
    },
  });
  return { owner, requests, focused };
}

test("Album addition counts missing captured Photos and retains changed review and scoped compensation", async () => {
  const { owner, requests, focused } = setup();
  for (const id of ["a", "b", "c"]) owner.toggle(id);
  await owner.mutate("picked");
  await owner.addToAlbum("album");
  expect(requests).toEqual([{ albumId: "album", photoIds: ["a", "b"] }]);
  expect(owner.model().count).toBe(3);
  expect(owner.model().result).toMatchObject({
    tone: "warning",
    review: { label: "Review 1 Photo" },
    compensation: { label: "Remove added Photos" },
  });
  expect(owner.model().result?.message).toContain(
    "1 Photo no longer in this Library",
  );
  expect(owner.model().result?.message).toContain("1 Photo changed elsewhere");
  await owner.reviewChanged();
  expect(focused).toEqual([1]);
  expect(owner.model().result?.compensation).toEqual({
    label: "Remove added Photos",
  });
});

test("an all-missing selection never admits an Album request", async () => {
  const { owner, requests } = setup();
  for (const id of ["a", "b", "c"]) owner.toggle(id);
  await owner.mutate("picked");
  owner.toggle("a");
  owner.toggle("b");
  await owner.addToAlbum("album");
  expect(requests).toEqual([]);
  expect(owner.model().count).toBe(1);
  expect(owner.model().result?.message).toContain(
    "1 Photo no longer in this Library",
  );
});

test("an admitted Album addition keeps its captured targets when selection changes", async () => {
  const gate = Promise.withResolvers<void>();
  const { owner, requests } = setup(gate.promise);
  for (const id of ["a", "b", "c"]) owner.toggle(id);
  await owner.mutate("picked");
  const addition = owner.addToAlbum("album");
  owner.toggle("b");
  owner.toggle("c");
  gate.resolve();
  await addition;
  expect(requests).toEqual([{ albumId: "album", photoIds: ["a", "b"] }]);
  expect(owner.model().result?.message).toContain(
    "1 Photo no longer in this Library",
  );
  expect(owner.model().result?.review).toBeUndefined();
});
