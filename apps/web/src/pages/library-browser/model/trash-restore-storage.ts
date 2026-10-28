/// A Restore intent is retained until its receipt is known. Retrying the same
/// operation and exclusions cannot resolve a second, wider Trash selection.
export type PendingTrashRestore = Readonly<{
  operationId: string;
  excludePhotoIds: ReadonlyArray<string>;
}>;

const KEY = "slipstream:trash-restore-operation";

export function readPendingTrashRestore(): PendingTrashRestore | undefined {
  try {
    const raw = window.localStorage.getItem(KEY);
    if (raw === null) return undefined;
    const value: unknown = JSON.parse(raw);
    if (typeof value !== "object" || value === null || Array.isArray(value))
      return undefined;
    const entry = value as Record<string, unknown>;
    if (
      typeof entry.operationId !== "string" ||
      entry.operationId.length === 0 ||
      !Array.isArray(entry.excludePhotoIds) ||
      !entry.excludePhotoIds.every(
        (id: unknown) => typeof id === "string" && id.length > 0,
      ) ||
      new Set(entry.excludePhotoIds).size !== entry.excludePhotoIds.length
    )
      return undefined;
    return {
      operationId: entry.operationId,
      excludePhotoIds: entry.excludePhotoIds,
    };
  } catch {
    return undefined;
  }
}

export function storePendingTrashRestore(
  intent: PendingTrashRestore | undefined,
): void {
  try {
    if (intent) window.localStorage.setItem(KEY, JSON.stringify(intent));
    else window.localStorage.removeItem(KEY);
  } catch {
    // Recovery remains available in memory while this page stays open.
  }
}
