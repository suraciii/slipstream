import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
  chmod,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";

import { expect, test, type Locator, type Page } from "@playwright/test";

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
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}
type BrowsePhoto = {
  id: string;
  available: boolean;
  selectionState: string;
  rating: number;
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
  await fetch(`${url}/api/browse/${opened.token}`, { method: "DELETE" });
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
  await fetch(`${url}/api/browse/${opened.token}`, { method: "DELETE" });
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
  await page.getByText(/^Ready · \d[\d,]* Photos?$/).waitFor();
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

test("exposes one main landmark while loading and after rendering", async ({
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
    await expect(page.getByText("Loading Library summary…")).toBeVisible();
    await expect(htmlMain).toHaveCount(1);
    await expect(mainLandmark).toHaveCount(1);
    await expect(mainLandmark).toHaveAttribute("id", "app");

    releaseOverview();
    await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
    await expect(htmlMain).toHaveCount(1);
    await expect(mainLandmark).toHaveCount(1);
    await expect(mainLandmark).toHaveAttribute("id", "app");
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
    "Detail Review",
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
        ".membership-controls",
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
      page.getByLabel("Album", { exact: true }),
      page.getByRole("button", { name: "Add to Album" }),
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

    await page
      .getByLabel("Album", { exact: true })
      .selectOption({ label: "Destination" });
    const membershipSaved = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname.endsWith("/members") &&
        response.status() === 200,
    );
    await page.getByRole("button", { name: "Add to Album" }).click();
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

    await page.getByRole("button", { name: "Detail Review" }).click();
    await expect(preview).toHaveCSS("touch-action", "none");
    await photoView.evaluate((view) => {
      view.scrollTop = 0;
    });
    const image = page.locator("[data-stage] img");
    const before = await image.evaluate((element) => element.style.transform);
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
    await expect(image).toHaveCSS("transform", /matrix\(2, 0, 0, 2, 120, 36\)/);
    expect(await image.evaluate((element) => element.style.transform)).not.toBe(
      before,
    );
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
      `fit Preview at ${viewport.width}x${viewport.height} yields real vertical touch scrolling while horizontal decisions and Detail pan remain owned`,
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

  const zero = page.getByRole("button", { name: "Clear Rating" });
  await expect(
    page.getByRole("button", { name: "Clear Rating", pressed: true }),
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
          ".facts dt, .rating-controls legend, .membership-controls label",
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
  await page.getByRole("button", { name: "Detail Review" }).click();
  await expect(
    page.getByRole("button", { name: "Exit Detail" }),
  ).toHaveAttribute("aria-pressed", "true");
  await swipe(page, 100, 220);
  await expect(page.getByText("1 / 2")).toBeVisible();
  expect((await state(running.url, albumId)).members[0]!.selectionState).toBe(
    "rejected",
  );
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Next" }).click(),
  );
  await expect(
    page.getByRole("button", { name: "Detail Review" }),
  ).toHaveAttribute("aria-pressed", "false");
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
    expect(controls.buttons).toEqual([true, true, true, true]);
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
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.getByRole("heading", { name: "All Photos" })).toBeVisible();

  // Adding the current Photo to an Album updates the bounded counts.
  await page
    .getByLabel("Album", { exact: true })
    .selectOption({ label: "Picks" });
  const firstAdd = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname.endsWith("/members") &&
      response.status() === 200,
  );
  await page.getByRole("button", { name: "Add to Album" }).click();
  await firstAdd;
  await expect(page.getByText("Added to the Album.")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(
    page.getByRole("button", { name: /Picks 1 Photo/ }),
  ).toBeVisible();

  // Adding an existing member is idempotent.
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await page
    .getByLabel("Album", { exact: true })
    .selectOption({ label: "Picks" });
  const idempotentAdd = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname.endsWith("/members") &&
      response.status() === 200,
  );
  await page.getByRole("button", { name: "Add to Album" }).click();
  await idempotentAdd;
  await expect(page.getByText("Added to the Album.")).toBeVisible();
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
  await page.getByRole("button", { name: "Remove from this Album" }).click();
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
    await page.getByRole("button", { name: "Remove from this Album" }).click();
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
  await page
    .getByLabel("Album", { exact: true })
    .selectOption({ label: "Picks" });

  await page.route("**/api/albums/*/members", (route) => route.abort());
  await page.getByRole("button", { name: "Add to Album" }).click();
  await expect(
    page.getByText("The Photo could not be added to the Album."),
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
  await expect(
    page.getByRole("button", { name: "Add to Album" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Add to Album" }).click();
  await retried;

  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
  await expect(page.getByText("Added to the Album.")).toBeVisible();
  await openSources(page);
  await expect(
    page.getByRole("button", { name: /^Picks 1 Photo$/ }),
  ).toBeVisible();
  await expect
    .poll(async () => (await state(running.url, albumId)).members)
    .toHaveLength(1);
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

  let releaseA!: () => void;
  const heldA = new Promise<void>((resolve) => {
    releaseA = resolve;
  });
  await page.route(`**/api/albums/${albumA}/members`, async (route) => {
    await heldA;
    await route.continue();
  });
  const picker = page.getByLabel("Album", { exact: true });
  await picker.selectOption(albumA);
  const requestA = page.waitForResponse((response) =>
    response.url().includes(`/api/albums/${albumA}/members`),
  );
  await page.getByRole("button", { name: "Add to Album" }).click();
  await expect(picker).toBeEnabled();

  await picker.selectOption(albumB);
  await expect(
    page.getByRole("button", { name: "Add to Album" }),
  ).toBeEnabled();
  const requestB = page.waitForResponse((response) =>
    response.url().includes(`/api/albums/${albumB}/members`),
  );
  await page.getByRole("button", { name: "Add to Album" }).click();
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
  await expect(page.getByLabel("Album", { exact: true })).toBeDisabled();

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
  await expect(page.getByLabel("Album", { exact: true })).toBeEnabled();
  await page
    .getByLabel("Album", { exact: true })
    .selectOption({ label: "Fresh" });
  await page.getByRole("button", { name: "Add to Album" }).click();
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
  await page.getByRole("button", { name: "Remove from this Album" }).click();
  await expect(
    page.getByText("The Photo could not be removed from the Album."),
  ).toBeVisible();
  // The failed control is re-enabled, not wedged.
  await expect(
    page.getByRole("button", { name: "Remove from this Album" }),
  ).toBeEnabled();
  await page.unroute("**/api/albums/*/members/remove");
  await page.getByRole("button", { name: "Remove from this Album" }).click();
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
  await page
    .getByLabel("Album", { exact: true })
    .selectOption({ label: "Other" });
  await page.getByRole("button", { name: "Add to Album" }).click();
  await page.getByRole("button", { name: "Remove from this Album" }).click();
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
  await page
    .getByLabel("Album", { exact: true })
    .selectOption({ label: "Other" });
  await page.getByRole("button", { name: "Add to Album" }).click();
  await page.getByRole("button", { name: "Remove from this Album" }).click();
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
  const removeButton = page.locator("[data-remove-from-album]");
  const removalSettled = page.waitForResponse(
    (response) =>
      response.url().includes("/members/remove") &&
      response.request().method() === "POST",
  );
  await removeButton.click();
  await expect(removeButton).toBeDisabled();
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
  await expect(removeButton).toBeDisabled();
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
  // The removed member is no longer removable within the open snapshot.
  await expect(removeButton).toBeHidden();
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
  await page
    .getByLabel("Album", { exact: true })
    .selectOption({ label: "Picks" });

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
  await page.getByRole("button", { name: "Add to Album" }).click();
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
  await expect(
    page.getByRole("heading", { name: "File Locations" }),
  ).toBeVisible();
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
    page.getByText(
      "Scan results changed File Locations. Reloaded the current Folders.",
    ),
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
    page.getByText(/Could not load File Locations \(shoot items 1–60\)/),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Library Folder/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", {
      name: /^Retry File Locations \(shoot items 1–60\)/,
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
    page.getByText(/Could not load File Locations \(shoot items 1–60\)/),
  ).toBeVisible();

  // A lower-priority admitted Album failure settles behind the actionable
  // range owner rather than erasing it. Exact range recovery reveals the
  // pending Album failure.
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Existing");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(
    page.getByText(/Could not load File Locations \(shoot items 1–60\)/),
  ).toBeVisible();
  await expect(
    page.getByText("An Album with this name already exists."),
  ).toBeHidden();

  failing = false;
  await page.getByRole("button", { name: /^Retry File Locations/ }).click();
  // Retrying loads only the failed range: the sibling child appears while
  // the already loaded root navigation stays intact.
  await expect(
    page.getByRole("button", { name: /nested 1 Photo/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /shoot · Subfolders/ }),
  ).toBeVisible();
  await expect(page.getByText(/Could not load File Locations/)).toBeHidden();
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
    name: /^Retry File Locations \(a items 1–60\)/,
  });
  const retryB = page.getByRole("button", {
    name: /^Retry File Locations \(b items 1–60\)/,
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
    page.getByText(
      "Scan results changed File Locations. Reloaded the current Folders.",
    ),
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
  await page.getByRole("button", { name: "Detail Review" }).click();
  const image = page.locator("[data-stage] img");
  const preview = page.locator("[data-preview]");
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
  await expect(image).toHaveCSS("transform", /matrix\(2, 0, 0, 2, 40, 30\)/);
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
  await page.getByRole("button", { name: "Exit Detail" }).click();
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

test("shows matching JPEG then RAW embedded JPEG through the mobile production Review Session", async ({
  page,
}) => {
  test.skip(!sample, "Set SLIPSTREAM_RAW_SAMPLE for the camera smoke");
  const cameraSample = sample!;
  const before = await sha256(cameraSample);
  const { base, root } = await fixture();
  const raw = join(root, `camera${extname(cameraSample)}`);
  const matching = join(root, "camera.jpg");
  await copyFile(cameraSample, raw);
  await writeFile(matching, await jpeg());
  const running = await server(base, root);
  const { albumId } = await createAlbum(running.url);
  await startReview(page, running.url, "Review", albumId);
  await expect(page.getByText("JPEG", { exact: true })).toBeVisible();
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
  await page.getByLabel("Album", { exact: true }).selectOption(albumId);

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
  await page.getByRole("button", { name: "Add to Album" }).click();
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
  await writePhotos(root, 8);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();
  await expect(page.locator(".photo-cell img")).toHaveCount(8);

  const renderCount = await page.evaluate(
    () =>
      new Promise<number>((resolve) => {
        const layer = document.querySelector<HTMLElement>("[data-grid-layer]");
        const viewport = document.querySelector<HTMLElement>(
          "[data-grid-viewport]",
        );
        if (!layer || !viewport) throw new Error("Grid elements are missing");
        const replaceChildren = layer.replaceChildren.bind(layer);
        let count = 0;
        layer.replaceChildren = (...nodes: Node[]) => {
          count += 1;
          replaceChildren(...nodes);
        };
        for (let index = 0; index < 8; index += 1)
          viewport.dispatchEvent(new Event("scroll"));
        requestAnimationFrame(() => resolve(count));
      }),
  );
  expect(renderCount).toBe(1);
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

  const failedCell = await cell.elementHandle();
  expect(failedCell).not.toBeNull();
  await page.locator("[data-grid-viewport]").evaluate((viewport) => {
    viewport.dispatchEvent(new Event("scroll"));
  });
  await expect
    .poll(() => failedCell!.evaluate((node) => node.isConnected))
    .toBe(false);
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

  await page.locator("[data-grid-viewport]").evaluate((viewport) => {
    viewport.dispatchEvent(new Event("scroll"));
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
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
test("an expired Browse snapshot reopens around the current Photo", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (let index = 0; index < 130; index += 1)
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
    page.getByRole("button", { name: /^Photo 1 of 130/ }),
  );
  await expect(page.getByText("1 / 130")).toBeVisible();
  const firstId = (await state(running.url, albumId)).members[0]!.photoId;
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);
  const reopenBodies: Array<Record<string, unknown>> = [];
  let expiredServed = false;
  await page.route(/\/api\/browse/, async (route) => {
    const request = route.request();
    if (request.method() === "POST") {
      reopenBodies.push(request.postDataJSON() as Record<string, unknown>);
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
  await page.locator("[data-grid-viewport]").evaluate((element) => {
    element.scrollTop = element.scrollHeight;
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
  await expect(
    page.getByRole("button", { name: /^Photo 1 of 130/ }),
  ).toBeVisible();
});

test("Photo Retry reloads the current aligned range after an expired reopen prefetch fails", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 70);
  const running = await server(base, root);
  await page.addInitScript(() => {
    const admissions: Array<{
      token: string;
      start: string;
      priority: RequestInit["priority"];
    }> = [];
    Object.defineProperty(window, "__slipstreamBrowseAdmissions", {
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
            priority: init?.priority,
          });
      }
      return nativeFetch(input, init);
    }) as typeof window.fetch;
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
  let releaseAdjacentSuccessor: () => void = () => undefined;
  const adjacentSuccessorGate = new Promise<void>((resolve) => {
    releaseAdjacentSuccessor = resolve;
  });
  let holdAdjacentSuccessors = false;
  let boundaryRequests = 0;
  let originalToken = "";
  let reopenedToken = "";
  let reopenWindowFailed = false;
  let browseAllocations = 0;
  let reopenPhotoId = "";
  const successfulBrowseStarts = new Map<string, Set<string>>();
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
      response.status() !== 200
    )
      return;
    const token = url.pathname.split("/").at(-1)!;
    const starts = successfulBrowseStarts.get(token) ?? new Set<string>();
    starts.add(url.searchParams.get("start") ?? "");
    successfulBrowseStarts.set(token, starts);
  });
  await page.route(
    (url) =>
      url.pathname.startsWith("/api/browse/") &&
      url.searchParams.get("start") === "10",
    async (route) => {
      const token = new URL(route.request().url()).pathname.split("/").at(-1)!;
      boundaryRequests += 1;
      if (boundaryRequests === 1) {
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
      if (token === reopenedToken && holdAdjacentSuccessors) {
        await adjacentSuccessorGate;
        try {
          await route.continue();
        } catch {
          /* Retry supersedes this held adjacent Photo prefetch. */
        }
        return;
      }
      await route.continue();
    },
  );
  try {
    await page.locator('[data-photo-index="59"]').click();
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
    await expect(photoRetry).toBeEnabled();
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
    const allocationsBeforeRetry = browseAllocations;
    const overviewsBeforeRetry = overviewRequests;
    await page.evaluate(() => {
      (
        window as typeof window & {
          __slipstreamBrowseAdmissions: unknown[];
        }
      ).__slipstreamBrowseAdmissions.length = 0;
    });
    const retriedPhotoRange = page.waitForRequest((request) => {
      const url = new URL(request.url());
      return (
        request.method() === "GET" &&
        url.pathname === `/api/browse/${reopenedToken}` &&
        url.searchParams.get("start") === "0" &&
        url.searchParams.get("limit") === "60"
      );
    });
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
    const retriedPhotoRangeRequest = await retriedPhotoRange;

    expect(retriedPhotoRangeRequest.method()).toBe("GET");
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
              priority: RequestInit["priority"];
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
      priority: "high",
    });
    expect(retryAdmissions).not.toContainEqual({
      token: reopenedToken,
      start: "10",
      priority: "high",
    });
    await refreshedPreview;
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
    const completedAdmissions = await page.evaluate(
      (token) =>
        (
          window as typeof window & {
            __slipstreamBrowseAdmissions: Array<{
              token: string;
              start: string;
              priority: RequestInit["priority"];
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
      priority: "low",
    });
    expect(completedAdmissions).not.toContainEqual({
      token: reopenedToken,
      start: "10",
      priority: "high",
    });
    expect(browseAllocations).toBe(allocationsBeforeRetry);
    expect(overviewRequests).toBe(overviewsBeforeRetry);
  } finally {
    releaseAdjacent();
    releaseAdjacentSuccessor();
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
  await page.keyboard.press("ArrowRight");
  await expect.poll(() => boundaryRequests).toBeGreaterThanOrEqual(2);
  staleBoundaryRequests = boundaryRequests;
  // The boundary Photo waits for its shared facts; Back to Grid must remain
  // available instead of claiming the unavailable Photo is already open.
  await expect(page.getByText("60 / 70")).toBeVisible();
  // The boundary window is still loading; Back to Grid must stay available.
  await expect(
    page.getByRole("button", { name: "Back to Grid" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-layer]")).toBeVisible();
  await expect
    .poll(() => viewport.evaluate((element) => element.scrollTop))
    .toBe(30 * 178);
  await expect(viewport).toBeFocused();
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
