import { describe, expect, test } from "bun:test";
import { emptySourceMessage } from "./empty-source-message.js";

describe("empty source message", () => {
  test("a Library with no supported files offers a Library check", () => {
    expect(emptySourceMessage("library", "all", {})).toEqual({
      message:
        "No supported Photos found. Check the Library Folder or add supported files, then run Check Library.",
      action: "check-library",
    });
  });

  test("a Folder shares the empty Library state", () => {
    expect(emptySourceMessage("folder", "all", {})).toEqual({
      message:
        "No supported Photos found. Check the Library Folder or add supported files, then run Check Library.",
      action: "check-library",
    });
  });

  test("an empty Album offers the direct route to All Photos", () => {
    const presentation = emptySourceMessage("album", "all", {});
    expect(presentation.action).toBe("browse-library");
    expect(presentation.message).toContain("This Album contains no Photos.");
  });

  test("an empty filtered result offers filter recovery instead of a Library check", () => {
    expect(emptySourceMessage("library", "picked", {})).toEqual({
      message: "No Photos match this filter.",
      action: "adjust-filters",
    });
  });

  test("each additional condition turns an empty result into a no-match state", () => {
    for (const conditions of [
      { ratingMinimum: 0 },
      { ratingMaximum: 5 },
      { kind: "raw" as const },
      { available: true },
      { capturedFrom: "2026-01-01T00:00:00" },
      { capturedBefore: "2026-02-01T00:00:00" },
    ])
      expect(emptySourceMessage("library", "all", conditions)).toEqual({
        message: "No Photos match this filter.",
        action: "adjust-filters",
      });
  });
});
