import { describe, expect, test } from "bun:test";
import { createComposableAutosave } from "./composable-autosave.js";
import {
  draftFromRecipe,
  type ComposableRecipeDraft,
  type ComposableSaveRequest,
} from "./composable-recipe-draft.js";
import type { BrowserFetch } from "./access-session.js";
import type { rebindComposableRecipe } from "../api/composable-recipe.js";

type SaveRequest = Extract<ComposableSaveRequest, { kind: "ok" }>["request"];
const parseSaveRequest = (body: string): SaveRequest =>
  JSON.parse(body) as SaveRequest;
const requestBody = (init?: RequestInit): string => {
  if (typeof init?.body !== "string")
    throw new Error("Expected a JSON request body");
  return init.body;
};
const requestUrl = (input: RequestInfo | URL): string =>
  input instanceof Request ? input.url : input.toString();
const draft = (photoId: string, ev: number): ComposableRecipeDraft => ({
  ...draftFromRecipe(photoId, "source\0opaque", null),
  currentStepId: "step",
  steps: [
    {
      stepId: "step",
      module: "darktable",
      input: { kind: "original", photoId, sourceRevision: "source\0opaque" },
      parameters: { schemaVersion: "unsupported-valid", tree: { ev } },
    },
  ],
});
const saved = (body: string, revision: string, outcome = "saved") => {
  const request = parseSaveRequest(body);
  const input = request.steps[0]?.input;
  return Response.json({
    outcome,
    sourceRevision: request.expectedSourceRevision,
    recipeVersion: revision,
    recipe: {
      photoId: input?.kind === "original" ? input.photoId : undefined,
      revision,
      sourceRevision: request.expectedSourceRevision,
      currentStepId: request.currentStepId,
      steps: request.steps,
    },
  });
};
function memoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => {
      values.delete(key);
    },
    setItem: (key, value) => {
      values.set(key, value);
    },
  };
}

test("missing current source retains saved intent without conflict on open, refresh, or use saved", () => {
  const recipe = {
    photoId: "p1",
    revision: "r1",
    sourceRevision: "source\u0000opaque",
    steps: draft("p1", 1).steps,
    currentStepId: "step",
  };
  const unavailable = {
    sourceRevision: "",
    currentSourceRevision: null,
    sourceAvailable: false,
    recipe,
  };
  const owner = createComposableAutosave(
    () => Promise.reject(new Error("No write expected")),
    () => {},
    memoryStorage(),
  );
  owner.open("p1", {
    sourceRevision: recipe.sourceRevision,
    currentSourceRevision: recipe.sourceRevision,
    sourceAvailable: true,
    recipe,
  });
  const state = owner.open("p1", unavailable);
  expect(state.conflict).toBe(false);
  expect(state.draft.sourceRevision).toBe(recipe.sourceRevision);
  expect(state.draft.steps).toEqual(recipe.steps);
  owner.useSaved("p1");
  expect(state.conflict).toBe(false);
  expect(state.draft.steps).toEqual(recipe.steps);
  const reopened = createComposableAutosave(
    () => Promise.reject(new Error("No write expected")),
    () => {},
    memoryStorage(),
  ).open("p1", unavailable);
  expect(reopened.conflict).toBe(false);
  expect(reopened.draft.sourceRevision).toBe(recipe.sourceRevision);
  const dirtyOwner = createComposableAutosave(
    () =>
      Promise.resolve(
        Response.json(
          { error: { code: "invalid_parameters" } },
          { status: 422 },
        ),
      ),
    () => {},
    memoryStorage(),
  );
  const dirtyState = dirtyOwner.open("p1", {
    sourceRevision: recipe.sourceRevision,
    recipe,
  });
  dirtyState.draft = { ...draft("p1", 3), baseRevision: "r1" };
  dirtyOwner.open("p1", unavailable);
  expect(dirtyState.conflict).toBe(false);
  expect(dirtyState.draft.steps[0]?.parameters.tree).toEqual({ ev: 3 });
});

