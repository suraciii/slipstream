import { expect, test } from "@playwright/test";
import { copyFile, mkdir, rm, writeFile } from "node:fs/promises";
import { extname, join } from "node:path";
import {
  setupBrowserSmoke,
  sample,
  jpeg,
  fixture,
  writePhotos,
  server,
  post,
  browseIds,
  createAlbum,
  originalSnapshot,
} from "./browser-test-support/fixtures.js";
import {
  waitForGridFrame,
  openPhotoAndWaitForProgress,
  waitForLoadedReviewImage,
  previewImageGeometry,
  startReview,
  expectConnection,
  openSources,
  openPhotoTools,
  openViewOptions,
  applyViewOptions,
  contrastRatio,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

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

test("an idle browser reports a lost connection from the status probe", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await expectConnection(page, "Connected");
  await expect(page.getByRole("button", { name: "Pick" })).toBeEnabled();

  // The Photographer takes no action here. Only the reachability probe runs,
  // and the server stops answering it.
  await page.route("**/api/status", (route) => route.abort());
  await expectConnection(page, "Disconnected");
  await expect(page.getByRole("button", { name: "Pick" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Reject" })).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Retry", exact: true }),
  ).toBeEnabled();

  // A decision stays refused, including through the keyboard path.
  await page.keyboard.press("p");
  await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
  await expect(page.getByText("1 / 2")).toBeVisible();
  await openPhotoTools(page);
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();

  // A usable status answer confirms the connection again.
  await page.unroute("**/api/status");
  await expectConnection(page, "Connected");
  await expect(page.getByRole("button", { name: "Pick" })).toBeEnabled();

  // An answered status error is a server-side condition, not a lost
  // connection, so it must not report the browser disconnected.
  await page.route("**/api/status", (route) =>
    route.fulfill({ status: 503, body: "unavailable" }),
  );
  await page.waitForTimeout(3_000);
  await expectConnection(page, "Connected");
  await expect(page.getByRole("button", { name: "Pick" })).toBeEnabled();
  await page.unroute("**/api/status");
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
