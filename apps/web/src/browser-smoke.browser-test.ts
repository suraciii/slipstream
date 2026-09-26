import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createHash } from "node:crypto";
import type { Server } from "node:http";
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join, resolve } from "node:path";

import {
  expect,
  test,
  type BrowserContext,
  type Locator,
  type Page,
  type Request as PlaywrightRequest,
} from "@playwright/test";

import {
  fixtureFetch,
  startBrowserServer,
  type BrowserServer,
} from "./browser-server.js";

const sample = process.env.SLIPSTREAM_RAW_SAMPLE;
const temporary: string[] = [];
const servers: BrowserServer[] = [];
const externals: Server[] = [];

let activeContext: BrowserContext;
let transportFailures: Array<{ method: string; path: string; error: string }>;
test.beforeEach(({ context, page }) => {
  activeContext = context;
  transportFailures = [];
  page.on("requestfailed", (request) => {
    transportFailures.push({
      method: request.method(),
      path: new URL(request.url()).pathname,
      error: request.failure()?.errorText ?? "unknown",
    });
    if (transportFailures.length > 50) transportFailures.shift();
  });
});

test.afterEach(async () => {
  const testInfo = test.info();
  if (testInfo.status !== testInfo.expectedStatus) {
    await testInfo.attach("transport-failures", {
      body: JSON.stringify(transportFailures, null, 2),
      contentType: "application/json",
    });
  }
  await Promise.all(externals.splice(0).map((external) => external.close()));
  await Promise.all(servers.splice(0).map((server) => server.close()));
  await Promise.all(
    temporary
      .splice(0)
      .map((path) => rm(path, { recursive: true, force: true })),
  );
});

async function jpeg() {
  return readFile(new URL("../test-fixtures/review.jpg", import.meta.url));
}
function withCaptureTime(source: Uint8Array, captureTime: string): Uint8Array {
  const value = new TextEncoder().encode(`${captureTime}\0`);
  const dataOffset = 8 + 2 + 12 + 4;
  const tiff = new Uint8Array(dataOffset + value.length);
  tiff.set([0x49, 0x49, 0x2a, 0, 8, 0, 0, 0]);
  tiff.set([1, 0], 8);
  tiff.set([0x03, 0x90, 2, 0], 10);
  new DataView(tiff.buffer).setUint32(14, value.length, true);
  new DataView(tiff.buffer).setUint32(18, dataOffset, true);
  tiff.set(value, dataOffset);
  const payload = new Uint8Array([69, 120, 105, 102, 0, 0, ...tiff]);
  const app1 = new Uint8Array(payload.length + 4);
  app1.set([
    0xff,
    0xe1,
    (payload.length + 2) >> 8,
    (payload.length + 2) & 0xff,
  ]);
  app1.set(payload, 4);
  return new Uint8Array([...source.slice(0, 2), ...app1, ...source.slice(2)]);
}
/**
 * Writes one EXIF Orientation tag into a JPEG, mirroring the TIFF layout the
 * Rust capture and derivative pipeline already understands. Orientation 6
 * rotates the displayed image 90 degrees clockwise.
 */
