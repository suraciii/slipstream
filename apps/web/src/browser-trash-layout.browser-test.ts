import { expect, test } from "@playwright/test";
import {
  setupBrowserSmoke,
  fixture,
  writePhotos,
  server,
} from "./browser-test-support/fixtures.js";
import { openViewOptions } from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

for (const viewport of [
  { width: 375, height: 800 },
  { width: 390, height: 800 },
  { width: 1000, height: 800 },
  { width: 667, height: 375 },
]) {
  const { width, height } = viewport;
  test(`Trash keeps recovery controls reachable and cleanup consent separate at ${width}×${height}`, async ({
    page,
  }) => {
    const { base, root } = await fixture();
    await writePhotos(root, 1);
    const running = await server(base, root);
    const location =
      "2026/photography-session-with-a-long-directory-name/another-long-directory/photograph-with-a-long-filename.jpg";
    const rows = Array.from({ length: 20 }, (_, index) => ({
      photoId: `removed-${index}`,
      originalId: `original-${index}`,
      originalAvailable: true,
      state: "captured",
      removedAtMs: 1000 + index,
      originalLocation: location,
      originalKind: "jpeg",
      originalSize: 1024,
      photo: {
        id: `removed-${index}`,
        available: true,
        original: { kind: "jpeg", available: true },
        originalFilename: "photograph-with-a-long-filename.jpg",
        selectionState: "unflagged",
        rating: 0,
        hasSavedEdits: false,
        preview: { state: "unavailable" },
      },
    }));
    let cleanupRequests = 0;
    await page.route(/\/api\/trash(?:\?|\/|$)/, async (route) => {
      const pathname = new URL(route.request().url()).pathname;
      if (pathname === "/api/trash") {
        await route.fulfill({
          json: {
            start: 0,
            limit: 50,
            total: rows.length,
            operation: { operationId: "removal-1", removed: rows.length },
            cleanupMaximum: 5000,
            restoreMaximum: 100,
            evaluatedAt: "2026-01-01T00:00:00Z",
            expiresAt: null,
            nextCursor: null,
            libraryRoot: root,
            namespace: "service",
            photos: rows,
          },
        });
        return;
      }
      if (pathname === "/api/trash/cleanup-preview") {
        const body = route.request().postDataJSON() as {
          items: Array<{ photoId: string; originalLocation: string }>;
        };
        await route.fulfill({
          json: {
            items: body.items.map((item) => ({
              photoId: item.photoId,
              originalLocation: item.originalLocation,
              originalKind: "jpeg",
              state: "captured",
              absence: "absent",
              message: null,
              blockers: [],
            })),
          },
        });
        return;
      }
      if (pathname === "/api/trash/cleanup") {
        cleanupRequests += 1;
        await route.abort();
        return;
      }
      await route.continue();
    });
    await page.setViewportSize(viewport);
    await page.goto(running.url);
    await expect(page.locator("[data-grid-status]")).toHaveText(
      "Ready · 1 Photo",
    );
    await openViewOptions(page);
    await page.locator("[data-removed-open]").click();
    const panel = page.locator("[data-removed-panel]");
    const first = panel.locator(".removed-item").first();
    await expect(panel.locator(".removed-item")).toHaveCount(20);
    await expect(first.locator(".removed-thumbnail-placeholder")).toBeVisible();
    await expect(first.locator("img")).toBeHidden();
    await expect(
      first.getByRole("button", { name: "Restore", exact: true }),
    ).toBeVisible();
    await expect(first.locator(".removed-location")).toHaveText(location);
    await expect(first.locator(".removed-name")).toHaveText(
      "photograph-with-a-long-filename.jpg",
    );
    await expect(first.locator(".removed-when").first()).toContainText(
      "Removed",
    );
    await expect(first.locator(".removed-when").last()).toContainText(
      "Original: available",
    );
    await expect(first.getByText("Photo ID:", { exact: false })).toBeHidden();
    await first.getByText("Details", { exact: true }).click();
    await expect(first.getByText("Photo ID:", { exact: false })).toContainText(
      "removed-0",
    );
    await expect(first.getByText("Photo ID:", { exact: false })).toContainText(
      "original-0",
    );
    await first.getByText("Details", { exact: true }).click();
    await expect(
      panel.locator("[data-removed-restore-selection]"),
    ).toBeHidden();
    await expect(panel.locator("[data-removed-delete]")).toBeHidden();
    await expect(panel.locator("[data-trash-copy-locations]")).toBeEnabled();
    await expect(panel.locator("[data-removed-select-all]")).toBeVisible();
    await expect(panel.locator("[data-removed-undo]")).toBeVisible();
    await expect(panel.locator("[data-trash-library-folder]")).toContainText(
      root,
    );
    await expect(panel.locator("[data-trash-library-folder]")).toContainText(
      "Server/container filesystem namespace, not this device",
    );
    await first.getByRole("checkbox").check();
    await expect(
      panel.locator("[data-removed-restore-selection]"),
    ).toBeVisible();
    await expect(panel.locator("[data-removed-selection-count]")).toContainText(
      "1 selected",
    );
    const reviewEntry = panel.locator("[data-removed-delete]");
    if (height <= 480) {
      const restoreLast = panel
        .locator(".removed-item")
        .last()
        .getByRole("button", { name: "Restore", exact: true });
      await restoreLast.scrollIntoViewIfNeeded();
      await expect(restoreLast).toBeInViewport();
      const undo = panel.locator("[data-removed-undo]");
      await undo.scrollIntoViewIfNeeded();
      await expect(undo).toBeInViewport();
      await reviewEntry.scrollIntoViewIfNeeded();
      await expect(reviewEntry).toBeInViewport();
    } else {
      const initialControlTop = await reviewEntry.evaluate(
        (element) => element.getBoundingClientRect().top,
      );
      await panel.locator(".removed-scroll").evaluate((element) => {
        element.scrollTop = element.scrollHeight;
      });
      expect(
        await reviewEntry.evaluate(
          (element) => element.getBoundingClientRect().top,
        ),
      ).toBe(initialControlTop);
      await expect(reviewEntry).toBeInViewport();
      await expect(panel.locator("[data-removed-status]")).toBeInViewport();
      await expect(
        panel.locator("[data-removed-restore-selection]"),
      ).toBeInViewport();
    }
    expect(
      await panel.evaluate(
        (element) => element.scrollWidth <= element.clientWidth,
      ),
    ).toBe(true);
    if (width < 761) {
      expect(
        await panel
          .locator("button:visible")
          .evaluateAll((buttons) =>
            buttons.every(
              (button) => button.getBoundingClientRect().height >= 44,
            ),
          ),
      ).toBe(true);
      expect(
        await first
          .getByRole("checkbox")
          .evaluate((element) => element.getBoundingClientRect().height),
      ).toBeGreaterThanOrEqual(44);
    }
    await reviewEntry.click();
    const review = page.locator("[data-trash-review]");
    await expect(review).toBeVisible();
    await expect(review.locator("[data-trash-review-location]")).toHaveText(
      location,
    );
    await expect(
      review.locator("[data-trash-review-item-albums]"),
    ).toBeVisible();
    await expect(review.locator(".trash-review-warning")).toBeVisible();
    await expect(review.locator(".trash-review-warning")).toContainText(
      "cannot undo",
    );
    await expect(review.locator("[data-trash-cancel]")).toBeFocused();
    expect(
      await review.evaluate(
        (element) => element.scrollWidth <= element.clientWidth,
      ),
    ).toBe(true);
    expect(
      await review
        .locator("[data-trash-confirm]")
        .evaluate((element) => getComputedStyle(element).backgroundColor),
    ).not.toBe(
      await review
        .locator("[data-trash-cancel]")
        .evaluate((element) => getComputedStyle(element).backgroundColor),
    );
    expect(cleanupRequests).toBe(0);
    await review.locator("[data-trash-cancel]").click();
    await expect(review).toBeHidden();
    await expect(reviewEntry).toBeFocused();
    await expect(panel.locator(".removed-item")).toHaveCount(20);
    expect(cleanupRequests).toBe(0);
  });
}
