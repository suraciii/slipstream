import { expect, test } from "bun:test";
import {
  createWorkspaceOutputController,
  type OutputFacts,
} from "./workspace-output-controller.js";
const saved: OutputFacts = {
  recipeRevision: "recipe-1",
  sourceRevision: "source-1",
  stepId: "selected",
  saving: false,
  dirty: false,
  conflict: false,
};
const artifact = {
  photoId: "photo",
  exportId: "xmp-1",
  target: "edit-state-xmp",
  state: "succeeded",
  recipeVersion: "recipe-1",
  sourceRevision: "source-1",
  createdAt: "2026-10-01T00:00:00Z",
  expiresAt: "2099-10-01T00:00:00Z",
  artifact: {
    filename: "edit.xmp",
    contentType: "application/rdf+xml",
    byteLength: 12,
    sha256: "a".repeat(64),
  },
};
test("XMP submits guarded recipe state and retains identical uncertain request", async () => {
  const bodies: unknown[] = [];
  let fail = true;
  const owner = createWorkspaceOutputController(
    (_input, init) => {
      if (init?.method === "POST") {
        if (typeof init.body !== "string")
          throw new Error("Expected a JSON request body");
        bodies.push(JSON.parse(init.body));
        if (fail) return Promise.reject(new Error("lost response"));
        return Promise.resolve(Response.json(artifact));
      }
      return Promise.resolve(Response.json({ exports: [] }));
    },
    { owns: () => true, facts: () => saved, render: () => {} },
  );
  owner.open("photo");
  await owner.submit("photo");
  expect(owner.view("photo").xmp.state).toBe("outcome-unknown");
  fail = false;
  await owner.submit("photo");
  expect(bodies[1]).toEqual(bodies[0]);
  expect(bodies[0]).toEqual(
    expect.objectContaining({
      expectedRecipeVersion: "recipe-1",
      expectedSourceRevision: "source-1",
    }),
  );
  expect(owner.view("photo").xmp.state).toBe("succeeded");
  owner.leave();
});
test("no selection or unconfirmed recipe blocks new XMP while old artifact remains downloadable", async () => {
  let facts = saved;
  let posts = 0;
  const owner = createWorkspaceOutputController(
    (_input, init) => {
      if (init?.method === "POST") posts++;
      return Promise.resolve(Response.json({ exports: [artifact] }));
    },
    { owns: () => true, facts: () => facts, render: () => {} },
  );
  owner.open("photo");
  await owner.refresh("photo");
  for (const next of [
    { ...saved, stepId: null },
    { ...saved, dirty: true },
    { ...saved, saving: true },
    { ...saved, conflict: true },
  ]) {
    facts = next;
    await owner.submit("photo");
    expect(owner.view("photo").xmp.canSubmit).toBe(false);
    expect(owner.view("photo").xmp.canDownload).toBe(true);
  }
  expect(posts).toBe(0);
  owner.leave();
});
