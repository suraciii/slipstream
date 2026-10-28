import { expect, test } from "@playwright/test";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  setupBrowserSmoke,
  fixture,
  jpegWithSize,
  server,
} from "./browser-test-support/fixtures.js";
import {
  openPhotoToolsView,
  previewImageGeometry,
  startReview,
  waitForLoadedReviewImage,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

test("a 640 × 480 Preview exposes source-limited detail at Fit", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "small.jpg"), await jpegWithSize(page, 640, 480));
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);

  const image = await previewImageGeometry(page);
  expect([image.naturalWidth, image.naturalHeight]).toEqual([640, 480]);
  expect(image.width).toBeLessThan(image.naturalWidth);
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "fit",
  );
  await openPhotoToolsView(page, "details");
  await expect(page.locator("[data-source]")).toHaveText(
    "JPEG · 640 × 480 · limited detail · Preview pixels are not enlarged",
  );
  await expect(page.locator(".photo-tools-preview")).toMatchAriaSnapshot(
    "- paragraph: /JPEG.*640 × 480.*limited detail.*Preview pixels are not enlarged/",
  );
});

test("a normal-review Preview reports enlargement at 200% and clears it at Fit", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(
    join(root, "large.jpg"),
    await jpegWithSize(page, 2560, 1920),
  );
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);

  const image = await previewImageGeometry(page);
  expect([image.naturalWidth, image.naturalHeight]).toEqual([2560, 1920]);
  await openPhotoToolsView(page, "details");
  const evidence = page.locator("[data-source]");
  await expect(evidence).toHaveText(
    "JPEG · 2560 × 1920 · normal review · Preview pixels are not enlarged",
  );

  await openPhotoToolsView(page, "zoom");
  const slider = page.getByRole("slider", { name: "Zoom percentage" });
  await slider.fill("200");
  await slider.dispatchEvent("input");
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "manual",
  );
  expect((await previewImageGeometry(page)).width).toBeCloseTo(5120, 0);
  await openPhotoToolsView(page, "details");
  await expect(evidence).toHaveText(
    "JPEG · 2560 × 1920 · normal review · Enlarged Preview pixels cannot add source detail",
  );
  await expect(page.locator(".photo-tools-preview")).toMatchAriaSnapshot(
    "- paragraph: /JPEG.*2560 × 1920.*normal review.*Enlarged Preview pixels cannot add source detail/",
  );

  await openPhotoToolsView(page, "zoom");
  await page.getByRole("button", { name: "Fit Window", exact: true }).click();
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "fit",
  );
  await openPhotoToolsView(page, "details");
  await expect(evidence).toHaveText(
    "JPEG · 2560 × 1920 · normal review · Preview pixels are not enlarged",
  );
});

test("resizing across the Fit enlargement threshold preserves Preview capability and image", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "small.jpg"), await jpegWithSize(page, 640, 480));
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  const image = await page.locator("[data-stage] img").elementHandle();
  if (!image) throw new Error("Preview image is missing");
  const source = await image.getAttribute("src");
  await openPhotoToolsView(page, "details");
  const evidence = page.locator("[data-source]");
  await expect(evidence).toHaveText(
    "JPEG · 640 × 480 · limited detail · Preview pixels are not enlarged",
  );
  expect((await previewImageGeometry(page)).width).toBeLessThan(640);

  await page.setViewportSize({ width: 1600, height: 1200 });
  await expect
    .poll(async () => (await previewImageGeometry(page)).width)
    .toBeGreaterThan(640);
  await expect(evidence).toHaveText(
    "JPEG · 640 × 480 · limited detail · Enlarged Preview pixels cannot add source detail",
  );
  await expect(page.locator(".photo-tools-preview")).toMatchAriaSnapshot(
    "- paragraph: /JPEG.*640 × 480.*limited detail.*Enlarged Preview pixels cannot add source detail/",
  );
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "fit",
  );
  expect(
    await image.evaluate(
      (element) => element === document.querySelector("[data-stage] img"),
    ),
  ).toBe(true);
  expect(await image.getAttribute("src")).toBe(source);

  await page.setViewportSize({ width: 390, height: 844 });
  await expect
    .poll(async () => (await previewImageGeometry(page)).width)
    .toBeLessThan(640);
  await expect(evidence).toHaveText(
    "JPEG · 640 × 480 · limited detail · Preview pixels are not enlarged",
  );
  expect(
    await image.evaluate(
      (element) => element === document.querySelector("[data-stage] img"),
    ),
  ).toBe(true);
  expect(await image.getAttribute("src")).toBe(source);
  await image.dispose();
});
