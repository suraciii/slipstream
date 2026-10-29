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
  waitForGridFrame,
  zoomLevel,
} from "./browser-test-support/surfaces.js";
import {
  expectControlsAboveSafeViewport,
  expectNoTextRangeCollision,
} from "./browser-test-support/review-geometry.js";

setupBrowserSmoke();

test("Library status opens in Photo View without disturbing the review or Grid anchor", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 40);
  const running = await server(base, root);
  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 40 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  const viewport = page.locator("[data-grid-viewport]");
  await viewport.focus();
  await page.keyboard.press("ArrowRight");
  for (let step = 0; step < 4; step += 1)
    await page.keyboard.press("ArrowDown");
  const columns = Number(await viewport.getAttribute("data-columns"));
  const expectedFocusedIndex = 4 * columns;
  await expect(
    page.locator(`[data-photo-index="${expectedFocusedIndex}"]`),
  ).toBeFocused();
  const focusedIndex = String(expectedFocusedIndex);
  const focusedPhoto = page.locator(`[data-photo-index="${focusedIndex}"]`);
  expect(
    await viewport.evaluate((element) => element.scrollTop),
  ).toBeGreaterThan(0);
  const gridAnchor = await viewport.evaluate((element) => {
    const top = element.getBoundingClientRect().top;
    const cell = Array.from(
      element.querySelectorAll<HTMLElement>(".photo-cell"),
    ).find((candidate) => candidate.getBoundingClientRect().bottom > top);
    if (!cell || !cell.dataset.photoIndex)
      throw new Error("Visible Grid anchor is missing");
    return {
      index: cell.dataset.photoIndex,
      offset: cell.getBoundingClientRect().top - top,
    };
  });
  await page.keyboard.press("Enter");
  await waitForLoadedReviewImage(page);

  const image = page.locator("[data-stage] img");
  const box = await image.boundingBox();
  if (!box) throw new Error("Rendered image has no geometry");
  const initialZoom = await zoomLevel(page);
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, -120);
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "manual",
  );
  const zoom = await zoomLevel(page);
  expect(zoom).toBeGreaterThan(initialZoom);
  const photo = await image.getAttribute("src");
  const position = await page.locator("[data-position]").textContent();
  const navigation = await page.evaluate(() => ({
    url: location.href,
    length: history.length,
    state: history.state as unknown,
  }));

  // Keep the real Library and its publication, but make the monitor report a
  // failed check so the shared notice is available while reviewing this Photo.
  await page.route("**/api/status", async (route) => {
    const response = await route.fetch();
    const status = (await response.json()) as Record<string, unknown>;
    await route.fulfill({ response, json: { ...status, state: "failed" } });
  });
  const entry = page.locator("[data-library-status-entry]");
  const toggle = entry.locator("[data-library-status-toggle]");
  const notice = entry.locator("[data-library-status-notice]");
  await expect(entry).toBeVisible();
  await expect(toggle).toHaveAccessibleName("Library status");
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(notice).toBeHidden();
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(notice).toBeVisible();
  await expect(notice).toContainText("Library check failed");
  await expect(page.locator("[data-review]")).toBeVisible();
  await expect(image).toHaveAttribute("src", photo!);
  await expect(page.locator("[data-position]")).toHaveText(position!);
  await expect(page.locator("[data-preview]")).toHaveAttribute(
    "data-zoom-state",
    "manual",
  );
  expect(await zoomLevel(page)).toBe(zoom);
  expect(
    await page.evaluate(() => ({
      url: location.href,
      length: history.length,
      state: history.state as unknown,
    })),
  ).toEqual(navigation);

  await page.getByRole("button", { name: "Back to Grid", exact: true }).click();
  await expect(focusedPhoto).toBeFocused();
  const anchorCell = page.locator(`[data-photo-index="${gridAnchor.index}"]`);
  await expect(anchorCell).toBeVisible();
  await expect
    .poll(async () => {
      const offset = await anchorCell.evaluate((cell) => {
        const viewport = cell.closest<HTMLElement>("[data-grid-viewport]");
        if (!viewport) throw new Error("Grid viewport is missing");
        return (
          cell.getBoundingClientRect().top -
          viewport.getBoundingClientRect().top
        );
      });
      return Math.abs(offset - gridAnchor.offset);
    })
    .toBeLessThan(3);
});

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
  await startReview(page, running.url, "All Photos");
  await page.setViewportSize({ width: 375, height: 667 });
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
  await expect(page.locator("[data-preview-ambient]")).toBeHidden();
  await page.locator("[data-dock-more]").click();
  const recoveryEntry = page
    .locator("[data-photo-tools-entries]")
    .getByRole("button", { name: "Review unavailable Originals", exact: true });
  await expect(recoveryEntry).toBeVisible();
  await recoveryEntry.click();
  await expect(page.locator("[data-recovery-panel]")).toBeVisible();
  await page.locator("[data-recovery-close]").click();
  await expect(page.locator("[data-recovery-panel]")).toBeHidden();
  await expect(page.locator("[data-photo-tools]")).toBeVisible();
  await expect(recoveryEntry).toBeFocused();
  await closePhotoTools(page);
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

