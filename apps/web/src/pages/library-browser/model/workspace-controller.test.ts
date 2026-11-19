import { expect, test } from "bun:test";
import type { BrowserFetch } from "../api/browser-fetch.js";
import { createWorkspaceController } from "./workspace-controller.js";
import type { WorkspaceViewModel } from "./workspace-controller-contract.js";

const file = (
  artifactId: string,
  kind: string,
  relativePath: string,
  revision = `${artifactId}-r1`,
) => ({
  artifactId,
  revision,
  relativePath,
  kind,
  state: "ready",
  byteLength: 12,
});

const workspace = (files: unknown[], nextCursor: string | null = null) => ({
  workspaceId: "w1",
  photoId: "a",
  path: "/srv/workspaces/w1",
  inputArtifactId: "input",
  files: { items: files, total: files.length, nextCursor },
});

const requestUrl = (input: RequestInfo | URL): string =>
  typeof input === "string"
    ? input
    : input instanceof URL
      ? input.href
      : input.url;

const respond =
  (
    handler: (
      input: RequestInfo | URL,
      init?: RequestInit,
    ) => Response | Promise<Response>,
  ): BrowserFetch =>
  (input, init) =>
    Promise.resolve(handler(input, init));

const controllerFor = (fetcher: BrowserFetch) => {
  const models: WorkspaceViewModel[] = [];
  const ready = Promise.withResolvers<void>();
  const controller = createWorkspaceController(
    fetcher,
    {
      workspaceVisible: () => true,
      renderWorkspace: (model) => {
        models.push({ ...model, files: [...model.files] });
        if (model.workspace) ready.resolve();
      },
    },
    { isAlive: () => true, isCurrentPhoto: () => true },
  );
  return { controller, models, ready: ready.promise };
};

test("a late workspace response never takes over another Photo", async () => {
  const pending = Promise.withResolvers<Response>();
  let current = "a";
  const models: WorkspaceViewModel[] = [];
  const controller = createWorkspaceController(
    respond((input) => {
      const url = requestUrl(input);
      if (url === "/api/photos/a/workspace") return pending.promise;
      return Response.json({
        workspaceId: "wb",
        photoId: "b",
        inputArtifactId: null,
        files: { items: [], total: 0, nextCursor: null },
      });
    }),
    {
      workspaceVisible: () => true,
      renderWorkspace: (model) => models.push(model),
    },
    { isAlive: () => true, isCurrentPhoto: (id) => id === current },
  );
  controller.open("a");
  current = "b";
  controller.open("b");
  const boundary = models.length;
  pending.resolve(
    Response.json({
      workspaceId: "wa",
      photoId: "a",
      inputArtifactId: "input",
      files: { items: [], total: 0, nextCursor: null },
    }),
  );
  await pending.promise;
  await Promise.resolve();
  expect(models.slice(boundary).every((model) => model.photoId === "b")).toBe(
    true,
  );
});

test("inspect and remove use only workspace file endpoints", async () => {
  const calls: string[] = [];
  let removed = false;
  const { controller, models, ready } = controllerFor(
    respond((input, init) => {
      const url = requestUrl(input);
      calls.push(`${init?.method ?? "GET"} ${url}`);
      if (url === "/api/photos/a/workspace")
        return Response.json(
          workspace([
            file("input", "input", "input/raw"),
            file("output", "output", "outputs/edit.tif", "output-r1"),
          ]),
        );
      if (
        url === "/api/workspaces/w1/files/output" &&
        init?.method === "DELETE"
      ) {
        removed = true;
        return Response.json({
          outcome: "removed",
          inputArtifactId: "input",
        });
      }
      if (url === "/api/workspaces/w1/files/output")
        return Response.json({ artifactId: "output", revision: "output-r1" });
      if (url === "/api/workspaces/w1/files?limit=60")
        return Response.json({
          items: removed ? [] : [file("output", "output", "outputs/edit.tif")],
          total: removed ? 0 : 1,
          nextCursor: null,
        });
      throw new Error(`Unexpected request ${url}`);
    }),
  );
  controller.open("a");
  await ready;
  await controller.action("a", "inspect", { artifactId: "output" });
  expect(models.at(-1)?.inspectedFile?.artifactId).toBe("output");
  await controller.action("a", "remove", {
    artifactId: "output",
    revision: "output-r1",
  });
  const last = models.at(-1)!;
  expect(last.inspectedFile).toBeUndefined();
  expect(last.status).toBe("File removed.");
  expect(last.notice).toBe("Removed outputs/edit.tif.");
  expect(calls).toContain("DELETE /api/workspaces/w1/files/output");
  expect(calls.some((call) => call.includes("/darktable"))).toBe(false);
  expect(calls.some((call) => call.includes("/commands"))).toBe(false);
});

