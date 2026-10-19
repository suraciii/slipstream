import { expect, test } from "@playwright/test";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  setupBrowserSmoke,
  servers,
  jpeg,
  fixture,
  writePhotos,
  server,
  createAlbum,
} from "./browser-test-support/fixtures.js";
import {
  actionWithProgress,
  openPhotoAndWaitForProgress,
  recordWindowRequests,
  waitForLoadedReviewImage,
  previewImageGeometry,
  previewStageGeometry,
  zoomLevel,
  waitForStageAtRest,
  startReview,
  closePhotoSurfaces,
  openSources,
  openPhotoToolsView,
  returnToPhotoTools,
  closePhotoTools,
  openRatingChoices,
  filmstripIndices,
  waitForFilmstripImages,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

test("starts from a Album, shows facts, accessible controls, and resumes persisted progress after restart", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg());
  await writeFile(join(root, "b.jpg"), await jpeg());
  let running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Picks");
  await startReview(page, running.url, "Picks", albumId);
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
  await expect(
    page.getByRole("button", { name: "Back to Grid", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "More", exact: true }),
  ).toBeVisible();
  await openPhotoToolsView(page, "details");
  await expect(
    page.getByText("JPEG · limited detail", { exact: true }),
  ).toBeVisible();
  await expect(page.locator("[data-source]")).toHaveAttribute(
    "title",
    "Limited by camera Preview resolution",
  );
  await expect(page.locator("[data-detail-limit]")).toBeVisible();
  await openPhotoToolsView(page, "zoom");
  for (const name of [
    "Fit Window",
    "Zoom in",
    "Zoom out",
    "Zoom to 100 percent",
  ])
    await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
  await returnToPhotoTools(page);
  await expect(
    page.getByRole("button", { name: "Clear flag", exact: true }),
  ).toBeVisible();
  await expect(
    page
      .locator("[data-photo-tools]")
      .getByRole("button", { name: "Undo", exact: true }),
  ).toHaveCount(0);
  await openRatingChoices(page);
  await expect(
    page.getByRole("button", { name: "Rate 5 stars", exact: true }),
  ).toBeVisible();
  // The decision acts on the Photo, so both surfaces close for it.
  await closePhotoSurfaces(page);
  await closePhotoTools(page);
  await page.locator("[data-dock-select]").click();
  await expect(page.locator("[data-selection]")).toHaveText("Picked");
  await expect(page.getByText("1 / 2")).toBeVisible();
  await page
    .locator("[data-photo-navigation]")
    .getByRole("button", { name: "Next", exact: true })
    .click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  // More review actions change state in place; explicit Next advances the
  // Album position that the restart below verifies.

  await page.goto("about:blank");
  await running.close();
  servers.splice(servers.indexOf(running), 1);
  running = await server(base, root);
  await page.goto(running.url);
  await openSources(page);
  await page.getByRole("link", { name: /^Picks \d+ Photos/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 2 of 2/ }),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  await actionWithProgress(page, albumId, () =>
    page.keyboard.press("ArrowLeft"),
  );
  await expect(page.locator("[data-selection]")).toHaveText("Picked");
});

