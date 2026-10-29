import { expect, test, type Page } from "@playwright/test";
import { unlink, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  setupBrowserSmoke,
  fixture,
  jpeg,
  jpegWithSize,
  withCaptureTime,
  writePhotos,
  server,
  libraryPhoto,
  post,
} from "./browser-test-support/fixtures.js";
import {
  applyViewOptions,
  openSources,
  openViewOptions,
  waitForGridFrame,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

/// The rating and Capture Time scenarios share six JPEG Photos:
/// a–e have Capture Times from January 2 through January 6; f has none.
/// Their Ratings are 5, 4, 2, 0, 3, and 4 respectively when seeded below.
/// Grid counts describe the filtered view; source progress remains unfiltered.

const CAPTURE_TIMES = {
  a: "2026-01-02T09:00:00",
  b: "2026-01-03T09:00:00",
  c: "2026-01-04T09:00:00",
  d: "2026-01-05T09:00:00",
  e: "2026-01-06T09:00:00",
} as const;

const RATINGS = {
  a: 5,
  b: 4,
  c: 2,
  d: 0,
  e: 3,
  f: 4,
} as const;

// EXIF DateTimeOriginal uses colons in the date and a space before the time;
// the filter controls use the corresponding ISO local date-time instead.
const exifCaptureTime = (captureTime: string): string =>
  captureTime.replace(/^(\d{4})-(\d{2})-(\d{2})T/, "$1:$2:$3 ");

/// Each JPEG is an independent Original; Capture Time is embedded in a–e.
const seedFilterSource = async (page: Page, root: string): Promise<void> => {
  const plain = await jpeg();
  const sized = await jpegWithSize(page, 400, 300);
  await writeFile(
    join(root, "a.jpg"),
    withCaptureTime(new Uint8Array(plain), exifCaptureTime(CAPTURE_TIMES.a)),
  );
  await writeFile(
    join(root, "b.jpg"),
    withCaptureTime(new Uint8Array(plain), exifCaptureTime(CAPTURE_TIMES.b)),
  );
  await writeFile(
    join(root, "c.jpg"),
    withCaptureTime(new Uint8Array(sized), exifCaptureTime(CAPTURE_TIMES.c)),
  );
  await writeFile(
    join(root, "d.jpg"),
    withCaptureTime(new Uint8Array(sized), exifCaptureTime(CAPTURE_TIMES.d)),
  );
  await writeFile(
    join(root, "e.jpg"),
    withCaptureTime(new Uint8Array(plain), exifCaptureTime(CAPTURE_TIMES.e)),
  );
  // f.jpg carries no Capture Time at all, so it can never match a bound.
  await writeFile(join(root, "f.jpg"), new Uint8Array(sized));
};

/// Resolve fixture names from Library facts, not assumed Browse order.
const ratePhotos = async (url: string): Promise<Record<string, string>> => {
  const byName: Record<string, string> = {};
  for (let index = 0; index < 6; index += 1) {
    const photo = await libraryPhoto(url, index);
    const name = photo.originalFilename?.replace(/\.jpg$/, "");
    expect(name && Object.hasOwn(RATINGS, name)).toBeTruthy();
    const key = name as keyof typeof RATINGS;
    expect(byName[key]).toBeUndefined();
    byName[key] = photo.id;
    const response = await post(url, `/api/photos/${photo.id}/state`, {
      field: "rating",
      value: RATINGS[key],
    });
    expect(response.ok).toBe(true);
  }
  return byName;
};

test("a selected high-rated view keeps its exact filtered count", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  await seedFilterSource(page, root);
  const running = await server(base, root);
  const ids = await ratePhotos(running.url);
  // Two picked Photos meet the Rating bound; d is picked but unrated.
  for (const name of ["a", "b", "d"]) {
    const response = await post(running.url, `/api/photos/${ids[name]}/state`, {
      field: "selectionState",
      value: "picked",
    });
    expect(response.ok).toBe(true);
  }

  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 6 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  // Selection State and Rating combine with AND, and the Rating bound is
  // inclusive, so a picked Photo with Rating 4 or more matches.
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("picked");
  await page.locator("[data-rating-min-select]").selectOption("4");
  await applyViewOptions(page);

  await expect(page).toHaveURL(/selection=picked/);
  await expect(page).toHaveURL(/rating-min=4/);
  await expect(page.locator(".photo-cell")).toHaveCount(2);
  await expect(
    page.locator(".photo-cell").filter({ hasText: "a.jpg" }),
  ).toHaveCount(1);
  await expect(
    page.locator(".photo-cell").filter({ hasText: "b.jpg" }),
  ).toHaveCount(1);
  // The filtered result count stays separate from the source progress counts,
  // which still describe all six Photos of the unfiltered source.
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 3 picked · 0 rejected · 3 unflagged",
  );

  // Every committed condition is named in the header, and the narrow header
  // gives that text its own row instead of clipping it behind the tools.
  await page.setViewportSize({ width: 320, height: 844 });
  const flag = page.locator("[data-view-options-flag]");
  await expect(flag).toBeVisible();
  await expect(flag).toContainText(/picked/i);
  await expect(flag).toContainText(/4/);
  expect(
    await flag.evaluate(
      (element) =>
        element.scrollWidth <= element.clientWidth + 1 &&
        element.scrollHeight <= element.clientHeight + 1,
    ),
  ).toBe(true);
  await expect(flag).toHaveCSS("overflow-y", "auto");
  expect(
    await page
      .locator(".grid-header")
      .evaluate((element) => element.getBoundingClientRect().height),
  ).toBeLessThanOrEqual(88);
  await page.setViewportSize({ width: 1000, height: 700 });

  // Rating 0 means No Rating, so a maximum of 0 matches only the unrated Photo.
  await openViewOptions(page);
  await page.locator("[data-rating-min-select]").selectOption("");
  await page.locator("[data-rating-max-select]").selectOption("0");
  await applyViewOptions(page);
  await expect(page).toHaveURL(/rating-max=0/);
  await expect(page.locator(".photo-cell")).toHaveCount(1);
  await expect(page.locator(".photo-cell").first()).toHaveAccessibleName(
    /d\.jpg/,
  );

  // Clearing returns to the source default filter without changing source.
  await openViewOptions(page);
  await page.locator("[data-filter-select]").selectOption("all");
  await page.locator("[data-rating-max-select]").selectOption("");
  await applyViewOptions(page);
  await expect(page.locator(".photo-cell")).toHaveCount(6);
});

