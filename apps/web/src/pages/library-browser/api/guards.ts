/**
 * Shared runtime guards for the wire boundary.
 *
 * Every API client validates the response bodies it consumes: the server's
 * answers are untrusted input, and a body the client cannot prove is a body it
 * must not turn into visible state. The guards live here so one canonical
 * implementation serves every client instead of being recreated per file.
 */

export const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

/// An object whose keys are exactly `keys`: an answer that omits, invents, or
/// duplicates a field is not the shape the contract names.
export const hasExactKeys = (
  value: Record<string, unknown>,
  keys: ReadonlyArray<string>,
): boolean =>
  Object.keys(value).length === keys.length &&
  keys.every((key) => Object.prototype.hasOwnProperty.call(value, key));

/// A non-negative integer count.
export const validCount = (value: unknown): value is number =>
  Number.isInteger(value) && Number(value) >= 0;
