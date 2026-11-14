import { expect, test, type Page } from "@playwright/test";
import {
  fixture,
  server,
  setupBrowserSmoke,
  writePhotos,
} from "./browser-test-support/fixtures.js";
import {
  openDockSecondary,
  openGrid,
  startReview,
  waitForLoadedReviewImage,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

const routeFailedStatus = (page: Page) =>
  page.route("**/api/status", async (route) => {
    const response = await route.fetch();
    const status = (await response.json()) as Record<string, unknown>;
    await route.fulfill({ response, json: { ...status, state: "failed" } });
  });

const expectPointerReachable = (page: Page, selector: string) =>
  expect
    .poll(() =>
      page
        .locator(selector)
        .first()
        .evaluate((target) => {
          const rect = target.getBoundingClientRect();
          const hit = document.elementFromPoint(
            rect.left + rect.width / 2,
            rect.top + rect.height / 2,
          );
          return hit === target || target.contains(hit);
        }),
    )
    .toBe(true);

test("Library status clears the Photo View dock at every supported size", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  await routeFailedStatus(page);

  const entry = page.locator("[data-library-status-entry]");
  const toggle = entry.locator("[data-library-status-toggle]");
  const notice = entry.locator("[data-library-status-notice]");
  await expect(entry).toBeVisible({ timeout: 15_000 });
  await expect(toggle).toHaveAccessibleName("Library check failed — Retry");
  await toggle.click();
  await expect(notice).toBeVisible();

  for (const viewport of [
    { width: 667, height: 375 },
    { width: 390, height: 844 },
    { width: 375, height: 667 },
    { width: 844, height: 390 },
  ]) {
    await page.setViewportSize(viewport);
    await expect(entry).toBeVisible();
    await expect(notice).toBeVisible();
    for (const selector of [
      "[data-dock-reject]",
      "[data-dock-rating]",
      "[data-dock-pick]",
      "[data-photo-navigation] button",
    ])
      await expectPointerReachable(page, selector);
    // Undo and More sit behind the dock disclosure at phone widths; open it so
    // the secondary pair is reachable as direct targets too, and prove the
    // disclosure control itself is reachable there.
    if (viewport.width <= 420)
      await expectPointerReachable(page, "[data-dock-overflow]");
    await openDockSecondary(page);
    for (const selector of ["[data-dock-undo]", "[data-dock-more]"])
      await expectPointerReachable(page, selector);
  }
});

test("Library status clears an expanded Grid batch tray", async ({ page }) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  await openGrid(page, running.url, "All Photos");
  await routeFailedStatus(page);

  const entry = page.locator("[data-library-status-entry]");
  const toggle = entry.locator("[data-library-status-toggle]");
  await expect(entry).toBeVisible({ timeout: 15_000 });
  await page.locator("[data-grid-select-mode]").click();
  for (const index of [0, 1, 2])
    await page.locator(`[data-photo-index="${index}"]`).click();
  await page.locator("[data-batch-pick]").click();
  await expect(page.locator("[data-grid-batch-result]")).toBeVisible();
  await page.locator("[data-grid-batch-result] summary").click();
  await expect(page.locator("[data-grid-batch-result]")).toHaveAttribute(
    "open",
    "",
  );

  const tray = page.locator("[data-grid-batch]");
  // The tray is measured as it expands; the entry must move before it can
  // cover the status toggle's pointer target.
  await expect
    .poll(async () => {
      const [trayBox, toggleBox] = await Promise.all([
        tray.boundingBox(),
        toggle.boundingBox(),
      ]);
      return Boolean(
        trayBox && toggleBox && toggleBox.y + toggleBox.height <= trayBox.y,
      );
    })
    .toBe(true);
  await expectPointerReachable(page, "[data-library-status-toggle]");
});
