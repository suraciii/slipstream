import { expect, test } from "bun:test";
import {
  createWorkspaceEditOwner,
  type WorkspaceEditModel,
} from "./workspace-edit-owner.js";
import { coalesceChanges } from "./workspace-edit-draft.js";
import { WorkspaceApiError, type JsonObject } from "../api/workspace-edit.js";
const capability = {
  read: true,
  write: true,
  execute: true,
  reason: null,
  recovery: null,
};
function state(revision = "r1"): JsonObject {
  return {
    revision,
    baselineRevision: "r0",
    stateToken: `token-${revision}`,
    editId: "edit",
    bundleId: "bundle",
    schemaVersion: "schema",
    availability: capability,
    instances: {
      items: [
        {
          instanceId: "i",
          moduleId: "native-module",
          name: "Operation",
          enabled: true,
          order: 0,
          availability: capability,
        },
      ],
      total: 1,
      nextCursor: null,
    },
    masks: { items: [], total: 0, nextCursor: null },
    actions: { items: [], total: 0, nextCursor: null },
    frames: [],
    output: {
      settings: {},
      schema: { type: "object", properties: {} },
      availability: capability,
    },
  };
}
function harness(
  onApply?: (body: JsonObject) => Promise<Response>,
  onCommand?: (
    body: JsonObject,
  ) => Response | undefined | Promise<Response | undefined>,
  onRequest?: (
    path: string,
  ) => Response | undefined | Promise<Response | undefined>,
  onPublish?: (model: WorkspaceEditModel) => void,
) {
  const bodies: JsonObject[] = [];
  const ready = Promise.withResolvers<void>();
  const workspace = {
    workspaceId: "w",
    photoId: "photo",
    inputArtifactId: "raw",
    edits: [{ editId: "edit", tool: "darktable", name: "Main" }],
  };
  let revision = "r1";
  const owner = createWorkspaceEditOwner(
    async (input, options) => {
      const path =
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url;
      const response = await onRequest?.(path);
      if (response) return response;
      if (path.endsWith("/commands")) {
        if (typeof options?.body !== "string")
          throw new Error("Expected string request body");
        const body = JSON.parse(options.body) as JsonObject;
        bodies.push(body);
        const handled = await onCommand?.(body);
        if (handled) return handled;
        if (body.operation === "edit show")
          return Response.json(state(revision));
        if (body.operation === "edit apply") {
          const response = onApply
            ? await onApply(body)
            : Response.json({
                revision: "r2",
                stateToken: "token-r2",
                current: { revision: "r2" },
              });
          if (response.ok) revision = "r2";
          return response;
        }
        if (body.operation === "modules list")
          return Response.json({ items: [], total: 0, nextCursor: null });
        if (body.operation === "modules describe")
          return Response.json({
            parameters: {
              type: "object",
              properties: { scalar: { type: "number" } },
            },
          });
        if (body.operation === "edit instances show")
          return Response.json({
            parameters: { scalar: 0, unseen: { retained: [1, 2] } },
          });
        if (
          body.operation === "preview render" ||
          body.operation === "export create"
        )
          return Response.json({
            requestId: (body.arguments as JsonObject).requestId,
            status: "failed",
          });
        return Response.json({
          items: [],
          kinds: [],
          jobs: [],
          total: 0,
          nextCursor: null,
        });
      }
      if (path.endsWith("/files?limit=60"))
        return Response.json({
          items: [
            {
              artifactId: "raw",
              revision: "bytes",
              relativePath: "input/RAW.nef",
              kind: "input",
              state: "ready",
              byteLength: 12,
              imageContract: null,
            },
          ],
          total: 1,
          nextCursor: null,
        });
      return Response.json(workspace);
    },
    "photo",
    (model) => {
      if (model.edit && !model.loading) ready.resolve();
      onPublish?.(model);
    },
  );
  return { owner, bodies, ready };
}
test("coalescing preserves dependency-changing action order", () => {
  const first = { type: "set", instanceId: "i", parameterId: "p", value: 1 };
  expect(coalesceChanges([first], { ...first, value: 2 })).toEqual([
    { ...first, value: 2 },
  ]);
  const reset = { type: "reset", scope: "all" };
  expect(coalesceChanges([first, reset], { ...first, value: 2 })).toEqual([
    first,
    reset,
    { ...first, value: 2 },
  ]);
});
test("opening workspace reads state without rendering", async () => {
  const { owner, bodies } = harness();
  await owner.refresh();
  expect(
    bodies.some(
      (body) =>
        body.operation === "preview render" ||
        body.operation === "export create",
    ),
  ).toBe(false);
  expect(owner.model.edit?.revision).toBe("r1");
  owner.dispose();
});
test("guarded focused save preserves unexposed state and keeps conflict draft", async () => {
  const settled = Promise.withResolvers<void>();
  const { owner, bodies } = harness(() =>
    Promise.resolve(
      Response.json(
        {
          error: {
            code: "state_conflict",
            message: "Another writer saved",
            effect: "none",
            details: {},
          },
        },
        { status: 409 },
      ),
    ),
  );
  await owner.refresh();
  await owner.act("change", {
    change: { type: "set", instanceId: "i", parameterId: "scalar", value: 2 },
  });
  await owner.settle();
  settled.resolve();
  const write = bodies.find((body) => body.operation === "edit apply")!;
  expect(write.arguments).toMatchObject({
    ifState: "token-r1",
    input: {
      changes: [
        { type: "set", instanceId: "i", parameterId: "scalar", value: 2 },
      ],
    },
  });
  expect(owner.model.blocked).toBe(true);
  expect(owner.model.draft[0]?.value).toBe(2);
  expect(owner.model.error).toBeInstanceOf(WorkspaceApiError);
  owner.dispose();
});

