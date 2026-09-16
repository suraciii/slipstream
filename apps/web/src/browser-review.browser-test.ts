import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  stat,
  writeFile,
  chmod,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";

import {
  expect,
  test,
  type Locator,
  type Page,
  type Request as PlaywrightRequest,
  type Route,
} from "@playwright/test";

import { startBrowserServer, type BrowserServer } from "./browser-server.js";

const sample = process.env.SLIPSTREAM_RAW_SAMPLE;
const temporary: string[] = [];
const servers: BrowserServer[] = [];

test.afterEach(async () => {
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
  // The server binds before its owned startup scan finishes, so tests wait
  // for the Library to settle before driving the UI.
  await expect
    .poll(
      async () => {
        const response = await fetch(`${running.url}/api/status`);
        return ((await response.json()) as { state: string }).state;
      },
      { timeout: 60_000 },
    )
    .toBe("idle");
  return running;
}
async function post(url: string, path: string, body: unknown) {
  return fetch(`${url}${path}`, {
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
  originals?: ReadonlyArray<Readonly<{ kind: string }>>;
};
type AlbumMember = BrowsePhoto & { photoId: string; position: number };
type AlbumState = { id: string; position: number; members: AlbumMember[] };

async function browseWindow(
  url: string,
  token: string,
  start: number,
): Promise<{ start: number; total: number; photos: BrowsePhoto[] }> {
  const window = (await (
    await fetch(`${url}/api/browse/${token}?start=${start}&limit=60`)
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
  await fetch(`${url}/api/browse/${opened.token}`, {
    method: "DELETE",
    headers: { Origin: url },
  });
  return ids;
}

async function browseOrderedIds(
  url: string,
  request: Record<string, unknown>,
): Promise<string[]> {
  const opened = (await (await post(url, "/api/browse", request)).json()) as {
    token: string;
    total: number;
  };
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
  await fetch(`${url}/api/browse/${opened.token}`, {
    method: "DELETE",
    headers: { Origin: url },
  });
  return ids;
}

async function createAlbum(url: string, name = "Review") {
  const photos = await browseIds(url);
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

function recordBrowseBodies(page: Page) {
  const bodies: Array<Record<string, unknown>> = [];
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/browse"
    )
      bodies.push(request.postDataJSON() as Record<string, unknown>);
  });
  return bodies;
}

async function evictFirstPhotoFact(page: Page) {
  const viewport = page.locator("[data-grid-viewport]");
  for (const [row, photoIndex] of [
    [30, 65],
    [60, 125],
    [90, 185],
  ] as const) {
    await viewport.evaluate((element, targetRow) => {
      element.scrollTop = targetRow * 178;
    }, row);
    await expect(
      page.locator(`[data-photo-index="${photoIndex}"]`),
    ).toBeVisible();
  }
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
const renderedGridSpan = (page: Page) =>
  page.evaluate(() => {
    const cells = Array.from(
      document.querySelectorAll<HTMLElement>(".photo-cell"),
    );
    const columns = new Set(cells.map((cell) => cell.style.left)).size;
    const firstTop = Math.min(
      ...cells.map((cell) => Number.parseFloat(cell.style.top)),
    );
    const firstIndex = (firstTop / 178) * columns;
    return {
      cells: cells.length,
      start: firstIndex,
      end: firstIndex + cells.length,
    };
  });

/// The aligned 60-Photo windows that cover a reported range.
const coveringWindowStarts = (start: number, end: number, total: number) => {
  const starts: number[] = [];
  for (let windowStart = 0; windowStart < total; windowStart += 60)
    if (windowStart < end && windowStart + 60 > start) starts.push(windowStart);
  if (total % 60 !== 0 && end === total && !starts.includes(total - 60))
    starts.push(total - 60);
  return starts;
};

/// Browse-window requests by aligned start, with the ones still in flight.
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
const expectGridConverged = async (
  page: Page,
  windows: Readonly<{ pending: Set<PlaywrightRequest> }>,
) => {
  await expect
    .poll(async () => ({
      placeholders: await page.locator(".cell-placeholder").count(),
      inFlight: windows.pending.size,
    }))
    .toEqual({ placeholders: 0, inFlight: 0 });
};

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
async function expectRenderedZoom(page: Page, scale: number) {
  await waitForLoadedReviewImage(page);
  await expect
    .poll(async () => {
      const image = await previewImageGeometry(page);
      return image.width / image.naturalWidth;
    })
    .toBeCloseTo(scale, 1);
}

// Fit depends on the live stage size, so a viewport change is only settled
// once the rendered size matches the current fit scale again.
async function waitForFit(page: Page) {
  await expect
    .poll(async () => {
      const stage = await previewStageGeometry(page);
      const image = await previewImageGeometry(page);
      const state = await page
        .locator("[data-preview]")
        .getAttribute("data-zoom-state");
      if (state !== "fit") return Number.POSITIVE_INFINITY;
      const scale = Math.min(
        stage.width / image.naturalWidth,
        stage.height / image.naturalHeight,
      );
      return Math.abs(image.width - image.naturalWidth * scale);
    })
    .toBeLessThan(1);
}

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
  await fetch(`${url}/api/browse/${opened.token}`, {
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
  await page
    .getByRole("button", { name: new RegExp(`^${escapedName}(?: |$)`) })
    .click();
  await page
    .locator("[data-grid-status]")
    .filter({ hasText: /^(?:Ready · \d[\d,]* Photos?|0 Photos)$/ })
    .waitFor();
  await waitForGridFrame(page);
}
async function openSources(page: Page) {
  for (const toggle of await page
    .getByRole("button", { name: "Sources", exact: true })
    .all()) {
    if (
      (await toggle.isVisible()) &&
      (await toggle.getAttribute("aria-expanded")) !== "true"
    ) {
      await toggle.click();
      return;
    }
  }
}

/// Album membership is read and managed in one panel: the facts list names the
/// Albums this Photo is in, and the Manage panel holds one checkbox per Album.
async function openMembershipPanel(page: Page) {
  const manage = page.getByRole("button", { name: "Manage", exact: true });
  if ((await manage.getAttribute("aria-expanded")) !== "true")
    await manage.click();
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
async function trackSettledMembershipReads(page: Page) {
  await page.addInitScript(() => {
    const nativeFetch = window.fetch.bind(window);
    let settled = 0;
    window.fetch = (async (
      input: Parameters<typeof window.fetch>[0],
      init?: Parameters<typeof window.fetch>[1],
    ) => {
      const response = await nativeFetch(input, init);
      const url =
        typeof input === "string"
          ? input
          : input instanceof Request
            ? input.url
            : String(input);
      if (url.endsWith("/albums") && (init?.method ?? "GET") === "GET")
        setTimeout(() => {
          settled += 1;
          document.documentElement.dataset.membershipReadsSettled =
            String(settled);
        }, 0);
      return response;
    }) as typeof window.fetch;
  });
}

async function settledMembershipReads(page: Page): Promise<number> {
  return Number(
    (await page
      .locator("html")
      .getAttribute("data-membership-reads-settled")) ?? "0",
  );
}
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
async function swipe(page: Page, from: number, to: number, y = 320) {
  const preview = page.locator("[data-preview]");
  await preview.dispatchEvent("pointerdown", {
    pointerId: 1,
    isPrimary: true,
    clientX: from,
    clientY: y,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointermove", {
    pointerId: 1,
    isPrimary: true,
    clientX: to,
    clientY: y,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointerup", {
    pointerId: 1,
    isPrimary: true,
    clientX: to,
    clientY: y,
    pointerType: "touch",
  });
}

async function touchDrag(
  page: Page,
  from: { x: number; y: number },
  to: { x: number; y: number },
) {
  const session = await page.context().newCDPSession(page);
  try {
    await session.send("Input.dispatchTouchEvent", {
      type: "touchStart",
      touchPoints: [from],
    });
    for (let step = 1; step <= 4; step += 1)
      await session.send("Input.dispatchTouchEvent", {
        type: "touchMove",
        touchPoints: [
          {
            x: from.x + ((to.x - from.x) * step) / 4,
            y: from.y + ((to.y - from.y) * step) / 4,
          },
        ],
      });
    await session.send("Input.dispatchTouchEvent", {
      type: "touchEnd",
      touchPoints: [],
    });
  } finally {
    await session.detach();
  }
}

async function interactiveGeometry(container: Locator) {
  return container.evaluate((root) => {
    const rootBox = root.getBoundingClientRect();
    return Array.from(
      root.querySelectorAll<HTMLElement>(
        'button:not([hidden]), input:not([hidden]), select:not([hidden]), [role="button"]:not([hidden]), [tabindex]:not([tabindex="-1"]):not([hidden]), [data-preview]',
      ),
    )
      .filter(
        (target) =>
          target.offsetParent !== null &&
          !target.closest<HTMLElement>("[inert]"),
      )
      .map((target) => {
        const box = target.getBoundingClientRect();
        return {
          name:
            target.getAttribute("aria-label") ??
            target.textContent?.trim() ??
            target.tagName,
          width: box.width,
          height: box.height,
          contained:
            box.left >= rootBox.left - 0.5 && box.right <= rootBox.right + 0.5,
        };
      });
  });
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
    const button = page.getByRole("button", {
      name: `${name} ${count}`,
      exact: true,
    });
    await expect(button).toBeVisible();
    await expect(button).toHaveAccessibleName(`${name} ${count}`);
    await expect(button.locator("span")).toHaveText(count);
    return button;
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
    .getByRole("button", { name: "Single Album 1 Photo", exact: true })
    .click();
  await expect(
    page.getByText("Ready · 1 Photo", { exact: true }),
  ).toBeVisible();
});

test("exposes one main and named sources navigation landmarks while loading and after rendering", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "photo.jpg"), await jpeg());
  const running = await server(base, root);

  let releaseOverview!: () => void;
  const overviewHeld = new Promise<void>((resolve) => {
    releaseOverview = resolve;
  });
  let observeOverview!: () => void;
  const overviewRequested = new Promise<void>((resolve) => {
    observeOverview = resolve;
  });
  let observeOverviewDelivered!: () => void;
  const overviewDelivered = new Promise<void>((resolve) => {
    observeOverviewDelivered = resolve;
  });
  let overviewCaptured = false;
  await page.route("**/api/overview", async (route) => {
    const response = await route.fetch();
    overviewCaptured = true;
    observeOverview();
    try {
      await overviewHeld;
      await route.fulfill({ response });
    } finally {
      observeOverviewDelivered();
    }
  });

  try {
    await page.goto(running.url);
    await overviewRequested;
    const mainLandmark = page.getByRole("main");
    const htmlMain = page.locator("main");
    const sourcesNavigation = page.getByRole("navigation", {
      name: "Library sources",
      exact: true,
    });
    const htmlNavigation = page.locator("nav");
    await expect(page.getByText("Loading Library summary…")).toBeVisible();
    await expect(htmlMain).toHaveCount(1);
    await expect(mainLandmark).toHaveCount(1);
    await expect(mainLandmark).toHaveAttribute("id", "app");
    await expect(htmlNavigation).toHaveCount(1);
    await expect(sourcesNavigation).toHaveCount(1);
    await expect(sourcesNavigation).toHaveAttribute(
      "aria-label",
      "Library sources",
    );

    releaseOverview();
    await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
    await expect(htmlMain).toHaveCount(1);
    await expect(mainLandmark).toHaveCount(1);
    await expect(mainLandmark).toHaveAttribute("id", "app");
    await expect(htmlNavigation).toHaveCount(1);
    await expect(sourcesNavigation).toHaveCount(1);
    await expect(sourcesNavigation).toHaveAttribute(
      "aria-label",
      "Library sources",
    );
  } finally {
    releaseOverview();
    if (overviewCaptured) await overviewDelivered;
    await page.unroute("**/api/overview");
  }
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
  await expect(page.getByText("Undecided", { exact: true })).toBeVisible();
  await expect(page.getByText("0 stars", { exact: true })).toBeVisible();
  await expect(page.getByText("JPEG", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Limited by camera Preview resolution"),
  ).toBeVisible();
  for (const name of [
    "Select",
    "Reject",
    "Clear",
    "Undo",
    "Previous",
    "Next",
    "Fit Window",
    "Zoom in",
    "Zoom out",
    "Zoom to 100 percent",
    "Rate 5 stars",
  ])
    await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
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
  await page.getByRole("button", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: /^Picks \d+ Photos/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 2 of 2/ }),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await expect(page.getByText("Selected", { exact: true })).toBeVisible();
});

test("narrow Grid keeps sources in a dismissible drawer and restores focus", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg());
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);

  const sources = page.locator("[data-source-toggle]");
  const panel = page.locator("#source-panel");
  await expect(sources).toBeVisible();
  await expect(sources).toHaveAttribute("aria-expanded", "false");
  await expect(panel).toHaveAttribute("aria-hidden", "true");
  expect(
    await page
      .locator("[data-grid-viewport]")
      .evaluate((node) => node.clientHeight),
  ).toBeGreaterThan(700);

  await sources.click();
  await expect(sources).toHaveAttribute("aria-expanded", "true");
  await expect(panel).toHaveAttribute("aria-hidden", "false");
  await expect(
    page.getByRole("button", { name: /^All Photos(?: |$)/ }),
  ).toBeVisible();
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
  expect(
    await page
      .locator("[data-grid-view]")
      .evaluate((node) => (node as HTMLElement).inert),
  ).toBe(true);
  await expect(
    page.getByRole("button", { name: "Close", exact: true }),
  ).toBeFocused();
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
  await page.keyboard.press("Escape");
  await expect(panel).toHaveAttribute("aria-hidden", "true");
  await expect(sources).toBeFocused();
  expect(
    await page
      .locator("[data-grid-view]")
      .evaluate((node) => (node as HTMLElement).inert),
  ).toBe(false);

  await sources.click();
  await page.getByRole("button", { name: /^All Photos(?: |$)/ }).click();
  await expect(panel).toHaveAttribute("aria-hidden", "true");
  await expect(page.locator("[data-grid-viewport]")).toBeFocused();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
});

test("narrow Grid uses its width and keeps Library Folder understandable", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText("Ready · 4 Photos")).toBeVisible();
  await waitForGridFrame(page);

  const grid = await page
    .locator("[data-grid-viewport]")
    .evaluate((viewport) => {
      const viewportBox = viewport.getBoundingClientRect();
      const cells = Array.from(
        viewport.querySelectorAll<HTMLElement>(".photo-cell"),
        (cell) => cell.getBoundingClientRect(),
      );
      const firstRow = cells.filter(
        (cell) => Math.abs(cell.top - cells[0]!.top) < 1,
      );
      return {
        columns: firstRow.length,
        cellWidth: firstRow[0]!.width,
        trailingSpace: viewportBox.right - firstRow.at(-1)!.right,
      };
    });
  expect(grid.columns).toBe(2);
  expect(grid.cellWidth).toBeGreaterThan(170);
  expect(grid.trailingSpace).toBeGreaterThanOrEqual(0);
  expect(grid.trailingSpace).toBeLessThanOrEqual(12);

  await openSources(page);
  const label = page.locator(".folder-root .source-card strong");
  await expect(label).toHaveText("Library Folder");
  expect(
    await label.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
});

test("short mobile viewports keep every Photo action reachable and operable", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 3);
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Destination" });
  await startReview(page, running.url, "All Photos");

  const viewports = [
    { width: 844, height: 390 },
    { width: 667, height: 375 },
    { width: 390, height: 844 },
  ];
  for (const [index, viewport] of viewports.entries()) {
    await page.setViewportSize(viewport);
    const photoView = page.locator("[data-photo-view]");
    await expect(photoView).toBeVisible();
    const layout = await photoView.evaluate((view) => {
      const bounds = (selector: string) =>
        (view.querySelector(selector) as HTMLElement).getBoundingClientRect();
      const preview = bounds("[data-preview]");
      const controlGroups = [
        ".decision-controls",
        ".rating-controls",
        ".membership",
        ".photo-controls",
      ].map((selector) => bounds(selector).height);
      return {
        clientHeight: view.clientHeight,
        scrollHeight: view.scrollHeight,
        clientWidth: view.clientWidth,
        scrollWidth: view.scrollWidth,
        previewHeight: preview.height,
        tallestControlGroup: Math.max(...controlGroups),
      };
    });
    if (viewport.height < 400)
      expect(layout.scrollHeight).toBeGreaterThan(layout.clientHeight);
    expect(layout.previewHeight).toBeGreaterThan(layout.tallestControlGroup);
    expect(layout.scrollWidth).toBe(layout.clientWidth);
    const photoTargets = await interactiveGeometry(photoView);
    expect(
      photoTargets.filter(({ width, height }) => width < 44 || height < 44),
    ).toEqual([]);
    expect(photoTargets.filter(({ contained }) => !contained)).toEqual([]);

    await openSources(page);
    const sourcePanel = page.locator("#source-panel");
    await expect(sourcePanel).toHaveAttribute("aria-hidden", "false");
    await page.getByRole("button", { name: "New Album" }).click();
    const sourceTargets = await interactiveGeometry(sourcePanel);
    expect(
      sourceTargets.filter(({ width, height }) => width < 44 || height < 44),
    ).toEqual([]);
    expect(sourceTargets.filter(({ contained }) => !contained)).toEqual([]);
    await page.getByRole("button", { name: "Cancel", exact: true }).click();
    await page.getByRole("button", { name: "Close", exact: true }).click();

    const controls = [
      page.getByRole("button", { name: "Back to Grid" }),
      page.getByRole("button", { name: `Rate ${index + 3} stars` }),
      page.getByRole("button", { name: "Manage", exact: true }),
      page.getByRole("button", { name: "Previous" }),
      page.getByRole("button", { name: "Next" }),
    ];
    for (const control of controls) {
      await control.scrollIntoViewIfNeeded();
      const contained = await control.evaluate((element) => {
        const target = element.getBoundingClientRect();
        const view = element
          .closest("[data-photo-view]")!
          .getBoundingClientRect();
        return (
          target.left >= view.left &&
          target.right <= view.right &&
          target.top >= view.top &&
          target.bottom <= view.bottom
        );
      });
      expect(contained).toBe(true);
    }
    await openMembershipPanel(page);
    const optionTargets = await interactiveGeometry(
      page.locator("[data-membership-panel]"),
    );
    expect(
      optionTargets.filter(({ width, height }) => width < 44 || height < 44),
    ).toEqual([]);
    expect(optionTargets.filter(({ contained }) => !contained)).toEqual([]);

    const ratingSaved = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname.endsWith("/state") &&
        response.status() === 200,
    );
    await page.getByRole("button", { name: `Rate ${index + 3} stars` }).click();
    await ratingSaved;
    await expect(
      page.getByText(`${index + 3} stars`, { exact: true }),
    ).toBeVisible();

    // Every viewport proves the membership toggle is operable, so the first
    // step normalizes the membership the previous viewport left behind.
    const destination = membershipCheckbox(page, "Destination");
    await expect(destination).toBeVisible();
    if (await destination.isChecked()) {
      const membershipRemoved = page.waitForResponse(
        (response) =>
          response.request().method() === "POST" &&
          new URL(response.url()).pathname.endsWith("/members/remove") &&
          response.status() === 200,
      );
      await destination.uncheck();
      await membershipRemoved;
    }
    const membershipSaved = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname.endsWith("/members") &&
        response.status() === 200,
    );
    await destination.check();
    await membershipSaved;
    await expect(page.getByText("Added to the Album.")).toBeVisible();

    await page.getByRole("button", { name: "Next" }).click();
    await expect(page.getByText("2 / 3")).toBeVisible();
    await page.getByRole("button", { name: "Previous" }).click();
    await expect(page.getByText("1 / 3")).toBeVisible();
    await page.getByRole("button", { name: "Back to Grid" }).click();
    await expect(page.locator("[data-grid-view]")).toBeVisible();
    const gridTargets = await interactiveGeometry(
      page.locator("[data-grid-view]"),
    );
    expect(
      gridTargets.filter(({ width, height }) => width < 44 || height < 44),
    ).toEqual([]);
    expect(gridTargets.filter(({ contained }) => !contained)).toEqual([]);

    if (index < viewports.length - 1) {
      await page.locator('[data-photo-index="0"]').click();
      await expect(photoView).toBeVisible();
      await expect
        .poll(() => photoView.evaluate((view) => view.scrollTop))
        .toBe(0);
    }
  }
});

test("wide desktop Preview retains fit gesture ownership", async ({ page }) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await page.setViewportSize({ width: 1280, height: 800 });

  await expect(page.locator("[data-preview]")).toHaveCSS(
    "touch-action",
    "none",
  );
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

  // Changing Photo resets the zoom state to Fit.
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await waitForLoadedReviewImage(page);
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await expect(fit).toHaveAttribute("aria-pressed", "true");
  expect(stateRequests).toBe(0);
});

test("desktop wheel zoom keeps the image point under the pointer stationary", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  await page.setViewportSize({ width: 900, height: 800 });
  await waitForFit(page);
  const preview = page.locator("[data-preview]");
  const slider = page.locator("[data-zoom-slider]");
  await slider.fill("400");
  await slider.dispatchEvent("input");

  const pointer = await preview.evaluate((surface) => {
    const box = surface.getBoundingClientRect();
    return {
      x: box.left + box.width * 0.32,
      y: box.top + box.height * 0.4,
    };
  });
  const before = await previewImageGeometry(page);
  const fractionX = (pointer.x - before.left) / before.width;
  const fractionY = (pointer.y - before.top) / before.height;
  await page.mouse.move(pointer.x, pointer.y);
  await page.mouse.wheel(0, -120);
  const after = await previewImageGeometry(page);
  expect(after.width).toBeCloseTo(before.width * 1.25, 0);
  // The same image point stays under the pointer while zooming.
  expect(pointer.x - fractionX * after.width).toBeCloseTo(after.left, 0);
  expect(pointer.y - fractionY * after.height).toBeCloseTo(after.top, 0);
  await page.mouse.wheel(0, 120);
  await expect
    .poll(async () => (await previewImageGeometry(page)).width)
    .toBeCloseTo(before.width, 0);
});

test("touch pinch zooms around the gesture midpoint without deciding", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  const preview = page.locator("[data-preview]");
  await preview.getByRole("button", { name: "Zoom to 100 percent" }).click();
  const before = await previewImageGeometry(page);
  expect(before.width).toBeCloseTo(before.naturalWidth, 0);
  const center = await preview.evaluate((surface) => {
    const box = surface.getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
  });
  let stateRequests = 0;
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname.endsWith("/state")
    )
      stateRequests += 1;
  });
  await preview.dispatchEvent("pointerdown", {
    pointerId: 21,
    isPrimary: true,
    clientX: center.x - 50,
    clientY: center.y,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointerdown", {
    pointerId: 22,
    isPrimary: false,
    clientX: center.x + 50,
    clientY: center.y,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointermove", {
    pointerId: 21,
    isPrimary: true,
    clientX: center.x - 100,
    clientY: center.y,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointermove", {
    pointerId: 22,
    isPrimary: false,
    clientX: center.x + 100,
    clientY: center.y,
    pointerType: "touch",
  });
  const pinched = await previewImageGeometry(page);
  expect(pinched.width).toBeCloseTo(before.width * 2, 0);
  expect(await zoomLevel(page)).toBe(200);
  await preview.dispatchEvent("pointerup", {
    pointerId: 21,
    isPrimary: true,
    clientX: center.x - 100,
    clientY: center.y,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointerup", {
    pointerId: 22,
    isPrimary: false,
    clientX: center.x + 100,
    clientY: center.y,
    pointerType: "touch",
  });
  expect(stateRequests).toBe(0);
  await expect(page.getByText("1 / 1")).toBeVisible();
});

test("window resize recomputes Fit and keeps a manual percentage", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  const preview = page.locator("[data-preview]");
  const level = page.locator("[data-zoom-level]");

  await page.setViewportSize({ width: 900, height: 800 });
  await waitForFit(page);
  const wideFit = await previewImageGeometry(page);

  await page.setViewportSize({ width: 600, height: 800 });
  await expect
    .poll(async () => (await previewImageGeometry(page)).width)
    .toBeLessThan(wideFit.width);
  const stage = await previewStageGeometry(page);
  const narrowFit = await previewImageGeometry(page);
  expect(narrowFit.width).toBeLessThanOrEqual(stage.width + 0.5);
  expect(narrowFit.height).toBeLessThanOrEqual(stage.height + 0.5);

  await page.locator("[data-zoom-slider]").fill("100");
  await page.locator("[data-zoom-slider]").dispatchEvent("input");
  await expect(level).toHaveText("100%");
  await page.setViewportSize({ width: 760, height: 800 });
  await expect(level).toHaveText("100%");
  await expect(preview).toHaveAttribute("data-zoom-state", "manual");
  const resized = await previewImageGeometry(page);
  expect(resized.width).toBeCloseTo(resized.naturalWidth, 0);
});

test("a pan that began while zoomed never becomes a decision", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);
  await waitForLoadedReviewImage(page);
  const preview = page.locator("[data-preview]");
  const slider = page.locator("[data-zoom-slider]");
  await slider.fill("300");
  await slider.dispatchEvent("input");
  await expectRenderedZoom(page, 3);

  let stateRequests = 0;
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname.endsWith("/state")
    )
      stateRequests += 1;
  });

  // The drag starts as a pan; restoring Fit mid-drag must not hand the
  // gesture back to the decision swipe that Fit owns.
  const center = await preview.evaluate((surface) => {
    const box = surface.getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
  });
  await page.mouse.move(center.x, center.y);
  await page.mouse.down();
  await page.mouse.move(center.x + 130, center.y + 10);
  await page.keyboard.press("f");
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await page.mouse.up();

  // The release handler runs before this navigation settles, so neither the
  // request count nor the durable Album fact can race it.
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 3")).toBeVisible();
  expect(stateRequests).toBe(0);
  expect(
    (await state(running.url, albumId)).members.map(
      (member) => member.selectionState,
    ),
  ).toEqual(["undecided", "undecided", "undecided"]);
});

test("zoom controls are disabled while no Preview image is measurable", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  // The Preview state stays ready while its bytes never arrive, which
  // leaves an image element on the stage without any pixels to measure.
  await page.route("**/api/derivatives/*/review/*", (route) => route.abort());
  await startReview(page, running.url, "All Photos");
  await expect(page.locator("[data-status]")).toHaveText(
    "Preview could not be loaded. You can continue browsing.",
  );

  const preview = page.locator("[data-preview]");
  const fit = page.getByRole("button", { name: "Fit Window", exact: true });
  const zoomOut = page.getByRole("button", { name: "Zoom out", exact: true });
  const zoomIn = page.getByRole("button", { name: "Zoom in", exact: true });
  const hundred = page.getByRole("button", {
    name: "Zoom to 100 percent",
    exact: true,
  });
  await expect(fit).toBeDisabled();
  await expect(zoomOut).toBeDisabled();
  await expect(zoomIn).toBeDisabled();
  await expect(hundred).toBeDisabled();
  await expect(page.locator("[data-zoom-slider]")).toBeDisabled();
  await expect(page.locator("[data-zoom-level]")).toHaveText("—");

  // Keyboard zoom is ignored without measurable pixels.
  await page.keyboard.press("+");
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");

  // A healthy Photo restores zoom control.
  await page.unroute("**/api/derivatives/*/review/*");
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await waitForLoadedReviewImage(page);
  await expect(hundred).toBeEnabled();
  await expect(fit).toBeEnabled();
  const slider = page.locator("[data-zoom-slider]");
  await slider.fill("300");
  await slider.dispatchEvent("input");
  await expectRenderedZoom(page, 3);
});

test("a Photo without usable Preview bytes reports no zoom percentage", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  let healthyPhotoId: string | undefined;
  await page.route("**/api/derivatives/*/review/*", async (route) => {
    const photoId = new URL(route.request().url()).pathname.split("/")[3]!;
    healthyPhotoId ??= photoId;
    if (photoId === healthyPhotoId) {
      await route.continue();
      return;
    }
    await route.abort();
  });
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  const preview = page.locator("[data-preview]");
  const level = page.locator("[data-zoom-level]");
  const slider = page.locator("[data-zoom-slider]");
  await slider.fill("300");
  await slider.dispatchEvent("input");
  await expect(level).toHaveText("300%");

  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await expect(page.locator("[data-status]")).toHaveText(
    "Preview could not be loaded. You can continue browsing.",
  );
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await expect(
    page.getByRole("button", { name: "Fit Window", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(level).toHaveText("—");
  await expect(slider).toBeDisabled();
});

test("a Fit below the manual floor keeps stepping monotonic", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const { base, root } = await fixture();
  // A 2560 px derivative in a narrow Preview area puts Fit below the 10%
  // manual floor, which the stepping controls must never exceed.
  await writeFile(
    join(root, "large.jpg"),
    await jpegWithSize(page, 2560, 2560),
  );
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  const preview = page.locator("[data-preview]");
  const level = page.locator("[data-zoom-level]");
  const slider = page.locator("[data-zoom-slider]");
  await page.setViewportSize({ width: 300, height: 844 });
  await waitForFit(page);

  const fitted = await previewImageGeometry(page);
  const fitPercent = Math.round((fitted.width / fitted.naturalWidth) * 100);
  expect(fitPercent).toBeLessThan(10);
  await expect(level).toHaveText(`${fitPercent}%`);
  // The slider spans the manual range only, and it must not contradict the
  // value it reports to assistive technology.
  expect(await slider.inputValue()).toBe("10");
  await expect(slider).toHaveAttribute("aria-valuetext", "10%");

  // Zooming out below the floor must not magnify the Preview.
  await page.getByRole("button", { name: "Zoom out", exact: true }).click();
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  const unzoomed = await previewImageGeometry(page);
  expect(unzoomed.width).toBeCloseTo(fitted.width, 1);
  await expect(level).toHaveText(`${fitPercent}%`);

  // Zooming in enters manual zoom above the Fit it started from.
  await page.getByRole("button", { name: "Zoom in", exact: true }).click();
  await expect(preview).toHaveAttribute("data-zoom-state", "manual");
  const zoomed = await previewImageGeometry(page);
  expect(zoomed.width).toBeGreaterThan(fitted.width);
  expect(zoomed.width).toBeCloseTo(zoomed.naturalWidth * 0.125, 0);
  await expect(level).toHaveText("13%");
});

test("keyboard zoom-in steps the Preview without deciding", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  const preview = page.locator("[data-preview]");
  let stateRequests = 0;
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname.endsWith("/state")
    )
      stateRequests += 1;
  });

  await waitForFit(page);
  const fitted = await previewImageGeometry(page);
  await page.keyboard.press("+");
  await expect(preview).toHaveAttribute("data-zoom-state", "manual");
  const stepped = await previewImageGeometry(page);
  expect(stepped.width).toBeCloseTo(fitted.width * 1.25, 0);
  const first = await zoomLevel(page);
  await page.keyboard.press("=");
  expect(await zoomLevel(page)).toBeGreaterThan(first);
  expect((await previewImageGeometry(page)).width).toBeCloseTo(
    fitted.width * 1.5625,
    0,
  );
  expect(stateRequests).toBe(0);
  await expect(page.getByText("1 / 2")).toBeVisible();
});

test("returning to Grid View resets the zoom state to Fit", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  const preview = page.locator("[data-preview]");
  const slider = page.locator("[data-zoom-slider]");
  await slider.fill("300");
  await slider.dispatchEvent("input");
  await expectRenderedZoom(page, 3);

  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-view]")).toBeVisible();
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");

  await page.locator('[data-photo-index="0"]').click();
  await waitForLoadedReviewImage(page);
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await waitForFit(page);
  const stage = await previewStageGeometry(page);
  const refitted = await previewImageGeometry(page);
  expect(refitted.width).toBeLessThanOrEqual(stage.width + 0.5);
  expect(refitted.height).toBeLessThanOrEqual(stage.height + 0.5);
  // The live percentage reports the composition this Photo's Fit produces.
  await expect(page.locator("[data-zoom-level]")).toHaveText(
    `${Math.round((refitted.width / refitted.naturalWidth) * 100)}%`,
  );
});

test("a manual pan is re-clamped after the Preview area grows", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  const preview = page.locator("[data-preview]");
  const slider = page.locator("[data-zoom-slider]");
  await slider.fill("200");
  await slider.dispatchEvent("input");
  await expectRenderedZoom(page, 2);

  // Panning past the bound stops at the bound.
  const center = await preview.evaluate((surface) => {
    const box = surface.getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
  });
  await page.mouse.move(center.x, center.y);
  await page.mouse.down();
  await page.mouse.move(center.x + 900, center.y);
  await page.mouse.up();
  const narrowStage = await previewStageGeometry(page);
  const panned = await previewImageGeometry(page);
  const narrowLimit = Math.max(0, (panned.width - narrowStage.width) / 2);
  expect(narrowLimit).toBeGreaterThan(0);
  expect(
    panned.left + panned.width / 2 - (narrowStage.left + narrowStage.width / 2),
  ).toBeCloseTo(narrowLimit, 0);

  // The wider Preview area holds the whole Photo again, so the manual pan is
  // re-clamped to zero instead of keeping the stale offset.
  await page.setViewportSize({ width: 1280, height: 800 });
  await expect
    .poll(async () => {
      const stage = await previewStageGeometry(page);
      const image = await previewImageGeometry(page);
      const limit = Math.max(0, (image.width - stage.width) / 2);
      const offset =
        image.left + image.width / 2 - (stage.left + stage.width / 2);
      return Math.abs(offset) - limit;
    })
    .toBeLessThanOrEqual(0.5);
  await expect(page.locator("[data-zoom-level]")).toHaveText("200%");
  await expect(preview).toHaveAttribute("data-zoom-state", "manual");
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
  await expect(page.locator("[data-metadata]")).toContainText("Details");
  await expect(page.locator("[data-metadata-capture-time]")).toHaveText(
    "2026-02-03T04:05:06.000000000",
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

function touchQualification(viewport: { width: number; height: number }) {
  return async ({ page }: { page: Page }) => {
    const { base, root } = await fixture();
    await writePhotos(root, 3);
    const running = await server(base, root);
    await startReview(page, running.url, "All Photos");
    await page.setViewportSize(viewport);

    const photoView = page.locator("[data-photo-view]");
    const preview = page.locator("[data-preview]");
    let stateRequests = 0;
    page.on("request", (request) => {
      if (
        request.method() === "POST" &&
        new URL(request.url()).pathname.endsWith("/state")
      )
        stateRequests += 1;
    });
    await photoView.evaluate((view) => {
      view.scrollTop = 0;
    });
    await preview.evaluate((surface) => {
      surface.addEventListener(
        "pointerdown",
        (event) => {
          const pointerEvent = event as PointerEvent;
          surface.setAttribute(
            "data-observed-pointer",
            `${pointerEvent.pointerType}:${pointerEvent.isTrusted}`,
          );
        },
        { once: true },
      );
    });
    await expect(preview).toHaveCSS("touch-action", "pan-y");
    const gesture = await preview.evaluate((surface) => {
      const previewBox = surface.getBoundingClientRect();
      const viewBox = surface
        .closest("[data-photo-view]")!
        .getBoundingClientRect();
      const top = Math.max(previewBox.top, viewBox.top) + 24;
      const bottom = Math.min(previewBox.bottom, viewBox.bottom) - 24;
      return {
        x: previewBox.left + previewBox.width / 2,
        top,
        bottom,
      };
    });
    expect(gesture.bottom - gesture.top).toBeGreaterThan(80);
    await touchDrag(
      page,
      { x: gesture.x, y: gesture.bottom },
      { x: gesture.x, y: gesture.top },
    );
    await expect
      .poll(() => photoView.evaluate((view) => view.scrollTop))
      .toBeGreaterThan(0);
    await expect(preview).toHaveAttribute(
      "data-observed-pointer",
      "touch:true",
    );
    await expect(page.getByText("1 / 3")).toBeVisible();
    expect(stateRequests).toBe(0);

    await photoView.evaluate((view) => {
      view.scrollTop = 0;
    });
    const horizontal = await preview.evaluate((surface) => {
      const box = surface.getBoundingClientRect();
      return {
        left: box.left + box.width / 2 - 60,
        right: box.left + box.width / 2 + 60,
        y: box.top + box.height / 2,
      };
    });
    let mutation = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname.endsWith("/state") &&
        response.status() === 200,
    );
    await touchDrag(
      page,
      { x: horizontal.left, y: horizontal.y },
      { x: horizontal.right, y: horizontal.y },
    );
    await mutation;
    await expect(page.getByText("2 / 3")).toBeVisible();
    expect(stateRequests).toBe(1);

    mutation = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname.endsWith("/state") &&
        response.status() === 200,
    );
    await touchDrag(
      page,
      { x: horizontal.right, y: horizontal.y },
      { x: horizontal.left, y: horizontal.y },
    );
    await mutation;
    await expect(page.getByText("3 / 3")).toBeVisible();
    expect(stateRequests).toBe(2);
    await page.getByRole("button", { name: "Previous" }).click();
    await expect(page.getByText("Rejected", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Previous" }).click();
    await expect(page.getByText("Selected", { exact: true })).toBeVisible();

    await page.locator("[data-zoom-slider]").fill("400");
    await page.locator("[data-zoom-slider]").dispatchEvent("input");
    await expect(preview).toHaveAttribute("data-zoom-state", "manual");
    await expect(preview).toHaveCSS("touch-action", "none");
    await expectRenderedZoom(page, 4);
    await waitForStageAtRest(page);
    await photoView.evaluate((view) => {
      view.scrollTop = 0;
    });
    const before = await previewImageGeometry(page);
    const detailGesture = await preview.evaluate((surface) => {
      const box = surface.getBoundingClientRect();
      return {
        from: {
          x: box.left + box.width / 2 - 60,
          y: box.top + box.height / 2,
        },
        to: {
          x: box.left + box.width / 2 + 60,
          y: box.top + box.height / 2 + 36,
        },
      };
    });
    const stateRequestsBeforeDetail = stateRequests;
    await touchDrag(page, detailGesture.from, detailGesture.to);
    const after = await previewImageGeometry(page);
    expect(after.left - before.left).toBeCloseTo(120, 0);
    expect(after.top - before.top).toBeCloseTo(36, 0);
    expect(stateRequests).toBe(stateRequestsBeforeDetail);
    expect(await photoView.evaluate((view) => view.scrollTop)).toBe(0);
    await expect(page.getByText("1 / 3")).toBeVisible();
  };
}

test.describe("touch qualification", () => {
  test.use({ hasTouch: true });

  for (const viewport of [
    { width: 844, height: 390 },
    { width: 667, height: 375 },
  ]) {
    test(
      `fit Preview at ${viewport.width}x${viewport.height} yields real vertical touch scrolling while horizontal decisions and manual zoom pan remain owned`,
      touchQualification(viewport),
    );
  }
});

test("current source and Rating are programmatic states and Back to Grid restores Photo focus", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Accessible Review");
  await startReview(page, running.url, "Accessible Review", albumId);

  await openSources(page);
  const currentSource = page.getByRole("button", {
    name: /^Accessible Review 2 Photos/,
  });
  await expect(currentSource).toHaveAttribute("aria-current", "true");
  await expect(
    page.getByRole("button", { name: /^All Photos 2 Photos/ }),
  ).not.toHaveAttribute("aria-current");
  await page.getByRole("button", { name: "Close", exact: true }).click();

  const ratingNames = [
    "Clear Rating, 0 stars",
    "Rate 1 star",
    "Rate 2 stars",
    "Rate 3 stars",
    "Rate 4 stars",
    "Rate 5 stars",
  ];
  for (const name of ratingNames)
    await expect(page.getByRole("button", { name, exact: true })).toBeVisible();

  const zero = page.getByRole("button", {
    name: "Clear Rating, 0 stars",
  });
  await expect(
    page.getByRole("button", {
      name: "Clear Rating, 0 stars",
      pressed: true,
    }),
  ).toBeVisible();
  await page.keyboard.press("5");
  await expect(
    page.getByRole("button", { name: "Rate 5 stars", pressed: true }),
  ).toBeVisible();
  await expect(zero).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByText("5 stars", { exact: true })).toBeVisible();

  const back = page.getByRole("button", { name: "Back to Grid" });
  await back.focus();
  expect(
    await back.evaluate((button) => {
      (button as HTMLButtonElement).click();
      return (
        document.activeElement ===
        document.querySelector("[data-grid-viewport]")
      );
    }),
  ).toBe(true);
  await waitForGridFrame(page);
  await expect(page.locator('[data-photo-index="0"]')).toBeFocused();
});

