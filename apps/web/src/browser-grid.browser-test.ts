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
  // The Grid presenter owns the applied column count for the selected
  // thumbnail step; the test reads the count instead of duplicating it.
  const columns = Number(await viewport.getAttribute("data-columns"));
  expect(columns).toBeGreaterThan(0);

  // The Grid viewport is the keyboard's entry: the first arrow enters at the
  // first visible Photo, and later arrows step from the cell it owns.
  await viewport.focus();
  await page.keyboard.press("ArrowRight");
  await expect(cell(0)).toBeFocused();
  const mediaBox = await page
    .locator(".photo-cell .cell-media")
    .first()
    .boundingBox();
  if (!mediaBox) throw new Error("Grid cell media has no geometry");
  expect(Math.abs(mediaBox.width / mediaBox.height - 1.5)).toBeLessThan(0.02);
  await expect(cell(0).locator(".stars span")).toHaveCount(5);
  await expect(cell(0).locator(".star-empty")).toHaveCount(5);
  await expect(cell(0).locator(".stars")).toHaveText("☆☆☆☆☆");
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

  // Arrow movement reaches a row outside the first bounded window; the
  // rendered and focused target cell is the observable proof that the Grid
  // joined the window that contains it.
  for (let step = 0; step < 20; step += 1)
    await page.keyboard.press("ArrowDown");
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
  await page.locator("[data-dock-reject]").click();
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
  await page.locator("[data-dock-next]").click();
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

test("narrow Large cells keep their 3:2 media and complete caption in one row", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 4 Photos",
  );
  await waitForGridFrame(page);
  await openViewOptions(page);
  await page.locator("[data-size-select]").selectOption("large");
  await applyViewOptions(page);
  await waitForGridFrame(page);
  await expect(page.locator("[data-grid-viewport]")).toHaveAttribute(
    "data-columns",
    "1",
  );
  const cells = page.locator(".photo-cell[data-photo-index]");
  await expect(cells).toHaveCount(4);
  const geometry = await cells.first().evaluate((cell) => {
    const media = cell.querySelector<HTMLElement>(".cell-media");
    const footer = cell.querySelector<HTMLElement>(".cell-footer");
    if (!media || !footer) throw new Error("Cell geometry is incomplete");
    const cellBox = cell.getBoundingClientRect();
    const mediaBox = media.getBoundingClientRect();
    const footerBox = footer.getBoundingClientRect();
    return {
      cellBottom: cellBox.bottom,
      cellHeight: cellBox.height,
      mediaRatio: mediaBox.width / mediaBox.height,
      footerBottom: footerBox.bottom,
    };
  });
  expect(Math.abs(geometry.mediaRatio - 1.5)).toBeLessThan(0.02);
  expect(geometry.footerBottom).toBeLessThanOrEqual(geometry.cellBottom + 1);
  const nextTop = (await cells.nth(1).boundingBox())!.y;
  expect(nextTop).toBeGreaterThanOrEqual(geometry.cellBottom);
});

test("Grid size steps fill complete rows and keep incomplete rows uniform", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  await writePhotos(root, 41);
  const running = await server(base, root);
  await page.setViewportSize({ width: 1200, height: 800 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 41 Photos$/)).toBeVisible();
  const viewport = page.locator("[data-grid-viewport]");
  await waitForGridFrame(page);

  for (const size of ["small", "medium", "large"]) {
    await openViewOptions(page);
    if (size === "small")
      await expect(page.locator("[data-size-select]")).toHaveValue("medium");
    await page.locator("[data-size-select]").selectOption(size);
    await applyViewOptions(page);
    for (const width of [1200, 761, 760, 390]) {
      await page.setViewportSize({ width, height: 800 });
      await waitForGridFrame(page);
      await viewport.evaluate((element) => {
        element.scrollTop = 0;
      });
      await expect(page.locator('[data-photo-index="0"]')).toBeVisible();
      await expect
        .poll(async () =>
          viewport.evaluate((element) => {
            const cells = Array.from(
              element.querySelectorAll<HTMLElement>(
                ".photo-cell[data-photo-index]",
              ),
            );
            const count = Number(element.dataset.columns);
            if (cells.length < count) return false;
            const first = cells[0]!.getBoundingClientRect();
            const last = cells[count - 1]!.getBoundingClientRect();
            const view = element.getBoundingClientRect();
            const left = first.left - view.left;
            const right = view.left + element.clientWidth - last.right;
            return (
              Math.abs(left - right) < 2 &&
              cells.slice(0, count).every((cell) => {
                const box = cell.getBoundingClientRect();
                const media = cell
                  .querySelector<HTMLElement>(".cell-media")!
                  .getBoundingClientRect();
                const footer = cell
                  .querySelector<HTMLElement>(".cell-footer")!
                  .getBoundingClientRect();
                return (
                  Math.abs(box.width - first.width) < 1 &&
                  Math.abs(box.height - first.height) < 1 &&
                  Math.abs(media.width / media.height - 1.5) < 0.02 &&
                  footer.bottom <= box.bottom + 1
                );
              })
            );
          }),
        )
        .toBe(true);
      const firstWidth = (await page
        .locator('[data-photo-index="0"]')
        .boundingBox())!.width;
      await viewport.evaluate((element) => {
        element.scrollTop = element.scrollHeight;
      });
      const last = page.locator('[data-photo-index="40"]');
      await expect(last).toBeVisible();
      expect(
        Math.abs((await last.boundingBox())!.width - firstWidth),
      ).toBeLessThan(1);
    }
  }
  // Find a Large-step transition without assuming a particular sidebar width.
  let lower = 800;
  let upper = 1200;
  const countAt = async (width: number) => {
    await page.setViewportSize({ width, height: 800 });
    await viewport.evaluate((element) => {
      element.scrollTop = 0;
    });
    await waitForGridFrame(page);
    await waitForGridFrame(page);
    return Number(await viewport.getAttribute("data-columns"));
  };
  const lowerCount = await countAt(lower);
  expect(await countAt(upper)).toBeGreaterThan(lowerCount);
  while (upper - lower > 1) {
    const middle = Math.floor((lower + upper) / 2);
    if ((await countAt(middle)) === lowerCount) lower = middle;
    else upper = middle;
  }
  expect(await countAt(lower)).toBe(lowerCount);
  expect(await countAt(upper)).toBe(lowerCount + 1);
});

