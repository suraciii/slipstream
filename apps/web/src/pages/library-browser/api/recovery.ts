import { isRecord, validCount } from "./guards.js";
import type { PhotoFetch } from "./photo.js";

/// One item of a bounded recovery review: the remembered facts of an
/// unavailable Original the review opened on. The state names what the
/// Library holds now, so an entry the review still lists can already be
/// recovered, in the Trash, or gone; only `unavailable` entries still wait
/// for a mapping.
export type RecoveryItemState =
  | "unavailable"
  | "available"
  | "removed"
  | "missing";

export type RecoveryItem = Readonly<{
  state: RecoveryItemState;
  originalId: string;
  photoId: string;
  location: string;
  kind: "raw" | "jpeg";
  rating: number;
  selectionState: "unflagged" | "picked" | "rejected";
  fingerprintEnrolled: boolean;
  albumCount: number;
  webUrl: string;
}>;

/// The explicit destination occupant a mapping may replace. Retire and bind
/// are separate choices, so the candidate names the exact Photo that would
/// leave, never whatever later occupies the same path.
export type RecoveryRetireCandidate = Readonly<{
  photoId: string;
  originalId: string;
  location: string;
}>;

/// A blocked mapping names the reason it cannot be applied. `null` means the
/// mapping can be applied once its required confirmations are given.
export type RecoveryBlockedReason =
  | "colliding"
  | "content-mismatch"
  | "destination-in-use"
  | "destination-removed"
  | "kind-mismatch"
  | "missing"
  | "unreadable";

/// One reviewed mapping of an unavailable Original to a destination. The
/// mappingId is the reviewed identity the apply step must repeat.
export type RecoveryMapping = Readonly<{
  mappingId: string;
  originalId: string;
  photoId: string;
  fromLocation: string;
  toLocation: string;
  kind: "raw" | "jpeg";
  outcome:
    | "matched"
    | "content-mismatch"
    | "missing"
    | "kind-mismatch"
    | "unreadable"
    | "occupied"
    | "colliding";
  verified: boolean;
  blockedReason: RecoveryBlockedReason | null;
  retire: RecoveryRetireCandidate | null;
}>;

/// One page of a bounded review. `nextCursor` continues the same review;
/// `expiresAt` is non-null exactly while a next page exists.
export type RecoveryPage<Item> = Readonly<{
  items: ReadonlyArray<Item>;
  total: number;
  nextCursor: string | null;
  evaluatedAt: string;
  expiresAt: string | null;
}>;

export type RecoveryListResult<Item> =
  | Readonly<{ kind: "ok"; page: RecoveryPage<Item> }>
  | Readonly<{ kind: "rejected"; status: number; message?: string }>
  | Readonly<{ kind: "malformed" }>;

/// One mapping of the reviewed set the apply step commits. The mappingId
/// repeats the reviewed correspondence; the confirmations are sent only for
/// the choices the Photographer made explicitly.
export type RecoveryApplyMapping = Readonly<{
  originalId: string;
  newLocation: string;
  mappingId: string;
  confirmUnverifiedContent?: true;
  retirePhotoId?: string;
}>;

export type AppliedRecoveryMapping = Readonly<{
  originalId: string;
  photoId: string;
  fromLocation: string;
  toLocation: string;
  webUrl: string;
  retired: RecoveryRetireCandidate | null;
}>;

export type RecoveryApplyResult =
  | Readonly<{
      kind: "applied";
      appliedMappings: number;
      refusedMappings: number;
      unavailablePhotos: number;
      mappings: ReadonlyArray<AppliedRecoveryMapping>;
    }>
  | Readonly<{
      kind: "refused";
      status: 409;
      message: string;
      rejections: ReadonlyArray<
        Readonly<{ originalId: string; reason: string }>
      >;
      appliedMappings: number;
      refusedMappings: number;
    }>
  | Readonly<{
      kind: "unknown";
      mappings: ReadonlyArray<RecoveryApplyMapping>;
    }>
  | Readonly<{ kind: "rejected"; status: number; message?: string }>;

/// One bounded page of a review; larger reviews load continuation pages.
export const RECOVERY_PAGE_LIMIT = 25;

const validKind = (value: unknown): value is "raw" | "jpeg" =>
  value === "raw" || value === "jpeg";

const validSelection = (
  value: unknown,
): value is "unflagged" | "picked" | "rejected" =>
  value === "unflagged" || value === "picked" || value === "rejected";

const validState = (value: unknown): value is RecoveryItemState =>
  value === "unavailable" ||
  value === "available" ||
  value === "removed" ||
  value === "missing";

const validOutcome = (value: unknown): value is RecoveryMapping["outcome"] =>
  value === "matched" ||
  value === "content-mismatch" ||
  value === "missing" ||
  value === "kind-mismatch" ||
  value === "unreadable" ||
  value === "occupied" ||
  value === "colliding";

const validBlockedReason = (value: unknown): value is RecoveryBlockedReason =>
  value === "colliding" ||
  value === "content-mismatch" ||
  value === "destination-in-use" ||
  value === "destination-removed" ||
  value === "kind-mismatch" ||
  value === "missing" ||
  value === "unreadable";