test("Photo View gestures navigate and decide while More owns one review sheet", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 3);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  for (const selector of ["[data-dock-previous]", "[data-dock-next]"])
    await expect(page.locator(selector)).toBeVisible();
  await expect(page.locator("[data-dock-previous]")).toBeDisabled();
  await expect(page.locator("[data-dock-next]")).toBeEnabled();
  await expect(page.locator("[data-rating]")).toHaveText(
    "Rating: 0 stars (no Rating)",
  );
  await expect(page.locator("[data-dock-rating]")).toHaveAttribute(
    "aria-controls",
    "rating-choices",
  );
  await expect(page.locator("[data-dock-rating]")).toHaveAttribute(
    "aria-label",
    "Rating, 0 stars; open Rating choices",
  );
  for (const name of ["Reject", "Select", "Undo", "More"])
    await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Status and Recovery" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Library management", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Back to Grid" }),
  ).toBeVisible();
  await page
    .locator("[data-photo-navigation]")
    .getByRole("button", { name: "Next", exact: true })
    .click();
  await expect(page.locator("[data-position]")).toHaveText("2 / 3");
  await expect(page.locator("[data-dock-previous]")).toBeEnabled();
  await expect(page.locator("[data-dock-next]")).toBeEnabled();
  await page
    .locator("[data-photo-navigation]")
    .getByRole("button", { name: "Previous", exact: true })
    .click();
  await expect(page.locator("[data-position]")).toHaveText("1 / 3");
  await page.getByRole("button", { name: "More", exact: true }).click();
  const tools = page.locator("[data-photo-tools]");
  await expect(
    tools.getByRole("button", { name: "Clear flag", exact: true }),
  ).toBeVisible();
  await expect(
    tools.getByRole("button", { name: "Select", exact: true }),
  ).toHaveCount(0);
  await expect(
    tools.getByRole("button", { name: "Next", exact: true }),
  ).toHaveCount(0);
  await page.locator("[data-photo-tools-close]").click();
  await page.locator("[data-dock-next]").click();
  await expect(page.locator("[data-position]")).toHaveText("2 / 3");
  await page.locator("[data-dock-previous]").click();
  await expect(page.locator("[data-position]")).toHaveText("1 / 3");
  await page.getByRole("button", { name: "More", exact: true }).click();
  await page.getByRole("button", { name: "Rating", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Rate 5 stars", exact: true }),
  ).toBeVisible();
  await expect(page.locator("#rating-choices")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Clear Rating, 0 stars" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    page.getByRole("button", { name: "Rate 5 stars" }),
  ).toHaveAttribute("aria-pressed", "false");
  await returnToPhotoTools(page);
  await closePhotoTools(page);
  await page
    .getByRole("button", { name: "Library management", exact: true })
    .click();
  await expect(page.locator("[data-view-options]")).toBeVisible();
  await expect(page.locator("[data-removed-open]")).toBeVisible();
  await page.locator("[data-view-options-close]").click();
  await expect(page.locator("[data-view-options]")).toBeHidden();
  await expect(
    page.getByRole("button", { name: "Library management", exact: true }),
  ).toBeFocused();
  await returnToPhotoTools(page);
  await closePhotoTools(page);

  const swipe = async (dx: number, dy: number) => {
    const box = await page.locator("[data-preview]").boundingBox();
    if (!box) throw new Error("Preview has no geometry");
    const x = box.x + box.width / 2;
    const y = box.y + box.height / 2;
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x + dx, y + dy, { steps: 4 });
    await page.mouse.up();
  };

  await swipe(0, -140);
  await expect(page.locator("[data-position]")).toHaveText("2 / 3");
  await waitForLoadedReviewImage(page);
  await expect(page.locator("[data-dock-select]")).toBeEnabled();
  const picked = page.waitForRequest(
    (request) =>
      request.method() === "POST" &&
      /\/state(?:\?|$)/.test(new URL(request.url()).pathname),
  );
  await swipe(140, 0);
  expect((await picked).postDataJSON()).toMatchObject({
    field: "selectionState",
    value: "picked",
  });
  await expect(page.locator("[data-position]")).toHaveText("3 / 3");
  await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
  await waitForLoadedReviewImage(page);
  const rejected = page.waitForRequest(
    (request) =>
      request.method() === "POST" &&
      /\/state(?:\?|$)/.test(new URL(request.url()).pathname),
  );
  await swipe(-140, 0);
  expect((await rejected).postDataJSON()).toMatchObject({
    field: "selectionState",
    value: "rejected",
  });
  await expect(page.locator("[data-position]")).toHaveText("3 / 3");
  await expect(page.locator("[data-selection]")).toHaveText("Rejected");
});

