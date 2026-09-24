import { describe, expect, test } from "bun:test";

import { formatCaptureTime } from "./capture-time.js";

describe("formatCaptureTime", () => {
  test("reshapes the normalized value without reinterpreting its timezone", () => {
    expect(formatCaptureTime("2026-03-04T10:00:00")).toBe("2026-03-04 10:00");
    // Capture Time is camera-local. A UTC designator or offset in the value
    // must not be applied, so the displayed time equals the recorded one.
    expect(formatCaptureTime("2026-03-04T23:30:00Z")).toBe("2026-03-04 23:30");
    expect(formatCaptureTime("2026-03-04T23:30:00+09:00")).toBe(
      "2026-03-04 23:30",
    );
  });

  test("shows an unrecognized value unchanged", () => {
    expect(formatCaptureTime("")).toBe("");
    expect(formatCaptureTime("2026-03-04")).toBe("2026-03-04");
    expect(formatCaptureTime("no Capture Time")).toBe("no Capture Time");
  });
});