test("a RAW-only view keeps RAW and same-basename JPEG Photos independent", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  await seedFilterSource(page, root);
  // The scanner classifies Originals by extension. This minimal TIFF RAW
  // fixture is distinct from its same-basename JPEG, even without a Preview.
  await writeFile(
    join(root, "a.ARW"),
    Buffer.from([0x49, 0x49, 0x2a, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
  );
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 7 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  await openViewOptions(page);
  await page.locator("[data-kind-select]").selectOption("raw");
  await applyViewOptions(page);

  await expect(page).toHaveURL(/kind=raw/);
  await expect(page.locator(".photo-cell")).toHaveCount(1);
  await expect(page.locator(".photo-cell").first()).toHaveAccessibleName(
    /a\.ARW/,
  );
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 picked · 0 rejected · 7 unflagged",
  );

  // The complementary condition includes the same-basename JPEG and all
  // five other JPEGs, without merging it into the RAW Photo.
  await openViewOptions(page);
  await page.locator("[data-kind-select]").selectOption("jpeg");
  await applyViewOptions(page);
  await expect(page).toHaveURL(/kind=jpeg/);
  await expect(page.locator(".photo-cell")).toHaveCount(6);
  await expect(
    page.locator(".photo-cell").filter({ hasText: "a.jpg" }),
  ).toHaveCount(1);
});