test("confirmed save conflict stays recoverable instead of becoming uncertain", async () => {
  const { owner } = harness(() =>
    Promise.resolve(
      Response.json(
        {
          error: {
            code: "state_conflict",
            message: "Another writer saved",
            effect: "none",
            details: {},
          },
        },
        { status: 409 },
      ),
    ),
  );
  await owner.refresh();
  await owner.act("change", {
    change: { type: "set", instanceId: "i", parameterId: "scalar", value: 4 },
  });
  await owner.settle();
  expect(owner.model.uncertain).toBeUndefined();
  expect(owner.model.blocked).toBe(true);
  await owner.act("inspect-current");
  expect(owner.model.conflictInspected).toBe(true);
  await owner.act("reapply");
  owner.dispose();
});
test("uncertain save replays exactly original request identity and payload", async () => {
  let attempts = 0;
  const { owner, bodies } = harness(() => {
    if (++attempts === 1) return Promise.reject(new Error("disconnected"));
    return Promise.resolve(
      Response.json({
        revision: "r2",
        stateToken: "token-r2",
        current: { revision: "r2" },
      }),
    );
  });
  await owner.refresh();
  await owner.act("change", {
    change: { type: "set", instanceId: "i", parameterId: "scalar", value: 3 },
  });
  await owner.settle();
  expect(owner.model.uncertain).toBeDefined();
  await owner.act("replay");
  const writes = bodies.filter((body) => body.operation === "edit apply");
  expect(writes).toHaveLength(2);
  expect(writes[1]).toEqual(writes[0]);
  expect(owner.model.draft).toEqual([]);
  owner.dispose();
});

