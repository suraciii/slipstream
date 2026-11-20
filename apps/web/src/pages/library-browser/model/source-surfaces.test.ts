import { expect, test } from "bun:test";
import { createSourceSurfaces } from "./source-surfaces.js";

const surfacesFor = (
  source: Readonly<{
    kind: string;
    folder?: Readonly<{ location: string }>;
  }>,
  load: (parent: string) => Promise<void>,
) =>
  createSourceSurfaces({
    folders: {
      tree: () => ({
        enabled: false,
        failures: [],
        rootExpanded: false,
        rootLoading: false,
        rootChildren: [],
        rootPendingFiles: 0,
        rootHasPendingDescendants: false,
      }),
      pending: () => undefined,
      load: (parent) => load(parent),
    },
    source,
    photoCount: () => 0,
    albums: () => [],
    view: { renderSources: () => {}, renderFolderPending: () => {} },
    albumManagement: () => ({ setAlbums: () => {} }),
    gridMulti: () => ({ renderBatchAlbums: () => {} }),
    photoActive: () => false,
    scan: () => undefined,
    alive: () => true,
  });

test("refreshing file locations reloads the open folder after the root", async () => {
  const calls: string[] = [];
  const surfaces = surfacesFor(
    { kind: "folder", folder: { location: "shoot" } },
    (parent) => {
      calls.push(parent);
      return Promise.resolve();
    },
  );
  await surfaces.refreshFileLocations();
  expect(calls).toEqual(["", "shoot"]);
});

test("refreshing file locations does not reload a non-folder source", async () => {
  const calls: string[] = [];
  const surfaces = surfacesFor({ kind: "library" }, (parent) => {
    calls.push(parent);
    return Promise.resolve();
  });
  await surfaces.refreshFileLocations();
  expect(calls).toEqual([""]);
});
