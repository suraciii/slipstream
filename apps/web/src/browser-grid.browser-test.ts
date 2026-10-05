import { expect, test } from "@playwright/test";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  setupBrowserSmoke,
  jpeg,
  withExifOrientation,
  jpegWithSize,
  fixture,
  writePhotos,
  server,
  browseIds,
  libraryPhoto,
  post,
} from "./browser-test-support/fixtures.js";
import {
  applyViewOptions,
  openViewOptions,
  waitForGridFrame,
  openPhotoToolsView,
  closePhotoTools,
  gridCellGeometry,
  expectAspectRatio,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

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
    "Source progress: 0 picked · 0 rejected · 4 unflagged",
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
  await expect(cell(0).locator(".cell-state.picked")).toHaveText("✓");
  await expect(progress).toHaveText(
    "Source progress: 1 picked · 0 rejected · 3 unflagged",
  );
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("p");
  await expect(progress).toHaveText(
    "Source progress: 1 picked · 0 rejected · 3 unflagged",
  );

  // A decision change moves one Photo between the counts.
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("x");
  await expect(cell(0).locator(".cell-state.rejected")).toHaveText("×");
  await expect(progress).toHaveText(
    "Source progress: 0 picked · 1 rejected · 3 unflagged",
  );

  // Undo returns that decision and the counts together: the Photo holds its
  // previous value again, which was selected.
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("Control+z");
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Last change undone.",
  );
  await expect(cell(0).locator(".cell-state.picked")).toHaveText("✓");
  await expect(progress).toHaveText(
    "Source progress: 1 picked · 0 rejected · 3 unflagged",
  );

  // Clearing a decision empties the counts for that Photo.
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("u");
  await expect(cell(0).locator(".cell-state")).toHaveCount(0);
  await expect(progress).toHaveText(
    "Source progress: 0 picked · 0 rejected · 4 unflagged",
  );

  // Photo View decides with the same counts, and the values come from the
  // server again after a reload.
  await cell(3).click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await openPhotoToolsView(page, "tools");
  await page.getByRole("button", { name: "Reject", exact: true }).click();
  await expect(page.locator("[data-selection]")).toHaveText("Rejected");
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 picked · 1 rejected · 3 unflagged",
  );
  await page.reload();
  await expect(page.getByText(/^Ready · 4 Photos$/)).toBeVisible();
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 picked · 1 rejected · 3 unflagged",
  );
});

test("Selection filters keep source counts, URLs, and Photo traversal stable", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  for (const [index, selectionState] of [
    [0, "picked"],
    [1, "rejected"],
    [2, "picked"],
  ] as const) {
    const response = await post(
      running.url,
      `/api/photos/${ids[index]}/state`,
      {
        field: "selectionState",
        value: selectionState,
      },
    );
    expect(response.ok).toBe(true);
  }

  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 4 Photos$/)).toBeVisible();
  await waitForGridFrame(page);
  const opens: string[] = [];
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/browse"
    )
      opens.push(request.postData() ?? "");
  });

  // Cancel is draft-only: it changes neither the URL nor the Browse Snapshot.
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("picked");
  await page.locator("[data-view-options-cancel]").click();
  await expect(page.locator("[data-view-options]")).toBeHidden();
  expect(opens).toEqual([]);
  await expect(page).toHaveURL(running.url + "/");

  // Apply evaluates one filtered Snapshot and carries its source-wide counts.
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("picked");
  await applyViewOptions(page);
  await expect.poll(() => opens.length).toBe(1);
  await expect(page).toHaveURL(/selection=picked/);
  await expect(page.locator(".photo-cell")).toHaveCount(2);
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 2 picked · 1 rejected · 1 unflagged",
  );

  // The filtered sequence owns Photo position and Previous/Next: the rejected
  // source member is never exposed by the traversal.
  await page.locator('[data-photo-index="0"]').click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await expect(page.locator("[data-position]")).toHaveText("1 / 2");
  await openPhotoToolsView(page, "tools");
  await page.getByRole("button", { name: "Next", exact: true }).click();
  await expect(page.locator("[data-position]")).toHaveText("2 / 2");
  const photoAddress = page.url();
  await page.reload();
  await expect(page.locator("[data-position]")).toHaveText("2 / 2");
  expect(page.url()).toBe(photoAddress);
  await page.goBack();
  await expect(page.locator(".photo-cell")).toHaveCount(2);
  await page.goForward();
  await expect(page.locator("[data-position]")).toHaveText("2 / 2");
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
    "3 Photos Picked.",
  );
  await expect(page.locator("[data-batch-retained]")).toBeVisible();
  await expect(page.locator("[data-grid-batch-result-text]")).toHaveText(
    "3 Photos Picked.",
  );
  await expect(page.locator("[data-grid-batch-result]")).toHaveAttribute(
    "data-tone",
    "success",
  );
  expect(batchBodies).toEqual([
    {
      photos: [
        { photoId: ids[0], expectedCurrent: "unflagged" },
        { photoId: ids[1], expectedCurrent: "unflagged" },
        { photoId: ids[2], expectedCurrent: "unflagged" },
      ],
      selectionState: "picked",
    },
  ]);
  for (const index of [0, 1, 2]) {
    await expect(cell(index).locator(".cell-state.picked")).toHaveText("✓");
    expect(await libraryPhoto(running.url, index)).toMatchObject({
      selectionState: "picked",
    });
  }
  await expect(visibleResults).toHaveText("Visible results: 4 of 4 Photos");
  await expect(progress).toHaveText(
    "Source progress: 3 picked · 0 rejected · 1 unflagged",
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
      selectionState: "unflagged",
    });
  }
  await expect(progress).toHaveText(
    "Source progress: 0 picked · 0 rejected · 4 unflagged",
  );
  await expect(page.locator("[data-review]")).toBeHidden();

  // The one-level description is consumed: nothing is left to undo.
  await page.keyboard.press("Control+z");
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "3 Photos restored.",
  );
  expect(undoWrites).toHaveLength(3);
});
