import { expect, test } from "@playwright/test";
import {
  fixture,
  jpeg,
  server,
  setupBrowserSmoke,
  writePhotos,
} from "./browser-test-support/fixtures.js";
import {
  closePhotoTools,
  openPhotoToolsView,
  openViewOptions,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

test("Grid and Nearby Photos replace missing and undecodable thumbnails and recover after Refresh", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  const imageBytes = await jpeg();
  let recovered = false;
  let releasePending: (() => void) | undefined;
  const pending = new Promise<void>((resolve) => {
    releasePending = resolve;
  });
  let pendingPhotoId = "";

  await page.route("**/thumbnail-test/*.jpg", async (route) => {
    await route.fulfill({
      contentType: "image/jpeg",
      body: route.request().url().endsWith("broken.jpg")
        ? Buffer.from("This response cannot decode as a JPEG")
        : imageBytes,
    });
  });
  await page.route(/\/api\/browse\/[^/?]+\?/, async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as {
      start: number;
      photos: Array<{
        id: string;
        selectionState: string;
        rating: number;
        preview: { state: string; thumbnailUrl?: string };
      }>;
    };
    for (const [offset, photo] of body.photos.entries()) {
      const index = body.start + offset;
      if (index === 0) {
        photo.selectionState = "picked";
        photo.rating = 4;
        photo.preview.thumbnailUrl = recovered
          ? "/thumbnail-test/recovered.jpg"
          : "/thumbnail-test/broken.jpg";
      } else if (!recovered && index === 1) {
        photo.preview = { state: "unavailable" };
      } else if (!recovered && index === 2) {
        photo.preview = { state: "failed" };
      } else if (!recovered && index === 3) {
        pendingPhotoId = photo.id;
        photo.preview = { state: "inspection-pending" };
      }
    }
    await route.fulfill({ response, json: body });
  });
  await page.route("**/api/photos/*/thumbnail", async (route) => {
    if (recovered) {
      await route.continue();
      return;
    }
    if (route.request().url().includes(`/${pendingPhotoId}/`)) await pending;
    await route.fulfill({ status: 404, json: { state: "failed" } });
  });

  await page.setViewportSize({ width: 1200, height: 800 });
  await page.goto(running.url);
  const cell = (index: number) => page.locator(`[data-photo-index="${index}"]`);
  await expect(cell(0).locator(".thumbnail-placeholder")).toHaveText(
    "Thumbnail delivery failed",
  );
  for (const [index, label] of [
    [1, "Preview unavailable"],
    [2, "Thumbnail failed"],
    [3, "Preparing Preview…"],
  ] as const) {
    await expect(cell(index).locator(".thumbnail-placeholder")).toHaveText(
      label,
    );
    await expect(cell(index).locator(".thumbnail-placeholder")).toBeVisible();
    await expect(cell(index).locator("img")).toBeHidden();
    await expect(cell(index)).toBeEnabled();
  }
  await expect(cell(0).locator("img")).toBeHidden();
  await expect(cell(0).locator("img")).not.toHaveAttribute("src");
  await expect(cell(0)).toHaveAccessibleName(
    /Photo 1 of 4.*Picked.*4 stars.*Thumbnail delivery failed/,
  );
  await expect(cell(0).locator(".cell-state.picked")).toBeVisible();
  const box = await cell(0).boundingBox();
  releasePending?.();

  await cell(0).click();
  await openPhotoToolsView(page, "nearby");
  const strip = page.locator("[data-filmstrip]");
  const neighbor = (index: number) =>
    strip.locator(`[data-filmstrip-index="${index}"]`);
  await expect(neighbor(0).locator(".thumbnail-placeholder")).toHaveText(
    "Thumbnail delivery failed",
  );
  await expect(neighbor(0).locator("img")).toBeHidden();
  await expect(neighbor(1).locator(".thumbnail-placeholder")).toHaveText(
    "Preview unavailable",
  );
  await expect(neighbor(2).locator(".thumbnail-placeholder")).toHaveText(
    "Thumbnail failed",
  );
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid", exact: true }).click();

  recovered = true;
  await openViewOptions(page);
  await page.locator("[data-refresh]").click();
  await expect(cell(0).locator("img")).toBeVisible();
  await expect(cell(0).locator(".thumbnail-placeholder")).toBeHidden();
  await expect(cell(0)).toHaveAccessibleName(/Photo 1 of 4.*Picked.*4 stars/);
  await expect(cell(0)).not.toHaveAccessibleName(/Thumbnail delivery failed/);
  const recoveredBox = await cell(0).boundingBox();
  if (await page.locator("[data-view-options]").isVisible())
    await page.locator("[data-view-options-cancel]").click();
  expect(recoveredBox?.width).toBe(box?.width);
  expect(recoveredBox?.height).toBe(box?.height);
  expect(
    await cell(0)
      .locator("img")
      .evaluate((image: HTMLImageElement) => image.naturalWidth),
  ).toBeGreaterThan(0);

  await cell(0).click();
  await openPhotoToolsView(page, "nearby");
  await expect(neighbor(0).locator("img")).toBeVisible();
  await expect(neighbor(0).locator(".thumbnail-placeholder")).toBeHidden();
});
