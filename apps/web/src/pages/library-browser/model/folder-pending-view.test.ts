import { expect, test } from "bun:test";
import { folderPendingViewModel } from "./folder-pending-view.js";

const pending = { files: 2, hasDescendants: false };

test("a normal folder or Photo view presents no pending strip", () => {
  expect(
    folderPendingViewModel({
      location: undefined,
      photoActive: false,
      pending,
      scan: undefined,
    }),
  ).toBeUndefined();
  expect(
    folderPendingViewModel({
      location: "shoot",
      photoActive: true,
      pending,
      scan: undefined,
    }),
  ).toBeUndefined();
  expect(
    folderPendingViewModel({
      location: "shoot",
      photoActive: false,
      pending: { files: 0, hasDescendants: false },
      scan: undefined,
    }),
  ).toBeUndefined();
});

test("an active same-scope check reports measured progress and blocks retry", () => {
  expect(
    folderPendingViewModel({
      location: "shoot",
      photoActive: false,
      pending,
      scan: {
        state: "recovering",
        completed: 1,
        total: 2,
        scope: "shoot",
      },
    }),
  ).toMatchObject({
    location: "shoot",
    pendingFiles: 2,
    active: true,
    progress: "Checking files… 1 / 2",
    actionEnabled: false,
  });
});

test("a different active check blocks this folder without claiming its progress", () => {
  expect(
    folderPendingViewModel({
      location: "shoot",
      photoActive: false,
      pending,
      scan: { state: "recovering", completed: 1, total: 2, scope: "other" },
    }),
  ).toMatchObject({
    location: "shoot",
    active: false,
    actionEnabled: false,
  });
});
