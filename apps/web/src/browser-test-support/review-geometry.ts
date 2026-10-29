import { expect, type Page } from "@playwright/test";

/// The primary controls the continuous review surface must keep above the safe
/// viewport: the context row, the one Previous/position/Next navigation group,
/// and the review dock's Reject, Rating, Pick, Undo, and More actions.
export const primaryReviewControls = [
  "[data-back]",
  "[data-selection]",
  "[data-rating]",
  "[data-dock-previous]",
  "[data-position]",
  "[data-dock-next]",
  "[data-dock-reject]",
  "[data-dock-rating]",
  "[data-dock-pick]",
  "[data-dock-undo]",
  "[data-dock-more]",
];

/// Asserts every primary control finishes above the safe viewport of a layout
/// of the given size, with no control pushed out by a row it did not ask for.
export async function expectControlsAboveSafeViewport(
  page: Page,
  viewportHeight: number,
) {
  expect(page.viewportSize()?.height, "review viewport height").toBe(
    viewportHeight,
  );
  for (const selector of primaryReviewControls) {
    const control = page.locator(selector);
    await expect(control).toBeVisible();
    const box = (await control.boundingBox())!;
    expect(
      box.y,
      `${selector} starts below the safe viewport`,
    ).toBeGreaterThanOrEqual(0);
    expect(
      box.y + box.height,
      `${selector} ends below the safe viewport`,
    ).toBeLessThanOrEqual(viewportHeight);
  }
  expect(
    await page
      .locator("[data-selection]")
      .evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
    "Selection State remains fully readable",
  ).toBe(true);
}

/// Asserts the review strip paints no overlapping text ranges. Element
/// rectangles can miss a collision, because a label paints outside its own box
/// when a track is too narrow for its text, so the painted text ranges of the
/// navigation group's own children are compared instead.
export async function expectNoTextRangeCollision(page: Page) {
  const overlaps = await page
    .locator("[data-photo-navigation]")
    .evaluate((strip) => {
      const ranges = Array.from(strip.children).flatMap((child) => {
        const range = document.createRange();
        range.selectNodeContents(child);
        return Array.from(range.getClientRects()).map((rect) => ({
          text: child.textContent ?? "",
          rect,
        }));
      });
      const collisions: string[] = [];
      for (let index = 0; index < ranges.length; index += 1) {
        for (let other = index + 1; other < ranges.length; other += 1) {
          const left = ranges[index]!;
          const right = ranges[other]!;
          if (
            left.rect.left < right.rect.right &&
            right.rect.left < left.rect.right &&
            left.rect.top < right.rect.bottom &&
            right.rect.top < left.rect.bottom
          )
            collisions.push(`${left.text} / ${right.text}`);
        }
      }
      return collisions;
    });
  expect(overlaps).toEqual([]);
}
