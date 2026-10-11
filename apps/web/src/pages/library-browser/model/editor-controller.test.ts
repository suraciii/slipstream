import { expect, test } from "bun:test";
import { createEditorController } from "./editor-controller.js";
import type { LibraryBrowserView } from "../ui/library-browser-view.js";
import type { WorkspaceEditModel } from "./workspace-edit-owner.js";

test("workspace preparation responses never take over another Photo", async () => {
  const pending = Promise.withResolvers<Response>();
  let selected = "a";
  const presented: string[] = [];
  const controller = createEditorController(
    async (input) => {
      const url =
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url;
      if (url === "/api/photos/a/workspace") return pending.promise;
      if (url === "/api/photos/b/workspace")
        return Response.json({
          workspaceId: "wb",
          photoId: "b",
          inputArtifactId: null,
          edits: [],
        });
      if (url.endsWith("/files?limit=60"))
        return Response.json({ items: [], total: 0, nextCursor: null });
      return Response.json({
        workspaceId: url.includes("wa") ? "wa" : "wb",
        photoId: url.includes("wa") ? "a" : "b",
        inputArtifactId: null,
        edits: [],
      });
    },
    {
      editorVisible: () => true,
      renderEditor: (model: WorkspaceEditModel) =>
        presented.push(model.photoId),
      clearEditorPreview() {},
      presentEditorPreview() {},
    } as unknown as LibraryBrowserView,
    {
      isAlive: () => true,
      isCurrentPhoto: (id) => selected === id,
      currentPhoto: () => undefined,
    },
  );
  controller.open("a");
  selected = "b";
  controller.open("b");
  const boundary = presented.length;
  pending.resolve(
    Response.json({
      workspaceId: "wa",
      photoId: "a",
      inputArtifactId: null,
      edits: [],
    }),
  );
  await pending.promise;
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
  expect(presented.slice(boundary)).not.toContain("a");
});
