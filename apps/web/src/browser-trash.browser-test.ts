import { rm } from "node:fs/promises";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import {
  setupBrowserSmoke,
  fixture,
  writePhotos,
  server,
  browseIds,
} from "./browser-test-support/fixtures.js";
import {
  waitForGridFrame,
  openViewOptions,
  applyViewOptions,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

test("rejected Photos leave the Library, return from Undo, and are restored from the Trash listing", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 3);
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  expect(ids).toHaveLength(3);

  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 3 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  // Two Photos are rejected, and the Grid is filtered to that result. Removal
  // is reviewed against the Snapshot the Photographer actually sees.
  const cell = (index: number) => page.locator(`[data-photo-index="${index}"]`);
  await page.locator("[data-grid-viewport]").focus();
  await page.keyboard.press("ArrowRight");
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("x");
  await expect(cell(0).locator(".cell-state.rejected")).toHaveText("×");
  await expect(cell(1)).toBeEnabled();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("x");
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 picked · 2 rejected · 1 unflagged",
  );
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("rejected");
  await applyViewOptions(page);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 2 Photos",
  );

  // The review names the result it covers and removes nothing by itself.
  const review = page.locator("[data-removal-review]");
  await openViewOptions(page);
  await page.locator("[data-removal-open]").click();
  await expect(review).toBeVisible();
  await expect(page.locator("[data-removal-summary]")).toHaveText(
    /^2 Photos reviewed as Rejected\./,
  );
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 2 Photos",
  );

  // One confirmation removes exactly the reviewed result, and the Library
  // reads again: the rejected result is empty and the Overview count drops.
  await page.locator("[data-removal-confirm]").click();
  await expect(page.locator("[data-removal-message]")).toHaveText(
    /^2 Photos removed from the Library\. Their Original Files are unchanged\.$/,
  );
  // The filtered source is now empty, so the Grid explains the empty result
  // instead of the reopen that produced it.
  await expect(page.locator("[data-grid-status]")).toHaveText("0 Photos");
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 picked · 0 rejected · 1 unflagged",
  );
  await page.locator("[data-removal-close]").click();
  await expect(review).toBeHidden();
  await expect(page.locator("[data-grid-empty-message]")).toHaveText(
    "No Photos match this filter.",
  );
  // The Library Overview count is the committed count, not the filtered one.
  await expect(
    page.getByRole("link", { name: /^All Photos 1 Photo$/ }),
  ).toBeVisible();

  // The operation-level Undo is recovered from persisted removal state after
  // the page owner is recreated by a reload.
  await page.reload();
  await expect(
    page.getByRole("link", { name: /^All Photos 1 Photo$/ }),
  ).toBeVisible();
  await openViewOptions(page);
  await page.locator("[data-removed-open]").click();
  const removed = page.locator("[data-removed-panel]");
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "2 Photos in Trash. Showing 1–2.",
  );
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    2,
  );
  await expect(page.locator("[data-removed-undo]")).toHaveText(
    "Undo the last removal (2)",
  );
  await page.locator("[data-removed-undo]").click();
  await expect(page.locator("[data-removed-message]")).toHaveText(
    /^2 Photos restored to the Library\.$/,
  );
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    0,
  );
  await page.locator("[data-removed-close]").click();
  await expect(removed).toBeHidden();
  await expect(
    page.getByRole("link", { name: /^All Photos 3 Photos$/ }),
  ).toBeVisible();

  // Restored Photos keep the decisions they had before the removal.
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 picked · 2 rejected · 1 unflagged",
  );
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Source reopened after the restore.",
  );

  // Remove again, then restore one named Photo from the listing: the durable
  // per-Photo path survives a reload.
  await openViewOptions(page);
  await page.locator("[data-removal-open]").click();
  await expect(page.locator("[data-removal-summary]")).toHaveText(
    /^2 Photos reviewed as Rejected\./,
  );
  await page.locator("[data-removal-confirm]").click();
  await expect(page.locator("[data-removal-message]")).toHaveText(
    /^2 Photos removed from the Library\./,
  );
  await page.locator("[data-removal-close]").click();
  await openViewOptions(page);
  await page.locator("[data-removed-open]").click();
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    2,
  );
  const restoredRow = page.locator("[data-removed-list] .removed-item").first();
  const restoredName = await restoredRow.locator(".removed-name").innerText();
  await restoredRow.getByRole("button", { name: "Restore" }).click();
  await expect(page.locator("[data-removed-message]")).toHaveText(
    /^1 Photo restored to the Library\.$/,
  );
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    1,
  );
  await expect(
    page.locator("[data-removed-list] .removed-name"),
  ).not.toHaveText(restoredName);
  await page.reload();
  await expect(
    page.getByRole("link", { name: /^All Photos 2 Photos$/ }),
  ).toBeVisible();
  await openViewOptions(page);
  await page.locator("[data-removed-open]").click();
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    1,
  );
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "1 Photo in Trash. Showing 1–1.",
  );
  await expect(
    page.locator("[data-removed-list] .removed-name"),
  ).not.toHaveText(restoredName);
  // A stale listing marker is answered truthfully: the Photo remains removed
  // and the Photographer is told that its removal state changed elsewhere.
  await page.route("**/api/photos/restore", async (route) => {
    const body = JSON.parse(route.request().postData() ?? "{}") as {
      photos?: ReadonlyArray<{ id?: unknown }>;
    };
    const photoId = body.photos?.[0]?.id;
    if (typeof photoId !== "string")
      throw new Error("restore request did not name a Photo");
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        counts: { restored: 0, changedElsewhere: 1, missing: 0 },
        changedElsewhere: [photoId],
        missing: [],
        operations: [],
      }),
    });
  });
  await page
    .locator("[data-removed-list] .removed-item")
    .getByRole("button", { name: "Restore" })
    .click();
  await expect(page.locator("[data-removed-message]")).toHaveText(
    "Nothing was restored. 1 Photo could not be restored because their removal state changed elsewhere.",
  );
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    1,
  );
  await page.unroute("**/api/photos/restore");
});

