import { expect, test } from "@playwright/test";
import { copyFile, mkdir, rename } from "node:fs/promises";
import { join } from "node:path";
import {
  fixture,
  post,
  server,
  setupBrowserSmoke,
  writePhotos,
} from "./browser-test-support/fixtures.js";

setupBrowserSmoke();

test("mobile recovery keeps bounded inventory and mapping controls reachable and maps the chosen Original", async ({
  page,
  context,
}) => {
  const { base, root } = await fixture();
  const oldPrefix = "Long remembered folder with recognizable camera filenames";
  const oldFolder = join(root, oldPrefix);
  await mkdir(oldFolder);
  await writePhotos(oldFolder, 30);
  const running = await server(base, root);
  const stored = join(base, "stored");
  await rename(oldFolder, stored);
  await post(running.url, "/api/scan", {});
  await expect
    .poll(async () => {
      const response = await context.request.get(`${running.url}/api/status`);
      const status = (await response.json()) as {
        state: string;
        lastRecovery?: { unavailablePhotos: number };
      };
      return status.state === "idle"
        ? status.lastRecovery?.unavailablePhotos
        : -1;
    })
    .toBe(30);
  await mkdir(join(root, "New folder"));
  await copyFile(
    join(stored, "000.jpg"),
    join(root, "New folder", "renamed.jpg"),
  );

  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await page.locator("[data-source-toggle]").click();
  await page
    .getByRole("button", { name: "Review unavailable Originals" })
    .click();
  const dialog = page.locator("[data-recovery-panel]");
  const body = dialog.locator(".recovery-body");
  const list = dialog.locator("[data-recovery-list]");
  const batch = dialog.locator("[data-recovery-propose]");
  const single = dialog.locator("[data-recovery-propose-single]");
  await expect(batch).toBeInViewport();
  await single.scrollIntoViewIfNeeded();
  await expect(single).toBeInViewport();
  await expect(dialog.locator("[data-recovery-close]")).toBeInViewport();
  expect(await body.evaluate((element) => element.scrollTop)).toBeLessThan(200);
  const geometry = await list.evaluate((element) => ({
    height: element.clientHeight,
    contentHeight: element.scrollHeight,
  }));
  expect(geometry.contentHeight).toBeGreaterThan(geometry.height);
  expect(geometry.height).toBeLessThanOrEqual(844 * 0.3 + 1);
  await expect(list.locator("li")).toHaveCount(25);
  await dialog.locator("[data-recovery-more]").click();
  await expect(list.locator("li")).toHaveCount(30);

  const row = list
    .locator("li")
    .filter({ has: page.getByRole("link", { name: "000.jpg", exact: true }) });
  const map = row.getByRole("button", { name: "Map this Original" });
  const originalId = await map.getAttribute("data-recovery-map-original");
  await map.click();
  await expect(dialog.locator("[data-recovery-single-original]")).toHaveValue(
    originalId!,
  );
  await expect(dialog.locator("[data-recovery-single-location]")).toBeFocused();
  await expect(
    dialog.locator("[data-recovery-single-location]"),
  ).toBeInViewport();
  await dialog
    .locator("[data-recovery-single-location]")
    .fill("New folder/renamed.jpg");
  const proposalRequest = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/recovery/propose") &&
      request.method() === "POST",
  );
  await single.click();
  expect((await proposalRequest).postDataJSON()).toEqual({
    originalId,
    newLocation: "New folder/renamed.jpg",
  });
  await expect(dialog.locator("[data-recovery-proposals]")).toContainText(
    `${oldPrefix}/000.jpg → New folder/renamed.jpg`,
  );
  const response = await post(running.url, "/api/recovery/unavailable", {
    limit: 60,
  });
  expect(response.status).toBe(200);
  const stillUnavailable = (await response.json()) as {
    items: Array<{ originalId: string; location: string }>;
  };
  expect(
    stillUnavailable.items.find((item) => item.originalId === originalId)
      ?.location,
  ).toBe(`${oldPrefix}/000.jpg`);
  expect(
    await dialog.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
  expect(
    await body.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
  await expect(dialog.locator("[data-recovery-close]")).toBeInViewport();
  await dialog.locator("[data-recovery-close]").click();
  await expect(dialog).not.toBeVisible();
});

test("the recovery review opens on the unavailable inventory before any mapping form", async ({
  page,
  context,
}) => {
  const { base, root } = await fixture();
  const oldPrefix = "Inventory before mapping";
  const oldFolder = join(root, oldPrefix);
  await mkdir(oldFolder);
  await writePhotos(oldFolder, 30);
  const running = await server(base, root);
  const stored = join(base, "stored");
  await rename(oldFolder, stored);
  await post(running.url, "/api/scan", {});
  await expect
    .poll(async () => {
      const response = await context.request.get(`${running.url}/api/status`);
      const status = (await response.json()) as {
        state: string;
        lastRecovery?: { unavailablePhotos: number };
      };
      return status.state === "idle"
        ? status.lastRecovery?.unavailablePhotos
        : -1;
    })
    .toBe(30);

  await page.setViewportSize({ width: 1000, height: 800 });
  await page.goto(running.url);
  // Sources is persistent at this width; its disclosure is only for compact layouts.
  await page
    .getByRole("button", { name: "Review unavailable Originals" })
    .click();
  const dialog = page.locator("[data-recovery-panel]");
  const list = dialog.locator("[data-recovery-list]");

  // The bounded unavailable inventory opens first, and it names the remembered
  // facts for each Original rather than an empty mapping form.
  await expect(dialog).toBeVisible();
  await expect(dialog.locator("[data-recovery-summary]")).toHaveText(
    "Showing 25 of 30 unavailable originals",
  );
  const firstEntry = list.locator("li").first();
  await expect(firstEntry).toBeVisible();
  // The entry names the filename, the Location, the kind, the remembered
  // decisions, and whether a fingerprint is available.
  await expect(firstEntry.locator("a")).toHaveText("000.jpg");
  await expect(firstEntry.locator(".recovery-entry-location")).toContainText(
    `${oldPrefix}/000.jpg`,
  );
  await expect(firstEntry.locator(".recovery-entry-facts")).toContainText(
    "JPEG · Unflagged · 0 stars · 0 albums ·",
  );
  await expect(firstEntry.locator(".recovery-entry-facts")).toContainText(
    /Fingerprint on file|No fingerprint/,
  );

  // The mapping configuration is entered from a named inventory item: the row
  // offers the mapping action for that Original, and nothing is proposed
  // before the Photographer chooses it.
  await expect(firstEntry.locator("[data-recovery-map-original]")).toHaveText(
    "Map this Original",
  );
  await expect(dialog.locator("[data-recovery-proposals]")).toBeHidden();
  await expect(dialog.locator("[data-recovery-apply]")).toBeHidden();
  // The mapping forms stay below the inventory, so no empty form precedes it.
  const order = await dialog.evaluate((element) => {
    const listTop = element
      .querySelector("[data-recovery-list]")!
      .getBoundingClientRect().top;
    const formTop = element
      .querySelector("[data-recovery-propose]")!
      .getBoundingClientRect().top;
    return { listTop, formTop };
  });
  expect(order.listTop).toBeLessThan(order.formTop);

  // Entering the mapping from the named item fills that Original's mapping.
  const originalId = await firstEntry
    .locator("[data-recovery-map-original]")
    .getAttribute("data-recovery-map-original");
  await firstEntry.locator("[data-recovery-map-original]").click();
  await expect(dialog.locator("[data-recovery-single-location]")).toBeFocused();
  await expect(dialog.locator("[data-recovery-single-original]")).toHaveValue(
    originalId!,
  );
  await expect(
    dialog.locator("[data-recovery-single-original] option"),
  ).toHaveCount(25);
  await dialog.locator("[data-recovery-more]").click();
  await expect(
    dialog.locator("[data-recovery-single-original] option"),
  ).toHaveCount(30);
  await expect(dialog.locator("[data-recovery-single-original]")).toHaveValue(
    originalId!,
  );
  // No mapping is proposed until the Photographer names the new location.
  await expect(dialog.locator("[data-recovery-proposal-summary]")).toBeHidden();
});
