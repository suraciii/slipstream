import { expect, test } from "bun:test";
import { createComposableAutosave } from "./composable-autosave.js";
import type { ComposableRecipe } from "../api/composable-recipe.js";

function fixture(initial = true) {
  let recipe: ComposableRecipe | null = initial
    ? {
        photoId: "p1",
        revision: "r0",
        sourceRevision: "s1",
        currentStepId: "step",
        steps: [
          {
            stepId: "step",
            module: "darktable",
            input: { kind: "original", photoId: "p1", sourceRevision: "s1" },
            parameters: { schemaVersion: "v1", tree: { ev: 0 } },
          },
        ],
      }
    : null;
  let revision = 0;
  let failure = "";
  let external = false;
  let pending: Promise<Response> | undefined;
  const writes: { path: string; body: Record<string, unknown> }[] = [];
  const read = () => ({
    photoId: "p1",
    sourceRevision: recipe?.sourceRevision ?? "s1",
    recipe,
  });
  const owner = createComposableAutosave(
    async (input, init) => {
      const path = input instanceof Request ? input.url : String(input);
      if (init?.method !== "POST") return Response.json(read());
      if (typeof init.body !== "string") throw new Error("Expected JSON body");
      const body = JSON.parse(init.body) as Record<string, unknown>;
      writes.push({ path, body });
      if (pending) return pending;
      if (failure)
        return Response.json({ error: { code: failure } }, { status: 409 });
      const prior = recipe?.steps[0];
      recipe = {
        photoId: "p1",
        revision: `r${++revision}`,
        sourceRevision: "s1",
        currentStepId: path.endsWith("processing-recipe")
          ? (body["currentStepId"] as string | null)
          : "step",
        steps: path.endsWith("processing-recipe")
          ? (body["steps"] as ComposableRecipe["steps"])
          : [
              {
                stepId: "step",
                module: "darktable",
                input: prior?.input ?? {
                  kind: "original",
                  photoId: "p1",
                  sourceRevision: "s1",
                },
                parameters: {
                  schemaVersion: "v1",
                  tree: { ev: path.endsWith("/reset") ? 0 : body["value"] },
                },
              },
            ],
      };
      const accepted = recipe.revision;
      if (external) recipe = { ...recipe, revision: "external" };
      return path.endsWith("processing-recipe")
        ? Response.json({
            outcome: "saved",
            recipe,
            recipeVersion: recipe.revision,
            sourceRevision: "s1",
          })
        : Response.json({ outcome: "saved", edit: { editRevision: accepted } });
    },
    () => {},
    undefined,
  );
  owner.open("p1", read());
  return {
    owner,
    writes,
    read,
    fail: (code: string) => {
      failure = code;
    },
    external: () => {
      external = true;
    },
    replaceSaved: (sourceRevision = "s1") => {
      if (recipe) recipe = { ...recipe, revision: "external", sourceRevision };
    },
    defer: (value: Promise<Response>) => {
      pending = value;
    },
  };
}

