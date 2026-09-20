import type { PhotoFetch } from "./photo.js";

/// One unavailable Photo listed by the bounded recovery review entry. It
/// remembers the folder, filename, format, and retained decisions so the
/// Photographer can judge a mapping without the file.
export type UnavailableOriginal = Readonly<{
  originalId: string;
  photoId: string;
  location: string;
  kind: "raw" | "jpeg";
  rating: number;
  selectionState: "undecided" | "selected" | "rejected";
  fingerprintEnrolled: boolean;
  albumCount: number;
}>;

export type RetireCandidate = Readonly<{
  photoId: string;
  originalId: string;
  location: string;
}>;

export type RecoveryProposal = Readonly<{
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
  retire?: RetireCandidate;
}>;

export type UnavailableOriginalsResult =
  | Readonly<{ kind: "ok"; unavailable: ReadonlyArray<UnavailableOriginal> }>
  | Readonly<{ kind: "rejected"; status: number }>
  | Readonly<{ kind: "malformed" }>;

export type ProposeResult =
  | Readonly<{ kind: "ok"; proposals: ReadonlyArray<RecoveryProposal> }>
  | Readonly<{ kind: "rejected"; status: number }>
  | Readonly<{ kind: "malformed" }>;

export type RecoveryApplyItem = Readonly<{
  originalId: string;
  newLocation: string;
  retireDestination: boolean;
}>;

export type ApplyRelocationsResult =
  | Readonly<{
      kind: "applied";
      relocatedPhotos: number;
      unavailablePhotos: number;
    }>
  | Readonly<{
      kind: "rejected";
      status: number;
      message?: string;
      rejections: ReadonlyArray<
        Readonly<{ originalId: string; reason: string }>
      >;
    }>
  | Readonly<{ kind: "malformed" }>;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

const optional = (
  value: unknown,
  predicate: (item: unknown) => boolean,
): boolean => value === undefined || predicate(value);

const validKind = (value: unknown): value is "raw" | "jpeg" =>
  value === "raw" || value === "jpeg";

const validSelection = (
  value: unknown,
): value is "undecided" | "selected" | "rejected" =>
  value === "undecided" || value === "selected" || value === "rejected";

const validUnavailable = (value: unknown): value is UnavailableOriginal =>
  isRecord(value) &&
  typeof value.originalId === "string" &&
  value.originalId.length > 0 &&
  typeof value.photoId === "string" &&
  value.photoId.length > 0 &&
  typeof value.location === "string" &&
  value.location.length > 0 &&
  validKind(value.kind) &&
  Number.isInteger(value.rating) &&
  Number(value.rating) >= 0 &&
  Number(value.rating) <= 5 &&
  validSelection(value.selectionState) &&
  typeof value.fingerprintEnrolled === "boolean" &&
  Number.isInteger(value.albumCount) &&
  Number(value.albumCount) >= 0;

const validRetire = (value: unknown): value is RetireCandidate =>
  isRecord(value) &&
  typeof value.photoId === "string" &&
  value.photoId.length > 0 &&
  typeof value.originalId === "string" &&
  typeof value.location === "string";

const validOutcome = (value: unknown): value is RecoveryProposal["outcome"] =>
  value === "matched" ||
  value === "content-mismatch" ||
  value === "missing" ||
  value === "kind-mismatch" ||
  value === "unreadable" ||
  value === "occupied" ||
  value === "colliding";

const validProposal = (value: unknown): value is RecoveryProposal =>
  isRecord(value) &&
  typeof value.originalId === "string" &&
  value.originalId.length > 0 &&
  typeof value.photoId === "string" &&
  typeof value.fromLocation === "string" &&
  typeof value.toLocation === "string" &&
  value.toLocation.length > 0 &&
  validKind(value.kind) &&
  validOutcome(value.outcome) &&
  typeof value.verified === "boolean" &&
  optional(value.retire, validRetire);