test("Grid reanchors on resize and text enlargement with required caption facts visible", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 150);
  const running = await server(base, root);
  await page.setViewportSize({ width: 1200, height: 800 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 150 Photos$/)).toBeVisible();
  await waitForGridFrame(page);
  const viewport = page.locator("[data-grid-viewport]");
  const count = Number(await viewport.getAttribute("data-columns"));
  const index = count * 8;
  await viewport.evaluate((element) => {
    const first = element.querySelector<HTMLElement>('[data-photo-index="0"]')!;
    const next = element.querySelector<HTMLElement>(
      `[data-photo-index="${element.dataset.columns}"]`,
    )!;
    element.scrollTop = 8 * (next.offsetTop - first.offsetTop) + 5;
  });
  const anchor = page.locator(`[data-photo-index="${index}"]`);
  await expect(anchor).toBeVisible();
  await anchor.focus();
  await page.keyboard.press("x");
  await expect(anchor).toHaveAccessibleName(/Rejected/);
  await page.keyboard.press("5");
  await expect(anchor).toHaveAccessibleName(/5 stars/);
  await page.setViewportSize({ width: 900, height: 800 });
  await expect(anchor).toBeFocused();
  await expect
    .poll(() =>
      anchor.evaluate((cell) => {
        const view = cell.closest<HTMLElement>("[data-grid-viewport]")!;
        const row = cell.getBoundingClientRect();
        const viewportBox = view.getBoundingClientRect();
        return row.bottom > viewportBox.top && row.top < viewportBox.bottom;
      }),
    )
    .toBe(true);
  await page.locator("[data-grid-select-mode]").click();
  await anchor.focus();

  await page.addStyleTag({ content: "html { font-size: 200% !important; }" });
  await expect(anchor).toBeFocused();
  await expect
    .poll(() =>
      anchor.evaluate((cell) => {
        const box = cell.getBoundingClientRect();
        const view = cell
          .closest<HTMLElement>("[data-grid-viewport]")!
          .getBoundingClientRect();
        const required = [".cell-name", ".cell-pos", ".state-text", ".stars"];
        return (
          box.bottom > view.top &&
          box.top < view.bottom &&
          required.every((selector) => {
            const fact = cell.querySelector<HTMLElement>(selector)!;
            const rect = fact.getBoundingClientRect();
            return (
              rect.width > 0 &&
              rect.height > 0 &&
              rect.bottom <= box.bottom + 1 &&
              rect.right <= box.right + 1
            );
          })
        );
      }),
    )
    .toBe(true);
  await expect(anchor).toHaveAccessibleName(/Rejected.*5 stars/);
  await page.locator("[data-grid-multi-done]").click();
  await anchor.focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("[data-review]")).toBeVisible();
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(anchor).toBeFocused();
});

test("the batch tray leaves the final Grid row scrollable above it", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 40);
  const running = await server(base, root);
  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 40 Photos$/)).toBeVisible();
  await waitForGridFrame(page);
  await page.locator("[data-grid-select-mode]").click();

  const viewport = page.locator("[data-grid-viewport]");
  await viewport.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  const last = page.locator('[data-photo-index="39"]');
  await expect(last).toBeVisible();
  await last.click();
  const tray = page.locator("[data-grid-batch]");
  await expect(tray).toBeVisible();

  // The tray's measured height becomes scrollable clearance. At the end of
  // that range the final row sits above the tray instead of under it. The
  // measurement arrives asynchronously, so re-scroll after it becomes
  // scrollable instead of assuming the first scroll already includes it.
  await expect
    .poll(async () => {
      await viewport.evaluate((element) => {
        element.scrollTop = element.scrollHeight;
      });
      const [cellBox, trayBox] = await Promise.all([
        last.boundingBox(),
        tray.boundingBox(),
      ]);
      if (!cellBox || !trayBox) return false;
      return cellBox.y + cellBox.height <= trayBox.y + 1;
    })
    .toBe(true);
});

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
  await expect(count).toHaveText("0 / 100 selected");
  await expect(page.locator("[data-grid-multi-done]")).toBeVisible();
  await expect(page.locator("[data-grid-batch]")).toBeHidden();
  await page.locator("[data-grid-multi-done]").click();
  await expect(page.locator("[data-grid-select-mode]")).toBeVisible();
  await page.locator("[data-grid-select-mode]").click();
  await page.keyboard.press("Escape");
  await expect(page.locator("[data-grid-select-mode]")).toBeVisible();
  await page.locator("[data-grid-select-mode]").click();
  for (const index of [0, 1, 2]) await cell(index).click();
  await expect(count).toHaveText("3 / 100 selected");
  await page.locator("[data-batch-pick]").click();
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
  await expect(count).toHaveText("3 / 100 selected");

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
