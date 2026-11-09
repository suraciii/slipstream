/// Browser addresses for Library Browser destinations.
///
/// This pure module owns the Browser Address Contract described by
/// [Browser Navigation](../../../../../../specs/library-browser/experience/design.md):
/// source kinds, canonical parameter order, bounded validators, decoding,
/// encoding, and view identity. It retains no state, issues no HTTP, and holds
/// no Photo facts. The history owner in `browser-navigation.ts` uses these
/// functions.

/// The three Library sources an address may name. Omission means `library`.
export type NavigationSourceKind = "library" | "folder" | "album";

/// An explicit view order. `album-order` is valid only for an Album and is
/// that source's default, so the encoder omits it.
export type NavigationViewOrder =
  | "capture-time-asc"
  | "capture-time-desc"
  | "album-order";

/// The Selection State filter an address may request. Omission means `all`.
export type NavigationSelectionFilter =
  | "all"
  | "unflagged"
  | "picked"
  | "rejected";

/// The Original File kind an address may request. A RAW Photo and a
/// same-basename JPEG Photo stay independent Photos under any kind condition.
export type NavigationOriginalKind = "raw" | "jpeg";

/// The defined Photo fact conditions an address may request. Omission of one
/// field applies no condition for that fact, and different facts combine with
/// AND. The same facts carry the same meaning as the CLI query contract, so
/// a Web link never names a condition the CLI cannot express.
export type NavigationFilterConditions = Readonly<{
  ratingMinimum?: number;
  ratingMaximum?: number;
  kind?: NavigationOriginalKind;
  available?: boolean;
  capturedFrom?: string;
  capturedBefore?: string;
}>;

/// One validated destination: a source reference, view order, Selection State
/// filter, defined fact conditions, and an optional stable Photo identity.
/// Absence of `photoId` means Grid.
export type NavigationDestination = Readonly<{
  source: NavigationSourceKind;
  folderPath?: string;
  albumId?: string;
  photoId?: string;
  order?: NavigationViewOrder;
  selection: NavigationSelectionFilter;
  conditions: NavigationFilterConditions;
}>;

/// The recognized parameters in their canonical address order.
const PARAMETER_ORDER = [
  "source",
  "folderPath",
  "albumId",
  "photoId",
  "order",
  "selection",
  "rating-min",
  "rating-max",
  "kind",
  "available",
  "captured-from",
  "captured-before",
] as const;

const SELECTION_FILTERS: ReadonlyArray<NavigationSelectionFilter> = [
  "all",
  "unflagged",
  "picked",
  "rejected",
];

const ORIGINAL_KINDS: ReadonlyArray<NavigationOriginalKind> = ["raw", "jpeg"];

/// Mirrors the server's identifier validator: 36 to 64 bytes of lowercase
/// hexadecimal or `-`.
export const validPhotoId = (value: string): boolean =>
  value.length >= 36 && value.length <= 64 && /^[0-9a-f-]+$/.test(value);

/// Mirrors the server's Folder Location validator: relative, component-valid,
/// bounded. An empty value is the Library Folder root.
const MAXIMUM_FOLDER_LOCATION_BYTES = 1024;
const MAXIMUM_FOLDER_COMPONENTS = 32;
const validFolderLocation = (value: string): boolean => {
  if (value.length === 0) return true;
  if (
    new TextEncoder().encode(value).length > MAXIMUM_FOLDER_LOCATION_BYTES ||
    value.startsWith("/")
  )
    return false;
  const components = value.split("/");
  return (
    components.length <= MAXIMUM_FOLDER_COMPONENTS &&
    components.every(
      (component) =>
        component.length > 0 &&
        component !== "." &&
        component !== ".." &&
        !component.includes("\0"),
    )
  );
};

const validOrder = (
  value: string,
  source: NavigationSourceKind,
): value is NavigationViewOrder =>
  value === "capture-time-asc" ||
  value === "capture-time-desc" ||
  (value === "album-order" && source === "album");

/// Mirrors the server's Capture Time boundary validator: the exact
/// camera-local `YYYY-MM-DDTHH:MM:SS` shape, with a real calendar date and a
/// time of day. No timezone or offset is accepted.
const CAPTURE_TIME_PATTERN =
  /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})$/;
export const validCaptureTimeBound = (value: string): boolean => {
  const parts = CAPTURE_TIME_PATTERN.exec(value);
  if (!parts) return false;
  const [, year, month, day, hour, minute, second] = parts;
  const monthNumber = Number(month);
  const dayNumber = Number(day);
  const leap =
    Number(year) % 4 === 0 &&
    (Number(year) % 100 !== 0 || Number(year) % 400 === 0);
  const days =
    monthNumber === 2
      ? leap
        ? 29
        : 28
      : [4, 6, 9, 11].includes(monthNumber)
        ? 30
        : 31;
  return (
    Number(year) > 0 &&
    monthNumber >= 1 &&
    monthNumber <= 12 &&
    dayNumber >= 1 &&
    dayNumber <= days &&
    Number(hour) <= 23 &&
    Number(minute) <= 59 &&
    Number(second) <= 59
  );
};

