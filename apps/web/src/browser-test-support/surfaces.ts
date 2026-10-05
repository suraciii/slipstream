import {
  expect,
  type Locator,
  type Page,
  type Request as PlaywrightRequest,
} from "@playwright/test";

export function progressResponse(page: Page, albumId: string, status = 200) {
  return page.waitForResponse(
    (response) =>
      response.url().includes(`/api/albums/${albumId}/progress`) &&
      response.request().method() === "POST" &&
      response.status() === status,
  );
}

export async function actionWithProgress(
  page: Page,
  albumId: string,
  action: () => Promise<unknown>,
) {
  const confirmed = progressResponse(page, albumId);
  await action();
  await confirmed;
}

export async function waitForGridFrame(page: Page) {
  await expect(page.locator("[data-grid-layer]")).toBeVisible();
  await page.evaluate(
    () =>
      new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
  );
}

/// The rendered Grid cells in order; each thumbnail URL names its Photo.
export const gridPhotoIds = (page: Page) =>
  page
    .locator(".photo-cell img")
    .evaluateAll((images) =>
      images.map((image) =>
        new URL(image.getAttribute("src") ?? "", location.origin).pathname
          .split("/")
          .at(-3),
      ),
    );

export async function expectGridOrder(page: Page, ids: string[]) {
  await expect.poll(() => gridPhotoIds(page)).toEqual(ids);
}

export async function openPhotoAndWaitForProgress(
  page: Page,
  albumId: string,
  photo: Locator,
) {
  const confirmed = progressResponse(page, albumId);
  await photo.click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await page.waitForFunction(
    () =>
      Boolean(document.querySelector("[data-stage] img")) ||
      document.body.innerText.includes("Preview unavailable"),
  );
  await confirmed;
}

/// The rendered Grid range as the layout presents it: the first rendered Photo
/// index and one past the last one, derived from cell geometry.
export const recordWindowRequests = (page: Page) => {
  const requested: number[] = [];
  const pending = new Set<PlaywrightRequest>();
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (request.method() !== "GET" || !url.pathname.startsWith("/api/browse/"))
      return;
    requested.push(Number(url.searchParams.get("start")));
    pending.add(request);
  });
  page.on("requestfinished", (request) => pending.delete(request));
  page.on("requestfailed", (request) => pending.delete(request));
  return { requested, pending };
};

/// Settles when every rendered cell has its Photo and no window is in flight.
export async function waitForLoadedReviewImage(page: Page) {
  const image = page.locator("[data-stage] img");
  await expect(image).toBeVisible();
  await page.waitForFunction(() => {
    const candidate = document.querySelector("[data-stage] img");
    return (
      candidate instanceof HTMLImageElement &&
      candidate.complete &&
      candidate.naturalWidth > 0
    );
  });
}

export async function previewImageGeometry(page: Page) {
  return page.locator("[data-stage] img").evaluate((element) => {
    const box = element.getBoundingClientRect();
    const image = element as HTMLImageElement;
    return {
      width: box.width,
      height: box.height,
      left: box.left,
      top: box.top,
      right: box.right,
      bottom: box.bottom,
      naturalWidth: image.naturalWidth,
      naturalHeight: image.naturalHeight,
    };
  });
}

export async function previewStageGeometry(page: Page) {
  return page.locator("[data-stage]").evaluate((element) => {
    const box = element.getBoundingClientRect();
    return {
      width: box.width,
      height: box.height,
      left: box.left,
      top: box.top,
      right: box.right,
      bottom: box.bottom,
    };
  });
}

export async function zoomLevel(page: Page) {
  const text = await page.locator("[data-zoom-level]").textContent();
  return Number((text ?? "").replace("%", ""));
}

// Decision swipes animate the stage back to rest; geometry measured during
// that animation carries the residual offset.
export async function waitForStageAtRest(page: Page) {
  await expect(page.locator("[data-stage]")).toHaveCSS("transform", "none");
}