test("Album forms focus their task and restore a stable initiating action", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Keep" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  const newAlbum = page.getByRole("button", { name: "New Album" });
  await newAlbum.click();
  let albumName = page.getByLabel("Album name");
  await expect(albumName).toBeFocused();
  expect(
    await albumName.evaluate((input: HTMLInputElement) => [
      input.selectionStart,
      input.selectionEnd,
    ]),
  ).toEqual([0, 0]);
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(newAlbum).toBeFocused();

  await newAlbum.click();
  albumName = page.getByLabel("Album name");
  await albumName.fill("Created");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(
    page.getByRole("button", { name: /^Created 0 Photos/ }),
  ).toBeVisible();
  await expect(newAlbum).toBeFocused();

  await page.getByRole("button", { name: "Rename Keep" }).click();
  albumName = page.getByLabel("Album name");
  await expect(albumName).toBeFocused();
  expect(
    await albumName.evaluate((input: HTMLInputElement) => [
      input.selectionStart,
      input.selectionEnd,
    ]),
  ).toEqual([0, 4]);

  await page.route("**/api/albums/*/rename", (route) => route.abort());
  await albumName.fill("Lost");
  await page.getByRole("button", { name: "Save Name" }).click();
  await expect(page.getByText("The Album could not be renamed.")).toBeVisible();
  await expect(albumName).toBeFocused();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("button", { name: "Rename Keep" })).toBeFocused();
  await page.unroute("**/api/albums/*/rename");

  await page.getByRole("button", { name: "Rename Keep" }).click();
  albumName = page.getByLabel("Album name");
  await albumName.fill("Kept");
  await page.getByRole("button", { name: "Save Name" }).click();
  await expect(page.getByRole("button", { name: "Rename Kept" })).toBeFocused();

  await page.getByRole("button", { name: "Delete Kept" }).click();
  const confirmDelete = page.getByRole("button", { name: "Delete Album" });
  await expect(confirmDelete).toBeFocused();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("button", { name: "Delete Kept" })).toBeFocused();

  let deleteStarted!: () => void;
  const started = new Promise<void>((resolve) => {
    deleteStarted = resolve;
  });
  let releaseDelete!: () => void;
  const heldDelete = new Promise<void>((resolve) => {
    releaseDelete = resolve;
  });
  await page.route("**/api/albums/*/delete", async (route) => {
    deleteStarted();
    await heldDelete;
    await route.abort();
  });
  await page.getByRole("button", { name: "Delete Kept" }).click();
  await confirmDelete.click();
  await started;
  await expect(
    page.getByRole("button", { name: "Cancel", exact: true }),
  ).toBeFocused();
  expect(
    await page.evaluate(() => document.activeElement !== document.body),
  ).toBe(true);
  releaseDelete();
  await expect(page.getByText("The Album could not be deleted.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Delete Kept" })).toBeFocused();
  await page.unroute("**/api/albums/*/delete");

  await page.getByRole("button", { name: "Delete Kept" }).click();
  await confirmDelete.click();
  await expect(
    page.getByRole("button", { name: /^Kept 0 Photos/ }),
  ).toBeHidden();
  await expect(newAlbum).toBeFocused();
});

test("Album names and management actions do not overlap", async ({ page }) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await createAlbum(running.url, "26春节");

  for (const viewport of [
    { width: 1440, height: 900 },
    { width: 390, height: 844 },
  ]) {
    await page.setViewportSize(viewport);
    await page.goto(running.url);
    if (viewport.width === 390)
      await page.locator("[data-source-toggle]").click();

    const row = page.locator(".album-row").filter({
      has: page.getByRole("button", { name: /^26春节 1 Photo/ }),
    });
    const card = row.locator(".source-card");
    const tools = row.locator(".album-tools");
    await expect(card).toBeVisible();
    await expect(tools).toBeVisible();

    const layout = await row.evaluate((container) => {
      const bounds = (selector: string) =>
        (
          container.querySelector(selector) as HTMLElement
        ).getBoundingClientRect();
      const rowBox = container.getBoundingClientRect();
      const cardBox = bounds(".source-card");
      const toolsBox = bounds(".album-tools");
      const label = container.querySelector(
        ".source-card strong",
      ) as HTMLElement;
      return {
        contained:
          cardBox.left >= rowBox.left &&
          cardBox.right <= rowBox.right &&
          toolsBox.left >= rowBox.left &&
          toolsBox.right <= rowBox.right,
        separated: cardBox.bottom <= toolsBox.top,
        labelFits: label.scrollWidth <= label.clientWidth,
      };
    });
    expect(layout).toEqual({
      contained: true,
      separated: true,
      labelFits: true,
    });
  }
});

test("a valid 120-character Album name stays contained in Grid and Photo View", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const emptyAlbumName = "a".repeat(120);
  const populatedAlbumName = "b".repeat(120);
  const createdEmpty = await post(running.url, "/api/albums", {
    name: emptyAlbumName,
  });
  expect(createdEmpty.ok).toBe(true);
  const { albumId } = await createAlbum(running.url, populatedAlbumName);
  const viewports = [
    { width: 1440, height: 900 },
    { width: 900, height: 900 },
    { width: 390, height: 844 },
    { width: 844, height: 390 },
  ];
  const emptyMessage =
    "This Album contains no Photos. Add Photos from another source's Photo View.";

  await openGrid(page, running.url, emptyAlbumName);
  for (const viewport of viewports) {
    await page.setViewportSize(viewport);
    await waitForGridFrame(page);
    await expect(page.locator("[data-grid-title]")).toHaveText(emptyAlbumName);
    await expect(
      page.getByRole("heading", { name: emptyAlbumName, exact: true }),
    ).toBeVisible();
    await expect(page.locator("[data-grid-status]")).toHaveText("0 Photos");
    await expect(page.locator("[data-grid-empty-message]")).toHaveText(
      emptyMessage,
    );
    await expect(page.locator("[data-grid-empty-message]")).toBeVisible();

    const gridLayout = await page
      .locator("[data-grid-view]")
      .evaluate((view) => {
        if (!(view instanceof HTMLElement))
          throw new Error("Grid View is missing");
        const browser = view.closest<HTMLElement>("[data-browser]");
        if (!browser) throw new Error("Library Browser is missing");
        const browserBox = browser.getBoundingClientRect();
        const contained = (element: HTMLElement) => {
          const box = element.getBoundingClientRect();
          return (
            box.left >= browserBox.left - 0.5 &&
            box.right <= browserBox.right + 0.5 &&
            box.top >= browserBox.top - 0.5 &&
            box.bottom <= browserBox.bottom + 0.5
          );
        };
        const elements = [
          view,
          view.querySelector<HTMLElement>(".grid-header"),
          view.querySelector<HTMLElement>("[data-grid-title]"),
          view.querySelector<HTMLElement>("[data-grid-status]"),
          view.querySelector<HTMLElement>("[data-grid-viewport]"),
          view.querySelector<HTMLElement>("[data-grid-empty]"),
          view.querySelector<HTMLElement>("[data-grid-empty-message]"),
        ];
        if (elements.some((element) => !element))
          throw new Error("Grid layout is incomplete");
        return {
          browserClientWidth: browser.clientWidth,
          browserScrollWidth: browser.scrollWidth,
          gridClientWidth: view.clientWidth,
          gridScrollWidth: view.scrollWidth,
          contained: elements.every((element) => contained(element!)),
        };
      });
    expect(gridLayout.browserScrollWidth).toBe(gridLayout.browserClientWidth);
    expect(gridLayout.gridScrollWidth).toBe(gridLayout.gridClientWidth);
    expect(gridLayout.contained).toBe(true);
  }

  await startReview(page, running.url, populatedAlbumName, albumId);
  const photoView = page.locator("[data-photo-view]");
  for (const viewport of viewports) {
    await page.setViewportSize(viewport);
    await expect(photoView).toBeVisible();
    await expect(page.locator("[data-photo-title]")).toHaveText(
      populatedAlbumName,
    );
    await expect(
      page.getByRole("heading", { name: populatedAlbumName, exact: true }),
    ).toBeVisible();

    const photoLayout = await photoView.evaluate((view) => {
      if (!(view instanceof HTMLElement))
        throw new Error("Photo View is missing");
      const browser = view.closest<HTMLElement>("[data-browser]");
      if (!browser) throw new Error("Library Browser is missing");
      const browserBox = browser.getBoundingClientRect();
      const horizontallyContained = (element: HTMLElement) => {
        const box = element.getBoundingClientRect();
        return (
          box.left >= browserBox.left - 0.5 &&
          box.right <= browserBox.right + 0.5
        );
      };
      const elements = [
        view,
        view.querySelector<HTMLElement>(".photo-header"),
        view.querySelector<HTMLElement>("[data-photo-title]"),
        view.querySelector<HTMLElement>("[data-position]"),
        view.querySelector<HTMLElement>(".photo-header-actions"),
        view.querySelector<HTMLElement>(".review-bar"),
        view.querySelector<HTMLElement>(".review-tools"),
      ];
      if (elements.some((element) => !element))
        throw new Error("Photo layout is incomplete");
      return {
        clientWidth: view.clientWidth,
        scrollWidth: view.scrollWidth,
        contained: elements.every((element) => horizontallyContained(element!)),
      };
    });
    expect(photoLayout.scrollWidth).toBe(photoLayout.clientWidth);
    expect(photoLayout.contained).toBe(true);

    const targets = await interactiveGeometry(photoView);
    expect(targets.filter(({ contained }) => !contained)).toEqual([]);
    if (viewport.width <= 760 || viewport.height <= 480)
      expect(
        targets.filter(({ width, height }) => width < 44 || height < 44),
      ).toEqual([]);
  }
});

test("Clear is available only for a decided Photo", async ({ page }) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);

  const clear = page.getByRole("button", { name: "Clear", exact: true });
  await expect(clear).toBeDisabled();
  const layout = await page.locator("[data-photo-view]").evaluate((view) => {
    const bounds = (selector: string) =>
      (view.querySelector(selector) as HTMLElement).getBoundingClientRect();
    const targets = Array.from(
      view.querySelectorAll<HTMLElement>(
        "button:not([hidden]), select:not([hidden])",
      ),
    )
      .filter((target) => target.offsetParent !== null)
      .map((target) => {
        const box = target.getBoundingClientRect();
        return { width: box.width, height: box.height };
      });
    return {
      targets,
      previewHeight: bounds("[data-preview]").height,
      reviewBarHeight: bounds(".review-bar").height,
      reviewToolsHeight: bounds(".review-tools").height,
    };
  });
  expect(
    layout.targets.every(({ width, height }) => width >= 44 && height >= 44),
  ).toBe(true);
  expect(layout.previewHeight).toBeGreaterThan(layout.reviewBarHeight);
  expect(layout.previewHeight).toBeGreaterThan(layout.reviewToolsHeight);
  const secondaryContrast = await page
    .locator("[data-photo-view]")
    .evaluate((view) =>
      Array.from(
        view.querySelectorAll<HTMLElement>(
          ".facts dt, .rating-controls legend, .membership-heading",
        ),
        (node) => {
          const surface = node.closest<HTMLElement>(
            ".review-bar, .review-tools",
          )!;
          return {
            foreground: getComputedStyle(node).color,
            background: getComputedStyle(surface).backgroundColor,
          };
        },
      ),
    );
  expect(
    secondaryContrast.every(
      ({ foreground, background }) =>
        contrastRatio(foreground, background) >= 4.5,
    ),
  ).toBe(true);
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await expect(page.getByText("Selected", { exact: true })).toBeVisible();
  await expect(clear).toBeEnabled();
});

test("visible controls and keyboard share mutation, advance, rating independence, and one-level undo semantics", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);

  await actionWithProgress(page, albumId, () => page.keyboard.press("p"));
  await expect(page.getByText("2 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "selected",
  );
  await page.keyboard.press("5");
  await expect(page.getByText("5 stars", { exact: true })).toBeVisible();
  expect((await state(running.url, albumId)).members[1]).toMatchObject({
    selectionState: "undecided",
    rating: 5,
  });
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Reject" }).click(),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await actionWithProgress(page, albumId, () =>
    page.keyboard.press("Control+z"),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
  await expect(page.getByText("Undecided", { exact: true })).toBeVisible();
  await expect(page.getByText("5 stars", { exact: true })).toBeVisible();
  await expect(page.getByText("Last change undone.")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Clear", exact: true }),
  ).toBeDisabled();
  await page.keyboard.press("u");
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
  await expect(page.getByText("2 / 3")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.keyboard.press("ArrowRight"),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.keyboard.press("ArrowLeft"),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
});

test("Sources owns the keyboard while the Photo View is inert", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);

  await page.keyboard.press("5");
  await expect(page.getByText("5 stars", { exact: true })).toBeVisible();
  const before = await state(running.url, albumId);
  let mutationRequests = 0;
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname.endsWith("/state")
    )
      mutationRequests += 1;
  });

  await openSources(page);
  await expect(
    page.getByRole("button", { name: "Close", exact: true }),
  ).toBeFocused();
  for (const key of [
    "ArrowLeft",
    "ArrowRight",
    "p",
    "x",
    "u",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "Control+z",
  ])
    await page.keyboard.press(key);

  expect(mutationRequests).toBe(0);
  expect(await state(running.url, albumId)).toEqual(before);

  await page.keyboard.press("Escape");
  await expect(
    page.getByRole("button", { name: "Sources", exact: true }).last(),
  ).toBeFocused();
  await expect(page.getByText("1 / 3")).toBeVisible();
  await expect(page.getByText("5 stars", { exact: true })).toBeVisible();
  await actionWithProgress(page, albumId, () => page.keyboard.press("x"));
  await expect(page.getByText("2 / 3")).toBeVisible();
});

test("fit-mode Pointer Events show pending feedback, ignore below threshold, and commit right/left only on release", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);

  const preview = page.locator("[data-preview]");
  await preview.dispatchEvent("pointerdown", {
    pointerId: 1,
    isPrimary: true,
    clientX: 120,
    clientY: 320,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointermove", {
    pointerId: 1,
    isPrimary: true,
    clientX: 160,
    clientY: 322,
    pointerType: "touch",
  });
  await expect(page.locator("[data-select-feedback]")).toHaveClass(/pending/);
  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "undecided",
  );
  await preview.dispatchEvent("pointerup", {
    pointerId: 1,
    isPrimary: true,
    clientX: 160,
    clientY: 322,
    pointerType: "touch",
  });
  await expect(page.getByText("1 / 3")).toBeVisible();
  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "undecided",
  );

  let releaseMutation!: () => void;
  const mutationReleased = new Promise<void>((resolve) => {
    releaseMutation = resolve;
  });
  await page.route("**/api/photos/*/state", async (route) => {
    await mutationReleased;
    await route.continue();
  });
  const progressAfterMutation = progressResponse(page, albumId);
  await swipe(page, 100, 190);
  await expect(page.getByText("1 / 3")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Back to Grid" }),
  ).toBeDisabled();
  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "undecided",
  );
  releaseMutation();
  await progressAfterMutation;
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.unroute("**/api/photos/*/state");
  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "selected",
  );
  await actionWithProgress(page, albumId, () => swipe(page, 250, 150));
  await expect(page.getByText("3 / 3")).toBeVisible();
  expect((await state(running.url, albumId)).members[1]!.selectionState).toBe(
    "rejected",
  );
});

test("persistence failure and disconnect do not advance or lie, and explicit Retry recovers in place", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);

  await page.route("**/api/photos/*/state", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: '{"error":"Mutation could not be persisted"}',
    }),
  );
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect(page.getByText(/could not be saved/)).toBeVisible();
  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "undecided",
  );
  await page.unroute("**/api/photos/*/state");

  await page.route("**/api/photos/*/state", (route) => route.abort());
  await page.getByRole("button", { name: "Reject" }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Zoom in" })).toBeEnabled();
  await page.locator("[data-zoom-slider]").fill("800");
  await page.locator("[data-zoom-slider]").dispatchEvent("input");
  const preview = page.locator("[data-preview]");
  await expectRenderedZoom(page, 8);
  const beforePan = await previewImageGeometry(page);
  await preview.dispatchEvent("pointerdown", {
    pointerId: 71,
    isPrimary: true,
    clientX: 100,
    clientY: 300,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointermove", {
    pointerId: 71,
    isPrimary: true,
    clientX: 140,
    clientY: 330,
    pointerType: "touch",
  });
  const afterPan = await previewImageGeometry(page);
  expect(afterPan.left - beforePan.left).toBeCloseTo(40, 0);
  expect(afterPan.top - beforePan.top).toBeCloseTo(30, 0);
  await preview.dispatchEvent("pointerup", {
    pointerId: 71,
    isPrimary: true,
    clientX: 140,
    clientY: 330,
    pointerType: "touch",
  });
  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "undecided",
  );
  await page.unroute("**/api/photos/*/state");
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Retry" }).click(),
  );
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
});

test("an answered non-conflict Undo failure remains retryable", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);

  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  const undo = page.getByRole("button", { name: "Undo" });
  await expect(undo).toBeEnabled();

  await page.route("**/api/photos/*/state", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: '{"error":"Undo could not be persisted"}',
    }),
  );
  const rejected = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname.endsWith("/state") &&
      response.status() === 503,
  );
  await undo.click();
  await rejected;

  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "selected",
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Undo could not be saved. Try Undo again."),
  ).toBeVisible();
  await expect(undo).toBeEnabled();

  await page.unroute("**/api/photos/*/state");
  await actionWithProgress(page, albumId, () => undo.click());
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect(page.getByText("Undecided", { exact: true })).toBeVisible();
  await expect(page.getByText("Last change undone.")).toBeVisible();
  await expect(undo).toBeDisabled();
  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "undecided",
  );
});

test("a stale answered Undo failure cannot restore Undo into a replacement source", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Undo Source");
  await startReview(page, running.url, "Undo Source", albumId);
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );

  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let intercepted = false;
  await page.route("**/api/photos/*/state", async (route) => {
    intercepted = true;
    await held;
    await route.fulfill({
      status: 503,
      contentType: "application/json",
      body: '{"error":"Undo could not be persisted"}',
    });
  });
  try {
    await page.getByRole("button", { name: "Undo" }).click();
    await expect.poll(() => intercepted).toBe(true);
    await openSources(page);
    await page.getByRole("button", { name: /^All Photos(?: |$)/ }).click();
    await expect(page.locator("[data-grid-title]")).toHaveText("All Photos");
    release();
    await page.getByText(/^Ready · 2 Photos$/).waitFor();
    await page.locator('[data-photo-index="0"]').click();
    await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
    await expect(
      page.getByText("Undo could not be saved. Try Undo again."),
    ).toHaveCount(0);
  } finally {
    release();
    await page.unroute("**/api/photos/*/state");
  }
});

test("an uncertain Undo retires Undo and requires Photo Retry", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );

  await page.route("**/api/photos/*/state", (route) => route.abort());
  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  await expect(
    page.getByText("Connection lost before Undo was confirmed."),
  ).toBeVisible();
  await page.unroute("**/api/photos/*/state");
});

test("stale undo conflict is visible and zoomed horizontal drag pans without mutating; navigation resets fit", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);

  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Undo" }).click(),
  );
  await expect(page.getByText("1 / 2")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  const firstId = (await state(running.url, albumId)).members[0]!.photoId;
  await post(running.url, `/api/photos/${firstId}/state`, {
    field: "selectionState",
    value: "rejected",
  });
  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByText(/no longer available/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Retry" }).click(),
  );
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();

  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await page.keyboard.press("d");
  const preview = page.locator("[data-preview]");
  await expect(preview).toHaveAttribute("data-zoom-state", "manual");
  await swipe(page, 100, 220);
  await expect(page.getByText("1 / 2")).toBeVisible();
  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "rejected",
  );
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Next" }).click(),
  );
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
});

test("Photo View recovery status wraps without hiding Retry or lower controls", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Status Layout");
  await startReview(page, running.url, "Status Layout", albumId);

  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Undo" }).click(),
  );
  await expect(page.getByText("1 / 2")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  const firstId = (await state(running.url, albumId)).members[0]!.photoId;
  await post(running.url, `/api/photos/${firstId}/state`, {
    field: "selectionState",
    value: "rejected",
  });
  await page.getByRole("button", { name: "Undo" }).click();

  const message =
    "Undo is no longer available because the Photo changed elsewhere. Retry to refresh its current state.";
  const status = page.locator("[data-status]");
  await expect(status).toHaveText(message);
  const retry = page.getByRole("button", { name: "Retry", exact: true });
  const photoView = page.locator("[data-photo-view]");
  const photoControls = page.locator(".photo-controls");
  for (const viewport of [
    { width: 390, height: 844 },
    { width: 844, height: 390 },
    { width: 1280, height: 800 },
  ]) {
    await page.setViewportSize(viewport);
    await photoView.evaluate((view) => {
      view.scrollTop = 0;
    });
    const metrics = await status.evaluate((element) => {
      const box = element.getBoundingClientRect();
      return {
        clientWidth: element.clientWidth,
        scrollWidth: element.scrollWidth,
        height: box.height,
      };
    });
    expect(metrics.clientWidth).toBeGreaterThan(0);
    expect(metrics.scrollWidth).toBeLessThanOrEqual(metrics.clientWidth);
    expect(metrics.height).toBeGreaterThan(0);
    await expect(retry).toBeEnabled();
    await expect(retry).toBeInViewport();

    await photoControls.scrollIntoViewIfNeeded();
    const controls = await photoControls.evaluate((controls) => {
      const view = controls.closest<HTMLElement>("[data-photo-view]");
      if (!view) throw new Error("Photo View is missing");
      const viewBox = view.getBoundingClientRect();
      const controlsBox = controls.getBoundingClientRect();
      return {
        contained:
          controlsBox.left >= viewBox.left &&
          controlsBox.right <= viewBox.right &&
          controlsBox.top >= viewBox.top &&
          controlsBox.bottom <= viewBox.bottom,
        buttons: Array.from(controls.querySelectorAll("button"), (button) => {
          const box = button.getBoundingClientRect();
          return (
            box.width >= 44 &&
            box.height >= 44 &&
            box.left >= viewBox.left &&
            box.right <= viewBox.right &&
            box.top >= viewBox.top &&
            box.bottom <= viewBox.bottom
          );
        }),
      };
    });
    expect(controls.contained).toBe(true);
    expect(controls.buttons).toEqual([true, true, true]);
  }

  await retry.click();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(status).toHaveText("Connected. Current state refreshed.");
  const steadyStatus = await status.evaluate((element) => ({
    height: element.getBoundingClientRect().height,
    lineHeight: Number.parseFloat(getComputedStyle(element).lineHeight),
  }));
  expect(steadyStatus.height).toBeLessThan(steadyStatus.lineHeight * 1.5);
});

test("keeps unavailable Photos ordered and allows their decisions without a Preview", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const missing = join(root, "a.jpg");
  await writeFile(missing, await jpeg());
  await writeFile(join(root, "b.jpg"), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  const initial = await state(running.url, albumId);
  await post(running.url, `/api/albums/${albumId}/progress`, {
    photoId: initial.members[0]!.photoId,
  });
  await rm(missing);
  await post(running.url, "/api/scan", {});
  await startReview(page, running.url, "Review", albumId);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await openSources(page);
  await page.getByRole("button", { name: /^Review \d+ Photos/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 2 of 2/ }),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await expect(page.getByText(/Original File is unavailable/)).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  expect((await state(running.url, albumId)).members[0]).toMatchObject({
    available: false,
    selectionState: "selected",
  });
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
    page.getByRole("button", { name: /All Photos 1 Photo/ }),
  ).toBeVisible();

  // Create through the inline form.
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Trip");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(
    page.getByRole("button", { name: /Trip 0 Photos/ }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Rename Trip" })).toBeVisible();

  // Rename keeps membership and identity semantics on the card.
  await page.getByRole("button", { name: "Rename Trip" }).click();
  await page.getByLabel("Album name").fill("Journey");
  await page.getByRole("button", { name: "Save Name" }).click();
  await expect(
    page.getByRole("button", { name: /Journey 0 Photos/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /Trip 0 Photos/ }),
  ).toBeHidden();

  // Deleting requires confirmation and states the safety contract.
  await page.getByRole("button", { name: "Delete Journey" }).click();
  await expect(
    page.getByText("Photos and Original Files remain unchanged."),
  ).toBeVisible();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(
    page.getByRole("button", { name: /Journey 0 Photos/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Delete Journey" }).click();
  await page.getByRole("button", { name: "Delete Album" }).click();
  await expect(
    page.getByRole("button", { name: /Journey 0 Photos/ }),
  ).toBeHidden();
  // Originals are untouched: All Photos keeps its count.
  await expect(
    page.getByRole("button", { name: /All Photos 1 Photo/ }),
  ).toBeVisible();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
});

test("creating an Album opens that exact empty Album on desktop and narrow layouts", async ({
  page,
}) => {
  for (const [index, viewport] of [
    { width: 1440, height: 900 },
    { width: 390, height: 844 },
  ].entries()) {
    const { base, root } = await fixture();
    await writeFile(join(root, "one.jpg"), await jpeg());
    const running = await server(base, root);
    const existingName = `Existing ${index + 1}`;
    const createdName = `Created ${index + 1}`;
    await post(running.url, "/api/albums", { name: existingName });
    await page.setViewportSize(viewport);
    await page.goto(running.url);
    await expect(
      page.getByText("Library ready", { exact: true }),
    ).toBeVisible();

    if (viewport.width === 390) await openSources(page);
    await page
      .getByRole("button", { name: new RegExp(`^${existingName} 0 Photos`) })
      .click();
    await expect(
      page.getByRole("heading", { name: existingName }),
    ).toBeVisible();

    if (viewport.width === 390) await openSources(page);
    await page.getByRole("button", { name: "New Album" }).click();
    await page.getByLabel("Album name").fill(createdName);
    await page.getByRole("button", { name: "Create Album" }).click();

    await expect(
      page.getByRole("heading", { name: createdName }),
    ).toBeVisible();
    await expect(page.locator("[data-grid-status]")).toHaveText("0 Photos");
    await expect(
      page.getByText(
        "This Album contains no Photos. Add Photos from another source's Photo View.",
      ),
    ).toBeVisible();
    if (viewport.width === 390) await openSources(page);
    const created = page.getByRole("button", {
      name: new RegExp(`^${createdName} 0 Photos`),
    });
    await expect(created).toHaveClass(/active/);
    await expect(
      page.getByRole("button", { name: `Rename ${createdName}` }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: `Delete ${createdName}` }),
    ).toBeVisible();
    if (viewport.width === 390)
      await page.getByRole("button", { name: "Close", exact: true }).click();
  }

  await openSources(page);
  await page.route("**/api/albums", async (route) => {
    if (route.request().method() !== "POST") return route.continue();
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        albums: [
          {
            id: "ambiguous-upper-id",
            name: "Ambiguous",
            photoCount: 0,
            hasSavedPosition: false,
          },
          {
            id: "ambiguous-lower-id",
            name: "ambiguous",
            photoCount: 0,
            hasSavedPosition: false,
          },
        ],
      }),
    });
  });
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Ambiguous");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(page.getByText("The Album could not be created.")).toBeVisible();
  await expect(page.getByLabel("Album name")).toHaveValue("Ambiguous");
  await expect(page.locator("[data-grid-title]")).toHaveText("Created 2");
  await page.unroute("**/api/albums");
});

test("a delayed Album creation cannot replace a newer source or Photo", async ({
  page,
}) => {
  for (const changedOwner of ["source", "photo"] as const) {
    const { base, root } = await fixture();
    await writePhotos(root, 2);
    const running = await server(base, root);
    await createAlbum(running.url, "Existing");
    await page.goto(running.url);
    await expect(
      page.getByText("Library ready", { exact: true }),
    ).toBeVisible();
    if (changedOwner === "photo")
      await page.getByRole("button", { name: /^Photo 1 of 2/ }).click();

    let markStarted!: () => void;
    const started = new Promise<void>((resolve) => {
      markStarted = resolve;
    });
    let release!: () => void;
    const released = new Promise<void>((resolve) => {
      release = resolve;
    });
    await page.route("**/api/albums", async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      const response = await route.fetch();
      markStarted();
      await released;
      await route.fulfill({ response });
    });

    await openSources(page);
    await page.getByRole("button", { name: "New Album" }).click();
    const createdName = `Delayed ${changedOwner}`;
    await page.getByLabel("Album name").fill(createdName);
    await page.getByRole("button", { name: "Create Album" }).click();
    await started;

    if (changedOwner === "source") {
      await page.getByRole("button", { name: /^Existing 2 Photos/ }).click();
      await expect(
        page.getByRole("heading", { name: "Existing" }),
      ).toBeVisible();
    } else {
      await page.getByRole("button", { name: "Close", exact: true }).click();
      await page.getByRole("button", { name: "Next" }).click();
      await expect(page.getByText("2 / 2")).toBeVisible();
    }

    release();
    if (changedOwner === "photo") {
      await expect(
        page.getByRole("heading", { name: "All Photos" }),
      ).toBeVisible();
      await expect(page.getByText("2 / 2")).toBeVisible();
      await openSources(page);
    }
    await expect(
      page.getByRole("button", {
        name: new RegExp(`^${createdName} 0 Photos`),
      }),
    ).toBeVisible();
    if (changedOwner === "source") {
      await expect(
        page.getByRole("heading", { name: "Existing" }),
      ).toBeVisible();
      await expect(
        page.getByRole("button", { name: /^Existing 2 Photos/ }),
      ).toHaveAttribute("aria-current", "true");
    } else {
      await expect(page.getByText("2 / 2")).toBeVisible();
    }
    await page.unroute("**/api/albums");
  }
});

test("deleting the open album returns to the All Photos source", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Session" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /Session 0 Photos/ }).click();
  await expect(page.getByRole("heading", { name: "Session" })).toBeVisible();
  await page.getByRole("button", { name: "Delete Session" }).click();
  await page.getByRole("button", { name: "Delete Album" }).click();
  await expect(page.getByRole("heading", { name: "All Photos" })).toBeVisible();
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
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(
    page.getByRole("button", { name: /Picks 1 Photo/ }),
  ).toBeVisible();

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
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(
    page.getByRole("button", { name: /Picks 1 Photo/ }),
  ).toBeVisible();

  // Removing from the open Album source updates the count while the open
  // snapshot keeps its copied order.
  await page.getByRole("button", { name: /Picks 1 Photo/ }).click();
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
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(
    page.getByRole("button", { name: /^Picks 0 Photos$/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /All Photos 1 Photo/ }),
  ).toBeVisible();
});

test("the membership panel lists the current Photo's Albums across sources and reloads", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const longName = "Summer Trip ".repeat(10).trim().slice(0, 120);
  const created = (await (
    await post(running.url, "/api/albums", { name: "Alpha" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const alphaId = created.albums.find((album) => album.name === "Alpha")!.id;
  await post(running.url, "/api/albums", { name: "Beta" });
  await post(running.url, "/api/albums", { name: longName });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();

  // A Photo in no Album states that plainly.
  await expect(page.getByText("Not in any Album yet")).toBeVisible();

  // Managing several Albums lists every one in Album-list order.
  await openMembershipPanel(page);
  await membershipCheckbox(page, "Alpha").check();
  await expect(page.locator("[data-membership-list] li")).toHaveText(["Alpha"]);
  await membershipCheckbox(page, "Beta").check();
  await expect(page.locator("[data-membership-list] li")).toHaveText([
    "Alpha",
    "Beta",
  ]);
  await membershipCheckbox(page, longName).check();
  await expect(page.locator("[data-membership-list] li")).toHaveText([
    "Alpha",
    "Beta",
    longName,
  ]);

  // A long Album name truncates visually, keeps its full text, and stays
  // inside its control group.
  const longest = page.locator("[data-membership-list] li").nth(2);
  const clipped = await longest.evaluate((element) => ({
    text: element.textContent,
    textOverflow: getComputedStyle(element).textOverflow,
    clipped: element.scrollWidth > element.clientWidth,
    right: element.getBoundingClientRect().right,
    containerRight: element.parentElement!.getBoundingClientRect().right,
  }));
  expect(clipped.text).toBe(longName);
  expect(clipped.textOverflow).toBe("ellipsis");
  expect(clipped.clipped).toBe(true);
  expect(clipped.right).toBeLessThanOrEqual(clipped.containerRight + 0.5);

  // The same facts hold in an Album source and after a reload.
  await openSources(page);
  await page.getByRole("button", { name: /^Alpha 1 Photo/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    alphaId,
    page.getByRole("button", { name: /^Photo 1 of 1/ }),
  );
  await expect(page.locator("[data-membership-list] li")).toHaveText([
    "Alpha",
    "Beta",
    longName,
  ]);
  await expect(membershipCheckbox(page, "Alpha")).toBeChecked();
  await expect(membershipCheckbox(page, "Beta")).toBeChecked();

  await page.reload();
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await openSources(page);
  await page.getByRole("button", { name: /^All Photos 1 Photo/ }).click();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.locator("[data-membership-list] li")).toHaveText([
    "Alpha",
    "Beta",
    longName,
  ]);
  await openMembershipPanel(page);
  await expect(membershipCheckbox(page, "Alpha")).toBeChecked();
  await expect(membershipCheckbox(page, "Beta")).toBeChecked();
  await expect(membershipCheckbox(page, longName)).toBeChecked();
});

test("a failed membership read stays retryable without blocking decisions", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Picks" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  await page.route("**/api/photos/*/albums", (route) => route.abort());
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await openMembershipPanel(page);
  await expect(page.getByText("Albums could not be loaded.")).toBeVisible();
  const retry = page.getByRole("button", { name: "Retry Albums" });
  await expect(retry).toBeVisible();

  // Membership being down leaves Preview, selection, Rating, and navigation
  // usable and never claims a disconnection.
  await expect(page.getByText("Disconnected", { exact: true })).toBeHidden();
  await waitForLoadedReviewImage(page);
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("Selected", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Rate 3 stars" }).click();
  await expect(page.getByText("3 stars", { exact: true })).toBeVisible();
  await expect(membershipCheckbox(page, "Picks")).toBeVisible();
  await expect(membershipCheckbox(page, "Picks")).toBeEnabled();

  // Retrying reloads only the membership facts.
  await page.unroute("**/api/photos/*/albums");
  await retry.click();
  await expect(page.getByText("Not in any Album yet")).toBeVisible();
  await expect(page.locator("[data-membership-list] li")).toHaveCount(0);
  await expect(retry).toBeHidden();
});

test("a held membership read is discarded when the Photo changes", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  const created = (await (
    await post(running.url, "/api/albums", { name: "First Only" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const albumId = created.albums.find(
    (album) => album.name === "First Only",
  )!.id;
  const [firstId] = await browseIds(running.url);
  await post(running.url, `/api/albums/${albumId}/members`, {
    photoIds: [firstId],
  });

  // Record when an aborted membership read settles, so the discard is
  // observed deterministically instead of by elapsed time.
  await page.addInitScript(() => {
    const nativeFetch = window.fetch.bind(window);
    window.fetch = (async (...args: Parameters<typeof fetch>) => {
      const input = args[0];
      const url =
        typeof input === "string"
          ? input
          : input instanceof Request
            ? input.url
            : String(input);
      try {
        return await nativeFetch(...args);
      } catch (error) {
        if (url.endsWith("/albums"))
          setTimeout(() => {
            document.documentElement.dataset.membershipAborted = "true";
          }, 0);
        throw error;
      }
    }) as typeof window.fetch;
  });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  let releaseFirst!: () => void;
  const firstReleased = new Promise<void>((resolve) => {
    releaseFirst = resolve;
  });
  let heldOnce = false;
  await page.route("**/api/photos/*/albums", async (route) => {
    if (heldOnce) {
      await route.continue();
      return;
    }
    heldOnce = true;
    await firstReleased;
    // The client aborted this read when the Photo changed.
    await route
      .fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          albums: [{ id: albumId, name: "First Only" }],
        }),
      })
      .catch(() => {});
  });
  try {
    await page.getByRole("button", { name: /^Photo 1 of 2/ }).click();
    await expect(page.getByText("Loading Albums…")).toBeVisible();
    await page.getByRole("button", { name: "Next" }).click();
    await expect(page.getByText("2 / 2")).toBeVisible();
    await expect(page.getByText("Not in any Album yet")).toBeVisible();
    // The superseded read has settled; its Album must never paint.
    await expect(page.locator("html")).toHaveAttribute(
      "data-membership-aborted",
      "true",
    );
    releaseFirst();
    await expect(page.getByText("Not in any Album yet")).toBeVisible();
    await expect(page.locator("[data-membership-list] li")).toHaveCount(0);
    await expect(page.getByText("First Only", { exact: true })).toBeHidden();
  } finally {
    releaseFirst();
    await page.unroute("**/api/photos/*/albums");
  }
});

test("deleting an Album re-verifies the current Photo's membership", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const created = (await (
    await post(running.url, "/api/albums", { name: "Keep" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const keepId = created.albums.find((album) => album.name === "Keep")!.id;
  const doomed = (await (
    await post(running.url, "/api/albums", { name: "Doomed" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const doomedId = doomed.albums.find((album) => album.name === "Doomed")!.id;
  const [photoId] = await browseIds(running.url);
  await post(running.url, `/api/albums/${keepId}/members`, {
    photoIds: [photoId],
  });
  await post(running.url, `/api/albums/${doomedId}/members`, {
    photoIds: [photoId],
  });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.locator("[data-membership-list] li")).toHaveText([
    "Keep",
    "Doomed",
  ]);

  // Deleting an Album the Photo belongs to re-verifies the current facts.
  await openSources(page);
  await page.getByRole("button", { name: "Delete Doomed" }).click();
  await page.getByRole("button", { name: "Delete Album" }).click();
  await page.getByRole("button", { name: "Close", exact: true }).click();
  await expect(page.locator("[data-membership-list] li")).toHaveText(["Keep"]);
  await openMembershipPanel(page);
  await expect(membershipCheckbox(page, "Keep")).toBeChecked();
  await expect(membershipCheckbox(page, "Doomed")).toHaveCount(0);
});

test("the membership panel is operable by keyboard", async ({ page }) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Keyboard" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();

  const manage = page.getByRole("button", { name: "Manage", exact: true });
  await manage.focus();
  await page.keyboard.press("Enter");
  await expect(manage).toHaveAttribute("aria-expanded", "true");

  const membership = membershipCheckbox(page, "Keyboard");
  await membership.focus();
  const added = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname.endsWith("/members") &&
      response.status() === 200,
  );
  await page.keyboard.press("Space");
  await added;
  await expect(membership).toBeChecked();
  // The re-render keeps keyboard focus on the operated checkbox.
  await expect(membership).toBeFocused();
  await expect(page.locator("[data-membership-list] li")).toHaveText([
    "Keyboard",
  ]);
});

test("an older saved-position response cannot supersede a newer Album removal", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Picks");
  await page.addInitScript(() => {
    const nativeFetch = window.fetch.bind(window);
    const instrumentedFetch = async (
      input: Parameters<typeof window.fetch>[0],
      init?: Parameters<typeof window.fetch>[1],
    ) => {
      const response = await nativeFetch(input, init);
      if (
        typeof input === "string" &&
        input.endsWith("/progress") &&
        init?.method === "POST"
      )
        setTimeout(() => {
          document.documentElement.dataset.savedPositionSettled = "true";
        }, 0);
      return response;
    };
    window.fetch = instrumentedFetch as typeof window.fetch;
  });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Picks 1 Photo$/ }).click();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();

  let markProgressPersisted!: () => void;
  const progressPersisted = new Promise<void>((resolve) => {
    markProgressPersisted = resolve;
  });
  let releaseProgress!: () => void;
  const progressHeld = new Promise<void>((resolve) => {
    releaseProgress = resolve;
  });
  let markOverviewCaptured!: () => void;
  const overviewCaptured = new Promise<void>((resolve) => {
    markOverviewCaptured = resolve;
  });
  let releaseOverview!: () => void;
  const overviewHeld = new Promise<void>((resolve) => {
    releaseOverview = resolve;
  });
  let markOverviewDelivered!: () => void;
  const overviewDelivered = new Promise<void>((resolve) => {
    markOverviewDelivered = resolve;
  });
  await page.route("**/api/albums/*/progress", async (route) => {
    const response = await route.fetch();
    markProgressPersisted();
    await progressHeld;
    await route.fulfill({ response });
  });
  await page.route("**/api/overview", async (route) => {
    const response = await route.fetch();
    const body = await response.body();
    const parsed = JSON.parse(body.toString()) as {
      albums: Array<{
        id: string;
        photoCount: number;
        hasSavedPosition: boolean;
      }>;
    };
    expect(parsed.albums.find((album) => album.id === albumId)).toMatchObject({
      photoCount: 0,
      hasSavedPosition: false,
    });
    markOverviewCaptured();
    await overviewHeld;
    try {
      await route.fulfill({ response, body });
    } finally {
      markOverviewDelivered();
    }
  });
  try {
    const savedPositionResponse = progressResponse(page, albumId);
    await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
    await progressPersisted;
    await toggleAlbumMembership(page, "Picks");
    await overviewCaptured;
    releaseProgress();
    const deliveredProgress = await savedPositionResponse;
    await deliveredProgress.finished();
    // The sentinel's next event-loop task runs only after the complete fetch
    // continuation, including the stale Album summary confirmation attempt.
    await expect(page.locator("html")).toHaveAttribute(
      "data-saved-position-settled",
      "true",
    );
    releaseOverview();
    await overviewDelivered;

    await expect(
      page.getByText(
        "Removed from the Album. It stays in this open view until reopened.",
      ),
    ).toBeVisible();
    await expect(page.getByText("1 / 1")).toBeVisible();
    await page.getByRole("button", { name: "Back to Grid" }).click();
    await expect(
      page.getByRole("button", { name: /^Picks 0 Photos$/ }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: /All Photos 1 Photo/ }),
    ).toBeVisible();
    expect((await state(running.url, albumId)).members).toHaveLength(0);
  } finally {
    releaseProgress();
    releaseOverview();
    await page.unroute("**/api/albums/*/progress");
    await page.unroute("**/api/overview");
  }
});