/// One Rating bound: an integer from 0 through 5. Rating 0 means No Rating and
/// is a real bound, so `0` is never treated as a missing value.
const validRatingBound = (value: string): number | undefined => {
  if (!/^\d+$/.test(value)) return undefined;
  const rating = Number(value);
  return rating <= 5 ? rating : undefined;
};

/// The address reading: either a validated destination or an invalid address.
/// Unknown query keys never make an address invalid; they are dropped on
/// canonicalization.
export type AddressDecoding =
  | Readonly<{ kind: "destination"; destination: NavigationDestination }>
  | Readonly<{ kind: "invalid" }>;

/// Reads the optional fact conditions of one address. Every value is checked
/// against its own fact: an out-of-range or inverted Rating range, an unknown
/// `kind` or `available` value, and a malformed or reversed Capture Time bound
/// make the whole address invalid. An omitted parameter applies no condition
/// for that fact.
const readFilterConditions = (
  present: ReadonlyMap<string, string>,
): NavigationFilterConditions | undefined => {
  const ratingMinimum = present.get("rating-min");
  const ratingMaximum = present.get("rating-max");
  const minimum =
    ratingMinimum === undefined ? undefined : validRatingBound(ratingMinimum);
  const maximum =
    ratingMaximum === undefined ? undefined : validRatingBound(ratingMaximum);
  if (ratingMinimum !== undefined && minimum === undefined) return undefined;
  if (ratingMaximum !== undefined && maximum === undefined) return undefined;
  if (minimum !== undefined && maximum !== undefined && minimum > maximum)
    return undefined;
  const kind = present.get("kind");
  if (
    kind !== undefined &&
    !ORIGINAL_KINDS.includes(kind as NavigationOriginalKind)
  )
    return undefined;
  const available = present.get("available");
  if (available !== undefined && available !== "true" && available !== "false")
    return undefined;
  const capturedFrom = present.get("captured-from");
  if (capturedFrom !== undefined && !validCaptureTimeBound(capturedFrom))
    return undefined;
  const capturedBefore = present.get("captured-before");
  if (capturedBefore !== undefined && !validCaptureTimeBound(capturedBefore))
    return undefined;
  // The lower bound is inclusive and the upper bound is exclusive, so equal
  // bounds name an empty range and are reversed.
  if (
    capturedFrom !== undefined &&
    capturedBefore !== undefined &&
    capturedFrom >= capturedBefore
  )
    return undefined;
  return {
    ...(minimum === undefined ? {} : { ratingMinimum: minimum }),
    ...(maximum === undefined ? {} : { ratingMaximum: maximum }),
    ...(kind === undefined ? {} : { kind: kind as NavigationOriginalKind }),
    ...(available === undefined ? {} : { available: available === "true" }),
    ...(capturedFrom === undefined ? {} : { capturedFrom }),
    ...(capturedBefore === undefined ? {} : { capturedBefore }),
  };
};

/// Decodes one query string with URL and URLSearchParams, so every value is
/// decoded exactly once. A duplicate recognized parameter, an empty value for
/// an optional parameter, a source-incompatible option, or a value the API
/// validators reject makes the whole address invalid.
export function decodeAddress(search: string): AddressDecoding {
  const params = new URLSearchParams(search);
  const present = new Map<string, string>();
  for (const key of PARAMETER_ORDER) {
    const found = params.getAll(key);
    // A repeated recognized parameter is ambiguous, so it is invalid rather
    // than first-wins.
    if (found.length > 1) return { kind: "invalid" };
    if (found.length === 1) present.set(key, found[0]!);
  }
  const source = present.get("source");
  if (source !== undefined && source.length === 0) return { kind: "invalid" };
  const kind: NavigationSourceKind | "invalid" =
    source === undefined
      ? "library"
      : source === "library" || source === "folder" || source === "album"
        ? source
        : "invalid";
  if (kind === "invalid") return { kind: "invalid" };
  const folderPath = present.get("folderPath");
  const albumId = present.get("albumId");
  if (folderPath !== undefined && albumId !== undefined)
    return { kind: "invalid" };
  if (kind === "folder") {
    // The Library Folder is an empty Folder Location, so the parameter is
    // required even when its value is empty.
    if (folderPath === undefined || !validFolderLocation(folderPath))
      return { kind: "invalid" };
  } else if (folderPath !== undefined) return { kind: "invalid" };
  if (kind === "album") {
    if (albumId === undefined || !validPhotoId(albumId))
      return { kind: "invalid" };
  } else if (albumId !== undefined) return { kind: "invalid" };
  const photoId = present.get("photoId");
  if (photoId !== undefined && !validPhotoId(photoId))
    return { kind: "invalid" };
  const order = present.get("order");
  if (order !== undefined && !validOrder(order, kind))
    return { kind: "invalid" };
  const selection = present.get("selection");
  if (
    selection !== undefined &&
    !SELECTION_FILTERS.includes(selection as NavigationSelectionFilter)
  )
    return { kind: "invalid" };
  const filter = (selection as NavigationSelectionFilter | undefined) ?? "all";
  // Each optional filter parameter is additionally invalid when its value
  // fails its own fact validation, and an inverted Rating range or a reversed
  // Capture Time range is invalid as a pair.
  const conditions = readFilterConditions(present);
  if (!conditions) return { kind: "invalid" };
  const photo = photoId !== undefined ? { photoId } : {};
  const viewOrder = order !== undefined ? { order } : {};
  return {
    kind: "destination",
    destination:
      kind === "folder"
        ? {
            source: "folder",
            folderPath: folderPath!,
            ...photo,
            ...viewOrder,
            selection: filter,
            conditions,
          }
        : kind === "album"
          ? {
              source: "album",
              albumId: albumId!,
              ...photo,
              ...viewOrder,
              selection: filter,
              conditions,
            }
          : {
              source: "library",
              ...photo,
              ...viewOrder,
              selection: filter,
              conditions,
            },
  };
}

