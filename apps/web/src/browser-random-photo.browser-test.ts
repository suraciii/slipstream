import { expect, test } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import {
  browseIds,
  createAlbum,
  fixture,
  jpeg,
  server,
  setupBrowserSmoke,
  writePhotos,
} from "./browser-test-support/fixtures.js";
import {
  applyViewOptions,
  openSources,
  openViewOptions,
  waitForGridFrame,
} from "./browser-test-support/surfaces.js";
setupBrowserSmoke();

// The random tests intercept the Browse Window route and hold it open, so one
// worker keeps each attempt's held response from leaking into a sibling.
test.describe.configure({ mode: "serial" });

/// The one random action owns a single button, so a test can drive both of its
/// placements by its accessible name and read the pending state from the same
/// node the Photographer sees.
const randomAction = (page: Page) =>
  page.getByRole("button", { name: /^Open a random Photo/ });

/// Waits until the Photo View has either a resolved Preview or its explained
/// unavailability, so a test never races the existing Preview rules.
const reviewVisible = async (page: Page) => {
  await expect(page.locator("[data-review]")).toBeVisible();
  await page.waitForFunction(
    () =>
      Boolean(document.querySelector("[data-stage] img")) ||
      document.body.innerText.includes("Preview unavailable"),
  );
};

/// Makes the draw deterministic: the stub answers the two-word request the
/// random helper makes with one fixed 53-bit word, so the target is the
/// unloaded second window rather than a position this run happened to land in.
const stubDrawToSecondWindow = async (page: Page): Promise<void> => {
  await page.addInitScript(() => {
    const original = crypto.getRandomValues.bind(crypto);
    crypto.getRandomValues = (<T extends ArrayBufferView>(array: T): T => {
      if (array instanceof Uint32Array && array.length === 2) {
        array[0] = 0;
        array[1] = 69;
        return array;
      }
      return original(array);
    }) as typeof crypto.getRandomValues;
  });
};

/// Fails only the first drawn-window read: 404 expires the Snapshot, 503
/// rejects the read, and 0 aborts its transport. Later reads reach the real
/// service so the test can exercise recovery without replacing the fixture.
const answerFirstBrowseRead = (
  page: Page,
  status: 0 | 404 | 503,
): { requests: string[] } => {
  // The drawn window is never the opening window, so the read the test owns is
  // the one after the ordinary source open.
  const pattern = /\/api\/browse\/[^/?#]+\?start=(?!0(?:&|$))/;
  const requests: string[] = [];
  let first = true;
  void page.route(pattern, async (route) => {
    requests.push(
      new URL(route.request().url()).searchParams.get("start") ?? "0",
    );
    if (!first) {
      await route.continue();
      return;
    }
    first = false;
    if (status === 0) {
      await route.abort("failed");
      return;
    }
    await route.fulfill({
      status,
      contentType: "application/json",
      body: JSON.stringify({ error: "unavailable" }),
    });
  });
  return { requests };
};

const wide = async (page: Page) => {
  await page.setViewportSize({ width: 1200, height: 800 });
};

test("Open a random Photo samples the whole Snapshot and returns through ordinary History", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 120);
  const running = await server(base, root);
  await wide(page);
  // The Browse Window reads the test owns: the opening window, then the one the
  // random draw resolves. A decision write or an Album mutation would be a
  // different contract, so the test records those separately.
  await stubDrawToSecondWindow(page);
  const reads: string[] = [];
  const writes: string[] = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (/^\/api\/browse\/[^/]+$/.test(url.pathname)) {
      if (request.method() === "GET")
        reads.push(url.searchParams.get("start") ?? "0");
      else writes.push(request.url());
      return;
    }
    if (/\/state$/.test(url.pathname) && request.method() === "POST")
      writes.push(request.url());
  });
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 120 Photos",
  );
  await waitForGridFrame(page);
  await expect(randomAction(page)).toHaveCount(1);
  // A wide Grid keeps the action in the header's secondary group, and the
  // header still fits one row.
  await expect(
    page.locator("[data-grid-tools] [data-random-photo]"),
  ).toHaveCount(1);
  await expect(
    page
      .locator("[data-view-options]")
      .getByRole("button", { name: /^Open a random Photo/ }),
  ).toHaveCount(0);
  expect(
    await page
      .locator(".grid-header")
      .evaluate((element) => element.getBoundingClientRect().height),
  ).toBeLessThanOrEqual(88);

  await randomAction(page).click();
  await reviewVisible(page);
  await expect(page.locator("[data-position]")).toHaveText("70 / 120");
  const address = page.url();
  expect(new URL(address).searchParams.get("selection")).toBeNull();
  expect(new URL(address).search).not.toMatch(/random|seed/i);
  // The draw read only the bounded window that held it — never the complete
  // source — and the destination wrote no decision.
  expect(reads).toEqual(["0", "60"]);
  expect(writes).toEqual([]);

  // Back returns to the Grid context that started the action.
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-review]")).toBeHidden();
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 120 Photos",
  );
  expect(page.url()).toBe(running.url + "/");

  // Reloading the committed Photo address opens that same Photo and performs no
  // new draw: the address owns the Photo identity, not a random request. The
  // first navigation's listener stops with its page, so a fresh one names the
  // reload's own bounded read.
  page.removeAllListeners("request");
  const reloadReads: string[] = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (
      /^\/api\/browse\/[^/]+$/.test(url.pathname) &&
      request.method() === "GET"
    )
      reloadReads.push(url.searchParams.get("start") ?? "0");
  });
  await page.goto(address);
  await reviewVisible(page);
  await expect(page.locator("[data-position]")).toHaveText("70 / 120");
  expect(page.url()).toBe(address);
  expect(reloadReads.every((start) => start === "0" || start === "60")).toBe(
    true,
  );
});