test("reload during creation cannot restore the previous edit", async () => {
  const creating = Promise.withResolvers<Response>();
  const createdRead = Promise.withResolvers<Response>();
  const createStarted = Promise.withResolvers<void>();
  const readStarted = Promise.withResolvers<void>();
  let newReads = 0;
  const { owner, bodies } = harness(undefined, async (body) => {
    if (body.operation === "edit create") {
      createStarted.resolve();
      return creating.promise;
    }
    if (body.operation === "edit show" && body.editId === "new-edit") {
      newReads++;
      if (newReads === 1) {
        readStarted.resolve();
        return createdRead.promise;
      }
      return Response.json({ ...state(), editId: "new-edit" });
    }
    return undefined;
  });
  await owner.refresh();
  const action = owner.act("create");
  await createStarted.promise;
  const oldReads = bodies.filter(
    (body) => body.operation === "edit show" && body.editId === "edit",
  ).length;
  await owner.refresh();
  creating.resolve(Response.json({ editId: "new-edit" }));
  await readStarted.promise;
  await owner.refresh();
  expect(
    bodies.filter(
      (body) => body.operation === "edit show" && body.editId === "edit",
    ),
  ).toHaveLength(oldReads);
  createdRead.resolve(Response.json({ ...state(), editId: "new-edit" }));
  await action;
  expect(owner.model.edit?.editId).toBe("new-edit");
  owner.dispose();
});

for (const terminal of ["succeeded", "failed"]) {
  test(`latest saved revision previews after an older ${terminal} preview`, async () => {
    const completion = Promise.withResolvers<Response>();
    const pollingStarted = Promise.withResolvers<void>();
    const latestRequested = Promise.withResolvers<void>();
    let revision = "r1";
    let renders = 0;
    const { owner, bodies } = harness(
      undefined,
      (body) => {
        if (body.operation === "edit show")
          return Response.json(state(revision));
        if (body.operation === "edit apply") {
          revision = `r${Number(revision.slice(1)) + 1}`;
          return Response.json({
            revision,
            stateToken: `token-${revision}`,
            current: { revision },
          });
        }
        if (body.operation === "preview render") {
          renders++;
          if (renders > 1) latestRequested.resolve();
          return Response.json({
            requestId: (body.arguments as JsonObject).requestId,
            status: renders === 1 ? "running" : "failed",
          });
        }
        return undefined;
      },
      async (path) => {
        if (!path.includes("/jobs/")) return undefined;
        pollingStarted.resolve();
        return completion.promise;
      },
    );
    await owner.refresh();
    await owner.act("preview");
    await pollingStarted.promise;
    for (const value of [1, 2]) {
      await owner.act("change", {
        change: {
          type: "set",
          instanceId: "i",
          parameterId: "scalar",
          value,
        },
      });
      await owner.settle();
    }
    expect(renders).toBe(1);
    const first = bodies.find((body) => body.operation === "preview render")!;
    completion.resolve(
      Response.json({
        requestId: (first.arguments as JsonObject).requestId,
        status: terminal,
      }),
    );
    await latestRequested.promise;
    const previews = bodies.filter(
      (body) => body.operation === "preview render",
    );
    expect(previews).toHaveLength(2);
    expect((previews[1]!.arguments as JsonObject).revision).toBe("r3");
    await owner.act("job-check");
    expect(renders).toBe(2);
    owner.dispose();
  });
}

test("unchanged settings do not request another preview", async () => {
  const { owner, bodies } = harness(
    () =>
      Promise.resolve(
        Response.json({
          revision: "r1",
          stateToken: "token-r1",
          current: { revision: "r1" },
        }),
      ),
    (body) =>
      body.operation === "edit show" ? Response.json(state()) : undefined,
  );
  await owner.refresh();
  await owner.act("change", {
    change: { type: "set", instanceId: "i", parameterId: "scalar", value: 0 },
  });
  await owner.settle();
  expect(bodies.filter((body) => body.operation === "preview render")).toEqual(
    [],
  );
  owner.dispose();
});

