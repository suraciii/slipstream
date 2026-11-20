import { expect, test } from "bun:test";
import { createApplicationOwner } from "./application-owner.js";
import { RecoveryGate } from "./async-ownership.js";
import { createFileLocationController } from "./file-location-controller.js";

test("pending folder state is exposed without hiding normal folders", async () => {
  const application = createApplicationOwner(
    () =>
      Promise.resolve(
        Response.json({
          published: true,
          publication: "p1",
          photoCount: 4,
          scan: { state: "idle", publication: "p1" },
          albums: [],
        }),
      ),
    { emit: () => {}, schedule: () => () => {} },
  );
  const gate = new RecoveryGate();
  const controller = createFileLocationController({
    fetcher: () =>
      Promise.resolve(
        Response.json({
          publication: "p1",
          parent: "",
          start: 0,
          limit: 60,
          total: 2,
          children: [
            {
              location: "pending",
              name: "pending",
              photoCount: 0,
              hasDescendantFolders: false,
              pendingFiles: 3,
              hasPendingDescendants: false,
            },
            {
              location: "normal",
              name: "normal",
              photoCount: 4,
              hasDescendantFolders: false,
              pendingFiles: 0,
              hasPendingDescendants: false,
            },
          ],
          pendingFiles: 3,
          hasPendingDescendants: true,
        }),
      ),
    application,
    recoveryGate: gate,
    isAlive: () => true,
    onChanged: () => {},
    onReachable: () => {},
  });
  try {
    await controller.load("", 0);
    const tree = controller.tree(() => false);
    expect(tree.rootPendingFiles).toBe(3);
    expect(tree.rootHasPendingDescendants).toBe(true);
    expect(tree.rootChildren[0]).toMatchObject({
      location: "pending",
      pendingFiles: 3,
      hasPendingDescendants: false,
    });
    expect(tree.rootChildren[1]).toMatchObject({
      location: "normal",
      pendingFiles: 0,
      hasPendingDescendants: false,
    });
    expect(controller.pending("pending")).toEqual({
      files: 3,
      hasDescendants: false,
    });
    expect(controller.pending("normal")).toEqual({
      files: 0,
      hasDescendants: false,
    });
  } finally {
    controller.dispose();
    application.dispose();
    gate.close();
  }
});