test("a successful membership retry recovers its exact Album connection", async ({
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
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();

  await page.route("**/api/albums/*/members", (route) => route.abort());
  await toggleAlbumMembership(page, "Picks");
  await expect(
    page.getByText("The Photo could not be added to the Album."),
  ).toBeVisible();
  await expect(
    page.getByText("Could not add this Photo to “Picks”."),
  ).toBeVisible();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();

  await page.unroute("**/api/albums/*/members");
  const retried = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === `/api/albums/${albumId}/members` &&
      response.status() === 200,
  );
  // The failed toggle restored the true state instead of wedging the control.
  await expect(membershipCheckbox(page, "Picks")).toBeEnabled();
  await expect(membershipCheckbox(page, "Picks")).not.toBeChecked();
  await membershipCheckbox(page, "Picks").check();
  await retried;

  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
  await expect(page.getByText("Added to the Album.")).toBeVisible();
  await expect(membershipCheckbox(page, "Picks")).toBeChecked();
  await openSources(page);
  await expect(
    page.getByRole("button", { name: /^Picks 1 Photo$/ }),
  ).toBeVisible();
  await expect
    .poll(async () => (await state(running.url, albumId)).members)
    .toHaveLength(1);
});

test("a membership read landing during a failed toggle never strands the panel", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const created = (await (
    await post(running.url, "/api/albums", { name: "Picks" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const albumId = created.albums.find((album) => album.name === "Picks")!.id;
  await trackSettledMembershipReads(page);

  // The read is answered only after the toggle is already in flight, and the
  // toggle fails after that answer reached the page. The answer must not stand
  // in for a panel the failed toggle then restores as loading.
  let releaseRead!: () => void;
  const readHeld = new Promise<void>((resolve) => {
    releaseRead = resolve;
  });
  let releaseWrite!: () => void;
  const writeHeld = new Promise<void>((resolve) => {
    releaseWrite = resolve;
  });
  let markReadArrived!: () => void;
  const readArrived = new Promise<void>((resolve) => {
    markReadArrived = resolve;
  });
  await page.route("**/api/photos/*/albums", async (route) => {
    const response = await route.fetch();
    markReadArrived();
    await readHeld;
    await route.fulfill({ response }).catch(() => {});
  });
  await page.route("**/api/albums/*/members", async (route) => {
    await writeHeld;
    await route.abort();
  });
  try {
    await page.goto(running.url);
    await expect(
      page.getByText("Library ready", { exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
    await openMembershipPanel(page);
    await expect(page.getByText("Loading Albums…")).toBeVisible();
    await readArrived;

    // Loading facts render the checkboxes unchecked, so this click only
    // states the intent; the in-flight write owns the panel from here.
    await membershipCheckbox(page, "Picks").click();
    releaseRead();
    // The delivered answer is discarded: a toggle owns the panel until it
    // settles, so the failed toggle still restores its own prior state.
    await expect.poll(() => settledMembershipReads(page)).toBeGreaterThan(0);
    releaseWrite();

    await expect(
      page.getByText("Could not add this Photo to “Picks”."),
    ).toBeVisible();
    await expect(page.getByText("Albums could not be loaded.")).toBeVisible();
    await expect(page.getByText("Loading Albums…")).toBeHidden();
    const retry = page.getByRole("button", { name: "Retry Albums" });
    await expect(retry).toBeVisible();
    await expect(membershipCheckbox(page, "Picks")).toBeEnabled();
    await expect(membershipCheckbox(page, "Picks")).not.toBeChecked();
    expect((await state(running.url, albumId)).members).toEqual([]);

    // Retrying reloads the membership facts and clears the failure.
    await page.unroute("**/api/photos/*/albums");
    await retry.click();
    await expect(page.getByText("Not in any Album yet")).toBeVisible();
    await expect(retry).toBeHidden();
    await expect(membershipCheckbox(page, "Picks")).toBeEnabled();
    await expect(membershipCheckbox(page, "Picks")).not.toBeChecked();
  } finally {
    releaseRead();
    releaseWrite();
    await page.unroute("**/api/photos/*/albums").catch(() => {});
    await page.unroute("**/api/albums/*/members").catch(() => {});
  }
});

test("a membership read never repaints over an in-flight toggle", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const picks = (await (
    await post(running.url, "/api/albums", { name: "Picks" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const picksId = picks.albums.find((album) => album.name === "Picks")!.id;
  const other = (await (
    await post(running.url, "/api/albums", { name: "Other" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const otherId = other.albums.find((album) => album.name === "Other")!.id;
  const [photoId] = await browseIds(running.url);
  await post(running.url, `/api/albums/${picksId}/members`, {
    photoIds: [photoId],
  });

  await trackSettledMembershipReads(page);
  let holdReads = false;
  let releaseReads!: () => void;
  const readsHeld = new Promise<void>((resolve) => {
    releaseReads = resolve;
  });
  let markReadHeld!: () => void;
  const heldReadArrived = new Promise<void>((resolve) => {
    markReadHeld = resolve;
  });
  await page.route("**/api/photos/*/albums", async (route) => {
    if (!holdReads) {
      await route.continue();
      return;
    }
    let response: Awaited<ReturnType<typeof route.fetch>> | undefined;
    try {
      response = await route.fetch();
    } catch {
      response = undefined;
    }
    markReadHeld();
    await readsHeld;
    if (response) await route.fulfill({ response }).catch(() => {});
    else await route.abort().catch(() => {});
  });
  let releaseRemoval!: () => void;
  const removalHeld = new Promise<void>((resolve) => {
    releaseRemoval = resolve;
  });
  let markRemovalArrived!: () => void;
  const removalArrived = new Promise<void>((resolve) => {
    markRemovalArrived = resolve;
  });
  await page.route(`**/api/albums/${picksId}/members/remove`, async (route) => {
    markRemovalArrived();
    await removalHeld;
    await route.continue();
  });
  try {
    await page.goto(running.url);
    await expect(
      page.getByText("Library ready", { exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
    await openMembershipPanel(page);
    await expect(membershipCheckbox(page, "Picks")).toBeChecked();

    // Adding to another Album leaves that Album's membership read unanswered.
    holdReads = true;
    await membershipCheckbox(page, "Other").check();
    await expect(page.getByText("Added to the Album.")).toBeVisible();
    await heldReadArrived;

    // Removing Picks is admitted while the read is still unanswered.
    await membershipCheckbox(page, "Picks").uncheck();
    await removalArrived;
    await expect(membershipCheckbox(page, "Picks")).not.toBeChecked();
    await expect(membershipCheckbox(page, "Picks")).toBeDisabled();

    // The answer still holds the pre-write membership. It must not repaint
    // the checkbox the visitor is operating.
    releaseReads();
    await expect.poll(() => settledMembershipReads(page)).toBeGreaterThan(1);
    await expect(membershipCheckbox(page, "Picks")).not.toBeChecked();
    await expect(membershipCheckbox(page, "Picks")).toBeDisabled();
    await expect(page.locator("[data-membership-list] li")).toHaveText([
      "Other",
    ]);

    releaseRemoval();
    await expect(page.getByText("Removed from the Album.")).toBeVisible();
    await expect(page.locator("[data-membership-list] li")).toHaveText([
      "Other",
    ]);
    await expect(membershipCheckbox(page, "Picks")).toBeEnabled();
    await expect(membershipCheckbox(page, "Picks")).not.toBeChecked();
    await expect
      .poll(async () => (await state(running.url, picksId)).members)
      .toHaveLength(0);
    await expect
      .poll(async () => (await state(running.url, otherId)).members)
      .toHaveLength(1);
  } finally {
    releaseReads();
    releaseRemoval();
    await page.unroute("**/api/photos/*/albums").catch(() => {});
    await page
      .unroute(`**/api/albums/${picksId}/members/remove`)
      .catch(() => {});
  }
});

test("different Album membership keys admit independently", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const createdA = (await (
    await post(running.url, "/api/albums", { name: "A" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const albumA = createdA.albums.find((album) => album.name === "A")!.id;
  const createdB = (await (
    await post(running.url, "/api/albums", { name: "B" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const albumB = createdB.albums.find((album) => album.name === "B")!.id;
  await page.goto(running.url);
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await openMembershipPanel(page);

  let releaseA!: () => void;
  const heldA = new Promise<void>((resolve) => {
    releaseA = resolve;
  });
  await page.route(`**/api/albums/${albumA}/members`, async (route) => {
    await heldA;
    await route.continue();
  });
  const requestA = page.waitForResponse((response) =>
    response.url().includes(`/api/albums/${albumA}/members`),
  );
  await membershipCheckbox(page, "A").check();
  // Only the Album with an in-flight toggle is disabled; the other Album
  // stays operable.
  await expect(membershipCheckbox(page, "A")).toBeDisabled();

  await expect(membershipCheckbox(page, "B")).toBeEnabled();
  const requestB = page.waitForResponse((response) =>
    response.url().includes(`/api/albums/${albumB}/members`),
  );
  await membershipCheckbox(page, "B").check();
  await requestB;
  releaseA();
  await requestA;
  await expect
    .poll(async () => {
      const overview = (await (
        await fetch(`${running.url}/api/overview`)
      ).json()) as { albums: Array<{ id: string; photoCount: number }> };
      const countA = overview.albums.find(
        (album) => album.id === albumA,
      )?.photoCount;
      const countB = overview.albums.find(
        (album) => album.id === albumB,
      )?.photoCount;
      return `${countA}:${countB}`;
    })
    .toBe("1:1");
});

test("album management failures are reported without claiming completion", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Keep" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  await page.route("**/api/albums/*/rename", (route) => route.abort());
  await page.getByRole("button", { name: "Rename Keep" }).click();
  await page.getByLabel("Album name").fill("Lost");
  await page.getByRole("button", { name: "Save Name" }).click();
  await expect(page.getByText("The Album could not be renamed.")).toBeVisible();
  await page.unroute("**/api/albums/*/rename");
  await page.getByRole("button", { name: "Save Name" }).click();
  await expect(
    page.getByRole("button", { name: /Lost 0 Photos/ }),
  ).toBeVisible();
});

test("album creation reports duplicates and validates name boundaries by code points", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Trip" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  // Duplicate names fail truthfully with 409 and keep the drafted name.
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Trip");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(
    page.getByText("An Album with this name already exists."),
  ).toBeVisible();
  await expect(page.getByLabel("Album name")).toHaveValue("Trip");

  // Blank names never reach the server.
  await page.getByLabel("Album name").fill("   ");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(page.getByText("Enter an Album name.")).toBeVisible();

  // 120 Unicode characters — including astral pairs that native maxlength
  // would count as 122 UTF-16 units — are accepted exactly like the server's
  // code-point rule.
  const boundary = "a".repeat(118) + "🎉".repeat(2);
  await page.getByLabel("Album name").fill(boundary);
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(
    page.getByRole("button", {
      name: new RegExp(`^${boundary} 0 Photos`),
    }),
  ).toBeVisible();

  // 121 code points are rejected before any request.
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("a".repeat(119) + "🎉".repeat(2));
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(
    page.getByText("Album names are at most 120 characters."),
  ).toBeVisible();
});

test("creating an album from the photo view opens it and makes it available for membership", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  // With no Albums at all the panel says so instead of offering a control.
  await expect(page.getByText("Not in any Album yet")).toBeVisible();
  await openMembershipPanel(page);
  await expect(page.getByText("No Albums yet.")).toBeVisible();

  await openSources(page);
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Fresh");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(page.getByRole("heading", { name: "Fresh" })).toBeVisible();
  await expect(
    page.getByText(
      "This Album contains no Photos. Add Photos from another source's Photo View.",
    ),
  ).toBeVisible();

  await openSources(page);
  await page.getByRole("button", { name: /^All Photos 1 Photo/ }).click();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await openMembershipPanel(page);
  await expect(membershipCheckbox(page, "Fresh")).toBeVisible();
  await membershipCheckbox(page, "Fresh").check();
  await expect(page.getByText("Added to the Album.")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(
    page.getByRole("button", { name: /^Fresh 1 Photo/ }),
  ).toBeVisible();
});

test("a failed removal stays retryable from the photo view", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Retry");
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Retry 1 Photo/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /^Photo 1 of 1/ }),
  );

  await page.route("**/api/albums/*/members/remove", (route) => route.abort());
  await toggleAlbumMembership(page, "Retry");
  await expect(
    page.getByText("The Photo could not be removed from the Album."),
  ).toBeVisible();
  await expect(
    page.getByText("Could not remove this Photo from “Retry”."),
  ).toBeVisible();
  // The failed toggle restored the true state and stays retryable.
  await expect(membershipCheckbox(page, "Retry")).toBeEnabled();
  await expect(membershipCheckbox(page, "Retry")).toBeChecked();
  await page.unroute("**/api/albums/*/members/remove");
  await membershipCheckbox(page, "Retry").uncheck();
  await expect(
    page.getByText(
      "Removed from the Album. It stays in this open view until reopened.",
    ),
  ).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(
    page.getByRole("button", { name: /^Retry 0 Photos/ }),
  ).toBeVisible();
});

test("renaming the open album updates every heading in place", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await createAlbum(running.url, "Before");
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Before 1 Photo/ }).click();
  await expect(page.getByRole("heading", { name: "Before" })).toBeVisible();

  await page.getByRole("button", { name: "Rename Before" }).click();
  await page.getByLabel("Album name").fill("After");
  await page.getByRole("button", { name: "Save Name" }).click();
  await expect(
    page.getByRole("button", { name: /^After 1 Photo/ }),
  ).toBeVisible();
  await expect(page.getByRole("heading", { name: "After" })).toBeVisible();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.getByRole("heading", { name: "After" })).toBeVisible();
});

test("album form operations do not clobber a newer form", async ({ page }) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const firstCreated = (await (
    await post(running.url, "/api/albums", { name: "Alpha" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const firstId = firstCreated.albums.find((item) => item.name === "Alpha")!.id;
  await post(running.url, "/api/albums", { name: "Beta" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  // Hold Alpha's rename while a newer Beta rename form is being edited.
  let release: (() => void) | undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(`**/api/albums/${firstId}/rename`, async (route) => {
    await released;
    await route.continue();
  });
  await page.getByRole("button", { name: "Rename Alpha" }).click();
  await page.getByLabel("Album name").fill("Alpha Two");
  await page.getByRole("button", { name: "Save Name" }).click();
  // While Alpha's request is pending, open and edit Beta's rename form.
  await page.getByRole("button", { name: "Rename Beta" }).click();
  await page.getByLabel("Album name").fill("Beta Two");
  release!();
  // Alpha settles, but Beta's in-progress form and draft survive.
  await expect(
    page.getByRole("button", { name: /^Alpha Two 0 Photos/ }),
  ).toBeVisible();
  await expect(page.getByLabel("Album name")).toHaveValue("Beta Two");
  await page.getByRole("button", { name: "Save Name" }).click();
  await expect(
    page.getByRole("button", { name: /^Beta Two 0 Photos/ }),
  ).toBeVisible();
});

test("a late album success cannot overwrite a newer removal notice", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await createAlbum(running.url, "Hold");
  await post(running.url, "/api/albums", { name: "Other" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Hold 1 Photo/ }).click();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();

  // Hold the membership add while a removal settles first.
  let release: (() => void) | undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/albums/*/members", async (route) => {
    await released;
    await route.continue();
  });
  await openMembershipPanel(page);
  await membershipCheckbox(page, "Other").check();
  await membershipCheckbox(page, "Hold").uncheck();
  const removedNotice = page.getByText(
    "Removed from the Album. It stays in this open view until reopened.",
  );
  await expect(removedNotice).toBeVisible();
  release!();
  // The admitted add still lands in the Album, but its late success cannot
  // overwrite the newer removal notice.
  await openSources(page);
  await expect(
    page.getByRole("button", { name: /^Other 1 Photo/ }),
  ).toBeVisible();
  await expect(removedNotice).toBeVisible();
});

test("a superseded album failure surfaces in the library summary", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await createAlbum(running.url, "Hold");
  await post(running.url, "/api/albums", { name: "Other" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Hold 1 Photo/ }).click();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();

  let fail = false;
  let release: (() => void) | undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/albums/*/members", async (route) => {
    await released;
    if (fail) await route.abort();
    else await route.continue();
  });
  await openMembershipPanel(page);
  await membershipCheckbox(page, "Other").check();
  await membershipCheckbox(page, "Hold").uncheck();
  await expect(
    page.getByText(
      "Removed from the Album. It stays in this open view until reopened.",
    ),
  ).toBeVisible();
  fail = true;
  release!();
  // The superseded failure is not dropped: it surfaces in the Library
  // summary while the Photo status keeps the newer removal notice.
  await openSources(page);
  await expect(
    page.getByText("The Photo could not be added to the Album."),
  ).toBeVisible();
  await expect(
    page.getByText(
      "Removed from the Album. It stays in this open view until reopened.",
    ),
  ).toBeVisible();
  // The superseded transport failure must not disconnect the UI the newer
  // successful action already restored.
  await expect(page.getByText("Disconnected")).toBeHidden();
});

test("a pending delete keeps a newer create form and its draft", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const doomed = (await (
    await post(running.url, "/api/albums", { name: "Doomed" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const doomedId = doomed.albums.find((item) => item.name === "Doomed")!.id;
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  let release: (() => void) | undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(`**/api/albums/${doomedId}/delete`, async (route) => {
    await released;
    await route.continue();
  });
  await page.getByRole("button", { name: "Delete Doomed" }).click();
  await page.getByRole("button", { name: "Delete Album" }).click();
  // While the deletion is pending, open a create form and draft a name.
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Draft");
  release!();
  await expect(
    page.getByRole("button", { name: /^Doomed 0 Photos/ }),
  ).toBeHidden();
  // The newer form and its draft survive the delete settlement.
  await expect(page.getByLabel("Album name")).toHaveValue("Draft");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(
    page.getByRole("button", { name: /^Draft 0 Photos/ }),
  ).toBeVisible();
});

test("a renamed open album reconnects under its new name", async ({ page }) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await createAlbum(running.url, "Before");
  await post(running.url, "/api/albums", { name: "Sibling" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Before 1 Photo/ }).click();
  await expect(page.getByRole("heading", { name: "Before" })).toBeVisible();

  await page.getByRole("button", { name: "Rename Before" }).click();
  await page.getByLabel("Album name").fill("After");
  await page.getByRole("button", { name: "Save Name" }).click();
  await expect(page.getByRole("heading", { name: "After" })).toBeVisible();

  // Disconnect without re-opening the source (so the remembered retry
  // source is still the one captured when the Album was opened), then
  // reconnect: the retry must use the renamed Album, not the stale name.
  await page.route("**/api/albums/*/rename", (route) => route.abort());
  await page.getByRole("button", { name: "Rename Sibling" }).click();
  await page.getByLabel("Album name").fill("Sibling Two");
  await page.getByRole("button", { name: "Save Name" }).click();
  await expect(page.getByText("Disconnected")).toBeVisible();
  await page.unroute("**/api/albums/*/rename");
  await page.getByRole("button", { name: "Retry connection" }).click();
  await expect(page.getByRole("heading", { name: "After" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^After 1 Photo/ }),
  ).toBeVisible();
});

test("an older overview success still bootstraps after a newer reload fails", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);

  let calls = 0;
  let releaseOlder!: () => void;
  const held = new Promise<void>((resolve) => {
    releaseOlder = resolve;
  });
  await page.route("**/api/overview", async (route) => {
    calls += 1;
    if (calls === 1 || calls === 3) {
      await route.fulfill({ status: 503, body: "unavailable" });
      return;
    }
    if (calls === 2) {
      const response = await route.fetch();
      await held;
      await route.fulfill({ response });
      return;
    }
    await route.continue();
  });

  await page.goto(running.url);
  await expect(page.getByText("Disconnected")).toBeVisible();
  const retry = page.getByRole("button", { name: "Retry connection" });
  await retry.click();
  await expect.poll(() => calls).toBe(2);
  // The second foreground reload owns failure presentation, but its failure
  // must not detach the older shared overview request.
  await retry.click();
  await expect.poll(() => calls).toBe(3);
  await expect(page.getByText("Disconnected")).toBeVisible();

  releaseOlder();
  // The valid shared response elects bootstrap, but it cannot release the
  // newer foreground reload's exact failure owner.
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
  await expect(page.getByText("Disconnected")).toBeVisible();
  await expect(
    page.getByText("Could not reach Slipstream. Check the server and retry."),
  ).toBeVisible();

  await page.unroute("**/api/overview");
  await retry.click();
  await expect(page.getByText("Connected")).toBeVisible();
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
});

test("the application status monitor owns scan failure, retry, and completion", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Keep" });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  let command: "rejected" | "held" | "lost" = "rejected";
  let statusMode: "failed" | "idle" | "inspecting" | "cycle" = "failed";
  let cycleStatusCalls = 0;
  let scanCalls = 0;
  let releaseHeldScan!: () => void;
  const heldScan = new Promise<void>((resolve) => {
    releaseHeldScan = resolve;
  });
  await page.route("**/api/status", async (route) => {
    const state =
      statusMode === "cycle"
        ? cycleStatusCalls++ === 0
          ? "applying"
          : "idle"
        : statusMode;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(
        state === "inspecting" ? { state, completed: 3, total: 10 } : { state },
      ),
    });
  });
  await page.route("**/api/scan", async (route) => {
    scanCalls += 1;
    if (command === "rejected") {
      await route.fulfill({ status: 503, body: "unavailable" });
      return;
    }
    if (command === "held") {
      await heldScan;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ state: "inspecting", completed: 3, total: 10 }),
      });
      return;
    }
    await route.abort();
  });

  const retryCheck = page.getByRole("button", {
    name: "Retry Library Check",
  });
  await expect(retryCheck).toBeVisible();
  await expect(retryCheck).toBeInViewport();
  await openSources(page);
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Keep");
  const duplicateAlbum = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/albums" &&
      response.status() === 409,
  );
  await page.getByRole("button", { name: "Create Album" }).click();
  await duplicateAlbum;
  await expect(page.getByLabel("Album name")).toHaveValue("Keep");
  await page.getByRole("button", { name: "Close", exact: true }).click();
  statusMode = "idle";
  await retryCheck.click();
  await expect(page.getByText("Disconnected")).toBeVisible();
  await expect(retryCheck).toBeVisible();
  await expect(page.getByText(/Library check complete/)).toBeHidden();

  command = "held";
  statusMode = "inspecting";
  await retryCheck.click();
  await expect(
    page.locator("[data-grid-summary]").getByText("Starting Library check…"),
  ).toBeInViewport();
  await expect(retryCheck).toBeHidden();
  await expect.poll(() => scanCalls).toBe(2);
  await expect(
    page
      .locator("[data-grid-summary]")
      .getByText("Inspecting Capture Time… 3 / 10"),
  ).toBeInViewport();
  expect(scanCalls).toBe(2);
  releaseHeldScan();
  statusMode = "failed";
  await expect(retryCheck).toBeVisible();

  // A lost HTTP response remains ambiguous until the monitor observes a real
  // non-idle→idle cycle. That monitor completion consumes the command once
  // and releases its exact Recovery claim.
  command = "lost";
  statusMode = "cycle";
  cycleStatusCalls = 0;
  await retryCheck.click();
  await expect(
    page
      .locator("[data-grid-summary]")
      .getByText(
        "Library check complete. Open Browse Snapshots remain unchanged.",
      ),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Refresh Current Source" }),
  ).toBeInViewport();
  await expect(page.getByText("Connected")).toBeVisible();
  await page.getByRole("button", { name: "Refresh Current Source" }).click();
  await expect(
    page.getByRole("button", { name: "Refresh Current Source" }),
  ).toBeHidden();
  await expect(page.locator("[data-grid-summary]")).toHaveText("");
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
  await expect(page.getByText(/Library check complete/)).toBeHidden();
});

test("an Overview failure cannot re-enable an admitted empty-Library check", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  let command: "rejected" | "held" = "rejected";
  let statusState = "failed";
  let scanCalls = 0;
  let overviewCalls = 0;
  let releaseHeldScan!: () => void;
  const heldScan = new Promise<void>((resolve) => {
    releaseHeldScan = resolve;
  });
  let releaseHeldStatus!: () => void;
  const heldStatus = new Promise<void>((resolve) => {
    releaseHeldStatus = resolve;
  });
  await page.route("**/api/status", async (route) => {
    if (statusState === "held") {
      await heldStatus;
      await route.fulfill({ status: 503, body: "unavailable" });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ state: statusState }),
    });
  });
  await page.route("**/api/scan", async (route) => {
    scanCalls += 1;
    if (command === "rejected") {
      await route.fulfill({ status: 503, body: "unavailable" });
      return;
    }
    await heldScan;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ state: "inspecting", completed: 0, total: 1 }),
    });
  });

  const checkLibrary = page.getByRole("button", { name: "Check Library" });
  await expect(checkLibrary).toBeVisible();
  const checkLibraryElement = await checkLibrary.elementHandle();
  expect(checkLibraryElement).not.toBeNull();
  const expectSameCheckLibraryDisabled = async (): Promise<void> => {
    expect(
      await checkLibraryElement!.evaluate((button) => ({
        connected: button.isConnected,
        current: button === document.querySelector("[data-grid-empty-action]"),
        disabled: (button as HTMLButtonElement).disabled,
      })),
    ).toEqual({ connected: true, current: true, disabled: true });
  };
  await checkLibrary.click();
  const retryCheck = page.getByRole("button", {
    name: "Retry Library Check",
  });
  await expect(retryCheck).toBeVisible();

  command = "held";
  statusState = "inspecting";
  await retryCheck.click();
  await expect.poll(() => scanCalls).toBe(2);
  await expect(
    page.locator("[data-grid-summary]").getByText("Inspecting Capture Time…"),
  ).toBeVisible();
  await expectSameCheckLibraryDisabled();
  await openSources(page);
  await expectSameCheckLibraryDisabled();
  await expect(
    page.getByRole("button", { name: "Retry connection" }),
  ).toBeVisible();

  statusState = "held";
  await page.route("**/api/overview", async (route) => {
    overviewCalls += 1;
    await route.fulfill({ status: 503, body: "unavailable" });
  });
  await page.getByRole("button", { name: "Retry connection" }).click();
  await expect.poll(() => overviewCalls).toBe(1);
  await expect(
    page
      .locator("[data-summary-status]")
      .getByText("Could not reach Slipstream. Check the server and retry."),
  ).toBeVisible();
  await page.getByRole("button", { name: "Close", exact: true }).click();
  await expect(
    page
      .locator("[data-grid-summary]")
      .getByText("Could not reach Slipstream. Check the server and retry."),
  ).toBeVisible();
  await expectSameCheckLibraryDisabled();
  expect(scanCalls).toBe(2);

  releaseHeldStatus();
  releaseHeldScan();
});

test("terminal scan completion fences a delayed applying-to-idle status pair", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const initialStatus = (await (
    await fetch(`${running.url}/api/status`)
  ).json()) as { publication: string };
  const nextPublication = "0000000000000002";
  await page.goto(running.url);

  let race = false;
  let statusCalls = 0;
  let statusStarted!: () => void;
  const firstStatusStarted = new Promise<void>((resolve) => {
    statusStarted = resolve;
  });
  let releaseApplying!: () => void;
  const applyingHeld = new Promise<void>((resolve) => {
    releaseApplying = resolve;
  });
  await page.route("**/api/status", async (route) => {
    if (!race) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          state: "failed",
          publication: initialStatus.publication,
        }),
      });
      return;
    }
    statusCalls += 1;
    if (statusCalls === 1) {
      statusStarted();
      await applyingHeld;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          state: "applying",
          publication: initialStatus.publication,
        }),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "idle",
        publication: nextPublication,
        completed: 1,
        total: 1,
      }),
    });
  });
  await page.route("**/api/scan", async (route) => {
    await firstStatusStarted;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "idle",
        publication: nextPublication,
        completed: 1,
        total: 1,
      }),
    });
  });
  let overviewCalls = 0;
  await page.route("**/api/overview", async (route) => {
    overviewCalls += 1;
    await route.continue();
  });

  const retryCheck = page.getByRole("button", {
    name: "Retry Library Check",
  });
  await expect(retryCheck).toBeVisible();
  race = true;
  await retryCheck.click();
  await expect(
    page.locator("[data-summary-status]").getByText(/Library check complete/),
  ).toBeVisible();
  await expect.poll(() => overviewCalls).toBe(1);
  releaseApplying();
  await expect.poll(() => statusCalls).toBeGreaterThanOrEqual(3);
  expect(overviewCalls).toBe(1);
});

test("a stale overview response cannot revert newer album state", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "One" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  // Capture the rename's (older) overview response immediately, then hold
  // its delivery while a newer create's refresh commits first.
  let captured = false;
  let release: (() => void) | undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/overview", async (route) => {
    if (!captured) {
      captured = true;
      const response = await route.fetch();
      await released;
      await route.fulfill({ response });
      return;
    }
    await route.continue();
  });
  await page.getByRole("button", { name: "Rename One" }).click();
  await page.getByLabel("Album name").fill("Two");
  await page.getByRole("button", { name: "Save Name" }).click();
  // The older response is captured (Albums: [Two]) but not yet delivered.
  // Create a newer Album whose refresh commits with both Albums.
  const createdConfirmed = page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/albums") &&
      response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Newest");
  await page.getByRole("button", { name: "Create Album" }).click();
  await createdConfirmed;
  await expect(
    page.getByRole("button", { name: /^Newest 0 Photos/ }),
  ).toBeVisible();
  // Release the stale response: it must be discarded, not applied.
  release!();
  await expect(
    page.getByRole("button", { name: /^Newest 0 Photos/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Two 0 Photos/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Newest 0 Photos/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Two 0 Photos/ }),
  ).toBeVisible();
});

test("publication validation rejects an overview body captured before replacement", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(
    page.getByRole("button", { name: /^All Photos 1 Photo/ }),
  ).toBeVisible();

  let captured!: () => void;
  const capturedOverview = new Promise<void>((resolve) => {
    captured = resolve;
  });
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let first = true;
  await page.route("**/api/overview", async (route) => {
    if (!first) {
      await route.continue();
      return;
    }
    first = false;
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    body.photoCount = 999;
    captured();
    await held;
    await route.fulfill({ response, json: body });
  });
  const capturedResponse = page.waitForResponse((response) =>
    response.url().endsWith("/api/overview"),
  );
  await page.locator("[data-retry]").evaluate((button) => {
    (button as HTMLButtonElement).click();
  });
  await capturedOverview;

  await writeFile(join(root, "two.jpg"), await jpeg());
  const scan = await post(running.url, "/api/scan", {});
  expect(scan.ok).toBe(true);
  release();
  await capturedResponse;
  await expect(
    page.getByRole("button", { name: /^All Photos 999 Photos/ }),
  ).toBeHidden();
  // A fresh request at the advanced publication floor commits current facts.
  await page.locator("[data-retry]").evaluate((button) => {
    (button as HTMLButtonElement).click();
  });
  await expect(
    page.getByRole("button", { name: /^All Photos 2 Photos/ }),
  ).toBeVisible({ timeout: 10_000 });
});

test("an unpublished Overview cannot replace an already published generation", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(
    page.getByRole("button", { name: /^All Photos 1 Photo/ }),
  ).toBeVisible();
  await page.route("**/api/overview", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        published: false,
        photoCount: 0,
        scan: { state: "initializing" },
        albums: [],
      }),
    });
  });
  const response = page.waitForResponse((item) =>
    item.url().endsWith("/api/overview"),
  );
  await page.locator("[data-retry]").evaluate((button) => {
    (button as HTMLButtonElement).click();
  });
  await response;
  await expect(
    page.getByRole("button", { name: /^All Photos 0 Photos/ }),
  ).toBeHidden();
  await expect(
    page.getByRole("button", { name: /^All Photos 1 Photo/ }),
  ).toBeVisible();
});

test("album form re-renders preserve caret position and validation messages", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  // A validation message survives a background source-list re-render.
  const foldersResponded = page.waitForResponse((response) =>
    response.url().includes("/api/file-locations"),
  );
  await page.route("**/api/file-locations*", async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 600));
    await route.continue();
  });
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  await page.getByRole("button", { name: "New Album" }).click();
  const input = page.getByLabel("Album name");
  await input.fill("a".repeat(121));
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(
    page.getByText("Album names are at most 120 characters."),
  ).toBeVisible();
  // Edit down to valid, leaving the caret mid-string.
  await input.fill("Naming");
  await input.evaluate((element) => {
    (element as HTMLInputElement).setSelectionRange(3, 3);
  });
  await foldersResponded;
  await expect(input).toHaveValue("Naming");
  await expect(input).toBeFocused();
  const caret = await input.evaluate((element) =>
    (element as HTMLInputElement).selectionStart ===
    (element as HTMLInputElement).selectionEnd
      ? (element as HTMLInputElement).selectionStart
      : -1,
  );
  expect(caret).toBe(3);
  // The message was cleared by editing; the re-render kept that state.
  await expect(
    page.getByText("Album names are at most 120 characters."),
  ).toBeHidden();
});

test("in-flight membership and delete operations stay disabled across re-renders", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  await writeFile(join(root, "two.jpg"), await jpeg());
  const running = await server(base, root);
  await createAlbum(running.url, "Slow");
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Slow 2 Photos/ }).click();
  let releasePreview!: () => void;
  const previewReleased = new Promise<void>((resolve) => {
    releasePreview = resolve;
  });
  await page.route("**/api/photos/*/preview", async (route) => {
    await previewReleased;
    await route.continue();
  });
  await page.getByRole("button", { name: /^Photo 1 of 2/ }).click();

  // Hold the removal while a background re-render lands: the control must
  // stay disabled and a second removal must not fire.
  let calls = 0;
  let release: (() => void) | undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/albums/*/members/remove", async (route) => {
    calls += 1;
    await released;
    await route.continue();
  });
  let folderDelivered!: () => void;
  const folderDeliveredSettled = new Promise<void>((resolve) => {
    folderDelivered = resolve;
  });
  await page.route("**/api/file-locations*", async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 500));
    await route.continue();
    folderDelivered();
  });
  const slowMembership = membershipCheckbox(page, "Slow");
  const removalSettled = page.waitForResponse(
    (response) =>
      response.url().includes("/members/remove") &&
      response.request().method() === "POST",
  );
  await openMembershipPanel(page);
  await slowMembership.click();
  await expect(slowMembership).toBeDisabled();
  // A routine Preview completion must not silently take ownership from the
  // user-initiated removal while that mutation is still in flight.
  releasePreview();
  await expect(page.locator("[data-stage] img")).toBeVisible();
  await openSources(page);
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  // Deterministically wait until the delayed folder response has been
  // delivered and its re-render landed, then verify the in-flight guard.
  await folderDeliveredSettled;
  await expect(slowMembership).toBeDisabled();
  release!();
  await removalSettled;
  await expect(page.locator("[data-status]")).toContainText(
    "Removed from the Album. It stays in this open view until reopened.",
    { timeout: 15000 },
  );
  await expect(
    page.getByRole("button", { name: /^Slow 1 Photo/ }),
  ).toBeVisible();
  expect(calls).toBe(1);
  // The removed member is no longer a member within the open snapshot.
  await expect(slowMembership).toBeEnabled();
  await expect(slowMembership).not.toBeChecked();
});

test("a current saved-position failure blocks decisions until Photo Retry confirms it", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Position Retry");
  await page.goto(running.url);
  await page.getByRole("button", { name: /^Position Retry 1 Photo/ }).click();

  let progressStatus = 503;
  await page.route("**/api/albums/*/progress", async (route) => {
    await route.fulfill({
      status: progressStatus,
      body: '{"error":"failed"}',
    });
  });
  const failed = progressResponse(page, albumId, 503);
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await failed;
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(
    page.getByText(
      "Album position could not be saved. Retry before making more decisions.",
    ),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();

  progressStatus = 409;
  const stale = progressResponse(page, albumId, 409);
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await stale;
  await expect(page.locator("[data-status]")).toHaveText(
    "Could not refresh this Photo. Retry to continue.",
  );
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Retry", exact: true }),
  ).toBeEnabled();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();

  await page.route("**/api/photos/*/preview", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "ready",
        url: "/review.jpg",
        source: "matching-jpeg",
        stale: true,
        message: "Showing retained Preview.",
      }),
    }),
  );
  const stalePreview = progressResponse(page, albumId, 409);
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await stalePreview;
  await expect(page.locator("[data-status]")).toHaveText(
    "Showing retained Preview.",
  );
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();

  await page.unroute("**/api/photos/*/preview");
  await page.unroute("**/api/albums/*/progress");
  const recovered = progressResponse(page, albumId);
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await recovered;
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
});

test("saved-position confirmation cannot be reverted by an older Overview", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["one.jpg", "two.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Resume Fence");
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  let releaseOverview!: () => void;
  const overviewGate = new Promise<void>((resolve) => {
    releaseOverview = resolve;
  });
  let markOverviewCaptured!: () => void;
  const overviewCaptured = new Promise<void>((resolve) => {
    markOverviewCaptured = resolve;
  });
  let markOverviewDelivered!: () => void;
  const overviewDelivered = new Promise<void>((resolve) => {
    markOverviewDelivered = resolve;
  });
  let held = false;
  await page.route("**/api/overview", async (route) => {
    if (held) {
      await route.continue();
      return;
    }
    held = true;
    const captured = await route.fetch();
    markOverviewCaptured();
    await overviewGate;
    try {
      await route.fulfill({ response: captured });
    } finally {
      markOverviewDelivered();
    }
  });
  try {
    await page.locator("[data-retry]").evaluate((element) => {
      (element as HTMLButtonElement).click();
    });
    await overviewCaptured;

    await page.getByRole("button", { name: /^Resume Fence 2 Photos$/ }).click();
    await expect(page.getByText("Ready · 2 Photos")).toBeVisible();
    await openPhotoAndWaitForProgress(
      page,
      albumId,
      page.getByRole("button", { name: /^Photo 1 of 2/ }),
    );
    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    await openSources(page);
    await expect(
      page.getByRole("button", {
        name: /^Resume Fence 2 Photos · Resume$/,
      }),
    ).toBeVisible();

    releaseOverview();
    await overviewDelivered;
    await expect(
      page.getByText("Library ready", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", {
        name: /^Resume Fence 2 Photos · Resume$/,
      }),
    ).toBeVisible();
  } finally {
    releaseOverview();
    await page.unroute("**/api/overview");
  }
});

test("an answered stale saved-position write is not a disconnection", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  await writeFile(join(root, "two.jpg"), await jpeg());
  const running = await server(base, root);
  await createAlbum(running.url, "Positions");
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  let progressWrites = 0;
  await page.route("**/api/albums/*/progress", (route) => {
    progressWrites += 1;
    return route.fulfill({ status: 404 });
  });
  await page.getByRole("button", { name: /^Positions 2 Photos/ }).click();
  await page.getByRole("button", { name: /^Photo 1 of 2/ }).click();
  // The server answers 404 when the saved member no longer exists; that is
  // an expected stale write, not a connectivity loss. Count writes so the
  // assertion targets the ArrowRight navigation specifically.
  // Let the photo-open write settle first so the baseline is stable.
  await expect.poll(() => progressWrites, { timeout: 5000 }).toBeGreaterThan(0);
  const writesBeforeNavigation = progressWrites;
  await page.keyboard.press("ArrowRight");
  await expect
    .poll(() => progressWrites, { timeout: 5000 })
    .toBe(writesBeforeNavigation + 1);
  await expect(page.getByRole("heading", { name: "Positions" })).toBeVisible();
  await expect(page.getByText("Disconnected")).toBeHidden();
});

test("queued stale saved positions are skipped and sent stale success is silent", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg", "d.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Progress Ownership");
  await startReview(page, running.url, "Progress Ownership", albumId);
  const members = (await state(running.url, albumId)).members;

  let releaseFirst!: () => void;
  let releaseCurrent!: () => void;
  const firstGate = new Promise<void>((resolve) => {
    releaseFirst = resolve;
  });
  const currentGate = new Promise<void>((resolve) => {
    releaseCurrent = resolve;
  });
  const sentPhotoIds: string[] = [];
  await page.route("**/api/albums/*/progress", async (route) => {
    const body = route.request().postDataJSON() as { photoId: string };
    sentPhotoIds.push(body.photoId);
    if (sentPhotoIds.length === 1) await firstGate;
    else if (sentPhotoIds.length === 2) await currentGate;
    await route.continue();
  });

  const firstResponse = progressResponse(page, albumId);
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 4")).toBeVisible();
  await expect.poll(() => sentPhotoIds.length).toBe(1);
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("3 / 4")).toBeVisible();
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("4 / 4")).toBeVisible();

  await page.evaluate(() => {
    (window as typeof window & { sourceMutations?: number }).sourceMutations =
      0;
    const source = document.querySelector("[data-source-list]")!;
    new MutationObserver(() => {
      const state = window as typeof window & { sourceMutations?: number };
      state.sourceMutations = (state.sourceMutations ?? 0) + 1;
    }).observe(source, { childList: true, subtree: true });
  });
  const currentResponse = progressResponse(page, albumId);
  releaseFirst();
  await firstResponse;
  await expect.poll(() => sentPhotoIds.length).toBe(2);
  expect(sentPhotoIds).toEqual([members[1]!.photoId, members[3]!.photoId]);
  expect(
    await page.evaluate(
      () =>
        (window as typeof window & { sourceMutations?: number })
          .sourceMutations ?? 0,
    ),
  ).toBe(0);

  releaseCurrent();
  await currentResponse;
  await page.unroute("**/api/albums/*/progress");
});

test("duplicate album names answer without presenting a disconnection", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Twin" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Twin");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(
    page.getByText("An Album with this name already exists."),
  ).toBeVisible();
  // A 409 conflict is a normal answered request, not a connectivity loss.
  await expect(page.getByText("Disconnected")).toBeHidden();

  // A newer successful action takes the summary back from the notice and
  // releases the channel for later background status writes.
  await page.getByLabel("Album name").fill("Fresh");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(
    page.getByRole("button", { name: /^Fresh 0 Photos/ }),
  ).toBeVisible();
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await expect(
    page.getByText("An Album with this name already exists."),
  ).toBeHidden();
});

test("album form inputs keep focus across background refreshes", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  // A slow File Location response re-renders the source list after the
  // create form is already being edited.
  const foldersResponded = page.waitForResponse((response) =>
    response.url().includes("/api/file-locations"),
  );
  await page.route("**/api/file-locations*", async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 600));
    await route.continue();
  });
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  await page.getByRole("button", { name: "New Album" }).click();
  const input = page.getByLabel("Album name");
  await input.fill("Focused");
  await foldersResponded;
  await expect(input).toHaveValue("Focused");
  await expect(input).toBeFocused();
});

test("source panel album failures report beside the library summary, not the photo status", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Panel" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();

  await page.route("**/api/albums/*/rename", (route) => route.abort());
  await openSources(page);
  await page.getByRole("button", { name: "Rename Panel" }).click();
  await page.getByLabel("Album name").fill("Nowhere");
  await page.getByRole("button", { name: "Save Name" }).click();
  // The failure lands in the Library summary that owns the form.
  await expect(page.getByText("The Album could not be renamed.")).toBeVisible();
  // The Photo-view status is not overwritten by the panel's failure.
  await expect(page.getByRole("status").first()).not.toContainText(
    "could not be renamed",
  );
});

test("an admitted album add completes after switching sources", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Picks" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await openMembershipPanel(page);

  // Hold the membership response while the source changes underneath.
  let release: (() => void) | undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/albums/*/members", async (route) => {
    const request = route.request();
    if (
      request.method() === "POST" &&
      !request.url().includes("/remove") &&
      !request.url().endsWith("/order") &&
      !request.url().endsWith("/progress")
    ) {
      await released;
    }
    await route.continue();
  });
  await membershipCheckbox(page, "Picks").check();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  release!();
  // The admitted mutation still updates the bounded Album list.
  await expect(
    page.getByRole("button", { name: /Picks 1 Photo/ }),
  ).toBeVisible();
  await expect(
    page.getByText("The Photo could not be added to the Album."),
  ).toBeHidden();
});

test("file locations show a bounded tree and open recursive folder sources", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "Trip"));
  await mkdir(join(root, "Trip/day2"));
  await mkdir(join(root, "Trip-extra"));
  await mkdir(join(root, "My Photos"));
  const data = await jpeg();
  for (const name of [
    "root.jpg",
    "Trip/one.jpg",
    "Trip/day2/two.jpg",
    "Trip-extra/three.jpg",
    "My Photos/space.jpg",
  ])
    await writeFile(join(root, name), data);
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Trip" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  // File Locations and Albums remain separate sections; a same-name Folder
  // and Album stay distinguishable by section.
  await expect(page.getByRole("heading", { name: "Folders" })).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Albums" }).first(),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Library Folder/ }),
  ).toBeVisible();

  // Expanding the root loads one bounded direct-child window.
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  await expect(
    page.getByRole("button", { name: /Trip · Subfolders/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Trip-extra 1 Photo" }),
  ).toBeVisible();
  // The same-name Album remains present in its own section.
  await expect(
    page.getByRole("button", { name: /Trip 0 Photos/ }),
  ).toBeVisible();

  // Expanding a child loads its own direct-child window.
  await page.getByRole("button", { name: "Toggle Trip subfolders" }).click();
  await expect(
    page.getByRole("button", { name: /day2 1 Photo/ }),
  ).toBeVisible();

  // Opening the folder source shows the recursive subtree count.
  await page.getByRole("button", { name: /Trip · Subfolders/ }).click();
  await expect(page.getByText("Ready · 2 Photos")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Trip · Folder" }),
  ).toBeVisible();

  // The component-aware rule keeps the same-prefix sibling separate.
  await page.getByRole("button", { name: /Trip-extra 1 Photo/ }).click();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();

  // A Folder name containing a space opens through decoded query values.
  await page.getByRole("button", { name: /My Photos 1 Photo/ }).click();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();

  // The Library Folder root source covers the whole Published Library.
  await page.getByRole("button", { name: /^Library Folder/ }).click();
  await expect(page.getByText("Ready · 5 Photos")).toBeVisible();
});

test("adds the current recursive Folder to an Album from Grid View", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "Trip/day2"), { recursive: true });
  const data = await jpeg();
  await writeFile(join(root, "Trip/one.jpg"), data);
  await writeFile(join(root, "Trip/day2/two.jpg"), data);
  const running = await server(base, root);
  const created = (await (
    await post(running.url, "/api/albums", { name: "Trip Picks" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const albumId = created.albums.find(
    (album) => album.name === "Trip Picks",
  )!.id;

  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await openSources(page);
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  await page.getByRole("button", { name: /Trip · Subfolders/ }).click();
  await expect(
    page.getByText("Ready · 2 Photos", { exact: true }),
  ).toBeVisible();

  await page
    .getByLabel("Add Folder to", { exact: true })
    .selectOption({ label: "Trip Picks" });
  const added = page.waitForResponse(
    (response) =>
      response.url().includes(`/api/albums/${albumId}/folder-members`) &&
      response.request().method() === "POST" &&
      response.status() === 200,
  );
  await page.getByRole("button", { name: "Add Folder", exact: true }).click();
  await added;
  await expect(
    page.getByText("Added 2 Photos. 0 already in the Album.", { exact: true }),
  ).toBeVisible();

  const persisted = await state(running.url, albumId);
  expect(persisted.members).toHaveLength(2);
});

test("an empty Library still shows and opens the Library Folder root", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await openSources(page);
  await expect(
    page.getByRole("button", { name: /^Library Folder 0 Photos/ }),
  ).toBeVisible();
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  await expect(page.getByRole("button", { name: "More Folders" })).toBeHidden();
  await page.getByRole("button", { name: /^Library Folder 0 Photos/ }).click();
  const emptyLibrary = page.getByText(
    "No supported Photos found. Check the Library Folder or add supported files, then run Check Library.",
  );
  await expect(emptyLibrary).toBeVisible();
  await expect(emptyLibrary).toBeInViewport();
  await writeFile(join(root, "added.jpg"), await jpeg());
  let scanCalls = 0;
  let releaseScan!: () => void;
  const scanHeld = new Promise<void>((resolve) => {
    releaseScan = resolve;
  });
  await page.route("**/api/scan", async (route) => {
    scanCalls += 1;
    const response = await route.fetch();
    await scanHeld;
    await route.fulfill({ response });
  });
  const check = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/scan" &&
      response.status() === 200,
  );
  const checkLibrary = page.getByRole("button", { name: "Check Library" });
  await checkLibrary.click();
  await expect(checkLibrary).toBeDisabled();
  await expect(
    page.locator("[data-grid-summary]").getByText("Starting Library check…"),
  ).toBeInViewport();
  expect(scanCalls).toBe(1);
  releaseScan();
  await check;
  await expect(
    page.locator("[data-grid-summary]").getByText(/Library check complete/),
  ).toBeVisible();
  const refreshCurrent = page.getByRole("button", {
    name: "Refresh Current Source",
  });
  await expect(refreshCurrent).toBeInViewport();
  await refreshCurrent.click();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
});

test("an empty All Photos source remains openable after switching away", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await openSources(page);

  const allPhotos = page.getByRole("button", {
    name: /^All Photos 0 Photos/,
  });
  await expect(allPhotos).toBeVisible();
  await expect(allPhotos).toBeEnabled();

  await page.getByRole("button", { name: /^Library Folder 0 Photos/ }).click();
  const emptyLibrary = page.getByText(
    "No supported Photos found. Check the Library Folder or add supported files, then run Check Library.",
  );
  await expect(emptyLibrary).toBeVisible();

  await openSources(page);
  await allPhotos.click();
  await expect(page.locator("[data-grid-title]")).toHaveText("All Photos");
  await expect(emptyLibrary).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Check Library" }),
  ).toBeVisible();
});

test("file location publication values stay unique across server restarts", async () => {
  const { base, root } = await fixture();
  const data = await jpeg();
  await writeFile(join(root, "one.jpg"), data);
  const first = await server(base, root);
  const responseOne = await fetch(
    `${first.url}/api/file-locations?start=0&limit=60`,
  );
  const windowOne = (await responseOne.json()) as { publication: string };
  await first.close();
  const second = await server(base, root);
  const responseTwo = await fetch(
    `${second.url}/api/file-locations?start=0&limit=60`,
  );
  const windowTwo = (await responseTwo.json()) as { publication: string };
  expect(windowOne.publication).not.toBe(windowTwo.publication);
});

test("a failed folder source open reconnects to the same folder, not All Photos", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "shoot"), { recursive: true });
  const data = await jpeg();
  await writeFile(join(root, "shoot/one.jpg"), data);
  await writeFile(join(root, "root.jpg"), data);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  await expect(
    page.getByRole("button", { name: /shoot 1 Photo/ }),
  ).toBeVisible();

  // The first folder-source open fails; the retry must reopen the same
  // folder source instead of silently falling back to All Photos.
  await page.route(
    /\/api\/browse$/,
    async (route) => {
      const request = route.request();
      const body = request.postDataBuffer();
      if (
        request.method() === "POST" &&
        body?.toString().includes("folderPath") &&
        body.toString().includes("shoot")
      )
        await route.abort();
      else await route.continue();
    },
    { times: 1 },
  );
  await page.getByRole("button", { name: /shoot 1 Photo/ }).click();
  await expect(
    page.getByText("Could not load this source. Retry to continue."),
  ).toBeVisible();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "shoot · Folder" }),
  ).toBeVisible();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
});

test("a remembered folder source waits for the File Location binding before reopening", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "shoot"));
  const data = await jpeg();
  await writeFile(join(root, "shoot/one.jpg"), data);
  await writeFile(join(root, "root.jpg"), data);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  await expect(
    page.getByRole("button", { name: /shoot 1 Photo/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: /shoot 1 Photo/ }).click();
  await expect(
    page.getByRole("heading", { name: "shoot · Folder" }),
  ).toBeVisible();

  // Both the Folder-source reopen and the File Location binding fail.
  let folderOpens = 0;
  await page.route(/\/api\/browse/, async (route) => {
    const body = route.request().postDataBuffer()?.toString() ?? "";
    if (route.request().method() === "POST" && body.includes("folderPath")) {
      folderOpens += 1;
      await route.abort();
      return;
    }
    await route.continue();
  });
  await page.route(/\/api\/file-locations/, (route) => route.abort());
  await page.getByRole("button", { name: "Refresh Source" }).click();
  await expect(
    page.getByText("Could not load this source. Retry to continue."),
  ).toBeVisible();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  const opensAfterRefresh = folderOpens;

  // The global Retry cannot bind File Locations, so it must NOT send a
  // publicationless Folder open: the truthful failure stays visible.
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(
    page.getByText("Could not load this source. Retry to continue."),
  ).toBeVisible();
  expect(folderOpens).toBe(opensAfterRefresh);

  // Once the binding and the source route recover, the same Retry reopens
  // the remembered Folder.
  await page.unroute(/\/api\/file-locations/);
  await page.unroute(/\/api\/browse/);
  // The Retry control hides as soon as the overview reconnects, so the
  // click is dispatched before that stability transition can hide it.
  await page.evaluate(() =>
    document.querySelector<HTMLButtonElement>("[data-retry]")?.click(),
  );
  await expect(
    page.getByRole("heading", { name: "shoot · Folder" }),
  ).toBeVisible();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
});

test("delayed File Location responses from a superseded publication are discarded", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "a/sub"), { recursive: true });
  const data = await jpeg();
  await writeFile(join(root, "a/sub/one.jpg"), data);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  await expect(
    page.getByRole("button", { name: /a · Subfolders/ }),
  ).toBeVisible();

  // Deliver one successful child window for `a` only after the publication
  // has been superseded and the browser has reloaded the current root.
  let release: (() => void) | undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(/\/api\/file-locations\?.*parent=a&/, async (route) => {
    const response = await route.fetch();
    await released;
    await route.fulfill({ response });
  });
  await page.getByRole("button", { name: "Toggle a subfolders" }).click();
  await writeFile(join(root, "a/sub/two.jpg"), data);
  await post(running.url, "/api/scan", {});
  await page.waitForFunction(async () => {
    const response = await fetch("/api/overview");
    const overview = (await response.json()) as { scan: { state: string } };
    return overview.scan.state === "idle";
  });
  const rootToggle = page.getByRole("button", {
    name: "Toggle Library Folder subfolders",
  });
  await rootToggle.click();
  await rootToggle.click();
  await expect(
    page.getByText("Library changed. Reloaded folders."),
  ).toBeVisible();

  // The delayed superseded window must not expand `a`: if it had been
  // accepted, this click would collapse it instead of loading the fresh
  // page, and the fresh recursive count would never appear.
  release!();
  await page.getByRole("button", { name: "Toggle a subfolders" }).click();
  await expect(
    page.getByRole("button", { name: /sub 2 Photos/ }),
  ).toBeVisible();
});

test("failed File Location ranges keep siblings and retry only the failed range", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "shoot/nested"), { recursive: true });
  const data = await jpeg();
  await writeFile(join(root, "shoot/one.jpg"), data);
  await writeFile(join(root, "shoot/nested/two.jpg"), data);
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Existing" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  let failing = true;
  await page.route(/\/api\/file-locations\?.*parent=shoot&/, async (route) => {
    if (failing) await route.abort();
    else await route.continue();
  });
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  await expect(
    page.getByRole("button", { name: /shoot · Subfolders/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Toggle shoot subfolders" }).click();
  await expect(
    page.getByText(/Could not load folders \(shoot items 1–60\)/),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Library Folder/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", {
      name: /^Retry Folders \(shoot items 1–60\)/,
    }),
  ).toBeVisible();

  // A successful unrelated root request proves reachability but cannot
  // release the exact failed-range Recovery claim.
  const rootToggle = page.getByRole("button", {
    name: "Toggle Library Folder subfolders",
  });
  await rootToggle.click();
  const rootReloaded = page.waitForResponse(
    (response) =>
      response.url().includes("/api/file-locations?") &&
      !response.url().includes("parent=shoot"),
  );
  await rootToggle.click();
  await rootReloaded;
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(
    page.getByText(/Could not load folders \(shoot items 1–60\)/),
  ).toBeVisible();

  // A lower-priority admitted Album failure settles behind the actionable
  // range owner rather than erasing it. Exact range recovery reveals the
  // pending Album failure.
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Existing");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(
    page.getByText(/Could not load folders \(shoot items 1–60\)/),
  ).toBeVisible();
  await expect(
    page.getByText("An Album with this name already exists."),
  ).toBeHidden();

  failing = false;
  await page.getByRole("button", { name: /^Retry Folders/ }).click();
  // Retrying loads only the failed range: the sibling child appears while
  // the already loaded root navigation stays intact.
  await expect(
    page.getByRole("button", { name: /nested 1 Photo/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /shoot · Subfolders/ }),
  ).toBeVisible();
  await expect(page.getByText(/Could not load folders/)).toBeHidden();
  await expect(
    page.getByText("An Album with this name already exists."),
  ).toBeVisible();
});

test("independent failed File Location parents keep exact retry ownership", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const data = await jpeg();
  for (const parent of ["a", "b"]) {
    await mkdir(join(root, parent, "nested"), { recursive: true });
    await writeFile(join(root, parent, "one.jpg"), data);
    await writeFile(join(root, parent, "nested", "two.jpg"), data);
  }
  const running = await server(base, root);
  await page.goto(running.url);
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  await expect(
    page.getByRole("button", { name: /a · Subfolders/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /b · Subfolders/ }),
  ).toBeVisible();

  const failing = new Set(["a", "b"]);
  await page.route("**/api/file-locations*", async (route) => {
    const parent = new URL(route.request().url()).searchParams.get("parent");
    if (parent && failing.has(parent)) {
      await route.abort();
      return;
    }
    await route.continue();
  });
  await page.getByRole("button", { name: "Toggle a subfolders" }).click();
  await page.getByRole("button", { name: "Toggle b subfolders" }).click();
  const retryA = page.getByRole("button", {
    name: /^Retry Folders \(a items 1–60\)/,
  });
  const retryB = page.getByRole("button", {
    name: /^Retry Folders \(b items 1–60\)/,
  });
  await expect(retryA).toBeVisible();
  await expect(retryB).toBeVisible();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();

  failing.delete("a");
  await retryA.click();
  await expect(
    page.getByRole("button", { name: /nested 1 Photo/ }).first(),
  ).toBeVisible();
  await expect(retryA).toBeHidden();
  await expect(retryB).toBeVisible();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();

  failing.delete("b");
  await retryB.click();
  await expect(retryB).toBeHidden();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
});

test("file locations reload coherently when a scan replaces the publication", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "shoot"));
  const data = await jpeg();
  await writeFile(join(root, "shoot/one.jpg"), data);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  await expect(
    page.getByRole("button", { name: /shoot 1 Photo/ }),
  ).toBeVisible();

  // A rescan that adds a Folder supersedes the retained publication.
  await mkdir(join(root, "later"));
  await writeFile(join(root, "later/two.jpg"), data);
  await post(running.url, "/api/scan", {});
  await page.waitForFunction(async () => {
    const response = await fetch("/api/overview");
    const overview = (await response.json()) as { scan: { state: string } };
    return overview.scan.state === "idle";
  });
  // Collapsing and re-expanding sends the superseded publication value; the
  // app reloads one coherent current publication instead of mixing windows.
  const rootToggle = page.getByRole("button", {
    name: "Toggle Library Folder subfolders",
  });
  await rootToggle.click();
  await rootToggle.click();
  await expect(
    page.getByText("Library changed. Reloaded folders."),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /later 1 Photo/ }),
  ).toBeVisible();
});

test("shows empty and no-album start states and only uses same-service requests", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg());
  const running = await server(base, root);
  const methods: string[] = [];
  page.on("request", (request) => methods.push(request.method()));
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await post(running.url, "/api/albums", { name: "Empty" });
  await page.reload();
  const empty = page.getByRole("button", { name: /^Empty \d+ Photos/ });
  await expect(empty).toBeVisible();
  // Empty Albums stay openable: they are valid sources, not disabled cards.
  await expect(empty).toBeEnabled();
  await empty.click();
  await expect(empty).toHaveClass(/active/);
  await expect(
    page.getByText(
      "This Album contains no Photos. Add Photos from another source's Photo View.",
    ),
  ).toBeVisible();
  await createAlbum(running.url, "Ready");
  await page.reload();
  await expect(
    page.getByRole("button", { name: "Ready 1 Photo", exact: true }),
  ).toBeEnabled();
  expect(
    methods.every(
      (method) => method === "GET" || method === "POST" || method === "DELETE",
    ),
    methods.join(","),
  ).toBe(true);
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
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: /^Progress \d+ Photos/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 2 of 3/ }),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.reload();
  await page.getByRole("button", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: /^Progress \d+ Photos/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 2 of 3/ }),
  );
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
  await page.getByRole("button", { name: /^Progress \d+ Photos/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 3 of 3/ }),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
});

test("binds gestures to their starting Photo and covers exact thresholds, cancellation, and disconnect", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);
  const preview = page.locator("[data-preview]");
  const event = async (type: string, x: number, time: number, pointerId = 41) =>
    preview.evaluate(
      (element, value) => {
        const item = new PointerEvent(value.type, {
          pointerId: value.pointerId,
          isPrimary: true,
          clientX: value.x,
          clientY: 320,
          pointerType: "touch",
          bubbles: true,
        });
        Object.defineProperty(item, "timeStamp", { value: value.time });
        element.dispatchEvent(item);
      },
      { type, x, time, pointerId },
    );

  await event("pointerdown", 100, 0);
  await actionWithProgress(page, albumId, () =>
    page.keyboard.press("ArrowRight"),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
  await event("pointerup", 200, 10);
  expect(
    (await state(running.url, albumId)).members
      .slice(0, 2)
      .map((x) => x.selectionState),
  ).toEqual(["undecided", "undecided"]);

  await event("pointerdown", 100, 0, 42);
  await event("pointermove", 171, 1000, 42);
  await event("pointerup", 171, 1000, 42);
  expect((await state(running.url, albumId)).members[1]!.selectionState).toBe(
    "undecided",
  );
  await actionWithProgress(page, albumId, async () => {
    await event("pointerdown", 100, 0, 43);
    await event("pointermove", 172, 1000, 43);
    await event("pointerup", 172, 1000, 43);
  });
  await expect(page.getByText("3 / 3")).toBeVisible();

  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await actionWithProgress(page, albumId, async () => {
    await event("pointerdown", 100, 0, 44);
    await event("pointermove", 148, 50, 44);
    await event("pointerup", 148, 50, 44);
  });
  await expect(page.getByText("3 / 3")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await event("pointerdown", 100, 0, 45);
  await event("pointermove", 148, 1000, 45);
  await event("pointerup", 148, 1000, 45);
  await expect(page.getByText("2 / 3")).toBeVisible();

  await event("pointerdown", 100, 0, 46);
  await event("pointermove", 200, 10, 46);
  await event("pointercancel", 200, 10, 46);
  await expect(page.locator("[data-select-feedback]")).not.toHaveClass(
    /pending/,
  );
  await event("pointerdown", 100, 0, 47);
  await event("pointermove", 200, 10, 47);
  await preview.dispatchEvent("lostpointercapture", { pointerId: 47 });
  await event("pointerup", 200, 10, 47);
  expect((await state(running.url, albumId)).members[1]!.selectionState).toBe(
    "selected",
  );

  await page.route("**/api/photos/*/preview", (route) => route.abort());
  await page.reload();
  await openSources(page);
  await page.getByRole("button", { name: /^Review(?: |$)/ }).click();
  await actionWithProgress(page, albumId, async () => {
    await page.getByRole("button", { name: /Photo 1 of/ }).click();
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  });
  await event("pointerdown", 100, 0, 48);
  await event("pointermove", 200, 10, 48);
  await expect(page.locator("[data-select-feedback]")).not.toHaveClass(
    /pending/,
  );
});

test("keyboard works from focused buttons, real client deltas pan, and uncertain mutation retires undo", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);

  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Next" }).click(),
  );
  await actionWithProgress(page, albumId, () => page.keyboard.press("p"));
  await expect(page.getByText("3 / 3")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await page.keyboard.press("5");
  await expect(page.getByText("5 stars", { exact: true })).toBeVisible();
  await page.locator("[data-zoom-slider]").fill("800");
  await page.locator("[data-zoom-slider]").dispatchEvent("input");
  const preview = page.locator("[data-preview]");
  await expect(preview).toHaveAttribute("data-zoom-state", "manual");
  await expectRenderedZoom(page, 8);
  const beforePan = await previewImageGeometry(page);
  await preview.dispatchEvent("pointerdown", {
    pointerId: 61,
    isPrimary: true,
    clientX: 100,
    clientY: 300,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointermove", {
    pointerId: 61,
    isPrimary: true,
    clientX: 140,
    clientY: 330,
    pointerType: "touch",
  });
  const afterPan = await previewImageGeometry(page);
  expect(afterPan.left).toBeCloseTo(beforePan.left + 40, 0);
  expect(afterPan.top).toBeCloseTo(beforePan.top + 30, 0);
  await preview.dispatchEvent("pointerup", {
    pointerId: 61,
    isPrimary: true,
    clientX: 140,
    clientY: 330,
    pointerType: "touch",
  });
  expect((await state(running.url, albumId)).members[1]!.selectionState).toBe(
    "selected",
  );
  await page.keyboard.press("f");
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await actionWithProgress(page, albumId, () => page.keyboard.press("x"));
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await actionWithProgress(page, albumId, () =>
    page.keyboard.press("Control+z"),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();

  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await page.route("**/api/photos/*/state", async (route) => {
    await route.fetch();
    await route.abort();
  });
  await page.getByRole("button", { name: "Reject" }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
  await page.unroute("**/api/photos/*/state");
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Retry" }).click(),
  );
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
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
  await expect(page.getByText("JPEG", { exact: true })).toBeVisible();
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
  await page.getByRole("button", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: /^Review(?: |$)/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 1 of/ }),
  );
  await waitForLoadedReviewImage(page);
  await expect(
    page.getByText("RAW embedded JPEG", { exact: true }),
  ).toBeVisible();
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

test("Library Review uses server Capture Time order, snapshots it, and stores no progress", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  await writeFile(
    join(root, "A.jpg"),
    withCaptureTime(source, "2026:01:01 10:00:00"),
  );
  await writeFile(
    join(root, "Z.jpg"),
    withCaptureTime(source, "2026:01:01 09:00:00"),
  );
  const running = await server(base, root);
  const ordered = await browseIds(running.url);
  expect(ordered).toHaveLength(2);
  const zId = ordered[0]!;
  const aId = ordered[1]!;
  const previewRequests: string[] = [];
  const stateBodies: unknown[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/preview")) previewRequests.push(request.url());
    if (request.url().includes("/state"))
      stateBodies.push(request.postDataJSON());
  });
  await page.goto(running.url);
  await page.getByRole("button", { name: /Photo 1 of 2/ }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect
    .poll(() => previewRequests.some((url) => url.includes(zId)))
    .toBe(true);
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  expect(stateBodies[0]).toMatchObject({
    field: "selectionState",
    value: "selected",
  });
  expect(stateBodies[0]).not.toHaveProperty("albumId");
  const overview = (await (
    await fetch(`${running.url}/api/overview`)
  ).json()) as {
    albums: unknown[];
  };
  expect(overview.albums).toEqual([]);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: /All Photos/ }).click();
  await page.getByRole("button", { name: /Photo 1 of 2/ }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();

  const { albumId } = await createAlbum(running.url, "Explicit order");
  await post(running.url, `/api/albums/${albumId}/order`, {
    photoIds: [aId, zId],
  });
  await page.reload();
  await page.getByRole("button", { name: /^Explicit order(?: |$)/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 1 of 2/ }),
  );
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect
    .poll(() => previewRequests.some((url) => url.includes(aId)))
    .toBe(true);
});

test("Grid sort offers one explicit Capture Time order and refreshes in that order", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  await writeFile(
    join(root, "A.jpg"),
    withCaptureTime(source, "2026:01:01 10:00:00"),
  );
  await writeFile(
    join(root, "B.jpg"),
    withCaptureTime(source, "2026:01:02 10:00:00"),
  );
  const tie = withCaptureTime(source, "2026:01:03 10:00:00");
  await writeFile(join(root, "C.jpg"), tie);
  await writeFile(join(root, "D.jpg"), tie);
  await writeFile(join(root, "M.jpg"), source);
  const running = await server(base, root);
  const ascending = await browseIds(running.url);
  const descending = await browseOrderedIds(running.url, {
    source: "library",
    order: "capture-time-desc",
  });
  expect(ascending).toHaveLength(5);
  // Only the Capture Time direction reverses: equal times keep their
  // tie-breaker direction, and Photos without a Capture Time stay last.
  expect(descending).toEqual([
    ascending[2],
    ascending[3],
    ascending[1],
    ascending[0],
    ascending[4],
  ]);

  const browseBodies = recordBrowseBodies(page);
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 5 Photos$/)).toBeVisible();
  const sort = page.locator("[data-sort-select]");
  await expect(sort).toBeVisible();
  await expect(sort).toBeEnabled();
  await expect(sort).toHaveAccessibleName("Sort");
  await expect(page.locator("[data-sort-select] option")).toHaveText([
    "Capture Time, earliest first",
    "Capture Time, latest first",
  ]);
  await expect(sort).toHaveValue("source-default");
  await expectGridOrder(page, ascending);

  // The control sits in the Grid's keyboard order.
  await page.locator("[data-grid-viewport]").focus();
  await page.keyboard.press("Shift+Tab");
  await expect(sort).toBeFocused();

  await sort.selectOption("capture-time-desc");
  await expectGridOrder(page, descending);
  await expect(sort).toHaveValue("capture-time-desc");
  expect(browseBodies.at(-1)).toEqual({
    source: "library",
    order: "capture-time-desc",
  });

  // An explicit refresh builds a new snapshot with the selected order.
  await openSources(page);
  await page.getByRole("button", { name: "Refresh Source" }).click();
  await expect(sort).toBeEnabled();
  await expectGridOrder(page, descending);
  await expect(sort).toHaveValue("capture-time-desc");
  expect(browseBodies.at(-1)).toEqual({
    source: "library",
    order: "capture-time-desc",
  });

  // The order belongs to the open view: nothing persists it across reloads.
  await page.reload();
  await expect(page.getByText(/^Ready · 5 Photos$/)).toBeVisible();
  await expect(page.locator("[data-sort-select]")).toHaveValue(
    "source-default",
  );
  await expectGridOrder(page, ascending);
});

test("Grid sort keeps the current Photo by identity and repositions around it", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  for (let index = 0; index < 12; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(2, "0")}.jpg`),
      withCaptureTime(
        source,
        `2026:01:0${1 + Math.floor(index / 6)} ${String(
          10 + (index % 6),
        ).padStart(2, "0")}:00:00`,
      ),
    );
  const running = await server(base, root);
  const ascending = await browseIds(running.url);
  const descending = await browseOrderedIds(running.url, {
    source: "library",
    order: "capture-time-desc",
  });
  expect(ascending).toHaveLength(12);
  expect(descending).toEqual([...ascending].reverse());

  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 12 Photos$/)).toBeVisible();
  const anchorId = ascending[8]!;
  const anchorCell = page.locator('[data-photo-index="8"]');
  await anchorCell.scrollIntoViewIfNeeded();
  await anchorCell.click();
  await expect(page.getByText("9 / 12")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-title]")).toHaveText("All Photos");

  const browseBodies = recordBrowseBodies(page);
  await page.locator("[data-sort-select]").selectOption("capture-time-desc");
  await expectGridOrder(page, descending);
  // The order change keeps the Photo at the current Grid position by ID
  // instead of reopening at the first position.
  expect(browseBodies.at(-1)).toEqual({
    source: "library",
    order: "capture-time-desc",
    photoId: anchorId,
  });
  const position = descending.indexOf(anchorId);
  expect(position).toBe(3);
  const columns = await page.locator(".photo-cell").evaluateAll((cells) => {
    const firstRow = (cells[0] as HTMLElement | undefined)?.style.top;
    return cells.filter((cell) => (cell as HTMLElement).style.top === firstRow)
      .length;
  });
  expect(columns).toBe(2);
  await expect
    .poll(() =>
      page
        .locator("[data-grid-viewport]")
        .evaluate((viewport) => viewport.scrollTop),
    )
    .toBe(Math.floor(position / columns) * 178);
});

test("Album sort defaults to Album order and time views leave positions alone", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  await writeFile(
    join(root, "A.jpg"),
    withCaptureTime(source, "2026:01:01 10:00:00"),
  );
  await writeFile(
    join(root, "B.jpg"),
    withCaptureTime(source, "2026:01:02 10:00:00"),
  );
  await writeFile(join(root, "M.jpg"), source);
  const running = await server(base, root);
  const ascending = await browseIds(running.url);
  const { albumId } = await createAlbum(running.url, "Explicit order");
  const albumOrder = [ascending[2]!, ascending[1]!, ascending[0]!];
  const descending = [ascending[1]!, ascending[0]!, ascending[2]!];
  // Persist a membership order that matches neither time view.
  await post(running.url, `/api/albums/${albumId}/order`, {
    photoIds: albumOrder,
  });

  const browseBodies = recordBrowseBodies(page);
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 3 Photos$/)).toBeVisible();
  await openSources(page);
  await page.getByRole("button", { name: /^Explicit order(?: |$)/ }).click();
  await expect(page.locator("[data-grid-title]")).toHaveText("Explicit order");
  const sort = page.locator("[data-sort-select]");
  await expect(page.locator("[data-sort-select] option")).toHaveText([
    "Album order",
    "Capture Time, earliest first",
    "Capture Time, latest first",
  ]);
  await expect(sort).toHaveValue("source-default");
  await expectGridOrder(page, albumOrder);

  await sort.selectOption("capture-time-asc");
  await expectGridOrder(page, ascending);
  expect(browseBodies.at(-1)).toEqual({
    source: "album",
    albumId,
    order: "capture-time-asc",
  });

  await sort.selectOption("capture-time-desc");
  await expectGridOrder(page, descending);
  expect(browseBodies.at(-1)).toEqual({
    source: "album",
    albumId,
    order: "capture-time-desc",
  });

  // A time view never rewrites persisted membership positions.
  const persisted = await state(running.url, albumId);
  expect(persisted.members.map((member) => member.photoId)).toEqual(albumOrder);
  expect(persisted.members.map((member) => member.position)).toEqual([0, 1, 2]);

  // Another source starts at its own default order...
  await openSources(page);
  await page.getByRole("button", { name: /All Photos/ }).click();
  await expect(page.locator("[data-grid-title]")).toHaveText("All Photos");
  await expect(page.locator("[data-sort-select]")).toHaveValue(
    "source-default",
  );
  await expect(page.locator("[data-sort-select] option")).toHaveText([
    "Capture Time, earliest first",
    "Capture Time, latest first",
  ]);
  await expectGridOrder(page, ascending);

  // ...and reopening this Album returns to its persisted order.
  await openSources(page);
  await page.getByRole("button", { name: /^Explicit order(?: |$)/ }).click();
  await expect(page.locator("[data-sort-select]")).toHaveValue(
    "source-default",
  );
  await expectGridOrder(page, albumOrder);
});

test("a sort open superseded by a newer source open leaves the newer order committed", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  await writeFile(
    join(root, "A.jpg"),
    withCaptureTime(source, "2026:01:01 10:00:00"),
  );
  await writeFile(
    join(root, "B.jpg"),
    withCaptureTime(source, "2026:01:02 10:00:00"),
  );
  const running = await server(base, root);
  const ascending = await browseIds(running.url);
  expect(ascending).toHaveLength(2);

  let releaseDescending = () => {};
  const heldDescending = new Promise<void>((resolve) => {
    releaseDescending = resolve;
  });
  const heldRoutes: Promise<void>[] = [];
  await page.route("**/api/browse", (route) => {
    const request = route.request();
    const body = request.postDataJSON() as { order?: string };
    if (request.method() !== "POST" || body.order !== "capture-time-desc")
      return route.continue();
    heldRoutes.push(
      heldDescending.then(() => route.continue()).catch(() => undefined),
    );
    return undefined;
  });
  const browseBodies = recordBrowseBodies(page);

  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 2 Photos$/)).toBeVisible();
  await expectGridOrder(page, ascending);
  const sort = page.locator("[data-sort-select]");
  await sort.selectOption("capture-time-desc");
  // The control cannot submit a second order while this open is busy.
  await expect(sort).toBeDisabled();
  await expect
    .poll(
      () =>
        browseBodies.filter((body) => body.order === "capture-time-desc")
          .length,
    )
    .toBe(1);

  // A newer source open supersedes the held order change.
  await openSources(page);
  await page.getByRole("button", { name: /All Photos/ }).click();
  await expect(sort).toBeEnabled();
  await expect(sort).toHaveValue("source-default");
  await expectGridOrder(page, ascending);

  releaseDescending();
  await Promise.all(heldRoutes);
  expect(
    browseBodies.filter((body) => body.order === "capture-time-desc"),
  ).toHaveLength(1);
  await expectGridOrder(page, ascending);
  await expect(sort).toHaveValue("source-default");
});

test("a Folder sort change waits for the File Location binding before reopening", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const { base, root } = await fixture();
  await mkdir(join(root, "shoot"));
  const source = await jpeg();
  await writeFile(
    join(root, "shoot/one.jpg"),
    withCaptureTime(source, "2026:01:01 10:00:00"),
  );
  await writeFile(
    join(root, "root.jpg"),
    withCaptureTime(source, "2026:01:02 10:00:00"),
  );
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  await expect(
    page.getByRole("button", { name: /shoot 1 Photo/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: /shoot 1 Photo/ }).click();
  await expect(
    page.getByRole("heading", { name: "shoot · Folder" }),
  ).toBeVisible();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();

  // A completed Library check resets the File Location binding; while that
  // route fails, the open Folder snapshot stays visible but unbound.
  await page.route(/\/api\/file-locations/, (route) => route.abort());
  const bindingAttempted = page.waitForRequest(/\/api\/file-locations/);
  await writeFile(
    join(root, "shoot/two.jpg"),
    withCaptureTime(source, "2026:01:03 10:00:00"),
  );
  await post(running.url, "/api/scan", {});
  await page.waitForFunction(async () => {
    const response = await fetch("/api/overview");
    const overview = (await response.json()) as { scan: { state: string } };
    return overview.scan.state === "idle";
  });
  await bindingAttempted;

  const browseBodies = recordBrowseBodies(page);
  const sort = page.locator("[data-sort-select]");
  await expect(sort).toBeEnabled();
  // The Folder tree failure is what the user sees first; the order change
  // must not add a second, different failure on top of it.
  const connectionBefore = await page.locator("[data-connection]").innerText();
  await sort.selectOption("capture-time-desc");

  // The guard defers the order change instead of sending a Folder open with
  // the superseded publication, so no Browse is attempted at all.
  await expect(
    page.getByText("Could not load this source. Retry to continue."),
  ).toBeVisible();
  expect(browseBodies).toEqual([]);
  // The retained snapshot stays truthful, the connection state is unchanged,
  // and the control still reports the open snapshot's order.
  await expect(
    page.getByRole("button", { name: /^Photo 1 of 1/ }),
  ).toBeVisible();
  await expect
    .poll(() => page.locator("[data-connection]").innerText())
    .toBe(connectionBefore);
  await expect(sort).toHaveValue("source-default");

  // Once the binding route recovers, a fresh open uses the current
  // publication and the selected order applies to the reopened Folder.
  await page.unroute(/\/api\/file-locations/);
  await page.getByRole("button", { name: "Refresh Source" }).click();
  await expect(page.getByText("Ready · 2 Photos")).toBeVisible();
  const publication = (
    (await (await fetch(`${running.url}/api/status`)).json()) as {
      publication: string;
    }
  ).publication;
  await sort.selectOption("capture-time-desc");
  const expected = await browseOrderedIds(running.url, {
    source: "folder",
    folderPath: "shoot",
    publication,
    order: "capture-time-desc",
  });
  expect(expected).toHaveLength(2);
  await expectGridOrder(page, expected);
  expect(browseBodies.at(-1)).toEqual({
    source: "folder",
    folderPath: "shoot",
    publication,
    order: "capture-time-desc",
  });
});

test("an Album sort change keeps the current Photo and its resume identity", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const { base, root } = await fixture();
  const source = await jpeg();
  for (let index = 0; index < 4; index += 1)
    await writeFile(
      join(root, `member-${index}.jpg`),
      withCaptureTime(source, `2026:01:0${index + 1} 10:00:00`),
    );
  const running = await server(base, root);
  const ascending = await browseIds(running.url);
  expect(ascending).toHaveLength(4);
  const { albumId } = await createAlbum(running.url, "Anchored");
  // Persist an Album order that matches neither Capture Time direction.
  const albumOrder = [
    ascending[2]!,
    ascending[0]!,
    ascending[3]!,
    ascending[1]!,
  ];
  await post(running.url, `/api/albums/${albumId}/order`, {
    photoIds: albumOrder,
  });
  const descending = [...ascending].reverse();
  const anchorId = albumOrder[1]!;
  const anchorPosition = descending.indexOf(anchorId);
  expect(anchorPosition).toBe(3);

  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 4 Photos$/)).toBeVisible();
  await openSources(page);
  await page.getByRole("button", { name: /^Anchored(?: |$)/ }).click();
  await expect(page.locator("[data-grid-title]")).toHaveText("Anchored");
  await expectGridOrder(page, albumOrder);

  // Open the second Album member, then change the order from the Grid.
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /^Photo 2 of 4/ }),
  );
  await expect(page.getByText("2 / 4")).toBeVisible();
  await expect
    .poll(async () => (await state(running.url, albumId)).position)
    .toBe(1);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-layer]")).toBeVisible();

  const browseBodies = recordBrowseBodies(page);
  const sort = page.locator("[data-sort-select]");
  await sort.selectOption("capture-time-desc");
  await expectGridOrder(page, descending);
  // The Album reopen carries the Album identity and the current Photo anchor.
  expect(browseBodies.at(-1)).toEqual({
    source: "album",
    albumId,
    order: "capture-time-desc",
    photoId: anchorId,
  });

  // The same Photo stays current at its new position in the changed order.
  await page.locator(`[data-photo-index="${anchorPosition}"]`).click();
  await expect(page.getByText(`${anchorPosition + 1} / 4`)).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-layer]")).toBeVisible();
  // A time view never rewrites the persisted Album positions.
  const persisted = await state(running.url, albumId);
  expect(persisted.members.map((member) => member.photoId)).toEqual(albumOrder);
  expect(persisted.members.map((member) => member.position)).toEqual([
    0, 1, 2, 3,
  ]);

  // Leaving and reopening the Album resumes by Photo identity under its own
  // default order instead of by the last view position index.
  await openSources(page);
  await page.getByRole("button", { name: /All Photos/ }).click();
  await expect(page.locator("[data-grid-title]")).toHaveText("All Photos");
  await openSources(page);
  await page.getByRole("button", { name: /^Anchored(?: |$)/ }).click();
  await expect(page.locator("[data-sort-select]")).toHaveValue(
    "source-default",
  );
  await expectGridOrder(page, albumOrder);
  await expect(
    page.getByRole("button", { name: /^Photo 2 of 4/ }),
  ).toBeVisible();
});

test("Previous and Next follow the order selected from the Grid", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const { base, root } = await fixture();
  const source = await jpeg();
  for (let index = 0; index < 5; index += 1)
    await writeFile(
      join(root, `step-${index}.jpg`),
      withCaptureTime(source, `2026:02:0${index + 1} 10:00:00`),
    );
  const running = await server(base, root);
  const ascending = await browseIds(running.url);
  expect(ascending).toHaveLength(5);
  const descending = await browseOrderedIds(running.url, {
    source: "library",
    order: "capture-time-desc",
  });
  expect(descending).toEqual([...ascending].reverse());
  const anchorId = ascending[3]!;
  const anchorPosition = descending.indexOf(anchorId);
  expect(anchorPosition).toBe(1);

  // The displayed Preview's URL names the current Photo, so navigation can be
  // checked by identity instead of by request arrival order.
  const currentPreviewId = () =>
    page
      .locator("[data-stage] img")
      .evaluate((image) =>
        new URL(image.getAttribute("src") ?? "", location.origin).pathname
          .split("/")
          .at(-3),
      );
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 5 Photos$/)).toBeVisible();
  const anchorCell = page.locator('[data-photo-index="3"]');
  await anchorCell.click();
  await expect(page.getByText("4 / 5")).toBeVisible();
  await expect.poll(currentPreviewId).toBe(anchorId);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-layer]")).toBeVisible();

  const browseBodies = recordBrowseBodies(page);
  await page.locator("[data-sort-select]").selectOption("capture-time-desc");
  await expectGridOrder(page, descending);
  expect(browseBodies.at(-1)).toEqual({
    source: "library",
    order: "capture-time-desc",
    photoId: anchorId,
  });

  // Reopening the anchored Photo uses the new order's position, and the
  // navigation steps through the new order's identity sequence.
  await page.locator(`[data-photo-index="${anchorPosition}"]`).click();
  await expect(page.getByText(`${anchorPosition + 1} / 5`)).toBeVisible();
  await expect.poll(currentPreviewId).toBe(anchorId);
  await page.keyboard.press("ArrowRight");
  await expect(page.getByText(`${anchorPosition + 2} / 5`)).toBeVisible();
  const nextId = descending[anchorPosition + 1]!;
  await expect.poll(currentPreviewId).toBe(nextId);
  await page.keyboard.press("ArrowRight");
  await expect(page.getByText(`${anchorPosition + 3} / 5`)).toBeVisible();
  const finalId = descending[anchorPosition + 2]!;
  await expect.poll(currentPreviewId).toBe(finalId);
  await page.keyboard.press("ArrowLeft");
  await expect(page.getByText(`${anchorPosition + 2} / 5`)).toBeVisible();
  await expect.poll(currentPreviewId).toBe(nextId);
});

