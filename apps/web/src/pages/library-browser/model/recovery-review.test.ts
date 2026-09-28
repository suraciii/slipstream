import { describe, expect, test } from "bun:test";
import {
  isRecoveryMappingApplicable,
  recoveryApplyMappings,
  type RecoveryMappingChoice,
} from "./recovery-review.js";
import type { RecoveryMapping } from "../api/recovery.js";

const mapping = (
  overrides: Partial<RecoveryMapping> = {},
): RecoveryMapping => ({
  mappingId: "map-1",
  originalId: "orig-1",
  photoId: "photo-1",
  fromLocation: "2023/travel/a.ARW",
  toLocation: "2024/travel/a.ARW",
  kind: "raw",
  outcome: "matched",
  verified: true,
  blockedReason: null,
  retire: null,
  ...overrides,
});

const choice = (
  overrides: Partial<RecoveryMappingChoice> = {},
): RecoveryMappingChoice => ({
  retireChosen: false,
  unverifiedConfirmed: false,
  ...overrides,
});

describe("recovery review apply batch", () => {
  test("repeats each reviewed mappingId and its destination", () => {
    const batch = recoveryApplyMappings(
      [mapping({ mappingId: "map-7", toLocation: "2024/x.ARW" })],
      new Map(),
    );
    expect(batch).toEqual([
      {
        originalId: "orig-1",
        newLocation: "2024/x.ARW",
        mappingId: "map-7",
      },
    ]);
  });

  test("a blocked mapping never applies, even fully confirmed", () => {
    const blocked = mapping({
      mappingId: "map-b",
      outcome: "occupied",
      blockedReason: "destination-in-use",
      retire: {
        photoId: "photo-9",
        originalId: "orig-9",
        location: "2024/travel/a.ARW",
      },
    });
    const choices = new Map([
      ["map-b", choice({ retireChosen: true, unverifiedConfirmed: true })],
    ]);
    expect(isRecoveryMappingApplicable(blocked, choices.get("map-b"))).toBe(
      false,
    );
    expect(recoveryApplyMappings([blocked], choices)).toEqual([]);
  });

  test("an unverified mapping requires the explicit acknowledgement", () => {
    const unverified = mapping({ mappingId: "map-u", verified: false });
    expect(recoveryApplyMappings([unverified], new Map())).toEqual([]);
    const acknowledged = recoveryApplyMappings(
      [unverified],
      new Map([["map-u", choice({ unverifiedConfirmed: true })]]),
    );
    expect(acknowledged).toEqual([
      {
        originalId: "orig-1",
        newLocation: "2024/travel/a.ARW",
        mappingId: "map-u",
        confirmUnverifiedContent: true,
      },
    ]);
  });

  test("a verified mapping never sends the unverified confirmation", () => {
    const batch = recoveryApplyMappings(
      [mapping({ mappingId: "map-v", verified: true })],
      new Map([["map-v", choice({ unverifiedConfirmed: true })]]),
    );
    expect(batch).toEqual([
      {
        originalId: "orig-1",
        newLocation: "2024/travel/a.ARW",
        mappingId: "map-v",
      },
    ]);
  });

  test("a retire candidate applies only with the explicit retire choice", () => {
    const occupied = mapping({
      mappingId: "map-o",
      outcome: "occupied",
      retire: {
        photoId: "photo-9",
        originalId: "orig-9",
        location: "2024/travel/a.ARW",
      },
    });
    expect(recoveryApplyMappings([occupied], new Map())).toEqual([]);
    const retired = recoveryApplyMappings(
      [occupied],
      new Map([["map-o", choice({ retireChosen: true })]]),
    );
    expect(retired).toEqual([
      {
        originalId: "orig-1",
        newLocation: "2024/travel/a.ARW",
        mappingId: "map-o",
        retirePhotoId: "photo-9",
      },
    ]);
  });

  test("a mapping without a retire candidate never sends retirePhotoId", () => {
    const batch = recoveryApplyMappings(
      [mapping({ mappingId: "map-p" })],
      new Map([["map-p", choice({ retireChosen: true })]]),
    );
    expect(batch).toEqual([
      {
        originalId: "orig-1",
        newLocation: "2024/travel/a.ARW",
        mappingId: "map-p",
      },
    ]);
  });

  test("a mixed batch keeps blocked and unconfirmed mappings out", () => {
    const batch = recoveryApplyMappings(
      [
        mapping({ mappingId: "map-ready" }),
        mapping({ mappingId: "map-blocked", blockedReason: "missing" }),
        mapping({ mappingId: "map-unverified", verified: false }),
        mapping({
          mappingId: "map-both",
          verified: false,
          retire: {
            photoId: "photo-9",
            originalId: "orig-9",
            location: "2024/travel/c.ARW",
          },
        }),
      ],
      new Map([
        // One acknowledgement alone cannot carry a retire choice, and one
        // retire choice alone cannot cover unverified content.
        ["map-unverified", choice({ retireChosen: true })],
        ["map-both", choice({ unverifiedConfirmed: true })],
      ]),
    );
    expect(batch).toEqual([
      {
        originalId: "orig-1",
        newLocation: "2024/travel/a.ARW",
        mappingId: "map-ready",
      },
    ]);
  });
});
