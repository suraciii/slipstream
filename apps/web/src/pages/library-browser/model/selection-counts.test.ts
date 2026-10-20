import { describe, expect, test } from "bun:test";
import type { SelectionCounts } from "../api/contracts.js";
import { adjustedSelectionCounts } from "./selection-counts.js";

const counts = (values: Partial<SelectionCounts> = {}): SelectionCounts => ({
  picked: 0,
  rejected: 0,
  unflagged: 0,
  ...values,
});

describe("adjusted selection counts", () => {
  test("an unchanged Selection State returns the original counts object", () => {
    const source = counts({ picked: 2 });
    expect(adjustedSelectionCounts(source, "picked", "picked")).toBe(source);
  });

  test("a transition moves one Photo out of the prior bucket", () => {
    const adjusted = adjustedSelectionCounts(
      counts({ picked: 3, rejected: 1 }),
      "picked",
      "rejected",
    );
    expect(adjusted).toEqual({ picked: 2, rejected: 2, unflagged: 0 });
  });

  test("an emptied bucket never drops below zero", () => {
    expect(
      adjustedSelectionCounts(
        counts({ picked: 0, unflagged: 5 }),
        "picked",
        "unflagged",
      ),
    ).toEqual({ picked: 0, rejected: 0, unflagged: 6 });
  });

  test("the changed result is frozen", () => {
    const adjusted = adjustedSelectionCounts(
      counts({ unflagged: 1 }),
      "unflagged",
      "picked",
    );
    expect(Object.isFrozen(adjusted)).toBe(true);
  });
});
