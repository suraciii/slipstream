import { test, expect } from "@playwright/test";
import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, copyFile, rm, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { createConnection } from "node:net";
import { once } from "node:events";
import { join, resolve } from "node:path";
import {
  startBrowserServer,
  fixtureFetch,
  type BrowserServer,
} from "./browser-server.js";
import { browseIds } from "./browser-test-support/fixtures.js";
import {
  capturedJob,
  editFixture,
  fileFixture,
  json,
  windowOf,
  type Command,
} from "./browser-test-support/workspace-edit-fixtures.js";
import type { ToolJob } from "./pages/library-browser/index.js";
import {
  openViewOptions,
  applyViewOptions,
  waitForLoadedReviewImage,
  openPhotoToolsView,
  closePhotoTools,
} from "./browser-test-support/surfaces.js";

let base: string;
async function runCli(
  args: readonly string[],
  environment: NodeJS.ProcessEnv,
): Promise<{ status: number | null; stdout: string; stderr: string }> {
  const child = spawn(
    resolve(process.env.SLIPSTREAM_CLI_BINARY ?? "target/debug/slipstream"),
    args,
    { env: environment, stdio: ["ignore", "pipe", "pipe"] },
  );
  const stdout: Buffer[] = [];
  const stderr: Buffer[] = [];
  child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
  child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
  const [status] = (await once(child, "close")) as [
    number | null,
    NodeJS.Signals | null,
  ];
  return {
    status,
    stdout: Buffer.concat(stdout).toString(),
    stderr: Buffer.concat(stderr).toString(),
  };
}
let server: BrowserServer;
test.beforeEach(async () => {
  base = await mkdtemp(join(tmpdir(), "slipstream-access-browser-"));
  const root = join(base, "originals");
  await mkdir(root);
  await copyFile(
    "apps/web/test-fixtures/review.jpg",
    join(root, "synthetic.jpg"),
  );
  server = await startBrowserServer({
    base,
    root,
    environment: {
      SLIPSTREAM_PHOTO_DEVELOPMENT: "disabled",
      SLIPSTREAM_FILM_MODULE: "disabled",
    },
  });
  await expect
    .poll(
      async () =>
        (
          (await (await fixtureFetch(`${server.url}/api/status`)).json()) as {
            state: string;
          }
        ).state,
    )
    .toBe("idle");
});
test.afterEach(async () => {
  await server?.close();
  if (base) await rm(base, { recursive: true, force: true });
});

