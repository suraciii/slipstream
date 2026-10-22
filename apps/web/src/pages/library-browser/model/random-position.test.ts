import { describe, expect, test } from "bun:test";
import { randomPosition } from "./random-position.js";

describe("randomPosition", () => {
  test("samples the complete position domain through the injected RNG", () => {
    expect(randomPosition(5, (maximum) => maximum - 1)).toBe(4);
    expect(randomPosition(5, () => 0)).toBe(0);
  });

  test("does not produce a target for an empty or invalid sequence", () => {
    expect(randomPosition(0, () => 0)).toBeUndefined();
    expect(randomPosition(-1, () => 0)).toBeUndefined();
    expect(randomPosition(Number.POSITIVE_INFINITY, () => 0)).toBeUndefined();
    expect(randomPosition(5, () => 5)).toBeUndefined();
  });

  test("allows the same position to be selected again", () => {
    const random = () => 2;
    expect(randomPosition(4, random)).toBe(2);
    expect(randomPosition(4, random)).toBe(2);
  });
});