test.describe("touch Preview input", () => {
  test.use({ hasTouch: true });

  test("touch upward swipe navigates to the next Photo in fit Preview", async ({
    page,
  }) => {
    const { base, root } = await fixture();
    await writePhotos(root, 2);
    const running = await server(base, root);
    await page.setViewportSize({ width: 390, height: 844 });
    await startReview(page, running.url, "All Photos");
    await waitForLoadedReviewImage(page);
    const stage = page.locator("[data-stage]");
    const image = page.locator("[data-stage] img");
    await expect(image).toHaveCSS("touch-action", "none");
    const box = await stage.boundingBox();
    if (!box) throw new Error("Preview stage has no geometry");
    const x = box.x + box.width / 2;
    const y = box.y + box.height / 2;
    const client = await page.context().newCDPSession(page);
    await client.send("Input.dispatchTouchEvent", {
      type: "touchStart",
      touchPoints: [{ x, y, id: 1 }],
    });
    for (const offset of [-35, -75, -140])
      await client.send("Input.dispatchTouchEvent", {
        type: "touchMove",
        touchPoints: [{ x, y: y + offset, id: 1 }],
      });
    await client.send("Input.dispatchTouchEvent", {
      type: "touchEnd",
      touchPoints: [],
    });
    await expect(page.locator("[data-position]")).toHaveText("2 / 2");
    await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
  });

  test("touch direction mapping separates Previous from decisions", async ({
    page,
  }) => {
    const { base, root } = await fixture();
    await writePhotos(root, 3);
    const running = await server(base, root);
    await page.setViewportSize({ width: 390, height: 844 });
    await startReview(page, running.url, "All Photos");
    await waitForLoadedReviewImage(page);
    const image = page.locator("[data-stage] img");
    const box = await image.boundingBox();
    if (!box) throw new Error("Rendered image has no geometry");
    const client = await page.context().newCDPSession(page);
    const touch = async (dx: number, dy: number) => {
      const x = box.x + box.width / 2;
      const y = box.y + box.height / 2;
      await client.send("Input.dispatchTouchEvent", {
        type: "touchStart",
        touchPoints: [{ x, y, id: 1 }],
      });
      for (const step of [0.25, 0.6, 1])
        await client.send("Input.dispatchTouchEvent", {
          type: "touchMove",
          touchPoints: [{ x: x + dx * step, y: y + dy * step, id: 1 }],
        });
      await client.send("Input.dispatchTouchEvent", {
        type: "touchEnd",
        touchPoints: [],
      });
    };
    await touch(0, -140);
    await expect(page.locator("[data-position]")).toHaveText("2 / 3");
    await waitForLoadedReviewImage(page);
    await touch(0, 140);
    await expect(page.locator("[data-position]")).toHaveText("1 / 3");
    await waitForLoadedReviewImage(page);
    const state = page.waitForRequest(
      (request) =>
        request.method() === "POST" &&
        /\/state(?:\?|$)/.test(new URL(request.url()).pathname),
    );
    await touch(140, 0);
    expect((await state).postDataJSON()).toMatchObject({
      field: "selectionState",
      value: "picked",
    });
    await expect(page.locator("[data-position]")).toHaveText("2 / 3");
    await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
  });

  test("touch from fit Preview letterbox scrolls without a decision", async ({
    page,
  }) => {
    const { base, root } = await fixture();
    await writePhotos(root, 1);
    const running = await server(base, root);
    await page.setViewportSize({ width: 390, height: 844 });
    await startReview(page, running.url, "All Photos");
    await waitForLoadedReviewImage(page);

    const geometry = await page.evaluate(() => {
      const stage = document.querySelector<HTMLElement>("[data-stage]");
      const image =
        document.querySelector<HTMLImageElement>("[data-stage] img");
      const view = document.querySelector<HTMLElement>("[data-photo-view]");
      if (!stage || !image || !view) throw new Error("Photo is missing");
      const stageBox = stage.getBoundingClientRect();
      const imageBox = image.getBoundingClientRect();
      const topGap = imageBox.top - stageBox.top;
      const bottomGap = stageBox.bottom - imageBox.bottom;
      if (topGap < 8 && bottomGap < 8)
        throw new Error("Fit Preview has no letterbox area");
      const y = topGap >= 8 ? stageBox.top + 4 : stageBox.bottom - 4;
      return {
        x: stageBox.left + stageBox.width / 2,
        y,
        before: view.scrollTop,
        scrollable: view.scrollHeight > view.clientHeight,
      };
    });
    expect(geometry.scrollable).toBe(true);
    let stateRequests = 0;
    page.on("request", (request) => {
      if (
        request.method() === "POST" &&
        new URL(request.url()).pathname.endsWith("/state")
      )
        stateRequests += 1;
    });
    const client = await page.context().newCDPSession(page);
    await client.send("Input.dispatchTouchEvent", {
      type: "touchStart",
      touchPoints: [{ x: geometry.x, y: geometry.y, id: 1 }],
    });
    for (const offset of [-35, -75, -140])
      await client.send("Input.dispatchTouchEvent", {
        type: "touchMove",
        touchPoints: [{ x: geometry.x, y: geometry.y + offset, id: 1 }],
      });
    await client.send("Input.dispatchTouchEvent", {
      type: "touchEnd",
      touchPoints: [],
    });
    await expect
      .poll(() =>
        page
          .locator("[data-photo-view]")
          .evaluate((element) => element.scrollTop),
      )
      .toBeGreaterThan(geometry.before);
    await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
    expect(stateRequests).toBe(0);
  });
});