test("a Library order switch aligns windows beyond the first Grid window", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  const source = await jpeg();
  const count = 70;
  for (let index = 0; index < count; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(3, "0")}.jpg`),
      withCaptureTime(
        source,
        `2026:03:01 ${String(Math.floor(index / 60)).padStart(2, "0")}:${String(
          index % 60,
        ).padStart(2, "0")}:00`,
      ),
    );
  const running = await server(base, root);
  const ascending = await browseIds(running.url);
  expect(ascending).toHaveLength(count);
  const descending = await browseOrderedIds(running.url, {
    source: "library",
    order: "capture-time-desc",
  });
  expect(descending).toEqual([...ascending].reverse());

  // The current Photo sits in the first window before the switch and in the
  // second one after it, so the reopened snapshot must realign its window.
  const anchorIndex = 5;
  const anchorId = ascending[anchorIndex]!;
  const anchorPosition = descending.indexOf(anchorId);
  expect(anchorPosition).toBe(count - 1 - anchorIndex);

  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 70 Photos$/)).toBeVisible();
  const anchorCell = page.locator(`[data-photo-index="${anchorIndex}"]`);
  await expect(anchorCell).toBeVisible();
  await anchorCell.click();
  await expect(page.getByText(`${anchorIndex + 1} / 70`)).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-layer]")).toBeVisible();

  const browseBodies = recordBrowseBodies(page);
  await page.locator("[data-sort-select]").selectOption("capture-time-desc");
  expect(browseBodies.at(-1)).toEqual({
    source: "library",
    order: "capture-time-desc",
    photoId: anchorId,
  });
  // The reopened snapshot renders the anchor's region of the new order
  // instead of restarting at its first position: every rendered cell carries
  // the new order's Photo at its own index, contiguously, and the anchor
  // stays current inside the viewport.
  const renderedCells = () =>
    page.evaluate(() =>
      Array.from(document.querySelectorAll<HTMLElement>(".photo-cell")).map(
        (cell) => ({
          index: Number(cell.dataset.photoIndex),
          id:
            new URL(
              cell.querySelector("img")?.getAttribute("src") ?? "",
              location.origin,
            ).pathname
              .split("/")
              .at(-3) ?? "",
        }),
      ),
    );
  await expect
    .poll(async () => {
      const cells = await renderedCells();
      return cells.length > 0 && cells.every((cell) => cell.id !== "");
    })
    .toBe(true);
  const rendered = await renderedCells();
  expect(rendered[0]!.index).toBeGreaterThan(0);
  expect(rendered.map((cell) => cell.index)).toEqual(
    Array.from(
      { length: rendered.length },
      (_, offset) => rendered[0]!.index + offset,
    ),
  );
  for (const cell of rendered) expect(cell.id).toBe(descending[cell.index]);
  // The anchored Photo is the current one again and is scrolled into view.
  const anchored = page.locator(`[data-photo-index="${anchorPosition}"]`);
  await expect(anchored).toBeVisible();
  expect(await renderedCells()).toContainEqual({
    index: anchorPosition,
    id: anchorId,
  });
  expect(
    await anchored.evaluate((cell) => {
      const viewport = cell.closest("[data-grid-viewport]");
      if (!(viewport instanceof HTMLElement)) return false;
      const cellBox = cell.getBoundingClientRect();
      const viewportBox = viewport.getBoundingClientRect();
      return (
        cellBox.bottom > viewportBox.top && cellBox.top < viewportBox.bottom
      );
    }),
  ).toBe(true);
});

test("active Library Review keeps its Capture Time snapshot until the next Session", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  await writeFile(
    join(root, "A.jpg"),
    withCaptureTime(source, "2026:01:01 10:00:00"),
  );
  await writeFile(
    join(root, "Z.jpg"),
    withCaptureTime(source, "2026:01:01 09:00:00"),
  );
  const running = await server(base, root);
  const initialIds = await browseIds(running.url);
  const zId = initialIds[0]!;
  const previews: string[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/preview")) previews.push(request.url());
  });
  await page.goto(running.url);
  await page.getByRole("button", { name: /Photo 1 of 2/ }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect.poll(() => previews.some((url) => url.includes(zId))).toBe(true);
  await writeFile(
    join(root, "B.jpg"),
    withCaptureTime(source, "2026:01:01 08:00:00"),
  );
  await post(running.url, "/api/scan", {});
  await page.route("**/api/photos/*/preview", (route) => route.abort());
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await page.unroute("**/api/photos/*/preview");
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: /All Photos/ }).click();
  await page.getByRole("button", { name: /Photo 1 of 3/ }).click();
  await expect(page.getByText("1 / 3")).toBeVisible();
  const expandedIds = await browseIds(running.url);
  const bId = expandedIds.find((id) => !initialIds.includes(id));
  expect(bId).toBeDefined();
  await expect
    .poll(() => previews.some((url) => url.includes(bId!)))
    .toBe(true);
});

test("Album Review snapshots explicit members across rescan and reconnect", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  await writeFile(
    join(root, "A.jpg"),
    withCaptureTime(source, "2026:01:01 10:00:00"),
  );
  await writeFile(
    join(root, "Z.jpg"),
    withCaptureTime(source, "2026:01:01 09:00:00"),
  );
  const running = await server(base, root);
  const initialIds = await browseIds(running.url);
  const aId = initialIds[1]!;
  const zId = initialIds[0]!;
  const { albumId } = await createAlbum(running.url, "Snapshot");
  await post(running.url, `/api/albums/${albumId}/order`, {
    photoIds: [aId, zId],
  });
  await startReview(page, running.url, "Snapshot", albumId);
  await expect(page.getByText("1 / 2")).toBeVisible();

  await writeFile(
    join(root, "B.jpg"),
    withCaptureTime(source, "2026:01:01 08:00:00"),
  );
  await post(running.url, "/api/scan", {});
  const rescannedIds = await browseIds(running.url);
  const bId = rescannedIds.find((id) => !initialIds.includes(id));
  expect(bId).toBeDefined();
  await post(running.url, `/api/albums/${albumId}/members`, {
    photoIds: [bId!],
  });
  await page.route("**/api/photos/*/preview", (route) => route.abort());
  await actionWithProgress(page, albumId, async () => {
    await page.getByRole("button", { name: "Next" }).click();
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  });
  await page.unroute("**/api/photos/*/preview");
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Retry" }).click(),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await openSources(page);
  await page.getByRole("button", { name: /^Snapshot(?: |$)/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 2 of 3/ }),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
});

test("reconnect retains confirmed undo and a delayed stale progress failure stays silent", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Recovery");
  await startReview(page, running.url, "Recovery", albumId);
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await page.route("**/api/photos/*/preview", (route) => route.abort());
  await actionWithProgress(page, albumId, async () => {
    await page.getByRole("button", { name: "Next" }).click();
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  });
  await page.unroute("**/api/photos/*/preview");
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Retry" }).click(),
  );
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Undo" }).click(),
  );
  await expect(page.getByText("1 / 3")).toBeVisible();

  let releaseFailure!: () => void;
  const failureReleased = new Promise<void>((resolve) => {
    releaseFailure = resolve;
  });
  let failed = false;
  await page.route("**/api/albums/*/progress", async (route) => {
    if (!failed) {
      failed = true;
      await failureReleased;
      await route.fulfill({ status: 503, body: '{"error":"failed"}' });
      return;
    }
    await route.continue();
  });
  const progressFailed = progressResponse(page, albumId, 503);
  const progressAfterFailure = progressResponse(page, albumId);
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("3 / 3")).toBeVisible();
  releaseFailure();
  await progressFailed;
  await progressAfterFailure;
  // The failed write belongs to the Photo that initiated it. The newer Photo
  // and its confirmed position remain current and are not disconnected by a
  // stale settlement.
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
  await page.unroute("**/api/albums/*/progress");
  await expect
    .poll(async () => (await state(running.url, albumId)).position)
    .toBe(2);
});

test("Album resume wraps past an unavailable saved member and retains it when all are unavailable", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Resume");
  const initial = await state(running.url, albumId);
  const savedId = initial.members[2]!.photoId;
  expect(
    initial.members.findIndex((member) => member.photoId === savedId),
  ).toBe(2);
  await post(running.url, `/api/albums/${albumId}/progress`, {
    photoId: savedId,
  });
  await rm(join(root, "c.jpg"));
  await post(running.url, "/api/scan", {});
  await page.goto(running.url);
  await page.getByRole("button", { name: /^Resume(?: |$)/ }).click();
  // The saved position becomes durable only when the page's progress write
  // is confirmed, and that write is asynchronous with Photo View. Waiting
  // for the confirmed POST removes the race where a reload could discard a
  // pending write and leave the saved member unchanged.
  const progressConfirmed = page.waitForResponse(
    (response) =>
      response.url().includes(`/api/albums/${albumId}/progress`) &&
      response.request().method() === "POST" &&
      response.status() === 200,
  );
  await page.getByRole("button", { name: /Photo 1 of 3/ }).click();
  await expect(page.getByText("1 / 3")).toBeVisible();
  await progressConfirmed;

  await page.getByRole("button", { name: "Back to Grid" }).click();
  await rm(join(root, "a.jpg"));
  await rm(join(root, "b.jpg"));
  await post(running.url, "/api/scan", {});
  // Every member is unavailable now, yet membership is retained; a fresh
  // snapshot resolves the saved position to its member index under the
  // fallback rules (the page moved the saved position to member 0 when it
  // opened Photo 1 earlier).
  await page.reload();
  await expect
    .poll(async () => {
      const retained = await state(running.url, albumId);
      return {
        available: retained.members.map((member) => member.available),
        position: retained.position,
      };
    })
    .toEqual({ available: [false, false, false], position: 0 });
  await page.getByRole("button", { name: /^Resume(?: |$)/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 3 of 3/ }),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
});

test("ready Preview facts render immediately during revalidation", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "photo.jpg"), await jpeg());
  const running = await server(base, root);
  await openGrid(page, running.url, "All Photos");
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.locator("[data-stage] img")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);

  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/photos/*/preview", (route) =>
    held.then(() => route.continue()),
  );
  const revalidated = page.waitForResponse(
    (response) =>
      response.url().includes("/api/photos/") &&
      response.url().endsWith("/preview") &&
      response.status() === 200,
  );
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await expect(page.locator("[data-stage] img")).toBeVisible();
  await expect(page.getByText("Loading Preview…")).toHaveCount(0);
  const ratingResponse = page.waitForResponse(
    (response) =>
      response.url().includes("/api/photos/") &&
      response.url().endsWith("/state") &&
      response.status() === 200,
  );
  await page.getByRole("button", { name: "Rate 5 stars" }).click();
  await ratingResponse;
  await expect(page.getByText("5 stars")).toBeVisible();
  release();
  await revalidated;
  await expect(page.getByText("5 stars")).toBeVisible();
  await page.unroute("**/api/photos/*/preview");
});

test("Grid thumbnails survive virtual re-renders without refetching", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (let index = 0; index < 8; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(2, "0")}.jpg`),
      await jpeg(),
    );
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  const thumbnailRequests: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.endsWith("/thumbnail"))
      thumbnailRequests.push(request.url());
  });
  await page.goto(running.url);
  const loadedThumbnails = () =>
    page.evaluate(
      () =>
        Array.from(
          document.querySelectorAll<HTMLImageElement>(".photo-cell img"),
        ).filter((image) => Boolean(image.getAttribute("src"))).length,
    );
  await expect.poll(loadedThumbnails).toBe(8);
  expect(thumbnailRequests).toHaveLength(8);
  expect(
    await page
      .locator(".photo-cell img")
      .evaluateAll((images) =>
        images.every(
          (image) =>
            image.getAttribute("fetchpriority") === "low" &&
            image.getAttribute("decoding") === "async",
        ),
      ),
  ).toBe(true);
  const viewport = page.locator("[data-grid-viewport]");
  for (let _ = 0; _ < 4; _ += 1) {
    await viewport.evaluate((element) =>
      element.dispatchEvent(new Event("scroll")),
    );
    await expect.poll(loadedThumbnails).toBe(8);
  }
  expect(thumbnailRequests).toHaveLength(8);
  await expect.poll(loadedThumbnails).toBe(8);
});

