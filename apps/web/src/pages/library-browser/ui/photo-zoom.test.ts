import { describe, expect, test } from "bun:test";
import { previewPixelsEnlarged } from "./photo-zoom.js";

describe("Preview display condition", () => {
  test("Fit reports enlargement only when the area enlarges both dimensions", () => {
    expect(previewPixelsEnlarged(640, 480, 960, 720)).toBe(true);
    expect(previewPixelsEnlarged(640, 480, 640, 480)).toBe(false);
  });

  test("manual zoom above 100% enlarges pixels", () => {
    expect(previewPixelsEnlarged(2560, 1920, 800, 600, 200)).toBe(true);
    expect(previewPixelsEnlarged(2560, 1920, 800, 600, 100)).toBe(false);
  });

  test("resize crosses the Fit threshold and returning to Fit clears manual enlargement", () => {
    expect(previewPixelsEnlarged(640, 480, 600, 450)).toBe(false);
    expect(previewPixelsEnlarged(640, 480, 800, 600)).toBe(true);
    expect(previewPixelsEnlarged(2560, 1920, 800, 600, 200)).toBe(true);
    expect(previewPixelsEnlarged(2560, 1920, 800, 600)).toBe(false);
  });

  test("a source-limited Preview can fit without pixel enlargement", () => {
    expect(previewPixelsEnlarged(640, 480, 320, 240)).toBe(false);
    expect(previewPixelsEnlarged(640, 480, 0, 0)).toBe(false);
  });
});
