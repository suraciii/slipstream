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
/// At phone widths the Undo and More entries sit behind the dock's secondary
/// disclosure, so the disclosure opens first and the entries are measured as
/// the direct targets they are once revealed.
export async function expectControlsAboveSafeViewport(
  page: Page,
  viewportHeight: number,
) {
  expect(page.viewportSize()?.height, "review viewport height").toBe(
    viewportHeight,
  );
  const secondary = page.locator("[data-dock-secondary]");
  const openedSecondary = await secondary.isHidden();
  if (openedSecondary) await page.locator("[data-dock-overflow]").click();
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
  // The disclosure is a measurement aid here, not the tested state: return
  // the review to the dock so later steps see the shipped layout.
  if (openedSecondary) await page.keyboard.press("Escape");
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
      const ranges = Array.from(strip.children).flatMap((child, childIndex) => {
        const range = document.createRange();
        range.selectNodeContents(child);
        return Array.from(range.getClientRects()).map((rect) => ({
          childIndex,
          text: child.textContent ?? "",
          rect,
        }));
      });
      const collisions: string[] = [];
      for (let index = 0; index < ranges.length; index += 1) {
        for (let other = index + 1; other < ranges.length; other += 1) {
          const left = ranges[index]!;
          const right = ranges[other]!;
          // A child with multiple line boxes or block children is allowed to
          // stack its own text; only different tracks may collide.
          if (left.childIndex === right.childIndex) continue;
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

/// Asserts the position and end-of-view boundary paint as separate text ranges
/// inside the navigation group's status stack.
export async function expectNavigationStatusTextRangesClear(page: Page) {
  const overlaps = await page
    .locator("[data-photo-navigation]")
    .evaluate((strip) => {
      const status = strip.querySelector<HTMLElement>(
        ".photo-navigation-status",
      );
      if (!status) return [];
      const elements = Array.from(status.children).filter(
        (child): child is HTMLElement =>
          child instanceof HTMLElement &&
          !child.hasAttribute("hidden") &&
          (child.textContent ?? "").trim() !== "",
      );
      const ranges = elements.flatMap((element, elementIndex) => {
        const range = document.createRange();
        range.selectNodeContents(element);
        return Array.from(range.getClientRects()).map((rect) => ({
          elementIndex,
          text: element.textContent ?? "",
          rect,
        }));
      });
      const collisions: string[] = [];
      for (let index = 0; index < ranges.length; index += 1) {
        for (let other = index + 1; other < ranges.length; other += 1) {
          const left = ranges[index]!;
          const right = ranges[other]!;
          if (left.elementIndex === right.elementIndex) continue;
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