const validRating = (value: unknown): boolean =>
  Number.isInteger(value) && Number(value) >= 0 && Number(value) <= 5;

const validItem = (value: unknown): value is RecoveryItem =>
  isRecord(value) &&
  validState(value.state) &&
  typeof value.originalId === "string" &&
  value.originalId.length > 0 &&
  typeof value.photoId === "string" &&
  value.photoId.length > 0 &&
  typeof value.location === "string" &&
  // A record the Library no longer holds has only remembered facts, so its
  // remembered location may be empty; an entry still waiting names one.
  (value.state === "missing" || value.location.length > 0) &&
  validKind(value.kind) &&
  validRating(value.rating) &&
  validSelection(value.selectionState) &&
  typeof value.fingerprintEnrolled === "boolean" &&
  validCount(value.albumCount) &&
  typeof value.webUrl === "string";

const validRetire = (value: unknown): value is RecoveryRetireCandidate =>
  isRecord(value) &&
  typeof value.photoId === "string" &&
  value.photoId.length > 0 &&
  typeof value.originalId === "string" &&
  value.originalId.length > 0 &&
  typeof value.location === "string";

const validMapping = (value: unknown): value is RecoveryMapping =>
  isRecord(value) &&
  typeof value.mappingId === "string" &&
  value.mappingId.length > 0 &&
  typeof value.originalId === "string" &&
  value.originalId.length > 0 &&
  typeof value.photoId === "string" &&
  value.photoId.length > 0 &&
  typeof value.fromLocation === "string" &&
  typeof value.toLocation === "string" &&
  value.toLocation.length > 0 &&
  validKind(value.kind) &&
  validOutcome(value.outcome) &&
  typeof value.verified === "boolean" &&
  (value.blockedReason === null || validBlockedReason(value.blockedReason)) &&
  (value.retire === null || validRetire(value.retire));

const validCursor = (value: unknown): value is string | null =>
  value === null || (typeof value === "string" && value.length > 0);

/// The shared list envelope. The server guarantees `expiresAt` is non-null
/// exactly while `nextCursor` is, so a page that breaks that promise is
/// malformed rather than silently truncated.
const validPage = <Item>(
  value: unknown,
  validEntry: (item: unknown) => item is Item,
): value is RecoveryPage<Item> =>
  isRecord(value) &&
  Array.isArray(value.items) &&
  value.items.every(validEntry) &&
  validCount(value.total) &&
  validCursor(value.nextCursor) &&
  typeof value.evaluatedAt === "string" &&
  value.evaluatedAt.length > 0 &&
  (value.expiresAt === null || typeof value.expiresAt === "string") &&
  (value.nextCursor === null) === (value.expiresAt === null);

const responseMessageBody = (body: unknown): string | undefined => {
  if (!isRecord(body)) return undefined;
  if (typeof body.error === "string" && body.error.length > 0)
    return body.error;
  if (typeof body.message === "string" && body.message.length > 0)
    return body.message;
  return undefined;
};

const responseMessage = async (
  response: Response,
): Promise<string | undefined> => {
  try {
    return responseMessageBody(await response.json());
  } catch {
    /* a refusal without a readable body keeps its status alone */
    return undefined;
  }
};

const requestListPage = async <Item>(
  fetcher: PhotoFetch,
  input: string,
  init: RequestInit,
  validEntry: (item: unknown) => item is Item,
  signal: AbortSignal,
): Promise<RecoveryListResult<Item>> => {
  let response: Response;
  try {
    response = await fetcher(input, { ...init, signal });
  } catch {
    return { kind: "rejected", status: 0 };
  }
  if (!response.ok) {
    const message = await responseMessage(response);
    return message === undefined
      ? { kind: "rejected", status: response.status }
      : { kind: "rejected", status: response.status, message };
  }
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    return { kind: "malformed" };
  }
  if (!validPage(body, validEntry)) return { kind: "malformed" };
  return { kind: "ok", page: body };
};

const recoveryBody = (body: object): RequestInit => ({
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(body),
});

const validLimit = (limit: number | undefined): number | undefined =>
  limit !== undefined && Number.isInteger(limit) && limit >= 1 && limit <= 60
    ? limit
    : undefined;

/// Opens one bounded review of the active unavailable Photos and returns its
/// first page. The review is frozen at this evaluation; continuation pages
/// repeat it by cursor.
export async function openUnavailableReview(
  fetcher: PhotoFetch,
  options: Readonly<{ limit?: number }> = {},
  signal: AbortSignal,
): Promise<RecoveryListResult<RecoveryItem>> {
  const limit = validLimit(options.limit);
  return requestListPage(
    fetcher,
    "/api/recovery/unavailable",
    recoveryBody(limit === undefined ? {} : { limit }),
    validItem,
    signal,
  );
}

