import { describe, expect, test } from "bun:test";

import { formatPhotoCount } from "./photo-count.js";

describe("formatPhotoCount", () => {
  test("labels a count with the singular or plural Photo noun", () => {
    expect(formatPhotoCount(0)).toBe("0 Photos");
    expect(formatPhotoCount(1)).toBe("1 Photo");
    expect(formatPhotoCount(2)).toBe("2 Photos");
  });
});