test("Trash cleanup reviews records and recovers a lost mixed outcome", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);

  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      value: {
        writeText: (text: string) => {
          sessionStorage.setItem("test-copied-trash", text);
          return Promise.resolve();
        },
      },
    });
  });
  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 2 Photos$/)).toBeVisible();
  await waitForGridFrame(page);
  await page.locator("[data-grid-viewport]").focus();
  for (const index of [0, 1]) {
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("x");
    await expect(
      page.locator(`[data-photo-index="${index}"] .cell-state.rejected`),
    ).toHaveText("×");
  }
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("rejected");
  await applyViewOptions(page);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 2 Photos",
  );
  await openViewOptions(page);
  await page.locator("[data-removal-open]").click();
  await page.locator("[data-removal-confirm]").click();
  await expect(page.locator("[data-removal-message]")).toContainText(
    "2 Photos removed",
  );
  await page.locator("[data-removal-close]").click();

  // This is a disposable fixture; the filesystem owner removes one file
  // outside the application before explicitly requesting record cleanup.
  await rm(join(root, "000.jpg"));
  await openViewOptions(page);
  await page.locator("[data-removed-open]").click();
  const panel = page.locator("[data-removed-panel]");
  await expect(panel.locator("[data-removed-list] .removed-item")).toHaveCount(
    2,
  );
  await expect(panel.locator("[data-trash-library-folder]")).toContainText(
    root,
  );
  await expect(panel.locator("[data-trash-copy-locations]")).toHaveText(
    "Copy file locations",
  );
  await panel.locator("[data-removed-select-all]").click();
  await panel.locator("[data-trash-copy-locations]").click();
  const copied = await page.evaluate(() =>
    sessionStorage.getItem("test-copied-trash"),
  );
  expect(copied).toContain(root);
  expect(copied).toContain("service");
  expect(copied).toContain("000.jpg");
  expect(copied).toContain("001.jpg");

  const review = page.locator("[data-trash-review]");
  await panel.locator("[data-removed-delete]").click();
  await expect(review).toBeVisible();
  await expect(
    review.getByRole("heading", { name: "Clean up Trash records" }),
  ).toBeVisible();
  await expect(review).toContainText(/no files are deleted/i);
  await expect(review).toContainText(/cannot undo|cannot be undone/i);
  await expect(review.locator("[data-trash-review-location]")).toHaveCount(2);
  await expect(review.locator("[data-trash-confirm]")).toHaveText(
    "Clean up 2 Photo records",
  );
  await page.keyboard.press("Escape");
  await expect(review).toBeHidden();
  await expect(panel.locator("[data-removed-list] .removed-item")).toHaveCount(
    2,
  );

  let cleanupCalls = 0;
  let operationId = "";
  let reviewedLocations: string[] = [];
  const receipts: Array<{
    counts: { cleaned: number; present: number };
    items: Array<{ originalLocation: string; state: string }>;
  }> = [];
  await page.route("**/api/trash/cleanup", async (route) => {
    cleanupCalls += 1;
    const body = route.request().postDataJSON() as {
      operationId: string;
      items: Array<{
        photoId: string;
        removedAtMs: number;
        originalId: string;
        originalLocation: string;
      }>;
    };
    operationId = body.operationId;
    reviewedLocations = body.items.map((item) => item.originalLocation).sort();
    expect(reviewedLocations).toEqual(["000.jpg", "001.jpg"]);
    expect(
      body.items.every(
        (item) => item.photoId && item.originalId && item.removedAtMs >= 0,
      ),
    ).toBe(true);
    expect(
      await page.evaluate(
        (id) =>
          Array.from({ length: localStorage.length }, (_, index) =>
            localStorage.key(index),
          ).some(
            (key) =>
              key !== null && (localStorage.getItem(key) ?? "").includes(id),
          ),
        operationId,
      ),
    ).toBe(true);
    const response = await route.fetch();
    expect(response.ok()).toBe(true);
    receipts.push(
      (await response.json()) as {
        counts: { cleaned: number; present: number };
        items: Array<{ originalLocation: string; state: string }>;
      },
    );
    await route.abort("connectionreset");
  });
  await panel.locator("[data-removed-delete]").click();
  await review.locator("[data-trash-confirm]").click();
  await expect(page.locator("[data-trash-check-result]")).toBeVisible();
  expect(receipts[0]?.counts.cleaned).toBe(1);
  expect(receipts[0]?.counts.present).toBe(1);
  expect(
    receipts[0]?.items.find((item) => item.originalLocation === "000.jpg")
      ?.state,
  ).toBe("cleaned");
  expect(
    receipts[0]?.items.find((item) => item.originalLocation === "001.jpg")
      ?.state,
  ).toBe("present");

  // Refresh the listing without resolving the cleanup, then restore the last
  // remaining record. The unknown outcome must survive the empty list.
  await panel.locator("[data-removed-close]").click();
  await openViewOptions(page);
  await page.locator("[data-removed-open]").click();
  await expect(panel.locator(".removed-location")).toHaveText("001.jpg");
  await panel.getByRole("button", { name: "Restore", exact: true }).click();
  await expect(panel.locator("[data-removed-status]")).toHaveText(
    "Trash is empty.",
  );
  await expect(panel.locator("[data-trash-check-result]")).toBeVisible();
  await expect(panel.locator("[data-removed-close]")).toBeVisible();
  await expect(panel.locator("[data-removed-select-all]")).toBeHidden();
  await expect(panel.locator("[data-trash-copy-locations]")).toBeHidden();
  await expect(panel.locator("[data-removed-delete]")).toBeHidden();

  // Recovery reads the original operation and converges without another cleanup.
  await page.reload();
  await openViewOptions(page);
  await page.locator("[data-removed-open]").click();
  await expect(panel.locator("[data-removed-status]")).toHaveText(
    "Trash is empty.",
  );
  await expect(page.locator("[data-trash-outcome]")).toContainText(/present/i);
  await expect(page.locator("[data-trash-outcome]")).toBeInViewport();
  expect(cleanupCalls).toBe(1);
});