test("390 by 844 keeps every primary control above the safe viewport beside a 650px Preview", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
  await expect(page.locator("[data-position]")).toHaveText("1 / 2");

  const preview = (await page.locator("[data-preview]").boundingBox())!;
  expect(preview.height).toBeGreaterThanOrEqual(650);
  await expectControlsAboveSafeViewport(page, 844);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

test("844 by 390 leaves no text range collision in the review strip", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await page.setViewportSize({ width: 844, height: 390 });
  await waitForLoadedReviewImage(page);
  await expect(page.locator("[data-position]")).toHaveText("1 / 2");
  await expectNoTextRangeCollision(page);
  await expectControlsAboveSafeViewport(page, 390);
});

test("gesture candidates name their effect only while active and tools retain local focus", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await startReview(page, running.url, "All Photos");
  await waitForLoadedReviewImage(page);
  const feedback = page.locator(".swipe-feedback:visible");
  await expect(feedback).toHaveCount(0);
  for (const pointerType of ["touch", "mouse", "pen"]) {
    for (const [dx, dy, label] of [
      [90, 0, "Pick"],
      [-90, 0, "Reject"],
      [0, -90, "Next"],
      [0, 90, "Previous"],
    ] as const) {
      await page.locator("[data-stage] img").evaluate(
        (image, args) => {
          const box = image.getBoundingClientRect();
          const init = {
            bubbles: true,
            pointerId: 1,
            pointerType: args.pointerType,
            isPrimary: true,
            clientX: box.x + box.width / 2,
            clientY: box.y + box.height / 2,
          };
          image.dispatchEvent(new PointerEvent("pointerdown", init));
          image.dispatchEvent(
            new PointerEvent("pointermove", {
              ...init,
              clientX: init.clientX + args.dx,
              clientY: init.clientY + args.dy,
            }),
          );
        },
        { dx, dy, pointerType },
      );
      await expect(feedback).toHaveText(label);
      await page.locator("[data-preview]").dispatchEvent("pointercancel", {
        pointerId: 1,
        pointerType,
        isPrimary: true,
      });
      await expect(feedback).toHaveCount(0);
      await expect(page.locator("[data-position]")).toHaveText("1 / 2");
      await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
    }
  }
  for (const trigger of ["keyboard", "wheel"]) {
    await page.keyboard.press("f");
    await page.locator("[data-stage] img").evaluate((image) => {
      const rect = image.getBoundingClientRect();
      const init = {
        bubbles: true,
        pointerId: 1,
        pointerType: "mouse",
        isPrimary: true,
        clientX: rect.x + rect.width / 2,
        clientY: rect.y + rect.height / 2,
      };
      image.dispatchEvent(new PointerEvent("pointerdown", init));
      image.dispatchEvent(
        new PointerEvent("pointermove", {
          ...init,
          clientX: init.clientX + 90,
        }),
      );
    });
    await expect(feedback).toHaveText("Pick");
    if (trigger === "keyboard") await page.keyboard.press("d");
    else
      await page
        .locator("[data-preview]")
        .dispatchEvent("wheel", { deltaY: -100 });
    await expect(page.locator("[data-preview]")).toHaveAttribute(
      "data-zoom-state",
      "manual",
    );
    await expect(feedback).toHaveCount(0);
    await page
      .locator("[data-preview]")
      .dispatchEvent("pointerup", { pointerId: 1 });
    await expect(page.locator("[data-position]")).toHaveText("1 / 2");
    await expect(page.locator("[data-selection]")).toHaveText("Unflagged");
  }
  await page.keyboard.press("f");
  await expect(page.locator("[data-preview]")).toHaveAccessibleDescription(
    /Preview detail is limited/,
  );
  const url = page.url();
  await page.locator("[data-dock-more]").click();
  const tools = page.locator("[data-photo-tools-view=tools]");
  await expect(
    tools.getByRole("region", { name: "Review", exact: true }),
  ).toBeVisible();
  await expect(
    tools.getByRole("region", { name: "Photo", exact: true }),
  ).toBeVisible();
  await expect(
    tools.getByRole("region", { name: "Library", exact: true }),
  ).toBeVisible();
  await tools.locator("summary").click();
  await expect(page.locator("[data-preview-help]")).toBeVisible();
  await tools.locator("[data-photo-tools-entry=details]").click();
  await expect(page.locator("[data-photo-tools] h2:visible")).toHaveText(
    "Details",
  );
  await expect(
    page.locator("[data-photo-tools-view=details] h3:visible"),
  ).toHaveCount(0);
  await page
    .locator("[data-photo-tools-view=details] [data-photo-tools-return]")
    .click();
  await expect(tools).toBeVisible();
  expect(page.url()).toBe(url);
  await page.keyboard.press("Escape");
  await expect(page.locator("[data-dock-more]")).toBeFocused();
});