test("recovered local settings stay guarded by the saved recipe when current source is absent", async () => {
  const storage = memoryStorage();
  const recipe = {
    photoId: "p1",
    revision: "r1",
    sourceRevision: "source\u0000opaque",
    steps: draft("p1", 0).steps,
    currentStepId: "step",
  };
  const local = { ...draft("p1", 2), baseRevision: "r1" };
  storage.setItem(
    "slipstream-composable-drafts-v1",
    JSON.stringify({ p1: { draft: local, pending: null } }),
  );
  const writes: string[] = [];
  const owner = createComposableAutosave(
    (_input, init) => {
      writes.push(requestBody(init));
      return Promise.resolve(
        Response.json(
          { error: { code: "resource_unavailable" } },
          { status: 503 },
        ),
      );
    },
    () => {},
    storage,
  );
  const state = owner.open("p1", {
    sourceRevision: "",
    currentSourceRevision: null,
    sourceAvailable: false,
    recipe,
  });
  expect(state.recovered).toBe(true);
  expect(state.conflict).toBe(false);
  expect(state.draft.steps[0]?.parameters.tree).toEqual({ ev: 2 });
  await owner.flush("p1");
  expect(parseSaveRequest(writes[0]!).expectedSourceRevision).toBe(
    recipe.sourceRevision,
  );
});

test("expired receipt never authorizes a fresh dependent write", async () => {
  const writes: string[] = [];
  const owner = createComposableAutosave(
    (_url, init) => {
      writes.push(requestBody(init));
      return Promise.resolve(
        Response.json({ error: { code: "receipt_expired" } }, { status: 409 }),
      );
    },
    () => {},
    memoryStorage(),
  );
  owner.open("p1", { sourceRevision: "source\u0000opaque", recipe: null });
  await owner.change("p1", draft("p1", 1));
  await owner.change("p1", draft("p1", 2));
  expect(writes.length).toBe(1);
  expect(owner.get("p1")?.uncertain).toBe(true);
  expect(owner.get("p1")?.note).toContain("receipt expired");
  await owner.flush("p1");
  expect(writes[1]).toBe(writes[0]);
});

test("explicit rebind uses observed source and preserves local parameter changes", async () => {
  const old = {
    photoId: "p1",
    revision: "old",
    sourceRevision: "source\u0000opaque",
    steps: draft("p1", 1).steps,
    currentStepId: "step",
  };
  const writes: { method: string | undefined; body: string }[] = [];
  const owner = createComposableAutosave(
    (url, init) => {
      const body = requestBody(init);
      writes.push({ method: init?.method, body });
      if (requestUrl(url).endsWith("/rebind")) {
        return Promise.resolve(
          Response.json({
            outcome: "saved",
            sourceRevision: "new",
            recipeVersion: "rebound",
            recipe: {
              ...old,
              revision: "rebound",
              sourceRevision: "new",
              steps: old.steps.map((step) => ({
                ...step,
                input: {
                  kind: "original",
                  photoId: "p1",
                  sourceRevision: "new",
                },
              })),
            },
          }),
        );
      }
      return Promise.resolve(saved(body, "final"));
    },
    () => {},
    memoryStorage(),
  );
  owner.open("p1", {
    sourceRevision: "source\u0000opaque",
    currentSourceRevision: "new",
    sourceAvailable: true,
    recipe: old,
  });
  await owner.change("p1", { ...draft("p1", 2), baseRevision: "old" });
  expect(writes).toEqual([]);
  owner.useSaved("p1");
  expect(owner.get("p1")?.conflict).toBe(true);
  await owner.change("p1", { ...draft("p1", 2), baseRevision: "old" });
  await owner.rebind("p1");
  expect(writes[0]?.method).toBe("POST");
  expect(
    (
      JSON.parse(writes[0]!.body) as Parameters<
        typeof rebindComposableRecipe
      >[2]
    ).newSourceRevision,
  ).toBe("new");
  expect(parseSaveRequest(writes[1]!.body).expectedRecipeRevision).toBe(
    "rebound",
  );
  expect(owner.get("p1")?.read.recipe?.steps[0]?.parameters.tree).toEqual({
    ev: 2,
  });
  expect(owner.get("p1")?.read.currentSourceRevision).toBe("new");
});