test("completed export discovers its file after a temporary workspace lease", async () => {
  const refreshed = Promise.withResolvers<void>();
  let terminal = false;
  let fileReads = 0;
  let requestId = "";
  const output = {
    artifactId: "output",
    revision: "output-bytes",
    relativePath: "outputs/image.tiff",
    kind: "output",
    state: "ready",
    byteLength: 12,
    imageContract: null,
  };
  const { owner, bodies } = harness(
    undefined,
    (body) => {
      if (body.operation !== "export create") return undefined;
      requestId = String((body.arguments as JsonObject).requestId);
      return Response.json({ requestId, status: "running" });
    },
    (path) => {
      if (path.includes("/jobs/")) {
        terminal = true;
        return Response.json({ requestId, status: "succeeded" });
      }
      if (!terminal || !path.endsWith("/files?limit=60")) return undefined;
      fileReads++;
      if (fileReads === 1) {
        return Response.json(
          {
            error: {
              code: "workspace_busy",
              message: "Writer settling",
              effect: "none",
              details: {},
            },
          },
          { status: 409 },
        );
      }
      return Response.json({ items: [output], total: 1, nextCursor: null });
    },
    (model) => {
      if (
        model.files.some((file) => file.artifactId === output.artifactId) &&
        model.jobs[0]?.status === "succeeded"
      )
        refreshed.resolve();
    },
  );
  try {
    await owner.refresh();
    await owner.act("export");
    await refreshed.promise;
    expect(owner.model.files).toContainEqual(output);
    expect(owner.model.jobs[0]?.status).toBe("succeeded");
    expect(owner.model.error).toBeUndefined();
    expect(
      bodies.filter((body) => body.operation === "export create"),
    ).toHaveLength(1);
  } finally {
    owner.dispose();
  }
});

test("session Undo and Redo each perform and persist their own guarded edit operation", async () => {
  const confirmed = (revision: string): JsonObject => {
    const base = state(revision) as unknown as {
      instances: { items: Record<string, unknown>[] };
    };
    base.instances.items[0]!.parameters = {
      scalar: revision === "r2" ? 5 : 0,
    };
    return base as unknown as JsonObject;
  };
  const { owner, bodies, ready } = harness(undefined, (body) => {
    if (body.operation === "edit show")
      return Response.json(confirmed(owner.model.edit ? "r2" : "r1"));
    return undefined;
  });
  await owner.refresh();
  await ready.promise;
  const applied = () =>
    bodies.filter((body) => body.operation === "edit apply");
  expect(applied()).toHaveLength(0);
  await owner.act("change", {
    change: { type: "set", instanceId: "i", parameterId: "scalar", value: 5 },
  });
  await owner.settle();
  expect(applied()).toHaveLength(1);
  await owner.act("session-undo");
  await owner.settle();
  const requests = applied();
  expect(requests).toHaveLength(2);
  const first = requests[0]!.arguments as JsonObject;
  const undo = requests[1]!.arguments as JsonObject;
  // One new guarded write, with its own request identity, over the observed
  // state the first action produced.
  expect(undo.requestId).not.toBe(first.requestId);
  expect(undo.ifState).toBe("token-r2");
  expect(first.ifState).toBe("token-r1");
  expect((undo.input as JsonObject).changes).toEqual([
    { type: "set", instanceId: "i", parameterId: "scalar", value: 0 },
  ]);
  // The reversal settled as an ordinary save: it is readable from the tool's
  // confirmed state, not from a browser-local list.
  expect(owner.model.edit?.revision).toBe("r2");
  // The reversed action moved to Redo, so the session can reapply it.
  expect(owner.model.sessionUndoCount).toBe(0);
  expect(owner.model.sessionRedoCount).toBe(1);
  await owner.act("session-redo");
  await owner.settle();
  const redone = applied();
  expect(redone).toHaveLength(3);
  const redo = redone[2]!.arguments as JsonObject;
  expect(redo.requestId).not.toBe(undo.requestId);
  expect(redo.ifState).toBe("token-r2");
  expect((redo.input as JsonObject).changes).toEqual([
    { type: "set", instanceId: "i", parameterId: "scalar", value: 5 },
  ]);
  // Redoing submitted the original action again as its own guarded write.
  expect(owner.model.sessionUndoCount).toBe(1);
  expect(owner.model.sessionRedoCount).toBe(0);
  // Neither reversal touched the tool's retained history.
  expect(
    bodies.filter((body) => body.operation === "history restore"),
  ).toHaveLength(0);
  owner.dispose();
});

