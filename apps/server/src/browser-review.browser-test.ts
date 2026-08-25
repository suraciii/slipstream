import { createHash } from "node:crypto";
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";

import { expect, test } from "@playwright/test";
import sharp from "sharp";

import { startServer, type RunningServer } from "./http-server.js";

const sample = process.env.SLIPSTREAM_RAW_SAMPLE;
const temporary: string[] = [];
const servers: RunningServer[] = [];

test.afterEach(async () => {
  await Promise.all(servers.splice(0).map((server) => server.close()));
  await Promise.all(
    temporary
      .splice(0)
      .map((path) => rm(path, { recursive: true, force: true })),
  );
});

async function jpeg(width = 80, height = 40, color = "#c04020") {
  return sharp({
    create: { width, height, channels: 3, background: color },
  })
    .jpeg()
    .toBuffer();
}

async function fixture() {
  const base = await mkdtemp(join(tmpdir(), "slipstream-browser-"));
  temporary.push(base);
  const root = join(base, "originals");
  await mkdir(root);
  return { base, root };
}

async function server(base: string, root: string) {
  const running = await startServer({
    libraryRoot: root,
    stateDirectory: join(base, "state"),
    databaseBasename: "library.sqlite",
    cacheDirectory: join(base, "cache"),
    host: "127.0.0.1",
    port: 42000 + Math.floor(Math.random() * 1000),
  });
  servers.push(running);
  return running;
}

test("uses the production server for source labels, unavailable state, controls, keyboard, and read-only navigation", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg(120, 60));
  const missing = join(root, "b.jpg");
  await writeFile(missing, await jpeg(100, 50));
  const running = await server(base, root);
  await rm(missing);
  await fetch(`${running.url}/api/scan`, { method: "POST" });
  const browserRequests: string[] = [];
  page.on("request", (request) => browserRequests.push(request.method()));

  await page.goto(running.url);
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect(page.getByText("JPEG", { exact: true })).toBeVisible();
  const image = page.locator("img");
  await expect(image).toBeVisible();
  await expect
    .poll(() =>
      image.evaluate((element: HTMLImageElement) => ({
        complete: element.complete,
        width: element.naturalWidth,
        height: element.naturalHeight,
      })),
    )
    .toEqual({ complete: true, width: 120, height: 60 });
  await expect(
    page.getByText("Limited by camera Preview resolution", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await expect(
    page.getByText("Original File is unavailable. Rescan after restoring it."),
  ).toBeVisible();
  await page.keyboard.press("ArrowLeft");
  await expect(page.getByText("1 / 2")).toBeVisible();
  await page.keyboard.press("ArrowRight");
  await page.getByRole("button", { name: "Previous" }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();
  expect(browserRequests.every((method) => method === "GET")).toBe(true);
});

test("keeps an unavailable Photo visible when the previous Preview response arrives late", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg(120, 60));
  const missing = join(root, "b.jpg");
  await writeFile(missing, await jpeg(100, 50));
  const running = await server(base, root);
  await rm(missing);
  await fetch(`${running.url}/api/scan`, { method: "POST" });

  let releasePreview!: () => void;
  const previewReleased = new Promise<void>((resolve) => {
    releasePreview = resolve;
  });
  let previewRequested!: () => void;
  const requestStarted = new Promise<void>((resolve) => {
    previewRequested = resolve;
  });
  await page.route("**/api/photos/*/preview", async (route) => {
    previewRequested();
    await previewReleased;
    await route.continue();
  });

  await page.goto(running.url);
  await requestStarted;
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await expect(
    page.getByText("Original File is unavailable. Rescan after restoring it."),
  ).toBeVisible();
  releasePreview();
  await page.waitForTimeout(250);
  await expect(page.getByText("2 / 2")).toBeVisible();
  await expect(
    page.getByText("Preview unavailable", { exact: true }),
  ).toBeVisible();
  await expect(page.locator("[data-source]")).toHaveText("—");
  await expect(page.locator("img")).toHaveCount(0);
});

test("ignores a detached Preview image failure after navigation", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg(120, 60));
  const missing = join(root, "b.jpg");
  await writeFile(missing, await jpeg(100, 50));
  const running = await server(base, root);
  await rm(missing);
  await fetch(`${running.url}/api/scan`, { method: "POST" });

  let derivativeRequested!: () => void;
  const derivativeStarted = new Promise<void>((resolve) => {
    derivativeRequested = resolve;
  });
  let abortDerivative!: () => void;
  const derivativeAborted = new Promise<void>((resolve) => {
    abortDerivative = resolve;
  });
  await page.route("**/api/photos/*/preview", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        state: "ready",
        source: "matching-jpeg",
        width: 120,
        height: 60,
        limitedDetail: true,
        stale: false,
        url: "/api/derivatives/detached.jpg",
      }),
    }),
  );
  await page.route("**/api/derivatives/detached.jpg", async (route) => {
    derivativeRequested();
    await derivativeAborted;
    await route.abort();
  });

  await page.goto(running.url);
  await derivativeStarted;
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await expect(
    page.getByText("Original File is unavailable. Rescan after restoring it."),
  ).toBeVisible();
  abortDerivative();
  await page.waitForTimeout(250);
  await expect(
    page.getByText("Original File is unavailable. Rescan after restoring it."),
  ).toBeVisible();
  await expect(page.locator("[data-source]")).toHaveText("—");
  await expect(page.locator("img")).toHaveCount(0);
});

test("labels a previous derivative as stale when current replacement fails", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const original = join(root, "photo.jpg");
  await writeFile(original, await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("JPEG", { exact: true })).toBeVisible();
  await writeFile(original, "malformed replacement");
  await fetch(`${running.url}/api/scan`, { method: "POST" });
  await page.reload();
  await expect(page.getByText(/Showing a stale Preview/)).toBeVisible();
  await expect(page.locator("img")).toBeVisible();
});

test("shows matching JPEG first and RAW embedded JPEG after corrupt-JPEG fallback through the production server", async ({
  page,
}) => {
  test.skip(!sample, "Set SLIPSTREAM_RAW_SAMPLE for the camera smoke");
  const cameraSample = sample!;
  const before = await sha256(cameraSample);
  const { base, root } = await fixture();
  const raw = join(root, `camera${extname(cameraSample)}`);
  const matching = join(root, "camera.jpg");
  await copyFile(cameraSample, raw);
  await writeFile(matching, await jpeg(160, 80, "#34905d"));
  const running = await server(base, root);

  await page.goto(running.url);
  await expect(page.getByText("JPEG", { exact: true })).toBeVisible();
  await writeFile(matching, "corrupt jpeg");
  await fetch(`${running.url}/api/scan`, { method: "POST" });
  await page.reload();
  await expect(
    page.getByText("RAW embedded JPEG", { exact: true }),
  ).toBeVisible();
  expect(await sha256(cameraSample)).toBe(before);
});

async function sha256(path: string): Promise<string> {
  return createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
}
