import { expect, test } from "@playwright/test";
import {
  fixture,
  server,
  setupBrowserSmoke,
  writePhotos,
} from "./browser-test-support/fixtures.js";
import {
  openDockSecondary,
  openPhotoToolsView,
  startReview,
  waitForLoadedReviewImage,
} from "./browser-test-support/surfaces.js";
import { expectControlsAboveSafeViewport } from "./browser-test-support/review-geometry.js";

setupBrowserSmoke();

test("Photo View shows review capture metadata in the facts line and Details", async ({
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
  await expect(page.locator("[data-details-identity]")).toContainText(
    "000.jpg",
  );
  await expect(page.locator("[data-details-identity]")).toContainText("1 / 1");
  await expect(page.locator("[data-metadata]")).toContainText("Capture facts");
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

test("Photo View clears capture facts on navigation and omits missing values", async ({
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
  // Missing facts are omitted from the line instead of shown as placeholders.
  for (const selector of [
    "[data-photo-facts-shutter-speed]",
    "[data-photo-facts-iso]",
    "[data-photo-facts-focal-length]",
  ])
    await expect(facts.locator(selector)).toBeHidden();

  await page.keyboard.press("ArrowRight");
  await expect(page.locator("[data-position]")).toHaveText("2 / 2");
  for (const selector of [
    "[data-photo-facts-capture]",
    "[data-photo-facts-aperture]",
    "[data-photo-facts-shutter-speed]",
    "[data-photo-facts-iso]",
    "[data-photo-facts-focal-length]",
  ])
    await expect(facts.locator(selector)).toBeHidden();
  // An empty capture set keeps the facts row and states the absence once,
  // rather than leaving an unexplained gap.
  await expect(facts).toBeVisible();
  await expect(page.locator("[data-photo-facts-empty]")).toHaveText(
    "No capture facts",
  );

  // Details groups a completely missing capture set into one clear state
  // instead of repeating five dash rows.
  await openPhotoToolsView(page, "details");
  await expect(page.locator("[data-metadata-empty]")).toBeVisible();
  for (const row of [
    "captureTime",
    "aperture",
    "iso",
    "shutterSpeed",
    "focalLength",
  ])
    await expect(page.locator(`[data-metadata-row='${row}']`)).toBeHidden();
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

test("enlarged text keeps complete capture facts and review actions reachable", async ({
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
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addStyleTag({ content: "html { font-size: 200% !important; }" });
  await waitForLoadedReviewImage(page);

  const label = page.locator("[data-dock-rating-label]");
  await expect(label).toHaveText("No Rating");
  const bounds = await page.evaluate(() => {
    const button = document.querySelector<HTMLElement>("[data-dock-rating]")!;
    const value = document.querySelector<HTMLElement>(
      "[data-dock-rating-label]",
    )!;
    const b = button.getBoundingClientRect();
    const v = value.getBoundingClientRect();
    return {
      overflow: document.documentElement.scrollWidth - innerWidth,
      inside:
        v.left >= b.left &&
        v.right <= b.right &&
        v.top >= b.top &&
        v.bottom <= b.bottom,
    };
  });
  expect(bounds.overflow).toBeLessThanOrEqual(0);
  expect(bounds.inside).toBe(true);

  await page
    .locator("[data-quick-action-dock]")
    .evaluate((element) => element.scrollIntoView({ block: "end" }));
  await openDockSecondary(page);
  for (const action of [
    "[data-dock-reject]",
    "[data-dock-rating]",
    "[data-dock-pick]",
    "[data-dock-undo]",
    "[data-dock-more]",
  ])
    await expect(page.locator(action)).toBeInViewport({ ratio: 1 });
});
