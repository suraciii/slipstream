import { describe, expect, test } from "bun:test";

import {
  addressFor,
  allPhotosDestination,
  decodeAddress,
  encodeAddress,
  gridDestination,
  NAVIGATION_STATE_NAMESPACE,
  NAVIGATION_STATE_VERSION,
  readEntryState,
  sameConditions,
  sameSourceView,
  writeEntryState,
  type NavigationDestination,
} from "./browser-navigation.js";

/// The illustrative identifiers the design's examples use. They satisfy the
/// server's identifier shape.
const ALBUM = "00000000-0000-4000-8000-000000000001";
const PHOTO_A = "00000000-0000-4000-8000-000000000004";
const PHOTO_B = "00000000-0000-4000-8000-000000000009";

/// Every destination literal in these tests omits the fact conditions it does
/// not exercise, so the address decoder's own default applies.
const withoutConditions = (
  value: Omit<NavigationDestination, "conditions">,
): NavigationDestination => ({ ...value, conditions: {} });

describe("browser address decoding", () => {
  test("the six documented examples decode to their destinations", () => {
    expect(decodeAddress("")).toEqual({
      kind: "destination",
      destination: withoutConditions({ source: "library", selection: "all" }),
    });
    expect(decodeAddress("?source=folder&folderPath=")).toEqual({
      kind: "destination",
      destination: withoutConditions({
        source: "folder",
        folderPath: "",
        selection: "all",
      }),
    });
    expect(
      decodeAddress(
        "?source=folder&folderPath=RAW%2F26-spring&selection=unflagged",
      ),
    ).toEqual({
      kind: "destination",
      destination: withoutConditions({
        source: "folder",
        folderPath: "RAW/26-spring",
        selection: "unflagged",
      }),
    });
    expect(decodeAddress(`?source=album&albumId=${ALBUM}`)).toEqual({
      kind: "destination",
      destination: withoutConditions({
        source: "album",
        albumId: ALBUM,
        selection: "all",
      }),
    });
    expect(
      decodeAddress(
        `?source=album&albumId=${ALBUM}&photoId=${PHOTO_A}&order=capture-time-desc`,
      ),
    ).toEqual({
      kind: "destination",
      destination: withoutConditions({
        source: "album",
        albumId: ALBUM,
        photoId: PHOTO_A,
        order: "capture-time-desc",
        selection: "all",
      }),
    });
    expect(decodeAddress(`?photoId=${PHOTO_B}`)).toEqual({
      kind: "destination",
      destination: withoutConditions({
        source: "library",
        photoId: PHOTO_B,
        selection: "all",
      }),
    });
  });

  test("a bare address and an omitted source both mean All Photos", () => {
    expect(decodeAddress("?selection=all")).toEqual({
      kind: "destination",
      destination: allPhotosDestination,
    });
    expect(decodeAddress("?source=library")).toEqual({
      kind: "destination",
      destination: allPhotosDestination,
    });
  });

  test("an Album accepts album-order and rejects it elsewhere", () => {
    expect(
      decodeAddress(`?source=album&albumId=${ALBUM}&order=album-order`),
    ).toEqual({
      kind: "destination",
      destination: withoutConditions({
        source: "album",
        albumId: ALBUM,
        order: "album-order",
        selection: "all",
      }),
    });
    expect(decodeAddress("?order=album-order")).toEqual({ kind: "invalid" });
    expect(
      decodeAddress("?source=folder&folderPath=&order=album-order"),
    ).toEqual({
      kind: "invalid",
    });
  });

  test("a duplicate recognized parameter is invalid", () => {
    expect(
      decodeAddress(`?source=album&albumId=${ALBUM}&albumId=${ALBUM}`),
    ).toEqual({ kind: "invalid" });
    expect(decodeAddress("?source=library&source=folder&folderPath=")).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress(`?photoId=${PHOTO_A}&photoId=${PHOTO_A}`)).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress("?selection=all&selection=unflagged")).toEqual({
      kind: "invalid",
    });
  });

  test("an empty value for an optional parameter is invalid", () => {
    expect(decodeAddress("?source=")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?photoId=")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?order=")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?selection=")).toEqual({ kind: "invalid" });
    expect(decodeAddress(`?source=album&albumId=${ALBUM}&photoId=`)).toEqual({
      kind: "invalid",
    });
  });

  test("fact conditions decode with defaults omitted", () => {
    expect(
      decodeAddress(
        "?rating-min=4&rating-max=5&kind=raw&available=false&captured-from=2026-01-02T09%3A00%3A00&captured-before=2026-01-04T09%3A00%3A00",
      ),
    ).toEqual({
      kind: "destination",
      destination: {
        source: "library",
        selection: "all",
        conditions: {
          ratingMinimum: 4,
          ratingMaximum: 5,
          kind: "raw",
          available: false,
          capturedFrom: "2026-01-02T09:00:00",
          capturedBefore: "2026-01-04T09:00:00",
        },
      },
    });
    // Rating 0 means No Rating, so it is a real bound rather than a default.
    expect(decodeAddress("?rating-min=0")).toEqual({
      kind: "destination",
      destination: {
        source: "library",
        selection: "all",
        conditions: { ratingMinimum: 0 },
      },
    });
    // An omitted parameter applies no condition for that fact.
    expect(decodeAddress("?kind=jpeg")).toEqual({
      kind: "destination",
      destination: {
        source: "library",
        selection: "all",
        conditions: { kind: "jpeg" },
      },
    });
  });

  test("fact condition values that fail their own fact are invalid", () => {
    expect(decodeAddress("?rating-min=6")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?rating-max=6")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?rating-min=-1")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?rating-min=4.5")).toEqual({ kind: "invalid" });
    // An inverted Rating range names an empty view and is invalid.
    expect(decodeAddress("?rating-min=5&rating-max=2")).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress("?kind=heic")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?available=yes")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?captured-from=2026-01-02T09%3A00")).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress("?captured-from=2026-02-30T09%3A00%3A00")).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress("?captured-from=2026-01-02 09%3A00%3A00")).toEqual({
      kind: "invalid",
    });
    // The lower bound is inclusive and the upper bound exclusive, so equal
    // bounds name an empty range and are reversed.
    expect(
      decodeAddress(
        "?captured-from=2026-01-02T09%3A00%3A00&captured-before=2026-01-02T09%3A00%3A00",
      ),
    ).toEqual({ kind: "invalid" });
    expect(
      decodeAddress(
        "?captured-from=2026-01-04T09%3A00%3A00&captured-before=2026-01-02T09%3A00%3A00",
      ),
    ).toEqual({ kind: "invalid" });
  });

  test("a repeated fact condition parameter is invalid", () => {
    expect(decodeAddress("?rating-min=4&rating-min=3")).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress("?kind=raw&kind=jpeg")).toEqual({
      kind: "invalid",
    });
    expect(
      decodeAddress(
        "?captured-from=2026-01-02T09%3A00%3A00&captured-from=2026-01-03T09%3A00%3A00",
      ),
    ).toEqual({ kind: "invalid" });
  });

  test("an empty value for an optional filter parameter is invalid", () => {
    expect(decodeAddress("?rating-min=")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?rating-max=")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?kind=")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?available=")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?captured-from=")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?captured-before=")).toEqual({ kind: "invalid" });
  });

  test("unknown values and source-incompatible options are invalid", () => {
    expect(decodeAddress("?source=everything")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?order=sideways")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?selection=maybe")).toEqual({ kind: "invalid" });
    expect(decodeAddress(`?source=album&albumId=${ALBUM}&folderPath=`)).toEqual(
      {
        kind: "invalid",
      },
    );
    expect(
      decodeAddress("?source=folder&folderPath=&albumId=" + ALBUM),
    ).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress("?source=library&folderPath=shoot")).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress("?source=library&albumId=" + ALBUM)).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress("?source=folder")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?source=album")).toEqual({ kind: "invalid" });
  });

  test("Folder Locations stay relative and component-valid after one decode", () => {
    expect(decodeAddress("?source=folder&folderPath=%2Fetc")).toEqual({
      kind: "invalid",
    });
    expect(
      decodeAddress("?source=folder&folderPath=RAW%2F..%2F..%2Fetc"),
    ).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress("?source=folder&folderPath=..")).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress("?source=folder&folderPath=.")).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress("?source=folder&folderPath=RAW%2F%2Fspring")).toEqual({
      kind: "invalid",
    });
    expect(
      decodeAddress(
        `?source=folder&folderPath=${encodeURIComponent("a".repeat(1025))}`,
      ),
    ).toEqual({ kind: "invalid" });
    expect(
      decodeAddress(
        `?source=folder&folderPath=${encodeURIComponent("a".repeat(1024))}`,
      ),
    ).toEqual({
      kind: "destination",
      destination: withoutConditions({
        source: "folder",
        folderPath: "a".repeat(1024),
        selection: "all",
      }),
    });
    expect(
      decodeAddress(
        `?source=folder&folderPath=${Array.from({ length: 33 }, () => "a").join("/")}`,
      ),
    ).toEqual({ kind: "invalid" });
  });

  test("an encoded reserved character inside a valid Location round-trips", () => {
    const decoded = decodeAddress("?source=folder&folderPath=26-spring%3F");
    expect(decoded).toEqual({
      kind: "destination",
      destination: withoutConditions({
        source: "folder",
        folderPath: "26-spring?",
        selection: "all",
      }),
    });
    if (decoded.kind !== "destination") return;
    expect(encodeAddress(decoded.destination)).toBe(
      "source=folder&folderPath=26-spring%3F",
    );
    // The encoded slash is a separator after the single decode, not an escape
    // from containment.
    const reEncoded = new URLSearchParams(encodeAddress(decoded.destination));
    expect(
      decodeAddress(`?source=folder&folderPath=${reEncoded.get("folderPath")}`),
    ).toEqual(decoded);
  });

  test("unknown query keys are ignored and never make an address invalid", () => {
    expect(decodeAddress(`?photoId=${PHOTO_A}&panel=tools&zoom=200`)).toEqual({
      kind: "destination",
      destination: withoutConditions({
        source: "library",
        photoId: PHOTO_A,
        selection: "all",
      }),
    });
    expect(decodeAddress("?returnUrl=%2Fadmin&source=library")).toEqual({
      kind: "destination",
      destination: allPhotosDestination,
    });
  });

  test("identifiers outside the server's shape are invalid", () => {
    expect(decodeAddress("?photoId=short")).toEqual({ kind: "invalid" });
    expect(decodeAddress("?photoId=" + "g".repeat(40))).toEqual({
      kind: "invalid",
    });
    expect(decodeAddress(`?source=album&albumId=${"a".repeat(65)}`)).toEqual({
      kind: "invalid",
    });
  });
});

