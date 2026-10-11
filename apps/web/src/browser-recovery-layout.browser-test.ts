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

test("mobile recovery keeps mapping tasks ahead of bounded lists and maps the chosen Original", async ({
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
    .getByRole("button", { name: "Review unavailable originals" })
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