test("an Album names itself, opens one member, and reopens the committed address", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  await createAlbum(running.url, "Round Robin", ids);

  await wide(page);
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 4 Photos",
  );
  await waitForGridFrame(page);
  await openSources(page);
  await page.getByRole("link", { name: /^Round Robin/ }).click();
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 4 Photos",
  );
  await waitForGridFrame(page);
  await expect(randomAction(page)).toBeEnabled();
  await expect(randomAction(page)).toHaveAccessibleName(
    "Open a random Photo from Round Robin",
  );

  await randomAction(page).click();
  await reviewVisible(page);
  await expect(page.locator("[data-position]")).toHaveText(/^\d+ \/ 4$/);
  const address = page.url();

  // Reloading the committed address opens that member again and performs no new
  // draw: the address owns the Photo identity, not a random request.
  await page.reload();
  await reviewVisible(page);
  expect(page.url()).toBe(address);
});

test("a filtered source is named in its accessible name and follows the committed filter", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  await wide(page);
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 4 Photos",
  );
  await waitForGridFrame(page);
  await expect(randomAction(page)).toBeEnabled();
  await expect(randomAction(page)).toHaveAccessibleName(
    "Open a random Photo from All Photos",
  );

  // The committed filter names itself in the accessible name, so the
  // Photographer can tell which sequence the action samples.
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("picked");
  await applyViewOptions(page);
  await expect(page.locator("[data-grid-status]")).toHaveText("0 Photos");
  await expect(page.locator("[data-grid-empty-message]")).toHaveText(
    "No Photos match this filter.",
  );
  await expect(randomAction(page)).toBeDisabled();
  await expect(randomAction(page)).toHaveAccessibleName(
    "Open a random Photo from All Photos · Picked filter",
  );

  // Deciding one Photo makes the filtered Snapshot non-empty again, so the
  // action follows the committed filter rather than the source's original size.
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("all");
  await applyViewOptions(page);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 4 Photos",
  );
  await page.locator('[data-photo-index="0"]').click();
  await reviewVisible(page);
  await page.locator("[data-dock-pick]").click();
  await expect(page.locator("[data-selection]")).toHaveText("Picked");
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 4 Photos",
  );
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("picked");
  await applyViewOptions(page);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 1 Photo",
  );
  await expect(randomAction(page)).toBeEnabled();
  await randomAction(page).click();
  await reviewVisible(page);
  await expect(page.locator("[data-position]")).toHaveText("1 / 1");
});

test("an empty Library is distinct from an empty filtered result", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const running = await server(base, root);
  await wide(page);
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText("0 Photos");
  await expect(page.locator("[data-grid-empty-message]")).toHaveText(
    "No supported Photos found. Check the Library Folder or add supported files, then run Check Library.",
  );
  await expect(randomAction(page)).toBeDisabled();
  await expect(randomAction(page)).toHaveAccessibleName(
    "Open a random Photo from All Photos",
  );
});

test("a narrow Grid moves the action into View options under Browse actions", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 2 Photos",
  );
  await waitForGridFrame(page);
  // The same node leaves the header, so no permanent row is added.
  await expect(
    page.locator("[data-grid-tools] [data-random-photo]"),
  ).toHaveCount(0);
  expect(
    await page
      .locator(".grid-header")
      .evaluate((element) => element.getBoundingClientRect().height),
  ).toBeLessThanOrEqual(88);
  await openViewOptions(page);
  await expect(randomAction(page)).toHaveCount(1);
  await expect(
    page.locator("[data-browse-actions] [data-random-photo]"),
  ).toHaveCount(1);
  await expect(page.getByText("Browse actions", { exact: true })).toBeVisible();
  // The moved control keeps its keyboard path: it is the surface's one
  // activation target and carries focus while it is the action.
  await randomAction(page).focus();
  await expect(randomAction(page)).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator("[data-view-options]")).toBeHidden();
  await reviewVisible(page);
});