for (const viewport of [
  { width: 1280, height: 800 },
  { width: 390, height: 540 },
]) {
  test(`HTTPS access, revalidation, cross-tab sign out and history at ${viewport.width}px`, async ({
    page,
    context,
  }) => {
    await page.setViewportSize(viewport);
    const destination = `${server.url}/?selection=picked`;
    await page.goto(destination);
    await expect(
      page.getByLabel("Access Token", { exact: true }),
    ).toBeVisible();
    await expect(page.locator("img")).toHaveCount(0);
    const anonymous = await context.request.get(`${server.url}/api/overview`);
    expect(anonymous.status()).toBe(401);
    expect(anonymous.headers()["cache-control"]).toBe("no-store");
    await page.getByRole("button", { name: "Show Access Token" }).click();
    await expect(
      page.getByLabel("Access Token", { exact: true }),
    ).toHaveAttribute("type", "text");
    await page
      .getByLabel("Access Token", { exact: true })
      .fill("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    await page
      .getByRole("button", { name: "Open library", exact: true })
      .click();
    await expect(page.getByRole("alert")).toContainText(/token/i);
    await page.getByLabel("Access Token", { exact: true }).fill(server.token);
    await page.getByLabel("Access Token", { exact: true }).press("Enter");
    await expect(
      page.getByRole("navigation", { name: "Sources", includeHidden: true }),
    ).toBeAttached();
    expect(new URL(page.url()).origin).toBe(server.url);
    expect(new URL(page.url()).searchParams.get("selection")).toBe("picked");
    expect(page.url()).not.toContain(server.token);
    expect(
      await page.evaluate(() => ({
        local: { ...localStorage },
        session: { ...sessionStorage },
      })),
    ).toEqual({ local: {}, session: {} });
    const cookies = await context.cookies();
    const session = cookies.find(
      (cookie) => cookie.name === "__Host-slipstream",
    );
    expect(session?.secure).toBe(true);
    expect(session?.httpOnly).toBe(true);
    expect(session?.sameSite).toBe("Lax");
    expect(await page.evaluate(() => document.cookie)).not.toContain(
      "__Host-slipstream",
    );
    const second = await context.newPage();
    await second.goto(server.url);
    await expect(
      second.getByRole("navigation", { name: "Sources", includeHidden: true }),
    ).toBeAttached();
    await page.bringToFront();
    // Revalidation of this same session must leave the mounted fetcher usable.
    await page.evaluate(() =>
      window.dispatchEvent(new PopStateEvent("popstate")),
    );
    await expect(
      page.getByRole("navigation", { name: "Sources", includeHidden: true }),
    ).toBeAttached();
    if (viewport.width < 600)
      await page.locator("[data-source-toggle]").click();
    await page.getByRole("button", { name: "Sign out", exact: true }).click();
    await expect(
      page.getByLabel("Access Token", { exact: true }),
    ).toBeVisible();
    await expect(
      second.getByLabel("Access Token", { exact: true }),
    ).toBeVisible();
    await expect(page.locator("img")).toHaveCount(0);
    await expect(second.locator("img")).toHaveCount(0);
    await page.goBack();
    await page.goto(destination);
    await expect(
      page.getByLabel("Access Token", { exact: true }),
    ).toBeVisible();
    expect(
      (await context.request.get(`${server.url}/api/status`)).status(),
    ).toBe(401);
  });
}

test("direct HTTP keeps access, cookie lifecycle and CLI authentication", async ({
  page,
  context,
}) => {
  const url = server.httpUrl;
  await page.goto(url);
  await expect(page.getByLabel("Access Token", { exact: true })).toBeVisible();
  await expect(page.locator(".access-transport-warning")).toBeVisible();
  expect((await context.request.get(`${url}/api/status`)).status()).toBe(401);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByLabel("Access Token", { exact: true }).press("Enter");
  await expect(
    page.getByRole("navigation", { name: "Sources", includeHidden: true }),
  ).toBeAttached();
  await expect(page.locator(".private-transport")).toContainText(
    "Unencrypted HTTP",
  );
  const cookie = (await context.cookies()).find(
    (cookie) => cookie.name === "slipstream",
  );
  expect(cookie?.secure).toBe(false);
  expect(cookie?.httpOnly).toBe(true);
  expect(cookie?.sameSite).toBe("Lax");
  const first: unknown = await (
    await context.request.get(`${url}/api/access/session`)
  ).json();
  await page.reload();
  await expect(
    page.getByRole("navigation", { name: "Sources", includeHidden: true }),
  ).toBeAttached();
  const second: unknown = await (
    await context.request.get(`${url}/api/access/session`)
  ).json();
  expect(second).toEqual(first);
  const denied = await context.request.post(`${url}/api/albums`, {
    headers: { Origin: url },
    data: { name: "Denied" },
  });
  expect(denied.status()).toBe(403);
  const cli = spawnSync(
    resolve(process.env.SLIPSTREAM_CLI_BINARY ?? "target/debug/slipstream"),
    ["--server", url, "--token-file", server.tokenFile, "status"],
    { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
  );
  expect(cli.status).toBe(0);
  const result: unknown = JSON.parse(cli.stdout);
  expect(result).toMatchObject({ status: "ok", error: null });
  expect(cli.stderr.match(/Warning:/g)).toHaveLength(1);
  expect(cli.stderr).not.toContain(server.token);
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page.getByLabel("Access Token", { exact: true })).toBeVisible();
  expect(
    (await context.cookies()).find((cookie) => cookie.name === "slipstream"),
  ).toBeUndefined();
  expect((await context.request.get(`${url}/api/status`)).status()).toBe(401);
});

test("wrong Origin is disclosed without rewriting the browser address", async ({
  page,
}) => {
  await server.close();
  await rm(join(base, "state"), { recursive: true, force: true });
  server = await startBrowserServer({
    base,
    root: join(base, "originals"),
    transport: "http",
    publicOrigin: "http://127.0.0.1:1",
    accessOrigins: ["http://127.0.0.1:1"],
  });
  const address = server.url;
  await page.goto(address);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "rejected this browser Origin",
  );
  expect(page.url()).toBe(`${address}/`);
  await expect(page.getByLabel("Access Token", { exact: true })).toBeVisible();
});

