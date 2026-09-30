import { describe, expect, test } from "bun:test";
import {
  destinationOrder,
  folderNameFor,
  liveDestination,
  resolveRestorationIndex,
  restorationGeometry,
  reusableForTraversal,
  type RestorationSource,
  type SnapshotFacts,
} from "./destination-policy.js";
import type {
  NavigationDestination,
  NavigationGridRestoration,
} from "./browser-navigation.js";
import type { FileLocationWindow } from "./file-location-owner.js";
import type {
  SourceAuthority,
  SourcePositionOutcome,
} from "./source-grid-owner.js";

const authority = (): SourceAuthority => ({}) as SourceAuthority;

const destination = (
  values: Partial<NavigationDestination>,
): NavigationDestination => ({
  source: "library",
  selection: "all",
  ...values,
});

const photoId = (letter: string): string => letter.repeat(36);

/// One Snapshot and its restoration reads over a fixed list of Photo
/// identities.
const snapshot = (
  ids: string[],
): Readonly<{
  source: RestorationSource & SnapshotFacts;
  authority: SourceAuthority;
}> => {
  const held = authority();
  return {
    authority: held,
    source: {
      source: { kind: "library" },
      order: "source-default",
      selection: "all",
      token: "token",
      authority: held,
      total: ids.length,
      isReady: () => true,
      isCurrent: (candidate) => candidate === held,
      photoAt: (index) => {
        const id = ids[index];
        return id === undefined ? undefined : { id };
      },
      findPhotoIndex: (candidate) => {
        const index = ids.indexOf(candidate);
        return index === -1 ? undefined : index;
      },
      resolvePhotoPosition: (_, candidate) =>
        Promise.resolve<SourcePositionOutcome>({
          kind: "resolved",
          authority: held,
          photoId: candidate,
          position: 0,
        }),
    },
  };
};

const restoration = (
  anchor: Readonly<{ photoId: string; indexHint: number; offset: number }>,
  focus: NavigationGridRestoration["focus"],
): NavigationGridRestoration => ({ anchor, focus });

describe("live destination", () => {
  test("an address names the open source, order, filter, and optional Photo", () => {
    expect(
      liveDestination({
        source: { kind: "library" },
        order: "source-default",
        selection: "all",
      }),
    ).toEqual({ source: "library", selection: "all" });
    expect(
      liveDestination(
        {
          source: { kind: "album", album: { id: photoId("a"), name: "Picks" } },
          order: "capture-time-desc",
          selection: "rejected",
        },
        photoId("p"),
      ),
    ).toEqual({
      source: "album",
      albumId: photoId("a"),
      photoId: photoId("p"),
      order: "capture-time-desc",
      selection: "rejected",
    });
    expect(
      liveDestination({
        source: {
          kind: "folder",
          folder: { location: "shoot", name: "Shoot" },
          publication: "pub",
        },
        order: "source-default",
        selection: "undecided",
      }),
    ).toEqual({
      source: "folder",
      folderPath: "shoot",
      selection: "undecided",
    });
  });

  test("an explicit order is kept", () => {
    expect(
      liveDestination({
        source: { kind: "library" },
        order: "capture-time-asc",
        selection: "all",
      }).order,
    ).toBe("capture-time-asc");
  });
});

describe("destination order", () => {
  test("an omitted order and the Album's own order leave the order to the server", () => {
    expect(destinationOrder(destination({}))).toBe("source-default");
    expect(
      destinationOrder(destination({ source: "album", albumId: photoId("a") })),
    ).toBe("source-default");
    expect(destinationOrder(destination({ order: "album-order" }))).toBe(
      "source-default",
    );
    expect(destinationOrder(destination({ order: "capture-time-desc" }))).toBe(
      "capture-time-desc",
    );
  });
});

describe("traversal reuse", () => {
  test("only a ready Snapshot of the same source, order, and filter is reusable", () => {
    const { source } = snapshot(["a", "b"]);
    expect(reusableForTraversal(source, undefined, destination({}))).toBe(true);
    expect(
      reusableForTraversal(
        source,
        undefined,
        destination({ selection: "rejected" }),
      ),
    ).toBe(false);
    expect(
      reusableForTraversal(
        source,
        undefined,
        destination({ order: "capture-time-asc" }),
      ),
    ).toBe(false);
    expect(
      reusableForTraversal(
        source,
        undefined,
        destination({ source: "album", albumId: photoId("a") }),
      ),
    ).toBe(false);
    expect(
      reusableForTraversal(
        { ...source, token: "" },
        undefined,
        destination({}),
      ),
    ).toBe(false);
    expect(
      reusableForTraversal(
        { ...source, isReady: () => false },
        undefined,
        destination({}),
      ),
    ).toBe(false);
  });

  test("a Folder entry is reusable only under its own publication", () => {
    const { source } = snapshot(["a"]);
    const folder = destination({ source: "folder", folderPath: "shoot" });
    const folderSnapshot: SnapshotFacts = {
      ...source,
      source: {
        kind: "folder",
        folder: { location: "shoot", name: "Shoot" },
        publication: "pub",
      },
    };
    expect(reusableForTraversal(folderSnapshot, "pub", folder, "pub")).toBe(
      true,
    );
    expect(reusableForTraversal(folderSnapshot, "pub", folder, "old")).toBe(
      false,
    );
    expect(reusableForTraversal(folderSnapshot, "pub", folder)).toBe(true);
  });
});

