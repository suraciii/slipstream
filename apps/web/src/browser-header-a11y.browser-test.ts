import { expect, test, type Page } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  fixture,
  jpeg,
  server,
  browseIds,
  setupBrowserSmoke,
  createAlbum,
} from "./browser-test-support/fixtures.js";
import {
  waitForGridFrame,
  openSources,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

/// Every width a header control must stay complete at, narrowest first.
const HEADER_WIDTHS = [320, 360, 390, 420, 760] as const;

/// A source name long enough that it must truncate at the narrowest header
/// budget, so the truncation assertion is about the rule and not about a name
/// that happens to fit.
const LONG_ALBUM_NAME = "Spring Capture Session Final Selection";

/// True when an element's painted text is cut off by its own box, which is what
/// a clipped control looks like: the text overflows the border box and no
/// ellipsis was applied to say so.
const clipped = (element: HTMLElement) => {
  const overflowX = element.scrollWidth - element.clientWidth;
  const overflowY = element.scrollHeight - element.clientHeight;
  const style = getComputedStyle(element);
  const allowsOverflow = style.overflow !== "visible";
  const marksIt = style.textOverflow === "ellipsis";
  if (overflowX <= 1 && overflowY <= 1) return false;
  return !allowsOverflow || !marksIt;
};

/// The controls the Grid header must never clip: the disclosure that carries the
/// source name and count, plus both tool entries.
const HEADER_CONTROLS = [
  "[data-source-toggle]",
  "[data-grid-view-options]",
  "[data-grid-select-mode]",
] as const;

/// Builds a Library with one long-named Album, so the header under test carries
/// a source name that must truncate and a result count that must not.
async function gridFixture(albumName: string) {
  const { base, root } = await fixture();
  await mkdir(join(root, "Capture"));
  const data = await jpeg();
  await writeFile(join(root, "root.jpg"), data);
  await writeFile(join(root, "Capture", "one.jpg"), data);
  await writeFile(join(root, "Capture", "two.jpg"), data);
  const running = await server(base, root);
  const photoIds = await browseIds(running.url);
  await createAlbum(running.url, albumName, photoIds.slice(0, 2));
  return { running };
}

/// Opens a source's Grid through its real navigation, so the header under test
/// is the one that source produces.
async function openAlbumGrid(page: Page, url: string, name: string) {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(url);
  await openSources(page);
  const escapedName = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  await page
    .locator("[data-source-list]")
    .getByRole("link", { name: new RegExp(`^${escapedName}(?: |$)`) })
    .click();
  await page
    .locator("[data-grid-status]")
    .filter({ hasText: /^(?:Ready · \d[\d,]* Photos?|0 Photos)$/ })
    .waitFor();
  await waitForGridFrame(page);
}

test("exactly one main landmark and one h1 at 320, 390, and 760", async ({
  page,
}) => {
  const { running } = await gridFixture(LONG_ALBUM_NAME);
  await openAlbumGrid(page, running.url, LONG_ALBUM_NAME);

  for (const width of [320, 390, 760] as const) {
    await page.setViewportSize({ width, height: 844 });
    // The application supplies the one main landmark; no screen may nest another.
    await expect(page.locator("main#app")).toHaveCount(1);
    await expect(page.locator("main")).toHaveCount(1);
    // The Grid heading is the single h1 the view exposes. A hidden view keeps
    // its heading out of the accessibility tree, so this counts what a reader
    // would reach, not what the markup happens to contain.
    await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
    expect(
      await page.evaluate(
        () =>
          Array.from(document.querySelectorAll("h1")).filter((heading) =>
            heading.checkVisibility(),
          ).length,
      ),
    ).toBe(1);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(
      LONG_ALBUM_NAME,
    );
  }
});

test("no Grid header control is clipped at 320, 360, 390, 420, or 760", async ({
  page,
}) => {
  const { running } = await gridFixture(LONG_ALBUM_NAME);
  await openAlbumGrid(page, running.url, LONG_ALBUM_NAME);

  for (const width of HEADER_WIDTHS) {
    await page.setViewportSize({ width, height: 844 });
    for (const selector of HEADER_CONTROLS) {
      const control = page.locator(selector);
      await expect(control).toBeVisible();
      const geometry = await control.evaluate((element) =>
        [element, ...Array.from(element.querySelectorAll("*"))].map((node) => ({
          name: node.tagName + "." + node.className,
          width: node.clientWidth,
          scrollWidth: node.scrollWidth,
          height: node.clientHeight,
          scrollHeight: node.scrollHeight,
        })),
      );
      expect(
        await control.evaluate(clipped),
        `${selector} is clipped at ${width}px: ${JSON.stringify(geometry)}`,
      ).toBe(false);
    }
  }
});

test("only a long source name truncates and stays available to assistive technology", async ({
  page,
}) => {
  const { running } = await gridFixture(LONG_ALBUM_NAME);
  await openAlbumGrid(page, running.url, LONG_ALBUM_NAME);

  await page.setViewportSize({ width: 320, height: 844 });
  const name = page.locator("[data-grid-title]");
  await expect(name).toHaveText(LONG_ALBUM_NAME);
  // The name is the only header element allowed to give up width, and it marks
  // the truncation rather than cutting the text off.
  expect(await name.evaluate(clipped)).toBe(false);
  const overflows = await name.evaluate(
    (element) => element.scrollWidth > element.clientWidth + 1,
  );
  expect(overflows).toBe(true);
  // The full name stays available to assistive technology: the disclosure's
  // accessible name is built from the heading and the count it contains, so
  // the truncated paint never becomes the announced name.
  await expect(page.locator("[data-source-toggle]")).toHaveAccessibleName(
    `Sources — ${LONG_ALBUM_NAME} Ready · 2 Photos`,
  );
  await name.hover();
  await expect(name).toHaveAttribute("title", LONG_ALBUM_NAME);

  await page.setViewportSize({ width: 760, height: 844 });
  // At a wide header the same name fits, so nothing truncates.
  expect(
    await name.evaluate(
      (element) => element.scrollWidth > element.clientWidth + 1,
    ),
  ).toBe(false);
});

test("View options and Select mode keep complete labels at every header width", async ({
  page,
}) => {
  const { running } = await gridFixture(LONG_ALBUM_NAME);
  await openAlbumGrid(page, running.url, LONG_ALBUM_NAME);

  for (const width of HEADER_WIDTHS) {
    await page.setViewportSize({ width, height: 844 });
    // The visible text and the accessible name are the same complete string,
    // so neither entry is shortened to fit the row.
    await expect(page.locator("[data-grid-view-options]")).toHaveText(
      "View options",
    );
    await expect(page.locator("[data-grid-select-mode]")).toHaveText(
      "Select mode",
    );
    await expect(page.locator("[data-grid-view-options]")).toHaveAccessibleName(
      "View options",
    );
    await expect(page.locator("[data-grid-select-mode]")).toHaveAccessibleName(
      "Select mode",
    );
    await expect(page.locator("[data-grid-select-mode]")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  }
});

test("the result count shares the source disclosure without truncating", async ({
  page,
}) => {
  const { running } = await gridFixture(LONG_ALBUM_NAME);
  await openAlbumGrid(page, running.url, LONG_ALBUM_NAME);

  for (const width of HEADER_WIDTHS) {
    await page.setViewportSize({ width, height: 844 });
    const count = page.locator("[data-grid-status]");
    await expect(count).toHaveText(/^Ready · 2 Photos$/);
    // The count is a required header fact, so it keeps the width its text
    // paints; only the name beside it gives up width.
    expect(
      await count.evaluate(
        (element) => element.scrollWidth > element.clientWidth + 1,
      ),
      `the result count truncates at ${width}px`,
    ).toBe(false);
    expect(
      await count.evaluate(
        (element) => element.closest("[data-source-toggle]") !== null,
      ),
      `the count leaves the disclosure at ${width}px`,
    ).toBe(true);
  }
});

test("the Sources separator exposes the width value and its bounds", async ({
  page,
}) => {
  const { running } = await gridFixture(LONG_ALBUM_NAME);
  await page.setViewportSize({ width: 1280, height: 844 });
  await page.goto(running.url);
  await waitForGridFrame(page);

  const resizer = page.locator("[data-source-resizer]");
  await expect(resizer).toBeVisible();
  await expect(resizer).toHaveAttribute("role", "separator");
  await expect(resizer).toHaveAttribute("aria-orientation", "vertical");

  const min = Number(await resizer.getAttribute("aria-valuemin"));
  const max = Number(await resizer.getAttribute("aria-valuemax"));
  expect(min).toBeLessThan(max);

  // A keyboard resize moves the announced value inside the bounds and leaves
  // the applied width reading the same number.
  await resizer.focus();
  const before = Number(await resizer.getAttribute("aria-valuenow"));
  await page.keyboard.press("ArrowRight");
  const after = Number(await resizer.getAttribute("aria-valuenow"));
  expect(after).toBeGreaterThan(before);
  expect(after).toBeLessThanOrEqual(max);
  await expect(page.locator("[data-source-width-status]")).toHaveText(
    `Sources width ${after} pixels, from ${min} to ${max}`,
  );
  const applied = await page.evaluate(() =>
    getComputedStyle(document.querySelector("[data-browser]")!)
      .getPropertyValue("--source-width")
      .trim(),
  );
  expect(applied).toBe(`${after}px`);

  // The separator reaches its upper bound and stays there.
  for (let step = 0; step < 40; step += 1)
    await page.keyboard.press("ArrowRight");
  await expect(resizer).toHaveAttribute("aria-valuenow", String(max));
});

test("the Access screen renders inside the application main landmark", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const running = await server(base, root);
  await page.context().clearCookies();

  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByLabel("Access Token", { exact: true })).toBeVisible();
  // The Access entry screen owns an h1 but must not nest a second main.
  await expect(page.locator("main#app")).toHaveCount(1);
  await expect(page.locator("main")).toHaveCount(1);
  await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
  await expect(page.locator(".access-screen h1")).toHaveText("Slipstream");
  // The Access screen sits inside the application landmark.
  expect(
    await page.evaluate(
      () =>
        document.querySelector(".access-screen")?.closest("main#app") !== null,
    ),
  ).toBe(true);
});

test("the Photo View exposes exactly one h1", async ({ page }) => {
  const { running } = await gridFixture(LONG_ALBUM_NAME);
  await openAlbumGrid(page, running.url, LONG_ALBUM_NAME);
  await page.locator(".photo-cell img").first().click();
  await expect(page.locator("[data-review]")).toBeVisible();

  for (const width of [320, 390, 760] as const) {
    await page.setViewportSize({ width, height: 844 });
    await expect(page.locator("main")).toHaveCount(1);
    await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
    expect(
      await page.evaluate(
        () =>
          Array.from(document.querySelectorAll("h1")).filter((heading) =>
            heading.checkVisibility(),
          ).length,
      ),
    ).toBe(1);
  }
  // The Photo heading retains the name of its current source.
  await expect(page.locator("[data-photo-title]")).toHaveText(LONG_ALBUM_NAME);
  await expect(page.locator(".photo-filename")).not.toBeEmpty();
});