for (const firstTransport of ["https", "http"] as const) {
  test(`Photo handoff across both transports, starting with ${firstTransport}`, async ({
    page,
    context,
  }) => {
    const [photoId] = await browseIds(server.url);
    if (!photoId) throw new Error("The handoff smoke requires one Photo");
    // Only the external editing tool is simulated: library access, Photo
    // decisions and both authenticated transport aliases remain real.
    const edit = editFixture(photoId, 0.5, "handoff-r1");
    edit.instances = windowOf([]);
    const workspace = {
      workspaceId: `workspace-${photoId}`,
      photoId,
      inputArtifactId: `input-${photoId}`,
      edits: [{ editId: edit.editId, name: "Photo edit", tool: "darktable" }],
    };
    // A complete one-pixel grayscale TIFF, not a native-render acceptance stub.
    const outputBytes = Buffer.alloc(123);
    outputBytes.write("II");
    outputBytes.writeUInt16LE(42, 2);
    outputBytes.writeUInt32LE(8, 4);
    outputBytes.writeUInt16LE(9, 8);
    const tags = [
      [256, 4, 1],
      [257, 4, 1],
      [258, 3, 8],
      [259, 3, 1],
      [262, 3, 1],
      [273, 4, 122],
      [277, 3, 1],
      [278, 4, 1],
      [279, 4, 1],
    ];
    for (const [index, tag] of tags.entries()) {
      const offset = 10 + index * 12;
      outputBytes.writeUInt16LE(tag[0]!, offset);
      outputBytes.writeUInt16LE(tag[1]!, offset + 2);
      outputBytes.writeUInt32LE(1, offset + 4);
      outputBytes.writeUInt32LE(tag[2]!, offset + 8);
    }
    outputBytes[122] = 128;
    const output = fileFixture(
      "handoff-output",
      outputBytes,
      "handoff-file-r1",
    );
    output.imageContract = {
      format: "tiff",
      precision: "uint8",
      transfer: "srgb",
      colorSpace: "gray",
      width: 1,
      height: 1,
    };
    const files = [
      fileFixture(workspace.inputArtifactId, outputBytes, "input-r1", "input"),
    ];
    const jobs: ToolJob[] = [];
    const exportCommands: Command[] = [];
    const downloadedRevisions: string[] = [];
    await page.route(`**/api/photos/${photoId}/workspace`, (route) =>
      json(route, workspace),
    );
    await page.route(
      `**/api/workspaces/${workspace.workspaceId}/**`,
      (route) => {
        const url = new URL(route.request().url());
        if (url.pathname.endsWith("/files"))
          return json(route, windowOf(files));
        if (url.pathname.endsWith(`/files/${output.artifactId}/bytes`)) {
          expect(url.searchParams.get("revision")).toBe(output.revision);
          downloadedRevisions.push(url.searchParams.get("revision")!);
          return route.fulfill({
            status: 200,
            contentType: "image/tiff",
            headers: {
              "Content-Disposition":
                'attachment; filename="handoff-output.tif"',
            },
            body: outputBytes,
          });
        }
        throw new Error(`Unexpected handoff workspace request ${url.pathname}`);
      },
    );
    await page.route(`**/api/workspaces/${workspace.workspaceId}`, (route) =>
      json(route, workspace),
    );
    await page.route(`**/api/photos/${photoId}/darktable/commands`, (route) => {
      const command = route.request().postDataJSON() as Command;
      expect(command.editId).toBe(edit.editId);
      switch (command.operation) {
        case "edit show":
          return json(route, edit);
        case "modules list":
        case "presets list":
        case "styles list":
        case "resources list":
        case "history list":
          return json(route, windowOf([]));
        case "resources kinds":
          return json(route, { kinds: [] });
        case "export list":
          return json(route, { jobs });
        case "export create": {
          expect(command.arguments.revision).toBe(edit.revision);
          expect(command.arguments.requestId).toEqual(expect.any(String));
          exportCommands.push(command);
          const job = capturedJob(
            photoId,
            String(command.arguments.requestId),
            edit.revision,
          );
          jobs.push(job);
          return json(route, job, 202);
        }
        default:
          throw new Error(`Unexpected handoff command ${command.operation}`);
      }
    });
    await page.route(`**/api/photos/${photoId}/darktable/jobs/*`, (route) => {
      const requestId = new URL(route.request().url()).pathname
        .split("/")
        .at(-1);
      const job = jobs.find((item) => item.requestId === requestId);
      if (!job) throw new Error(`Unknown handoff export ${requestId}`);
      job.status = "succeeded";
      job.state = "succeeded";
      job.result = { artifact: output };
      if (!files.some((file) => file.artifactId === output.artifactId))
        files.push(output);
      return json(route, job);
    });
    const origins =
      firstTransport === "https"
        ? [server.url, server.httpUrl]
        : [server.httpUrl, server.url];
    const privateOrigins: string[] = [];
    page.on("request", (request) => {
      const url = new URL(request.url());
      if (url.pathname.startsWith("/api/")) privateOrigins.push(url.origin);
    });
    await page.setViewportSize({ width: 1000, height: 700 });
    for (const [index, origin] of origins.entries()) {
      await page.goto("about:blank");
      // Cookies are host-scoped, so the two ports share a cookie jar. Clear it
      // to prove each transport really establishes its own permitted profile.
      await context.clearCookies();
      const cli = await runCli(
        [
          "--server",
          origin,
          "--token-file",
          server.tokenFile,
          "photos",
          "get",
          photoId,
        ],
        {
          ...process.env,
          SSL_CERT_FILE: resolve("tools/test-tls/cert.pem"),
        },
      );
      expect(cli.status, cli.stderr).toBe(0);
      const envelope = JSON.parse(cli.stdout) as {
        status: string;
        data: { webUrl: string; selectionState: string };
      };
      expect(envelope.status).toBe("ok");
      expect(envelope.data.selectionState).toBe(
        index === 0 ? "unflagged" : "rejected",
      );
      const destination = `${origin}/?photoId=${photoId}`;
      expect(envelope.data.webUrl).toBe(destination);
      expect(cli.stderr).not.toContain(server.token);
      expect(cli.stderr.match(/Warning:/g)?.length ?? 0).toBe(
        origin.startsWith("http:") ? 1 : 0,
      );
      privateOrigins.length = 0;
      await page.goto(envelope.data.webUrl);
      await expect(
        page.getByLabel("Access Token", { exact: true }),
      ).toBeVisible();
      if (origin.startsWith("http:"))
        await expect(page.locator(".access-transport-warning")).toBeVisible();
      await page.getByLabel("Access Token", { exact: true }).fill(server.token);
      await page.getByLabel("Access Token", { exact: true }).press("Enter");
      await waitForLoadedReviewImage(page);
      expect(new URL(page.url()).origin).toBe(origin);
      expect(new URL(page.url()).searchParams.get("photoId")).toBe(photoId);
      expect(page.url()).not.toContain(server.token);
      if (origin.startsWith("http:"))
        await expect(page.locator(".private-transport")).toContainText(
          "Unencrypted HTTP",
        );
      const cookieName = origin.startsWith("https:")
        ? "__Host-slipstream"
        : "slipstream";
      const cookie = (await context.cookies()).find(
        (entry) => entry.name === cookieName,
      );
      expect(cookie).toMatchObject({
        secure: origin.startsWith("https:"),
        httpOnly: true,
        sameSite: "Lax",
        path: "/",
      });
      const otherCookieName = origin.startsWith("https:")
        ? "slipstream"
        : "__Host-slipstream";
      expect(
        (await context.cookies()).find(
          (entry) => entry.name === otherCookieName,
        ),
      ).toBeUndefined();
      const session = await context.request.get(`${origin}/api/access/session`);
      expect(session.status()).toBe(200);
      await expect(page.locator("[data-selection]")).toHaveText(
        index === 0 ? "Unflagged" : "Rejected",
      );
      await closePhotoTools(page);
      await page
        .locator(index === 0 ? "[data-dock-reject]" : "[data-dock-pick]")
        .click();
      await expect(page.locator("[data-selection]")).toHaveText(
        index === 0 ? "Rejected" : "Picked",
      );
      await page.reload();
      await waitForLoadedReviewImage(page);
      await expect(page.locator("[data-selection]")).toHaveText(
        index === 0 ? "Rejected" : "Picked",
      );
      await openPhotoToolsView(page, "edit");
      const exportButton = page
        .locator("[data-workspace-editor-actions]")
        .getByRole("button", { name: "Export", exact: true });
      await expect(exportButton).toBeEnabled();
      const admissionPromise = page.waitForResponse((response) => {
        if (
          !response.url().endsWith(`/api/photos/${photoId}/darktable/commands`)
        )
          return false;
        const command = response.request().postDataJSON() as Command;
        return command.operation === "export create";
      });
      await exportButton.click();
      expect((await admissionPromise).status()).toBe(202);
      await expect.poll(() => exportCommands.length).toBe(index + 1);
      const requestId = String(exportCommands[index]!.arguments.requestId);
      await expect(
        page.locator(
          `[data-workspace-jobs] article[data-request-id="${requestId}"]`,
        ),
      ).toContainText("succeeded");
      await page.locator("[data-workspace-files] > summary").click();
      const downloadButton = page
        .locator(
          `[data-workspace-file-groups] article[data-artifact-id="${output.artifactId}"]`,
        )
        .getByRole("link", { name: "Download", exact: true });
      await expect(downloadButton).toBeVisible();
      const downloadUrl = new URL(
        (await downloadButton.getAttribute("href"))!,
        origin,
      );
      expect(downloadUrl.origin).toBe(origin);
      expect(downloadUrl.searchParams.get("revision")).toBe(output.revision);
      await expect(downloadButton).toHaveAttribute(
        "download",
        "handoff-output.tif",
      );
      // Chromium bypasses Playwright routing for a[download] requests. Let the
      // simulated tool's Content-Disposition response initiate the download
      // instead, retaining the real link URL and authenticated active origin.
      await downloadButton.evaluate((link) => link.removeAttribute("download"));
      const downloadPromise = page.waitForEvent("download");
      await downloadButton.click();
      const download = await downloadPromise;
      expect(download.suggestedFilename()).toBe("handoff-output.tif");
      const path = await download.path();
      if (!path)
        throw new Error("The browser did not retain the TIFF download");
      expect(await readFile(path)).toEqual(outputBytes);
      expect(downloadedRevisions).toHaveLength(index + 1);
      await closePhotoTools(page);
      await page
        .getByRole("button", { name: "Back to Grid", exact: true })
        .click();
      const allPhotos = page.getByRole("link", {
        name: /^All Photos 1 Photo$/,
      });
      await expect(allPhotos).toBeVisible();
      expect(
        new URL((await allPhotos.getAttribute("href")) ?? "", origin).origin,
      ).toBe(origin);
      await allPhotos.click();
      await expect(page.locator("[data-grid-status]")).toHaveText(
        "Ready · 1 Photo",
      );
      expect(new URL(page.url()).origin).toBe(origin);
      if (index === 1) {
        await page.goto(destination);
        await waitForLoadedReviewImage(page);
        await page.locator("[data-dock-reject]").click();
        await expect(page.locator("[data-selection]")).toHaveText("Rejected");
        await page
          .getByRole("button", { name: "Back to Grid", exact: true })
          .click();
      }
      await openViewOptions(page);
      await page.locator("[data-filter-select]").selectOption("rejected");
      await applyViewOptions(page);
      await expect(page.locator("[data-grid-status]")).toHaveText(
        "Ready · 1 Photo",
      );
      await openViewOptions(page);
      await page.locator("[data-removal-open]").click();
      await expect(page.locator("[data-removal-summary]")).toHaveText(
        /^1 Photo reviewed as Rejected\./,
      );
      await page.locator("[data-removal-confirm]").click();
      await expect(page.locator("[data-removal-message]")).toHaveText(
        /^1 Photo removed from the Library\./,
      );
      await page.locator("[data-removal-close]").click();
      await page.reload();
      await openViewOptions(page);
      await page.locator("[data-removed-open]").click();
      const row = page.locator("[data-removed-list] .removed-item");
      await expect(row).toHaveCount(1);
      await row.getByRole("button", { name: "Restore", exact: true }).click();
      await expect(page.locator("[data-removed-message]")).toHaveText(
        /^1 Photo restored to the Library\./,
      );
      await page.locator("[data-removed-close]").click();
      await page.goto(destination);
      await waitForLoadedReviewImage(page);
      await expect(page.locator("[data-selection]")).toHaveText("Rejected");
      const confirmed = await runCli(
        [
          "--server",
          origin,
          "--token-file",
          server.tokenFile,
          "photos",
          "get",
          photoId,
        ],
        {
          ...process.env,
          SSL_CERT_FILE: resolve("tools/test-tls/cert.pem"),
        },
      );
      expect(confirmed.status, confirmed.stderr).toBe(0);
      expect(JSON.parse(confirmed.stdout)).toMatchObject({
        status: "ok",
        data: {
          selectionState: "rejected",
          webUrl: destination,
          removedAtMs: null,
        },
      });
      expect(new Set(privateOrigins)).toEqual(new Set([origin]));
    }
  });
}