test("an unavailable Original describes the Original File, not Preview readiness", async ({
  page,
  context,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  await writePhotos(root, 6);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 6 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  // Removing the Original only changes the remembered availability after a
  // Library scan; the existing Preview is a separate fact.
  await unlink(join(root, "000.jpg"));
  expect((await post(running.url, "/api/scan", {})).status).toBe(200);
  await expect
    .poll(
      async () => {
        const response = await context.request.get(`${running.url}/api/status`);
        const status = (await response.json()) as {
          state: string;
          lastRecovery?: { unavailablePhotos: number };
        };
        return status.state === "idle"
          ? status.lastRecovery?.unavailablePhotos
          : -1;
      },
      { timeout: 60_000 },
    )
    .toBe(1);
  await page.reload();
  await expect(page.getByText(/^Ready · 6 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  await openViewOptions(page);
  await page.locator("[data-available-select]").selectOption("false");
  await applyViewOptions(page);

  await expect(page).toHaveURL(/available=false/);
  await expect(page.locator(".photo-cell")).toHaveCount(1);
  await expect(page.locator(".photo-cell").first()).toHaveAccessibleName(
    /000\.jpg/,
  );
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 picked · 0 rejected · 6 unflagged",
  );

  // A compact cell keeps the exceptional fact in its media placeholder and
  // its caption inside the uniform cell.
  await page.setViewportSize({ width: 390, height: 844 });
  await waitForGridFrame(page);
  const unavailable = page.locator(".photo-cell").first();
  await expect(unavailable.locator(".thumbnail-placeholder")).toBeVisible();
  await expect(unavailable.locator(".thumbnail-placeholder")).toContainText(
    "Original unavailable",
  );
  const unavailableGeometry = await unavailable.evaluate((cell) => {
    const footer = cell.querySelector<HTMLElement>(".cell-footer");
    if (!footer) throw new Error("Cell footer is missing");
    return {
      cellBottom: cell.getBoundingClientRect().bottom,
      footerBottom: footer.getBoundingClientRect().bottom,
    };
  });
  expect(unavailableGeometry.footerBottom).toBeLessThanOrEqual(
    unavailableGeometry.cellBottom + 1,
  );
  await expect(page.locator("[data-view-options-flag]")).toContainText(
    "Unavailable",
  );

  // The complementary condition keeps the other five.
  await openViewOptions(page);
  await page.locator("[data-available-select]").selectOption("true");
  await applyViewOptions(page);
  await expect(page).toHaveURL(/available=true/);
  await expect(page.locator(".photo-cell")).toHaveCount(5);
});

test("a Capture Time range is inclusive below and exclusive above", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  await seedFilterSource(page, root);
  const running = await server(base, root);

  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 6 Photos$/)).toBeVisible();
  await waitForGridFrame(page);

  // The lower bound includes its own second; the upper bound excludes it, and
  // the Photo without a Capture Time never matches a supplied bound.
  await openViewOptions(page);
  await page.locator("[data-captured-from]").fill(CAPTURE_TIMES.b);
  await page.locator("[data-captured-before]").fill(CAPTURE_TIMES.e);
  await applyViewOptions(page);

  await expect(page).toHaveURL(/captured-from=2026-01-03T09%3A00%3A00/);
  await expect(page).toHaveURL(/captured-before=2026-01-06T09%3A00%3A00/);
  // b, c, and d match: the lower bound includes b, the upper bound excludes e,
  // and f has no Capture Time.
  await expect(page.locator(".photo-cell")).toHaveCount(3);
  for (const name of ["b.jpg", "c.jpg", "d.jpg"]) {
    await expect(
      page.locator(".photo-cell").filter({ hasText: name }),
    ).toHaveCount(1);
  }
  await expect(page.locator("[data-grid-source-progress]")).toHaveText(
    "Source progress: 0 picked · 0 rejected · 6 unflagged",
  );
});

test("Refresh, Retry, and Snapshot expiry retain the committed filter", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  await seedFilterSource(page, root);
  const running = await server(base, root);
  await ratePhotos(running.url);
  await page.setViewportSize({ width: 1400, height: 800 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 6 Photos$/)).toBeVisible();
  await openViewOptions(page);
  await page.locator("[data-rating-min-select]").selectOption("4");
  await page.locator("[data-kind-select]").selectOption("jpeg");
  await page.locator("[data-available-select]").selectOption("true");
  await page.locator("[data-captured-from]").fill(CAPTURE_TIMES.a);
  await page.locator("[data-captured-before]").fill(CAPTURE_TIMES.d);
  await applyViewOptions(page);
  await expect(page.locator(".photo-cell")).toHaveCount(2);
  const address = page.url();
  const expected = {
    ratingMinimum: 4,
    kind: "jpeg",
    available: true,
    capturedFrom: CAPTURE_TIMES.a,
    capturedBefore: CAPTURE_TIMES.d,
  };
  const opens: Record<string, unknown>[] = [];
  let failOpen = false;
  let expireWindow = false;
  await page.route("**/api/browse", async (route) => {
    if (route.request().method() !== "POST") return route.continue();
    opens.push(route.request().postDataJSON() as Record<string, unknown>);
    if (failOpen) {
      failOpen = false;
      return route.fulfill({ status: 503, json: { error: "unavailable" } });
    }
    return route.continue();
  });
  await page.route(/\/api\/browse\/[^/?]+\?/, async (route) => {
    if (expireWindow) {
      expireWindow = false;
      return route.fulfill({ status: 404, json: { error: "expired" } });
    }
    return route.continue();
  });
  const refresh = async () => {
    await openViewOptions(page);
    await page.locator("[data-refresh]").click();
  };
  const assertFiltered = async () => {
    await expect(page.locator(".photo-cell")).toHaveCount(2);
    expect(page.url()).toBe(address);
    expect(opens.at(-1)).toMatchObject(expected);
  };
  await refresh();
  await expect.poll(() => opens.length).toBe(1);
  await expect(page.locator("[data-filter-select]")).toBeEnabled();
  await assertFiltered();
  failOpen = true;
  await refresh();
  await expect(page.locator("[data-grid-status]")).toContainText("Retry");
  await page.locator("[data-view-options-cancel]").click();
  await openSources(page);
  await page.locator("[data-retry]").click();
  await expect.poll(() => opens.length).toBe(3);
  await assertFiltered();
  expireWindow = true;
  await refresh();
  await expect.poll(() => opens.length).toBe(5);
  await expect(page.locator("[data-filter-select]")).toBeEnabled();
  await assertFiltered();
  expect(opens[3]).toMatchObject(expected);
});