test("primary edit and reset share chronological compatibility Undo and Redo", async () => {
  const f = fixture();
  await f.owner.primary("p1", 1);
  await f.owner.primary("p1");
  expect(f.owner.get("p1")?.undo).toHaveLength(2);
  await f.owner.undo("p1");
  expect(f.owner.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({ ev: 1 });
  await f.owner.redo("p1");
  expect(f.owner.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({ ev: 0 });
  const state = f.owner.get("p1")!;
  await f.owner.change("p1", {
    ...state.draft,
    steps: state.draft.steps.map((step) => ({
      ...step,
      parameters: { ...step.parameters, tree: { ev: 2, compatibility: true } },
    })),
  });
  await f.owner.primary("p1", 3);
  await f.owner.undo("p1");
  expect(f.owner.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({
    ev: 2,
    compatibility: true,
  });
  await f.owner.undo("p1");
  expect(f.owner.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({ ev: 0 });
  expect(f.writes.at(-1)?.body["expectedRecipeRevision"]).toBe("r7");
});

test("first exposure Undo returns absent prior intent through guarded zero-step save", async () => {
  const f = fixture(false);
  await f.owner.primary("p1", 1);
  await f.owner.undo("p1");
  expect(f.owner.get("p1")?.draft.steps).toEqual([]);
  expect(f.writes.at(-1)?.body).toMatchObject({
    expectedRecipeRevision: "r1",
    currentStepId: null,
    steps: [],
  });
  await f.owner.redo("p1");
  expect(f.owner.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({ ev: 1 });
});

test("no-op primary confirmation preserves history; external revision invalidates it", async () => {
  const f = fixture();
  await f.owner.primary("p1", 0);
  expect(f.owner.get("p1")?.undo).toHaveLength(0);
  await f.owner.primary("p1", 1);
  f.external();
  await f.owner.primary("p1", 2);
  expect(f.owner.get("p1")?.undo).toHaveLength(0);
  expect(f.owner.get("p1")?.conflict).toBe(true);
  const count = f.writes.length;
  await f.owner.undo("p1");
  expect(f.writes).toHaveLength(count);
});

test("uncertain primary action blocks history and replays exact request identity", async () => {
  const f = fixture();
  await f.owner.primary("p1", 1);
  f.defer(Promise.reject(new Error("response lost")));
  await f.owner.primary("p1", 2);
  expect(f.owner.get("p1")?.uncertain).toBe(true);
  const count = f.writes.length;
  await f.owner.undo("p1");
  await f.owner.primary("p1", 3);
  expect(f.writes).toHaveLength(count);
  await f.owner.flush("p1");
  expect(f.writes.at(-1)?.body).toEqual(f.writes.at(-2)?.body);
});

test("failed primary action retains intent and adds no false confirmed history", async () => {
  const f = fixture();
  f.fail("edit_conflict");
  await f.owner.primary("p1", 2);
  expect(f.owner.get("p1")?.failure).toBe(true);
  expect(f.owner.get("p1")?.conflict).toBe(true);
  expect(f.owner.get("p1")?.undo).toHaveLength(0);
  f.fail("");
  await f.owner.reapply("p1");
  expect(f.owner.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({ ev: 2 });
  expect(f.writes.at(-1)?.body["requestId"]).not.toBe(
    f.writes[0]?.body["requestId"],
  );
});

test("definite primary refusal and retry preserve confirmed Undo and Redo", async () => {
  const f = fixture();
  await f.owner.primary("p1", 1);
  await f.owner.primary("p1", 2);
  await f.owner.undo("p1");
  const undo = structuredClone(f.owner.get("p1")!.undo);
  const redo = structuredClone(f.owner.get("p1")!.redo);
  expect(undo).toHaveLength(1);
  expect(redo).toHaveLength(1);
  f.fail("invalid_value");
  await f.owner.primary("p1", 3);
  await f.owner.reapply("p1");
  expect(f.owner.get("p1")?.failure).toBe(true);
  expect(f.owner.get("p1")?.undo).toEqual(undo);
  expect(f.owner.get("p1")?.redo).toEqual(redo);
  f.fail("");
  await f.owner.reapply("p1");
  expect(f.owner.get("p1")?.undo).toHaveLength(2);
  expect(f.owner.get("p1")?.redo).toHaveLength(0);
  await f.owner.undo("p1");
  expect(f.owner.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({ ev: 1 });
  await f.owner.undo("p1");
  expect(f.owner.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({ ev: 0 });
});

test("primary refusal retry invalidates history when the saved base changed externally", async () => {
  const f = fixture();
  await f.owner.primary("p1", 1);
  await f.owner.primary("p1", 2);
  await f.owner.undo("p1");
  f.fail("edit_conflict");
  f.replaceSaved();
  await f.owner.primary("p1", 3);
  await f.owner.reapply("p1");
  expect(f.owner.get("p1")?.undo).toHaveLength(0);
  expect(f.owner.get("p1")?.redo).toHaveLength(0);
  expect(f.writes.at(-1)?.body["expectedEditRevision"]).toBe("external");
  const count = f.writes.length;
  await f.owner.undo("p1");
  expect(f.writes).toHaveLength(count);
});

test("primary refusal retry clears stale history and refuses a changed Original", async () => {
  const f = fixture();
  await f.owner.primary("p1", 1);
  f.fail("invalid_value");
  await f.owner.primary("p1", 2);
  f.replaceSaved("s2");
  const count = f.writes.length;
  await f.owner.reapply("p1");
  expect(f.owner.get("p1")?.undo).toHaveLength(0);
  expect(f.owner.get("p1")?.redo).toHaveLength(0);
  expect(f.owner.get("p1")?.conflict).toBe(true);
  expect(f.writes).toHaveLength(count);
});

test("pending primary settlement remains Photo owned through departure", async () => {
  const read = { photoId: "p1", sourceRevision: "s1", recipe: null };
  const response = Promise.withResolvers<Response>();
  let recipe: ComposableRecipe | null = null;
  const owner = createComposableAutosave(
    async (_input, init) =>
      init?.method === "POST"
        ? response.promise
        : Response.json({ ...read, recipe }),
    () => {},
    undefined,
  );
  owner.open("p1", read);
  const saving = owner.primary("p1", 1);
  owner.open("p2", { sourceRevision: "s2", recipe: null });
  recipe = {
    photoId: "p1",
    revision: "r1",
    sourceRevision: "s1",
    currentStepId: "step",
    steps: [
      {
        stepId: "step",
        module: "darktable",
        input: { kind: "original", photoId: "p1", sourceRevision: "s1" },
        parameters: { schemaVersion: "v1", tree: { ev: 1 } },
      },
    ],
  };
  response.resolve(
    Response.json({ outcome: "saved", edit: { editRevision: "r1" } }),
  );
  await saving;
  expect(owner.get("p1")?.undo).toHaveLength(1);
  expect(owner.get("p1")?.saving).toBe(false);
  expect(owner.get("p2")?.draft.steps).toEqual([]);
  expect(owner.open("p1", { ...read, recipe }).undo).toHaveLength(1);
});