test("returning to the library keeps it visible while access is checked", async ({
  page,
}) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  const sources = page.getByRole("navigation", {
    name: "Sources",
    includeHidden: true,
  });
  await expect(sources).toBeAttached();

  let releaseCheck!: () => void;
  const checking = new Promise<void>((resolve) => {
    releaseCheck = resolve;
  });
  let checkStarted!: () => void;
  const started = new Promise<void>((resolve) => {
    checkStarted = resolve;
  });
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "hidden",
    });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await expect(sources).toBeVisible();
  await page.route("**/api/access/session", async (route) => {
    if (route.request().method() !== "GET") return route.continue();
    checkStarted();
    await checking;
    await route.abort();
  });
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "visible",
    });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await started;
  await expect(sources).toBeVisible();
  await expect(page.getByText("Checking access…")).toHaveCount(0);
  const failedCheck = page.waitForEvent("requestfailed", (request) =>
    request.url().endsWith("/api/access/session"),
  );
  releaseCheck();
  await failedCheck;
  await page.unroute("**/api/access/session");
  await expect(sources).toBeVisible();
  await expect(page.getByLabel("Access Token", { exact: true })).toHaveCount(0);
});

test("confirmed session loss on return removes the library", async ({
  page,
}) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  const sources = page.getByRole("navigation", {
    name: "Sources",
    includeHidden: true,
  });
  await expect(sources).toBeAttached();
  await page.route("**/api/access/session", async (route) => {
    if (route.request().method() !== "GET") return route.continue();
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ authenticated: false, configured: true }),
    });
  });
  await page.evaluate(() =>
    document.dispatchEvent(new Event("visibilitychange")),
  );
  await expect(page.getByLabel("Access Token", { exact: true })).toBeVisible();
  await expect(sources).toHaveCount(0);
});