describe("folder name", () => {
  test("a loaded window names the location and an unloaded one falls back", () => {
    const loaded = (
      location: string,
      name: string,
    ): FileLocationWindow["children"][number] => ({
      location,
      name,
      photoCount: 1,
      hasDescendantFolders: false,
    });
    const windows = {
      window: (parent: string) =>
        parent === "shoot"
          ? {
              page: 0,
              total: 2,
              children: [
                loaded("shoot/roll-01", "Roll 01"),
                loaded("shoot/roll-02", "Roll 02"),
              ],
            }
          : undefined,
    };
    expect(folderNameFor(windows, "shoot/roll-01")).toBe("Roll 01");
    expect(folderNameFor(windows, "shoot/roll-09")).toBe("roll-09");
    expect(folderNameFor(windows, "loose")).toBe("loose");
    expect(folderNameFor(windows, "")).toBe("Library Folder");
    expect(folderNameFor({ window: () => undefined }, "shoot/roll-01")).toBe(
      "roll-01",
    );
  });
});

describe("Grid restoration resolution", () => {
  test("a retained anchor resolves to its retained index without a lookup", async () => {
    const { source } = snapshot(["a", "b", "c"]);
    expect(
      await resolveRestorationIndex(
        source,
        source.authority,
        0,
        restoration(
          { photoId: "c", indexHint: 0, offset: 0 },
          { kind: "grid" },
        ),
      ),
    ).toBe(2);
  });

  test("an evicted anchor is resolved by stable identity", async () => {
    const { source, authority: live } = snapshot(["a"]);
    const resolved: SourcePositionOutcome = {
      kind: "resolved",
      authority: live,
      photoId: "z",
      position: 4,
    };
    const restoring: RestorationSource = {
      ...source,
      resolvePhotoPosition: () => Promise.resolve(resolved),
    };
    expect(
      await resolveRestorationIndex(
        restoring,
        live,
        1,
        restoration(
          { photoId: "z", indexHint: 1, offset: 0 },
          { kind: "grid" },
        ),
      ),
    ).toBe(4);
  });

  test("a missing Photo clamps the captured index hint into the source", async () => {
    const { source, authority: live } = snapshot(["a", "b"]);
    const restoring: RestorationSource = {
      ...source,
      resolvePhotoPosition: () =>
        Promise.resolve<SourcePositionOutcome>({
          kind: "missing",
          authority: live,
          photoId: "z",
        }),
    };
    const anchor = { photoId: "z", indexHint: 9, offset: 0 };
    expect(
      await resolveRestorationIndex(
        restoring,
        live,
        0,
        restoration(anchor, { kind: "grid" }),
      ),
    ).toBe(1);
    const empty = { ...restoring, total: 0 };
    expect(
      await resolveRestorationIndex(
        empty,
        live,
        0,
        restoration(anchor, { kind: "grid" }),
      ),
    ).toBe(0);
  });

  test("a retryable lookup keeps the resolved position", async () => {
    const { source, authority: live } = snapshot(["a"]);
    const restoring: RestorationSource = {
      ...source,
      resolvePhotoPosition: () =>
        Promise.resolve<SourcePositionOutcome>({
          kind: "failed",
          authority: live,
          photoId: "z",
          transportLost: true,
        }),
    };
    expect(
      await resolveRestorationIndex(
        restoring,
        live,
        3,
        restoration(
          { photoId: "z", indexHint: 0, offset: 0 },
          { kind: "grid" },
        ),
      ),
    ).toBe(3);
  });

  test("a superseded Snapshot resolves nothing, before and after the lookup", async () => {
    const { source, authority: live } = snapshot(["a"]);
    const anchor = restoration(
      { photoId: "z", indexHint: 0, offset: 0 },
      { kind: "grid" },
    );
    const superseded = { ...source, isCurrent: () => false };
    expect(
      await resolveRestorationIndex(superseded, live, 3, anchor),
    ).toBeUndefined();
    let supersede = false;
    const late: RestorationSource = {
      ...source,
      isCurrent: () => !supersede,
      resolvePhotoPosition: () => {
        supersede = true;
        return Promise.resolve<SourcePositionOutcome>({
          kind: "resolved",
          authority: live,
          photoId: "z",
          position: 2,
        });
      },
    };
    expect(
      await resolveRestorationIndex(late, live, 0, anchor),
    ).toBeUndefined();
  });

  test("geometry restores the captured row and names the focused cell it still holds", () => {
    const { source } = snapshot(["a", "b", "c"]);
    const anchor = { photoId: "c", indexHint: 2, offset: 40 };
    expect(
      restorationGeometry(
        source,
        2,
        restoration(anchor, { kind: "photo", photoId: "b" }),
      ),
    ).toEqual({ index: 2, offset: 40, focusIndex: 1 });
    expect(
      restorationGeometry(source, 2, restoration(anchor, { kind: "grid" })),
    ).toEqual({ index: 2, offset: 40 });
    // An anchor the Snapshot no longer holds restores geometry only.
    expect(
      restorationGeometry(
        source,
        0,
        restoration(anchor, { kind: "photo", photoId: "b" }),
      ),
    ).toEqual({ index: 0, offset: 40 });
  });
});