// A replaced Preview image only receives its zoom geometry once its bytes
// have loaded, so rendered size is the observable proof of applied zoom.
export async function startReview(
  page: Page,
  url: string,
  name = "Review",
  albumId?: string,
) {
  await openGrid(page, url, name);
  const photo = page.locator('[data-photo-index="0"]');
  await expect(photo).toHaveAccessibleName(/Photo 1 of/);
  if (albumId) {
    await openPhotoAndWaitForProgress(page, albumId, photo);
    return;
  }
  await photo.click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await page.waitForFunction(
    () =>
      Boolean(document.querySelector("[data-stage] img")) ||
      document.body.innerText.includes("Preview unavailable"),
  );
}
export async function openGrid(page: Page, url: string, name: string) {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(url);
  await openSources(page);
  const escapedName = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const source = page
    .locator("[data-source-list]")
    .getByRole("link", { name: new RegExp(`^${escapedName}(?: |$)`) });
  // Waiting for the link before clicking keeps a Library summary that is still
  // arriving from surfacing as an unexplained click timeout.
  await expect(source).toBeVisible();
  await source.click();
  await page
    .locator("[data-grid-status]")
    .filter({ hasText: /^(?:Ready · \d[\d,]* Photos?|0 Photos)$/ })
    .waitFor();
  await waitForGridFrame(page);
}
/// Opens the Sources surface from the current-source disclosure. Its accessible
/// name identifies both Sources and the current source.
/// Waits until exactly one of the two views owns the screen. Both surfaces this
/// file drives — Sources and View options — are reachable while a source open
/// is still pending, so this only settles which view is showing.
export async function settledView(page: Page) {
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          Array.from(
            document.querySelectorAll<HTMLElement>(
              "[data-grid-view], [data-photo-view]",
            ),
          ).filter((node) => node.hidden).length,
      ),
    )
    .toBe(1);
}

/// Waits until the view the current address names is the one showing, so a
/// Sources disclosure never races the Grid shell a Photo destination renders
/// before the Photo it resolves opens. A Grid destination has no shell to wait
/// out — its Grid may stay pending for as long as the test holds its open — so
/// the guard reads the address rather than the status text. View options
/// deliberately does not wait for this, because it is reachable while an open
/// is pending.
export async function settledDestination(page: Page) {
  await expect
    .poll(() =>
      page.evaluate(() => {
        const namesPhoto = new URL(window.location.href).searchParams.has(
          "photoId",
        );
        const grid = document.querySelector<HTMLElement>("[data-grid-view]");
        const photo = document.querySelector<HTMLElement>("[data-photo-view]");
        if (photo && !photo.hidden) {
          const position =
            document.querySelector("[data-position]")?.textContent ?? "";
          return /^[1-9]\d* \/ [1-9]\d*$/.test(position.trim());
        }
        // A Grid that is not standing in for a Photo destination has settled,
        // whether or not its source open is still pending.
        return Boolean(grid && !grid.hidden && !namesPhoto);
      }),
    )
    .toBe(true);
}

/// The indicators that carry the connection state: the application header on a
/// wide layout, and the open view's own header on a narrow one.
export const connectionIndicator = (
  page: Page,
  state: "Connected" | "Disconnected",
) =>
  page
    .locator(
      "[data-connection], [data-grid-connection], [data-photo-connection]",
    )
    .filter({ hasText: state });

/// Reads the connection state the open layout presents. A narrow layout has no
/// dedicated brand or connected-status row, so a normal state presents no
/// connection text at all and only a failure is shown beside the affected
/// primary action.
export async function expectConnection(
  page: Page,
  state: "Connected" | "Disconnected",
) {
  if (state === "Connected" && (await page.locator(".app-header").isHidden())) {
    await expect(connectionIndicator(page, "Connected")).toHaveCount(0);
    return;
  }
  await expect(connectionIndicator(page, state)).toBeVisible();
}

/// Closes the Photo View's supporting surfaces the way the platform does, so
/// a disclosure in the background becomes reachable again. A native modal makes
/// the rest of the document inert while it is open.
export async function closePhotoSurfaces(page: Page) {
  const surface = page.locator("[data-photo-tools]");
  if (!(await surface.isVisible())) return;
  await page.keyboard.press("Escape");
  await expect(surface).toBeHidden();
}

export async function openSources(page: Page) {
  await settledDestination(page);
  await closePhotoSurfaces(page);
  // The application establishes its initial destination after it mounts, and
  // opening that source closes Sources. Waiting for the Grid status or the
  // Photo position that establishment writes keeps that close out of the click
  // below, which would otherwise race it on a loaded machine and lose the
  // surface it just opened.
  await page.waitForFunction(() => {
    const status =
      document.querySelector("[data-grid-status]")?.textContent ?? "";
    const position =
      document.querySelector("[data-position]")?.textContent ?? "";
    return status !== "" || /^[1-9]\d* \/ [1-9]\d*$/.test(position.trim());
  });
  // Exactly one disclosure is visible: the Grid's on a narrow Grid, the Photo
  // View's while a Photo is open. Resolving it by visibility keeps a
  // destination render that briefly presents the Grid shell from racing the
  // click.
  const disclosure = page.locator(
    "[data-source-toggle]:visible, [data-photo-source-toggle]:visible",
  );
  if ((await disclosure.count()) === 0) {
    // A wide Grid keeps Sources as the resizable sidebar, which is always shown.
    await expect(page.locator("#source-panel")).toBeVisible();
    return;
  }
  if ((await disclosure.getAttribute("aria-expanded")) !== "true")
    await disclosure.click();
  await expect(page.locator("[data-source-dialog]")).toBeVisible();
}

