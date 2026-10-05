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
      await page.getByRole("button", { name: /^Sources/ }).click();
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
    "target/debug/slipstream",
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

for (const firstTransport of ["https", "http"] as const) {
  test(`Photo handoff across both transports, starting with ${firstTransport}`, async ({
    page,
    context,
  }) => {
    const [photoId] = await browseIds(server.url);
    if (!photoId) throw new Error("The handoff smoke requires one Photo");
    const current = await fixtureFetch(
      `${server.url}/api/photos/${photoId}/processing-recipe`,
    );
    expect(current.status).toBe(200);
    const source = (await current.json()) as { sourceRevision: string };
    // Saving a recipe and exporting its XMP snapshot never executes a module.
    const saved = await fixtureFetch(
      `${server.url}/api/photos/${photoId}/processing-recipe`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          requestId: "handoff-recipe",
          expectedRecipeRevision: null,
          expectedSourceRevision: source.sourceRevision,
          currentStepId: "handoff-step",
          steps: [
            {
              stepId: "handoff-step",
              module: "darktable",
              input: {
                kind: "original",
                photoId,
                sourceRevision: source.sourceRevision,
              },
              parameters: {
                schemaVersion: "darktable-params-1",
                tree: {
                  stack: [
                    {
                      operation: "exposure",
                      multiPriority: 0,
                      enabled: true,
                      params: { mode: "EXPOSURE_MODE_MANUAL", exposure: 0.5 },
                    },
                  ],
                },
              },
            },
          ],
        }),
      },
    );
    expect(saved.status).toBe(201);
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
      // The second alias observes the first alias's saved Photo decision.
      await openPhotoToolsView(page, "tools");
      await page
        .getByRole("button", {
          name: index === 0 ? "Reject" : "Pick",
          exact: true,
        })
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
      const xmp = page.locator('[data-editor-output="xmp"]');
      await expect(
        xmp.getByRole("button", { name: "Export edit state", exact: true }),
      ).toBeEnabled();
      await xmp
        .getByRole("button", { name: "Export edit state", exact: true })
        .click();
      const downloadButton = xmp.getByRole("button", {
        name: "Download XMP",
        exact: true,
      });
      await expect(downloadButton).toBeVisible();
      const downloadPromise = page.waitForEvent("download");
      await downloadButton.click();
      const download = await downloadPromise;
      expect(download.suggestedFilename()).toMatch(/\.xmp$/);
      const path = await download.path();
      if (!path) throw new Error("The browser did not retain the XMP download");
      expect(await readFile(path, "utf8")).toContain("RecipeSnapshot");
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
        await openPhotoToolsView(page, "tools");
        await page.getByRole("button", { name: "Reject", exact: true }).click();
        await closePhotoTools(page);
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

test("failed history check can retry and reopen the Library", async ({
  page,
}) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  const sources = page.getByRole("navigation", { name: "Sources" });
  await expect(sources).toBeVisible();
  await page.route("**/api/access/session", (route) => route.abort());
  await page.evaluate(() =>
    window.dispatchEvent(new PopStateEvent("popstate")),
  );
  await expect(sources).toBeHidden();
  const retry = page.getByRole("button", {
    name: "Check access again",
    exact: true,
  });
  await expect(retry).toBeVisible();
  await page.unroute("**/api/access/session");
  await retry.click();
  await expect(sources).toBeVisible();
  await expect(page.getByText("Checking access…", { exact: true })).toHaveCount(
    0,
  );
  await page.getByRole("button", { name: "New Album", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Create Album", exact: true }),
  ).toBeVisible();
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
    await expect(
      page.getByRole("navigation", { name: "Sources" }),
    ).toBeVisible();
    expect(checks).toBe(1);
  } finally {
    release();
  }
});