test("Trash cleanup displays a named retained-work blocker", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 1 Photo$/)).toBeVisible();
  await waitForGridFrame(page);
  await page.locator("[data-grid-viewport]").focus();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("x");
  await expect(
    page.locator('[data-photo-index="0"] .cell-state.rejected'),
  ).toHaveText("×");
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("rejected");
  await applyViewOptions(page);
  await openViewOptions(page);
  await page.locator("[data-removal-open]").click();
  await page.locator("[data-removal-confirm]").click();
  await expect(page.locator("[data-removal-message]")).toContainText(
    "1 Photo removed",
  );
  await page.locator("[data-removal-close]").click();

  // The transport fixture supplies a retained-work refusal; no cleanup is
  // forwarded, so the real Photo record remains available for Restore.
  await page.route("**/api/trash/cleanup", async (route) => {
    const body = route.request().postDataJSON() as {
      operationId: string;
      items: Array<{ photoId: string; originalLocation: string }>;
    };
    expect(body.items).toHaveLength(1);
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        operationId: body.operationId,
        reviewed: 1,
        counts: {
          cleaned: 0,
          changed: 0,
          present: 0,
          blocked: 1,
          unavailable: 0,
          inspectionFailed: 0,
        },
        items: [
          {
            photoId: body.items[0]!.photoId,
            originalLocation: body.items[0]!.originalLocation,
            originalKind: "jpeg",
            state: "blocked",
            message: null,
            blockers: [
              {
                kind: "workspace",
                name: "workspace-keeper",
                action:
                  "Remove this retained work before cleaning up the record.",
              },
            ],
          },
        ],
      }),
    });
  });
  await openViewOptions(page);
  await page.locator("[data-removed-open]").click();
  const panel = page.locator("[data-removed-panel]");
  await panel.locator("[data-removed-select-all]").click();
  await panel.locator("[data-removed-delete]").click();
  await page.locator("[data-trash-confirm]").click();
  await expect(page.locator("[data-trash-outcome]")).toContainText(/blocked/i);
  await expect(page.locator("[data-trash-outcome]")).toBeInViewport();
  await expect(page.locator("[data-trash-outcome]")).toContainText(
    "workspace-keeper",
  );
  await expect(page.locator("[data-trash-outcome]")).toContainText(
    "Remove this retained work before cleaning up the record.",
  );
  await expect(panel.locator("[data-removed-list] .removed-item")).toHaveCount(
    1,
  );
  await expect(
    panel.getByRole("button", { name: "Restore", exact: true }),
  ).toBeEnabled();
});

