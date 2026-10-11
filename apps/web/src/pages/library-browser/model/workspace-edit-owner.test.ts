import { expect, test } from "bun:test";
import { createWorkspaceEditOwner } from "./workspace-edit-owner.js";
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
  onCommand?: (body: JsonObject) => Promise<Response | undefined>,
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
