import { expect, test } from "bun:test";
import { createEditorController } from "./editor-controller.js";
import type {
  EditorViewPort,
  EditorSurfaceFactories,
} from "./editor-controller-contract.js";
import type { WorkspaceEditModel } from "./workspace-edit-owner.js";
import type { PhotoSummary } from "../api/contracts.js";

test("workspace preparation responses never take over another Photo", async () => {
  const pending = Promise.withResolvers<Response>();
  let selected = "a";
  const presented: string[] = [];
  const surfaces: EditorSurfaceFactories = {
    createFilm: () => ({
      element: {} as HTMLElement,
      open: async () => {},
      dispose() {},
    }),
    createCatalog: () => ({
      element: {} as HTMLElement,
      bind() {},
      dispose() {},
    }),
  };
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
    } satisfies EditorViewPort,
    {
      isAlive: () => true,
      isCurrentPhoto: (id) => selected === id,
      currentPhoto: () => undefined,
      libraryPhase: () => "idle",
    },
    surfaces,
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

test("camera reference follows only the current Photo and accepts late Preview publication", () => {
  let photo: PhotoSummary = {
    id: "a",
    available: true,
    original: { kind: "jpeg", available: true },
    selectionState: "unflagged",
    rating: 0,
    hasSavedEdits: false,
    preview: { state: "inspection-pending" },
  };
  const references: (string | undefined)[] = [];
  const controller = createEditorController(
    () => new Promise<Response>(() => {}),
    {
      editorVisible: () => true,
      renderEditor: (model) => references.push(model.cameraReferenceUrl),
      clearEditorPreview() {},
      presentEditorPreview() {},
    },
    {
      isAlive: () => true,
      isCurrentPhoto: (id) => id === "a",
      currentPhoto: () => photo,
      libraryPhase: () => "idle",
    },
    {
      createFilm: () => {
        throw new Error("not used");
      },
      createCatalog: () => {
        throw new Error("not used");
      },
    },
  );
  controller.open("a");
  expect(references.at(-1)).toBeUndefined();
  photo = { ...photo, preview: { state: "ready", url: "/a-preview" } };
  controller.referenceChanged("a");
  expect(references.at(-1)).toBe("/a-preview");
  photo = { ...photo, id: "b", preview: { state: "ready", url: "/b-preview" } };
  controller.referenceChanged("a");
  expect(references.at(-1)).toBeUndefined();
});