test("hydrated Grid thumbnails render without thumbnail API requests", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (let index = 0; index < 8; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(2, "0")}.jpg`),
      await jpeg(),
    );
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  for (const id of ids) {
    const response = await fetch(`${running.url}/api/photos/${id}/thumbnail`);
    expect(response.ok).toBe(true);
  }

  await page.setViewportSize({ width: 390, height: 844 });
  const thumbnailRequests: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.endsWith("/thumbnail"))
      thumbnailRequests.push(request.url());
  });
  await page.goto(running.url);
  const renderedThumbnails = () => page.locator(".photo-cell img").count();
  await expect.poll(renderedThumbnails).toBe(8);
  expect(thumbnailRequests).toHaveLength(0);
  const thumbnail = page.locator(".photo-cell img").first();
  await expect(thumbnail).toHaveAttribute(
    "src",
    /\/api\/derivatives\/[^/]+\/thumbnail\/[^/]+\.jpg$/,
  );
  await expect(thumbnail).toHaveAttribute("fetchpriority", "low");
  await expect(thumbnail).toHaveAttribute("decoding", "async");
  await thumbnail.click();
  const preview = page.locator("[data-stage] img");
  await expect(preview).toBeVisible();
  await expect(preview).toHaveAttribute("fetchpriority", "high");
  await expect(preview).toHaveAttribute("decoding", "async");
});

test("source switching reaches Ready while Grid derivatives remain held", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 70);
  const running = await server(base, root);
  await createAlbum(running.url, "Held Derivatives");

  let release!: () => void;
  const derivativesHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let derivativeRequests = 0;
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.includes("/api/derivatives/"))
      derivativeRequests += 1;
  });
  await page.route("**/api/derivatives/**", (route) =>
    derivativesHeld.then(() => route.continue()).catch(() => undefined),
  );
  try {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(running.url);
    await expect(page.getByText(/^Ready · 70 Photos$/)).toBeVisible();
    await expect.poll(() => derivativeRequests).toBeGreaterThan(0);

    const viewport = page.locator("[data-grid-viewport]");
    const requestsBeforeScroll = derivativeRequests;
    const scrolledDerivative = page.waitForRequest((request) =>
      new URL(request.url()).pathname.includes("/api/derivatives/"),
    );
    await viewport.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
      element.dispatchEvent(new Event("scroll"));
    });
    await scrolledDerivative;
    expect(derivativeRequests).toBeGreaterThan(requestsBeforeScroll);

    const pendingGridImage = await page
      .locator(".photo-cell img")
      .first()
      .elementHandle();
    expect(pendingGridImage).not.toBeNull();
    await openSources(page);
    await page.getByRole("button", { name: "Refresh Source" }).click();
    await expect(page.locator("[data-grid-title]")).toHaveText("All Photos");
    await expect(page.getByText(/^Ready · 70 Photos$/)).toBeVisible();
    expect(
      await pendingGridImage!.evaluate((image) => image.hasAttribute("src")),
    ).toBe(false);

    await openSources(page);
    await page
      .getByRole("button", { name: /^Held Derivatives 70 Photos/ })
      .click();
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Held Derivatives",
    );
    await expect(page.getByText(/^Ready · 70 Photos$/)).toBeVisible();
    expect(derivativeRequests).toBeGreaterThan(1);
  } finally {
    release();
    await page.unroute("**/api/derivatives/**");
  }
});

test("an admitted Album write settles after application teardown without presentation", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const created = (await (
    await post(running.url, "/api/albums", { name: "Detached" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const albumId = created.albums.find((album) => album.name === "Detached")!.id;
  await page.goto(running.url);
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await openMembershipPanel(page);

  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(`**/api/albums/${albumId}/members`, async (route) => {
    await held;
    await route.continue();
  });
  const settled = page.waitForResponse((response) =>
    response.url().includes(`/api/albums/${albumId}/members`),
  );
  await membershipCheckbox(page, "Detached").check();
  await page.evaluate(() =>
    window.dispatchEvent(new PageTransitionEvent("pagehide")),
  );
  release();
  await settled;
  await expect
    .poll(async () => {
      const overview = (await (
        await fetch(`${running.url}/api/overview`)
      ).json()) as { albums: Array<{ id: string; photoCount: number }> };
      return overview.albums.find((album) => album.id === albumId)?.photoCount;
    })
    .toBe(1);
  await expect(page.getByText("Added to the Album.")).toBeHidden();
});

test("application teardown halts image ownership and releases the Browse token", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
  let releaseStatus!: () => void;
  const statusReleased = new Promise<void>((resolve) => {
    releaseStatus = resolve;
  });
  await page.route("**/api/status", async (route) => {
    await statusReleased;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ state: "failed" }),
    });
  });
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  const image = page.getByRole("img", { name: "Photo 1 of 1" });
  await expect(image).toHaveAttribute("src", /.+/);

  const statusRequest = page.waitForRequest((request) =>
    request.url().endsWith("/api/status"),
  );
  await statusRequest;
  const statusResponse = page.waitForResponse((response) =>
    response.url().endsWith("/api/status"),
  );
  const released = page.waitForRequest(
    (request) =>
      request.method() === "DELETE" && request.url().includes("/api/browse/"),
  );
  let browseReleaseRequests = 0;
  page.on("request", (request) => {
    if (request.method() === "DELETE" && request.url().includes("/api/browse/"))
      browseReleaseRequests += 1;
  });
  await page.evaluate(() => {
    window.dispatchEvent(new PageTransitionEvent("pagehide"));
    window.dispatchEvent(new PageTransitionEvent("pagehide"));
  });
  releaseStatus();
  await statusResponse;
  await released;
  expect(browseReleaseRequests).toBe(1);
  await expect(image).not.toHaveAttribute("src", /.+/);
  await expect(
    page.getByRole("button", { name: "Retry Library Check" }),
  ).toBeHidden();
});

test("application teardown during File Location rebind stays silent", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "shoot/nested"), { recursive: true });
  await writeFile(join(root, "shoot/nested/photo.jpg"), await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  await expect(
    page.getByRole("button", { name: "Toggle shoot subfolders" }),
  ).toBeVisible();

  let releaseOverview!: () => void;
  const overviewHeld = new Promise<void>((resolve) => {
    releaseOverview = resolve;
  });
  let observeOverview!: () => void;
  const overviewRequested = new Promise<void>((resolve) => {
    observeOverview = resolve;
  });
  await page.route("**/api/overview", async (route) => {
    observeOverview();
    await overviewHeld;
    await route.continue();
  });
  await page.route("**/api/file-locations*", async (route) => {
    await route.fulfill({ status: 409 });
  });
  const pageErrors: Error[] = [];
  page.on("pageerror", (error) => pageErrors.push(error));

  await page.getByRole("button", { name: "Toggle shoot subfolders" }).click();
  await overviewRequested;
  await page.evaluate(() =>
    window.dispatchEvent(new PageTransitionEvent("pagehide")),
  );
  const overviewResponse = page.waitForResponse((response) =>
    response.url().endsWith("/api/overview"),
  );
  releaseOverview();
  await overviewResponse;
  await page.evaluate(
    () =>
      new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
  );

  expect(pageErrors).toEqual([]);
});

test("leaving Photo View cancels a pending review image transfer", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "photo.jpg"), await jpeg());
  const running = await server(base, root);
  const [photoId] = await browseIds(running.url);
  const previewResponse = await fetch(
    `${running.url}/api/photos/${photoId}/preview`,
  );
  expect(previewResponse.ok).toBe(true);
  const preview = (await previewResponse.json()) as { url?: string };
  expect(preview.url).toContain("/review/");

  let release!: () => void;
  const reviewHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/derivatives/**/review/**", (route) =>
    reviewHeld.then(() => route.continue()).catch(() => undefined),
  );
  try {
    await page.goto(running.url);
    const reviewRequest = page.waitForRequest((request) =>
      new URL(request.url()).pathname.includes("/review/"),
    );
    await page.locator('[data-photo-index="0"]').click();
    await reviewRequest;
    const pendingReviewImage = await page
      .locator("[data-stage] img")
      .elementHandle();
    expect(pendingReviewImage).not.toBeNull();

    await page.getByRole("button", { name: "Back to Grid" }).click();
    await expect(page.getByText(/^Ready · 1 Photo$/)).toBeVisible();
    expect(
      await pendingReviewImage!.evaluate((image) => image.hasAttribute("src")),
    ).toBe(false);
  } finally {
    release();
    await page.unroute("**/api/derivatives/**/review/**");
  }
});

test("answered Browse-window failure owns source Retry and does not declare Ready", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  let windowMode: "wrong-range" | "bad-photo" | "failed" | "ready" =
    "wrong-range";
  const windowTokens: string[] = [];
  const windowStarts: string[] = [];
  let browseAllocations = 0;
  let overviewRequests = 0;
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (request.method() === "POST" && url.pathname === "/api/browse")
      browseAllocations += 1;
    if (request.method() === "GET" && url.pathname === "/api/overview")
      overviewRequests += 1;
  });
  await page.route(/\/api\/browse\//, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (request.method() === "GET") {
      windowTokens.push(url.pathname.split("/").at(-1)!);
      windowStarts.push(url.searchParams.get("start")!);
    }
    if (request.method() !== "GET" || windowMode === "ready") {
      await route.continue();
      return;
    }
    if (windowMode === "wrong-range" || windowMode === "bad-photo") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body:
          windowMode === "wrong-range"
            ? '{"start":999,"total":1,"photos":[]}'
            : '{"start":0,"total":1,"photos":[null]}',
      });
      return;
    }
    await route.fulfill({ status: 500, body: '{"error":"failed"}' });
  });

  await page.goto(running.url);
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByText(/returned an invalid response/)).toBeVisible();
  await expect(page.getByText(/Ready · 1 Photo/)).toBeHidden();
  await expect.poll(() => overviewRequests).toBeGreaterThan(0);
  const initialOverviewRequests = overviewRequests;

  windowMode = "bad-photo";
  await page.getByRole("button", { name: "Retry connection" }).click();
  await expect(page.getByText(/returned an invalid response/)).toBeVisible();
  windowMode = "failed";
  await page.getByRole("button", { name: "Retry connection" }).click();
  await expect(
    page.getByText(/could not be loaded \(HTTP 500\)/),
  ).toBeVisible();
  windowMode = "ready";
  await page.getByRole("button", { name: "Retry connection" }).click();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  expect(browseAllocations).toBe(1);
  expect(overviewRequests).toBe(initialOverviewRequests);
  expect(new Set(windowTokens).size).toBe(1);
  expect(new Set(windowStarts)).toEqual(new Set(["0"]));
});

test("current Preview HTTP failure disconnects until Photo Retry", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
  let previewMode:
    | "typed-200"
    | "typed-503"
    | "unknown-state"
    | "ready-without-url" = "typed-200";
  await page.route("**/api/photos/*/preview", async (route) => {
    await route.fulfill({
      status: previewMode === "typed-503" ? 503 : 200,
      contentType: "application/json",
      body:
        previewMode === "ready-without-url"
          ? '{"state":"ready"}'
          : previewMode === "unknown-state"
            ? '{"state":"pending"}'
            : '{"state":"unavailable","message":"service unavailable"}',
    });
  });
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Connection lost. Retry to refresh this Photo."),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();

  previewMode = "typed-503";
  const typed503 = page.waitForResponse(
    (response) =>
      response.url().includes("/preview") && response.status() === 503,
  );
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await typed503;
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();

  previewMode = "unknown-state";
  const unknownState = page.waitForResponse(
    (response) =>
      response.url().includes("/preview") && response.status() === 200,
  );
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await unknownState;
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();

  previewMode = "ready-without-url";
  const malformedReady = page.waitForResponse(
    (response) =>
      response.url().includes("/preview") && response.status() === 200,
  );
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await malformedReady;
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();

  await page.unroute("**/api/photos/*/preview");
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
});

test("Photo Retry reports a replacement current fact instead of remaining in progress", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
  await page.route("**/api/photos/*/preview", (route) =>
    route.fulfill({ status: 503, body: '{"error":"failed"}' }),
  );
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();

  let replacementServed = false;
  await page.route(
    (url) =>
      url.pathname.startsWith("/api/browse/") &&
      url.searchParams.get("start") === "0",
    async (route) => {
      const response = await route.fetch();
      const body = (await response.json()) as {
        photos: Array<Record<string, unknown>>;
      };
      replacementServed = true;
      await route.fulfill({
        response,
        json: {
          ...body,
          photos: [{ ...body.photos[0], id: "replacement-photo" }],
        },
      });
    },
  );
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect.poll(() => replacementServed).toBe(true);
  await expect(page.locator("[data-status]")).toHaveText(
    "Could not refresh this Photo. Retry to continue.",
  );
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Retry", exact: true }),
  ).toBeEnabled();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();
});

test("source switching aborts a pending current-Photo Preview request", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 8);
  const running = await server(base, root);
  await createAlbum(running.url, "Preview Abort Target");

  let release!: () => void;
  const previewHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let heldUrl: string | undefined;
  await page.route("**/api/photos/*/preview", (route) => {
    heldUrl = route.request().url();
    return previewHeld.then(() => route.continue()).catch(() => undefined);
  });
  try {
    await page.goto(running.url);
    await page.locator('[data-photo-index="0"]').click();
    await expect.poll(() => heldUrl).toBeTruthy();
    const canceledUrl = heldUrl!;
    const previewCanceled = page.waitForEvent(
      "requestfailed",
      (request) => request.url() === canceledUrl,
    );

    await openSources(page);
    await page
      .getByRole("button", { name: /^Preview Abort Target 8 Photos/ })
      .click();
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Preview Abort Target",
    );
    await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();
    await previewCanceled;
  } finally {
    release();
    await page.unroute("**/api/photos/*/preview");
  }
});

test("opening Photo View aborts Grid fallback thumbnail requests", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 8);
  const running = await server(base, root);

  let release!: () => void;
  const thumbnailHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let heldUrl: string | undefined;
  await page.route("**/api/photos/*/thumbnail", (route) => {
    heldUrl = route.request().url();
    return thumbnailHeld.then(() => route.continue()).catch(() => undefined);
  });
  try {
    await page.goto(running.url);
    await expect.poll(() => heldUrl).toBeTruthy();
    const canceledUrl = heldUrl!;
    const thumbnailCanceled = page.waitForEvent(
      "requestfailed",
      (request) => request.url() === canceledUrl,
    );

    await page.locator('[data-photo-index="0"]').click();
    await expect(page.getByText("1 / 8")).toBeVisible();
    await thumbnailCanceled;
  } finally {
    release();
    await page.unroute("**/api/photos/*/thumbnail");
  }
});

test("a superseded source open is aborted before the newer source renders", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 8);
  const running = await server(base, root);
  const { albumId: firstAlbumId } = await createAlbum(
    running.url,
    "First Source",
  );
  await createAlbum(running.url, "Second Source");

  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();

  let release!: () => void;
  const staleGate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let staleHeld = false;
  await page.route("**/api/browse", async (route) => {
    const request = route.request();
    if (
      request.method() === "POST" &&
      (request.postDataJSON() as { albumId?: string }).albumId ===
        firstAlbumId &&
      !staleHeld
    ) {
      staleHeld = true;
      await staleGate;
    }
    try {
      await route.continue();
    } catch {
      /* a newer source may abort the intercepted request */
    }
  });
  try {
    await openSources(page);
    await page.getByRole("button", { name: /^First Source 8 Photos/ }).click();
    await expect.poll(() => staleHeld).toBe(true);
    const staleCanceled = page.waitForEvent("requestfailed", (request) => {
      if (
        request.method() !== "POST" ||
        new URL(request.url()).pathname !== "/api/browse"
      )
        return false;
      return (
        (request.postDataJSON() as { albumId?: string }).albumId ===
        firstAlbumId
      );
    });
    await openSources(page);
    await page.getByRole("button", { name: /^Second Source 8 Photos/ }).click();
    await expect(page.locator("[data-grid-title]")).toHaveText("Second Source");
    await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();

    await staleCanceled;
    await expect(page.locator("[data-grid-title]")).toHaveText("Second Source");
    await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();
  } finally {
    release();
    await page.unroute("**/api/browse");
  }
});

test("source changes invalidate queued Grid renders", async ({ page }) => {
  const { base, root } = await fixture();
  await writePhotos(root, 8);
  const running = await server(base, root);
  await createAlbum(running.url, "Boundary Source");

  const openedResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/browse",
  );
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  const opened = (await (await openedResponse).json()) as { token: string };
  const oldToken = opened.token;
  await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();
  await page.locator('[data-photo-index="0"]').click();
  await expect(page.locator("[data-review]")).toBeVisible();

  let staleWindowRequests = 0;
  await page.route("**/api/browse/**", async (route) => {
    const request = route.request();
    if (
      request.method() === "GET" &&
      request.url().includes(`/api/browse/${oldToken}?`)
    )
      staleWindowRequests += 1;
    await route.continue();
  });
  try {
    await page.evaluate(() => {
      document.querySelector<HTMLButtonElement>("[data-back]")!.click();
      document
        .querySelector<HTMLElement>("[data-grid-viewport]")!
        .dispatchEvent(new Event("scroll"));
      const source = Array.from(
        document.querySelectorAll<HTMLButtonElement>(".source-card"),
      ).find((button) => button.textContent?.startsWith("Boundary Source"));
      if (!source) throw new Error("Boundary source is missing");
      source.click();
      document
        .querySelector<HTMLElement>("[data-grid-viewport]")!
        .dispatchEvent(new Event("scroll"));
    });
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Boundary Source",
    );
    await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();
    await page.evaluate(
      () =>
        new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
    );
    expect(staleWindowRequests).toBe(0);
  } finally {
    await page.unroute("**/api/browse/**");
  }
});

test("source switching cancels the previous pending window", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 120);
  const running = await server(base, root);
  await createAlbum(running.url, "Abort Target");

  const openedResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/browse",
  );
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  const openedBody = (await (await openedResponse).json()) as {
    token: string;
  };
  const oldBrowseToken = openedBody.token;
  await expect(page.getByText(/^Ready · 120 Photos$/)).toBeVisible();

  let release!: () => void;
  const windowHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let oldWindowUrl: string | undefined;
  await page.route("**/api/browse/**", (route) => {
    const request = route.request();
    if (
      request.method() === "GET" &&
      request.url().includes(`/api/browse/${oldBrowseToken}?`)
    ) {
      oldWindowUrl = request.url();
      return windowHeld.then(() => route.continue()).catch(() => undefined);
    }
    return route.continue();
  });
  try {
    const viewport = page.locator("[data-grid-viewport]");
    await viewport.evaluate((element) => {
      element.scrollTop = 30 * 178;
      element.dispatchEvent(new Event("scroll"));
    });
    await expect.poll(() => oldWindowUrl).toBeTruthy();
    const oldWindowCanceled = page.waitForEvent(
      "requestfailed",
      (request) => request.url() === oldWindowUrl,
    );

    await openSources(page);
    await page
      .getByRole("button", { name: /^Abort Target 120 Photos/ })
      .click();
    await expect(page.locator("[data-grid-title]")).toHaveText("Abort Target");
    await expect(page.getByText(/^Ready · 120 Photos$/)).toBeVisible();
    await oldWindowCanceled;
  } finally {
    release();
    await page.unroute("**/api/browse/**");
  }
});

test("source switching aborts fallback thumbnail requests", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 70);
  const running = await server(base, root);
  await createAlbum(running.url, "Fallback Abort");

  let release!: () => void;
  const thumbnailGate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let thumbnailRequests = 0;
  await page.route("**/api/photos/*/thumbnail", (route) => {
    thumbnailRequests += 1;
    return thumbnailGate.then(() => route.continue()).catch(() => undefined);
  });
  try {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(running.url);
    await expect(page.getByText(/^Ready · 70 Photos$/)).toBeVisible();
    await expect.poll(() => thumbnailRequests).toBeGreaterThan(0);
    const thumbnailCanceled = page.waitForEvent("requestfailed", (request) =>
      new URL(request.url()).pathname.endsWith("/thumbnail"),
    );

    await openSources(page);
    await page
      .getByRole("button", { name: /^Fallback Abort 70 Photos/ })
      .click();
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Fallback Abort",
    );
    await expect(page.getByText(/^Ready · 70 Photos$/)).toBeVisible();
    await thumbnailCanceled;
  } finally {
    release();
    await page.unroute("**/api/photos/*/thumbnail");
  }
});

test("scroll events coalesce Grid rendering to one animation frame", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 120);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await openGrid(page, running.url, "All Photos");

  const frameBatches = await page.evaluate(
    () =>
      new Promise<number[]>((resolve) => {
        const layer = document.querySelector<HTMLElement>("[data-grid-layer]");
        const viewport = document.querySelector<HTMLElement>(
          "[data-grid-viewport]",
        );
        if (!layer || !viewport) throw new Error("Grid elements are missing");
        const batches: number[] = [];
        let current = 0;
        const observer = new MutationObserver((records) => {
          if (records.some((record) => record.type === "childList"))
            current += 1;
        });
        observer.observe(layer, { childList: true });
        viewport.scrollTop = 30 * 178;
        for (let index = 0; index < 8; index += 1)
          viewport.dispatchEvent(new Event("scroll"));
        const collect = (remaining: number) =>
          requestAnimationFrame(() => {
            batches.push(current);
            current = 0;
            if (remaining > 1) collect(remaining - 1);
            else {
              observer.disconnect();
              resolve(batches);
            }
          });
        collect(3);
      }),
  );
  // Eight scroll events in one frame merge into exactly one Grid DOM update,
  // and no frame mutates the Grid more than once.
  expect(frameBatches[0]).toBe(1);
  expect(frameBatches.every((count) => count <= 1)).toBe(true);
});

/// Opens the default library source without changing the viewport, so a test
/// can drive the Grid at a viewport size of its own.
const openInitialGrid = async (page: Page, url: string, index = 0) => {
  await page.goto(url);
  await expect(
    page.locator(`[data-photo-index="${index}"] img`),
  ).toHaveAttribute("src", /\/thumbnail\//);
  await waitForGridFrame(page);
};

const sortedUnique = (values: ReadonlyArray<number>) =>
  [...new Set(values)].sort((left, right) => left - right);

/// Scrolls the Grid and waits for the merged render that reports the range.
const scrollGrid = async (page: Page, scrollTop: number | "end") => {
  await page.locator("[data-grid-viewport]").evaluate((viewport, target) => {
    viewport.scrollTop = target === "end" ? viewport.scrollHeight : target;
    viewport.dispatchEvent(new Event("scroll"));
  }, scrollTop);
  await waitForGridFrame(page);
};

test("a scroll burst admits one window request per covering aligned window", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 300);
  const running = await server(base, root);
  await page.setViewportSize({ width: 2560, height: 1440 });
  const windows = recordWindowRequests(page);
  await openInitialGrid(page, running.url);
  const visible = await renderedGridSpan(page);
  expect(windows.requested).toEqual(
    coveringWindowStarts(visible.start, visible.end, 300),
  );
  expect(windows.requested.length).toBeGreaterThan(1);

  await page.locator("[data-grid-viewport]").evaluate((viewport) => {
    viewport.scrollTop = viewport.scrollHeight;
    for (let index = 0; index < 10; index += 1)
      viewport.dispatchEvent(new Event("scroll"));
  });
  await waitForGridFrame(page);
  await expectGridConverged(page, windows);
  const tail = await renderedGridSpan(page);
  expect(tail.end).toBe(300);
  // One request per aligned window however many scroll events reported it: no
  // window is requested twice and none outside the covered range.
  expect(windows.requested).toEqual(
    sortedUnique([
      ...coveringWindowStarts(visible.start, visible.end, 300),
      ...coveringWindowStarts(tail.start, tail.end, 300),
    ]),
  );
  await expect(page.locator('[data-photo-index="299"] img')).toHaveAttribute(
    "src",
    /\/thumbnail\//,
  );
});

test("a scrolled Grid keeps the DOM of the Photos that stay rendered", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 120);
  const running = await server(base, root);
  let releaseWindow: () => void = () => undefined;
  const windowGate = new Promise<void>((resolve) => {
    releaseWindow = resolve;
  });
  await page.route(
    (url) =>
      url.pathname.startsWith("/api/browse/") &&
      url.searchParams.get("start") === "60",
    (route) => windowGate.then(() => route.continue()).catch(() => undefined),
  );
  try {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(running.url);
    await expect(page.locator('[data-photo-index="0"]')).toBeVisible();
    const viewport = page.locator("[data-grid-viewport]");
    // Rows 29 and 30 render 54–69 and 56–71: the loaded window ends at 60, so
    // the Photos entering beyond it stay placeholders.
    await viewport.evaluate((element) => {
      element.scrollTop = 29 * 178;
      element.dispatchEvent(new Event("scroll"));
    });
    await expect(page.locator('[data-photo-index="59"]')).toBeVisible();
    // The unloaded window beyond index 60 renders stable placeholders instead
    // of Photo cells.
    await expect(page.locator('[data-photo-index="60"]')).toHaveCount(0);
    await expect(page.locator(".cell-placeholder").first()).toBeVisible();
    const retainedCell = await page
      .locator('[data-photo-index="58"]')
      .elementHandle();
    const retainedImage = await page
      .locator('[data-photo-index="58"] img')
      .elementHandle();
    const retainedPlaceholder = await page
      .locator(".photo-cell:has(.cell-placeholder):not([data-photo-index])")
      .first()
      .elementHandle();
    expect(retainedCell).not.toBeNull();
    expect(retainedImage).not.toBeNull();
    expect(retainedPlaceholder).not.toBeNull();

    await scrollGrid(page, 30 * 178);
    // The Photos that stayed keep their button and their image; nothing is
    // rebound or refetched.
    expect(await retainedCell!.evaluate((node) => node.isConnected)).toBe(true);
    expect(await retainedImage!.evaluate((node) => node.isConnected)).toBe(
      true,
    );
    expect(
      await retainedImage!.evaluate(
        (node) => node === node.closest(".photo-cell")?.querySelector("img"),
      ),
    ).toBe(true);
    expect(
      await retainedPlaceholder!.evaluate((node) => node.isConnected),
    ).toBe(true);
  } finally {
    releaseWindow();
    await page.unroute(/\/api\/browse\//);
  }
});

test("a fast multi-window scroll mutates the Grid at most once per frame", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 300);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await openGrid(page, running.url, "All Photos");

  const frameBatches = await page.evaluate(
    () =>
      new Promise<number[]>((resolve) => {
        const layer = document.querySelector<HTMLElement>("[data-grid-layer]");
        const viewport = document.querySelector<HTMLElement>(
          "[data-grid-viewport]",
        );
        if (!layer || !viewport) throw new Error("Grid elements are missing");
        const batches: number[] = [];
        let current = 0;
        const observer = new MutationObserver((records) => {
          if (records.some((record) => record.type === "childList"))
            current += 1;
        });
        observer.observe(layer, { childList: true });
        let step = 0;
        const advance = () =>
          requestAnimationFrame(() => {
            batches.push(current);
            current = 0;
            step += 1;
            if (step < 8) {
              viewport.scrollTop = step * 6 * 178;
              viewport.dispatchEvent(new Event("scroll"));
              viewport.dispatchEvent(new Event("scroll"));
              advance();
            } else {
              observer.disconnect();
              resolve(batches);
            }
          });
        viewport.scrollTop = 6 * 178;
        viewport.dispatchEvent(new Event("scroll"));
        viewport.dispatchEvent(new Event("scroll"));
        advance();
      }),
  );
  // Data-driven updates merge into the scroll render: no frame touches the
  // Grid more than once while windows arrive under a fast scroll.
  expect(frameBatches.every((count) => count <= 1)).toBe(true);
  expect(frameBatches.some((count) => count === 1)).toBe(true);
});

test("a scroll reversal converges without churn or repeat requests", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 120);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  const windows = recordWindowRequests(page);
  await openGrid(page, running.url, "All Photos");
  await scrollGrid(page, 30 * 178);
  await expectGridConverged(page, windows);
  expect(sortedUnique(windows.requested)).toContain(60);
  const loadedRequests = [...windows.requested];
  const retainedCell = await page
    .locator('[data-photo-index="70"]')
    .elementHandle();
  expect(retainedCell).not.toBeNull();

  const mutations = await page.locator("[data-grid-viewport]").evaluate(
    (element) =>
      new Promise<number>((resolve) => {
        const gridLayer =
          document.querySelector<HTMLElement>("[data-grid-layer]");
        if (!gridLayer) throw new Error("Grid layer is missing");
        const start = element.scrollTop;
        let count = 0;
        const observer = new MutationObserver((records) => {
          count += records.filter(
            (record) => record.type === "childList",
          ).length;
        });
        observer.observe(gridLayer, { childList: true });
        element.scrollTop = start + 2 * 178;
        element.dispatchEvent(new Event("scroll"));
        element.scrollTop = start;
        element.dispatchEvent(new Event("scroll"));
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            observer.disconnect();
            resolve(count);
          }),
        );
      }),
  );
  // Down and back within one frame reports the same range: the settled Grid
  // neither churns its cells nor asks for a window it already has.
  expect(mutations).toBe(0);
  expect(windows.requested).toEqual(loadedRequests);
  expect(await retainedCell!.evaluate((node) => node.isConnected)).toBe(true);
  await expectGridConverged(page, windows);
});

test("out-of-order window settlements render every loaded Photo position", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 300);
  const running = await server(base, root);
  let releaseFirst: () => void = () => undefined;
  const firstGate = new Promise<void>((resolve) => {
    releaseFirst = resolve;
  });
  await page.route(
    (url) =>
      url.pathname.startsWith("/api/browse/") &&
      url.searchParams.get("start") === "60",
    async (route) => {
      await firstGate;
      try {
        await route.continue();
      } catch {
        /* the range may be superseded before the held response lands */
      }
    },
  );
  try {
    await page.setViewportSize({ width: 2560, height: 1440 });
    await openInitialGrid(page, running.url);
    const viewport = page.locator("[data-grid-viewport]");
    await viewport.evaluate((element) => {
      element.scrollTop = 6 * 178;
      element.dispatchEvent(new Event("scroll"));
    });
    // The later window commits while the range's first window is still held.
    await expect(page.locator('[data-photo-index="180"] img')).toHaveAttribute(
      "src",
      /\/thumbnail\//,
    );
    const laterCell = await page
      .locator('[data-photo-index="200"]')
      .elementHandle();
    expect(laterCell).not.toBeNull();

    releaseFirst();
    await expect(page.locator('[data-photo-index="60"] img')).toHaveAttribute(
      "src",
      /\/thumbnail\//,
    );
    await expect(page.locator(".cell-placeholder")).toHaveCount(0);
    // The late window fills its own positions without evicting the range that
    // already committed.
    expect(await laterCell!.evaluate((node) => node.isConnected)).toBe(true);
    await expect(page.locator('[data-photo-index="200"] img')).toHaveAttribute(
      "src",
      /\/thumbnail\//,
    );
  } finally {
    releaseFirst();
    await page.unroute(/\/api\/browse\//);
  }
});

test("a large viewport loads only covering windows and stays bounded", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 300);
  const running = await server(base, root);
  await page.setViewportSize({ width: 2560, height: 1440 });
  const windows = recordWindowRequests(page);
  await openInitialGrid(page, running.url);
  const visible = await renderedGridSpan(page);
  expect(windows.requested).toEqual(
    coveringWindowStarts(visible.start, visible.end, 300),
  );
  expect(visible.cells).toBeLessThan(300);

  await scrollGrid(page, "end");
  await expectGridConverged(page, windows);
  const tail = await renderedGridSpan(page);
  expect(tail.end).toBe(300);
  expect(tail.cells).toBeLessThan(300);
  expect(sortedUnique(windows.requested)).toEqual(
    sortedUnique([
      ...coveringWindowStarts(visible.start, visible.end, 300),
      ...coveringWindowStarts(tail.start, tail.end, 300),
    ]),
  );
  expect(await page.locator(".photo-cell").count()).toBeLessThan(300);
});

test("a source switch rebuilds the Grid range for the replacement source", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 80);
  const running = await server(base, root);
  const windows = recordWindowRequests(page);
  await openGrid(page, running.url, "All Photos");
  const previousImage = await page
    .locator('[data-photo-index="0"] img')
    .elementHandle();
  expect(await previousImage!.getAttribute("src")).toMatch(/\/thumbnail\//);
  const sourceRequests = windows.requested.length;

  // The replacement source presents the same Photos, so it must rebuild the
  // cells and re-attach their thumbnails instead of reusing the previous
  // source's DOM, and admit its own covering windows.
  await openSources(page);
  await page.getByRole("button", { name: /^Library Folder/ }).click();
  await expect(page.locator("[data-grid-title]")).toHaveText(
    "Library Folder · Folder",
  );
  await expect(page.locator('[data-photo-index="0"] img')).toHaveAttribute(
    "src",
    /\/thumbnail\//,
  );
  expect(await previousImage!.evaluate((node) => node.isConnected)).toBe(false);
  await expectGridConverged(page, windows);
  const visible = await renderedGridSpan(page);
  expect(windows.requested.slice(sourceRequests)).toEqual(
    coveringWindowStarts(visible.start, visible.end, 80),
  );
});

test("Back to Grid restoration supersedes a queued scroll render", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 20);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 20 Photos$/)).toBeVisible();

  const currentCell = page.locator('[data-photo-index="16"]');
  await currentCell.scrollIntoViewIfNeeded();
  await currentCell.click();
  await expect(page.getByText("17 / 20")).toBeVisible();

  const restoredScrollTop = await page.evaluate(
    () =>
      new Promise<number>((resolve) => {
        const viewport = document.querySelector<HTMLElement>(
          "[data-grid-viewport]",
        );
        const back = document.querySelector<HTMLButtonElement>(
          "[data-review] [data-back]",
        );
        if (!viewport || !back) throw new Error("Grid controls are missing");
        viewport.scrollTop = 0;
        viewport.dispatchEvent(new Event("scroll"));
        back.click();
        requestAnimationFrame(() =>
          requestAnimationFrame(() => resolve(viewport.scrollTop)),
        );
      }),
  );

  expect(restoredScrollTop).toBeGreaterThan(0);
  await expect(currentCell).toBeVisible();
});

test("hydrated Grid thumbnail delivery failures stay attached to the Photo", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "photo.jpg"), await jpeg());
  const running = await server(base, root);
  const [photoId] = await browseIds(running.url);
  const response = await fetch(
    `${running.url}/api/photos/${photoId}/thumbnail`,
  );
  expect(response.ok).toBe(true);

  let thumbnailApiRequests = 0;
  let derivativeRequests = 0;
  page.on("request", (request) => {
    const pathname = new URL(request.url()).pathname;
    if (/^\/api\/photos\/[^/]+\/thumbnail$/.test(pathname))
      thumbnailApiRequests += 1;
    if (
      pathname.includes("/api/derivatives/") &&
      pathname.includes("/thumbnail/")
    )
      derivativeRequests += 1;
  });
  await page.route("**/*", async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    if (
      route.request().method() === "GET" &&
      pathname.startsWith("/api/browse/")
    ) {
      const browseResponse = await route.fetch();
      const body = (await browseResponse.json()) as {
        photos: BrowsePhoto[];
      };
      await route.fulfill({
        response: browseResponse,
        json: {
          ...body,
          photos: body.photos.map((photo) => ({
            ...photo,
            ambiguous: true,
          })),
        },
      });
      return;
    }
    if (
      pathname.includes("/api/derivatives/") &&
      pathname.includes("/thumbnail/")
    ) {
      await route.fulfill({ status: 404, body: "missing derivative" });
      return;
    }
    await route.continue();
  });
  await page.goto(running.url);
  const image = page.locator(".photo-cell img").first();
  await image.scrollIntoViewIfNeeded();
  const cell = page.locator('[data-photo-index="0"]');
  const facts = cell.locator(".cell-facts");
  const factsText = "Ambiguous pairing · Thumbnail delivery failed";
  const accessibleName =
    "Photo 1 of 1 — Undecided — 0 stars — Ambiguous pairing — Thumbnail delivery failed";
  await expect(image).toHaveAttribute("alt", "Photo 1 of 1");
  await expect(facts).toBeVisible();
  await expect(facts).toHaveText(factsText);
  await expect(cell).toHaveAccessibleName(
    /Photo 1 of 1.*Ambiguous pairing.*Thumbnail delivery failed/,
  );
  expect(thumbnailApiRequests).toBe(0);
  expect(derivativeRequests).toBe(1);

  // A merged Grid update reuses the visible Photo's cell and the image that
  // already failed, so the failure stays attached to that Photo instead of a
  // replacement node.
  const failedCell = await cell.elementHandle();
  const failedImage = await image.elementHandle();
  expect(failedCell).not.toBeNull();
  expect(failedImage).not.toBeNull();
  await page.locator("[data-grid-viewport]").evaluate((viewport) => {
    viewport.dispatchEvent(new Event("scroll"));
  });
  await waitForGridFrame(page);
  expect(await failedCell!.evaluate((node) => node.isConnected)).toBe(true);
  expect(await failedImage!.evaluate((node) => node.isConnected)).toBe(true);
  await expect(facts).toHaveText(factsText);
  await expect(cell).toHaveAccessibleName(accessibleName);

  for (const viewport of [
    { width: 390, height: 844 },
    { width: 844, height: 390 },
  ]) {
    await page.setViewportSize(viewport);
    await waitForGridFrame(page);
    await cell.scrollIntoViewIfNeeded();
    await expect(cell).toBeVisible();
    await expect(cell).toBeEnabled();
    await expect(facts).toBeVisible();
    await expect(facts).toHaveText(factsText);
    await expect(cell).toHaveAccessibleName(accessibleName);

    const geometry = await facts.evaluate((label) => {
      const cell = label.closest<HTMLElement>(".photo-cell");
      const viewport = label.closest<HTMLElement>("[data-grid-viewport]");
      if (!cell || !viewport) throw new Error("Grid geometry is missing");
      const factsBox = label.getBoundingClientRect();
      const cellBox = cell.getBoundingClientRect();
      const viewportBox = viewport.getBoundingClientRect();
      const inside = (inner: DOMRect, outer: DOMRect) =>
        inner.left >= outer.left - 1 &&
        inner.right <= outer.right + 1 &&
        inner.top >= outer.top - 1 &&
        inner.bottom <= outer.bottom + 1;
      return {
        factsInsideCell: inside(factsBox, cellBox),
        cellInsideViewport: inside(cellBox, viewportBox),
        factsNotInternallyClipped:
          label.scrollWidth <= label.clientWidth &&
          label.scrollHeight <= label.clientHeight,
      };
    });
    expect(geometry).toEqual({
      factsInsideCell: true,
      cellInsideViewport: true,
      factsNotInternallyClipped: true,
    });

    await cell.click();
    await expect(page.getByText("1 / 1")).toBeVisible();
    await page.getByRole("button", { name: "Back to Grid" }).click();
    await waitForGridFrame(page);
  }
  expect(thumbnailApiRequests).toBe(0);
  expect(derivativeRequests).toBe(1);
});

test("Grid presents independent Photo, pairing, and Preview facts without removing actions", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  const hydrated = await fetch(`${running.url}/api/photos/${ids[3]}/thumbnail`);
  expect(hydrated.ok).toBe(true);
  const thumbnailRequests: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.endsWith("/thumbnail"))
      thumbnailRequests.push(request.url());
  });
  await page.route("**/api/browse/**", async (route) => {
    if (route.request().method() !== "GET") {
      await route.continue();
      return;
    }
    const response = await route.fetch();
    const body = (await response.json()) as {
      start: number;
      total: number;
      photos: Array<
        BrowsePhoto & {
          ambiguous: boolean;
          originals: Array<Readonly<{ kind: string; available: boolean }>>;
          preview: Readonly<{ state: string }>;
        }
      >;
    };
    const photos = body.photos.map((photo) => {
      const position = ids.indexOf(photo.id);
      if (position === 0)
        return {
          ...photo,
          available: false,
          originals: photo.originals.map((original) => ({
            ...original,
            available: false,
          })),
          preview: { state: "unavailable" },
        };
      if (position === 1)
        return {
          ...photo,
          ambiguous: true,
          preview: { state: "unavailable" },
        };
      if (position === 2)
        return { ...photo, preview: { state: "unavailable" } };
      return {
        ...photo,
        preview: { ...photo.preview, state: "failed" },
      };
    });
    await route.fulfill({ response, json: { ...body, photos } });
  });

  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 4 Photos$/)).toBeVisible();
  const first = page.locator('[data-photo-index="0"]');
  const second = page.locator('[data-photo-index="1"]');
  const third = page.locator('[data-photo-index="2"]');
  const fourth = page.locator('[data-photo-index="3"]');
  const factLabels = page.locator(".cell-facts");
  await expect(factLabels).toHaveCount(4);
  for (const label of await factLabels.all()) await expect(label).toBeVisible();
  await expect(first.locator(".cell-facts")).toHaveText(
    "Photo unavailable · Preview unavailable",
  );
  await expect(second.locator(".cell-facts")).toHaveText(
    "Ambiguous pairing · Preview unavailable",
  );
  await expect(third.locator(".cell-facts")).toHaveText("Preview unavailable");
  await expect(fourth.locator(".cell-facts")).toHaveText("Preview failed");
  await expect(first).toHaveAccessibleName(
    /Photo 1 of 4.*Photo unavailable.*Preview unavailable/,
  );
  await expect(second).toHaveAccessibleName(
    /Photo 2 of 4.*Ambiguous pairing.*Preview unavailable/,
  );
  await expect(third).toHaveAccessibleName(/Photo 3 of 4.*Preview unavailable/);
  await expect(fourth).toHaveAccessibleName(/Photo 4 of 4.*Preview failed/);
  await expect(fourth.locator("img")).toHaveAttribute("src", /\/thumbnail\//);
  for (const cell of [first, second, third, fourth])
    await expect(cell).toBeEnabled();
  expect(thumbnailRequests).toHaveLength(0);
  await first.click();
  await expect(page.getByText("1 / 4")).toBeVisible();
});

test("detached Grid image errors cannot poison the replacement cell", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "photo.jpg"), await jpeg());
  const running = await server(base, root);
  const [photoId] = await browseIds(running.url);
  const response = await fetch(
    `${running.url}/api/photos/${photoId}/thumbnail`,
  );
  expect(response.ok).toBe(true);

  await page.goto(running.url);
  const currentCell = page.locator('[data-photo-index="0"]');
  const currentImage = currentCell.locator("img");
  await expect(currentImage).toHaveAttribute("src", /\/thumbnail\//);
  const detachedImage = await currentImage.elementHandle();
  expect(detachedImage).not.toBeNull();

  // Photo View hands the Grid back by rebuilding its cells, so the captured
  // image is detached from the cell that now presents the Photo.
  await currentCell.click();
  await expect(page.getByText("1 / 1")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);
  expect(await detachedImage!.evaluate((image) => image.isConnected)).toBe(
    false,
  );
  await detachedImage!.evaluate((image) =>
    image.dispatchEvent(new Event("error")),
  );

  await expect(currentImage).toHaveAttribute("alt", "Photo 1 of 1");
  await expect(currentImage).toHaveAttribute("src", /\/thumbnail\//);
  await expect(currentCell.locator(".cell-facts")).toBeHidden();
});

/**
 * Geometry for one rendered Grid cell. The image box is the rendered Photo,
 * not the media area, because .thumbnail sizes itself from the derivative's
 * natural pixels inside the cell media area. `facts` carries the visible fact
 * text so a caller can prove which cell renders a Photo-state indicator.
 */
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

async function cellBoxes(
  page: Page,
): Promise<Array<Readonly<{ index: number; width: number; height: number }>>> {
  return page.evaluate(() =>
    Array.from(
      document.querySelectorAll<HTMLElement>(".grid-layer > .photo-cell"),
      (cell) => {
        const box = cell.getBoundingClientRect();
        return {
          index: Number(cell.dataset.photoIndex ?? -1),
          width: box.width,
          height: box.height,
        };
      },
    ),
  );
}

/** Ratio tolerance for rendered pixels versus the derivative's natural size. */
function expectAspectRatio(geometry: GridCellGeometry): void {
  const rendered = geometry.imageWidth / geometry.imageHeight;
  const natural = geometry.naturalWidth / geometry.naturalHeight;
  expect(Math.abs(rendered - natural) / natural).toBeLessThan(0.02);
}

test("Grid cells keep uniform cards while displaying true Photo aspect ratios", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const { base, root } = await fixture();
  const samples = [
    { name: "a-landscape.jpg", width: 320, height: 180 },
    { name: "b-portrait.jpg", width: 180, height: 320 },
    { name: "c-square.jpg", width: 256, height: 256 },
    { name: "d-panorama.jpg", width: 960, height: 160 },
  ];
  for (const sample of samples)
    await writeFile(
      join(root, sample.name),
      await jpegWithSize(page, sample.width, sample.height),
    );
  const running = await server(base, root);
  // One sample also carries a Photo-state indicator, so the footer grows and
  // the media area shrinks inside the very same uniform card. Samples sort by
  // path, so index 1 is b-portrait.jpg; the assertions below fail loudly if
  // that ordering changes.
  const ids = await browseIds(running.url);
  expect(ids).toHaveLength(samples.length);
  const factPhotoId = ids[1]!;
  await page.route("**/api/browse/**", async (route) => {
    if (route.request().method() !== "GET") {
      await route.continue();
      return;
    }
    const response = await route.fetch();
    const body = (await response.json()) as {
      photos: Array<{ id: string; ambiguous: boolean }>;
    };
    await route.fulfill({
      response,
      json: {
        ...body,
        photos: body.photos.map((photo) =>
          photo.id === factPhotoId ? { ...photo, ambiguous: true } : photo,
        ),
      },
    });
  });
  const loadedThumbnails = () =>
    page.evaluate(
      () =>
        Array.from(
          document.querySelectorAll<HTMLImageElement>(".photo-cell img"),
        ).filter((image) => image.complete && image.naturalWidth > 0).length,
    );
  for (const viewport of [
    { width: 1280, height: 800 },
    { width: 390, height: 844 },
    { width: 844, height: 390 },
  ]) {
    await page.setViewportSize(viewport);
    await page.goto(running.url);
    await expect(page.getByText("Ready · 4 Photos")).toBeVisible();
    await waitForGridFrame(page);
    await expect.poll(loadedThumbnails).toBe(4);
    const cells = await gridCellGeometry(page);
    expect(cells).toHaveLength(4);
    // Uniform cards: the Grid stays a regular, virtualizable unit grid.
    const widths = new Set(cells.map((cell) => Math.round(cell.cellWidth)));
    const heights = new Set(cells.map((cell) => Math.round(cell.cellHeight)));
    expect(widths.size).toBe(1);
    expect(heights.size).toBe(1);
    const naturalRatio = (cell: GridCellGeometry) =>
      cell.naturalWidth / cell.naturalHeight;
    // Thumbnail derivatives preserve the source ratio; the panorama is
    // downscaled to the bounded target, so classify by ratio, not pixels.
    const landscape = cells.find(
      (cell) => naturalRatio(cell) > 1.4 && naturalRatio(cell) < 2.2,
    );
    const portrait = cells.find((cell) => naturalRatio(cell) < 0.8);
    const square = cells.find(
      (cell) => Math.abs(naturalRatio(cell) - 1) < 0.05,
    );
    const panorama = cells.find((cell) => naturalRatio(cell) > 3);
    expect(
      landscape && portrait && square && panorama,
      "each sample aspect class renders one cell",
    ).toBeTruthy();
    // Exactly one cell renders the Photo-state indicator, and it is the
    // portrait whose thumbnail is loaded.
    expect(cells.filter((cell) => cell.facts !== null)).toHaveLength(1);
    expect(portrait!.facts).toBe("Ambiguous pairing");
    expect(landscape!.facts).toBeNull();
    // Sources below the bounded derivative target keep their exact pixels.
    expect([landscape!.naturalWidth, landscape!.naturalHeight]).toEqual([
      320, 180,
    ]);
    expect([portrait!.naturalWidth, portrait!.naturalHeight]).toEqual([
      180, 320,
    ]);
    expect([square!.naturalWidth, square!.naturalHeight]).toEqual([256, 256]);
    for (const cell of cells) {
      expectAspectRatio(cell);
      expect(cell.imageInsideMedia).toBe(true);
      // Indicators live beside or beneath the image, never over it.
      expect(cell.indicatorsOverlapImage).toBe(false);
      expect(cell.imageWidth).toBeLessThanOrEqual(cell.mediaWidth + 1);
      expect(cell.imageHeight).toBeLessThanOrEqual(cell.mediaHeight + 1);
    }
    // Orientation is visible at a glance: wide, tall, square, and panoramic
    // Photos render as distinct shapes inside identical cards.
    expect(landscape!.imageWidth).toBeGreaterThan(landscape!.imageHeight);
    expect(landscape!.imageWidth).toBeGreaterThanOrEqual(
      landscape!.mediaWidth - 1,
    );
    expect(portrait!.imageHeight).toBeGreaterThan(portrait!.imageWidth);
    expect(portrait!.imageHeight).toBeGreaterThanOrEqual(
      portrait!.mediaHeight - 1,
    );
    // The fact-bearing cell grew its footer and shrank its media area without
    // changing the uniform card box or the Photo's rendered ratio.
    expect(portrait!.mediaHeight).toBeLessThan(landscape!.mediaHeight);
    expect(Math.abs(portrait!.cellHeight - landscape!.cellHeight)).toBeLessThan(
      0.5,
    );
    expect(Math.abs(square!.imageWidth - square!.imageHeight)).toBeLessThan(
      1.5,
    );
    expect(panorama!.imageHeight).toBeLessThan(landscape!.imageHeight);
  }
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

test("a RAW and JPEG pair renders one Grid cell with the JPEG's single rotation", async ({
  page,
}) => {
  const { base, root } = await fixture();
  // Same stem, two Originals, so the pair forms one Photo. The JPEG claims EXIF
  // orientation 6 (320x180 displayed as 180x320) and the RAW bytes are
  // deliberately unreadable, so the Preview comes from the matching JPEG.
  await writeFile(
    join(root, "a.jpg"),
    withExifOrientation(await jpegWithSize(page, 320, 180), 6),
  );
  await writeFile(join(root, "a.ARW"), "raw-bytes-a");
  const running = await server(base, root);
  const opened = (await (
    await post(running.url, "/api/browse", { source: "library" })
  ).json()) as { token: string; total: number };
  const paired = await browseWindow(running.url, opened.token, 0);
  await fetch(`${running.url}/api/browse/${opened.token}`, {
    method: "DELETE",
    headers: { Origin: running.url },
  });
  // Pairing is name-based and independent of byte validity: the RAW and the
  // JPEG are one Photo with both members, not two Photos.
  expect(paired.total).toBe(1);
  expect(
    paired.photos[0]!.originals?.map((original) => original.kind).sort(),
  ).toEqual(["jpeg", "raw"]);
  await page.goto(running.url);
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
  await waitForGridFrame(page);
  await expect(page.locator(".photo-cell[data-photo-index]")).toHaveCount(1);
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
  // Orientation 6 is baked once for the pair too: a second rotation would
  // render 320x180.
  expect(cell!.naturalWidth).toBe(180);
  expect(cell!.naturalHeight).toBe(320);
  expect(cell!.imageHeight).toBeGreaterThan(cell!.imageWidth);
  // The complete composition displays: the ratio survives, nothing crops, and
  // no indicator covers the image.
  expectAspectRatio(cell!);
  expect(cell!.imageInsideMedia).toBe(true);
  expect(cell!.imageWidth).toBeLessThanOrEqual(cell!.mediaWidth + 1);
  expect(cell!.imageHeight).toBeLessThanOrEqual(cell!.mediaHeight + 1);
  expect(cell!.indicatorsOverlapImage).toBe(false);
});

test("Grid placeholders and late thumbnails never change cell geometry", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const { base, root } = await fixture();
  await writePhotos(root, 70);
  const running = await server(base, root);
  let holdThumbnails = true;
  const heldThumbnails: Array<() => void> = [];
  await page.route("**/api/derivatives/**", async (route) => {
    if (!holdThumbnails) return route.continue();
    await new Promise<void>((resolve) => heldThumbnails.push(resolve));
    return route.continue();
  });
  await page.goto(running.url);
  await expect(page.getByText("Ready · 70 Photos")).toBeVisible();
  await waitForGridFrame(page);
  await expect(
    page.locator(".photo-cell[data-photo-index]").first(),
  ).toBeVisible();
  // The thumbnail bytes are still held, so every image box is unresolved.
  expect(
    await page.evaluate(() =>
      Array.from(
        document.querySelectorAll<HTMLImageElement>(".photo-cell img"),
      ).every((image) => image.naturalWidth === 0),
    ),
  ).toBe(true);
  // Cells exist with final geometry before any thumbnail byte arrives.
  const beforeThumbnails = await cellBoxes(page);
  expect(beforeThumbnails.length).toBeGreaterThan(0);
  holdThumbnails = false;
  for (const release of heldThumbnails.splice(0)) release();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          Array.from(
            document.querySelectorAll<HTMLImageElement>(".photo-cell img"),
          ).filter((image) => image.complete && image.naturalWidth > 0).length,
      ),
    )
    .toBeGreaterThan(0);
  const afterThumbnails = await cellBoxes(page);
  expect(afterThumbnails).toEqual(beforeThumbnails);

  // A window that has not delivered facts yet keeps the same cell box while
  // it shows its placeholder.
  const heldWindows: Array<() => void> = [];
  let holdWindows = true;
  await page.route(
    (url) =>
      url.pathname.startsWith("/api/browse/") && url.searchParams.has("start"),
    async (route) => {
      if (!holdWindows) return route.continue();
      await new Promise<void>((resolve) => heldWindows.push(resolve));
      return route.continue();
    },
  );
  const viewport = page.locator("[data-grid-viewport]");
  await viewport.evaluate((element) => {
    element.scrollTop = 30 * 178;
    element.dispatchEvent(new Event("scroll"));
  });
  await expect(page.locator(".cell-placeholder").first()).toBeVisible();
  const placeholder = await page
    .locator(".cell-placeholder")
    .first()
    .evaluate((element) => {
      const cell = element.closest<HTMLElement>(".photo-cell");
      if (!cell) throw new Error("Placeholder cell is missing");
      const box = cell.getBoundingClientRect();
      return { width: box.width, height: box.height };
    });
  const loaded = beforeThumbnails[0]!;
  expect(Math.abs(placeholder.width - loaded.width)).toBeLessThan(0.5);
  expect(Math.abs(placeholder.height - loaded.height)).toBeLessThan(0.5);
  holdWindows = false;
  for (const release of heldWindows.splice(0)) release();
  await expect(page.locator(".cell-placeholder")).toHaveCount(0);
  const loadedCell = await page
    .locator(".photo-cell[data-photo-index]")
    .first()
    .evaluate((cell) => {
      const box = cell.getBoundingClientRect();
      return { width: box.width, height: box.height };
    });
  expect(Math.abs(loadedCell.width - placeholder.width)).toBeLessThan(0.5);
  expect(Math.abs(loadedCell.height - placeholder.height)).toBeLessThan(0.5);
});

test("a completed mutation cannot reopen or advance a superseding source", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Mutation Target");
  await page.goto(running.url);
  await page.locator('[data-photo-index="0"]').click();
  await expect(page.getByText("1 / 2")).toBeVisible();

  let release!: () => void;
  const stateHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let held = false;
  await page.route("**/api/photos/*/state", async (route) => {
    held = true;
    await stateHeld;
    try {
      await route.continue();
    } catch {
      /* the page may close only during failed test cleanup */
    }
  });
  try {
    await page.getByRole("button", { name: "Select" }).click();
    await expect.poll(() => held).toBe(true);
    const mutationCompleted = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname.endsWith("/state") &&
        response.status() === 200,
    );

    await openSources(page);
    await page
      .getByRole("button", { name: /^Mutation Target 2 Photos/ })
      .click();
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Mutation Target",
    );
    await expect(page.getByText(/^Ready · 2 Photos$/)).toBeVisible();
    release();
    await mutationCompleted;

    await expect(page.locator("[data-review]")).toBeHidden();
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Mutation Target",
    );
    await expect
      .poll(
        async () =>
          (await state(running.url, albumId)).members[0]!.selectionState,
      )
      .toBe("selected");
  } finally {
    release();
    await page.unroute("**/api/photos/*/state");
  }
});

test("an Undo Preview continuation cannot label or persist a newer Photo", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  const [firstPhotoId] = await browseIds(running.url);
  await page.goto(running.url);
  await page.locator('[data-photo-index="0"]').click();
  await expect(page.getByText("1 / 2")).toBeVisible();
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();

  let release!: () => void;
  const previewHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let held = false;
  await page.route(`**/api/photos/${firstPhotoId}/preview`, async (route) => {
    held = true;
    await previewHeld;
    try {
      await route.continue();
    } catch {
      /* navigating to the newer Photo aborts the Undo Preview */
    }
  });
  try {
    await page.getByRole("button", { name: /^Undo/ }).click();
    await expect.poll(() => held).toBe(true);
    await page.getByRole("button", { name: "Next" }).click();
    await expect(page.getByText("2 / 2")).toBeVisible();
    release();
    await expect(page.getByText("Last change undone.")).toHaveCount(0);
  } finally {
    release();
    await page.unroute(`**/api/photos/${firstPhotoId}/preview`);
  }
});

test("opening a Photo from the Grid persists the Album position", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg", "d.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "GridPos");
  await openGrid(page, running.url, "GridPos");
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /^Photo 3 of 4/ }),
  );
  await expect(page.getByText("3 / 4")).toBeVisible();
  await expect
    .poll(async () => (await state(running.url, albumId)).position)
    .toBe(2);

  await page.reload();
  let browsePosition: number | undefined;
  page.on("response", (response) => {
    if (
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/browse"
    )
      void response
        .json()
        .then(
          (body: { position?: number }) =>
            (browsePosition = body.position ?? browsePosition),
        )
        .catch(() => undefined);
  });
  await openSources(page);
  await page.getByRole("button", { name: /^GridPos(?: |$)/ }).click();
  await page.getByText(/^Ready · /).waitFor();
  await expect(
    page.getByRole("button", { name: /^Photo 3 of 4/ }),
  ).toBeVisible();
  await expect.poll(() => browsePosition).toBe(2);
});

test("Undo returns to the affected Photo and refreshes its Preview and facts", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await openGrid(page, running.url, "Review");
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /^Photo 1 of/ }),
  );
  await expect(page.getByText("1 / 3")).toBeVisible();
  await expect(page.locator("[data-stage] img")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Undo" }).click(),
  );
  await expect(page.getByText("1 / 3")).toBeVisible();
  await expect(page.getByText("Undecided", { exact: true })).toBeVisible();
  await expect(page.getByText("Last change undone.")).toBeVisible();
  await expect(page.locator("[data-stage] img")).toBeVisible();
});

test("Undo reloads the affected Photo after its facts leave the loaded window", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (let index = 0; index < 200; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(3, "0")}.jpg`),
      await jpeg(),
    );
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Wide");
  await openGrid(page, running.url, "Wide");
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /^Photo 1 of 200/ }),
  );
  await expect(page.getByText("1 / 200")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByText("2 / 200")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);
  await evictFirstPhotoFact(page);

  const reloaded = page.waitForRequest((request) => {
    const url = new URL(request.url());
    return (
      request.method() === "GET" &&
      url.pathname.startsWith("/api/browse/") &&
      url.searchParams.get("start") === "0" &&
      url.searchParams.get("limit") === "60"
    );
  });
  await Promise.all([
    reloaded,
    actionWithProgress(page, albumId, () => page.keyboard.press("Control+z")),
  ]);

  await expect(page.getByText("1 / 200")).toBeVisible();
  await expect(page.getByText("Undecided", { exact: true })).toBeVisible();
  await expect(page.getByText("Last change undone.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "undecided",
  );
});