/// Opens the Photo tools surface from the primary action region and waits for
/// its list. Photo tools is one native modal: at most one supporting surface is
/// active, and closing it returns focus to the More entry that opened it.
export async function openPhotoTools(page: Page) {
  const surface = page.locator("[data-photo-tools]");
  if (await surface.isVisible()) return;
  await page.locator("[data-dock-more]").click();
  await expect(surface).toBeVisible();
  await expect(page.locator("[data-photo-tools-view='tools']")).toBeVisible();
}

/// Moves Photo tools to one of its subviews, which replaces the list content
/// and carries its own local return.
export async function openPhotoToolsView(page: Page, view: string) {
  await openPhotoTools(page);
  const target = page.locator(`[data-photo-tools-view='${view}']`);
  if (await target.isVisible()) return;
  await returnToPhotoTools(page);
  await page.locator(`[data-photo-tools-entry='${view}']`).click();
  await expect(target).toBeVisible();
}

/** Opens the explicit legacy snapshot controls when a browser test exercises
 * the compatibility surface. The product UI keeps this disclosure collapsed
 * in the normal Edit State flow. */
export async function openPhotoEditorAdvanced(page: Page) {
  const details = page.locator("[data-photo-editor-advanced]");
  await expect(details).toBeVisible();
  if ((await details.getAttribute("open")) === null)
    await details.locator("summary").click();
  await expect(details).toHaveAttribute("open", "");
}

/// Returns from a Photo tools subview to its list. The return is local: it adds
/// no browser history entry.
export async function returnToPhotoTools(page: Page) {
  const back = page.locator("[data-photo-tools-return]:visible");
  if ((await back.count()) === 0) return;
  await back.first().click();
  await expect(page.locator("[data-photo-tools-view='tools']")).toBeVisible();
}

/// Closes Photo tools, so the background Photo controls are reachable again.
export async function closePhotoTools(page: Page) {
  const surface = page.locator("[data-photo-tools]");
  if (!(await surface.isVisible())) return;
  await page.locator("[data-photo-tools-close]").click();
  await expect(surface).toBeHidden();
}

/// Opens the explicit Rating view inside the single Photo tools sheet.
export async function openRatingChoices(page: Page) {
  await openPhotoToolsView(page, "rating");
}

/// Opens View options, which owns the Selection State filter, the source
/// order, the thumbnail size, the complete source counts, and the
/// source-specific actions.
export async function openViewOptions(page: Page) {
  const surface = page.locator("[data-view-options]");
  if (await surface.isVisible()) return;
  await settledView(page);
  await page.locator("[data-grid-view-options]").click();
  await expect(surface).toBeVisible();
}

/// Reads the Library summary. A narrow layout keeps it inside the Sources
/// surface, which stays closed until it is disclosed.
export async function applyViewOptions(page: Page) {
  await page.locator("[data-view-options-apply]").click();
  await expect(page.locator("[data-view-options]")).toBeHidden();
}

/// Album membership is read and managed in one panel: the facts list names the
/// Albums this Photo is in, and the Manage panel holds one checkbox per Album.
export async function openMembershipPanel(page: Page) {
  await openPhotoToolsView(page, "albums");
  const manage = page.getByRole("button", { name: "Manage", exact: true });
  if ((await manage.getAttribute("aria-expanded")) !== "true")
    await manage.click();
  await expect(page.locator("[data-membership-panel]")).toBeVisible();
}

export function membershipCheckbox(page: Page, albumName: string) {
  return page.getByRole("checkbox", { name: albumName, exact: true });
}

export async function toggleAlbumMembership(page: Page, albumName: string) {
  await openMembershipPanel(page);
  await membershipCheckbox(page, albumName).click();
}