/// Loads the next page of the same unavailable review. An expired or invalid
/// cursor is rejected with status 409 and the server's reason.
export async function continueUnavailableReview(
  fetcher: PhotoFetch,
  cursor: string,
  signal: AbortSignal,
): Promise<RecoveryListResult<RecoveryItem>> {
  return requestListPage(
    fetcher,
    `/api/recovery/unavailable/${encodeURIComponent(cursor)}`,
    { method: "GET" },
    validItem,
    signal,
  );
}

/// Proposes one folder-prefix batch of mappings. Proposals are inspectable
/// and never write. A scope larger than one review admits is rejected with
/// status 413.
export async function proposeRelocationBatch(
  fetcher: PhotoFetch,
  oldPrefix: string,
  newPrefix: string,
  options: Readonly<{ limit?: number }> = {},
  signal: AbortSignal,
): Promise<RecoveryListResult<RecoveryMapping>> {
  const limit = validLimit(options.limit);
  return requestListPage(
    fetcher,
    "/api/recovery/propose",
    recoveryBody(
      limit === undefined
        ? { oldPrefix, newPrefix }
        : { oldPrefix, newPrefix, limit },
    ),
    validMapping,
    signal,
  );
}

/// Proposes one mapping for a single unavailable Original, for renamed or
/// split files a folder-prefix batch cannot express. The answer is one page
/// with a single mapping and no continuation.
export async function proposeSingleRelocation(
  fetcher: PhotoFetch,
  originalId: string,
  newLocation: string,
  signal: AbortSignal,
): Promise<RecoveryListResult<RecoveryMapping>> {
  return requestListPage(
    fetcher,
    "/api/recovery/propose",
    recoveryBody({ originalId, newLocation }),
    validMapping,
    signal,
  );
}

/// Loads the next page of the same prefix proposal review. An expired or
/// invalid cursor is rejected with status 409 and the server's reason.
export async function continueRelocationProposals(
  fetcher: PhotoFetch,
  cursor: string,
  signal: AbortSignal,
): Promise<RecoveryListResult<RecoveryMapping>> {
  return requestListPage(
    fetcher,
    `/api/recovery/proposals/${encodeURIComponent(cursor)}`,
    { method: "GET" },
    validMapping,
    signal,
  );
}

const validAppliedMapping = (value: unknown): value is AppliedRecoveryMapping =>
  isRecord(value) &&
  typeof value.originalId === "string" &&
  value.originalId.length > 0 &&
  typeof value.photoId === "string" &&
  value.photoId.length > 0 &&
  typeof value.fromLocation === "string" &&
  typeof value.toLocation === "string" &&
  value.toLocation.length > 0 &&
  typeof value.webUrl === "string" &&
  (value.retired === null || validRetire(value.retired));

/// Commits the reviewed mappings. The whole batch commits atomically or is
/// refused with per-mapping reasons and nothing applied.
export async function applyRecoveryMappings(
  fetcher: PhotoFetch,
  mappings: ReadonlyArray<RecoveryApplyMapping>,
  signal: AbortSignal,
): Promise<RecoveryApplyResult> {
  const unknownResult: RecoveryApplyResult = { kind: "unknown", mappings };
  let response: Response;
  try {
    response = await fetcher("/api/recovery/apply", {
      ...recoveryBody({ mappings }),
      signal,
    });
  } catch {
    return unknownResult;
  }
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    return unknownResult;
  }
  if (response.ok) {
    if (
      !isRecord(body) ||
      !validCount(body.appliedMappings) ||
      !validCount(body.refusedMappings) ||
      !validCount(body.unavailablePhotos) ||
      !Array.isArray(body.mappings) ||
      body.appliedMappings !== mappings.length ||
      body.refusedMappings !== 0 ||
      body.mappings.length !== mappings.length ||
      !body.mappings.every(validAppliedMapping) ||
      !body.mappings.every(
        (item, index) =>
          isRecord(item) &&
          item.originalId === mappings[index]?.originalId &&
          item.toLocation === mappings[index]?.newLocation,
      )
    )
      return unknownResult;
    return {
      kind: "applied",
      appliedMappings: body.appliedMappings,
      refusedMappings: body.refusedMappings,
      unavailablePhotos: body.unavailablePhotos,
      mappings: body.mappings,
    };
  }
  if (response.status !== 409) return unknownResult;
  if (
    !isRecord(body) ||
    typeof body.message !== "string" ||
    !validCount(body.appliedMappings) ||
    !validCount(body.refusedMappings) ||
    body.appliedMappings !== 0 ||
    body.refusedMappings !== mappings.length ||
    !Array.isArray(body.rejections) ||
    body.rejections.length !== mappings.length ||
    !body.rejections.every(
      (item, index) =>
        isRecord(item) &&
        typeof item.originalId === "string" &&
        typeof item.reason === "string" &&
        item.originalId === mappings[index]?.originalId,
    )
  )
    return unknownResult;
  return {
    kind: "refused",
    status: 409,
    message: body.message,
    rejections: body.rejections,
    appliedMappings: body.appliedMappings,
    refusedMappings: body.refusedMappings,
  };
}