test("Preview zoom is explicit, bounded, and never records a decision", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);

  const preview = page.locator("[data-preview]");
  await openPhotoToolsView(page, "zoom");
  const fit = page.getByRole("button", { name: "Fit Window", exact: true });
  const zoomIn = page.getByRole("button", { name: "Zoom in", exact: true });
  const hundred = page.getByRole("button", {
    name: "Zoom to 100 percent",
    exact: true,
  });
  const slider = page.locator("[data-zoom-slider]");
  const level = page.locator("[data-zoom-level]");

  // Fit is the default state, reports the percentage it produces, and keeps
  // the complete composition inside the Preview area.
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await expect(fit).toHaveAttribute("aria-pressed", "true");
  await expect(preview).toHaveCSS("touch-action", "pan-y");
  const stage = await previewStageGeometry(page);
  const fitted = await previewImageGeometry(page);
  expect(fitted.width).toBeLessThanOrEqual(stage.width + 0.5);
  expect(fitted.height).toBeLessThanOrEqual(stage.height + 0.5);
  expect(
    Math.abs(
      fitted.width / fitted.naturalWidth - fitted.height / fitted.naturalHeight,
    ),
  ).toBeLessThan(0.01);
  await expect(level).toHaveText(
    `${Math.round((fitted.width / fitted.naturalWidth) * 100)}%`,
  );

  // The zoom in control changes the real display size by one step.
  await zoomIn.click();
  await expect(preview).toHaveAttribute("data-zoom-state", "manual");
  const stepped = await previewImageGeometry(page);
  expect(stepped.width).toBeCloseTo(fitted.width * 1.25, 0);
  expect(await zoomLevel(page)).toBe(
    Math.round((stepped.width / stepped.naturalWidth) * 100),
  );

  // The slider changes the display size and the percentage together.
  await slider.fill("300");
  await slider.dispatchEvent("input");
  const slid = await previewImageGeometry(page);
  expect(slid.width).toBeCloseTo(slid.naturalWidth * 3, 0);
  expect(await zoomLevel(page)).toBe(300);

  // 100% maps one Preview pixel to one CSS pixel.
  await hundred.click();
  const actual = await previewImageGeometry(page);
  expect(actual.width).toBeCloseTo(actual.naturalWidth, 0);
  expect(actual.height).toBeCloseTo(actual.naturalHeight, 0);
  expect(await zoomLevel(page)).toBe(100);

  // Fit Window restores the complete composition.
  await fit.click();
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await expect(fit).toHaveAttribute("aria-pressed", "true");
  const refitted = await previewImageGeometry(page);
  expect(refitted.width).toBeLessThanOrEqual(stage.width + 0.5);
  expect(refitted.height).toBeLessThanOrEqual(stage.height + 0.5);

  // A zoomed drag pans instead of deciding, and the pan is bounded.
  let stateRequests = 0;
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname.endsWith("/state")
    )
      stateRequests += 1;
  });
  await slider.fill("300");
  await slider.dispatchEvent("input");
  // The Zoom controls live in Photo tools, so the surface closes for the
  // Preview gestures it would otherwise cover.
  await closePhotoTools(page);
  await waitForStageAtRest(page);
  const beforePan = await previewImageGeometry(page);
  const center = await preview.evaluate((surface) => {
    const box = surface.getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
  });
  await page.mouse.move(center.x, center.y);
  await page.mouse.down();
  await page.mouse.move(center.x + 60, center.y + 10);
  await page.mouse.up();
  const afterPan = await previewImageGeometry(page);
  const limitX = Math.max(0, (beforePan.width - stage.width) / 2);
  const limitY = Math.max(0, (beforePan.height - stage.height) / 2);
  expect(afterPan.left - beforePan.left).toBeCloseTo(Math.min(limitX, 60), 0);
  expect(afterPan.top - beforePan.top).toBeCloseTo(Math.min(limitY, 10), 0);
  expect(stateRequests).toBe(0);
  await expect(page.getByText("1 / 2")).toBeVisible();

  // Dragging far cannot pull the image out of view.
  await page.mouse.move(center.x, center.y);
  await page.mouse.down();
  await page.mouse.move(center.x + 900, center.y + 900);
  await page.mouse.up();
  const bounded = await previewImageGeometry(page);
  if (bounded.width > stage.width) {
    expect(bounded.left).toBeLessThanOrEqual(stage.left + 0.5);
    expect(bounded.right).toBeGreaterThanOrEqual(stage.right - 0.5);
  } else {
    expect(bounded.left + bounded.width / 2).toBeCloseTo(
      stage.left + stage.width / 2,
      0,
    );
  }
  if (bounded.height > stage.height) {
    expect(bounded.top).toBeLessThanOrEqual(stage.top + 0.5);
    expect(bounded.bottom).toBeGreaterThanOrEqual(stage.bottom - 0.5);
  } else {
    expect(bounded.top + bounded.height / 2).toBeCloseTo(
      stage.top + stage.height / 2,
      0,
    );
  }
  expect(stateRequests).toBe(0);

  // The keyboard reaches Fit, detail zoom, and stepped zoom.
  await page.keyboard.press("f");
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await page.keyboard.press("d");
  await expect(preview).toHaveAttribute("data-zoom-state", "manual");
  expect(await zoomLevel(page)).toBe(200);
  await page.keyboard.press("-");
  expect(await zoomLevel(page)).toBe(160);
  await expect(preview).toHaveCSS("touch-action", "none");

  // No horizontal page overflow while zoomed.
  const layout = await page.locator("[data-photo-view]").evaluate((view) => {
    const previewBox = view.querySelector<HTMLElement>("[data-preview]");
    if (!previewBox) throw new Error("Preview is missing");
    return {
      viewWidth: view.clientWidth,
      viewScrollWidth: view.scrollWidth,
      previewWidth: previewBox.getBoundingClientRect().width,
    };
  });
  expect(layout.viewScrollWidth).toBe(layout.viewWidth);
  expect(layout.previewWidth).toBeLessThanOrEqual(layout.viewWidth);

  // Changing Photo resets the zoom state to Fit. Navigating is a background
  // action, so the disclosure closes for it.
  await closePhotoTools(page);
  await page.keyboard.press("ArrowRight");
  await expect(page.getByText("2 / 2")).toBeVisible();
  await waitForLoadedReviewImage(page);
  await expect(preview).toHaveAttribute("data-zoom-state", "fit");
  await openPhotoToolsView(page, "zoom");
  await expect(fit).toHaveAttribute("aria-pressed", "true");
  expect(stateRequests).toBe(0);
});

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