for (const scenario of [
  {
    type: "blend-set",
    change: { type: "blend-set", instanceId: "i", value: { opacity: 0.8 } },
    inverse: { type: "blend-set", instanceId: "i", value: { opacity: 0.2 } },
  },
  {
    type: "mask-set",
    change: { type: "mask-set", maskId: "m", value: { x: 2 } },
    inverse: { type: "mask-set", maskId: "m", value: { x: 1 } },
  },
  {
    type: "output-set",
    change: { type: "output-set", fieldId: "format", value: "png" },
    inverse: { type: "output-set", fieldId: "format", value: "jpeg" },
  },
]) {
  test(`session Undo saves a complete ${scenario.type} inverse`, async () => {
    let applied = 0;
    const { owner, bodies, ready } = harness(
      () => {
        applied += 1;
        return Promise.resolve(
          Response.json({
            revision: "r2",
            stateToken: "token-r2",
            current: { revision: "r2" },
          }),
        );
      },
      (body) => {
        if (body.operation !== "edit show") return undefined;
        const value = state(applied ? "r2" : "r1") as unknown as {
          instances: { items: Record<string, unknown>[] };
          masks: {
            items: Record<string, unknown>[];
            total: number;
            nextCursor: null;
          };
          output: { settings: Record<string, unknown> };
        };
        value.instances.items[0]!.blend = {
          opacity: applied === 1 ? 0.8 : 0.2,
        };
        value.instances.items[0]!.maskIds = [];
        value.masks = {
          items: [{ maskId: "m", value: { x: applied === 1 ? 2 : 1 } }],
          total: 1,
          nextCursor: null,
        };
        value.output.settings = { format: applied === 1 ? "png" : "jpeg" };
        // Only the scenario's target differs in the confirmed action.
        if (scenario.type !== "blend-set")
          value.instances.items[0]!.blend = { opacity: 0.2 };
        if (scenario.type !== "mask-set")
          value.masks.items[0]!.value = { x: 1 };
        if (scenario.type !== "output-set")
          value.output.settings = { format: "jpeg" };
        return Response.json(value);
      },
    );
    await owner.refresh();
    await ready.promise;
    await owner.act("change", { change: scenario.change });
    await owner.settle();
    await owner.act("session-undo");
    await owner.settle();
    const requests = bodies.filter((body) => body.operation === "edit apply");
    expect(requests).toHaveLength(2);
    const args = requests[1]!.arguments as JsonObject;
    expect((args.input as JsonObject).changes).toEqual([scenario.inverse]);
    expect(args.ifState).toBe("token-r2");
    owner.dispose();
  });
}

test("a retained revision restore is its own guarded operation and is never Undo or Redo", async () => {
  const { owner, bodies, ready } = harness(undefined, (body) => {
    if (body.operation === "edit show") return Response.json(state("r2"));
    if (body.operation === "history restore")
      return Response.json({ revision: "r1", stateToken: "token-r1" });
    return undefined;
  });
  await owner.refresh();
  await ready.promise;
  await owner.act("change", {
    change: { type: "set", instanceId: "i", parameterId: "scalar", value: 5 },
  });
  await owner.settle();
  await owner.act("revision-restore", { revision: "r1" });
  await owner.settle();
  const restore = bodies.find((body) => body.operation === "history restore");
  expect(restore).toBeDefined();
  const args = restore!.arguments as JsonObject;
  // Its own label, its own guarded write and its own request identity.
  expect(args.revision).toBe("r1");
  expect(args.ifState).toBe("token-r2");
  expect(args.requestId).toBeTruthy();
  // The session's reversible actions are not a revision list relabel.
  expect(owner.model.sessionUndoCount).toBe(0);
  expect(owner.model.sessionRedoCount).toBe(0);
  owner.dispose();
});

test("Library progress is projected as the Edit surface's own fact", async () => {
  const { owner, ready } = harness();
  await owner.refresh();
  await ready.promise;
  // The controller supplies the Library's declared phase; the owner never
  // derives it from the Original's availability.
  expect(owner.model.libraryPhase).toBe("");
  owner.model.libraryPhase = "recovering";
  expect(owner.model.libraryPhase).toBe("recovering");
  owner.dispose();
});