/// Lists every unavailable Original with its remembered facts. The entry is
/// bounded by what the Library remembers; nothing is read from the files.
export async function fetchUnavailableOriginals(
  fetcher: PhotoFetch,
  signal: AbortSignal,
): Promise<UnavailableOriginalsResult> {
  let response: Response;
  try {
    response = await fetcher("/api/recovery/unavailable", { signal });
  } catch {
    return { kind: "rejected", status: 0 };
  }
  if (!response.ok) return { kind: "rejected", status: response.status };
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    return { kind: "malformed" };
  }
  if (
    !isRecord(body) ||
    !Array.isArray(body.unavailable) ||
    !body.unavailable.every(validUnavailable)
  )
    return { kind: "malformed" };
  return { kind: "ok", unavailable: body.unavailable };
}

/// Proposes one folder-prefix batch of relocations. Proposals are
/// inspectable and never write.
export async function proposeRelocations(
  fetcher: PhotoFetch,
  oldPrefix: string,
  newPrefix: string,
  signal: AbortSignal,
): Promise<ProposeResult> {
  return propose(
    fetcher,
    { oldPrefix, newPrefix },
    (body) =>
      isRecord(body) &&
      Array.isArray(body.proposals) &&
      body.proposals.every(validProposal),
    (body) => body.proposals as ReadonlyArray<RecoveryProposal>,
    signal,
  );
}

/// Proposes one mapping for a single unavailable Original, for renamed or
/// split files a folder-prefix batch cannot express.
export async function proposeSingleRelocation(
  fetcher: PhotoFetch,
  originalId: string,
  newLocation: string,
  signal: AbortSignal,
): Promise<ProposeResult> {
  return propose(
    fetcher,
    { originalId, newLocation },
    validProposal,
    (body) => [body as RecoveryProposal],
    signal,
  );
}

async function propose(
  fetcher: PhotoFetch,
  requestBody: object,
  validate: (body: unknown) => boolean,
  extract: (body: Record<string, unknown>) => ReadonlyArray<RecoveryProposal>,
  signal: AbortSignal,
): Promise<ProposeResult> {
  let response: Response;
  try {
    response = await fetcher("/api/recovery/propose", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(requestBody),
      signal,
    });
  } catch {
    return { kind: "rejected", status: 0 };
  }
  if (!response.ok) return { kind: "rejected", status: response.status };
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    return { kind: "malformed" };
  }
  if (!isRecord(body) || !validate(body)) return { kind: "malformed" };
  return { kind: "ok", proposals: extract(body) };
}

/// Commits one confirmed manual relocation batch. The whole batch commits
/// atomically or is refused with per-mapping reasons.
export async function applyRelocations(
  fetcher: PhotoFetch,
  relocations: ReadonlyArray<RecoveryApplyItem>,
  signal: AbortSignal,
): Promise<ApplyRelocationsResult> {
  let response: Response;
  try {
    response = await fetcher("/api/recovery/apply", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ relocations }),
      signal,
    });
  } catch {
    return { kind: "rejected", status: 0, rejections: [] };
  }
  let body: unknown = null;
  try {
    body = await response.json();
  } catch {
    if (response.ok) return { kind: "malformed" };
  }
  if (response.ok) {
    if (
      !isRecord(body) ||
      !Number.isInteger(body.relocatedPhotos) ||
      !Number.isInteger(body.unavailablePhotos)
    )
      return { kind: "malformed" };
    const relocatedPhotos = body.relocatedPhotos as number;
    const unavailablePhotos = body.unavailablePhotos as number;
    return { kind: "applied", relocatedPhotos, unavailablePhotos };
  }
  if (response.status !== 409) {
    return { kind: "rejected", status: response.status, rejections: [] };
  }
  if (
    !isRecord(body) ||
    typeof body.message !== "string" ||
    !Array.isArray(body.rejections) ||
    !body.rejections.every(
      (item) =>
        isRecord(item) &&
        typeof item.originalId === "string" &&
        typeof item.reason === "string",
    )
  )
    return { kind: "malformed" };
  return {
    kind: "rejected",
    status: 409,
    message: body.message,
    rejections: body.rejections,
  };
}
