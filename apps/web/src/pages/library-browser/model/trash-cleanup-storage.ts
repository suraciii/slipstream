import type { TrashCleanupTarget } from "../api/trash.js";

const PREFIX = "slipstream:trash-cleanup-attempt:";

export type PendingTrashCleanup = Readonly<{
  operationId: string;
  items: ReadonlyArray<TrashCleanupTarget>;
}>;

const validTarget = (value: unknown): value is TrashCleanupTarget => {
  if (!value || typeof value !== "object") return false;
  const target = value as Record<string, unknown>;
  return (
    typeof target.photoId === "string" &&
    target.photoId.length > 0 &&
    typeof target.removedAtMs === "number" &&
    Number.isSafeInteger(target.removedAtMs) &&
    target.removedAtMs >= 0 &&
    typeof target.originalId === "string" &&
    target.originalId.length > 0 &&
    typeof target.originalLocation === "string" &&
    target.originalLocation.length > 0
  );
};

const validPending = (value: unknown): value is PendingTrashCleanup => {
  if (!value || typeof value !== "object") return false;
  const pending = value as Record<string, unknown>;
  return (
    typeof pending.operationId === "string" &&
    pending.operationId.length > 0 &&
    Array.isArray(pending.items) &&
    pending.items.length > 0 &&
    pending.items.every(validTarget)
  );
};

/** Each attempt has its own key, so concurrent tabs never overwrite one another. */
export function readPendingTrashCleanups(): ReadonlyArray<PendingTrashCleanup> {
  try {
    const pending: PendingTrashCleanup[] = [];
    for (let index = 0; index < window.localStorage.length; index++) {
      const key = window.localStorage.key(index);
      if (!key?.startsWith(PREFIX)) continue;
      const raw = window.localStorage.getItem(key);
      if (!raw) continue;
      let parsed: unknown;
      try {
        parsed = JSON.parse(raw);
      } catch {
        continue;
      }
      if (validPending(parsed) && key === `${PREFIX}${parsed.operationId}`)
        pending.push(parsed);
    }
    return pending;
  } catch {
    return [];
  }
}

/** A cleanup cannot be sent unless its identity and exact intent survive a reload. */
export function storePendingTrashCleanup(
  operationId: string,
  items: ReadonlyArray<TrashCleanupTarget>,
): boolean {
  const pending: PendingTrashCleanup = { operationId, items };
  try {
    const key = `${PREFIX}${operationId}`;
    window.localStorage.setItem(key, JSON.stringify(pending));
    return window.localStorage.getItem(key) !== null;
  } catch {
    return false;
  }
}

export function clearPendingTrashCleanup(operationId: string): void {
  try {
    window.localStorage.removeItem(`${PREFIX}${operationId}`);
  } catch {
    // Retaining an already settled identity is safe; it can be checked again.
  }
}
