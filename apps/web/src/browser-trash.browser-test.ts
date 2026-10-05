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
  await page.locator("[data-removal-open]").click();
  await expect(page.locator("[data-removal-summary]")).toHaveText(
    /^2 Photos reviewed as Rejected\./,
  );
  await page.locator("[data-removal-confirm]").click();
  await expect(page.locator("[data-removal-message]")).toHaveText(
    /^2 Photos removed from the Library\./,
  );
  await page.locator("[data-removal-close]").click();
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

test("Trash permanent deletion reviews the files, reports outcomes, and blocks pending verification", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  await writePhotos(root, 3);
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  expect(ids).toHaveLength(3);

  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 3 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  // Two Photos are rejected and removed into Trash through the real flow.
  const cell = (index: number) => page.locator(`[data-photo-index="${index}"]`);
  await page.locator("[data-grid-viewport]").focus();
  await page.keyboard.press("ArrowRight");
  await expect(cell(0)).toBeEnabled();
  await page.keyboard.press("x");
  await expect(cell(0).locator(".cell-state.rejected")).toHaveText("×");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("x");
  await expect(cell(1).locator(".cell-state.rejected")).toHaveText("×");
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("rejected");
  await applyViewOptions(page);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 2 Photos",
  );
  await page.locator("[data-removal-open]").click();
  await page.locator("[data-removal-confirm]").click();
  await expect(page.locator("[data-removal-message]")).toHaveText(
    /^2 Photos removed from the Library\./,
  );
  await page.locator("[data-removal-close]").click();

  // Every Trash route below is a scenario stub against the frozen contract:
  // the listing names the review maximum, one item is still pending
  // verification with its retained operation id, and the review, delete, and
  // operation responses are controlled so the outcomes are deterministic.
  const first = ids[0]!;
  const second = ids[1]!;
  const pendingId = "photo-pending";
  const pendingOperationId = "operation-pending";
  const storageKey = "slipstream:trash-deletion-operation";
  const facts = (photoId: string) =>
    photoId === second
      ? { location: "2024/b.cr2", kind: "raw" as const, size: 2048 }
      : { location: `2024/${photoId}.jpg`, kind: "jpeg" as const, size: 1024 };
  let trashRows = [first, second, pendingId];
  let reviewCalls = 0;
  let deleteCalls = 0;
  const listingPhoto = (photoId: string) => ({
    removedAtMs: photoId === first ? 3000 : photoId === second ? 2000 : 1000,
    originalLocation: facts(photoId).location,
    originalKind: facts(photoId).kind,
    originalSize: facts(photoId).size,
    pendingVerificationOperationId:
      photoId === pendingId ? pendingOperationId : null,
    photo: {
      id: photoId,
      available: true,
      original: { kind: facts(photoId).kind, available: true },
      originalFilename: photoId === second ? "b.cr2" : `${photoId}.jpg`,
      selectionState: "unflagged",
      rating: 0,
      hasSavedEdits: false,
      preview: { state: "unavailable" },
    },
  });
  const reviewItem = (photoId: string) => ({
    photoId,
    removedAtMs: 2000,
    originalId: `original-${photoId}`,
    originalLocation: facts(photoId).location,
    originalKind: facts(photoId).kind,
    size: facts(photoId).size,
    albums: [{ id: "album-1", name: "Keepers" }],
  });
  const operationItem = (
    photoId: string,
    state: string,
    message: string | null = null,
  ) => ({
    photoId,
    state,
    originalLocation: facts(photoId).location,
    originalKind: facts(photoId).kind,
    size: facts(photoId).size,
    message,
  });
  await page.route(/\/api\/trash/, async (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;
    if (pathname === "/api/trash") {
      const url = new URL(request.url());
      const start = Number(url.searchParams.get("start") ?? "0");
      const limit = Number(url.searchParams.get("limit") ?? "50");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          start,
          limit,
          total: trashRows.length,
          operation: null,
          reviewMaximum: 50,
          photos: trashRows.slice(start, start + limit).map(listingPhoto),
        }),
      });
      return;
    }
    if (pathname === "/api/trash/review") {
      const body = JSON.parse(request.postData() ?? "{}") as {
        operationId?: string;
        photoIds?: string[];
      };
      reviewCalls += 1;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          reviewCalls === 1
            ? {
                operationId: body.operationId,
                items: [],
                rejected: [
                  { photoId: first, reason: "missing" },
                  { photoId: second, reason: "changed-elsewhere" },
                ],
              }
            : {
                operationId: body.operationId,
                items: (body.photoIds ?? []).map(reviewItem),
                rejected: [],
              },
        ),
      });
      return;
    }
    if (pathname === "/api/trash/delete") {
      const body = JSON.parse(request.postData() ?? "{}") as {
        operationId?: string;
      };
      deleteCalls += 1;
      if (deleteCalls === 1) {
        // The first confirmation loses its response: nothing is claimed.
        // The delay keeps the in-flight presentation observable.
        const settled = Promise.withResolvers<void>();
        setTimeout(settled.resolve, 500);
        await settled.promise;
        await route.abort("connectionreset");
        return;
      }
      // The retry repeats only the unresolved items of the same operation.
      trashRows = trashRows.filter((photoId) => photoId !== first);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          operationId: body.operationId,
          reviewed: 2,
          logicalBytesDeleted: 1024,
          items: [
            operationItem(first, "deleted"),
            operationItem(second, "missing", "No such file"),
          ],
        }),
      });
      return;
    }
    if (pathname.startsWith("/api/trash/operations/")) {
      const operationId = decodeURIComponent(pathname.split("/").pop() ?? "");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          operationId === pendingOperationId
            ? {
                operationId,
                reviewed: 1,
                logicalBytesDeleted: 0,
                items: [operationItem(pendingId, "deleting")],
              }
            : {
                operationId,
                reviewed: 2,
                logicalBytesDeleted: 1024,
                items: [
                  operationItem(first, "deleted"),
                  operationItem(second, "deleting"),
                ],
              },
        ),
      });
      return;
    }
    throw new Error(`Unexpected Trash route ${pathname}`);
  });

  await page.locator("[data-removed-open]").click();
  const removed = page.locator("[data-removed-panel]");
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "3 Photos in Trash. Showing 1–3.",
  );

  // The pending-verification item cannot be selected or restored, and its
  // row explains why.
  const pendingRow = page.locator(
    "[data-removed-list] .removed-item[data-trash-pending-item]",
  );
  await expect(pendingRow).toHaveCount(1);
  await expect(pendingRow.locator("input[type=checkbox]")).toBeDisabled();
  await expect(
    pendingRow.getByRole("button", { name: "Restore" }),
  ).toBeDisabled();
  await expect(
    pendingRow.locator("[data-trash-pending] .removed-pending-marker"),
  ).toHaveText(
    "Pending verification — the deletion outcome is still being verified.",
  );

  // Check result fetches the retained operation and presents its state.
  await pendingRow.locator("[data-trash-row-check]").click();
  await expect(page.locator("[data-trash-outcome-title]")).toHaveText(
    "Deletion outcome",
  );
  await expect(page.locator("[data-trash-outcome-pending]")).toHaveText(
    "Pending verification 1",
  );
  await expect(page.locator("[data-trash-outcome-deleted]")).toHaveText(
    "Deleted 0",
  );

  // Select all skips the pending-verification item.
  await page.locator("[data-removed-select-all]").click();
  await expect(page.locator("[data-removed-selection-count]")).toHaveText(
    "2 selected.",
  );
  await expect(page.locator("[data-removed-message]")).toHaveText(
    "1 item pending verification was not selected.",
  );

  // A review that rejected every selected item offers no delete action.
  const review = page.locator("[data-trash-review]");
  await page.locator("[data-removed-delete]").click();
  await expect(review).toBeVisible();
  await expect(page.locator("[data-trash-review-rejected]")).toBeVisible();
  await expect(page.locator("[data-trash-review-rejected-heading]")).toHaveText(
    "2 items could not be reviewed:",
  );
  const rejectedItems = page.locator("[data-trash-review-rejected-item]");
  await expect(rejectedItems).toHaveCount(2);
  await expect(
    rejectedItems.nth(0).locator("[data-trash-review-rejected-reason]"),
  ).toHaveText("Original missing");
  await expect(
    rejectedItems.nth(1).locator("[data-trash-review-rejected-reason]"),
  ).toHaveText("Original changed since it was removed");
  await expect(page.locator("[data-trash-confirm]")).toBeHidden();
  // Cancel deletes nothing: the delete route is never called.
  await page.locator("[data-trash-cancel]").click();
  await expect(review).toBeHidden();
  expect(deleteCalls).toBe(0);

  // The full review names the count, the logical bytes, the Album, and each
  // file's Location, kind, and size before the one irreversible action.
  await page.locator("[data-removed-delete]").click();
  await expect(review).toBeVisible();
  await expect(page.locator("[data-trash-review-summary]")).toHaveText(
    "2 Photos selected for permanent deletion · 3,072 logical bytes.",
  );
  await expect(page.locator(".trash-review-warning")).toHaveText(
    "Deletion removes each Photo from every Album that contains it. Slipstream cannot undo it.",
  );
  await expect(page.locator("[data-trash-review-albums]")).toHaveText(
    "1 Album affected: Keepers.",
  );
  const reviewItems = page.locator("[data-trash-review-item]");
  await expect(reviewItems).toHaveCount(2);
  await expect(
    reviewItems.nth(0).locator("[data-trash-review-location]"),
  ).toHaveText(`2024/${first}.jpg`);
  await expect(
    reviewItems.nth(0).locator("[data-trash-review-kind]"),
  ).toHaveText("JPEG");
  await expect(
    reviewItems.nth(0).locator("[data-trash-review-size]"),
  ).toHaveText("1,024 bytes");
  await expect(
    reviewItems.nth(0).locator("[data-trash-review-item-albums]"),
  ).toHaveText("Keepers");
  await expect(
    reviewItems.nth(1).locator("[data-trash-review-location]"),
  ).toHaveText("2024/b.cr2");
  await expect(
    reviewItems.nth(1).locator("[data-trash-review-kind]"),
  ).toHaveText("RAW");
  await expect(
    reviewItems.nth(1).locator("[data-trash-review-size]"),
  ).toHaveText("2,048 bytes");
  await expect(page.locator("[data-trash-confirm]")).toHaveText(
    "Permanently delete 2 Original Files",
  );
  await page.locator("[data-trash-cancel]").click();
  await expect(review).toBeHidden();
  expect(deleteCalls).toBe(0);

  // Confirming opens the delete route. The response is lost, so the surface
  // offers recovery instead of claiming failure or success.
  await page.locator("[data-removed-delete]").click();
  await expect(review).toBeVisible();
  await page.locator("[data-trash-confirm]").click();
  await expect(page.locator("[data-trash-confirm]")).toHaveText(
    "Permanently deleting…",
  );
  await expect(review).toBeHidden();
  await expect(page.locator("[data-trash-outcome-title]")).toHaveText(
    "The deletion result could not be confirmed.",
  );
  await expect(page.locator("[data-trash-check-result]")).toBeVisible();
  await expect(page.locator("[data-trash-retry-delete]")).toBeVisible();
  expect(
    await page.evaluate((key) => window.localStorage.getItem(key), storageKey),
  ).toBeTruthy();

  // Check result recovers the operation: one deletion is confirmed, one item
  // is still pending verification, and the retained id stays.
  await page.locator("[data-trash-check-result]").click();
  await expect(page.locator("[data-trash-outcome-title]")).toHaveText(
    "Deletion outcome",
  );
  await expect(page.locator("[data-trash-outcome-deleted]")).toHaveText(
    "Deleted 1",
  );
  await expect(page.locator("[data-trash-outcome-pending]")).toHaveText(
    "Pending verification 1",
  );
  await expect(page.locator("[data-trash-outcome-bytes]")).toHaveText(
    "Logical bytes deleted: 1,024",
  );
  expect(
    await page.evaluate((key) => window.localStorage.getItem(key), storageKey),
  ).toBeTruthy();

  // Retry repeats only the unresolved items and settles the operation.
  await page.locator("[data-trash-retry-delete]").click();
  await expect(page.locator("[data-trash-outcome-title]")).toHaveText(
    "Deletion outcome",
  );
  await expect(page.locator("[data-trash-outcome-deleted]")).toHaveText(
    "Deleted 1",
  );
  await expect(page.locator("[data-trash-outcome-missing]")).toHaveText(
    "Missing 1",
  );
  await expect(page.locator("[data-trash-outcome-bytes]")).toHaveText(
    "Logical bytes deleted: 1,024",
  );
  const outcomeItems = page.locator("[data-trash-outcome-item]");
  await expect(outcomeItems).toHaveCount(1);
  await expect(outcomeItems.first()).toHaveText(
    "Missing · RAW · 2024/b.cr2 · 2,048 bytes · No such file",
  );
  // The listing refreshes with the confirmed deletions: the deleted item
  // leaves Trash while the missing one stays inspectable.
  await expect(page.locator("[data-removed-status]")).toHaveText(
    "2 Photos in Trash. Showing 1–2.",
  );
  await expect(page.locator("[data-removed-list] .removed-item")).toHaveCount(
    2,
  );
  expect(deleteCalls).toBe(2);
  // A settled operation releases the retained id.
  expect(
    await page.evaluate((key) => window.localStorage.getItem(key), storageKey),
  ).toBeNull();
  await page.locator("[data-removed-close]").click();
  await expect(removed).toBeHidden();
});