test("an unknown removal outcome reloads file facts before retry", async () => {
  const { controller, models, ready } = controllerFor(
    respond((input, init) => {
      const url = requestUrl(input);
      if (url === "/api/photos/a/workspace")
        return Response.json(
          workspace([
            file("input", "input", "input/raw", "input-r1"),
            file("output", "output", "outputs/edit.tif", "output-r1"),
          ]),
        );
      if (url === "/api/workspaces/w1/files/output") {
        if (init?.method === "DELETE")
          return Response.json(
            {
              error: {
                code: "outcome_unknown",
                message: "The removal outcome is unknown.",
                effect: "unknown",
                details: {},
              },
            },
            { status: 500 },
          );
        return Response.json({ artifactId: "output", revision: "output-r1" });
      }
      if (url === "/api/workspaces/w1/files?limit=60")
        return Response.json({
          items: [file("input", "input", "input/raw", "input-r1")],
          total: 1,
          nextCursor: null,
        });
      throw new Error(`Unexpected request ${url}`);
    }),
  );
  controller.open("a");
  await ready;
  await controller.action("a", "inspect", { artifactId: "output" });
  await controller.action("a", "remove", {
    artifactId: "output",
    revision: "output-r1",
  });
  const last = models.at(-1)!;
  expect(last.inspectedFile).toBeUndefined();
  expect(last.error?.effect).toBe("unknown");
  expect(last.status).toBe("Removal outcome unknown.");
  expect(last.files.map((item) => item.artifactId)).toEqual(["input"]);
});

test("download admits a ready revision through the mounted fetcher", async () => {
  const calls: string[] = [];
  const { controller, models, ready } = controllerFor(
    respond((input, init) => {
      const url = requestUrl(input);
      calls.push(`${init?.method ?? "GET"} ${url}`);
      if (url === "/api/photos/a/workspace")
        return Response.json(
          workspace([file("output", "output", "outputs/edit.tif")]),
        );
      if (url === "/api/workspaces/w1/files/output/bytes?revision=output-r1") {
        expect(new Headers(init?.headers).get("Range")).toBe("bytes=0-0");
        return new Response(null, { status: 200 });
      }
      throw new Error(`Unexpected request ${url}`);
    }),
  );
  controller.open("a");
  await ready;
  await controller.action("a", "download", { artifactId: "output" });
  const last = models.at(-1)!;
  expect(last.download).toEqual({
    artifactId: "output",
    url: "/api/workspaces/w1/files/output/bytes?revision=output-r1",
  });
  expect(last.downloadingArtifactId).toBeUndefined();
  expect(last.status).toBe("Ready.");
  expect(calls).toContain(
    "GET /api/workspaces/w1/files/output/bytes?revision=output-r1",
  );
});

test("a settled removal preserves a newer inspection", async () => {
  const deletion = Promise.withResolvers<Response>();
  const { controller, models, ready } = controllerFor(
    respond((input, init) => {
      const url = requestUrl(input);
      if (url === "/api/photos/a/workspace")
        return Response.json(
          workspace([
            file("input", "input", "input/raw", "input-r1"),
            file("output", "output", "outputs/edit.tif", "output-r1"),
          ]),
        );
      if (url === "/api/workspaces/w1/files/output")
        return init?.method === "DELETE"
          ? deletion.promise
          : Response.json({ artifactId: "output", revision: "output-r1" });
      if (url === "/api/workspaces/w1/files/input")
        return Response.json({ artifactId: "input", revision: "input-r1" });
      if (url === "/api/workspaces/w1/files?limit=60")
        return Response.json({
          items: [file("input", "input", "input/raw", "input-r1")],
          total: 1,
          nextCursor: null,
        });
      throw new Error(`Unexpected request ${url}`);
    }),
  );
  controller.open("a");
  await ready;
  await controller.action("a", "inspect", { artifactId: "output" });
  const removal = controller.action("a", "remove", {
    artifactId: "output",
    revision: "output-r1",
  });
  await controller.action("a", "inspect", { artifactId: "input" });
  expect(models.at(-1)?.inspectedFile?.artifactId).toBe("input");
  deletion.resolve(
    Response.json({ outcome: "removed", inputArtifactId: "input" }),
  );
  await removal;
  const last = models.at(-1)!;
  expect(last.inspectedFile?.artifactId).toBe("input");
  expect(last.notice).toBe("Removed outputs/edit.tif.");
  expect(last.files.map((item) => item.artifactId)).toEqual(["input"]);
});