test("background window settlement preserves View options draft until Apply or Cancel", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const { base, root } = await fixture();
  await seedFilterSource(page, root);
  const running = await server(base, root);
  await ratePhotos(running.url);
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 6 Photos$/)).toBeVisible();
  await openViewOptions(page);
  await page.locator("[data-rating-min-select]").selectOption("4");
  await applyViewOptions(page);
  await expect(page.locator(".photo-cell")).toHaveCount(3);
  const held = Promise.withResolvers<void>();
  const requested = Promise.withResolvers<void>();
  await page.route(/\/api\/browse\/[^/?]+\?/, async (route) => {
    requested.resolve();
    await held.promise;
    await route.continue();
  });
  await openViewOptions(page);
  await expect(page.locator("[data-rating-min-select]")).toHaveValue("4");
  await page.locator("[data-filter-select]").selectOption("unflagged");
  await page.locator("[data-sort-select]").selectOption("capture-time-desc");
  await page.locator("[data-refresh]").click();
  await requested.promise;
  await page.locator("[data-rating-min-select]").selectOption("3");
  await page.locator("[data-rating-max-select]").selectOption("5");
  await page.locator("[data-kind-select]").selectOption("jpeg");
  await page.locator("[data-available-select]").selectOption("true");
  await page.locator("[data-captured-from]").fill(CAPTURE_TIMES.a);
  await page.locator("[data-captured-before]").fill(CAPTURE_TIMES.d);
  held.resolve();
  await expect(page.locator("[data-filter-select]")).toBeEnabled();
  await expect(page.locator("[data-filter-select]")).toHaveValue("unflagged");
  await expect(page.locator("[data-sort-select]")).toHaveValue(
    "capture-time-desc",
  );
  await expect(page.locator("[data-rating-min-select]")).toHaveValue("3");
  await expect(page.locator("[data-rating-max-select]")).toHaveValue("5");
  await expect(page.locator("[data-kind-select]")).toHaveValue("jpeg");
  await expect(page.locator("[data-available-select]")).toHaveValue("true");
  await expect(page.locator("[data-captured-from]")).toHaveValue(
    CAPTURE_TIMES.a,
  );
  await expect(page.locator("[data-captured-before]")).toHaveValue(
    CAPTURE_TIMES.d,
  );
  await applyViewOptions(page);
  await expect(page.locator(".photo-cell")).toHaveCount(2);
  await expect(page).toHaveURL(/rating-min=3/);
  await expect(page).toHaveURL(/rating-max=5/);
  await expect(page).toHaveURL(/kind=jpeg/);
  await expect(page).toHaveURL(/available=true/);
  await expect(page).toHaveURL(/captured-from=/);
  await expect(page).toHaveURL(/captured-before=/);
  await expect(page).toHaveURL(/selection=unflagged/);
  await expect(page).toHaveURL(/order=capture-time-desc/);
  await openViewOptions(page);
  await expect(page.locator("[data-rating-min-select]")).toHaveValue("3");
  await expect(page.locator("[data-kind-select]")).toHaveValue("jpeg");
  await expect(page.locator("[data-captured-from]")).toHaveValue(
    CAPTURE_TIMES.a,
  );
  await page.locator("[data-rating-min-select]").selectOption("4");
  await page.locator("[data-kind-select]").selectOption("");
  await page.locator("[data-captured-from]").fill("");
  await page.locator("[data-view-options-cancel]").click();
  await openViewOptions(page);
  await expect(page.locator("[data-rating-min-select]")).toHaveValue("3");
  await expect(page.locator("[data-kind-select]")).toHaveValue("jpeg");
  await expect(page.locator("[data-captured-from]")).toHaveValue(
    CAPTURE_TIMES.a,
  );
});
