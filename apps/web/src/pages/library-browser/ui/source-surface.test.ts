import { describe, expect, test } from "bun:test";
import {
  SOURCE_WIDTH_DEFAULT,
  SOURCE_WIDTH_MAXIMUM,
  SOURCE_WIDTH_MINIMUM,
  SOURCE_WIDTH_STEP,
  clampSourceWidth,
  sourceWidthAnnouncement,
} from "./source-surface.js";

describe("source width bounds", () => {
  test("clamps to the separator's min and max", () => {
    expect(clampSourceWidth(0)).toBe(SOURCE_WIDTH_MINIMUM);
    expect(clampSourceWidth(SOURCE_WIDTH_MINIMUM - 40)).toBe(
      SOURCE_WIDTH_MINIMUM,
    );
    expect(clampSourceWidth(SOURCE_WIDTH_MAXIMUM + 40)).toBe(
      SOURCE_WIDTH_MAXIMUM,
    );
    expect(clampSourceWidth(5000)).toBe(SOURCE_WIDTH_MAXIMUM);
  });

  test("passes a value inside the bounds through unchanged", () => {
    expect(clampSourceWidth(224)).toBe(224);
    expect(clampSourceWidth(300)).toBe(300);
  });

  test("defaults inside the bounds", () => {
    expect(SOURCE_WIDTH_DEFAULT).toBeGreaterThanOrEqual(SOURCE_WIDTH_MINIMUM);
    expect(SOURCE_WIDTH_DEFAULT).toBeLessThanOrEqual(SOURCE_WIDTH_MAXIMUM);
  });

  test("the one keyboard step stays inside the bounds", () => {
    expect(SOURCE_WIDTH_STEP).toBeGreaterThan(0);
    // A step from either bound cannot escape it, so a repeated keypress
    // settles at the bound instead of overshooting into an illegal width.
    expect(clampSourceWidth(SOURCE_WIDTH_MINIMUM - SOURCE_WIDTH_STEP)).toBe(
      SOURCE_WIDTH_MINIMUM,
    );
    expect(clampSourceWidth(SOURCE_WIDTH_MAXIMUM + SOURCE_WIDTH_STEP)).toBe(
      SOURCE_WIDTH_MAXIMUM,
    );
  });
});

describe("source width announcement", () => {
  test("states the applied value and both bounds", () => {
    expect(sourceWidthAnnouncement(224)).toBe(
      `Sources width 224 pixels, from ${SOURCE_WIDTH_MINIMUM} to ${SOURCE_WIDTH_MAXIMUM}`,
    );
  });

  test("announces the clamped value, never the requested one", () => {
    // The announcement must read the width owner's value, so an out-of-range
    // request announces what actually moved the Sources width.
    expect(sourceWidthAnnouncement(SOURCE_WIDTH_MAXIMUM + 500)).toBe(
      `Sources width ${SOURCE_WIDTH_MAXIMUM} pixels, from ${SOURCE_WIDTH_MINIMUM} to ${SOURCE_WIDTH_MAXIMUM}`,
    );
    expect(sourceWidthAnnouncement(SOURCE_WIDTH_MINIMUM - 500)).toBe(
      `Sources width ${SOURCE_WIDTH_MINIMUM} pixels, from ${SOURCE_WIDTH_MINIMUM} to ${SOURCE_WIDTH_MAXIMUM}`,
    );
  });

  test("a fractional step announces a whole pixel width", () => {
    // Pointer moves land on sub-pixel widths, so the announcement reads a whole
    // pixel rather than a fraction the separator never paints.
    expect(sourceWidthAnnouncement(224.4)).toContain("224 pixels");
    expect(sourceWidthAnnouncement(224.6)).toContain("225 pixels");
  });
});
