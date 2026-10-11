import { expect, test } from "@playwright/test";
import {
  fixture,
  server,
  setupBrowserSmoke,
  writePhotos,
} from "./browser-test-support/fixtures.js";
import {
  openViewOptions,
  applyViewOptions,
  waitForGridFrame,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

test("Trash distinguishes loading, failure and empty after a real removal and Undo", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 1 Photo",
  );
  await waitForGridFrame(page);
  // A management round trip discards unapplied choices. Apply must commit
  // the choices shown after returning, never a hidden draft from before Trash.
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("rejected");
  await page.locator("[data-removed-open]").click();
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "Trash is empty.",
  );
  await page.locator("[data-removed-close]").click();
  await expect(page.locator("[data-filter-select]")).toHaveValue("all");
  await applyViewOptions(page);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 1 Photo",
  );
  await expect(page.locator("[data-view-options-flag]")).toBeHidden();

  const cell = page.locator('[data-photo-index="0"]');
  await cell.focus();
  await page.keyboard.press("x");
  await expect(cell.locator(".cell-state.rejected")).toBeVisible();
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("rejected");
  await applyViewOptions(page);
  await expect(page.locator("[data-view-options-flag]")).toContainText(
    "Rejected",
  );
  for (const viewport of [
    { width: 375, height: 667 },
    { width: 390, height: 844 },
  ]) {
    await page.setViewportSize(viewport);
    expect(
      await page
        .locator(".grid-header")
        .evaluate((element) => element.getBoundingClientRect().height),
    ).toBeLessThanOrEqual(88);
    expect(
      await page
        .locator(".grid-header")
        .evaluate((element) => element.scrollWidth <= element.clientWidth),
    ).toBe(true);
    await expect(page.locator("[data-grid-view-options]")).toBeInViewport();
    await expect(page.locator("[data-grid-select-mode]")).toBeInViewport();
  }
  await openViewOptions(page);
  await page.locator("[data-removal-open]").click();
  await page.locator("[data-removal-confirm]").click();
  await expect(page.locator("[data-removal-undo]")).toBeVisible();
  await page.locator("[data-removal-close]").click();
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let first = true;
  await page.route(/\/api\/trash\?/, async (route) => {
    if (!first) {
      await route.continue();
      return;
    }
    first = false;
    await held;
    await route.fulfill({ status: 503, json: { error: "unavailable" } });
  });
  await openViewOptions(page);
  await page.locator("[data-removed-open]").click();
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "Loading Trash…",
  );
  await expect(page.locator("[data-removed-undo]")).toBeHidden();
  release();
  await expect(page.locator("[data-removed-retry]")).toBeVisible();
  await expect(page.locator("[data-removed-status]")).not.toContainText(
    "empty",
  );
  await expect(page.locator("[data-removed-undo]")).toBeHidden();
  await page.locator("[data-removed-retry]").click();
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    1,
  );
  await expect(page.locator("[data-removed-undo]")).toBeVisible();
  await page.locator("[data-removed-undo]").click();
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "Trash is empty.",
  );
  await expect(page.locator("[data-removed-undo]")).toBeHidden();
  await page.locator("[data-removed-close]").click();
  await page.reload();
  await openViewOptions(page);
  await page.locator("[data-removed-open]").click();
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "Trash is empty.",
  );
  await expect(page.locator("[data-removed-undo]")).toBeHidden();
});