describe("browser address encoding", () => {
  test("the encoder omits defaults and keeps canonical parameter order", () => {
    expect(encodeAddress(allPhotosDestination)).toBe("");
    expect(
      encodeAddress(
        withoutConditions({
          source: "folder",
          folderPath: "",
          selection: "all",
        }),
      ),
    ).toBe("source=folder&folderPath=");
    expect(
      encodeAddress(
        withoutConditions({
          source: "folder",
          folderPath: "RAW/26-spring",
          selection: "unflagged",
        }),
      ),
    ).toBe("source=folder&folderPath=RAW%2F26-spring&selection=unflagged");
    expect(
      encodeAddress(
        withoutConditions({
          source: "album",
          albumId: ALBUM,
          selection: "all",
        }),
      ),
    ).toBe(`source=album&albumId=${ALBUM}`);
    expect(
      encodeAddress(
        withoutConditions({
          source: "album",
          albumId: ALBUM,
          photoId: PHOTO_A,
          order: "capture-time-desc",
          selection: "all",
        }),
      ),
    ).toBe(
      `source=album&albumId=${ALBUM}&photoId=${PHOTO_A}&order=capture-time-desc`,
    );
    expect(
      encodeAddress(
        withoutConditions({
          source: "library",
          photoId: PHOTO_B,
          selection: "all",
        }),
      ),
    ).toBe(`photoId=${PHOTO_B}`);
    // album-order is the Album default, so it is omitted.
    expect(
      encodeAddress(
        withoutConditions({
          source: "album",
          albumId: ALBUM,
          order: "album-order",
          selection: "all",
        }),
      ),
    ).toBe(`source=album&albumId=${ALBUM}`);
  });

  test("fact conditions follow selection in canonical order", () => {
    expect(
      encodeAddress({
        source: "library",
        selection: "unflagged",
        conditions: {
          ratingMinimum: 4,
          ratingMaximum: 5,
          kind: "raw",
          available: true,
          capturedFrom: "2026-01-02T09:00:00",
          capturedBefore: "2026-01-04T09:00:00",
        },
      }),
    ).toBe(
      "selection=unflagged&rating-min=4&rating-max=5&kind=raw&available=true" +
        "&captured-from=2026-01-02T09%3A00%3A00" +
        "&captured-before=2026-01-04T09%3A00%3A00",
    );
    // A condition on its own is emitted after selection, and the other
    // conditions are omitted rather than sent as empty values.
    expect(
      encodeAddress({
        source: "library",
        selection: "all",
        conditions: { kind: "jpeg" },
      }),
    ).toBe("kind=jpeg");
    // Rating 0 is a real bound, so it is emitted rather than omitted.
    expect(
      encodeAddress({
        source: "library",
        selection: "all",
        conditions: { ratingMinimum: 0 },
      }),
    ).toBe("rating-min=0");
    // available=false is a real condition, so it is emitted.
    expect(
      encodeAddress({
        source: "library",
        selection: "all",
        conditions: { available: false },
      }),
    ).toBe("available=false");
  });

  test("a full condition set round-trips through encode and decode", () => {
    const conditions = {
      ratingMinimum: 4,
      ratingMaximum: 5,
      kind: "raw",
      available: false,
      capturedFrom: "2026-01-02T09:00:00",
      capturedBefore: "2026-01-04T09:00:00",
    } as const;
    const search = encodeAddress({
      source: "library",
      selection: "all",
      conditions,
    });
    expect(decodeAddress(search)).toEqual({
      kind: "destination",
      destination: { source: "library", selection: "all", conditions },
    });
  });

  test("every documented example round-trips through encode and decode", () => {
    for (const search of [
      "",
      "?source=folder&folderPath=",
      "?source=folder&folderPath=RAW%2F26-spring&selection=unflagged",
      `?source=album&albumId=${ALBUM}`,
      `?source=album&albumId=${ALBUM}&photoId=${PHOTO_A}&order=capture-time-desc`,
      `?photoId=${PHOTO_B}`,
    ]) {
      const decoded = decodeAddress(search);
      expect(decoded.kind).toBe("destination");
      if (decoded.kind !== "destination") return;
      const encoded = encodeAddress(decoded.destination);
      expect(encoded ? `?${encoded}` : "").toBe(search);
    }
  });

  test("an address is canonicalized by dropping unknown keys", () => {
    const decoded = decodeAddress(
      `?panel=tools&source=folder&folderPath=&zoom=2`,
    );
    expect(decoded.kind).toBe("destination");
    if (decoded.kind !== "destination") return;
    expect(addressFor(decoded.destination)).toBe("/?source=folder&folderPath=");
  });

  test("a Grid destination drops the Photo identity", () => {
    expect(
      gridDestination(
        withoutConditions({
          source: "album",
          albumId: ALBUM,
          photoId: PHOTO_A,
          selection: "unflagged",
        }),
      ),
    ).toEqual(
      withoutConditions({
        source: "album",
        albumId: ALBUM,
        selection: "unflagged",
      }),
    );
  });

  test("same source, order, and filter is what lets a traversal reuse a Snapshot", () => {
    const left = withoutConditions({
      source: "folder",
      folderPath: "shoot",
      selection: "unflagged",
    });
    expect(sameSourceView(left, { ...left })).toBe(true);
    expect(sameSourceView(left, { ...left, selection: "all" })).toBe(false);
    expect(sameSourceView(left, { ...left, order: "capture-time-desc" })).toBe(
      false,
    );
    expect(
      sameSourceView(left, {
        source: "folder",
        folderPath: "other",
        conditions: {},
        selection: "unflagged",
      }),
    ).toBe(false);
    expect(sameSourceView(left, { ...left, photoId: PHOTO_A })).toBe(true);
  });
});