test("Photo View shows a bounded filmstrip of neighbors and navigates through it", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 6);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 6 Photos$/)).toBeVisible();

  await page.locator('[data-photo-index="2"]').click();
  await waitForLoadedReviewImage(page);
  await openPhotoToolsView(page, "nearby");
  const strip = page.locator("[data-filmstrip]");
  await expect(strip).toBeVisible();
  // Bounded and centered: two neighbors on each side of the current Photo.
  await expect(strip.locator(".filmstrip-cell")).toHaveCount(5);
  expect(await filmstripIndices(page)).toEqual([0, 1, 2, 3, 4]);
  await expect(strip.locator('[data-filmstrip-index="2"]')).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(
    strip.locator('.filmstrip-cell[aria-current="true"]'),
  ).toHaveCount(1);
  await waitForFilmstripImages(page);

  // Activating a neighbor navigates there without recording a decision, and
  // the strip re-centers on the new current Photo. The neighbor is already
  // in the loaded window, so the strip admits no window request of its own.
  const windows = recordWindowRequests(page);
  await strip.locator('[data-filmstrip-index="4"]').click();
  await expect(page.locator("[data-position]")).toHaveText("5 / 6");
  expect(windows.requested).toEqual([]);
  await expect(page.locator("[data-photo-filename]")).toHaveText("004.jpg");
  await waitForLoadedReviewImage(page);
  await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "fit",
  );
  // The source's end clamps the strip instead of wrapping around it.
  expect(await filmstripIndices(page)).toEqual([2, 3, 4, 5]);
  await expect(strip.locator('[data-filmstrip-index="4"]')).toHaveAttribute(
    "aria-current",
    "true",
  );

  // The same path serves the boundary: an edge Photo keeps its clamp and
  // never wraps around the source.
  await strip.locator('[data-filmstrip-index="5"]').click();
  await expect(page.locator("[data-position]")).toHaveText("6 / 6");
  await waitForLoadedReviewImage(page);
  expect(await filmstripIndices(page)).toEqual([3, 4, 5]);
  await expect(strip.locator('[data-filmstrip-index="5"]')).toHaveAttribute(
    "aria-current",
    "true",
  );
});
