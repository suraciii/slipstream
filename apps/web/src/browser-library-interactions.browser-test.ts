import { expect, test } from "@playwright/test";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import {
  fixture,
  server,
  setupBrowserSmoke,
  writePhotos,
  post,
} from "./browser-test-support/fixtures.js";
import {
  openSources,
  openViewOptions,
  applyViewOptions,
  waitForGridFrame,
  openMembershipPanel,
  membershipCheckbox,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

test("thumbnail size commits only on Apply without reopening the source", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 4 Photos",
  );
  await waitForGridFrame(page);
  const cellWidth = () =>
    page
      .locator('[data-photo-index="0"]')
      .evaluate((cell) => cell.getBoundingClientRect().width);
  const initialWidth = await cellWidth();
  let opens = 0;
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/browse"
    )
      opens++;
  });
  for (const close of ["cancel", "close", "escape", "management"] as const) {
    await openViewOptions(page);
    await page.locator("[data-size-select]").selectOption("large");
    expect(await cellWidth()).toBe(initialWidth);
    if (close === "escape") await page.keyboard.press("Escape");
    else if (close === "management") {
      await page.locator("[data-removed-open]").click();
      await page.locator("[data-removed-close]").click();
    } else await page.locator(`[data-view-options-${close}]`).click();
    await openViewOptions(page);
    await expect(page.locator("[data-size-select]")).toHaveValue("medium");
    await applyViewOptions(page);
    expect(await cellWidth()).toBe(initialWidth);
  }
  await openViewOptions(page);
  await page.locator("[data-size-select]").selectOption("large");
  await applyViewOptions(page);
  await expect.poll(cellWidth).toBeGreaterThan(initialWidth);
  await openViewOptions(page);
  await expect(page.locator("[data-size-select]")).toHaveValue("large");
  await page.locator("[data-view-options-cancel]").click();
  expect(opens).toBe(0);
});

test("failed membership has no false checkboxes and Retry restores confirmed membership", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Picks" });
  let fail = true;
  await page.route(/\/api\/photos\/[^/]+\/albums$/, async (route) => {
    if (fail)
      await route.fulfill({ status: 503, json: { error: "unavailable" } });
    else await route.continue();
  });
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 1 Photo",
  );
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await openMembershipPanel(page);
  await expect(page.locator("[data-membership-retry]")).toBeVisible();
  await expect(page.locator("[data-membership-options] input")).toHaveCount(0);
  fail = false;
  await page.locator("[data-membership-retry]").click();
  await expect(membershipCheckbox(page, "Picks")).toBeEnabled();
  await expect(membershipCheckbox(page, "Picks")).not.toBeChecked();
});

test("nested Folder expansion and paging keep focus on their controls", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await Promise.all(
    Array.from({ length: 121 }, async (_, index) => {
      const folder = join(
        root,
        "Parent",
        `Child-${String(index).padStart(3, "0")}`,
      );
      await mkdir(folder, { recursive: true });
      await writePhotos(folder, 1);
    }),
  );
  const running = await server(base, root);
  await page.setViewportSize({ width: 1200, height: 800 });
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 121 Photos",
  );
  await openSources(page);
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  const expand = page.getByRole("button", { name: "Toggle Parent subfolders" });
  await expand.focus();
  await expand.press("Enter");
  await expect(expand).toHaveAttribute("aria-expanded", "true");
  await expect(expand).toBeFocused();
  const next = page.locator('[data-focus-key="folder:next:Parent"]');
  const previous = page.locator('[data-focus-key="folder:previous:Parent"]');
  await expect(next).toBeEnabled();
  await next.focus();
  await next.press("Enter");
  await expect(
    page.locator(".folder-pager .folder-page-label").last(),
  ).toHaveText("2 / 3");
  await expect(next).toBeFocused();
  await next.press("Enter");
  await expect(
    page.locator(".folder-pager .folder-page-label").last(),
  ).toHaveText("3 / 3");
  await expect(next).toBeDisabled();
  await expect(previous).toBeFocused();
  await previous.press("Enter");
  await expect(
    page.locator(".folder-pager .folder-page-label").last(),
  ).toHaveText("2 / 3");
  await expect(previous).toBeFocused();
  await previous.press("Enter");
  await expect(
    page.locator(".folder-pager .folder-page-label").last(),
  ).toHaveText("1 / 3");
  await expect(previous).toBeDisabled();
  await expect(next).toBeFocused();
  await expand.focus();
  await expand.press("Enter");
  await expect(expand).toHaveAttribute("aria-expanded", "false");
  await expect(expand).toBeFocused();
});
