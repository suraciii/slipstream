import { expect, test } from "bun:test";
import { createNavigationSession } from "./navigation-session.js";
import {
  allPhotosDestination,
  type NavigationDestination,
} from "./browser-navigation.js";

const album: NavigationDestination = {
  source: "album",
  albumId: "00000000-0000-4000-8000-000000000001",
  selection: "rejected",
};
const folder: NavigationDestination = {
  source: "folder",
  folderPath: "Trip",
  selection: "all",
};

test("a newer complete destination invalidates older same-source establishment", () => {
  const session = createNavigationSession({ kind: "invalid" });
  const previous = session.begin({ ...album, photoId: "photo-a" });
  const current = session.begin({ ...album, photoId: "photo-b" });
  expect(session.isCurrent(previous)).toBe(false);
  expect(session.isCurrent(current)).toBe(true);
  const repeated = session.begin(current.destination);
  expect(session.isCurrent(current)).toBe(false);
  expect(session.isCurrent(repeated)).toBe(true);
  session.dispose();
  expect(session.isCurrent(repeated)).toBe(false);
});

test("startup is consumed once and retains stable restoration metadata", () => {
  const anchor = {
    photoId: "00000000-0000-4000-8000-000000000009",
    indexHint: 30,
    offset: 12,
  };
  const focus = { kind: "grid" as const };
  const session = createNavigationSession({
    kind: "destination",
    destination: folder,
    entry: { version: 1, entryId: "test", anchor, focus },
  });
  expect(session.takeStartup()).toEqual({
    destination: folder,
    restoration: { anchor, focus },
  });
  expect(session.takeStartup()).toBeUndefined();
});

test("retry preserves requested filter and is invalidated by a new source intent", () => {
  const session = createNavigationSession({ kind: "invalid" });
  session.fail(album);
  expect(session.takeRetry()).toEqual(album);
  expect(session.takeRetry()).toBeUndefined();
  session.fail(album);
  session.begin(folder);
  expect(session.takeRetry()).toBeUndefined();
});

test("invalid startup has a single explained All Photos fallback", () => {
  const session = createNavigationSession({ kind: "invalid" });
  expect(session.takeStartup()).toEqual({
    destination: allPhotosDestination,
    explanation:
      "That link is not a valid Library Browser address. Showing All Photos.",
  });
  expect(session.takeStartup()).toBeUndefined();
});

test("disposal withdraws pending destination and retained navigation metadata", () => {
  const session = createNavigationSession({ kind: "invalid" });
  session.fail(album);
  session.requireCurrentFolder("Trip");
  session.captureGrid({
    anchor: {
      photoId: "00000000-0000-4000-8000-000000000009",
      indexHint: 30,
      offset: 12,
    },
    focus: { kind: "grid" },
  });
  session.dispose();
  session.dispose();
  session.fail(folder);
  session.requireCurrentFolder("Later");
  expect(session.takeStartup()).toBeUndefined();
  expect(session.takeRetry()).toBeUndefined();
  expect(session.takeCurrentFolder()).toBeUndefined();
  expect(session.gridRestoration).toBeUndefined();
  expect(session.isCurrent(session.begin(allPhotosDestination))).toBe(false);
});