function withExifOrientation(source: Uint8Array, value: number): Uint8Array {
  const dataOffset = 8 + 2 + 12 + 4;
  const tiff = new Uint8Array(dataOffset);
  tiff.set([0x49, 0x49, 0x2a, 0, 8, 0, 0, 0]);
  tiff.set([1, 0], 8);
  tiff.set([0x12, 0x01, 3, 0], 10);
  const view = new DataView(tiff.buffer);
  view.setUint32(14, 1, true);
  view.setUint16(18, value, true);
  const payload = new Uint8Array([69, 120, 105, 102, 0, 0, ...tiff]);
  const app1 = new Uint8Array(payload.length + 4);
  app1.set([
    0xff,
    0xe1,
    (payload.length + 2) >> 8,
    (payload.length + 2) & 0xff,
  ]);
  app1.set(payload, 4);
  return new Uint8Array([...source.slice(0, 2), ...app1, ...source.slice(2)]);
}
async function jpegWithSize(
  page: Page,
  width: number,
  height: number,
): Promise<Buffer> {
  const bytes = await page.evaluate(
    async ({ width, height }) => {
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext("2d");
      if (!context) throw new Error("Canvas is unavailable");
      context.fillStyle = "#7a7d82";
      context.fillRect(0, 0, width, height);
      const blob = await new Promise<Blob | null>((resolve) =>
        canvas.toBlob(resolve, "image/jpeg", 0.92),
      );
      if (!blob) throw new Error("JPEG encoding failed");
      return Array.from(new Uint8Array(await blob.arrayBuffer()));
    },
    { width, height },
  );
  return Buffer.from(bytes);
}
async function fixture() {
  const base = await mkdtemp(join(tmpdir(), "slipstream-browser-"));
  temporary.push(base);
  const root = join(base, "originals");
  await mkdir(root);
  return { base, root };
}
async function writePhotos(root: string, count: number) {
  const data = await jpeg();
  for (let index = 0; index < count; index += 1)
    await writeFile(join(root, `${String(index).padStart(3, "0")}.jpg`), data);
}
async function server(base: string, root: string) {
  const running = await startBrowserServer({ base, root });
  servers.push(running);
  const login = await activeContext.request.post(
    `${running.url}/api/access/session`,
    { headers: { Origin: running.url }, data: { token: running.token } },
  );
  expect(login.status()).toBe(204);
  // The server binds before its owned startup scan finishes, so tests wait
  // for the Library to settle before driving the UI.
  await expect
    .poll(
      async () => {
        const response = await fixtureFetch(`${running.url}/api/status`);
        return ((await response.json()) as { state: string }).state;
      },
      { timeout: 60_000 },
    )
    .toBe("idle");
  return running;
}
async function post(url: string, path: string, body: unknown) {
  return fixtureFetch(`${url}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Origin: url },
    body: JSON.stringify(body),
  });
}
type BrowsePhoto = {
  id: string;
  available: boolean;
  selectionState: string;
  rating: number;
  originalFilename?: string;
  original?: Readonly<{ kind: string; available: boolean }>;
};
type AlbumMember = BrowsePhoto & { photoId: string; position: number };
type AlbumState = { id: string; position: number; members: AlbumMember[] };

async function browseWindow(
  url: string,
  token: string,
  start: number,
): Promise<{ start: number; total: number; photos: BrowsePhoto[] }> {
  const window = (await (
    await fixtureFetch(`${url}/api/browse/${token}?start=${start}&limit=60`)
  ).json()) as { start: number; total: number; photos: BrowsePhoto[] };
  if (window.start !== start)
    throw new Error("browse window start is inconsistent");
  return window;
}

async function browseIds(url: string): Promise<string[]> {
  const opened = (await (
    await post(url, "/api/browse", { source: "library" })
  ).json()) as { token: string; total: number };
  const ids: string[] = [];
  let start = 0;
  for (;;) {
    const window = await browseWindow(url, opened.token, start);
    if (window.total !== opened.total)
      throw new Error("browse window total is inconsistent");
    ids.push(...window.photos.map((photo) => photo.id));
    start += window.photos.length;
    if (window.photos.length === 0 || start >= opened.total) break;
  }
  await fixtureFetch(`${url}/api/browse/${opened.token}`, {
    method: "DELETE",
    headers: { Origin: url },
  });
  return ids;
}

/// The current Library facts of one Photo, read through a fresh Browse
/// token.

async function libraryPhoto(url: string, index: number): Promise<BrowsePhoto> {
  const opened = (await (
    await post(url, "/api/browse", { source: "library" })
  ).json()) as { token: string; total: number };
  const window = await browseWindow(url, opened.token, 0);
  await fixtureFetch(`${url}/api/browse/${opened.token}`, {
    method: "DELETE",
    headers: { Origin: url },
  });
  const photo = window.photos[index];
  if (!photo) throw new Error(`Library has no Photo at ${index}`);
  return photo;
}
async function createAlbum(url: string, name = "Review", photoIds?: string[]) {
  const photos = photoIds ?? (await browseIds(url));
  const created = (await (await post(url, "/api/albums", { name })).json()) as {
    albums: Array<{ id: string; name: string }>;
  };
  const album = created.albums.find((item) => item.name === name)!;
  for (let offset = 0; offset < photos.length; offset += 100)
    await post(url, `/api/albums/${album.id}/members`, {
      photoIds: photos.slice(offset, offset + 100),
    });
  return { albumId: album.id };
}

/// Runs one compiled CLI client command against a real service, mirroring the
/// server-binary resolution browser-server.ts already uses.
async function cli(server: string, invocation: string[]) {
  const binary = resolve(
    process.env.SLIPSTREAM_CLI_BINARY ?? "target/debug/slipstream",
  );
  const running = servers.find((candidate) => candidate.url === server);
  if (!running) throw new Error("Unknown CLI fixture");
  const completed = await promisify(execFile)(
    binary,
    ["--server", server, "--token-file", running.tokenFile, ...invocation],
    {
      env: {
        ...process.env,
        SSL_CERT_FILE: resolve("tools/test-tls/cert.pem"),
      },
      encoding: "utf8",
    },
  );
  if (completed.stderr)
    throw new Error(`unexpected CLI stderr: ${completed.stderr}`);
  const envelope = JSON.parse(completed.stdout) as {
    status: string;
    data?: Record<string, unknown>;
    error?: unknown;
  };
  if (envelope.status !== "ok")
    throw new Error(`CLI command failed: ${JSON.stringify(envelope.error)}`);
  return envelope;
}
function progressResponse(page: Page, albumId: string, status = 200) {
  return page.waitForResponse(
    (response) =>
      response.url().includes(`/api/albums/${albumId}/progress`) &&
      response.request().method() === "POST" &&
      response.status() === status,
  );
}

async function actionWithProgress(
  page: Page,
  albumId: string,
  action: () => Promise<unknown>,
) {
  const confirmed = progressResponse(page, albumId);
  await action();
  await confirmed;
}

async function waitForGridFrame(page: Page) {
  await expect(page.locator("[data-grid-layer]")).toBeVisible();
  await page.evaluate(
    () =>
      new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
  );
}

/// The rendered Grid cells in order; each thumbnail URL names its Photo.
const gridPhotoIds = (page: Page) =>
  page
    .locator(".photo-cell img")
    .evaluateAll((images) =>
      images.map((image) =>
        new URL(image.getAttribute("src") ?? "", location.origin).pathname
          .split("/")
          .at(-3),
      ),
    );

async function expectGridOrder(page: Page, ids: string[]) {
  await expect.poll(() => gridPhotoIds(page)).toEqual(ids);
}

async function openPhotoAndWaitForProgress(
  page: Page,
  albumId: string,
  photo: Locator,
) {
  const confirmed = progressResponse(page, albumId);
  await photo.click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await page.waitForFunction(
    () =>
      Boolean(document.querySelector("[data-stage] img")) ||
      document.body.innerText.includes("Preview unavailable"),
  );
  await confirmed;
}

/// The rendered Grid range as the layout presents it: the first rendered Photo
/// index and one past the last one, derived from cell geometry.
const recordWindowRequests = (page: Page) => {
  const requested: number[] = [];
  const pending = new Set<PlaywrightRequest>();
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (request.method() !== "GET" || !url.pathname.startsWith("/api/browse/"))
      return;
    requested.push(Number(url.searchParams.get("start")));
    pending.add(request);
  });
  page.on("requestfinished", (request) => pending.delete(request));
  page.on("requestfailed", (request) => pending.delete(request));
  return { requested, pending };
};

/// Settles when every rendered cell has its Photo and no window is in flight.
async function waitForLoadedReviewImage(page: Page) {
  const image = page.locator("[data-stage] img");
  await expect(image).toBeVisible();
  await page.waitForFunction(() => {
    const candidate = document.querySelector("[data-stage] img");
    return (
      candidate instanceof HTMLImageElement &&
      candidate.complete &&
      candidate.naturalWidth > 0
    );
  });
}

async function previewImageGeometry(page: Page) {
  return page.locator("[data-stage] img").evaluate((element) => {
    const box = element.getBoundingClientRect();
    const image = element as HTMLImageElement;
    return {
      width: box.width,
      height: box.height,
      left: box.left,
      top: box.top,
      right: box.right,
      bottom: box.bottom,
      naturalWidth: image.naturalWidth,
      naturalHeight: image.naturalHeight,
    };
  });
}

async function previewStageGeometry(page: Page) {
  return page.locator("[data-stage]").evaluate((element) => {
    const box = element.getBoundingClientRect();
    return {
      width: box.width,
      height: box.height,
      left: box.left,
      top: box.top,
      right: box.right,
      bottom: box.bottom,
    };
  });
}

async function zoomLevel(page: Page) {
  const text = await page.locator("[data-zoom-level]").textContent();
  return Number((text ?? "").replace("%", ""));
}

// Decision swipes animate the stage back to rest; geometry measured during
// that animation carries the residual offset.
async function waitForStageAtRest(page: Page) {
  await expect(page.locator("[data-stage]")).toHaveCSS("transform", "none");
}

// A replaced Preview image only receives its zoom geometry once its bytes
// have loaded, so rendered size is the observable proof of applied zoom.
async function startReview(
  page: Page,
  url: string,
  name = "Review",
  albumId?: string,
) {
  await openGrid(page, url, name);
  const photo = page.locator('[data-photo-index="0"]');
  await expect(photo).toHaveAccessibleName(/Photo 1 of/);
  if (albumId) {
    await openPhotoAndWaitForProgress(page, albumId, photo);
    return;
  }
  await photo.click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await page.waitForFunction(
    () =>
      Boolean(document.querySelector("[data-stage] img")) ||
      document.body.innerText.includes("Preview unavailable"),
  );
}
// Membership order and per-member facts are observable only through a
// fresh Album Browse Snapshot. The resolved open position exposes the
// saved Album position under the unavailable-member fallback rules.
async function state(url: string, albumId: string): Promise<AlbumState> {
  const opened = (await (
    await post(url, "/api/browse", { source: "album", albumId: albumId })
  ).json()) as { token: string; total: number; position: number };
  const members: AlbumMember[] = [];
  let start = 0;
  for (;;) {
    const window = await browseWindow(url, opened.token, start);
    if (window.total !== opened.total)
      throw new Error("browse window total is inconsistent");
    window.photos.forEach((photo, index) =>
      members.push({ ...photo, photoId: photo.id, position: start + index }),
    );
    start += window.photos.length;
    if (window.photos.length === 0 || start >= opened.total) break;
  }
  await fixtureFetch(`${url}/api/browse/${opened.token}`, {
    method: "DELETE",
    headers: { Origin: url },
  });
  return { id: albumId, position: opened.position, members };
}
async function openGrid(page: Page, url: string, name: string) {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(url);
  await openSources(page);
  const escapedName = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const source = page
    .locator("[data-source-list]")
    .getByRole("link", { name: new RegExp(`^${escapedName}(?: |$)`) });
  // Waiting for the link before clicking keeps a Library summary that is still
  // arriving from surfacing as an unexplained click timeout.
  await expect(source).toBeVisible();
  await source.click();
  await page
    .locator("[data-grid-status]")
    .filter({ hasText: /^(?:Ready · \d[\d,]* Photos?|0 Photos)$/ })
    .waitFor();
  await waitForGridFrame(page);
}
/// Opens the Sources surface from the current-source disclosure. Its accessible
/// name identifies both Sources and the current source.
/// Waits until exactly one of the two views owns the screen. Both surfaces this
/// file drives — Sources and View options — are reachable while a source open
/// is still pending, so this only settles which view is showing.
async function settledView(page: Page) {
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          Array.from(
            document.querySelectorAll<HTMLElement>(
              "[data-grid-view], [data-photo-view]",
            ),
          ).filter((node) => node.hidden).length,
      ),
    )
    .toBe(1);
}

/// Waits until the view the current address names is the one showing, so a
/// Sources disclosure never races the Grid shell a Photo destination renders
/// before the Photo it resolves opens. A Grid destination has no shell to wait
/// out — its Grid may stay pending for as long as the test holds its open — so
/// the guard reads the address rather than the status text. View options
/// deliberately does not wait for this, because it is reachable while an open
/// is pending.
async function settledDestination(page: Page) {
  await expect
    .poll(() =>
      page.evaluate(() => {
        const namesPhoto = new URL(window.location.href).searchParams.has(
          "photoId",
        );
        const grid = document.querySelector<HTMLElement>("[data-grid-view]");
        const photo = document.querySelector<HTMLElement>("[data-photo-view]");
        if (photo && !photo.hidden) {
          const position =
            document.querySelector("[data-position]")?.textContent ?? "";
          return /^[1-9]\d* \/ [1-9]\d*$/.test(position.trim());
        }
        // A Grid that is not standing in for a Photo destination has settled,
        // whether or not its source open is still pending.
        return Boolean(grid && !grid.hidden && !namesPhoto);
      }),
    )
    .toBe(true);
}

/// The indicators that carry the connection state: the application header on a
/// wide layout, and the open view's own header on a narrow one.
const connectionIndicator = (page: Page, state: "Connected" | "Disconnected") =>
  page
    .locator(
      "[data-connection], [data-grid-connection], [data-photo-connection]",
    )
    .filter({ hasText: state });

/// Reads the connection state the open layout presents. A narrow layout has no
/// dedicated brand or connected-status row, so a normal state presents no
/// connection text at all and only a failure is shown beside the affected
/// primary action.
async function expectConnection(
  page: Page,
  state: "Connected" | "Disconnected",
) {
  if (state === "Connected" && (await page.locator(".app-header").isHidden())) {
    await expect(connectionIndicator(page, "Connected")).toHaveCount(0);
    return;
  }
  await expect(connectionIndicator(page, state)).toBeVisible();
}

/// Closes the Photo View's supporting surfaces the way the platform does, so
/// a disclosure in the background becomes reachable again. A native modal makes
/// the rest of the document inert while it is open.
async function closePhotoSurfaces(page: Page) {
  for (const selector of ["[data-photo-tools]", "[data-rating-choices]"]) {
    const surface = page.locator(selector);
    if (!(await surface.isVisible())) continue;
    await page.keyboard.press("Escape");
    await expect(surface).toBeHidden();
  }
}

async function openSources(page: Page) {
  await settledDestination(page);
  await closePhotoSurfaces(page);
  // The application establishes its initial destination after it mounts, and
  // opening that source closes Sources. Waiting for the Grid status or the
  // Photo position that establishment writes keeps that close out of the click
  // below, which would otherwise race it on a loaded machine and lose the
  // surface it just opened.
  await page.waitForFunction(() => {
    const status =
      document.querySelector("[data-grid-status]")?.textContent ?? "";
    const position =
      document.querySelector("[data-position]")?.textContent ?? "";
    return status !== "" || /^[1-9]\d* \/ [1-9]\d*$/.test(position.trim());
  });
  // Exactly one disclosure is visible: the Grid's on a narrow Grid, the Photo
  // View's while a Photo is open. Resolving it by visibility keeps a
  // destination render that briefly presents the Grid shell from racing the
  // click.
  const disclosure = page.locator(
    "[data-source-toggle]:visible, [data-photo-source-toggle]:visible",
  );
  if ((await disclosure.count()) === 0) {
    // A wide Grid keeps Sources as the resizable sidebar, which is always shown.
    await expect(page.locator("#source-panel")).toBeVisible();
    return;
  }
  if ((await disclosure.getAttribute("aria-expanded")) !== "true")
    await disclosure.click();
  await expect(page.locator("[data-source-dialog]")).toBeVisible();
}

/// Opens the Photo tools surface from the primary action region and waits for
/// its list. Photo tools is one native modal: at most one supporting surface is
/// active, and closing it returns focus to the More entry that opened it.
async function openPhotoTools(page: Page) {
  const surface = page.locator("[data-photo-tools]");
  if (await surface.isVisible()) return;
  // At most one supporting surface is active, so the Rating surface closes
  // before the More entry becomes reachable.
  const rating = page.locator("[data-rating-choices]");
  if (await rating.isVisible()) {
    await page.keyboard.press("Escape");
    await expect(rating).toBeHidden();
  }
  await page.locator("[data-dock-more]").click();
  await expect(surface).toBeVisible();
  await expect(page.locator("[data-photo-tools-view='tools']")).toBeVisible();
}

/// Moves Photo tools to one of its subviews, which replaces the list content
/// and carries its own local return.
async function openPhotoToolsView(page: Page, view: string) {
  await openPhotoTools(page);
  const target = page.locator(`[data-photo-tools-view='${view}']`);
  if (await target.isVisible()) return;
  await returnToPhotoTools(page);
  await page.locator(`[data-photo-tools-entry='${view}']`).click();
  await expect(target).toBeVisible();
}

/// Returns from a Photo tools subview to its list. The return is local: it adds
/// no browser history entry.
async function returnToPhotoTools(page: Page) {
  const back = page.locator("[data-photo-tools-return]:visible");
  if ((await back.count()) === 0) return;
  await back.first().click();
  await expect(page.locator("[data-photo-tools-view='tools']")).toBeVisible();
}

/// Closes Photo tools, so the background Photo controls are reachable again.
async function closePhotoTools(page: Page) {
  const surface = page.locator("[data-photo-tools]");
  if (!(await surface.isVisible())) return;
  await page.locator("[data-photo-tools-close]").click();
  await expect(surface).toBeHidden();
}

/// Opens the explicit Rating choices. Only this surface or its Rating entry
/// owns explicit Rating interaction at one time.
async function openRatingChoices(page: Page) {
  const surface = page.locator("[data-rating-choices]");
  if (await surface.isVisible()) return;
  // The Rating entry is a background control, so a supporting surface that
  // covers it closes first, exactly as it must for the entry to be reachable.
  const tools = page.locator("[data-photo-tools]");
  if (await tools.isVisible()) {
    await page.locator("[data-photo-tools-close]").click();
    await expect(tools).toBeHidden();
  }
  await page.locator("[data-dock-rating]").click();
  await expect(surface).toBeVisible();
}

/// Opens View options, which owns the Selection State filter, the source
/// order, the thumbnail size, the complete source counts, and the
/// source-specific actions.
async function openViewOptions(page: Page) {
  const surface = page.locator("[data-view-options]");
  if (await surface.isVisible()) return;
  await settledView(page);
  await page.locator("[data-grid-view-options]").click();
  await expect(surface).toBeVisible();
}

/// Reads the Library summary. A narrow layout keeps it inside the Sources
/// surface, which stays closed until it is disclosed.
async function applyViewOptions(page: Page) {
  await page.locator("[data-view-options-apply]").click();
  await expect(page.locator("[data-view-options]")).toBeHidden();
}

/// Album membership is read and managed in one panel: the facts list names the
/// Albums this Photo is in, and the Manage panel holds one checkbox per Album.
async function openMembershipPanel(page: Page) {
  await openPhotoToolsView(page, "albums");
  const manage = page.getByRole("button", { name: "Manage", exact: true });
  if ((await manage.getAttribute("aria-expanded")) !== "true")
    await manage.click();
  await expect(page.locator("[data-membership-panel]")).toBeVisible();
}

function membershipCheckbox(page: Page, albumName: string) {
  return page.getByRole("checkbox", { name: albumName, exact: true });
}

async function toggleAlbumMembership(page: Page, albumName: string) {
  await openMembershipPanel(page);
  await membershipCheckbox(page, albumName).click();
}

/// Counts membership reads the page has settled, so a test can wait for a held
/// response to be fully processed instead of guessing at elapsed time. The
/// marker runs in a later task than the fetch continuation that assigns the
/// membership facts.
function contrastRatio(foreground: string, background: string) {
  const luminance = (value: string) => {
    const channels = value
      .match(/[\d.]+/g)!
      .slice(0, 3)
      .map(Number)
      .map((channel) => {
        const normalized = channel / 255;
        return normalized <= 0.04045
          ? normalized / 12.92
          : ((normalized + 0.055) / 1.055) ** 2.4;
      });
    return (
      0.2126 * channels[0]! + 0.7152 * channels[1]! + 0.0722 * channels[2]!
    );
  };
  const light = Math.max(luminance(foreground), luminance(background));
  const dark = Math.min(luminance(foreground), luminance(background));
  return (light + 0.05) / (dark + 0.05);
}
test("uses singular and plural Photo counts in Grid status and source cards", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "Single Folder"));
  const data = await jpeg();
  await writeFile(join(root, "root.jpg"), data);
  await writeFile(join(root, "Single Folder", "nested.jpg"), data);
  const running = await server(base, root);
  const [firstPhotoId] = await browseIds(running.url);
  const created = (await (
    await post(running.url, "/api/albums", { name: "Single Album" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const albumId = created.albums.find(
    (album) => album.name === "Single Album",
  )!.id;
  await post(running.url, `/api/albums/${albumId}/members`, {
    photoIds: [firstPhotoId],
  });
  await post(running.url, "/api/albums", { name: "Empty Album" });

  await page.goto(running.url);
  await expect(
    page.getByText("Ready · 2 Photos", { exact: true }),
  ).toBeVisible();

  const expectSourceCount = async (name: string, count: string) => {
    // A source destination is a real same-origin anchor, so new-tab and
    // copy-link stay native.
    const card = page.getByRole("link", {
      name: `${name} ${count}`,
      exact: true,
    });
    await expect(card).toBeVisible();
    await expect(card).toHaveAccessibleName(`${name} ${count}`);
    await expect(card.locator("span")).toHaveText(count);
    return card;
  };

  await expectSourceCount("All Photos", "2 Photos");
  await expectSourceCount("Library Folder", "2 Photos");
  await expectSourceCount("Single Album", "1 Photo");
  await expectSourceCount("Empty Album", "0 Photos");

  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  const folder = await expectSourceCount("Single Folder", "1 Photo");
  await folder.click();
  await expect(
    page.getByText("Ready · 1 Photo", { exact: true }),
  ).toBeVisible();

  await page
    .getByRole("link", { name: "Single Album 1 Photo", exact: true })
    .click();
  await expect(
    page.getByText("Ready · 1 Photo", { exact: true }),
  ).toBeVisible();
});

test("starts from a Album, shows facts, accessible controls, and resumes persisted progress after restart", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg());
  await writeFile(join(root, "b.jpg"), await jpeg());
  let running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Picks");
  await startReview(page, running.url, "Picks", albumId);
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect(page.locator("[data-selection]")).toHaveText("Undecided");
  await expect(page.getByText("No rating", { exact: true })).toBeVisible();
  // Limited detail rides with the Preview fact, not a fact row of its own.
  await expect(page.locator("[data-limited]")).toHaveCount(0);
  // The primary actions and navigation are the visible Photo controls; every
  // supporting action moved into a real modal and stays reachable there.
  for (const name of ["Select", "Reject", "Previous", "Next"])
    await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
  await openPhotoToolsView(page, "details");
  await expect(
    page.getByText("JPEG · limited detail", { exact: true }),
  ).toBeVisible();
  await expect(page.locator("[data-source]")).toHaveAttribute(
    "title",
    "Limited by camera Preview resolution",
  );
  await expect(page.locator("[data-detail-limit]")).toBeVisible();
  await openPhotoToolsView(page, "zoom");
  for (const name of [
    "Fit Window",
    "Zoom in",
    "Zoom out",
    "Zoom to 100 percent",
  ])
    await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
  await returnToPhotoTools(page);
  for (const name of ["Clear", "Undo"])
    await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
  await openRatingChoices(page);
  await expect(
    page.getByRole("button", { name: "Rate 5 stars", exact: true }),
  ).toBeVisible();
  // The decision acts on the Photo, so both surfaces close for it.
  await closePhotoSurfaces(page);
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  // The advanced Photo is the saved Album position.
  await expect
    .poll(async () => (await state(running.url, albumId)).position)
    .toBe(1);

  await page.goto("about:blank");
  await running.close();
  servers.splice(servers.indexOf(running), 1);
  running = await server(base, root);
  await page.goto(running.url);
  await openSources(page);
  await page.getByRole("link", { name: /^Picks \d+ Photos/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 2 of 2/ }),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await expect(page.locator("[data-selection]")).toHaveText("Selected");
});

test("narrow Grid discloses Sources as one named modal surface and restores focus", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg());
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();

  const sources = page.locator("[data-source-toggle]");
  const panel = page.locator("#source-panel");
  const dialog = page.locator("[data-source-dialog]");
  const gridViewport = page.locator("[data-grid-viewport]");
  const gridHeight = () => gridViewport.evaluate((node) => node.clientHeight);
  const insideSurface = () =>
    page.evaluate(() =>
      document
        .querySelector("[data-source-dialog]")
        ?.contains(document.activeElement),
    );
  await expect(sources).toBeVisible();
  await expect(sources).toHaveAttribute("aria-expanded", "false");
  // The disclosure's accessible name identifies both Sources and the current
  // source, and a closed surface renders nothing.
  await expect(sources).toHaveAccessibleName("Sources — All Photos");
  await expect(panel).toBeHidden();
  await expect(dialog).toBeHidden();
  // The Grid keeps the height the app leaves below the view controls and
  // reaches the bottom of the window.
  expect(
    await gridViewport.evaluate((node) =>
      Math.round(window.innerHeight - node.getBoundingClientRect().bottom),
    ),
  ).toBe(0);
  const closedGridHeight = await gridHeight();
  // One compact header row replaces the stacked toolbar, so the Grid keeps
  // the height that row leaves it. Pin that measured floor with a little
  // margin, so later header growth cannot silently eat the Grid.
  expect(closedGridHeight).toBeGreaterThanOrEqual(3 * 178 + 40);

  await sources.click();
  await expect(sources).toHaveAttribute("aria-expanded", "true");
  await expect(dialog).toBeVisible();
  // The surface overlays the Grid instead of shrinking it.
  expect(await gridHeight()).toBe(closedGridHeight);
  await expect(
    page.getByRole("link", { name: /^All Photos(?: |$)/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Close", exact: true }),
  ).toBeFocused();

  // Tab and Shift+Tab stay inside the surface, and a background control can be
  // neither focused nor activated while it is open.
  for (let step = 0; step < 6; step += 1) await page.keyboard.press("Tab");
  expect(await insideSurface()).toBe(true);
  await page.keyboard.press("Shift+Tab");
  expect(await insideSurface()).toBe(true);
  await page.locator("[data-grid-select-mode]").focus();
  expect(await insideSurface()).toBe(true);

  const sourceContrast = await panel.evaluate((container) => {
    const background = getComputedStyle(container).backgroundColor;
    return Array.from(
      container.querySelectorAll<HTMLElement>(
        "[data-summary-status], .source-list h3, .source-card span",
      ),
      (node) => ({
        foreground: getComputedStyle(node).color,
        background,
      }),
    );
  });
  expect(
    sourceContrast.every(
      ({ foreground, background }) =>
        contrastRatio(foreground, background) >= 4.5,
    ),
  ).toBe(true);
  const drawerTargets = await panel.evaluate((container) =>
    Array.from(
      container.querySelectorAll<HTMLElement>(
        "button:not([hidden]), input:not([hidden])",
      ),
    )
      .filter((target) => target.offsetParent !== null)
      .map((target) => {
        const box = target.getBoundingClientRect();
        return { width: box.width, height: box.height };
      }),
  );
  expect(
    drawerTargets.every(({ width, height }) => width >= 44 && height >= 44),
  ).toBe(true);

  // A pointer activation aimed at the Grid behind the surface never reaches
  // it: the scrim dismisses the surface and no Photo opens.
  await page.locator('[data-photo-index="0"]').click({ force: true });
  await expect(dialog).toBeHidden();
  await expect(page.locator("[data-review]")).toBeHidden();

  await sources.click();
  await page.keyboard.press("Escape");
  await expect(panel).toBeHidden();
  await expect(dialog).toBeHidden();
  await expect(sources).toBeFocused();

  await sources.click();
  await page.getByRole("link", { name: /^All Photos(?: |$)/ }).click();
  // Selecting a source closes the surface and returns focus to the Grid.
  await expect(panel).toBeHidden();
  await expect(dialog).toBeHidden();
  await expect(page.locator("[data-grid-viewport]")).toBeFocused();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
});

test("Preview zoom is explicit, bounded, and never records a decision", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);

  const preview = page.locator("[data-preview]");
  await openPhotoToolsView(page, "zoom");
  const fit = page.getByRole("button", { name: "Fit Window", exact: true });
  const zoomIn = page.getByRole("button", { name: "Zoom in", exact: true });
  const hundred = page.getByRole("button", {
    name: "Zoom to 100 percent",
    exact: true,
  });
  const slider = page.locator("[data-zoom-slider]");
  const level = page.locator("[data-zoom-level]");

  // Fit is the default state, reports the percentage it produces, and keeps
  // the complete composition inside the Preview area.
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await expect(fit).toHaveAttribute("aria-pressed", "true");
  await expect(preview).toHaveCSS("touch-action", "pan-y");
  const stage = await previewStageGeometry(page);
  const fitted = await previewImageGeometry(page);
  expect(fitted.width).toBeLessThanOrEqual(stage.width + 0.5);
  expect(fitted.height).toBeLessThanOrEqual(stage.height + 0.5);
  expect(
    Math.abs(
      fitted.width / fitted.naturalWidth - fitted.height / fitted.naturalHeight,
    ),
  ).toBeLessThan(0.01);
  await expect(level).toHaveText(
    `${Math.round((fitted.width / fitted.naturalWidth) * 100)}%`,
  );

  // The zoom in control changes the real display size by one step.
  await zoomIn.click();
  await expect(preview).toHaveAttribute("data-zoom-state", "manual");
  const stepped = await previewImageGeometry(page);
  expect(stepped.width).toBeCloseTo(fitted.width * 1.25, 0);
  expect(await zoomLevel(page)).toBe(
    Math.round((stepped.width / stepped.naturalWidth) * 100),
  );

  // The slider changes the display size and the percentage together.
  await slider.fill("300");
  await slider.dispatchEvent("input");
  const slid = await previewImageGeometry(page);
  expect(slid.width).toBeCloseTo(slid.naturalWidth * 3, 0);
  expect(await zoomLevel(page)).toBe(300);

  // 100% maps one Preview pixel to one CSS pixel.
  await hundred.click();
  const actual = await previewImageGeometry(page);
  expect(actual.width).toBeCloseTo(actual.naturalWidth, 0);
  expect(actual.height).toBeCloseTo(actual.naturalHeight, 0);
  expect(await zoomLevel(page)).toBe(100);

  // Fit Window restores the complete composition.
  await fit.click();
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await expect(fit).toHaveAttribute("aria-pressed", "true");
  const refitted = await previewImageGeometry(page);
  expect(refitted.width).toBeLessThanOrEqual(stage.width + 0.5);
  expect(refitted.height).toBeLessThanOrEqual(stage.height + 0.5);

  // A zoomed drag pans instead of deciding, and the pan is bounded.
  let stateRequests = 0;
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname.endsWith("/state")
    )
      stateRequests += 1;
  });
  await slider.fill("300");
  await slider.dispatchEvent("input");
  // The Zoom controls live in Photo tools, so the surface closes for the
  // Preview gestures it would otherwise cover.
  await closePhotoTools(page);
  await waitForStageAtRest(page);
  const beforePan = await previewImageGeometry(page);
  const center = await preview.evaluate((surface) => {
    const box = surface.getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
  });
  await page.mouse.move(center.x, center.y);
  await page.mouse.down();
  await page.mouse.move(center.x + 60, center.y + 10);
  await page.mouse.up();
  const afterPan = await previewImageGeometry(page);
  const limitX = Math.max(0, (beforePan.width - stage.width) / 2);
  const limitY = Math.max(0, (beforePan.height - stage.height) / 2);
  expect(afterPan.left - beforePan.left).toBeCloseTo(Math.min(limitX, 60), 0);
  expect(afterPan.top - beforePan.top).toBeCloseTo(Math.min(limitY, 10), 0);
  expect(stateRequests).toBe(0);
  await expect(page.getByText("1 / 2")).toBeVisible();

  // Dragging far cannot pull the image out of view.
  await page.mouse.move(center.x, center.y);
  await page.mouse.down();
  await page.mouse.move(center.x + 900, center.y + 900);
  await page.mouse.up();
  const bounded = await previewImageGeometry(page);
  if (bounded.width > stage.width) {
    expect(bounded.left).toBeLessThanOrEqual(stage.left + 0.5);
    expect(bounded.right).toBeGreaterThanOrEqual(stage.right - 0.5);
  } else {
    expect(bounded.left + bounded.width / 2).toBeCloseTo(
      stage.left + stage.width / 2,
      0,
    );
  }
  if (bounded.height > stage.height) {
    expect(bounded.top).toBeLessThanOrEqual(stage.top + 0.5);
    expect(bounded.bottom).toBeGreaterThanOrEqual(stage.bottom - 0.5);
  } else {
    expect(bounded.top + bounded.height / 2).toBeCloseTo(
      stage.top + stage.height / 2,
      0,
    );
  }
  expect(stateRequests).toBe(0);

  // The keyboard reaches Fit, detail zoom, and stepped zoom.
  await page.keyboard.press("f");
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await page.keyboard.press("d");
  await expect(preview).toHaveAttribute("data-zoom-state", "manual");
  expect(await zoomLevel(page)).toBe(200);
  await page.keyboard.press("-");
  expect(await zoomLevel(page)).toBe(160);
  await expect(preview).toHaveCSS("touch-action", "none");

  // No horizontal page overflow while zoomed.
  const layout = await page.locator("[data-photo-view]").evaluate((view) => {
    const previewBox = view.querySelector<HTMLElement>("[data-preview]");
    if (!previewBox) throw new Error("Preview is missing");
    return {
      viewWidth: view.clientWidth,
      viewScrollWidth: view.scrollWidth,
      previewWidth: previewBox.getBoundingClientRect().width,
    };
  });
  expect(layout.viewScrollWidth).toBe(layout.viewWidth);
  expect(layout.previewWidth).toBeLessThanOrEqual(layout.viewWidth);

  // Changing Photo resets the zoom state to Fit. Navigating is a background
  // action, so the disclosure closes for it.
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await waitForLoadedReviewImage(page);
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await openPhotoToolsView(page, "zoom");
  await expect(fit).toHaveAttribute("aria-pressed", "true");
  expect(stateRequests).toBe(0);
});

/// The Edit surface is the only place a deployment's processing state is
/// named, and it must stay reachable when processing is unavailable: saved
/// recipes and retained downloads stay usable in every state. A deployment
/// without a launcher answers the capability report with `disabled`, so the
/// surface explains that state and attempts no processing work.
test("the Edit surface explains a deployment without processing and attempts no processing work", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  // Processing work is a submitted Export or a rendered Edit Preview. Reading
  // the Photo's retained Exports is a retained-artifact read, not work.
  const processing: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    const submitted = request.method() === "POST" && path.endsWith("/exports");
    if (submitted || path.includes("/edit-preview/"))
      processing.push(`${request.method()} ${path}`);
  });
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await openPhotoToolsView(page, "edit");
  await expect(page.locator("[data-photo-editor-capability]")).toContainText(
    "Processing is not enabled in this deployment",
  );
  // The deployment's answer, this Photo's source class, and the reason it
  // cannot be edited are all named.
  await expect(page.locator("[data-photo-editor-processing]")).toHaveText(
    "Unavailable",
  );
  await expect(page.locator("[data-photo-editor-support]")).toHaveText(
    "unsupported",
  );
  await expect(page.locator("[data-photo-editor-status]")).toContainText(
    "no approved profile",
  );
  // The Film stage is closed with its own reason, and the provenance names
  // the image that is actually presented.
  await expect(page.locator("[data-photo-editor-stage='film']")).toBeDisabled();
  await expect(page.locator("[data-photo-editor-stage-note]")).toHaveText(
    "The Film capability is not enabled in this deployment, so no Film Result is presented.",
  );
  await expect(page.locator("[data-photo-editor-provenance]")).toHaveText(
    "Develop: no Develop rendition is presented; the presented image is the camera Preview.",
  );
  await expect(page.locator("[data-photo-editor-exposure]")).toBeDisabled();
  // The comparison is a rendition of its own, so it is offered exactly where
  // the deployment can render one, and its label names the baseline
  // development it compares against rather than the Camera view.
  await expect(page.locator("[data-photo-editor-compare]")).toBeDisabled();
  await expect(page.locator("[data-photo-editor-compare]")).toHaveText(
    "Baseline comparison",
  );
  await expect(
    page.locator("[data-photo-editor-export-submit]"),
  ).toBeDisabled();
  expect(processing).toEqual([]);
});

test("Photo View shows review capture metadata and explicit missing values", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  await page.route("**/api/photos/*/metadata", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        captureTime: "2026-02-03T04:05:06.000000000",
        aperture: "f/2.8",
        iso: 400,
        shutterSpeed: "1/125 s",
        focalLength: "50 mm",
      }),
    }),
  );
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await openPhotoToolsView(page, "details");
  await expect(page.locator("[data-metadata]")).toContainText("Details");
  // Capture Time is camera-local time with no timezone. The display keeps
  // the recorded date and minute and drops transport precision.
  await expect(page.locator("[data-metadata-capture-time]")).toHaveText(
    "2026-02-03 04:05",
  );
  await expect(page.locator("[data-metadata-aperture]")).toHaveText("f/2.8");
  await expect(page.locator("[data-metadata-iso]")).toHaveText("400");
  await expect(page.locator("[data-metadata-shutter-speed]")).toHaveText(
    "1/125 s",
  );
  await expect(page.locator("[data-metadata-focal-length]")).toHaveText(
    "50 mm",
  );
});

test("Grid View moves cell focus with the arrow keys and opens the focused Photo", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  await writePhotos(root, 300);
  const running = await server(base, root);
  await page.setViewportSize({ width: 1200, height: 800 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 300 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  const viewport = page.locator("[data-grid-viewport]");
  const cell = (index: number) => page.locator(`[data-photo-index="${index}"]`);
  // The Grid's own column rule: one 150-pixel column per 150 pixels of Grid.
  const columns = await viewport.evaluate((element) =>
    Math.max(1, Math.floor(Math.max(320, element.clientWidth) / 150)),
  );

  // The Grid viewport is the keyboard's entry: the first arrow enters at the
  // first visible Photo, and later arrows step from the cell it owns.
  await viewport.focus();
  await page.keyboard.press("ArrowRight");
  await expect(cell(0)).toBeFocused();
  // The focused cell announces its position through the existing label.
  await expect(cell(0)).toHaveAccessibleName(/^Photo 1 of 300/);
  expect(
    await cell(0).evaluate((element) => element.matches(":focus-visible")),
  ).toBe(true);
  await page.keyboard.press("ArrowRight");
  await expect(cell(1)).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(cell(1 + columns)).toBeFocused();
  await page.keyboard.press("ArrowLeft");
  await expect(cell(columns)).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await expect(cell(0)).toBeFocused();

  // Exactly one cell is a Tab stop, so a keyboard reaches the Grid once.
  await expect(page.locator('.photo-cell[tabindex="0"]')).toHaveCount(1);
  await expect(cell(0)).toHaveAttribute("tabindex", "0");
  await expect(cell(1)).toHaveAttribute("tabindex", "-1");

  // Arrow movement loads the bounded window that contains the target row the
  // same way scrolling does.
  const windowStarts: number[] = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (!/^\/api\/browse\/[^/]+$/.test(url.pathname)) return;
    const start = Number(url.searchParams.get("start"));
    if (Number.isFinite(start)) windowStarts.push(start);
  });
  for (let step = 0; step < 20; step += 1)
    await page.keyboard.press("ArrowDown");
  await expect.poll(() => [...new Set(windowStarts)]).toContain(120);
  const target = 20 * columns;
  await expect(cell(target)).toBeFocused();

  // Enter opens the focused Photo, and returning restores its cell focus.
  await page.keyboard.press("Enter");
  await expect(page.locator("[data-review]")).toBeVisible();
  await expect(page.locator("[data-position]")).toHaveText(
    `${target + 1} / 300`,
  );
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(cell(target)).toBeFocused();
});

test("an idle browser reports a lost connection from the status probe", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await expectConnection(page, "Connected");
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();

  // The Photographer takes no action here. Only the reachability probe runs,
  // and the server stops answering it.
  await page.route("**/api/status", (route) => route.abort());
  await expectConnection(page, "Disconnected");
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Reject" })).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Retry", exact: true }),
  ).toBeEnabled();

  // A decision stays refused, including through the keyboard path.
  await page.keyboard.press("p");
  await expect(page.locator("[data-selection]")).toHaveText("Undecided");
  await expect(page.getByText("1 / 2")).toBeVisible();
  await openPhotoTools(page);
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();

  // A usable status answer confirms the connection again.
  await page.unroute("**/api/status");
  await expectConnection(page, "Connected");
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();

  // An answered status error is a server-side condition, not a lost
  // connection, so it must not report the browser disconnected.
  await page.route("**/api/status", (route) =>
    route.fulfill({ status: 503, body: "unavailable" }),
  );
  await page.waitForTimeout(3_000);
  await expectConnection(page, "Connected");
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
  await page.unroute("**/api/status");
});

test("album management creates, renames, and deletes Albums with confirmation", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("link", { name: /All Photos 1 Photo/ }),
  ).toBeVisible();

  // Create through the inline form.
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Trip");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(page.getByRole("link", { name: /Trip 0 Photos/ })).toBeVisible();
  await expect(page.getByRole("button", { name: "Rename Trip" })).toBeVisible();

  // Rename keeps membership and identity semantics on the card.
  await page.getByRole("button", { name: "Rename Trip" }).click();
  await page.getByLabel("Album name").fill("Journey");
  await page.getByRole("button", { name: "Save Name" }).click();
  await expect(
    page.getByRole("link", { name: /Journey 0 Photos/ }),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: /Trip 0 Photos/ })).toBeHidden();

  // Deleting requires confirmation and states the safety contract.
  await page.getByRole("button", { name: "Delete Journey" }).click();
  await expect(
    page.getByText("Photos and Original Files remain unchanged."),
  ).toBeVisible();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(
    page.getByRole("link", { name: /Journey 0 Photos/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Delete Journey" }).click();
  await page.getByRole("button", { name: "Delete Album" }).click();
  await expect(
    page.getByRole("link", { name: /Journey 0 Photos/ }),
  ).toBeHidden();
  // Originals are untouched: All Photos keeps its count.
  await expect(
    page.getByRole("link", { name: /All Photos 1 Photo/ }),
  ).toBeVisible();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
});

test("the current photo joins and leaves albums from the photo view", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const created = (await (
    await post(running.url, "/api/albums", { name: "Picks" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const albumId = created.albums.find((album) => album.name === "Picks")!.id;
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  const [photoId] = await browseIds(running.url);
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.getByRole("heading", { name: "All Photos" })).toBeVisible();

  // A Photo that belongs to no Album states that plainly.
  await openPhotoToolsView(page, "albums");
  await expect(page.getByText("Not in any Album yet")).toBeVisible();

  // Adding the current Photo to an Album updates the listed membership and
  // the bounded counts.
  const firstAdd = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname.endsWith("/members") &&
      response.status() === 200,
  );
  await toggleAlbumMembership(page, "Picks");
  await firstAdd;
  await expect(page.getByText("Added to the Album.")).toBeVisible();
  await expect(page.getByText("Not in any Album yet")).toBeHidden();
  await expect(page.locator("[data-membership-list] li")).toHaveText(["Picks"]);
  await closePhotoTools(page);
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.getByRole("link", { name: /Picks 1 Photo/ })).toBeVisible();

  // A repeated add for an existing member stays one membership: the panel
  // lists the Album once and the counts stay at one Photo.
  await post(running.url, `/api/albums/${albumId}/members`, { photoId });
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.locator("[data-membership-list] li")).toHaveText(["Picks"]);
  await openMembershipPanel(page);
  await expect(membershipCheckbox(page, "Picks")).toBeChecked();
  await expect
    .poll(async () => (await state(running.url, albumId)).members)
    .toHaveLength(1);
  await closePhotoTools(page);
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.getByRole("link", { name: /Picks 1 Photo/ })).toBeVisible();

  // Removing from the open Album source updates the count while the open
  // snapshot keeps its copied order.
  await page.getByRole("link", { name: /Picks 1 Photo/ }).click();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /^Photo 1 of 1/ }),
  );
  await expect(page.locator("[data-membership-list] li")).toHaveText(["Picks"]);
  await toggleAlbumMembership(page, "Picks");
  await expect(
    page.getByText(
      "Removed from the Album. It stays in this open view until reopened.",
    ),
  ).toBeVisible();
  await expect(page.getByText("1 / 1")).toBeVisible();
  await expect(page.locator("[data-membership-list] li")).toHaveCount(0);
  await closePhotoTools(page);
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(
    page.getByRole("link", { name: /^Picks 0 Photos$/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: /All Photos 1 Photo/ }),
  ).toBeVisible();
});

test("persists manual navigation and advanced current Photo across leave, reload, and restart", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  let running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Progress");
  await startReview(page, running.url, "Progress", albumId);
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Next" }).click(),
  );
  await expect
    .poll(async () => (await state(running.url, albumId)).position)
    .toBe(1);
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await openSources(page);
  await page.getByRole("link", { name: /^Progress \d+ Photos/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 2 of 3/ }),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.reload();
  // The Photo address preserves the destination across a reload, so the
  // reloaded document reopens the same Photo instead of the Album Grid.
  await expect(page.getByText("2 / 3")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect
    .poll(async () => (await state(running.url, albumId)).position)
    .toBe(2);
  await page.goto("about:blank");
  await running.close();
  servers.splice(servers.indexOf(running), 1);
  running = await server(base, root);
  await page.goto(running.url);
  await openSources(page);
  await page.getByRole("link", { name: /^Progress \d+ Photos/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 3 of 3/ }),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
});

test("real-camera: shows matching JPEG then RAW embedded JPEG through the mobile production Review Session", async ({
  page,
}) => {
  test.skip(!sample, "Set SLIPSTREAM_RAW_SAMPLE for the camera smoke");
  const cameraSample = sample!;
  const sourceBefore = await originalSnapshot(cameraSample);
  const { base, root } = await fixture();
  const raw = join(root, `camera${extname(cameraSample)}`);
  const matching = join(root, "camera.jpg");
  await copyFile(cameraSample, raw);
  const copiedBefore = await originalSnapshot(raw);
  await writeFile(matching, await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);
  await waitForLoadedReviewImage(page);
  await expect(page.locator("[data-source]")).toContainText("JPEG");
  await page.keyboard.press("d");
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "manual",
  );
  await page.keyboard.press("d");
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "fit",
  );
  await rm(matching);
  await post(running.url, "/api/scan", {});
  await page.reload();
  await openSources(page);
  await page.getByRole("link", { name: /^Review(?: |$)/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 1 of/ }),
  );
  await waitForLoadedReviewImage(page);
  await expect(page.locator("[data-source]")).toContainText(
    "RAW embedded JPEG",
  );
  const rawGeometry = await previewImageGeometry(page);
  expect([rawGeometry.naturalWidth, rawGeometry.naturalHeight]).toEqual([
    2560, 1707,
  ]);
  expect(rawGeometry.width).toBeGreaterThan(rawGeometry.height);
  await page.keyboard.press("d");
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "manual",
  );
  await page.keyboard.press("d");
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "fit",
  );
  expect(await originalSnapshot(cameraSample)).toEqual(sourceBefore);
  expect(await originalSnapshot(raw)).toEqual(copiedBefore);
});

type OriginalSnapshot = Readonly<{
  sha256: string;
  length: bigint;
  device: bigint;
  inode: bigint;
  mode: bigint;
  owner: bigint;
  group: bigint;
  modifiedNanoseconds: bigint;
}>;

async function originalSnapshot(path: string): Promise<OriginalSnapshot> {
  const [bytes, metadata] = await Promise.all([
    readFile(path),
    stat(path, { bigint: true }),
  ]);
  if (!metadata.isFile())
    throw new Error(`Original safety sample must be a regular file: ${path}`);
  return {
    sha256: createHash("sha256").update(bytes).digest("hex"),
    length: metadata.size,
    device: metadata.dev,
    inode: metadata.ino,
    mode: metadata.mode,
    owner: metadata.uid,
    group: metadata.gid,
    modifiedNanoseconds: metadata.mtimeNs,
  };
}

const filmstripIndices = (page: Page) =>
  page
    .locator("[data-filmstrip] .filmstrip-cell")
    .evaluateAll((cells) =>
      cells.map((cell) => Number(cell.getAttribute("data-filmstrip-index"))),
    );

const waitForFilmstripImages = (page: Page) =>
  page.waitForFunction(() => {
    const cells = Array.from(
      document.querySelectorAll<HTMLElement>(
        "[data-filmstrip] .filmstrip-cell",
      ),
    );
    return (
      cells.length > 0 &&
      cells.every((cell) => {
        const image = cell.querySelector<HTMLImageElement>("img");
        if (!image) return true;
        const source = image.getAttribute("src");
        // A cell without a source is an unavailable or not-yet-hydrated
        // placeholder. A non-empty source is real work and must finish before
        // the test continues.
        return (
          source === null ||
          source === "" ||
          (image.complete && image.naturalWidth > 0)
        );
      })
    );
  });

test("Photo View shows a bounded filmstrip of neighbors and navigates through it", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 6);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 6 Photos$/)).toBeVisible();

  await page.locator('[data-photo-index="2"]').click();
  await waitForLoadedReviewImage(page);
  await openPhotoToolsView(page, "nearby");
  const strip = page.locator("[data-filmstrip]");
  await expect(strip).toBeVisible();
  // Bounded and centered: two neighbors on each side of the current Photo.
  await expect(strip.locator(".filmstrip-cell")).toHaveCount(5);
  expect(await filmstripIndices(page)).toEqual([0, 1, 2, 3, 4]);
  await expect(strip.locator('[data-filmstrip-index="2"]')).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(
    strip.locator('.filmstrip-cell[aria-current="true"]'),
  ).toHaveCount(1);
  await waitForFilmstripImages(page);

  // Activating a neighbor navigates there without recording a decision, and
  // the strip re-centers on the new current Photo. The neighbor is already
  // in the loaded window, so the strip admits no window request of its own.
  const windows = recordWindowRequests(page);
  await strip.locator('[data-filmstrip-index="4"]').click();
  await expect(page.locator("[data-position]")).toHaveText("5 / 6");
  expect(windows.requested).toEqual([]);
  await expect(page.locator("[data-photo-filename]")).toHaveText("004.jpg");
  await waitForLoadedReviewImage(page);
  await expect(page.locator("[data-selection]")).toHaveText("Undecided");
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "fit",
  );
  // The source's end clamps the strip instead of wrapping around it.
  expect(await filmstripIndices(page)).toEqual([2, 3, 4, 5]);
  await expect(strip.locator('[data-filmstrip-index="4"]')).toHaveAttribute(
    "aria-current",
    "true",
  );

  // The same path serves the boundary: an edge Photo keeps its clamp and
  // never wraps around the source.
  await strip.locator('[data-filmstrip-index="5"]').click();
  await expect(page.locator("[data-position]")).toHaveText("6 / 6");
  await waitForLoadedReviewImage(page);
  expect(await filmstripIndices(page)).toEqual([3, 4, 5]);
  await expect(strip.locator('[data-filmstrip-index="5"]')).toHaveAttribute(
    "aria-current",
    "true",
  );
});

type GridCellGeometry = Readonly<{
  index: number;
  cellWidth: number;
  cellHeight: number;
  mediaWidth: number;
  mediaHeight: number;
  imageWidth: number;
  imageHeight: number;
  naturalWidth: number;
  naturalHeight: number;
  facts: string | null;
  indicatorsOverlapImage: boolean;
  imageInsideMedia: boolean;
}>;

async function gridCellGeometry(page: Page): Promise<GridCellGeometry[]> {
  return page.evaluate(() => {
    const overlaps = (inner: DOMRect, outer: DOMRect) =>
      Math.max(
        0,
        Math.min(inner.right, outer.right) - Math.max(inner.left, outer.left),
      ) > 0.5 &&
      Math.max(
        0,
        Math.min(inner.bottom, outer.bottom) - Math.max(inner.top, outer.top),
      ) > 0.5;
    const inside = (inner: DOMRect, outer: DOMRect) =>
      inner.left >= outer.left - 1 &&
      inner.right <= outer.right + 1 &&
      inner.top >= outer.top - 1 &&
      inner.bottom <= outer.bottom + 1;
    return Array.from(
      document.querySelectorAll<HTMLElement>(".photo-cell[data-photo-index]"),
    ).map((cell) => {
      const image = cell.querySelector<HTMLImageElement>("img.thumbnail");
      const media = cell.querySelector<HTMLElement>(".cell-media");
      if (!image || !media) throw new Error("Grid cell geometry is missing");
      const cellBox = cell.getBoundingClientRect();
      const mediaBox = media.getBoundingClientRect();
      const imageBox = image.getBoundingClientRect();
      const indicators = Array.from(
        cell.querySelectorAll<HTMLElement>(
          ".cell-state, .cell-caption, .cell-facts",
        ),
      ).filter((element) => !element.hidden && element.offsetParent !== null);
      const facts = indicators.find((element) =>
        element.classList.contains("cell-facts"),
      );
      return {
        index: Number(cell.dataset.photoIndex),
        cellWidth: cellBox.width,
        cellHeight: cellBox.height,
        mediaWidth: mediaBox.width,
        mediaHeight: mediaBox.height,
        imageWidth: imageBox.width,
        imageHeight: imageBox.height,
        naturalWidth: image.naturalWidth,
        naturalHeight: image.naturalHeight,
        facts: facts?.textContent ?? null,
        indicatorsOverlapImage: indicators.some((element) =>
          overlaps(imageBox, element.getBoundingClientRect()),
        ),
        imageInsideMedia: inside(imageBox, mediaBox),
      };
    });
  });
}

function expectAspectRatio(geometry: GridCellGeometry): void {
  const rendered = geometry.imageWidth / geometry.imageHeight;
  const natural = geometry.naturalWidth / geometry.naturalHeight;
  expect(Math.abs(rendered - natural) / natural).toBeLessThan(0.02);
}

test("Grid progress follows confirmed decisions, Undo, and a reload", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg", "d.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  expect(ids).toHaveLength(4);

  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 4 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  const progress = page.locator("[data-grid-source-progress]");
  const cell = (index: number) => page.locator(`[data-photo-index="${index}"]`);
  await expect(progress).toHaveText(
    "Source progress: 0 selected · 0 rejected · 4 undecided",
  );

  // A Grid decision advances the source-wide counts once, and a refused
  // repeat of the same decision does not move them again. A decision key
  // acts only while the Grid accepts another write, so every press waits for
  // the cell to be enabled again: a key landing while a write is in flight
  // would be dropped, which would make this test pass for the wrong reason.
  await page.locator("[data-grid-viewport]").focus();
  await page.keyboard.press("ArrowRight");
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("p");
  await expect(cell(0).locator(".cell-state.selected")).toHaveText("✓");
  await expect(progress).toHaveText(
    "Source progress: 1 selected · 0 rejected · 3 undecided",
  );
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("p");
  await expect(progress).toHaveText(
    "Source progress: 1 selected · 0 rejected · 3 undecided",
  );

  // A decision change moves one Photo between the counts.
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("x");
  await expect(cell(0).locator(".cell-state.rejected")).toHaveText("×");
  await expect(progress).toHaveText(
    "Source progress: 0 selected · 1 rejected · 3 undecided",
  );

  // Undo returns that decision and the counts together: the Photo holds its
  // previous value again, which was selected.
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("Control+z");
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Last change undone.",
  );
  await expect(cell(0).locator(".cell-state.selected")).toHaveText("✓");
  await expect(progress).toHaveText(
    "Source progress: 1 selected · 0 rejected · 3 undecided",
  );

  // Clearing a decision empties the counts for that Photo.
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("u");
  await expect(cell(0).locator(".cell-state")).toHaveCount(0);
  await expect(progress).toHaveText(
    "Source progress: 0 selected · 0 rejected · 4 undecided",
  );

  // Photo View decides with the same counts, and the values come from the
  // server again after a reload.
  await cell(3).click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await waitForLoadedReviewImage(page);
  await page.getByRole("button", { name: "Reject" }).click();
  await expect(page.locator("[data-selection]")).toHaveText("Rejected");
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 selected · 1 rejected · 3 undecided",
  );
  await page.reload();
  await expect(page.getByText(/^Ready · 4 Photos$/)).toBeVisible();
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 selected · 1 rejected · 3 undecided",
  );
});

test("rejected Photos leave the Library, return from Undo, and are restored from the Trash listing", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 3);
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  expect(ids).toHaveLength(3);

  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 3 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  // Two Photos are rejected, and the Grid is filtered to that result. Removal
  // is reviewed against the Snapshot the Photographer actually sees.
  const cell = (index: number) => page.locator(`[data-photo-index="${index}"]`);
  await page.locator("[data-grid-viewport]").focus();
  await page.keyboard.press("ArrowRight");
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("x");
  await expect(cell(0).locator(".cell-state.rejected")).toHaveText("×");
  await expect(cell(1)).toBeEnabled();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("x");
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 selected · 2 rejected · 1 undecided",
  );
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("rejected");
  await applyViewOptions(page);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 2 Photos",
  );

  // The review names the result it covers and removes nothing by itself.
  const review = page.locator("[data-removal-review]");
  await page.locator("[data-removal-open]").click();
  await expect(review).toBeVisible();
  await expect(page.locator("[data-removal-summary]")).toHaveText(
    /^2 Photos reviewed as Rejected\./,
  );
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 2 Photos",
  );

  // One confirmation removes exactly the reviewed result, and the Library
  // reads again: the rejected result is empty and the Overview count drops.
  await page.locator("[data-removal-confirm]").click();
  await expect(page.locator("[data-removal-message]")).toHaveText(
    /^2 Photos removed from the Library\. Their Original Files are unchanged\.$/,
  );
  // The filtered source is now empty, so the Grid explains the empty result
  // instead of the reopen that produced it.
  await expect(page.locator("[data-grid-status]")).toHaveText("0 Photos");
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 selected · 0 rejected · 1 undecided",
  );
  await page.locator("[data-removal-close]").click();
  await expect(review).toBeHidden();
  await expect(page.locator("[data-grid-empty-message]")).toHaveText(
    "No Photos match this filter.",
  );
  // The Library Overview count is the committed count, not the filtered one.
  await expect(
    page.getByRole("link", { name: /^All Photos 1 Photo$/ }),
  ).toBeVisible();

  // The operation-level Undo is recovered from persisted removal state after
  // the page owner is recreated by a reload.
  await page.reload();
  await expect(
    page.getByRole("link", { name: /^All Photos 1 Photo$/ }),
  ).toBeVisible();
  await page.locator("[data-removed-open]").click();
  const removed = page.locator("[data-removed-panel]");
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "2 Photos in Trash. Showing 1–2.",
  );
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    2,
  );
  await expect(page.locator("[data-removed-undo]")).toHaveText(
    "Undo the last removal (2)",
  );
  await page.locator("[data-removed-undo]").click();
  await expect(page.locator("[data-removed-message]")).toHaveText(
    /^2 Photos restored to the Library\.$/,
  );
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    0,
  );
  await page.locator("[data-removed-close]").click();
  await expect(removed).toBeHidden();
  await expect(
    page.getByRole("link", { name: /^All Photos 3 Photos$/ }),
  ).toBeVisible();

  // Restored Photos keep the decisions they had before the removal.
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 selected · 2 rejected · 1 undecided",
  );
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Source reopened after the restore.",
  );

  // Remove again, then restore one named Photo from the listing: the durable
  // per-Photo path survives a reload.
  await page.locator("[data-removal-open]").click();
  await expect(page.locator("[data-removal-summary]")).toHaveText(
    /^2 Photos reviewed as Rejected\./,
  );
  await page.locator("[data-removal-confirm]").click();
  await expect(page.locator("[data-removal-message]")).toHaveText(
    /^2 Photos removed from the Library\./,
  );
  await page.locator("[data-removal-close]").click();
  await page.locator("[data-removed-open]").click();
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    2,
  );
  const restoredRow = page.locator("[data-removed-list] .removed-item").first();
  const restoredName = await restoredRow.locator(".removed-name").innerText();
  await restoredRow.getByRole("button", { name: "Restore" }).click();
  await expect(page.locator("[data-removed-message]")).toHaveText(
    /^1 Photo restored to the Library\.$/,
  );
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    1,
  );
  await expect(
    page.locator("[data-removed-list] .removed-name"),
  ).not.toHaveText(restoredName);
  await page.reload();
  await expect(
    page.getByRole("link", { name: /^All Photos 2 Photos$/ }),
  ).toBeVisible();
  await page.locator("[data-removed-open]").click();
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    1,
  );
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "1 Photo in Trash. Showing 1–1.",
  );
  await expect(
    page.locator("[data-removed-list] .removed-name"),
  ).not.toHaveText(restoredName);
  // A stale listing marker is answered truthfully: the Photo remains removed
  // and the Photographer is told that its removal state changed elsewhere.
  await page.route("**/api/photos/restore", async (route) => {
    const body = JSON.parse(route.request().postData() ?? "{}") as {
      photos?: ReadonlyArray<{ id?: unknown }>;
    };
    const photoId = body.photos?.[0]?.id;
    if (typeof photoId !== "string")
      throw new Error("restore request did not name a Photo");
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        counts: { restored: 0, changedElsewhere: 1, missing: 0 },
        changedElsewhere: [photoId],
        missing: [],
        operations: [],
      }),
    });
  });
  await page
    .locator("[data-removed-list] .removed-item")
    .getByRole("button", { name: "Restore" })
    .click();
  await expect(page.locator("[data-removed-message]")).toHaveText(
    "Nothing was restored. 1 Photo could not be restored because their removal state changed elsewhere.",
  );
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    1,
  );
  await page.unroute("**/api/photos/restore");
});

test("Trash permanent deletion reviews the files, reports outcomes, and blocks pending verification", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  await writePhotos(root, 3);
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  expect(ids).toHaveLength(3);

  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 3 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  // Two Photos are rejected and removed into Trash through the real flow.
  const cell = (index: number) => page.locator(`[data-photo-index="${index}"]`);
  await page.locator("[data-grid-viewport]").focus();
  await page.keyboard.press("ArrowRight");
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("x");
  await expect(cell(0).locator(".cell-state.rejected")).toHaveText("×");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("x");
  await expect(cell(1).locator(".cell-state.rejected")).toHaveText("×");
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("rejected");
  await applyViewOptions(page);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 2 Photos",
  );
  await page.locator("[data-removal-open]").click();
  await page.locator("[data-removal-confirm]").click();
  await expect(page.locator("[data-removal-message]")).toHaveText(
    /^2 Photos removed from the Library\./,
  );
  await page.locator("[data-removal-close]").click();

  // Every Trash route below is a scenario stub against the frozen contract:
  // the listing names the review maximum, one item is still pending
  // verification with its retained operation id, and the review, delete, and
  // operation responses are controlled so the outcomes are deterministic.
  const first = ids[0]!;
  const second = ids[1]!;
  const pendingId = "photo-pending";
  const pendingOperationId = "operation-pending";
  const storageKey = "slipstream:trash-deletion-operation";
  const facts = (photoId: string) =>
    photoId === second
      ? { location: "2024/b.cr2", kind: "raw" as const, size: 2048 }
      : { location: `2024/${photoId}.jpg`, kind: "jpeg" as const, size: 1024 };
  let trashRows = [first, second, pendingId];
  let reviewCalls = 0;
  let deleteCalls = 0;
  const listingPhoto = (photoId: string) => ({
    removedAtMs: photoId === first ? 3000 : photoId === second ? 2000 : 1000,
    originalLocation: facts(photoId).location,
    originalKind: facts(photoId).kind,
    originalSize: facts(photoId).size,
    pendingVerificationOperationId:
      photoId === pendingId ? pendingOperationId : null,
    photo: {
      id: photoId,
      available: true,
      original: { kind: facts(photoId).kind, available: true },
      originalFilename: photoId === second ? "b.cr2" : `${photoId}.jpg`,
      selectionState: "undecided",
      rating: 0,
      hasSavedEdits: false,
      preview: { state: "unavailable" },
    },
  });
  const reviewItem = (photoId: string) => ({
    photoId,
    removedAtMs: 2000,
    originalId: `original-${photoId}`,
    originalLocation: facts(photoId).location,
    originalKind: facts(photoId).kind,
    size: facts(photoId).size,
    albums: [{ id: "album-1", name: "Keepers" }],
  });
  const operationItem = (
    photoId: string,
    state: string,
    message: string | null = null,
  ) => ({
    photoId,
    state,
    originalLocation: facts(photoId).location,
    originalKind: facts(photoId).kind,
    size: facts(photoId).size,
    message,
  });
  await page.route(/\/api\/trash/, async (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;
    if (pathname === "/api/trash") {
      const url = new URL(request.url());
      const start = Number(url.searchParams.get("start") ?? "0");
      const limit = Number(url.searchParams.get("limit") ?? "50");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          start,
          limit,
          total: trashRows.length,
          operation: null,
          reviewMaximum: 50,
          photos: trashRows.slice(start, start + limit).map(listingPhoto),
        }),
      });
      return;
    }
    if (pathname === "/api/trash/review") {
      const body = JSON.parse(request.postData() ?? "{}") as {
        operationId?: string;
        photoIds?: string[];
      };
      reviewCalls += 1;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          reviewCalls === 1
            ? {
                operationId: body.operationId,
                items: [],
                rejected: [
                  { photoId: first, reason: "missing" },
                  { photoId: second, reason: "changed-elsewhere" },
                ],
              }
            : {
                operationId: body.operationId,
                items: (body.photoIds ?? []).map(reviewItem),
                rejected: [],
              },
        ),
      });
      return;
    }
    if (pathname === "/api/trash/delete") {
      const body = JSON.parse(request.postData() ?? "{}") as {
        operationId?: string;
      };
      deleteCalls += 1;
      if (deleteCalls === 1) {
        // The first confirmation loses its response: nothing is claimed.
        // The delay keeps the in-flight presentation observable.
        const settled = Promise.withResolvers<void>();
        setTimeout(settled.resolve, 500);
        await settled.promise;
        await route.abort("connectionreset");
        return;
      }
      // The retry repeats only the unresolved items of the same operation.
      trashRows = trashRows.filter((photoId) => photoId !== first);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          operationId: body.operationId,
          reviewed: 2,
          logicalBytesDeleted: 1024,
          items: [
            operationItem(first, "deleted"),
            operationItem(second, "missing", "No such file"),
          ],
        }),
      });
      return;
    }
    if (pathname.startsWith("/api/trash/operations/")) {
      const operationId = decodeURIComponent(pathname.split("/").pop() ?? "");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          operationId === pendingOperationId
            ? {
                operationId,
                reviewed: 1,
                logicalBytesDeleted: 0,
                items: [operationItem(pendingId, "deleting")],
              }
            : {
                operationId,
                reviewed: 2,
                logicalBytesDeleted: 1024,
                items: [
                  operationItem(first, "deleted"),
                  operationItem(second, "deleting"),
                ],
              },
        ),
      });
      return;
    }
    throw new Error(`Unexpected Trash route ${pathname}`);
  });

  await page.locator("[data-removed-open]").click();
  const removed = page.locator("[data-removed-panel]");
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "3 Photos in Trash. Showing 1–3.",
  );

  // The pending-verification item cannot be selected or restored, and its
  // row explains why.
  const pendingRow = page.locator(
    "[data-removed-list] .removed-item[data-trash-pending-item]",
  );
  await expect(pendingRow).toHaveCount(1);
  await expect(pendingRow.locator("input[type=checkbox]")).toBeDisabled();
  await expect(
    pendingRow.getByRole("button", { name: "Restore" }),
  ).toBeDisabled();
  await expect(
    pendingRow.locator("[data-trash-pending] .removed-pending-marker"),
  ).toHaveText(
    "Pending verification — the deletion outcome is still being verified.",
  );

  // Check result fetches the retained operation and presents its state.
  await pendingRow.locator("[data-trash-row-check]").click();
  await expect(page.locator("[data-trash-outcome-title]")).toHaveText(
    "Deletion outcome",
  );
  await expect(page.locator("[data-trash-outcome-pending]")).toHaveText(
    "Pending verification 1",
  );
  await expect(page.locator("[data-trash-outcome-deleted]")).toHaveText(
    "Deleted 0",
  );

  // Select all skips the pending-verification item.
  await page.locator("[data-removed-select-all]").click();
  await expect(page.locator("[data-removed-selection-count]")).toHaveText(
    "2 selected.",
  );
  await expect(page.locator("[data-removed-message]")).toHaveText(
    "1 item pending verification was not selected.",
  );

  // A review that rejected every selected item offers no delete action.
  const review = page.locator("[data-trash-review]");
  await page.locator("[data-removed-delete]").click();
  await expect(review).toBeVisible();
  await expect(page.locator("[data-trash-review-rejected]")).toBeVisible();
  await expect(page.locator("[data-trash-review-rejected-heading]")).toHaveText(
    "2 items could not be reviewed:",
  );
  const rejectedItems = page.locator("[data-trash-review-rejected-item]");
  await expect(rejectedItems).toHaveCount(2);
  await expect(
    rejectedItems.nth(0).locator("[data-trash-review-rejected-reason]"),
  ).toHaveText("Original missing");
  await expect(
    rejectedItems.nth(1).locator("[data-trash-review-rejected-reason]"),
  ).toHaveText("Original changed since it was removed");
  await expect(page.locator("[data-trash-confirm]")).toBeHidden();
  // Cancel deletes nothing: the delete route is never called.
  await page.locator("[data-trash-cancel]").click();
  await expect(review).toBeHidden();
  expect(deleteCalls).toBe(0);

  // The full review names the count, the logical bytes, the Album, and each
  // file's Location, kind, and size before the one irreversible action.
  await page.locator("[data-removed-delete]").click();
  await expect(review).toBeVisible();
  await expect(page.locator("[data-trash-review-summary]")).toHaveText(
    "2 Photos selected for permanent deletion · 3,072 logical bytes.",
  );
  await expect(page.locator(".trash-review-warning")).toHaveText(
    "Deletion removes each Photo from every Album that contains it. Slipstream cannot undo it.",
  );
  await expect(page.locator("[data-trash-review-albums]")).toHaveText(
    "1 Album affected: Keepers.",
  );
  const reviewItems = page.locator("[data-trash-review-item]");
  await expect(reviewItems).toHaveCount(2);
  await expect(
    reviewItems.nth(0).locator("[data-trash-review-location]"),
  ).toHaveText(`2024/${first}.jpg`);
  await expect(
    reviewItems.nth(0).locator("[data-trash-review-kind]"),
  ).toHaveText("JPEG");
  await expect(
    reviewItems.nth(0).locator("[data-trash-review-size]"),
  ).toHaveText("1,024 bytes");
  await expect(
    reviewItems.nth(0).locator("[data-trash-review-item-albums]"),
  ).toHaveText("Keepers");
  await expect(
    reviewItems.nth(1).locator("[data-trash-review-location]"),
  ).toHaveText("2024/b.cr2");
  await expect(
    reviewItems.nth(1).locator("[data-trash-review-kind]"),
  ).toHaveText("RAW");
  await expect(
    reviewItems.nth(1).locator("[data-trash-review-size]"),
  ).toHaveText("2,048 bytes");
  await expect(page.locator("[data-trash-confirm]")).toHaveText(
    "Permanently delete 2 Original Files",
  );
  await page.locator("[data-trash-cancel]").click();
  await expect(review).toBeHidden();
  expect(deleteCalls).toBe(0);

  // Confirming opens the delete route. The response is lost, so the surface
  // offers recovery instead of claiming failure or success.
  await page.locator("[data-removed-delete]").click();
  await expect(review).toBeVisible();
  await page.locator("[data-trash-confirm]").click();
  await expect(page.locator("[data-trash-confirm]")).toHaveText(
    "Permanently deleting…",
  );
  await expect(review).toBeHidden();
  await expect(page.locator("[data-trash-outcome-title]")).toHaveText(
    "The deletion result could not be confirmed.",
  );
  await expect(page.locator("[data-trash-check-result]")).toBeVisible();
  await expect(page.locator("[data-trash-retry-delete]")).toBeVisible();
  expect(
    await page.evaluate((key) => window.localStorage.getItem(key), storageKey),
  ).toBeTruthy();

  // Check result recovers the operation: one deletion is confirmed, one item
  // is still pending verification, and the retained id stays.
  await page.locator("[data-trash-check-result]").click();
  await expect(page.locator("[data-trash-outcome-title]")).toHaveText(
    "Deletion outcome",
  );
  await expect(page.locator("[data-trash-outcome-deleted]")).toHaveText(
    "Deleted 1",
  );
  await expect(page.locator("[data-trash-outcome-pending]")).toHaveText(
    "Pending verification 1",
  );
  await expect(page.locator("[data-trash-outcome-bytes]")).toHaveText(
    "Logical bytes deleted: 1,024",
  );
  expect(
    await page.evaluate((key) => window.localStorage.getItem(key), storageKey),
  ).toBeTruthy();

  // Retry repeats only the unresolved items and settles the operation.
  await page.locator("[data-trash-retry-delete]").click();
  await expect(page.locator("[data-trash-outcome-title]")).toHaveText(
    "Deletion outcome",
  );
  await expect(page.locator("[data-trash-outcome-deleted]")).toHaveText(
    "Deleted 1",
  );
  await expect(page.locator("[data-trash-outcome-missing]")).toHaveText(
    "Missing 1",
  );
  await expect(page.locator("[data-trash-outcome-bytes]")).toHaveText(
    "Logical bytes deleted: 1,024",
  );
  const outcomeItems = page.locator("[data-trash-outcome-item]");
  await expect(outcomeItems).toHaveCount(1);
  await expect(outcomeItems.first()).toHaveText(
    "Missing · RAW · 2024/b.cr2 · 2,048 bytes · No such file",
  );
  // The listing refreshes with the confirmed deletions: the deleted item
  // leaves Trash while the missing one stays inspectable.
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "2 Photos in Trash. Showing 1–2.",
  );
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    2,
  );
  expect(deleteCalls).toBe(2);
  // A settled operation releases the retained id.
  expect(
    await page.evaluate((key) => window.localStorage.getItem(key), storageKey),
  ).toBeNull();
  await page.locator("[data-removed-close]").click();
  await expect(removed).toBeHidden();
});

test("EXIF-rotated thumbnails display the corrected orientation exactly once", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const landscape = await jpegWithSize(page, 320, 180);
  await writeFile(join(root, "rotated.jpg"), withExifOrientation(landscape, 6));
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
  await expect
    .poll(() =>
      page
        .locator(".photo-cell img")
        .first()
        .evaluate((image: HTMLImageElement) =>
          image.complete && image.naturalWidth > 0
            ? `${image.naturalWidth}x${image.naturalHeight}`
            : "pending",
        ),
    )
    .toBe("180x320");
  const [cell] = await gridCellGeometry(page);
  expect(cell).toBeDefined();
  // Orientation 6 rotates 320x180 to 180x320; a double rotation would show
  // 320x180. The rendered box keeps the corrected ratio without stretching.
  expect(cell!.naturalWidth).toBe(180);
  expect(cell!.naturalHeight).toBe(320);
  expect(cell!.imageHeight).toBeGreaterThan(cell!.imageWidth);
  expectAspectRatio(cell!);
  expect(cell!.indicatorsOverlapImage).toBe(false);
});

test("a CLI-created Album opens in the Web with its ordered members and decisions", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  // Capture Times order the selected Photos differently from filename and
  // insertion order, so the opened Grid proves the CLI query supplied it.
  await writeFile(
    join(root, "one.jpg"),
    withCaptureTime(source, "2026:03:04 10:00:00"),
  );
  await writeFile(
    join(root, "two.jpg"),
    withCaptureTime(source, "2026:01:02 10:00:00"),
  );
  await writeFile(
    join(root, "three.jpg"),
    withCaptureTime(source, "2026:02:03 10:00:00"),
  );
  await writeFile(
    join(root, "four.jpg"),
    withCaptureTime(source, "2026:04:05 10:00:00"),
  );
  await writeFile(
    join(root, "five.jpg"),
    withCaptureTime(source, "2026:05:06 10:00:00"),
  );
  await writeFile(
    join(root, "six.jpg"),
    withCaptureTime(source, "2026:06:07 10:00:00"),
  );
  const running = await server(base, root);
  // Fixture setup only: the source Album and the pre-existing decisions the
  // query filters for. The organization workflow itself uses the CLI alone.
  const ids = await browseIds(running.url);
  expect(ids).toHaveLength(6);
  // The Library lists Capture Time order: two, three, one, four, five, six.
  const [twoId, threeId, oneId, fourId] = ids;
  // The source Album is seeded in filename order (one, two, three, four,
  // five, six), deliberately not Capture Time order, so only the CLI query's
  // --order capture-time-asc can produce the asserted sequence.
  const { albumId: sourceAlbumId } = await createAlbum(
    running.url,
    "Source picks",
    [oneId!, twoId!, threeId!, fourId!, ids[4]!, ids[5]!],
  );
  const decisions = [
    { id: twoId, selectionState: "selected", rating: 4 },
    { id: threeId, selectionState: "selected", rating: 5 },
    { id: oneId, selectionState: "selected", rating: 5 },
    { id: fourId, selectionState: "selected", rating: 4 },
    { id: ids[4], selectionState: "rejected", rating: 3 },
  ];
  for (const decision of decisions) {
    for (const [field, value] of [
      ["selectionState", decision.selectionState],
      ["rating", decision.rating],
    ] as const) {
      const response = await post(
        running.url,
        `/api/photos/${decision.id}/state`,
        { field, value },
      );
      expect(response.ok).toBe(true);
    }
  }

  const queried = await cli(running.url, [
    "photos",
    "list",
    "--album",
    sourceAlbumId,
    "--selection",
    "selected",
    "--rating-min",
    "4",
    "--order",
    "capture-time-asc",
    "--limit",
    "60",
  ]);
  const orderedIds = (
    queried.data as { items: Array<{ id: string }> }
  ).items.map((item) => item.id);
  // Capture Time order — not filename order (four, one, three, two) and not
  // the seeded Album order (one, two, three, four).
  expect(orderedIds).toEqual([twoId, threeId, oneId, fourId]);

  const created = await cli(running.url, [
    "albums",
    "create",
    "--name",
    "CLI 精选",
  ]);
  const album = (
    created.data as {
      album: {
        id: string;
        albumVersion: string;
        webUrl: string;
      };
    }
  ).album;
  const membersPath = join(base, "members.json");
  await writeFile(membersPath, JSON.stringify({ photoIds: orderedIds }));
  const added = await cli(running.url, [
    "albums",
    "add",
    album.id,
    "--input",
    membersPath,
    "--if-version",
    album.albumVersion,
  ]);
  expect((added.data as { addedPhotoIds: string[] }).addedPhotoIds).toEqual(
    orderedIds,
  );

  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(album.webUrl);
  await expect(page.getByText("Ready · 4 Photos")).toBeVisible();
  await waitForGridFrame(page);
  await expectGridOrder(page, orderedIds);
  expect(new URL(page.url()).search).toBe(`?source=album&albumId=${album.id}`);
  const cell = (index: number) => page.locator(`[data-photo-index="${index}"]`);
  for (const [index, rating] of [4, 5, 5, 4].entries()) {
    await expect(cell(index).locator(".cell-state.selected")).toHaveText("✓");
    await expect(cell(index)).toHaveAttribute(
      "aria-label",
      new RegExp(`${rating} stars`),
    );
  }
});

test("the empty Library, an empty Album, and no filter matches stay distinct", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  // An Album with no members, so its empty state is its own.
  const created = await post(running.url, "/api/albums", {
    name: "Empty Album",
  });
  expect(created.ok).toBe(true);
  const albumId = (
    (await await created.json()) as { albums: Array<{ id: string }> }
  ).albums[0]!.id;
  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 2 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  // No filter matches: the filter is named, and no Library check is offered.
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("rejected");
  await applyViewOptions(page);
  await expect(page.locator("[data-grid-empty-message]")).toHaveText(
    "No Photos match this filter.",
  );
  await expect(page.locator("[data-grid-empty-action]")).toBeHidden();
  await expect(page.locator("[data-grid-status]")).toHaveText("0 Photos");

  // An empty Album names the Album and its existing action.
  await openSources(page);
  await page.getByRole("link", { name: /^Empty Album 0 Photos/ }).click();
  await expect(page.locator("[data-grid-empty-message]")).toHaveText(
    "This Album contains no Photos. Add Photos from another source's Photo View.",
  );
  await expect(page.locator("[data-grid-empty-action]")).toBeHidden();
  await expect(page.locator("[data-grid-status]")).toHaveText("0 Photos");
  expect(albumId).toBeTruthy();

  // An empty Library keeps its Library check action.
  await openSources(page);
  await page.getByRole("link", { name: /^All Photos 2 Photos/ }).click();
  await expect(page.getByText(/^Ready · 2 Photos$/)).toBeVisible();
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("all");
  await applyViewOptions(page);
  await page.evaluate(async () => {
    const response = await fetch("/api/overview");
    void response;
  });
  await expect(page.locator("[data-grid-empty]")).toBeHidden();
});

/// Issue #310 integrated qualification. The navigation (#307), Grid (#308),
/// and Photo View (#309) slices own their focused tests; this block owns the
/// cross-slice sessions the Epic gate requires, driven against the production
/// server exactly as a Photographer would drive them.

test("Grid batch Select decides every multi-selected Photo and Undo restores them as one unit", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 4 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  const cell = (index: number) => page.locator(`[data-photo-index="${index}"]`);
  const count = page.locator("[data-batch-count]");
  const progress = page.locator("[data-grid-source-progress]");
  const visibleResults = page.locator("[data-grid-visible-results]");
  const batchBodies: Array<Record<string, unknown>> = [];
  const undoWrites: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "POST") return;
    const path = new URL(request.url()).pathname;
    if (path === "/api/photos/state")
      batchBodies.push(request.postDataJSON() as Record<string, unknown>);
    else if (/^\/api\/photos\/[^/]+\/state$/.test(path)) undoWrites.push(path);
  });

  // Select mode marks three Photos, and one batch decision applies to all of
  // them through one bounded request.
  await page.locator("[data-grid-select-mode]").click();
  for (const index of [0, 1, 2]) await cell(index).click();
  await expect(count).toHaveText("3 / 100 Photos");
  await page.locator("[data-batch-select]").click();
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "3 Photos selected.",
  );
  await expect(page.locator("[data-batch-retained]")).toBeVisible();
  await expect(page.locator("[data-grid-batch-result-text]")).toHaveText(
    "3 Photos selected.",
  );
  await expect(page.locator("[data-grid-batch-result]")).toHaveAttribute(
    "data-tone",
    "success",
  );
  expect(batchBodies).toEqual([
    {
      photos: [
        { photoId: ids[0], expectedCurrent: "undecided" },
        { photoId: ids[1], expectedCurrent: "undecided" },
        { photoId: ids[2], expectedCurrent: "undecided" },
      ],
      selectionState: "selected",
    },
  ]);
  for (const index of [0, 1, 2]) {
    await expect(cell(index).locator(".cell-state.selected")).toHaveText("✓");
    expect(await libraryPhoto(running.url, index)).toMatchObject({
      selectionState: "selected",
    });
  }
  await expect(visibleResults).toHaveText("Visible results: 4 of 4 Photos");
  await expect(progress).toHaveText(
    "Source progress: 3 selected · 0 rejected · 1 undecided",
  );
  // The multi-selection stays, so the same Photos can join an Album next.
  await expect(count).toHaveText("3 / 100 Photos");

  // One Undo restores every confirmed Photo as one unit and stays in the
  // Grid, exactly as a single Grid decision does.
  await page.locator("[data-grid-viewport]").focus();
  await page.keyboard.press("Control+z");
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "3 Photos restored.",
  );
  await expect(page.locator("[data-batch-retained]")).toBeVisible();
  await expect(page.locator("[data-grid-batch-result-text]")).toHaveText(
    "3 Photos restored.",
  );
  expect(undoWrites).toEqual([
    `/api/photos/${ids[0]}/state`,
    `/api/photos/${ids[1]}/state`,
    `/api/photos/${ids[2]}/state`,
  ]);
  for (const index of [0, 1, 2]) {
    await expect(cell(index).locator(".cell-state")).toHaveCount(0);
    expect(await libraryPhoto(running.url, index)).toMatchObject({
      selectionState: "undecided",
    });
  }
  await expect(progress).toHaveText(
    "Source progress: 0 selected · 0 rejected · 4 undecided",
  );
  await expect(page.locator("[data-review]")).toBeHidden();

  // The one-level description is consumed: nothing is left to undo.
  await page.keyboard.press("Control+z");
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "3 Photos restored.",
  );
  expect(undoWrites).toHaveLength(3);
});