describe("view identity", () => {
  const library = (conditions: NavigationDestination["conditions"]) => ({
    source: "library" as const,
    selection: "all" as const,
    conditions,
  });

  test("an omitted condition and an explicitly default one are the same view", () => {
    // An explicitly default condition and an omitted one are the same view, so
    // a traversal reuses the live Snapshot instead of reopening the source.
    expect(sameConditions({}, { ratingMinimum: undefined as never })).toBe(
      true,
    );
    expect(sameConditions({}, { kind: undefined as never })).toBe(true);
  });

  test("a changed condition is a different view", () => {
    expect(sameSourceView(library({}), library({ ratingMinimum: 4 }))).toBe(
      false,
    );
    expect(sameSourceView(library({}), library({ kind: "raw" }))).toBe(false);
    expect(sameSourceView(library({}), library({ available: false }))).toBe(
      false,
    );
    expect(
      sameSourceView(
        library({}),
        library({ capturedFrom: "2026-01-02T09:00:00" }),
      ),
    ).toBe(false);
    expect(
      sameSourceView(
        library({ ratingMinimum: 4 }),
        library({ ratingMinimum: 4, ratingMaximum: 5 }),
      ),
    ).toBe(false);
  });

  test("source, order, and Selection State still decide reuse", () => {
    expect(
      sameSourceView(
        { source: "library", selection: "all", conditions: {} },
        { source: "library", selection: "all", conditions: {} },
      ),
    ).toBe(true);
    expect(
      sameSourceView(
        { source: "library", selection: "all", conditions: {} },
        { source: "library", selection: "rejected", conditions: {} },
      ),
    ).toBe(false);
    expect(
      sameSourceView(
        { source: "library", selection: "all", conditions: {} },
        {
          source: "library",
          selection: "all",
          order: "capture-time-desc",
          conditions: {},
        },
      ),
    ).toBe(false);
  });
});