test("an unknown removal retry reuses its request identity", async () => {
  const requestIds: string[] = [];
  let removed = false;
  const { controller, models, ready } = controllerFor(
    respond((input, init) => {
      const url = requestUrl(input);
      if (url === "/api/photos/a/workspace")
        return Response.json(
          workspace([
            file("input", "input", "input/raw", "input-r1"),
            file("output", "output", "outputs/edit.tif", "output-r1"),
          ]),
        );
      if (url === "/api/workspaces/w1/files/output") {
        if (init?.method === "DELETE") {
          const body = JSON.parse(
            typeof init.body === "string" ? init.body : "{}",
          ) as { requestId: string };
          requestIds.push(body.requestId);
          if (requestIds.length === 1)
            return Response.json(
              {
                error: {
                  code: "outcome_unknown",
                  message: "The removal outcome is unknown.",
                  effect: "unknown",
                  details: {},
                },
              },
              { status: 500 },
            );
          removed = true;
          return Response.json({
            outcome: "removed",
            inputArtifactId: "input",
          });
        }
        return Response.json({ artifactId: "output", revision: "output-r1" });
      }
      if (url === "/api/workspaces/w1/files?limit=60")
        return Response.json({
          items: removed
            ? [file("input", "input", "input/raw", "input-r1")]
            : [
                file("input", "input", "input/raw", "input-r1"),
                file("output", "output", "outputs/edit.tif", "output-r1"),
              ],
          total: removed ? 1 : 2,
          nextCursor: null,
        });
      throw new Error(`Unexpected request ${url}`);
    }),
  );
  controller.open("a");
  await ready;
  await controller.action("a", "inspect", { artifactId: "output" });
  await controller.action("a", "remove", {
    artifactId: "output",
    revision: "output-r1",
  });
  await controller.action("a", "inspect", { artifactId: "output" });
  await controller.action("a", "remove", {
    artifactId: "output",
    revision: "output-r1",
  });
  expect(requestIds).toHaveLength(2);
  expect(requestIds[1]).toBe(requestIds[0]);
  expect(models.at(-1)?.status).toBe("File removed.");
});

test("an unknown removal reload cannot write a newer workspace", async () => {
  const reload = Promise.withResolvers<Response>();
  const { controller, models, ready } = controllerFor(
    respond((input, init) => {
      const url = requestUrl(input);
      if (url === "/api/photos/a/workspace")
        return Response.json(
          workspace([file("output", "output", "outputs/edit.tif")]),
        );
      if (url === "/api/workspaces/w1/files/output")
        return init?.method === "DELETE"
          ? Response.json(
              {
                error: {
                  code: "outcome_unknown",
                  message: "The removal outcome is unknown.",
                  effect: "unknown",
                  details: {},
                },
              },
              { status: 500 },
            )
          : Response.json({ artifactId: "output", revision: "output-r1" });
      if (url === "/api/workspaces/w1/files?limit=60") return reload.promise;
      if (url === "/api/photos/b/workspace")
        return Response.json({
          workspaceId: "w2",
          photoId: "b",
          inputArtifactId: "input-b",
          files: { items: [], total: 0, nextCursor: null },
        });
      throw new Error(`Unexpected request ${url}`);
    }),
  );
  controller.open("a");
  await ready;
  await controller.action("a", "inspect", { artifactId: "output" });
  const removal = controller.action("a", "remove", {
    artifactId: "output",
    revision: "output-r1",
  });
  await Promise.resolve();
  controller.open("b");
  await Promise.resolve();
  reload.resolve(Response.json({ items: [], total: 0, nextCursor: null }));
  await removal;
  const last = models.at(-1)!;
  expect(last.photoId).toBe("b");
  expect(last.notice).toBeUndefined();
  expect(last.status).toBe("No completed files yet.");
});

test("the bounded file cursor appends the next page", async () => {
  const { controller, models, ready } = controllerFor(
    respond((input) => {
      const url = requestUrl(input);
      if (url === "/api/photos/a/workspace")
        return Response.json(
          workspace([file("input", "input", "input/raw")], "next-page"),
        );
      if (url === "/api/workspace-files?cursor=next-page")
        return Response.json({
          items: [file("output", "output", "outputs/edit.tif")],
          total: 2,
          nextCursor: null,
        });
      throw new Error(`Unexpected request ${url}`);
    }),
  );
  controller.open("a");
  await ready;
  await controller.action("a", "files-more");
  expect(models.at(-1)?.files.map((item) => item.artifactId)).toEqual([
    "input",
    "output",
  ]);
});