test("restored page keeps private content closed until access is verified", async ({
  page,
}) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  const sources = page.getByRole("navigation", { name: "Sources" });
  await expect(sources).toBeVisible();
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const status: unknown = await (
    await page.request.get(`${server.url}/api/access/session`)
  ).json();
  let started!: () => void;
  const checked = new Promise<void>((resolve) => {
    started = resolve;
  });
  await page.route("**/api/access/session", async (route) => {
    if (route.request().method() !== "GET") return route.continue();
    started();
    await held;
    await route.fulfill({ json: status });
  });
  await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
  await expect(sources).toBeHidden();
  await page.evaluate(() =>
    window.dispatchEvent(
      new PageTransitionEvent("pageshow", { persisted: true }),
    ),
  );
  await checked;
  await expect(sources).toBeHidden();
  release();
  await expect(sources).toBeVisible();
});

test("temporary history access failure keeps the current Library usable and reconciles traversal", async ({
  page,
}) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  const sources = page.getByRole("navigation", { name: "Sources" });
  await expect(sources).toBeVisible();
  await expect(page.getByText(/^Ready · 1 Photo$/)).toBeVisible();
  const initialUrl = page.url();
  await page.evaluate(() =>
    history.pushState({ test: "next" }, "", "?selection=picked"),
  );
  await page.route("**/api/access/session", (route) => route.abort());
  await page.evaluate(() =>
    window.dispatchEvent(new PopStateEvent("popstate")),
  );
  await expect(sources).toBeVisible();
  await expect(page.getByLabel("Access Token", { exact: true })).toHaveCount(0);
  expect(page.url()).toContain("selection=picked");
  expect(page.url()).not.toBe(initialUrl);
  await page.unroute("**/api/access/session");
  await page.evaluate(() => history.back());
  await expect(page).toHaveURL(initialUrl);
  await expect(sources).toBeVisible();
  await expect(page.getByLabel("Access Token", { exact: true })).toHaveCount(0);
});