describe("history entry state", () => {
  const entry = {
    version: NAVIGATION_STATE_VERSION,
    entryId: "e1",
    anchor: { photoId: PHOTO_A, indexHint: 12, offset: 24 },
    focus: { kind: "photo" as const, photoId: PHOTO_A },
    parentGridEntryId: "e0",
    folderPublication: "publication-1",
  };

  test("a valid entry is read back with its bounded metadata", () => {
    expect(readEntryState({ [NAVIGATION_STATE_NAMESPACE]: entry })).toEqual(
      entry,
    );
  });

  test("absent, malformed, and unknown-version state is a direct entry", () => {
    expect(readEntryState(undefined)).toBeUndefined();
    expect(readEntryState(null)).toBeUndefined();
    expect(readEntryState({})).toBeUndefined();
    expect(
      readEntryState({ [NAVIGATION_STATE_NAMESPACE]: {} }),
    ).toBeUndefined();
    expect(
      readEntryState({
        [NAVIGATION_STATE_NAMESPACE]: { ...entry, version: 2 },
      }),
    ).toBeUndefined();
    expect(
      readEntryState({
        [NAVIGATION_STATE_NAMESPACE]: { ...entry, entryId: "" },
      }),
    ).toBeUndefined();
    expect(
      readEntryState({
        [NAVIGATION_STATE_NAMESPACE]: { ...entry, anchor: { photoId: "x" } },
      }),
    ).toBeUndefined();
    expect(
      readEntryState({
        [NAVIGATION_STATE_NAMESPACE]: { ...entry, focus: { kind: "cell" } },
      }),
    ).toBeUndefined();
    expect(
      readEntryState({
        [NAVIGATION_STATE_NAMESPACE]: { ...entry, parentGridEntryId: 4 },
      }),
    ).toBeUndefined();
  });

  test("the namespace merges without overwriting other history state fields", () => {
    expect(
      writeEntryState(
        { scroll: 12, other: true },
        {
          version: NAVIGATION_STATE_VERSION,
          entryId: "e1",
        },
      ),
    ).toEqual({
      scroll: 12,
      other: true,
      [NAVIGATION_STATE_NAMESPACE]: {
        version: NAVIGATION_STATE_VERSION,
        entryId: "e1",
      },
    });
    expect(writeEntryState(undefined, entry)).toEqual({
      [NAVIGATION_STATE_NAMESPACE]: entry,
    });
  });
});