/// True when an address omits the order its source already defaults to.
const omitsDefaultOrder = (destination: NavigationDestination): boolean =>
  destination.order === undefined ||
  (destination.source === "album" && destination.order === "album-order");

/// Encodes one destination, omitting defaults and emitting the recognized
/// parameters in canonical order. Folder Locations keep their encoded
/// separators, so an encoded slash round-trips as a separator and an encoded
/// reserved character round-trips without a second decode.
export function encodeAddress(destination: NavigationDestination): string {
  const params = new URLSearchParams();
  if (destination.source !== "library")
    params.set("source", destination.source);
  if (destination.source === "folder")
    params.set("folderPath", destination.folderPath ?? "");
  if (destination.source === "album")
    params.set("albumId", destination.albumId ?? "");
  if (destination.photoId) params.set("photoId", destination.photoId);
  if (!omitsDefaultOrder(destination)) params.set("order", destination.order!);
  if (destination.selection !== "all")
    params.set("selection", destination.selection);
  // The fact conditions follow `selection` in the canonical order. A condition
  // the Photographer did not set is omitted, so an address never names a
  // restriction the open view does not apply.
  const conditions = destination.conditions;
  if (conditions.ratingMinimum !== undefined)
    params.set("rating-min", String(conditions.ratingMinimum));
  if (conditions.ratingMaximum !== undefined)
    params.set("rating-max", String(conditions.ratingMaximum));
  if (conditions.kind !== undefined) params.set("kind", conditions.kind);
  if (conditions.available !== undefined)
    params.set("available", String(conditions.available));
  if (conditions.capturedFrom !== undefined)
    params.set("captured-from", conditions.capturedFrom);
  if (conditions.capturedBefore !== undefined)
    params.set("captured-before", conditions.capturedBefore);
  return params.toString();
}

/// The canonical address of one destination on the application path. Unknown
/// keys are dropped, because the encoder emits only recognized parameters.
export function addressFor(
  destination: NavigationDestination,
  path = "/",
): string {
  const query = encodeAddress(destination);
  return query ? `${path}?${query}` : path;
}

/// The All Photos Grid with default options: the bare application address.
export const allPhotosDestination: NavigationDestination = Object.freeze({
  source: "library",
  selection: "all",
  conditions: {},
});

/// The Grid destination of one source: no Photo identity, so the address
/// names the Grid rather than one Photo.
export const gridDestination = (
  destination: NavigationDestination,
): NavigationDestination => {
  const grid: NavigationDestination = {
    source: destination.source,
    ...(destination.source === "folder"
      ? { folderPath: destination.folderPath }
      : {}),
    ...(destination.source === "album" ? { albumId: destination.albumId } : {}),
    ...(destination.order ? { order: destination.order } : {}),
    selection: destination.selection,
    conditions: destination.conditions,
  };
  return grid;
};

/// True when two condition sets name the same restrictions. An omitted
/// condition and an explicitly default one are the same view, so a traversal
/// reuses the live Snapshot instead of reopening the source.
export const sameConditions = (
  left: NavigationFilterConditions,
  right: NavigationFilterConditions,
): boolean =>
  sameBound(left.ratingMinimum, right.ratingMinimum) &&
  sameBound(left.ratingMaximum, right.ratingMaximum) &&
  sameBound(left.kind, right.kind) &&
  sameBound(left.available, right.available) &&
  sameBound(left.capturedFrom, right.capturedFrom) &&
  sameBound(left.capturedBefore, right.capturedBefore);

/// One optional condition value compared with an absent key and an explicit
/// `undefined` treated alike, because an omitted condition and an explicitly
/// default one are the same view.
const sameBound = <T>(left: T | undefined, right: T | undefined): boolean =>
  (left ?? undefined) === (right ?? undefined);

/// True when two destinations name the same source, order, and filter, so a
/// traversal can reuse the live Snapshot instead of reopening the source.
export const sameSourceView = (
  left: NavigationDestination,
  right: NavigationDestination,
): boolean =>
  left.source === right.source &&
  left.folderPath === right.folderPath &&
  left.albumId === right.albumId &&
  left.order === right.order &&
  left.selection === right.selection &&
  sameConditions(left.conditions, right.conditions);