/// Counts membership reads the page has settled, so a test can wait for a held
/// response to be fully processed instead of guessing at elapsed time. The
/// marker runs in a later task than the fetch continuation that assigns the
/// membership facts.
export function contrastRatio(foreground: string, background: string) {
  const luminance = (value: string) => {
    const channels = value
      .match(/[\d.]+/g)!
      .slice(0, 3)
      .map(Number)
      .map((channel) => {
        const normalized = channel / 255;
        return normalized <= 0.04045
          ? normalized / 12.92
          : ((normalized + 0.055) / 1.055) ** 2.4;
      });
    return (
      0.2126 * channels[0]! + 0.7152 * channels[1]! + 0.0722 * channels[2]!
    );
  };
  const light = Math.max(luminance(foreground), luminance(background));
  const dark = Math.min(luminance(foreground), luminance(background));
  return (light + 0.05) / (dark + 0.05);
}

export const filmstripIndices = (page: Page) =>
  page
    .locator("[data-filmstrip] .filmstrip-cell")
    .evaluateAll((cells) =>
      cells.map((cell) => Number(cell.getAttribute("data-filmstrip-index"))),
    );

export const waitForFilmstripImages = (page: Page) =>
  page.waitForFunction(() => {
    const cells = Array.from(
      document.querySelectorAll<HTMLElement>(
        "[data-filmstrip] .filmstrip-cell",
      ),
    );
    return (
      cells.length > 0 &&
      cells.every((cell) => {
        const image = cell.querySelector<HTMLImageElement>("img");
        if (!image) return true;
        const source = image.getAttribute("src");
        // A cell without a source is an unavailable or not-yet-hydrated
        // placeholder. A non-empty source is real work and must finish before
        // the test continues.
        return (
          source === null ||
          source === "" ||
          (image.complete && image.naturalWidth > 0)
        );
      })
    );
  });

type GridCellGeometry = Readonly<{
  index: number;
  cellWidth: number;
  cellHeight: number;
  mediaWidth: number;
  mediaHeight: number;
  imageWidth: number;
  imageHeight: number;
  naturalWidth: number;
  naturalHeight: number;
  facts: string | null;
  indicatorsOverlapImage: boolean;
  imageInsideMedia: boolean;
}>;

export async function gridCellGeometry(
  page: Page,
): Promise<GridCellGeometry[]> {
  return page.evaluate(() => {
    const overlaps = (inner: DOMRect, outer: DOMRect) =>
      Math.max(
        0,
        Math.min(inner.right, outer.right) - Math.max(inner.left, outer.left),
      ) > 0.5 &&
      Math.max(
        0,
        Math.min(inner.bottom, outer.bottom) - Math.max(inner.top, outer.top),
      ) > 0.5;
    const inside = (inner: DOMRect, outer: DOMRect) =>
      inner.left >= outer.left - 1 &&
      inner.right <= outer.right + 1 &&
      inner.top >= outer.top - 1 &&
      inner.bottom <= outer.bottom + 1;
    return Array.from(
      document.querySelectorAll<HTMLElement>(".photo-cell[data-photo-index]"),
    ).map((cell) => {
      const image = cell.querySelector<HTMLImageElement>("img.thumbnail");
      const media = cell.querySelector<HTMLElement>(".cell-media");
      if (!image || !media) throw new Error("Grid cell geometry is missing");
      const cellBox = cell.getBoundingClientRect();
      const mediaBox = media.getBoundingClientRect();
      const imageBox = image.getBoundingClientRect();
      const indicators = Array.from(
        cell.querySelectorAll<HTMLElement>(
          ".cell-state, .cell-caption, .cell-facts",
        ),
      ).filter((element) => !element.hidden && element.offsetParent !== null);
      const facts = indicators.find((element) =>
        element.classList.contains("cell-facts"),
      );
      return {
        index: Number(cell.dataset.photoIndex),
        cellWidth: cellBox.width,
        cellHeight: cellBox.height,
        mediaWidth: mediaBox.width,
        mediaHeight: mediaBox.height,
        imageWidth: imageBox.width,
        imageHeight: imageBox.height,
        naturalWidth: image.naturalWidth,
        naturalHeight: image.naturalHeight,
        facts: facts?.textContent ?? null,
        indicatorsOverlapImage: indicators.some((element) =>
          overlaps(imageBox, element.getBoundingClientRect()),
        ),
        imageInsideMedia: inside(imageBox, mediaBox),
      };
    });
  });
}

export function expectAspectRatio(geometry: GridCellGeometry): void {
  const rendered = geometry.imageWidth / geometry.imageHeight;
  const natural = geometry.naturalWidth / geometry.naturalHeight;
  expect(Math.abs(rendered - natural) / natural).toBeLessThan(0.02);
}