describe("photo-owned composable autosave", () => {
  test("late confirmation preserves newer settings and coalesces against its confirmed revision", async () => {
    const writes: string[] = [];
    const pending = Promise.withResolvers<Response>();
    const fetcher: BrowserFetch = (_url, init) => {
      writes.push(requestBody(init));
      if (writes.length === 1) return pending.promise;
      return Promise.resolve(saved(writes.at(-1)!, "r2"));
    };
    const owner = createComposableAutosave(fetcher, () => {}, memoryStorage());
    owner.open("p1", { sourceRevision: "source\0opaque", recipe: null });
    const first = owner.change("p1", draft("p1", 1));
    owner.open("p2", { sourceRevision: "s2", recipe: null });
    void owner.change("p1", draft("p1", 2));
    void owner.change("p1", draft("p1", 3));
    expect(writes.length).toBe(1);
    pending.resolve(saved(writes[0]!, "r1"));
    await first;
    expect(writes.length).toBe(2);
    expect(parseSaveRequest(writes[1]!).expectedRecipeRevision).toBe("r1");
    expect(parseSaveRequest(writes[1]!).steps[0]?.parameters.tree).toEqual({
      ev: 3,
    });
    expect(owner.get("p1")?.read.recipe?.revision).toBe("r2");
    expect(owner.get("p2")?.read.recipe).toBeNull();
  });

  test("unknown outcome blocks dependent writes and replays exact bytes before saving newer settings", async () => {
    const writes: string[] = [];
    const fetcher: BrowserFetch = (_url, init) => {
      writes.push(requestBody(init));
      if (writes.length === 1)
        return Promise.reject(new Error("lost response"));
      return Promise.resolve(
        saved(
          writes.at(-1)!,
          writes.length === 2 ? "r1" : "r2",
          writes.length === 2 ? "replayed" : "saved",
        ),
      );
    };
    const owner = createComposableAutosave(fetcher, () => {}, memoryStorage());
    owner.open("p1", { sourceRevision: "source\0opaque", recipe: null });
    await owner.change("p1", draft("p1", 1));
    void owner.change("p1", draft("p1", 2));
    expect(writes.length).toBe(1);
    expect(owner.get("p1")?.uncertain).toBe(true);
    owner.useSaved("p1");
    expect(owner.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({ ev: 2 });
    await owner.flush("p1");
    expect(writes[1]).toBe(writes[0]);
    expect(parseSaveRequest(writes[2]!).steps[0]?.parameters.tree).toEqual({
      ev: 2,
    });
  });

  test("mismatched successful acknowledgement remains uncertain", async () => {
    const owner = createComposableAutosave(
      (_url, init) => {
        const request = parseSaveRequest(requestBody(init));
        const changed = {
          ...request,
          steps: request.steps.map((step) => ({
            ...step,
            parameters: { ...step.parameters, tree: { ev: 999 } },
          })),
        };
        return Promise.resolve(saved(JSON.stringify(changed), "r1"));
      },
      () => {},
      memoryStorage(),
    );
    owner.open("p1", { sourceRevision: "source\0opaque", recipe: null });
    await owner.change("p1", draft("p1", 1));
    expect(owner.get("p1")?.uncertain).toBe(true);
    expect(owner.get("p1")?.read.recipe).toBeNull();
    expect(owner.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({ ev: 1 });
  });

  test("conflict retains local intent and explicit reapply uses freshly observed guard", async () => {
    const writes: string[] = [];
    const remote = {
      photoId: "p1",
      sourceRevision: "source\0opaque",
      revision: "remote",
      currentStepId: "step",
      steps: draft("p1", 10).steps,
    };
    const fetcher: BrowserFetch = (_url, init) => {
      if (!init?.method)
        return Promise.resolve(
          Response.json({
            photoId: "p1",
            sourceRevision: "source\0opaque",
            recipe: remote,
          }),
        );
      writes.push(requestBody(init));
      if (writes.length === 1)
        return Promise.resolve(
          Response.json(
            { error: { code: "recipe_conflict" } },
            { status: 409 },
          ),
        );
      return Promise.resolve(saved(writes.at(-1)!, "r2"));
    };
    const owner = createComposableAutosave(fetcher, () => {}, memoryStorage());
    owner.open("p1", { sourceRevision: "source\0opaque", recipe: null });
    await owner.change("p1", draft("p1", 1));
    void owner.change("p1", draft("p1", 2));
    expect(writes.length).toBe(1);
    expect(owner.get("p1")?.conflict).toBe(true);
    await owner.reapply("p1");
    expect(parseSaveRequest(writes[1]!).expectedRecipeRevision).toBe("remote");
    expect(parseSaveRequest(writes[1]!).steps[0]?.parameters.tree).toEqual({
      ev: 2,
    });
  });

  test("undo and redo save action snapshots with current revision guards", async () => {
    const writes: string[] = [];
    const owner = createComposableAutosave(
      (_url, init) => {
        writes.push(requestBody(init));
        return Promise.resolve(saved(writes.at(-1)!, `r${writes.length}`));
      },
      () => {},
      memoryStorage(),
    );
    owner.open("p1", { sourceRevision: "source\0opaque", recipe: null });
    await owner.change("p1", draft("p1", 1));
    await owner.change("p1", draft("p1", 2));
    await owner.undo("p1");
    expect(parseSaveRequest(writes[2]!).steps[0]?.parameters.tree).toEqual({
      ev: 1,
    });
    expect(parseSaveRequest(writes[2]!).expectedRecipeRevision).toBe("r2");
    await owner.redo("p1");
    expect(parseSaveRequest(writes[3]!).steps[0]?.parameters.tree).toEqual({
      ev: 2,
    });
    expect(parseSaveRequest(writes[3]!).expectedRecipeRevision).toBe("r3");
  });

  test("recovery of newer draft retains exact earlier pending snapshot and does not overwrite newer server", async () => {
    const storage = memoryStorage();
    const original = createComposableAutosave(
      () => Promise.reject(new Error("lost")),
      () => {},
      storage,
    );
    original.open("p1", { sourceRevision: "source\0opaque", recipe: null });
    await original.change("p1", draft("p1", 1));
    void original.change("p1", draft("p1", 2));
    const writes: string[] = [];
    const recovered = createComposableAutosave(
      (_url, init) => {
        writes.push(requestBody(init));
        return Promise.resolve(
          saved(writes.at(-1)!, writes.length === 1 ? "r1" : "r2"),
        );
      },
      () => {},
      storage,
    );
    recovered.open("p1", { sourceRevision: "source\0opaque", recipe: null });
    expect(recovered.get("p1")?.recovered).toBe(true);
    expect(recovered.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({
      ev: 2,
    });
    await recovered.flush("p1");
    expect(parseSaveRequest(writes[0]!).steps[0]?.parameters.tree).toEqual({
      ev: 1,
    });
    expect(parseSaveRequest(writes[1]!).steps[0]?.parameters.tree).toEqual({
      ev: 2,
    });
  });

  test("storage refusal keeps online session drafts with truthful recovery state", async () => {
    const storage = memoryStorage();
    storage.setItem = () => {
      throw new Error("quota");
    };
    const owner = createComposableAutosave(
      () => Promise.reject(new Error("offline")),
      () => {},
      storage,
    );
    owner.open("p1", { sourceRevision: "source\0opaque", recipe: null });
    await owner.change("p1", draft("p1", 1));
    expect(owner.get("p1")?.recoveryAvailable).toBe(false);
    expect(owner.get("p1")?.draft.steps[0]?.parameters.tree).toEqual({ ev: 1 });
    expect(owner.get("p1")?.uncertain).toBe(true);
  });
});