test("an evicted Undo reload cannot write into a replacement source", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 200);
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Wide Race");
  await openGrid(page, running.url, "Wide Race");
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /^Photo 1 of 200/ }),
  );
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);
  await evictFirstPhotoFact(page);

  const firstPhotoId = (await state(running.url, albumId)).members[0]!.photoId;
  let release!: () => void;
  const reloadHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let held = false;
  let heldUrl: string | undefined;
  let stateWrites = 0;
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === `/api/photos/${firstPhotoId}/state`
    )
      stateWrites += 1;
  });
  await page.route("**/api/browse/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (
      !held &&
      request.method() === "GET" &&
      url.searchParams.get("start") === "0"
    ) {
      held = true;
      heldUrl = request.url();
      await reloadHeld;
      try {
        await route.continue();
      } catch {
        /* the replacement source aborts the evicted Undo reload */
      }
      return;
    }
    await route.continue();
  });
  try {
    await page.keyboard.press("Control+z");
    await expect.poll(() => held).toBe(true);
    const oldReloadCanceled = page.waitForEvent(
      "requestfailed",
      (request) => request.url() === heldUrl,
    );

    await openSources(page);
    await page.getByRole("button", { name: /^All Photos 200 Photos/ }).click();
    await expect(page.locator("[data-grid-title]")).toHaveText("All Photos");
    await expect(page.getByText("Ready · 200 Photos")).toBeVisible();
    await oldReloadCanceled;
    release();
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
    );

    expect(stateWrites).toBe(0);
    await expect(page.locator("[data-undo]")).toBeDisabled();
    expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
      "selected",
    );
  } finally {
    release();
    await page.unroute("**/api/browse/**");
  }
});