test("confirmed history rejection removes the Library", async ({ page }) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  const sources = page.getByRole("navigation", { name: "Sources" });
  await expect(sources).toBeVisible();
  await page.route("**/api/access/session", (route) =>
    route.fulfill({ json: { authenticated: false, configured: true } }),
  );
  await page.evaluate(() =>
    window.dispatchEvent(new PopStateEvent("popstate")),
  );
  await expect(page.getByLabel("Access Token", { exact: true })).toBeVisible();
  await expect(sources).toHaveCount(0);
});

test("cookie writes reject missing CSRF and revoked session cannot restore private views", async ({
  page,
  context,
}) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  await expect(
    page.getByRole("navigation", { name: "Sources", includeHidden: true }),
  ).toBeAttached();
  expect(
    (
      await context.request.post(`${server.url}/api/scan`, {
        headers: { Origin: server.url },
      })
    ).status(),
  ).toBe(403);
  const second = await context.newPage();
  await second.goto(server.url);
  await expect(
    second.getByRole("navigation", { name: "Sources", includeHidden: true }),
  ).toBeAttached();
  await page.bringToFront();
  const status = (await (
    await context.request.get(`${server.url}/api/access/session`)
  ).json()) as { csrfToken: string };
  expect(
    (
      await context.request.delete(`${server.url}/api/access/session`, {
        headers: { Origin: server.url, "X-CSRF-Token": status.csrfToken },
      })
    ).status(),
  ).toBe(204);
  await page.evaluate(() =>
    window.dispatchEvent(new PopStateEvent("popstate")),
  );
  await expect(page.getByLabel("Access Token", { exact: true })).toBeVisible();
  await expect(page.locator("img")).toHaveCount(0);
  await expect(
    second.getByLabel("Access Token", { exact: true }),
  ).toBeVisible();
});