test("captured Trash selection follows a retained cursor and excludes a row", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 3);
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  expect(ids).toHaveLength(3);

  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      value: {
        writeText: (text: string) => {
          sessionStorage.setItem("test-copied-trash", text);
          return Promise.resolve();
        },
      },
    });
  });

  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 3 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  // All three Photos are removed into Trash through the reviewed removal.
  const cell = (index: number) => page.locator(`[data-photo-index="${index}"]`);
  await page.locator("[data-grid-viewport]").focus();
  for (const index of [0, 1, 2]) {
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("x");
    await expect(cell(index).locator(".cell-state.rejected")).toHaveText("×");
  }
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("rejected");
  await applyViewOptions(page);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 3 Photos",
  );
  await openViewOptions(page);
  await page.locator("[data-removal-open]").click();
  const removal = page.locator("[data-removal-review]");
  await expect(removal).toBeVisible();
  await removal.locator("[data-removal-confirm]").click();
  await expect(page.locator("[data-removal-message]")).toHaveText(
    /^3 Photos removed from the Library\./,
  );
  await removal.locator("[data-removal-close]").click();
  await expect(page.locator("[data-grid-status]")).toHaveText("0 Photos");

  // Split a disposable listing fixture over two cursor pages. A later live
  // offset read would include an arrival that was not in this capture.
  type ListingItem = {
    photoId: string;
    removedAtMs: number;
    originalId: string;
    originalLocation: string;
    originalKind: string;
    originalSize: number | null;
    originalAvailable: boolean;
    state: string;
    photo: Record<string, unknown>;
  };
  let captured: ListingItem[] = [];
  let firstPageReads = 0;
  const cursors: string[] = [];
  await page.route(/\/api\/trash(?:\?|$)/, async (route) => {
    const url = new URL(route.request().url());
    const cursor = url.searchParams.get("cursor");
    if (captured.length === 0) {
      const response = await route.fetch();
      const listing = (await response.json()) as { photos: ListingItem[] };
      const template = listing.photos[0]!;
      captured = Array.from({ length: 51 }, (_, index) => {
        const photoId = `captured-${index}`;
        return {
          ...template,
          photoId,
          originalId: `original-${index}`,
          originalLocation: `fixture/${index}.jpg`,
          photo: {
            ...template.photo,
            id: photoId,
            originalFilename: `${index}.jpg`,
          },
        };
      });
    }
    if (cursor) cursors.push(cursor);
    else firstPageReads += 1;
    const start = cursor ? 50 : 0;
    const rows = cursor
      ? captured.slice(50)
      : firstPageReads === 1
        ? captured.slice(0, 50)
        : [
            {
              ...captured[0]!,
              photoId: "later-arrival",
              originalId: "later-original",
              originalLocation: "fixture/later.jpg",
              photo: { ...captured[0]!.photo, id: "later-arrival" },
            },
            ...captured.slice(1, 50),
          ];
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        start,
        limit: 50,
        total: 51,
        cleanupMaximum: 5000,
        restoreMaximum: 100,
        evaluatedAt: "2026-01-01T00:00:00Z",
        expiresAt: cursor ? null : "2099-01-01T00:00:00Z",
        nextCursor: cursor ? null : "retained-page-2",
        libraryRoot: root,
        namespace: "service",
        operation: null,
        photos: rows,
      }),
    });
  });

  await page.route(/\/api\/photos\/[^/]+\/albums$/, (route) =>
    route.fulfill({ json: { albums: [] } }),
  );
  await openViewOptions(page);
  await page.locator("[data-removed-open]").click();
  const panel = page.locator("[data-removed-panel]");
  await expect(panel.locator("[data-removed-list] .removed-item")).toHaveCount(
    50,
  );
  await expect(panel.locator("[data-trash-copy-locations]")).toBeEnabled();
  await panel.locator("[data-trash-copy-locations]").click();
  await expect(panel.locator("[data-removed-message]")).toContainText(
    "file locations copied",
  );
  const allLocations = await page.evaluate(() =>
    sessionStorage.getItem("test-copied-trash"),
  );
  expect(allLocations).toContain(`Library Folder: ${JSON.stringify(root)}`);
  expect(allLocations).toContain(
    "server/container filesystem, not client-local",
  );
  expect(allLocations).toContain("2026-01-01T00:00:00Z");
  expect(allLocations).toContain("2099-01-01T00:00:00Z");
  expect(allLocations).toContain('"originalId":"original-50"');
  expect(allLocations).toContain('"originalLocation":"fixture/50.jpg"');
  await expect(panel.locator("[data-removed-selection-count]")).toHaveText(
    "Nothing selected.",
  );
  await expect(panel.locator("[data-removed-delete]")).toBeHidden();
  await panel.locator("[data-removed-select-all]").click();
  await expect.poll(() => cursors).toContain("retained-page-2");
  const excluded = panel.locator("[data-removed-list] .removed-item").first();
  await excluded.locator("input[type=checkbox]").uncheck();
  await panel.locator("[data-trash-copy-locations]").click();
  await expect(panel.locator("[data-removed-message]")).toContainText(
    "Selected file locations copied",
  );
  const selectedLocations = await page.evaluate(() =>
    sessionStorage.getItem("test-copied-trash"),
  );
  expect(selectedLocations).not.toContain('"originalLocation":"fixture/0.jpg"');
  expect(selectedLocations).toContain('"originalLocation":"fixture/50.jpg"');
  await expect(excluded.locator("[data-trash-excluded]")).toBeVisible();

  await page.route("**/api/trash/cleanup-preview", async (route) => {
    const body = route.request().postDataJSON() as {
      items: Array<{ photoId: string; originalLocation: string }>;
    };
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        items: body.items.map((item) => ({
          photoId: item.photoId,
          originalLocation: item.originalLocation,
          originalKind: "jpeg",
          state: "captured",
          absence: "absent",
          message: null,
          blockers: [],
        })),
      }),
    });
  });
  await panel.locator("[data-removed-delete]").click();
  const review = page.locator("[data-trash-review]");
  await expect(review).toBeVisible();
  await expect(review.locator("[data-trash-review-location]")).toHaveCount(50);
  expect(
    await review.locator("[data-trash-review-location]").allTextContents(),
  ).not.toContain("fixture/0.jpg");
  await expect(review.locator("[data-trash-confirm]")).toHaveText(
    "Clean up 50 Photo records",
  );
  // Opening and dismissing the review never submits anything.
  await review.locator("[data-trash-cancel]").click();
  await expect(review).toBeHidden();
  expect(firstPageReads).toBe(1);
  await expect(panel.locator("[data-removed-list] .removed-item")).toHaveCount(
    50,
  );

  let submitted = false;
  await page.route("**/api/trash/cleanup", async (route) => {
    submitted = true;
    const body = route.request().postDataJSON() as {
      operationId: string;
      items: Array<{
        photoId: string;
        removedAtMs: number;
        originalId: string;
        originalLocation: string;
      }>;
    };
    expect(body.items).toHaveLength(50);
    expect(body.items.map((item) => item.photoId)).toEqual(
      captured.slice(1).map((item) => item.photoId),
    );
    expect(body.items).toEqual(
      captured
        .slice(1)
        .map(({ photoId, removedAtMs, originalId, originalLocation }) => ({
          photoId,
          removedAtMs,
          originalId,
          originalLocation,
        })),
    );
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        operationId: body.operationId,
        reviewed: 50,
        counts: {
          cleaned: 0,
          changed: 0,
          present: 50,
          blocked: 0,
          unavailable: 0,
          inspectionFailed: 0,
        },
        items: body.items.map((item) => ({
          photoId: item.photoId,
          originalLocation: item.originalLocation,
          originalKind: "jpeg",
          state: "present",
          message: null,
          blockers: [],
        })),
      }),
    });
  });
  await panel.locator("[data-removed-delete]").click();
  await review.locator("[data-trash-confirm]").click();
  await expect(page.locator("[data-trash-outcome-counts]")).toContainText(
    "Present 50",
  );
  expect(submitted).toBe(true);
});
