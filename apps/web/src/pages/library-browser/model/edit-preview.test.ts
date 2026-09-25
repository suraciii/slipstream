import { describe, expect, test } from "bun:test";
import {
  BASELINE_SETTINGS,
  CURRENT_SETTINGS,
  comparisonIsCurrent,
  comparisonRefusal,
  editPreviewUri,
  encodedSourceRevision,
  type Comparison,
} from "./edit-preview.js";

const headers = (values: Record<string, string>) => ({
  get: (name: string) => values[name] ?? null,
});

const served = (overrides: Record<string, string> = {}) => ({
  "slipstream-edit-preview-photo-id": "photo-1",
  "slipstream-edit-preview-stage": "develop",
  "slipstream-edit-preview-settings": BASELINE_SETTINGS,
  "slipstream-edit-preview-source-revision": encodedSourceRevision("café"),
  ...overrides,
});

const expected: Comparison = {
  photoId: "photo-1",
  stage: "develop",
  sourceRevision: "café",
};

describe("editPreviewUri", () => {
  test("asks for the chosen stage under the chosen settings selector", () => {
    expect(editPreviewUri("photo-1", "develop", CURRENT_SETTINGS)).toBe(
      "/api/photos/photo-1/edit-preview/develop?settings=current",
    );
    // The comparison keeps the stage and its display conversion constant, so
    // it is the same route and stage under the baseline selector.
    expect(editPreviewUri("photo-1", "develop", BASELINE_SETTINGS)).toBe(
      "/api/photos/photo-1/edit-preview/develop?settings=baseline",
    );
  });

  test("encodes a Photo ID instead of splicing it into the path", () => {
    expect(editPreviewUri("photo/one", "develop", BASELINE_SETTINGS)).toBe(
      "/api/photos/photo%2Fone/edit-preview/develop?settings=baseline",
    );
  });
});

describe("encodedSourceRevision", () => {
  test("encodes the opaque revision as the route's header does", () => {
    expect(encodedSourceRevision("")).toBe("");
    expect(encodedSourceRevision("abc")).toBe("616263");
    // Multi-byte revisions are encoded per UTF-8 byte, not per character.
    expect(encodedSourceRevision("é")).toBe("c3a9");
  });
});

describe("comparisonIsCurrent", () => {
  test("survives a saved-settings change and follows the Photo, stage, and source", () => {
    expect(comparisonIsCurrent(expected, expected)).toBe(true);
    // The baseline names no saved recipe, so a recipe revision is not part of
    // the comparison's identity: only the Photo, stage, and source revision are.
    expect(
      comparisonIsCurrent(expected, { ...expected, sourceRevision: "beef" }),
    ).toBe(false);
    expect(comparisonIsCurrent(expected, { ...expected, stage: "film" })).toBe(
      false,
    );
    expect(
      comparisonIsCurrent(expected, { ...expected, photoId: "photo-2" }),
    ).toBe(false);
    expect(comparisonIsCurrent(undefined, expected)).toBe(false);
  });

  test("holds for a Photo whose source revision is unknown", () => {
    const unread = { ...expected, sourceRevision: null };
    expect(comparisonIsCurrent(unread, unread)).toBe(true);
  });
});

describe("comparisonRefusal", () => {
  test("accepts the requested comparison and nothing else", () => {
    expect(comparisonRefusal(headers(served()), expected)).toBe("");
  });

  test("refuses a rendition of the current settings", () => {
    // The route answers `current` as well; presenting it as the comparison
    // would compare the current settings against themselves.
    expect(
      comparisonRefusal(
        headers(served({ "slipstream-edit-preview-settings": "current" })),
        expected,
      ),
    ).not.toBe("");
    expect(comparisonRefusal(headers({}), expected)).not.toBe("");
  });

  test("refuses another Photo, stage, or source revision", () => {
    expect(
      comparisonRefusal(
        headers(served({ "slipstream-edit-preview-photo-id": "photo-2" })),
        expected,
      ),
    ).not.toBe("");
    expect(
      comparisonRefusal(
        headers(served({ "slipstream-edit-preview-stage": "film" })),
        expected,
      ),
    ).not.toBe("");
    // The header carries the wire encoding of the revision, so a comparison
    // requested for a source that moved while it rendered is refused.
    expect(
      comparisonRefusal(
        headers({
          ...served(),
          "slipstream-edit-preview-source-revision":
            encodedSourceRevision("another source"),
        }),
        expected,
      ),
    ).not.toBe("");
  });

  test("refuses a rendition served for a source revision the workspace has none of", () => {
    const unread = { ...expected, sourceRevision: null };
    expect(comparisonRefusal(headers(served()), unread)).not.toBe("");
    expect(
      comparisonRefusal(
        headers(served({ "slipstream-edit-preview-source-revision": "" })),
        unread,
      ),
    ).toBe("");
  });
});