test("known expiry hides private content while disconnected", async ({
  page,
  context,
}) => {
  await page.clock.install();
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  await expect(
    page.getByRole("navigation", { name: "Sources" }),
  ).toBeAttached();
  await context.setOffline(true);
  await page.clock.fastForward(7 * 24 * 60 * 60 * 1000 + 1000);
  await expect(page.getByRole("alert")).toContainText("expired");
  await expect(page.getByLabel("Access Token", { exact: true })).toBeVisible();
  await expect(page.locator("img")).toHaveCount(0);
});

test("failed sign out hides Photos without claiming confirmed revocation", async ({
  page,
  context,
}) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  await expect(
    page.getByRole("navigation", { name: "Sources" }),
  ).toBeAttached();
  await context.setOffline(true);
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "could not confirm sign out",
  );
  await expect(page.locator("img")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Retry sign out", exact: true }),
  ).toBeVisible();
});

for (const status of [307, 308]) {
  test(`token exchange refuses cross-origin ${status} redirects`, async ({
    page,
  }) => {
    let externalRequests = 0;
    await page.route("https://redirect.invalid/**", async (route) => {
      externalRequests++;
      await route.fulfill({
        status: 204,
        headers: {
          "Access-Control-Allow-Origin": server.url,
          "Access-Control-Allow-Methods": "POST, OPTIONS",
          "Access-Control-Allow-Headers": "content-type",
        },
      });
    });
    await page.route("**/api/access/session", async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      await route.fulfill({
        status,
        headers: { Location: "https://redirect.invalid/collect" },
      });
    });
    await page.goto(server.url);
    await page.getByLabel("Access Token", { exact: true }).fill(server.token);
    await page
      .getByRole("button", { name: "Open library", exact: true })
      .click();
    await expect(page.getByRole("alert")).toContainText(
      "did not confirm whether access opened",
    );
    expect(externalRequests).toBe(0);
    await expect(page.locator("img")).toHaveCount(0);
  });
}

