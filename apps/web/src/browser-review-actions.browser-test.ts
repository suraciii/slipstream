import { expect, test } from "@playwright/test";
import { unlink } from "node:fs/promises";
import { join } from "node:path";
import {
  fixture,
  post,
  server,
  setupBrowserSmoke,
  writePhotos,
} from "./browser-test-support/fixtures.js";
import {
  closePhotoTools,
  openPhotoToolsView,
  startReview,
  waitForLoadedReviewImage,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

test("review strip and Photo tools share persisted decisions and Undo", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  const picked = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().includes("/state"),
  );
  await page.locator("[data-dock-pick]").click();
  await picked;
  await expect(page.locator("[data-selection]")).toHaveText("Picked");
  await page.locator("[data-dock-undo]").click();
  await openPhotoToolsView(page, "tools");
  await expect(page.locator("[data-photo-tools-clear]")).toBeVisible();
  await closePhotoTools(page);
  await page.reload();
  await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
  const rejected = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().includes("/state"),
  );
  await page.locator("[data-dock-reject]").click();
  await rejected;
  await expect(page.locator("[data-selection]")).toHaveText("Rejected");
  await page.reload();
  await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
  await expect(page.locator("[data-position]")).toHaveText("1 / 2");
});

test("Photo write failure stays visible in tools and after returning to review", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  await page.setViewportSize({ width: 375, height: 667 });
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  await page.route("**/api/photos/*/state", (route) =>
    route.fulfill({
      status: 409,
      contentType: "application/json",
      body: JSON.stringify({ error: "photo_changed" }),
    }),
  );
  await page.locator("[data-dock-pick]").click();
  const status = page.locator("[data-status]");
  await status.scrollIntoViewIfNeeded();
  await expect(status).toBeInViewport();
  await closePhotoTools(page);
  await status.scrollIntoViewIfNeeded();
  await expect(status).toBeInViewport();
  await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
  await page.unroute("**/api/photos/*/state");
  await page.locator("[data-retry-photo]").click();
  await expect(page.locator("[data-dock-pick]")).toBeEnabled();
  await page.locator("[data-dock-pick]").click();
  await expect(page.locator("[data-selection]")).toHaveText("Picked");
});

test("unavailable Original keeps its Photo review and visible recovery action", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  await unlink(join(root, "000.jpg"));
  expect((await post(running.url, "/api/scan", {})).status).toBe(200);
  await page.reload();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  const unavailableCell = page.getByRole("button", { name: /Photo 1 of 1/ });
  await expect(unavailableCell).toHaveAccessibleName(/Original unavailable/);
  await unavailableCell.click();
  const status = page.locator("[data-status]");
  await expect(page.locator("[data-stage]")).toContainText(
    "Preview unavailable",
  );
  await expect(status).toContainText("Original File is unavailable");
  await expect(status).toBeVisible();
  await status
    .getByRole("button", { name: "Review unavailable Originals" })
    .click();
  await expect(page.locator("[data-recovery-panel]")).toBeVisible();
});

test("review controls preserve Preview space and reachable targets across rotation", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  await page.locator("[data-dock-pick]").click();
  await expect(page.locator("[data-selection]")).toHaveText("Picked");
  for (const { width, height, minimum } of [
    { width: 375, height: 500, minimum: 480 },
    { width: 375, height: 667, minimum: 480 },
    { width: 390, height: 844, minimum: 650 },
    { width: 667, height: 375, minimum: 240 },
    { width: 760, height: 480, minimum: 240 },
    { width: 761, height: 480, minimum: 240 },
    { width: 761, height: 481, minimum: 480 },
    { width: 844, height: 390, minimum: 240 },
    { width: 1024, height: 768, minimum: 480 },
    { width: 1280, height: 800, minimum: 480 },
  ]) {
    await page.setViewportSize({ width, height });
    await expect
      .poll(
        async () =>
          (await page.locator("[data-preview]").boundingBox())?.height ?? 0,
      )
      .toBeGreaterThanOrEqual(minimum);
    for (const selector of [
      "[data-dock-pick]",
      "[data-dock-reject]",
      "[data-dock-undo]",
      "[data-dock-more]",
    ]) {
      const control = page.locator(selector);
      await control.scrollIntoViewIfNeeded();
      await expect(control).toBeInViewport();
      const box = await control.boundingBox();
      expect(box?.width).toBeGreaterThanOrEqual(44);
      expect(box?.height).toBeGreaterThanOrEqual(44);
    }
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    await expect(page.locator("[data-selection]")).toHaveText("Picked");
    await expect(page.locator("[data-position]")).toHaveText("1 / 2");
  }
  await page.setViewportSize({ width: 375, height: 667 });
  await page.locator("html").evaluate((root) => {
    root.style.fontSize = "200%";
  });
  for (const selector of [
    "[data-selection]",
    "[data-dock-pick]",
    "[data-dock-reject]",
    "[data-dock-undo]",
    "[data-dock-more]",
  ]) {
    const control = page.locator(selector);
    await control.scrollIntoViewIfNeeded();
    await expect(control).toBeInViewport();
    expect(
      await control.evaluate((node) => {
        const box = node.getBoundingClientRect();
        const hit = document.elementFromPoint(
          box.x + box.width / 2,
          box.y + box.height / 2,
        );
        return hit === node || node.contains(hit);
      }),
    ).toBe(true);
  }
  expect(
    await page
      .locator("[data-review]")
      .evaluate((node) => node.scrollWidth <= node.clientWidth),
  ).toBe(true);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});
