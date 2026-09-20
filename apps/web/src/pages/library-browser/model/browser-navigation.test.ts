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
  sameSourceView,
  writeEntryState,
  type NavigationDestination,
} from "./browser-navigation.js";

/// The illustrative identifiers the design's examples use. They satisfy the
/// server's identifier shape.
const ALBUM = "00000000-0000-4000-8000-000000000001";
const PHOTO_A = "00000000-0000-4000-8000-000000000004";
const PHOTO_B = "00000000-0000-4000-8000-000000000009";

const destination = (value: NavigationDestination): NavigationDestination =>
  value;

describe("browser address decoding", () => {
  test("the six documented examples decode to their destinations", () => {
    expect(decodeAddress("")).toEqual({
      kind: "destination",
      destination: destination({ source: "library", selection: "all" }),
    });
    expect(decodeAddress("?source=folder&folderPath=")).toEqual({
      kind: "destination",
      destination: destination({
        source: "folder",
        folderPath: "",
        selection: "all",
      }),
    });
    expect(
      decodeAddress(
        "?source=folder&folderPath=RAW%2F26-spring&selection=undecided",
      ),
    ).toEqual({
      kind: "destination",
      destination: destination({
        source: "folder",
        folderPath: "RAW/26-spring",
        selection: "undecided",
      }),
    });
    expect(decodeAddress(`?source=album&albumId=${ALBUM}`)).toEqual({
      kind: "destination",
      destination: destination({
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
      destination: destination({
        source: "album",
        albumId: ALBUM,
        photoId: PHOTO_A,
        order: "capture-time-desc",
        selection: "all",
      }),
    });
    expect(decodeAddress(`?photoId=${PHOTO_B}`)).toEqual({
      kind: "destination",
      destination: destination({
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
      destination: destination({
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
    expect(decodeAddress("?selection=all&selection=undecided")).toEqual({
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
      destination: destination({
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
      destination: destination({
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
      destination: destination({
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
        destination({
          source: "folder",
          folderPath: "",
          selection: "all",
        }),
      ),
    ).toBe("source=folder&folderPath=");
    expect(
      encodeAddress(
        destination({
          source: "folder",
          folderPath: "RAW/26-spring",
          selection: "undecided",
        }),
      ),
    ).toBe("source=folder&folderPath=RAW%2F26-spring&selection=undecided");
    expect(
      encodeAddress(
        destination({ source: "album", albumId: ALBUM, selection: "all" }),
      ),
    ).toBe(`source=album&albumId=${ALBUM}`);
    expect(
      encodeAddress(
        destination({
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
        destination({ source: "library", photoId: PHOTO_B, selection: "all" }),
      ),
    ).toBe(`photoId=${PHOTO_B}`);
    // album-order is the Album default, so it is omitted.
    expect(
      encodeAddress(
        destination({
          source: "album",
          albumId: ALBUM,
          order: "album-order",
          selection: "all",
        }),
      ),
    ).toBe(`source=album&albumId=${ALBUM}`);
  });

  test("every documented example round-trips through encode and decode", () => {
    for (const search of [
      "",
      "?source=folder&folderPath=",
      "?source=folder&folderPath=RAW%2F26-spring&selection=undecided",
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
        destination({
          source: "album",
          albumId: ALBUM,
          photoId: PHOTO_A,
          selection: "undecided",
        }),
      ),
    ).toEqual(
      destination({ source: "album", albumId: ALBUM, selection: "undecided" }),
    );
  });

  test("same source, order, and filter is what lets a traversal reuse a Snapshot", () => {
    const left = destination({
      source: "folder",
      folderPath: "shoot",
      selection: "undecided",
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
        selection: "undecided",
      }),
    ).toBe(false);
    expect(sameSourceView(left, { ...left, photoId: PHOTO_A })).toBe(true);
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