test("HTTPS fixture closes connections that have not started their TLS handshake", async () => {
  const socket = createConnection({
    host: "127.0.0.1",
    port: Number(new URL(server.url).port),
  });
  await once(socket, "connect");
  let closed = false;
  const closing = server.close().then(() => {
    closed = true;
  });
  try {
    await expect.poll(() => closed).toBe(true);
  } finally {
    socket.destroy();
    await closing;
  }
});

test("visible window focus changes do not hide the Library or check access", async ({
  page,
}) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  await expect(page.getByRole("navigation", { name: "Sources" })).toBeVisible();
  let checks = 0;
  await page.route("**/api/access/session", async (route) => {
    checks++;
    await route.abort();
  });
  await page.evaluate(() => {
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
  });
  await expect(page.getByRole("navigation", { name: "Sources" })).toBeVisible();
  await expect(page.locator(".access-check-overlay")).toHaveCount(0);
  expect(checks).toBe(0);
});

test("image and history restoration share one status check", async ({
  page,
  context,
}) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  await expect(page.getByRole("navigation", { name: "Sources" })).toBeVisible();
  const image = page.locator('img[src*="/api/private/derivatives/"]').first();
  await expect(image).toBeVisible();
  const status: unknown = await (
    await context.request.get(`${server.url}/api/access/session`)
  ).json();
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let checks = 0;
  await page.route("**/api/access/session", async (route) => {
    checks++;
    await held;
    await route.fulfill({ json: status });
  });
  try {
    await image.evaluate((node) => node.dispatchEvent(new Event("error")));
    await expect.poll(() => checks).toBe(1);
    await page.evaluate(() =>
      window.dispatchEvent(new PopStateEvent("popstate")),
    );
    await expect(page.locator(".access-check-overlay")).toBeVisible();
    release();
    await expect(page.locator(".access-check-overlay")).toHaveCount(0);
    await expect(
      page.getByRole("navigation", { name: "Sources" }),
    ).toBeVisible();
    expect(checks).toBe(1);
  } finally {
    release();
  }
});