for (const status of [503, 0] as const) {
  test(`a bounded read failure (${status}) keeps the Grid and retries its exact window`, async ({
    page,
  }) => {
    const { base, root } = await fixture();
    await writePhotos(root, 120);
    const running = await server(base, root);
    await wide(page);
    await stubDrawToSecondWindow(page);
    await page.goto(running.url);
    await expect(page.locator("[data-grid-status]")).toHaveText(
      "Ready · 120 Photos",
    );
    await waitForGridFrame(page);

    const { requests } = answerFirstBrowseRead(page, status);
    await randomAction(page).click();
    // The Grid stays, no Photo is claimed, and no Photo entry enters History.
    await expect(page.locator("[data-review]")).toBeHidden();
    await expect(page.locator("[data-grid-status]")).toHaveText(
      status === 0
        ? "Connection lost while loading Photos 61–120. Retry this range."
        : "Photos 61–120 could not be loaded (HTTP 503). Retry this range.",
    );
    expect(page.url()).toBe(running.url + "/");
    expect(requests).toEqual(["60"]);

    await openSources(page);
    await expect(page.locator("[data-retry]")).toBeVisible();
    await page.locator("[data-retry]").click();
    await expect(page.locator("[data-grid-status]")).toHaveText(
      "Ready · 120 Photos",
    );
    await expect(randomAction(page)).toBeEnabled();
    expect(requests).toEqual(["60", "60"]);
    expect(page.url()).toBe(running.url + "/");
    await expect(page.locator("[data-review]")).toBeHidden();
  });
}

test("an expired Snapshot keeps the Grid and names Refresh Current Source", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 120);
  const running = await server(base, root);
  await wide(page);
  await stubDrawToSecondWindow(page);
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 120 Photos",
  );
  await waitForGridFrame(page);

  const { requests } = answerFirstBrowseRead(page, 404);
  await randomAction(page).click();
  // The Snapshot is not silently replaced: the Grid keeps its authority, no
  // Photo entry is written, and the status names the recovery path the source
  // already offers.
  await expect(page.locator("[data-review]")).toBeHidden();
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Library order expired. Refresh Current Source to continue.",
  );
  expect(page.url()).toBe(running.url + "/");
  expect(requests).toEqual(["60"]);

  await openViewOptions(page);
  await expect(page.locator("[data-refresh]")).toBeVisible();
  await page.locator("[data-refresh]").click();
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 120 Photos",
  );
  await expect(randomAction(page)).toBeEnabled();
});

test("the action admits one attempt and presents its pending state", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 120);
  const running = await server(base, root);
  await wide(page);
  await stubDrawToSecondWindow(page);
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 120 Photos",
  );
  await waitForGridFrame(page);

  const { promise: held, resolve: release } = Promise.withResolvers<void>();
  let first = true;
  await page.route(
    /\/api\/browse\/[^/?#]+\?start=(?!0(?:&|$))/,
    async (route) => {
      if (!first) {
        await route.continue();
        return;
      }
      first = false;
      await held;
      await route.continue();
    },
  );

  await randomAction(page).click();
  await expect(randomAction(page)).toBeDisabled();
  await expect(randomAction(page)).toHaveAttribute("aria-busy", "true");
  await expect(randomAction(page)).toHaveText("Opening random Photo…");
  await expect(page.locator("[data-review]")).toBeHidden();
  await expect(page.locator('[data-photo-index="0"]')).toBeDisabled();
  release();
  await reviewVisible(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-review]")).toBeHidden();
  await expect(randomAction(page)).toHaveText("Open a random Photo");
  await expect(randomAction(page)).toBeEnabled();
  await expect(page.locator('[data-photo-index="0"]')).toBeEnabled();
});

test("an Original Folder names itself and keeps the action beside its source actions", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const folder = join(root, "Coast 2024");
  await mkdir(folder);
  await writeFile(join(folder, "a.jpg"), await jpeg());
  await writeFile(join(folder, "b.jpg"), await jpeg());
  const running = await server(base, root);
  await wide(page);
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 2 Photos",
  );
  await openSources(page);
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  await page.getByRole("link", { name: /^Coast 2024/ }).click();
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 2 Photos",
  );
  await waitForGridFrame(page);
  await expect(randomAction(page)).toHaveAccessibleName(
    "Open a random Photo from Coast 2024 · Folder",
  );
  await openViewOptions(page);
  // Browse actions and the source's own actions share the surface without
  // collapsing into one row.
  await expect(page.getByText("Browse actions", { exact: true })).toBeVisible();
  await expect(page.locator("[data-refresh]")).toBeVisible();
});
