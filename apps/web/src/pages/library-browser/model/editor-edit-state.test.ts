import { expect, test } from "bun:test";
import { createEditorEditState } from "./editor-edit-state.js";

const state = (photoId: string, revision: string | null = null) => ({
  photoId,
  editRevision: revision,
  sourceRevision: "source",
  currentSourceRevision: "source",
  sourceAvailable: true,
  requiresRebind: false,
  canSave: false,
  canPreview: false,
  canExport: false,
  current: null,
});

test("no saved state is a checked start state", async () => {
  const editor = createEditorEditState(async () => Response.json(state("p1")), {
    owns: () => true,
    render: () => {},
  });
  await editor.load("p1");
  expect(editor.blocked).toBe(false);
  expect(editor.read?.current).toBe(null);
  expect(editor.note).toContain("start editing");
});

test("concurrent reads share ownership and late reads cannot affect another Photo", async () => {
  const first = Promise.withResolvers<Response>();
  let calls = 0;
  let active = "p1";
  const editor = createEditorEditState(
    async () => {
      calls += 1;
      return calls === 1 ? first.promise : Response.json(state("p2"));
    },
    { owns: (id) => id === active, render: () => {} },
  );
  const pending = editor.load("p1");
  expect(editor.load("p1")).toBe(pending);
  editor.reset();
  active = "p2";
  await editor.load("p2");
  first.resolve(Response.json(state("p1")));
  await pending;
  expect(editor.read?.photoId).toBe("p2");
  expect(calls).toBe(2);
});

test("read failure keeps processing blocked", async () => {
  const editor = createEditorEditState(
    async () => {
      throw new Error("offline");
    },
    {
      owns: () => true,
      render: () => {},
    },
  );
  await editor.load("p1");
  expect(editor.blocked).toBe(true);
  expect(editor.note).toContain("could not be read");
});
