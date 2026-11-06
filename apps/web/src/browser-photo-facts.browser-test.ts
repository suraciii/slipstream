import { expect, test } from "@playwright/test";
import {
  fixture,
  server,
  setupBrowserSmoke,
  writePhotos,
} from "./browser-test-support/fixtures.js";
import {
  openPhotoToolsView,
  startReview,
  waitForLoadedReviewImage,
} from "./browser-test-support/surfaces.js";
import { expectControlsAboveSafeViewport } from "./browser-test-support/review-geometry.js";

setupBrowserSmoke();

test("Photo View shows review capture metadata and explicit missing values", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  await page.route("**/api/photos/*/metadata", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        captureTime: "2026-02-03T04:05:06.000000000",
        aperture: "f/2.8",
        iso: 400,
        shutterSpeed: "1/125 s",
        focalLength: "50 mm",
      }),
    }),
  );
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");

  // Capture facts are part of the primary review context. Normal availability
  // and Preview provenance stay out of this row.
  const facts = page.locator("[data-photo-facts-line]");
  await expect(facts.locator("[data-photo-facts-value]")).toHaveText([
    "2026-02-03 04:05",
    "f/2.8",
    "1/125 s",
    "ISO 400",
    "50 mm",
  ]);
  await expect(facts.locator(".photo-facts-label")).toHaveText([
    "Capture Time: ",
    "Aperture: ",
    "Shutter Speed: ",
    "ISO: ",
    "Focal Length: ",
  ]);
  await expect(
    facts.locator("[data-photo-facts-capture] [data-photo-facts-value]"),
  ).toHaveText("2026-02-03 04:05");
  await expect(
    facts.locator("[data-photo-facts-aperture] [data-photo-facts-value]"),
  ).toHaveText("f/2.8");
  await expect(
    facts.locator("[data-photo-facts-shutter-speed] [data-photo-facts-value]"),
  ).toHaveText("1/125 s");
  await expect(
    facts.locator("[data-photo-facts-iso] [data-photo-facts-value]"),
  ).toHaveText("ISO 400");
  await expect(
    facts.locator("[data-photo-facts-focal-length] [data-photo-facts-value]"),
  ).toHaveText("50 mm");
  await expect(facts).not.toContainText("Available");
  await expect(facts).not.toContainText("Preview");

  await openPhotoToolsView(page, "details");
  await expect(page.locator("[data-metadata]")).toContainText("Details");
  // Capture Time is camera-local time with no timezone. The display keeps
  // the recorded date and minute and drops transport precision.
  await expect(page.locator("[data-metadata-capture-time]")).toHaveText(
    "2026-02-03 04:05",
  );
  await expect(page.locator("[data-metadata-aperture]")).toHaveText("f/2.8");
  await expect(page.locator("[data-metadata-iso]")).toHaveText("400");
  await expect(page.locator("[data-metadata-shutter-speed]")).toHaveText(
    "1/125 s",
  );
  await expect(page.locator("[data-metadata-focal-length]")).toHaveText(
    "50 mm",
  );
});

test("Photo View clears capture facts on navigation and dashes each missing value", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  let requests = 0;
  await page.route("**/api/photos/*/metadata", (route) => {
    requests += 1;
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(
        requests === 1
          ? {
              captureTime: "2026-02-03T04:05:06.000000000",
              aperture: "f/2.8",
            }
          : {},
      ),
    });
  });
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  const facts = page.locator("[data-photo-facts-line]");
  await expect(
    facts.locator("[data-photo-facts-capture] [data-photo-facts-value]"),
  ).toHaveText("2026-02-03 04:05");
  await expect(
    facts.locator("[data-photo-facts-aperture] [data-photo-facts-value]"),
  ).toHaveText("f/2.8");
  for (const selector of [
    "[data-photo-facts-shutter-speed] [data-photo-facts-value]",
    "[data-photo-facts-iso] [data-photo-facts-value]",
    "[data-photo-facts-focal-length] [data-photo-facts-value]",
  ])
    await expect(facts.locator(selector)).toHaveText("—");

  await page.keyboard.press("ArrowRight");
  await expect(page.locator("[data-position]")).toHaveText("2 / 2");
  for (const selector of [
    "[data-photo-facts-capture] [data-photo-facts-value]",
    "[data-photo-facts-aperture] [data-photo-facts-value]",
    "[data-photo-facts-shutter-speed] [data-photo-facts-value]",
    "[data-photo-facts-iso] [data-photo-facts-value]",
    "[data-photo-facts-focal-length] [data-photo-facts-value]",
  ])
    await expect(facts.locator(selector)).toHaveText("—");
  await expect(facts).not.toContainText("f/2.8");
});

test("short landscape keeps capture facts and the complete Rating value", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  await page.route("**/api/photos/*/metadata", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        captureTime: "2026-02-03T04:05:06.000000000",
        aperture: "f/2.8",
        iso: 400,
        shutterSpeed: "1/125 s",
        focalLength: "50 mm",
      }),
    }),
  );
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  const facts = page.locator("[data-photo-facts-line]");
  for (const { width, height } of [
    { width: 844, height: 390 },
    { width: 667, height: 375 },
  ]) {
    await page.setViewportSize({ width, height });
    await waitForLoadedReviewImage(page);
    await expect(facts).toBeVisible();
    for (const [selector, value] of [
      [
        "[data-photo-facts-capture] [data-photo-facts-value]",
        "2026-02-03 04:05",
      ],
      ["[data-photo-facts-aperture] [data-photo-facts-value]", "f/2.8"],
      ["[data-photo-facts-shutter-speed] [data-photo-facts-value]", "1/125 s"],
      ["[data-photo-facts-iso] [data-photo-facts-value]", "ISO 400"],
      ["[data-photo-facts-focal-length] [data-photo-facts-value]", "50 mm"],
    ] as const)
      await expect(facts.locator(selector)).toHaveText(value);
    await expect(page.locator("[data-dock-rating-label]")).toHaveText(
      "No Rating",
    );
    await expectControlsAboveSafeViewport(page, height);
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
  }
});