test("failed Browse recovery clears the expired token before Retry opens a fresh snapshot", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 130);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await openGrid(page, running.url, "All Photos");

  let expired = false;
  let expiredToken = "";
  let reopenAttempts = 0;
  let releases = 0;
  let holdFreshOpen = false;
  let releaseFreshOpen: () => void = () => undefined;
  const freshOpenGate = new Promise<void>((resolve) => {
    releaseFreshOpen = resolve;
  });
  const boundaryTokens: string[] = [];
  await page.route(/\/api\/browse/, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const pathToken = url.pathname.split("/").at(-1)!;
    if (request.method() === "GET" && url.searchParams.get("start") === "60") {
      boundaryTokens.push(pathToken);
      if (!expired) {
        expired = true;
        expiredToken = pathToken;
        await route.fulfill({
          status: 404,
          contentType: "application/json",
          body: '{"error":"Browse source expired or not found"}',
        });
        return;
      }
    }
    if (request.method() === "DELETE" && pathToken === expiredToken) {
      releases += 1;
    }
    if (request.method() === "POST" && expired) {
      reopenAttempts += 1;
      if (reopenAttempts === 1) {
        await route.fulfill({ status: 503, body: '{"error":"reopen failed"}' });
        return;
      }
      if (holdFreshOpen) await freshOpenGate;
    }
    await route.continue();
  });
  const scrollBoundary = () =>
    page.locator("[data-grid-viewport]").evaluate((element) => {
      element.scrollTop = 30 * 178;
      element.dispatchEvent(new Event("scroll"));
    });
  try {
    await scrollBoundary();
    await expect.poll(() => reopenAttempts).toBe(1);
    await expect(page.locator("[data-status]")).toHaveText(
      "This source expired and could not be reopened. Retry the connection.",
    );
    const requestsAfterFailure = boundaryTokens.length;
    await scrollBoundary();
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
    );
    expect(boundaryTokens).toHaveLength(requestsAfterFailure);

    const freshOpen = page.waitForRequest(
      (request) =>
        request.method() === "POST" &&
        new URL(request.url()).pathname === "/api/browse",
    );
    await page.locator("[data-source-toggle]").click();
    const sourceRetry = page.getByRole("button", { name: "Retry connection" });
    await expect(sourceRetry).toBeVisible();
    await expect(sourceRetry).toBeEnabled();
    holdFreshOpen = true;
    const sourceRetryAdmitted = await sourceRetry.evaluate((button) => {
      (button as HTMLButtonElement).click();
      return (button as HTMLButtonElement).disabled;
    });
    expect(sourceRetryAdmitted).toBe(true);
    const freshRequest = await freshOpen;
    await expect(page.locator("[data-retry]")).toBeDisabled();
    releaseFreshOpen();
    const freshResponse = await freshRequest.response();
    expect(freshResponse?.status()).toBe(200);
    const freshToken = ((await freshResponse!.json()) as { token: string })
      .token;
    expect(freshToken).not.toBe(expiredToken);
    await expect(page.getByText("Ready · 130 Photos")).toBeVisible();
    await expect(page.locator("[data-retry]")).toBeEnabled();

    const freshBoundary = page.waitForRequest((request) => {
      const url = new URL(request.url());
      return (
        request.method() === "GET" &&
        url.searchParams.get("start") === "60" &&
        url.pathname.endsWith(`/${freshToken}`)
      );
    });
    await scrollBoundary();
    await freshBoundary;
    expect(boundaryTokens.slice(1)).not.toContain(expiredToken);
    expect(releases).toBe(1);
    expect(reopenAttempts).toBe(2);
  } finally {
    releaseFreshOpen();
    await page.unroute(/\/api\/browse/);
  }
});

test("repeated failure of one source range keeps one exact Recovery owner", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 130);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await openGrid(page, running.url, "All Photos");

  let attempts = 0;
  let failing = true;
  await page.route(/\/api\/browse\//, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (request.method() === "GET" && url.searchParams.get("start") === "60") {
      attempts += 1;
      if (failing) {
        await route.fulfill({ status: 503, body: '{"error":"failed"}' });
        return;
      }
    }
    await route.continue();
  });
  const scrollBoundary = () =>
    page.locator("[data-grid-viewport]").evaluate((element) => {
      element.scrollTop = 30 * 178;
      element.dispatchEvent(new Event("scroll"));
    });
  try {
    await scrollBoundary();
    await expect.poll(() => attempts).toBeGreaterThanOrEqual(1);
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();

    const afterFirst = attempts;
    await scrollBoundary();
    await expect.poll(() => attempts).toBeGreaterThan(afterFirst);
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();

    failing = false;
    const recovered = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return (
        response.request().method() === "GET" &&
        url.searchParams.get("start") === "60" &&
        response.status() === 200
      );
    });
    await scrollBoundary();
    await recovered;
    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    await expect(page.locator('[data-photo-index="60"]')).toBeVisible();
  } finally {
    await page.unroute(/\/api\/browse\//);
  }
});

test("a source establishment failure retires the claim it replaces", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 120);
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Second Source");
  await page.setViewportSize({ width: 390, height: 844 });

  let libraryToken = "";
  let albumPhase = false;
  let failLibraryRange = true;
  let failAlbumWindow = true;
  let albumWindowFailures = 0;
  await page.route(/\/api\/browse\//, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const start = url.searchParams.get("start");
    if (request.method() === "GET" && url.pathname.endsWith(libraryToken)) {
      if (failLibraryRange && start === "60") {
        await route.fulfill({ status: 503, body: '{"error":"failed"}' });
        return;
      }
    }
    if (
      request.method() === "GET" &&
      albumPhase &&
      url.pathname !== `/api/browse/${libraryToken}` &&
      start === "0"
    ) {
      albumWindowFailures += 1;
      if (failAlbumWindow) {
        await route.fulfill({ status: 503, body: '{"error":"failed"}' });
        return;
      }
    }
    await route.continue();
  });

  try {
    const openedResponse = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname === "/api/browse",
    );
    await page.goto(running.url);
    libraryToken = ((await (await openedResponse).json()) as { token: string })
      .token;
    await expect(page.getByText(/^Ready · 120 Photos$/)).toBeVisible();

    // The first source owns a blocking range failure.
    await page.locator("[data-grid-viewport]").evaluate((element) => {
      element.scrollTop = element.scrollHeight;
      element.dispatchEvent(new Event("scroll"));
    });
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();

    // Opening the second source fails on its establishing window, which
    // replaces the first source's claim with its own.
    await openSources(page);
    const albumOpen = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname === "/api/browse" &&
        (response.request().postDataJSON() as { albumId?: string }).albumId ===
          albumId,
    );
    albumPhase = true;
    await page.getByRole("button", { name: /^Second Source/ }).click();
    await expect.poll(() => albumWindowFailures).toBeGreaterThan(0);
    albumPhase = false;
    await albumOpen;
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();

    // Retrying the second source releases its claim and the predecessor
    // claim the transition replaced.
    failAlbumWindow = false;
    failLibraryRange = false;
    await openSources(page);
    await page.getByRole("button", { name: "Retry connection" }).click();
    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await expect(page.locator("[data-sources]")).toBeHidden();
    await page.locator('[data-photo-index="0"]').click();
    await expect(page.getByText(/^1 \/ 120$/)).toBeVisible();
    await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
  } finally {
    await page.unroute(/\/api\/browse\//);
  }
});

test("a failed expired reopen binds the thumbnails of its retained cells again", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 250);
  const running = await server(base, root);

  // Every Grid thumbnail image stays in flight: its source is set and its
  // bytes never arrive, which is the image state a Grid boundary detaches.
  let releaseImages!: () => void;
  const imagesReleased = new Promise<void>((resolve) => {
    releaseImages = resolve;
  });
  await page.route("**/api/derivatives/*/thumbnail/*", async (route) => {
    await imagesReleased;
    await route.continue();
  });

  let releaseExpired!: () => void;
  const expiredReleased = new Promise<void>((resolve) => {
    releaseExpired = resolve;
  });
  let releaseReopen!: () => void;
  const reopenReleased = new Promise<void>((resolve) => {
    releaseReopen = resolve;
  });
  try {
    await openGrid(page, running.url, "All Photos");
    const viewport = page.locator("[data-grid-viewport]");
    // Install the Browse route only after the source is open: the first
    // window it holds is the one a scroll demand asks for.
    let holdFirstWindow = true;
    let expiredWindowStart: string | undefined;
    let reopenRequested = false;
    let reopenHeld = false;
    let reopenServingWindow = false;
    await page.route(/\/api\/browse/, async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (request.method() === "POST" && reopenRequested) {
        reopenHeld = true;
        await reopenReleased;
        reopenServingWindow = true;
        await route.continue();
        return;
      }
      if (request.method() === "GET") {
        const start = url.searchParams.get("start");
        if (holdFirstWindow && start !== null) {
          holdFirstWindow = false;
          expiredWindowStart = start;
          await expiredReleased;
          reopenRequested = true;
          await route.fulfill({
            status: 404,
            contentType: "application/json",
            body: '{"error":"Browse source expired or not found"}',
          });
          return;
        }
        if (reopenServingWindow) {
          reopenServingWindow = false;
          await route.fulfill({
            status: 503,
            contentType: "application/json",
            body: '{"error":"failed"}',
          });
          return;
        }
      }
      await route.continue();
    });

    // Present an unloaded middle window and hold its answer, then present the
    // loaded tail: the reopen is admitted while real retained tail cells are
    // still rendered.
    const middleScrollTop = await viewport.evaluate(
      (element) => element.scrollHeight / 2,
    );
    await scrollGrid(page, middleScrollTop);
    await expect.poll(() => expiredWindowStart !== undefined).toBe(true);
    await scrollGrid(page, "end");
    const tailPhoto = page.getByRole("button", {
      name: /^Photo 250 of 250/,
    });
    await expect(tailPhoto).toBeVisible();
    await expect(tailPhoto).toBeEnabled();
    await expect(tailPhoto.locator("img")).toHaveAttribute("src", /\S/);

    // Serve the held window as an expired Snapshot, and hold the reopen so
    // its own window is still pending when the retained cells rebuild.
    releaseExpired();
    await expect.poll(() => reopenHeld).toBe(true);

    // The reopen detached the images it owned mid-flight. The retained cells
    // must bind their thumbnails again instead of staying blank until the
    // next scroll.
    await expect(tailPhoto.locator("img")).toHaveAttribute("src", /\S/);
    releaseReopen();
    await expect(page.locator("[data-grid-status]")).toContainText("503");
    await expect(tailPhoto).toBeVisible();
  } finally {
    releaseExpired();
    releaseReopen();
    releaseImages();
    await page.unroute(/\/api\/browse/);
    await page.unroute("**/api/derivatives/*/thumbnail/*");
  }
});

test("Grid Retry replays a clamped tail range from its original Photo anchor", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 70);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  let overviewRequests = 0;
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (request.method() === "GET" && url.pathname === "/api/overview")
      overviewRequests += 1;
  });
  await openGrid(page, running.url, "All Photos");
  await expect.poll(() => overviewRequests).toBeGreaterThan(0);
  const initialOverviewRequests = overviewRequests;

  let tailAttempts = 0;
  let failing = true;
  let holdTailRetry = false;
  let releaseTailRetry: () => void = () => undefined;
  const tailRetryGate = new Promise<void>((resolve) => {
    releaseTailRetry = resolve;
  });
  let markTailRetryStarted!: () => void;
  const tailRetryStarted = new Promise<void>((resolve) => {
    markTailRetryStarted = resolve;
  });
  let tailRetryObserved = false;
  let browseAllocations = 0;
  const tokens: string[] = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (request.method() === "POST" && url.pathname === "/api/browse")
      browseAllocations += 1;
  });
  await page.route(/\/api\/browse\//, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (request.method() === "GET" && url.searchParams.get("start") === "10") {
      tailAttempts += 1;
      tokens.push(url.pathname.split("/").at(-1)!);
      if (failing) {
        await route.fulfill({ status: 503, body: '{"error":"failed"}' });
        return;
      }
      if (holdTailRetry) {
        if (!tailRetryObserved) {
          tailRetryObserved = true;
          markTailRetryStarted();
        }
        await tailRetryGate;
      }
    }
    await route.continue();
  });
  try {
    await page.locator("[data-grid-viewport]").evaluate((element) => {
      element.scrollTop = element.scrollHeight;
      element.dispatchEvent(new Event("scroll"));
    });
    await expect.poll(() => tailAttempts).toBeGreaterThan(0);
    await expect(page.locator("[data-grid-status]")).toHaveText(
      "Photos 11–70 could not be loaded (HTTP 503). Retry this range.",
    );
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();

    const failedAttempts = tailAttempts;
    failing = false;
    holdTailRetry = true;
    await page.locator("[data-source-toggle]").click();
    const sourceRetry = page.getByRole("button", { name: "Retry connection" });
    await sourceRetry.click();
    await tailRetryStarted;
    await expect.poll(() => tailAttempts).toBeGreaterThan(failedAttempts);
    await expect(sourceRetry).toBeDisabled();
    releaseTailRetry();
    await expect(page.locator("[data-grid-status]")).toHaveText(
      "Ready · 70 Photos",
    );
    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    await expect(page.locator('[data-photo-index="69"]')).toBeVisible();
    expect(new Set(tokens).size).toBe(1);
    expect(browseAllocations).toBe(0);
    expect(overviewRequests).toBe(initialOverviewRequests);
  } finally {
    releaseTailRetry();
    await page.unroute(/\/api\/browse\//);
  }
});

