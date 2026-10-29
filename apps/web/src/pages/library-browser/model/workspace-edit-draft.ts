import type { EditChange } from "./workspace-edit-model.js";

export function coalesceChanges(
  changes: EditChange[],
  next: EditChange,
): EditChange[] {
  const key = (change: EditChange) =>
    change.type === "set" &&
    typeof change.instanceId === "string" &&
    typeof change.parameterId === "string"
      ? `set:${change.instanceId}:${change.parameterId}`
      : change.type === "output-set" && typeof change.fieldId === "string"
        ? `output:${change.fieldId}`
        : change.type === "blend-set" && typeof change.instanceId === "string"
          ? `blend:${change.instanceId}`
          : null;
  const nextKey = key(next);
  if (nextKey === null) return [...changes, next];
  let index = -1;
  for (let position = changes.length - 1; position >= 0; position--) {
    const change = changes[position];
    if (change && key(change) === nextKey) {
      index = position;
      break;
    }
  }
  if (index < 0) return [...changes, next];
  // Only adjacent assignments may coalesce across no dependency-changing action.
  if (changes.slice(index + 1).some((change) => key(change) === null))
    return [...changes, next];
  return changes.map((change, position) =>
    position === index ? next : change,
  );
}
