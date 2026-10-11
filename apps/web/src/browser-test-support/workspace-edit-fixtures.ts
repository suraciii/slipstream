import { expect, type Page, type Route } from "@playwright/test";
import { createHash } from "node:crypto";
import { browseIds, fixture, server, writePhotos } from "./fixtures.js";
import {
  startReview,
  openPhotoTools,
  openPhotoToolsView,
  returnToPhotoTools,
} from "./surfaces.js";
import type {
  EditView,
  JsonObject,
  ToolJob,
  Workspace,
  WorkspaceFile,
} from "../pages/library-browser/index.js";

export const capability = {
  read: true,
  write: true,
  execute: true,
  reason: null,
  recovery: null,
};
export const windowOf = <T>(items: T[]) => ({
  items,
  total: items.length,
  nextCursor: null,
});
export type Command = {
  operation: string;
  editId: string | null;
  arguments: JsonObject;
};
export type CapturedCommand = {
  photoId: string;
  body: string;
  command: Command;
};
export type WorkspaceFixture = {
  url: string;
  photos: string[];
  workspaces: Map<string, Workspace>;
  edits: Map<string, EditView>;
  files: Map<string, WorkspaceFile[]>;
  jobs: Map<string, ToolJob[]>;
  bytes: Map<string, Buffer>;
  commands: CapturedCommand[];
  downloads: string[];
  removals: { artifactId: string; body: JsonObject }[];
  command?: (
    route: Route,
    captured: CapturedCommand,
  ) => Promise<void | boolean>;
  remove?: (
    route: Route,
    photoId: string,
    artifactId: string,
    body: JsonObject,
  ) => Promise<void>;
};
export const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
export function editFixture(
  photoId: string,
  exposure = 0,
  revision = "r1",
): EditView {
  return {
    photoId,
    editId: `edit-${photoId}`,
    revision,
    stateToken: `state-${revision}`,
    baselineRevision: "r0",
    bundleId: "fixture-darktable",
    schemaVersion: "v1",
    availability: capability,
    instances: windowOf([
      {
        instanceId: "exposure-1",
        moduleId: "exposure",
        name: "Exposure",
        enabled: true,
        order: 0,
        parameters: { exposure },
        availability: capability,
      },
    ]),
    masks: windowOf([]),
    actions: windowOf([]),
    frames: [],
    output: {
      settings: { format: "jpeg" },
      schema: {
        type: "object",
        properties: { format: { type: "string", enum: ["jpeg", "png"] } },
      },
      availability: capability,
    },
  };
}
export function fileFixture(
  artifactId: string,
  bytes: Buffer,
  revision = "file-r1",
  kind = "output",
): WorkspaceFile {
  return {
    artifactId,
    revision,
    relativePath: `${kind}s/${artifactId}.tif`,
    kind,
    state: "ready",
    byteLength: bytes.length,
    sha256: createHash("sha256").update(bytes).digest("hex"),
    imageContract: {
      format: "tiff",
      precision: "float32",
      transfer: "linear",
      colorSpace: "linear-rec2020",
      width: 16,
      height: 12,
    },
  };
}
export function retainFile(
  state: WorkspaceFixture,
  photoId: string,
  artifactId: string,
  bytes: Buffer,
  revision = "file-r1",
  kind = "output",
) {
  const file = fileFixture(artifactId, bytes, revision, kind);
  state.files.get(photoId)!.push(file);
  state.bytes.set(`${artifactId}:${revision}`, bytes);
  return file;
}
export function capturedJob(
  photoId: string,
  requestId: string,
  revision = "r1",
  status = "accepted",
  operation = "export create",
): ToolJob {
  return {
    photoId,
    workspaceId: `workspace-${photoId}`,
    editId: `edit-${photoId}`,
    requestId,
    status,
    state: status,
    operation,
    revision,
    capture: {
      revision,
      input: { artifactId: `input-${photoId}`, revision: "input-r1" },
    },
    result: null,
  };
}
export async function setupWorkspace(page: Page): Promise<WorkspaceFixture> {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  const photos = await browseIds(running.url);
  const state: WorkspaceFixture = {
    url: running.url,
    photos,
    workspaces: new Map(),
    edits: new Map(),
    files: new Map(),
    jobs: new Map(),
    bytes: new Map(),
    commands: [],
    downloads: [],
    removals: [],
  };
  for (const photoId of photos) {
    state.workspaces.set(photoId, {
      workspaceId: `workspace-${photoId}`,
      photoId,
      inputArtifactId: `input-${photoId}`,
      observedSourceRevision: "source-r1",
      edits: [
        { editId: `edit-${photoId}`, name: "Photo edit", tool: "darktable" },
      ],
    });
    state.edits.set(photoId, editFixture(photoId));
    state.files.set(photoId, []);
    state.jobs.set(photoId, []);
    retainFile(
      state,
      photoId,
      `input-${photoId}`,
      Buffer.from("prepared input"),
      "input-r1",
      "input",
    );
  }
  await page.route("**/api/photos/*/workspace", (route) => {
    const photoId = new URL(route.request().url()).pathname.split("/")[3]!;
    return json(route, state.workspaces.get(photoId));
  });
  await page.route("**/api/workspaces/**", async (route) => {
    const url = new URL(route.request().url());
    const parts = url.pathname.split("/");
    const photoId = photos.find(
      (id) => state.workspaces.get(id)!.workspaceId === parts[3],
    );
    if (!photoId)
      throw new Error(`Unexpected workspace request ${url.pathname}`);
    if (parts.length === 4) return json(route, state.workspaces.get(photoId));
    if (parts[4] !== "files")
      throw new Error(`Unexpected workspace operation ${url.pathname}`);
    if (!parts[5]) return json(route, windowOf(state.files.get(photoId)!));
    const artifactId = decodeURIComponent(parts[5]);
    const file = state.files
      .get(photoId)!
      .find((item) => item.artifactId === artifactId);
    if (!file)
      return json(
        route,
        {
          error: {
            code: "not_found",
            effect: "none",
            message: "File no longer retained",
          },
        },
        404,
      );
    if (parts[6] === "bytes") {
      state.downloads.push(url.pathname + url.search);
      const bytes = state.bytes.get(
        `${artifactId}:${url.searchParams.get("revision")}`,
      );
      if (!bytes)
        return json(
          route,
          {
            error: {
              code: "artifact_changed",
              effect: "none",
              message: "Selected file revision is unavailable",
            },
          },
          409,
        );
      return route.fulfill({
        status: 200,
        contentType: "image/tiff",
        headers: {
          "Content-Disposition": `attachment; filename="${file.relativePath.split("/").at(-1)}"`,
        },
        body: bytes,
      });
    }
    if (route.request().method() === "DELETE") {
      const body = route.request().postDataJSON() as JsonObject;
      state.removals.push({ artifactId, body });
      if (state.remove) return state.remove(route, photoId, artifactId, body);
      if (body.revision !== file.revision)
        return json(
          route,
          {
            error: {
              code: "artifact_changed",
              effect: "none",
              message: "Review changed file again",
            },
          },
          409,
        );
      state.files.set(
        photoId,
        state.files
          .get(photoId)!
          .filter((item) => item.artifactId !== artifactId),
      );
      return json(route, { removed: true });
    }
    return json(route, file);
  });
  await page.route("**/api/photos/*/darktable/jobs/**", (route) => {
    const parts = new URL(route.request().url()).pathname.split("/");
    const photoId = parts[3]!;
    const job = state.jobs
      .get(photoId)!
      .find((item) => item.requestId === parts[6]);
    if (!job) throw new Error("Unknown fixture job");
    if (parts[7] === "cancel") {
      job.status = "cancelled";
      job.state = "cancelled";
    }
    return json(route, job);
  });
  await page.route("**/api/photos/*/darktable/commands", async (route) => {
    const photoId = new URL(route.request().url()).pathname.split("/")[3]!;
    const body = route.request().postData()!;
    const command = JSON.parse(body) as Command;
    const captured = { photoId, body, command };
    state.commands.push(captured);
    if (state.command && (await state.command(route, captured)) !== false)
      return;
    const edit = state.edits.get(photoId)!;
    const args = command.arguments;
    switch (command.operation) {
      case "edit show":
        return json(route, edit);
      case "modules list":
        return json(
          route,
          windowOf([
            {
              moduleId: "exposure",
              name: "Exposure",
              availability: capability,
            },
          ]),
        );
      case "modules describe":
        return json(route, {
          moduleId: "exposure",
          parameters: {
            type: "object",
            properties: {
              exposure: {
                type: "number",
                title: "Exposure (EV)",
                minimum: -5,
                maximum: 5,
              },
            },
          },
        });
      case "edit instances show":
        return json(
          route,
          edit.instances.items.find(
            (item) => item.instanceId === args.instanceId,
          ),
        );
      case "resources kinds":
        return json(route, { kinds: [] });
      case "resources list":
      case "presets list":
      case "styles list":
        if (command.editId !== edit.editId)
          throw new Error("Fixture catalog requires its owning edit");
        return json(route, {
          ...windowOf([]),
          catalog: {
            catalogId: `catalog-${edit.editId}`,
            scope: "edit-local",
            executionHost: "fixture-host",
            stateDirectory: `${root}/workspaces/workspace-${photoId}/edits/${edit.editId}`,
          },
        });
      case "history list":
        return json(route, windowOf([]));
      case "export list":
        return json(route, {
          jobs: state.jobs
            .get(photoId)!
            .filter((job) => job.operation === "export create"),
        });
      case "edit apply": {
        if (args.ifState !== edit.stateToken)
          return json(
            route,
            {
              error: {
                code: "state_conflict",
                effect: "none",
                message: "Another writer changed the saved edit",
              },
            },
            409,
          );
        if (
          !args.input ||
          typeof args.input !== "object" ||
          !("changes" in args.input) ||
          !Array.isArray(args.input.changes)
        )
          throw new Error("Fixture apply requires changes");
        const changes: unknown[] = args.input.changes;
        const next = structuredClone(edit);
        for (const change of changes) {
          if (
            !change ||
            typeof change !== "object" ||
            !("type" in change) ||
            change.type !== "set" ||
            !("instanceId" in change) ||
            !("parameterId" in change) ||
            !("value" in change)
          )
            throw new Error("Unsupported fixture edit");
          const instance = next.instances.items.find(
            (item) => item.instanceId === change.instanceId,
          )!;
          instance.parameters = {
            ...instance.parameters,
            [String(change.parameterId)]: change.value,
          };
        }
        next.revision = `r${Number(edit.revision.slice(1)) + 1}`;
        next.stateToken = `state-${next.revision}`;
        state.edits.set(photoId, next);
        return json(route, {
          revision: next.revision,
          stateToken: next.stateToken,
          current: { revision: next.revision },
        });
      }
      case "preview render":
      case "export create": {
        const job = capturedJob(
          photoId,
          String(args.requestId),
          String(args.revision),
          "accepted",
          command.operation,
        );
        state.jobs.get(photoId)!.push(job);
        return json(route, job, 202);
      }
      case "export retry": {
        const previous = state.jobs
          .get(photoId)!
          .find((job) => job.requestId === args.id)!;
        const job = {
          ...structuredClone(previous),
          requestId: String(args.requestId),
          status: "accepted",
          state: "accepted",
          result: null,
        };
        state.jobs.get(photoId)!.push(job);
        return json(route, job, 202);
      }
      default:
        throw new Error(
          `Unhandled canonical fixture command ${command.operation}`,
        );
    }
  });
  return state;
}
export async function openWorkspace(page: Page, state: WorkspaceFixture) {
  await startReview(page, state.url, "All Photos");
  await openPhotoToolsView(page, "edit");
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toBeVisible();
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Export", exact: true }),
  ).toBeEnabled();
}
export async function navigate(page: Page, direction: "Next" | "Previous") {
  await openPhotoTools(page);
  await returnToPhotoTools(page);
  await page.getByRole("button", { name: direction, exact: true }).click();
  await openPhotoToolsView(page, "edit");
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toBeVisible();
}
export async function openWorkspaceFiles(page: Page) {
  await openPhotoToolsView(page, "edit");
  const files = page.locator("[data-workspace-files]");
  await expect(files).toBeVisible();
  if ((await files.getAttribute("open")) === null)
    await files.locator("summary").click();
  await expect(files).toHaveAttribute("open", "");
}
export async function setExposure(page: Page, value: string) {
  const input = page.getByLabel("Exposure (EV)", { exact: true });
  await input.fill(value);
  await input.press("Tab");
}
export const exportButton = (page: Page) =>
  page.getByRole("button", { name: "Export", exact: true });
export const jobRow = (page: Page, requestId: string) =>
  page.locator(`[data-workspace-jobs] article[data-request-id="${requestId}"]`);
export const fileRow = (page: Page, artifactId: string) =>
  page.locator(
    `[data-workspace-file-groups] article[data-artifact-id="${artifactId}"]`,
  );
export const commandsFor = (
  state: WorkspaceFixture,
  photoId: string,
  operation: string,
) =>
  state.commands.filter(
    (item) => item.photoId === photoId && item.command.operation === operation,
  );