test("Grid Retry reloads only exact failed ranges on the current Browse token", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 180);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  const thumbnailRoute = "**/api/photos/*/thumbnail";
  await page.route(thumbnailRoute, (route) =>
    route.fulfill({ status: 503, body: '{"error":"not under test"}' }),
  );
  let overviewRequests = 0;
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (request.method() === "GET" && url.pathname === "/api/overview")
      overviewRequests += 1;
  });
  await openGrid(page, running.url, "All Photos");
  await expect.poll(() => overviewRequests).toBeGreaterThan(0);
  const initialOverviewRequests = overviewRequests;

  const attempts = new Map<string, number>();
  const tokens: string[] = [];
  let phase: "initial" | "first-retry" | "final-retry" = "initial";
  let browseAllocations = 0;
  let firstWindowReloads = 0;
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (request.method() === "POST" && url.pathname === "/api/browse")
      browseAllocations += 1;
    if (
      request.method() === "GET" &&
      url.pathname.startsWith("/api/browse/") &&
      url.searchParams.get("start") === "0"
    )
      firstWindowReloads += 1;
  });
  await page.route(/\/api\/browse\//, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const start = url.searchParams.get("start");
    if (request.method() !== "GET" || (start !== "60" && start !== "120")) {
      await route.continue();
      return;
    }
    tokens.push(url.pathname.split("/").at(-1)!);
    const attempt = (attempts.get(start) ?? 0) + 1;
    attempts.set(start, attempt);
    if (phase === "initial" || (phase === "first-retry" && start === "120")) {
      await route.fulfill({ status: 503, body: '{"error":"failed"}' });
      return;
    }
    await route.continue();
  });
  const scrollToIndex = (index: number) =>
    page.locator("[data-grid-viewport]").evaluate((element, target) => {
      element.scrollTop = target === 0 ? 0 : (target / 2 + 2) * 178;
      element.dispatchEvent(new Event("scroll"));
    }, index);
  try {
    await scrollToIndex(60);
    await expect.poll(() => attempts.get("60") ?? 0).toBeGreaterThan(0);
    await expect(page.locator("[data-grid-status]")).toHaveText(
      "Photos 61–120 could not be loaded (HTTP 503). Retry this range.",
    );
    await scrollToIndex(120);
    await expect.poll(() => attempts.get("120") ?? 0).toBeGreaterThan(0);
    await expect(page.locator("[data-grid-status]")).toHaveText(
      "Photos 121–180 could not be loaded (HTTP 503). Retry this range.",
    );
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();

    const initial60 = attempts.get("60")!;
    const initial120 = attempts.get("120")!;
    phase = "first-retry";
    await page.locator("[data-source-toggle]").click();
    await page.getByRole("button", { name: "Retry connection" }).click();
    await expect.poll(() => attempts.get("60") ?? 0).toBeGreaterThan(initial60);
    await expect
      .poll(() => attempts.get("120") ?? 0)
      .toBeGreaterThan(initial120);
    await expect(page.locator("[data-grid-status]")).toHaveText(
      "Photos 121–180 could not be loaded (HTTP 503). Retry this range.",
    );
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
    expect(browseAllocations).toBe(0);
    expect(overviewRequests).toBe(initialOverviewRequests);
    expect(firstWindowReloads).toBe(0);
    expect(new Set(tokens).size).toBe(1);

    const recovered60 = attempts.get("60")!;
    const failed120 = attempts.get("120")!;
    phase = "final-retry";
    await page.getByRole("button", { name: "Retry connection" }).click();
    await expect
      .poll(() => attempts.get("120") ?? 0)
      .toBeGreaterThan(failed120);
    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    expect(attempts.get("60")).toBe(recovered60);
    expect(browseAllocations).toBe(0);
    expect(overviewRequests).toBe(initialOverviewRequests);

    await page.getByRole("button", { name: "Close" }).click();
    await scrollToIndex(0);
    await expect(page.locator('[data-photo-index="0"]')).toBeEnabled();
    expect(firstWindowReloads).toBe(0);
  } finally {
    await page.unroute(/\/api\/browse\//);
    await page.unroute(thumbnailRoute);
  }
});
test("an expired Album snapshot replaces retired membership memory", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (let index = 0; index < 250; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(3, "0")}.jpg`),
      await jpeg(),
    );
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Expiry");
  await openGrid(page, running.url, "Expiry");
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /^Photo 1 of 250/ }),
  );
  await expect(page.getByText("1 / 250")).toBeVisible();
  const firstId = (await state(running.url, albumId)).members[0]!.photoId;
  await openMembershipPanel(page);
  await membershipCheckbox(page, "Expiry").uncheck();
  await expect(
    page.getByText(
      "Removed from the Album. It stays in this open view until reopened.",
    ),
  ).toBeVisible();
  await expect(membershipCheckbox(page, "Expiry")).not.toBeChecked();
  const readded = await post(running.url, `/api/albums/${albumId}/members`, {
    photoIds: [firstId],
  });
  expect(readded.status).toBe(200);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);
  const viewport = page.locator("[data-grid-viewport]");
  // Retain a tail Photo while a middle range remains unopened. The later
  // expiry therefore starts replacement while a real old cell can render.
  await viewport.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
    element.dispatchEvent(new Event("scroll"));
  });
  await expect(
    page.getByRole("button", { name: /^Photo 250 of 250/ }),
  ).toBeVisible();
  const reopenBodies: Array<Record<string, unknown>> = [];
  let releaseReopen!: () => void;
  const reopenReleased = new Promise<void>((resolve) => {
    releaseReopen = resolve;
  });
  let markReopenStarted!: () => void;
  const reopenStarted = new Promise<void>((resolve) => {
    markReopenStarted = resolve;
  });
  let expiredServed = false;
  await page.route(/\/api\/browse/, async (route) => {
    const request = route.request();
    if (request.method() === "POST") {
      reopenBodies.push(request.postDataJSON() as Record<string, unknown>);
      markReopenStarted();
      await reopenReleased;
      await route.continue();
      return;
    }
    if (request.method() === "GET" && !expiredServed) {
      expiredServed = true;
      await route.fulfill({
        status: 404,
        contentType: "application/json",
        body: '{"error":"Browse source expired or not found"}',
      });
      return;
    }
    await route.continue();
  });
  try {
    await viewport.evaluate((element) => {
      element.scrollTop = element.scrollHeight / 2;
      element.dispatchEvent(new Event("scroll"));
    });
    await expect.poll(() => expiredServed).toBe(true);
    await expect
      .poll(() =>
        reopenBodies.some(
          (body) =>
            body.source === "album" &&
            body.albumId === albumId &&
            body.photoId === firstId,
        ),
      )
      .toBe(true);
    await reopenStarted;
    await viewport.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
      element.dispatchEvent(new Event("scroll"));
    });
    const tailPhoto = page.getByRole("button", {
      name: /^Photo 250 of 250/,
    });
    await expect(tailPhoto).toBeVisible();
    await expect(tailPhoto).toBeDisabled();
    releaseReopen();
    await expect(tailPhoto).toBeEnabled();
    await openPhotoAndWaitForProgress(page, albumId, tailPhoto);
    await openMembershipPanel(page);
    await expect(membershipCheckbox(page, "Expiry")).toBeChecked();
  } finally {
    releaseReopen();
    await page.unroute(/\/api\/browse/);
  }
});

const replacementFirstWindowFailures = [
  {
    name: "returns HTTP 503",
    status: "could not be loaded (HTTP 503)",
    response: {
      status: 503,
      contentType: "application/json",
      body: '{"error":"replacement window unavailable"}',
    },
  },
  {
    name: "returns malformed data",
    status: "returned an invalid response",
    response: {
      status: 200,
      contentType: "application/json",
      body: '{"photos":[]}',
    },
  },
] as const;

for (const failure of replacementFirstWindowFailures) {
  test(`a replacement Album keeps retained Grid cells unavailable when its first window ${failure.name}`, async ({
    page,
  }) => {
    const { base, root } = await fixture();
    await writePhotos(root, 250);
    const running = await server(base, root);
    const { albumId } = await createAlbum(running.url, "Replacement failure");
    await openGrid(page, running.url, "Replacement failure");
    await openPhotoAndWaitForProgress(
      page,
      albumId,
      page.getByRole("button", { name: /^Photo 1 of 250/ }),
    );
    const firstId = (await state(running.url, albumId)).members[0]!.photoId;
    await openMembershipPanel(page);
    await membershipCheckbox(page, "Replacement failure").uncheck();
    await expect(
      page.getByText(
        "Removed from the Album. It stays in this open view until reopened.",
      ),
    ).toBeVisible();
    const readded = await post(running.url, `/api/albums/${albumId}/members`, {
      photoIds: [firstId],
    });
    expect(readded.status).toBe(200);
    await page.getByRole("button", { name: "Back to Grid" }).click();
    await waitForGridFrame(page);

    const viewport = page.locator("[data-grid-viewport]");
    await viewport.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
      element.dispatchEvent(new Event("scroll"));
    });
    const tailPhoto = page.getByRole("button", {
      name: /^Photo 250 of 250/,
    });
    await expect(tailPhoto).toBeVisible();
    await expect(tailPhoto).toBeEnabled();

    const reopenBodies: Array<Record<string, unknown>> = [];
    let releaseExpired!: () => void;
    const expiredReleased = new Promise<void>((resolve) => {
      releaseExpired = resolve;
    });
    let releaseReopen!: () => void;
    const reopenReleased = new Promise<void>((resolve) => {
      releaseReopen = resolve;
    });
    let markReopenStarted!: () => void;
    const reopenStarted = new Promise<void>((resolve) => {
      markReopenStarted = resolve;
    });
    let expiredRequested = false;
    let expiredServed = false;
    let replacementWindowFailures = 0;
    const replacementWindow = page
      .waitForResponse(
        (response) =>
          response.request().method() === "POST" &&
          new URL(response.url()).pathname === "/api/browse" &&
          response.status() === 200,
      )
      .then(async (response) => {
        const body = (await response.json()) as {
          position: number;
          token: string;
          total: number;
        };
        const windowSize = 60;
        return {
          token: body.token,
          start: Math.max(
            0,
            Math.min(
              Math.floor(body.position / windowSize) * windowSize,
              Math.max(0, body.total - windowSize),
            ),
          ),
        };
      });
    await page.route(/\/api\/browse/, async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (request.method() === "POST") {
        reopenBodies.push(request.postDataJSON() as Record<string, unknown>);
        markReopenStarted();
        await reopenReleased;
        await route.continue();
        return;
      }
      if (request.method() !== "GET") {
        await route.continue();
        return;
      }
      if (!expiredServed) {
        expiredRequested = true;
        await expiredReleased;
        expiredServed = true;
        await route.fulfill({
          status: 404,
          contentType: "application/json",
          body: '{"error":"Browse source expired or not found"}',
        });
        return;
      }
      const replacement = await replacementWindow;
      if (url.pathname === `/api/browse/${replacement.token}`) {
        if (url.searchParams.get("start") === String(replacement.start)) {
          replacementWindowFailures += 1;
          await route.fulfill(failure.response);
          return;
        }
        await route.continue();
        return;
      }
      await route.continue();
    });

    try {
      // Present an unloaded middle window so the first Browse request after
      // this point fails on the expired Snapshot, then present the loaded tail
      // again: the reopen is admitted while real retained tail cells are still
      // rendered, without rendering another virtualized range for it.
      const middleScrollTop = await viewport.evaluate(
        (element) => element.scrollHeight / 2,
      );
      await scrollGrid(page, middleScrollTop);
      await expect.poll(() => expiredRequested).toBe(true);
      await scrollGrid(page, "end");
      await expect(tailPhoto).toBeVisible();
      await expect(tailPhoto).toBeEnabled();
      releaseExpired();
      await expect.poll(() => expiredServed).toBe(true);
      await expect
        .poll(() =>
          reopenBodies.some(
            (body) =>
              body.source === "album" &&
              body.albumId === albumId &&
              body.photoId === firstId,
          ),
        )
        .toBe(true);
      await reopenStarted;
      await expect(tailPhoto).toBeVisible();
      await expect(tailPhoto).toBeDisabled();
      releaseReopen();
      await replacementWindow;
      await expect.poll(() => replacementWindowFailures).toBeGreaterThan(0);
      await expect(page.locator("[data-grid-status]")).toContainText(
        failure.status,
      );
      await expect(tailPhoto).toBeVisible();
      await expect(tailPhoto).toBeDisabled();
    } finally {
      releaseExpired();
      releaseReopen();
      await page.unroute(/\/api\/browse/);
    }
  });
}

test("a failed expired Album reopen retains retired membership memory", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 70);
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Expiry failure");
  await openGrid(page, running.url, "Expiry failure");
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /^Photo 1 of 70/ }),
  );
  await openMembershipPanel(page);
  await membershipCheckbox(page, "Expiry failure").uncheck();
  await expect(
    page.getByText(
      "Removed from the Album. It stays in this open view until reopened.",
    ),
  ).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();

  let expiredServed = false;
  await page.route(/\/api\/browse/, async (route) => {
    const request = route.request();
    if (request.method() === "POST") {
      await route.fulfill({
        status: 503,
        contentType: "application/json",
        body: '{"error":"reopen unavailable"}',
      });
      return;
    }
    if (request.method() === "GET" && !expiredServed) {
      expiredServed = true;
      await route.fulfill({
        status: 404,
        contentType: "application/json",
        body: '{"error":"Browse source expired or not found"}',
      });
      return;
    }
    await route.continue();
  });
  const viewport = page.locator("[data-grid-viewport]");
  await viewport.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await expect.poll(() => expiredServed).toBe(true);
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "This source expired and could not be reopened. Retry the connection.",
  );

  await viewport.evaluate((element) => {
    element.scrollTop = 0;
  });
  await expect(
    page.getByRole("button", { name: /^Photo 1 of 70/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: /^Photo 1 of 70/ }).click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await expect(membershipCheckbox(page, "Expiry failure")).not.toBeChecked();
});

test("Photo View recovery defers Grid windows until Grid is visible", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 130);
  const running = await server(base, root);
  await page.addInitScript(() => {
    const admissions: Array<{ token: string; start: string }> = [];
    Object.defineProperty(window, "__slipstreamGridAdmissions", {
      value: admissions,
    });
    const nativeFetch = window.fetch.bind(window);
    window.fetch = ((input, init) => {
      if (typeof input === "string") {
        const url = new URL(input, window.location.href);
        if (url.pathname.startsWith("/api/browse/") && !init?.method)
          admissions.push({
            token: url.pathname.split("/").at(-1) ?? "",
            start: url.searchParams.get("start") ?? "",
          });
      }
      return nativeFetch(input, init);
    }) as typeof window.fetch;
  });
  await openGrid(page, running.url, "All Photos");

  const viewport = page.locator("[data-grid-viewport]");
  await viewport.evaluate((element) => {
    Object.defineProperty(element, "clientWidth", {
      configurable: true,
      value: 900,
    });
    Object.defineProperty(element, "clientHeight", {
      configurable: true,
      value: 900,
    });
    window.dispatchEvent(new Event("resize"));
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await viewport.evaluate((element) => {
    element.scrollTop = 4 * 178;
    element.dispatchEvent(new Event("scroll"));
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await expect(page.locator('[data-photo-index="59"]')).toHaveCount(1);
  // Settle the visible tail before entering Photo View. Reopen then clears the
  // retained facts, leaving a known missing tail for the hidden render below.
  await expect(page.locator('[data-photo-index="60"]')).toHaveCount(1);
  await page.evaluate(() => {
    const gridView = document.querySelector<HTMLElement>("[data-grid-view]");
    const gridLayer = document.querySelector<HTMLElement>("[data-grid-layer]");
    if (!gridView || !gridLayer) throw new Error("Grid surface not found");
    const hiddenMutations = { count: 0 };
    Object.defineProperty(window, "__slipstreamHiddenGridMutations", {
      value: hiddenMutations,
    });
    new MutationObserver((records) => {
      if (gridView.hidden)
        hiddenMutations.count += records.filter(
          (record) => record.type === "childList",
        ).length;
    }).observe(gridLayer, { childList: true });
  });

  let previewMode: "failed" | "unavailable" = "failed";
  await page.route("**/api/photos/*/preview", async (route) => {
    if (previewMode === "failed") {
      await route.fulfill({ status: 503, body: '{"error":"failed"}' });
      return;
    }
    await route.fulfill({
      status: 404,
      contentType: "application/json",
      body: '{"state":"unavailable","message":"reopen preview unavailable"}',
    });
  });

  let expiredServed = false;
  let reopenStarted = false;
  let markReopenStarted: () => void = () => undefined;
  const reopenStartedGate = new Promise<void>((resolve) => {
    markReopenStarted = resolve;
  });
  let releaseReopen: () => void = () => undefined;
  const reopenGate = new Promise<void>((resolve) => {
    releaseReopen = resolve;
  });
  await page.route(/\/api\/browse(?:\/|$)/, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (request.method() === "GET" && url.pathname.startsWith("/api/browse/")) {
      const start = url.searchParams.get("start") ?? "";
      if (!expiredServed && start === "0") {
        expiredServed = true;
        await route.fulfill({
          status: 404,
          contentType: "application/json",
          body: '{"error":"Browse source expired or not found"}',
        });
        return;
      }
      await route.continue();
      return;
    }
    if (
      request.method() === "POST" &&
      url.pathname === "/api/browse" &&
      expiredServed &&
      !reopenStarted
    ) {
      reopenStarted = true;
      markReopenStarted();
      await reopenGate;
    }
    await route.continue();
  });

  try {
    await page.locator('[data-photo-index="59"]').click();
    await expect(page.getByText("60 / 130")).toBeVisible();
    await expect(page.locator("[data-retry-photo]")).toBeVisible();
    previewMode = "unavailable";

    const replacementOpen = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname === "/api/browse" &&
        response.status() === 200,
    );
    const replacementWindow = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return (
        response.request().method() === "GET" &&
        url.pathname.startsWith("/api/browse/") &&
        url.searchParams.get("start") === "0" &&
        response.status() === 200
      );
    });
    await page.locator("[data-retry-photo]").click();
    await reopenStartedGate;
    await expect(page.locator("[data-review]")).toBeVisible();
    releaseReopen();

    const opened = (await replacementOpen).json() as Promise<{
      token: string;
    }>;
    const reopenedToken = (await opened).token;
    await replacementWindow;
    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    await expect(page.locator("[data-stage]")).toContainText(
      "Preview unavailable",
    );

    const hiddenGridMutations = await page.evaluate(
      () =>
        (
          window as typeof window & {
            __slipstreamHiddenGridMutations: { count: number };
          }
        ).__slipstreamHiddenGridMutations.count,
    );
    expect(hiddenGridMutations).toBe(0);
    const hiddenTailAdmissions = await page.evaluate(
      (token) =>
        (
          window as typeof window & {
            __slipstreamGridAdmissions: Array<{
              token: string;
              start: string;
            }>;
          }
        ).__slipstreamGridAdmissions.filter(
          (admission) => admission.token === token && admission.start === "60",
        ),
      reopenedToken,
    );
    expect(hiddenTailAdmissions).toEqual([]);

    const visibleTailRequest = page.waitForRequest((request) => {
      const url = new URL(request.url());
      return (
        request.method() === "GET" &&
        url.pathname === `/api/browse/${reopenedToken}` &&
        url.searchParams.get("start") === "60"
      );
    });
    await page.getByRole("button", { name: "Back to Grid" }).click();
    const tailRequest = await visibleTailRequest;
    expect(tailRequest.method()).toBe("GET");
    await expect(page.locator("[data-grid-view]")).toBeVisible();
    await expect(page.locator('[data-photo-index="60"]')).toBeVisible();
  } finally {
    releaseReopen();
    await page.unroute(/\/api\/browse(?:\/|$)/);
    await page.unroute("**/api/photos/*/preview");
  }
});

test("Photo Retry reloads the current aligned range after an expired reopen prefetch fails", async ({
  page,
}, testInfo) => {
  const { base, root } = await fixture();
  await writePhotos(root, 70);
  const running = await server(base, root);
  type RetryTransportEvent = {
    stage:
      | "playwright-browse-request"
      | "playwright-preview-request"
      | "browse-response"
      | "preview-response"
      | "preview-settled";
    at: number;
    limit?: string;
    method?: string;
    milestone?: string;
    photoId?: string;
    priority?: string;
    retryEpoch?: string;
    status?: number;
    start?: string;
    token?: string;
    url: string;
  };
  const retryTransportTrace: RetryTransportEvent[] = [];
  let droppedRetryTransportEvents = 0;
  const recordRetryTransport = (event: RetryTransportEvent) => {
    if (retryTransportTrace.length === 64) {
      retryTransportTrace.shift();
      droppedRetryTransportEvents += 1;
    }
    retryTransportTrace.push(event);
  };
  let retryBrowsePath = "";
  let retryPreviewPath = "";
  let reopenedToken = "";
  let retryStage = "establishing expired-reopen failure";
  let retryTraceArmed = false;
  let expectedRetryEpoch = "";
  let retryCurrentRangeBrowseResponseSeen = false;
  let retryPreviewRequestedBeforeBrowseResponse = false;
  await page.addInitScript(() => {
    const retryEpoch = window as typeof window & {
      __slipstreamRetryHandlerEpoch: number;
    };
    retryEpoch.__slipstreamRetryHandlerEpoch = 0;
    window.addEventListener(
      "click",
      (event) => {
        const target = event.target;
        if (target instanceof Element && target.closest("[data-retry-photo]"))
          retryEpoch.__slipstreamRetryHandlerEpoch += 1;
      },
      true,
    );
    const admissions: Array<{
      token: string;
      start: string;
      limit: string;
      priority: RequestInit["priority"];
      retryEpoch: string;
      url: string;
    }> = [];
    const droppedAdmissions = { count: 0 };
    Object.defineProperty(window, "__slipstreamBrowseAdmissions", {
      value: admissions,
    });
    Object.defineProperty(window, "__slipstreamBrowseAdmissionsDropped", {
      value: droppedAdmissions,
    });
    const nativeFetch = window.fetch.bind(window);
    window.fetch = ((input, init) => {
      if (typeof input === "string") {
        const url = new URL(input, window.location.href);
        const browse = url.pathname.startsWith("/api/browse/");
        const preview =
          url.pathname.startsWith("/api/photos/") &&
          url.pathname.endsWith("/preview");
        if ((browse || preview) && !init?.method) {
          if (browse) {
            if (admissions.length === 64) {
              admissions.shift();
              droppedAdmissions.count += 1;
            }
            admissions.push({
              token: url.pathname.split("/").at(-1) ?? "",
              start: url.searchParams.get("start") ?? "",
              limit: url.searchParams.get("limit") ?? "",
              priority: init?.priority,
              retryEpoch: String(retryEpoch.__slipstreamRetryHandlerEpoch),
              url: `${url.pathname}${url.search}`,
            });
          }
          const headers = new Headers(init?.headers);
          headers.set(
            "x-slipstream-test-priority",
            init?.priority ?? "unspecified",
          );
          headers.set(
            "x-slipstream-test-retry-epoch",
            String(retryEpoch.__slipstreamRetryHandlerEpoch),
          );
          return nativeFetch(input, { ...init, headers });
        }
      }
      return nativeFetch(input, init);
    }) as typeof window.fetch;
  });
  page.on("request", (request) => {
    if (
      !retryTraceArmed ||
      request.method() !== "GET" ||
      request.headers()["x-slipstream-test-independent-probe"] === "true"
    )
      return;
    const url = new URL(request.url());
    const priority = request.headers()["x-slipstream-test-priority"];
    const retryEpoch = request.headers()["x-slipstream-test-retry-epoch"];
    if (
      url.pathname.startsWith("/api/photos/") &&
      url.pathname.endsWith("/preview") &&
      `${url.pathname}${url.search}` === retryPreviewPath &&
      priority === "high" &&
      retryEpoch === expectedRetryEpoch &&
      !retryCurrentRangeBrowseResponseSeen
    )
      retryPreviewRequestedBeforeBrowseResponse = true;
    if (url.pathname.startsWith("/api/browse/")) {
      recordRetryTransport({
        stage: "playwright-browse-request",
        at: Date.now(),
        limit: url.searchParams.get("limit") ?? "",
        method: request.method(),
        ...(priority !== undefined ? { priority } : {}),
        ...(retryEpoch !== undefined ? { retryEpoch } : {}),
        start: url.searchParams.get("start") ?? "",
        token: url.pathname.split("/").at(-1) ?? "",
        url: `${url.pathname}${url.search}`,
      });
    } else if (
      url.pathname.startsWith("/api/photos/") &&
      url.pathname.endsWith("/preview")
    ) {
      recordRetryTransport({
        stage: "playwright-preview-request",
        at: Date.now(),
        method: request.method(),
        photoId: url.pathname.split("/").at(-2) ?? "",
        ...(priority !== undefined ? { priority } : {}),
        ...(retryEpoch !== undefined ? { retryEpoch } : {}),
        url: `${url.pathname}${url.search}`,
      });
    }
  });
  page.on("response", (response) => {
    const request = response.request();
    if (
      !retryTraceArmed ||
      request.method() !== "GET" ||
      request.headers()["x-slipstream-test-independent-probe"] === "true"
    )
      return;
    const url = new URL(response.url());
    const priority = request.headers()["x-slipstream-test-priority"];
    const retryEpoch = request.headers()["x-slipstream-test-retry-epoch"];
    if (
      url.pathname.startsWith("/api/browse/") &&
      `${url.pathname}${url.search}` === retryBrowsePath &&
      url.searchParams.get("start") === "0" &&
      priority === "high" &&
      retryEpoch === expectedRetryEpoch &&
      response.status() === 200
    )
      retryCurrentRangeBrowseResponseSeen = true;
    if (url.pathname.startsWith("/api/browse/")) {
      recordRetryTransport({
        stage: "browse-response",
        at: Date.now(),
        limit: url.searchParams.get("limit") ?? "",
        method: request.method(),
        ...(priority !== undefined ? { priority } : {}),
        ...(retryEpoch !== undefined ? { retryEpoch } : {}),
        start: url.searchParams.get("start") ?? "",
        status: response.status(),
        token: url.pathname.split("/").at(-1) ?? "",
        url: `${url.pathname}${url.search}`,
      });
    } else if (
      url.pathname.startsWith("/api/photos/") &&
      url.pathname.endsWith("/preview")
    ) {
      recordRetryTransport({
        stage: "preview-response",
        at: Date.now(),
        method: request.method(),
        photoId: url.pathname.split("/").at(-2) ?? "",
        ...(priority !== undefined ? { priority } : {}),
        ...(retryEpoch !== undefined ? { retryEpoch } : {}),
        status: response.status(),
        url: `${url.pathname}${url.search}`,
      });
    }
  });
  let overviewRequests = 0;
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (request.method() === "GET" && url.pathname === "/api/overview")
      overviewRequests += 1;
  });
  await openGrid(page, running.url, "All Photos");
  await expect.poll(() => overviewRequests).toBeGreaterThan(0);
  const initialOverviewRequests = overviewRequests;

  const viewport = page.locator("[data-grid-viewport]");
  await viewport.evaluate((element) => {
    Object.defineProperty(element, "clientWidth", {
      configurable: true,
      value: 900,
    });
    Object.defineProperty(element, "clientHeight", {
      configurable: true,
      value: 900,
    });
    window.dispatchEvent(new Event("resize"));
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await viewport.evaluate((element) => {
    element.scrollTop = 3 * 178;
    element.dispatchEvent(new Event("scroll"));
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await expect(page.locator('[data-photo-index="59"]')).toHaveCount(1);
  await expect(page.locator('[data-photo-index="60"]')).toHaveCount(0);

  let releaseAdjacent: () => void = () => undefined;
  const adjacentGate = new Promise<void>((resolve) => {
    releaseAdjacent = resolve;
  });
  let settleAdjacent: () => void = () => undefined;
  const adjacentSettled = new Promise<void>((resolve) => {
    settleAdjacent = resolve;
  });
  let releaseIndependentHighProbe: () => void = () => undefined;
  const independentHighProbeGate = new Promise<void>((resolve) => {
    releaseIndependentHighProbe = resolve;
  });
  let observeIndependentHighProbe: () => void = () => undefined;
  const independentHighProbeObserved = new Promise<void>((resolve) => {
    observeIndependentHighProbe = resolve;
  });
  let settleIndependentHighProbe: () => void = () => undefined;
  const independentHighProbeSettled = new Promise<void>((resolve) => {
    settleIndependentHighProbe = resolve;
  });
  let independentHighProbeHeld = false;
  let releaseAdjacentSuccessor: () => void = () => undefined;
  const adjacentSuccessorGate = new Promise<void>((resolve) => {
    releaseAdjacentSuccessor = resolve;
  });
  let holdAdjacentSuccessors = false;
  let boundaryRequests = 0;
  let originalToken = "";
  let reopenWindowFailed = false;
  let browseAllocations = 0;
  let reopenPhotoId = "";
  const successfulBrowseStarts = new Map<string, Set<string>>();
  const attachRetryStageTrace = async (error: unknown) => {
    const retryEvents = retryTraceArmed ? retryTransportTrace.slice() : [];
    const browserFetchTrace = retryTraceArmed
      ? await page
          .evaluate(() => {
            const state = window as typeof window & {
              __slipstreamBrowseAdmissions?: Array<{
                limit: string;
                priority: RequestInit["priority"];
                retryEpoch: string;
                start: string;
                token: string;
                url: string;
              }>;
              __slipstreamBrowseAdmissionsDropped?: { count: number };
            };
            return {
              dropped: state.__slipstreamBrowseAdmissionsDropped?.count ?? 0,
              events:
                state.__slipstreamBrowseAdmissions?.map((admission) => ({
                  stage: "browser-fetch-admission",
                  limit: admission.limit,
                  priority: admission.priority,
                  retryEpoch: admission.retryEpoch,
                  start: admission.start,
                  token: admission.token,
                  url: admission.url,
                })) ?? [],
            };
          })
          .catch(() => ({ dropped: 0, events: [] }))
      : { dropped: 0, events: [] };
    const body = JSON.stringify(
      {
        error: error instanceof Error ? error.message : String(error),
        expected: {
          browse: reopenedToken
            ? `/api/browse/${reopenedToken}?start=0&limit=60`
            : undefined,
          preview: reopenPhotoId
            ? `/api/photos/${reopenPhotoId}/preview`
            : undefined,
        },
        phase: retryStage,
        traceArmed: retryTraceArmed,
        droppedBrowserFetchAdmissions: browserFetchTrace.dropped,
        droppedRetryTransportEvents,
        stages: {
          browserFetchAdmission: browserFetchTrace.events,
          playwrightBrowseRequest: retryEvents.filter(
            (event) => event.stage === "playwright-browse-request",
          ),
          browseResponse: retryEvents.filter(
            (event) => event.stage === "browse-response",
          ),
          playwrightPreviewRequest: retryEvents.filter(
            (event) => event.stage === "playwright-preview-request",
          ),
          preview: retryEvents.filter(
            (event) =>
              event.stage === "preview-response" ||
              event.stage === "preview-settled",
          ),
        },
      },
      null,
      2,
    );
    const path = testInfo.outputPath("photo-retry-stage-trace.json");
    await writeFile(path, body);
    await testInfo.attach("photo-retry-stage-trace.json", {
      path,
      contentType: "application/json",
    });
  };
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (request.method() === "POST" && url.pathname === "/api/browse") {
      browseAllocations += 1;
      const body = request.postDataJSON() as { photoId?: string };
      reopenPhotoId = body.photoId ?? "";
    }
  });
  page.on("response", (response) => {
    const request = response.request();
    const url = new URL(response.url());
    if (
      request.method() !== "GET" ||
      !url.pathname.startsWith("/api/browse/") ||
      response.status() !== 200 ||
      request.headers()["x-slipstream-test-independent-probe"] === "true"
    )
      return;
    const token = url.pathname.split("/").at(-1)!;
    const starts = successfulBrowseStarts.get(token) ?? new Set<string>();
    starts.add(url.searchParams.get("start") ?? "");
    successfulBrowseStarts.set(token, starts);
  });
  const retryBoundaryUrl = (url: URL) =>
    url.pathname.startsWith("/api/browse/") &&
    url.searchParams.get("start") === "10";
  // Keep Retry pending until the disabled-control assertions have observed it.
  // A fast response must not race these assertions on a slower browser.
  let releaseCurrentRange: () => void = () => undefined;
  const currentRangeGate = new Promise<void>((resolve) => {
    releaseCurrentRange = resolve;
  });
  const currentRangeUrl = (url: URL) =>
    url.pathname.startsWith("/api/browse/") &&
    url.searchParams.get("start") === "0";
  const currentRangeRoute = async (route: Route) => {
    if (
      expectedRetryEpoch &&
      route.request().headers()["x-slipstream-test-retry-epoch"] ===
        expectedRetryEpoch
    )
      await currentRangeGate;
    await route.continue();
  };
  const retryBoundaryRoute = async (route: Route) => {
    const token = new URL(route.request().url()).pathname.split("/").at(-1)!;
    if (
      route.request().headers()["x-slipstream-test-independent-probe"] ===
      "true"
    ) {
      independentHighProbeHeld = true;
      observeIndependentHighProbe();
      try {
        await independentHighProbeGate;
        try {
          await route.fulfill({ status: 200, body: "{}" });
        } catch {
          /* Test teardown may have canceled the held raw probe. */
        }
      } finally {
        settleIndependentHighProbe();
      }
      return;
    }
    boundaryRequests += 1;
    if (boundaryRequests === 1) {
      expect(route.request().headers()["x-slipstream-test-priority"]).toBe(
        "low",
      );
      try {
        await adjacentGate;
        try {
          await route.continue();
        } catch {
          /* current navigation supersedes this adjacent Photo prefetch */
        }
      } finally {
        settleAdjacent();
      }
      return;
    }
    if (!originalToken) {
      originalToken = token;
      await route.fulfill({
        status: 404,
        contentType: "application/json",
        body: '{"error":"Browse source expired or not found"}',
      });
      return;
    }
    if (token !== originalToken && !reopenWindowFailed) {
      reopenedToken = token;
      reopenWindowFailed = true;
      // Close the gate before exposing the failed response so no successor
      // request can pass between the failure UI and Retry admission.
      holdAdjacentSuccessors = true;
      await route.fulfill({ status: 503, body: '{"error":"failed"}' });
      return;
    }
    if (
      token === reopenedToken &&
      holdAdjacentSuccessors &&
      route.request().headers()["x-slipstream-test-priority"] === "low"
    ) {
      await adjacentSuccessorGate;
      try {
        await route.continue();
      } catch {
        /* Retry supersedes this held adjacent Photo prefetch. */
      }
      return;
    }
    await route.continue();
  };
  await page.route(retryBoundaryUrl, retryBoundaryRoute);
  await page.route(currentRangeUrl, currentRangeRoute);
  try {
    // This fixture mounts Photo 60 with synthetic Grid dimensions. A pointer
    // click scrolls it into the real viewport and admits an unrelated Grid
    // window before Photo View opens. Dispatch only the opening action so the
    // held boundary request belongs to the adjacent Photo prefetch under test.
    await page.locator('[data-photo-index="59"]').dispatchEvent("click");
    await expect(page.getByText("60 / 70")).toBeVisible();
    await expect.poll(() => boundaryRequests).toBe(1);

    // The first request is the held adjacent prefetch. Navigating across the
    // boundary promotes that work into a new Photo-owned GET; its 404 reopens
    // around Photo 60. The new token's source-owned first window succeeds,
    // then the Photo-owned adjacent tail prefetch fails.
    await page.getByRole("button", { name: "Next" }).click();
    await expect.poll(() => reopenWindowFailed).toBe(true);
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
    const photoRetry = page.locator("[data-retry-photo]");
    await expect(photoRetry).toBeVisible();
    await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();
    expect(reopenedToken).not.toBe("");
    expect(reopenedToken).not.toBe(originalToken);
    expect(successfulBrowseStarts.get(reopenedToken)).toContain("0");
    expect(browseAllocations).toBe(1);
    expect(overviewRequests).toBe(initialOverviewRequests);
    expect(reopenPhotoId).not.toBe("");

    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();
    // The original-token adjacent request has already been superseded by the
    // reopen. Release its stale route now; the successor gate remains held so
    // the retry still proves Preview completion before adjacent work settles.
    releaseAdjacent();
    await adjacentSettled;
    await expect(photoRetry).toBeEnabled();
    const allocationsBeforeRetry = browseAllocations;
    const overviewsBeforeRetry = overviewRequests;
    retryStage = "awaiting current-range admission";
    retryBrowsePath = `/api/browse/${reopenedToken}?start=0&limit=60`;
    retryPreviewPath = `/api/photos/${reopenPhotoId}/preview`;
    retryTransportTrace.length = 0;
    droppedRetryTransportEvents = 0;
    retryCurrentRangeBrowseResponseSeen = false;
    retryPreviewRequestedBeforeBrowseResponse = false;
    const retryEpoch = await page.evaluate(
      () =>
        (
          window as typeof window & {
            __slipstreamRetryHandlerEpoch: number;
          }
        ).__slipstreamRetryHandlerEpoch + 1,
    );
    expectedRetryEpoch = String(retryEpoch);
    retryTraceArmed = true;
    const retriedHighPriorityBrowse = page.waitForRequest((request) => {
      const url = new URL(request.url());
      return (
        request.method() === "GET" &&
        `${url.pathname}${url.search}` === retryBrowsePath &&
        request.headers()["x-slipstream-test-priority"] === "high" &&
        request.headers()["x-slipstream-test-retry-epoch"] ===
          String(retryEpoch)
      );
    });
    const broadHighPriorityBrowse = page.waitForRequest((request) => {
      const url = new URL(request.url());
      return (
        request.method() === "GET" &&
        url.pathname.startsWith("/api/browse/") &&
        request.headers()["x-slipstream-test-priority"] === "high"
      );
    });
    retryStage = "proving the Retry click boundary";
    const independentHighPriorityBrowse = page.waitForRequest(
      (request) =>
        request.headers()["x-slipstream-test-independent-probe"] === "true",
    );
    await page.evaluate((path) => {
      void fetch(path, {
        priority: "high",
        headers: { "x-slipstream-test-independent-probe": "true" },
      }).catch(() => undefined);
    }, `/api/browse/${reopenedToken}?start=10&limit=60`);
    await independentHighProbeObserved;
    const independentHighRequest = await independentHighPriorityBrowse;
    const broadHighRequest = await broadHighPriorityBrowse;
    expect(broadHighRequest.url()).toBe(independentHighRequest.url());
    expect(broadHighRequest.method()).toBe("GET");
    expect(
      broadHighRequest.headers()["x-slipstream-test-independent-probe"],
    ).toBe("true");
    expect(broadHighRequest.headers()["x-slipstream-test-retry-epoch"]).toBe(
      "0",
    );
    expect(independentHighRequest.headers()["x-slipstream-test-priority"]).toBe(
      "high",
    );
    expect(
      independentHighRequest.headers()["x-slipstream-test-retry-epoch"],
    ).toBe("0");
    const independentHighUrl = new URL(independentHighRequest.url());
    expect(independentHighUrl.pathname).toBe(`/api/browse/${reopenedToken}`);
    expect(independentHighUrl.searchParams.get("start")).toBe("10");
    expect(independentHighUrl.searchParams.get("limit")).toBe("60");
    await page.evaluate(() => {
      const state = window as typeof window & {
        __slipstreamBrowseAdmissions: unknown[];
        __slipstreamBrowseAdmissionsDropped: { count: number };
      };
      state.__slipstreamBrowseAdmissions.length = 0;
      state.__slipstreamBrowseAdmissionsDropped.count = 0;
    });
    retryStage = "awaiting current-range admission";
    const refreshedPreview = page.waitForResponse((response) => {
      const request = response.request();
      const url = new URL(response.url());
      return (
        request.method() === "GET" &&
        url.pathname === `/api/photos/${reopenPhotoId}/preview` &&
        url.search === "" &&
        response.status() === 200
      );
    });
    await photoRetry.click();
    await expect(photoRetry).toBeDisabled();
    await expect(page.getByRole("button", { name: "Next" })).toBeDisabled();
    const retriedPhotoRangeRequest = await retriedHighPriorityBrowse;
    releaseCurrentRange();
    releaseIndependentHighProbe();
    await independentHighProbeSettled;

    retryStage = "awaiting refreshed Preview response";
    expect(retriedPhotoRangeRequest.method()).toBe("GET");
    expect(
      retriedPhotoRangeRequest.headers()["x-slipstream-test-priority"],
    ).toBe("high");
    expect(
      retriedPhotoRangeRequest.headers()["x-slipstream-test-retry-epoch"],
    ).toBe(String(retryEpoch));
    const retriedRequest = new URL(retriedPhotoRangeRequest.url());
    expect(retriedRequest.pathname).toBe(`/api/browse/${reopenedToken}`);
    expect(retriedRequest.searchParams.get("start")).toBe("0");
    expect(retriedRequest.searchParams.get("limit")).toBe("60");
    const retryAdmissions = await page.evaluate(
      (token) =>
        (
          window as typeof window & {
            __slipstreamBrowseAdmissions: Array<{
              token: string;
              start: string;
              limit: string;
              priority: RequestInit["priority"];
              retryEpoch: string;
              url: string;
            }>;
          }
        ).__slipstreamBrowseAdmissions.filter(
          (admission) => admission.token === token,
        ),
      reopenedToken,
    );
    expect(retryAdmissions).toContainEqual({
      token: reopenedToken,
      start: "0",
      limit: "60",
      priority: "high",
      retryEpoch: String(retryEpoch),
      url: retryBrowsePath,
    });
    expect(retryAdmissions).not.toContainEqual({
      token: reopenedToken,
      start: "10",
      limit: "60",
      priority: "high",
      retryEpoch: String(retryEpoch),
      url: `/api/browse/${reopenedToken}?start=10&limit=60`,
    });
    await refreshedPreview;
    expect(retryCurrentRangeBrowseResponseSeen).toBe(true);
    expect(retryPreviewRequestedBeforeBrowseResponse).toBe(false);
    retryStage = "awaiting Retry settlement";
    releaseAdjacentSuccessor();
    await expect(photoRetry).toBeEnabled();
    await expect(page.getByRole("button", { name: "Next" })).toBeEnabled();
    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    await expect(page.locator("[data-status]")).toHaveText(
      "Connected. Current state refreshed.",
    );
    await expect(page.getByText("60 / 70")).toBeVisible();
    await expect(page.getByText("JPEG", { exact: true })).toBeVisible();
    await expect(
      page.getByRole("img", { name: "Photo 60 of 70" }),
    ).toBeVisible();
    recordRetryTransport({
      stage: "preview-settled",
      at: Date.now(),
      milestone: "Connected. Current state refreshed.",
      url: retryPreviewPath,
    });
    const completedAdmissions = await page.evaluate(
      (token) =>
        (
          window as typeof window & {
            __slipstreamBrowseAdmissions: Array<{
              token: string;
              start: string;
              limit: string;
              priority: RequestInit["priority"];
              retryEpoch: string;
              url: string;
            }>;
          }
        ).__slipstreamBrowseAdmissions.filter(
          (admission) => admission.token === token,
        ),
      reopenedToken,
    );
    expect(completedAdmissions).toContainEqual({
      token: reopenedToken,
      start: "10",
      limit: "60",
      priority: "low",
      retryEpoch: String(retryEpoch),
      url: `/api/browse/${reopenedToken}?start=10&limit=60`,
    });
    expect(completedAdmissions).not.toContainEqual({
      token: reopenedToken,
      start: "10",
      limit: "60",
      priority: "high",
      retryEpoch: String(retryEpoch),
      url: `/api/browse/${reopenedToken}?start=10&limit=60`,
    });
    expect(browseAllocations).toBe(allocationsBeforeRetry);
    expect(overviewRequests).toBe(overviewsBeforeRetry);
    retryStage = "complete";
  } catch (error) {
    try {
      await attachRetryStageTrace(error);
    } catch {
      // Preserve the original workflow failure if diagnostics cannot be written.
    }
    throw error;
  } finally {
    releaseAdjacent();
    releaseIndependentHighProbe();
    if (independentHighProbeHeld) await independentHighProbeSettled;
    releaseAdjacentSuccessor();
    releaseCurrentRange();
    await page.unroute(currentRangeUrl, currentRangeRoute);
    await page.unroute(retryBoundaryUrl, retryBoundaryRoute);
  }
});

test("navigation promotes an aborted adjacent window to current priority", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 70);
  const running = await server(base, root);
  await page.setViewportSize({ width: 1200, height: 1100 });
  await openGrid(page, running.url, "All Photos");
  await page.locator("[data-grid-viewport]").evaluate((viewport) => {
    Object.defineProperty(viewport, "clientWidth", {
      configurable: true,
      value: 900,
    });
    Object.defineProperty(viewport, "clientHeight", {
      configurable: true,
      value: 900,
    });
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await page.locator("[data-grid-viewport]").evaluate((viewport) => {
    viewport.scrollTop = 3 * 178;
    viewport.dispatchEvent(new Event("scroll"));
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await expect(page.locator('[data-photo-index="59"]')).toHaveCount(1);
  await expect(page.locator('[data-photo-index="60"]')).toHaveCount(0);

  let releaseFirst!: () => void;
  const firstGate = new Promise<void>((resolve) => {
    releaseFirst = resolve;
  });
  let boundaryRequests = 0;
  await page.route(
    (url) =>
      url.pathname.startsWith("/api/browse/") &&
      url.searchParams.get("start") === "10",
    async (route) => {
      boundaryRequests += 1;
      if (boundaryRequests === 1) {
        await firstGate;
        try {
          await route.continue();
        } catch {
          /* current navigation aborts the adjacent request */
        }
        return;
      }
      await route.continue();
    },
  );
  try {
    await page.locator('[data-photo-index="59"]').evaluate((cell) => {
      (cell as HTMLButtonElement).click();
    });
    await expect(page.getByText("60 / 70")).toBeVisible();
    await expect.poll(() => boundaryRequests).toBe(1);

    await page.getByRole("button", { name: "Next" }).click();
    await expect.poll(() => boundaryRequests).toBe(2);
    await expect(page.getByText("61 / 70")).toBeVisible();
  } finally {
    releaseFirst();
  }
});

test("stale opaque Photo windows cannot claim Recovery after Back to Grid", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (let index = 0; index < 70; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(3, "0")}.jpg`),
      await jpeg(),
    );
  const running = await server(base, root);
  await openGrid(page, running.url, "All Photos");
  let releaseBoundary: () => void = () => undefined;
  const boundaryGate = new Promise<void>((resolve) => {
    releaseBoundary = resolve;
  });
  let boundaryRequests = 0;
  let staleBoundaryRequests = 0;
  await page.route(
    (url) =>
      url.pathname.startsWith("/api/browse/") &&
      url.searchParams.get("start") === "10",
    async (route) => {
      const requestNumber = (boundaryRequests += 1);
      await boundaryGate;
      try {
        if (requestNumber <= staleBoundaryRequests)
          await route.fulfill({ status: 503, body: '{"error":"stale"}' });
        else await route.continue();
      } catch {
        /* Back to Grid may abort transport before the stale response lands. */
      }
    },
  );
  const viewport = page.locator("[data-grid-viewport]");
  await viewport.evaluate((element) => {
    Object.defineProperty(element, "clientWidth", {
      configurable: true,
      value: 320,
    });
    window.dispatchEvent(new Event("resize"));
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await viewport.evaluate((element) => {
    element.scrollTop = 29 * 178;
  });
  await page.locator('[data-photo-index="59"]').click();
  await expect(page.getByText("60 / 70")).toBeVisible();
  const currentImage = page.locator("[data-stage] img");
  await expect(currentImage).toBeVisible();
  await page.waitForFunction(() => {
    const image = document.querySelector("[data-stage] img");
    return (
      image instanceof HTMLImageElement &&
      image.complete &&
      image.naturalWidth > 0
    );
  });
  const retainedPreviewSrc = await currentImage.getAttribute("src");
  if (!retainedPreviewSrc) throw new Error("Current Preview src is missing");
  await expect(page.getByRole("button", { name: "Fit Window" })).toBeEnabled();
  await page.keyboard.press("ArrowRight");
  await expect.poll(() => boundaryRequests).toBeGreaterThanOrEqual(2);
  staleBoundaryRequests = boundaryRequests;
  // The boundary Photo waits for its shared facts; Back to Grid must remain
  // available instead of claiming the unavailable Photo is already open.
  await expect(page.getByText("60 / 70")).toBeVisible();
  await expect(currentImage).toHaveAttribute("src", retainedPreviewSrc);
  await expect(page.getByRole("button", { name: "Fit Window" })).toBeEnabled();
  // The boundary window is still loading; Back to Grid must stay available.
  await expect(
    page.getByRole("button", { name: "Back to Grid" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-layer]")).toBeVisible();
  await expect(currentImage).not.toHaveAttribute("src", retainedPreviewSrc);
  // The pending boundary Photo never committed, so returning to Grid keeps
  // the last visible Photo's position instead of adopting the failed target.
  await expect
    .poll(() => viewport.evaluate((element) => element.scrollTop))
    .toBe(29 * 178);
  await waitForGridFrame(page);
  await expect(page.locator('[data-photo-index="59"]')).toBeFocused();
  releaseBoundary();
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Retry connection" }),
  ).toBeHidden();
  // The abandoned open must not wedge the browser: a new Photo opens normally.
  await page.locator('[data-photo-index="57"]').click();
  await expect(page.getByText("58 / 70")).toBeVisible();
  await expect(page.getByRole("button", { name: /^Select/ })).toBeEnabled();
  expect(boundaryRequests).toBeGreaterThanOrEqual(1);
});

test("file locations stay bounded on a 40,000-Photo Library with a large folder hierarchy", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const base = await mkdtemp(join(tmpdir(), "slipstream-browser-40k-folders-"));
  temporary.push(base);
  const root = join(base, "originals");
  await mkdir(root);
  await mkdir(join(base, "state"));
  await mkdir(join(base, "cache"));
  await chmod(join(base, "state"), 0o700);
  // Canonical v5 state with 40,000 direct root Folders x 1 Photo each.
  const generator = `
    const { Database } = await import("bun:sqlite");
    const database = new Database(process.env.STATE_DB);
    database.exec(await Bun.file(process.env.SCHEMA_PATH).text());
    const insertOriginal = database.prepare(
      "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available) VALUES(?1,?2,'jpeg',1,1.0,1)",
    );
    const insertPhoto = database.prepare(
      "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,sort_path) VALUES(?1,?2,0,1,'inspection-pending',?3)",
    );
    const insertBinding = database.prepare(
      "INSERT INTO library_metadata VALUES('canonical_root',?1)",
    );
    database.exec("BEGIN");
    for (let index = 0; index < 40000; index += 1) {
      const path = "f" + String(index).padStart(5, "0") + "/one.jpg";
      const originalId = index.toString(16).padStart(8, "0").repeat(8);
      const photoId = (0x100000 + index).toString(16).padStart(8, "0").repeat(8);
      insertOriginal.run(originalId, path);
      insertPhoto.run(photoId, originalId, path);
    }
    database.exec("COMMIT");
    insertBinding.run(process.env.ROOT);
    database.close();
  `;
  execFileSync("bun", ["-e", generator], {
    env: {
      ...process.env,
      STATE_DB: join(base, "state", "library.sqlite"),
      ROOT: root,
      SCHEMA_PATH: join(process.cwd(), "compatibility/sqlite/schema-v5.sql"),
    },
    stdio: "inherit",
  });

  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();

  // Expanding the root loads exactly one enforced direct-child page out of
  // 40,000 with explicit pager controls.
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  const folderCards = page.locator(".source-panel .source-card");
  await expect(folderCards).toHaveCount(62, { timeout: 10_000 });
  await expect(page.getByText("1 / 667")).toBeVisible();
  // Paging to a late position replaces the retained page: the DOM stays at
  // one window no matter how deep the navigation reaches.
  for (let page_index = 2; page_index <= 6; page_index += 1) {
    await page.getByRole("button", { name: "More Folders" }).click();
    await expect(page.getByText(`${page_index} / 667`)).toBeVisible();
  }
  expect(await page.locator(".folder-row .source-card").count()).toBe(61);
  await page.getByRole("button", { name: "Previous Folders" }).click();
  await expect(page.getByText("5 / 667")).toBeVisible();
  expect(await page.locator(".folder-row .source-card").count()).toBe(61);

  // Opening a Folder from the current page stays bounded end to end.
  await page.locator(".folder-child .source-card").first().click();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();

  const metrics = await page.evaluate(() => ({
    domCount: document.querySelectorAll("*").length,
    cellCount: document.querySelectorAll(".photo-cell").length,
    folderButtons: document.querySelectorAll(".folder-row").length,
  }));
  expect(metrics.cellCount).toBeLessThan(120);
  expect(metrics.folderButtons).toBe(61);
  expect(metrics.domCount).toBeLessThan(3_000);
});

test("a persisted 40,000-Photo Library is served from persisted state and stays browsable across the startup rescan", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const base = await mkdtemp(join(tmpdir(), "slipstream-browser-40k-"));
  temporary.push(base);
  const root = join(base, "originals");
  await mkdir(root);
  await mkdir(join(base, "state"));
  await mkdir(join(base, "cache"));
  await chmod(join(base, "state"), 0o700);
  // The test process runs under Playwright's Node loader, so the canonical
  // v4 state is generated through Bun's SQLite in a child process.
  const generator = `
    const { Database } = await import("bun:sqlite");
    const database = new Database(process.env.STATE_DB);
    database.exec(await Bun.file(process.env.SCHEMA_PATH).text());
    const insertOriginal = database.prepare(
      "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available) VALUES(?1,?2,'jpeg',1,1.0,1)",
    );
    const insertPhoto = database.prepare(
      "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,sort_path) VALUES(?1,?2,0,1,'inspection-pending',?3)",
    );
    const insertBinding = database.prepare(
      "INSERT INTO library_metadata VALUES('canonical_root',?1)",
    );
    database.exec("BEGIN");
    for (let index = 0; index < 40000; index += 1) {
      const path = String(index).padStart(6, "0") + ".jpg";
      const originalId = index.toString(16).padStart(8, "0").repeat(8);
      const photoId = (0x100000 + index).toString(16).padStart(8, "0").repeat(8);
      insertOriginal.run(originalId, path);
      insertPhoto.run(photoId, originalId, path);
    }
    database.exec("COMMIT");
    insertBinding.run(process.env.ROOT);
    database.close();
  `;
  execFileSync("bun", ["-e", generator], {
    env: {
      ...process.env,
      STATE_DB: join(base, "state", "library.sqlite"),
      ROOT: root,
      SCHEMA_PATH: join(process.cwd(), "compatibility/sqlite/schema-v4.sql"),
    },
    stdio: "inherit",
  });

  const running = await server(base, root);
  const gridMetrics = () =>
    page.evaluate(() => {
      const viewport = document.querySelector<HTMLElement>(
        "[data-grid-viewport]",
      );
      if (!viewport) throw new Error("grid viewport is missing");
      return {
        viewportHeight: viewport.clientHeight,
        scrollHeight: viewport.scrollHeight,
        innerHeight: window.innerHeight,
        cellCount: document.querySelectorAll(".photo-cell").length,
        domCount: document.querySelectorAll("*").length,
      };
    });
  const expectBoundedGrid = async () => {
    const metrics = await gridMetrics();
    expect(metrics.viewportHeight).toBeGreaterThan(0);
    expect(metrics.viewportHeight).toBeLessThanOrEqual(metrics.innerHeight);
    expect(metrics.scrollHeight).toBeGreaterThan(metrics.viewportHeight);
    expect(metrics.cellCount).toBeGreaterThan(0);
    expect(metrics.cellCount).toBeLessThan(200);
    expect(metrics.domCount).toBeLessThan(2_000);
    return metrics;
  };

  // The published Library is served immediately from persisted state; the
  // overview stays bounded instead of transferring 40,000 Photo facts.
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto(running.url);
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "All Photos 40,000 Photos" }),
  ).toBeVisible();
  const overviewBytes = await page.evaluate(
    async () => (await (await fetch("/api/overview")).text()).length,
  );
  expect(overviewBytes).toBeLessThan(20_000);
  const initialGrid = await expectBoundedGrid();

  await page.setViewportSize({ width: 1280, height: 1200 });
  await expect
    .poll(async () => (await gridMetrics()).cellCount)
    .toBeGreaterThan(initialGrid.cellCount);
  const tallGrid = await expectBoundedGrid();

  await page.setViewportSize({ width: 1280, height: 720 });
  await expect
    .poll(async () => (await gridMetrics()).cellCount)
    .toBeLessThan(tallGrid.cellCount);
  await expectBoundedGrid();

  const firstCell = page.locator('[data-photo-index="0"]');
  await expect(firstCell).toBeVisible();
  await expect(firstCell).toBeEnabled();
  await firstCell.click();
  await expect(page.getByText("1 / 40000")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();

  const viewport = page.locator("[data-grid-viewport]");
  await viewport.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
    element.dispatchEvent(new Event("scroll"));
  });
  const lastCell = page.locator('[data-photo-index="39999"]');
  await expect(lastCell).toBeVisible({ timeout: 30_000 });
  await expect(lastCell).toBeEnabled();
  await expectBoundedGrid();
  await lastCell.click();
  await expect(page.getByText("40000 / 40000")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();

  // Reducing the column count must restore the current late Photo after the
  // virtual canvas grows for the narrower layout.
  await page.setViewportSize({ width: 1100, height: 720 });
  await expect(lastCell).toBeVisible();
  await expectBoundedGrid();

  // Re-check the original mobile viewport contract after exercising the
  // desktop late-window path.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();
  await expectBoundedGrid();

  // The owned startup rescan settles without emptying or reordering the
  // source; late-window protocol browsing stays bounded afterward.
  await expect
    .poll(
      async () => {
        const response = await fetch(`${running.url}/api/status`);
        return ((await response.json()) as { state: string }).state;
      },
      { timeout: 120_000 },
    )
    .toBe("idle");
  const lateWindow: { total: number; photos: unknown[] } = await page.evaluate(
    async () => {
      const opened = (await (
        await fetch("/api/browse", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ source: "library" }),
        })
      ).json()) as { token: string };
      const window = (await (
        await fetch(`/api/browse/${opened.token}?start=39940&limit=60`)
      ).json()) as { total: number; photos: unknown[] };
      return window;
    },
  );
  expect(lateWindow.total).toBe(40_000);
  expect(lateWindow.photos).toHaveLength(60);
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();
});
