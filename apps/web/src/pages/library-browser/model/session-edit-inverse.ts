//! The inverse of one completed edit action, expressed in the tool's own
//! change vocabulary.
//!
//! Session Undo and Redo are ordinary edit actions over the tool-owned Edit
//! State. A reversal must therefore name real changes the tool admits, not a
//! browser-local relabeling of the saved state. Each inverse is computed from
//! the confirmed state the action replaced and the confirmed state it
//! produced, so the tool records the reversal exactly as it records a slider
//! change.
//!
//! A change the tool's vocabulary cannot invert truthfully yields no inverse,
//! and the owner reports that Undo is unavailable rather than partially
//! reversing the action or substituting an unrelated reset.
//!
//! See [Autosave and Reversible Editing](../../../../../../specs/photo-development/spec.md#autosave-and-reversible-editing).

import { isRecord } from "../api/guards.js";
import type { EditChange } from "./workspace-edit-model.js";

type Change = EditChange;
type Item = Record<string, unknown>;

const equal = (left: unknown, right: unknown): boolean =>
  JSON.stringify(left) === JSON.stringify(right);

const remaining = (item: Item, omitted: string[]): Item =>
  Object.fromEntries(
    Object.entries(item).filter(([key]) => !omitted.includes(key)),
  );

// A bounded edit-show window cannot establish what changed outside the page.
const itemsOf = (edit: Item, key: string): Item[] | undefined => {
  const window = edit[key];
  if (window === undefined) return [];
  if (!isRecord(window) || !Array.isArray(window.items)) return undefined;
  if (
    window.nextCursor != null ||
    (typeof window.total === "number" && window.total !== window.items.length)
  )
    return undefined;
  return window.items.every(isRecord) ? window.items : undefined;
};

const keyed = (items: Item[], key: string): Map<string, Item> | undefined => {
  const result = new Map<string, Item>();
  for (const item of items) {
    const id = item[key];
    if (typeof id !== "string" || !id || result.has(id)) return undefined;
    result.set(id, item);
  }
  return result;
};

/**
 * The changes that return one confirmed edit state to another. Only the
 * changes the tool's own vocabulary can express appear here.
 */
export function inverseChanges(
  before: unknown,
  after: unknown,
): Change[] | undefined {
  if (!isRecord(before) || !isRecord(after)) return undefined;
  if (
    (before.instances === undefined) !== (after.instances === undefined) ||
    (before.masks === undefined) !== (after.masks === undefined)
  )
    return undefined;
  const beforeInstances = itemsOf(before, "instances");
  const afterInstances = itemsOf(after, "instances");
  const beforeMasks = itemsOf(before, "masks");
  const afterMasks = itemsOf(after, "masks");
  if (
    !beforeInstances ||
    !afterInstances ||
    !beforeMasks ||
    !afterMasks ||
    !itemsOf(before, "actions") ||
    !itemsOf(after, "actions")
  )
    return undefined;
  const instances = keyed(afterInstances, "instanceId");
  const masks = keyed(afterMasks, "maskId");
  if (
    !instances ||
    !masks ||
    !keyed(beforeInstances, "instanceId") ||
    !keyed(beforeMasks, "maskId") ||
    beforeInstances.length !== afterInstances.length ||
    beforeMasks.length !== afterMasks.length
  )
    return undefined;
  if (
    !equal(
      beforeInstances.map((item) => item.instanceId),
      afterInstances.map((item) => item.instanceId),
    ) ||
    !equal(
      beforeMasks.map((item) => item.maskId),
      afterMasks.map((item) => item.maskId),
    )
  )
    return undefined;
  if (
    !equal(before.frames, after.frames) ||
    !equal(before.actions, after.actions) ||
    !equal(before.bundleId, after.bundleId) ||
    !equal(before.schemaVersion, after.schemaVersion)
  )
    return undefined;

  const inverses: Change[] = [];
  for (const previous of beforeInstances) {
    const id = previous.instanceId as string;
    const current = instances.get(id);
    if (!current) return undefined;
    const blendChanged = !equal(previous.blend, current.blend);
    if (
      !equal(
        remaining(previous, [
          "enabled",
          "parameters",
          "blend",
          ...(blendChanged ? ["maskIds"] : []),
        ]),
        remaining(current, [
          "enabled",
          "parameters",
          "blend",
          ...(blendChanged ? ["maskIds"] : []),
        ]),
      )
    )
      return undefined;
    if (previous.enabled !== current.enabled) {
      if (typeof previous.enabled !== "boolean") return undefined;
      inverses.push({
        type: "instance-enable",
        instanceId: id,
        enabled: previous.enabled,
      });
    }
    if (
      (!isRecord(previous.parameters) || !isRecord(current.parameters)) &&
      !equal(previous.parameters, current.parameters)
    )
      return undefined;
    const beforeParameters = isRecord(previous.parameters)
      ? previous.parameters
      : {};
    const afterParameters = isRecord(current.parameters)
      ? current.parameters
      : {};
    for (const key of new Set([
      ...Object.keys(beforeParameters),
      ...Object.keys(afterParameters),
    ])) {
      if (equal(beforeParameters[key], afterParameters[key])) continue;
      if (!(key in beforeParameters) || !(key in afterParameters))
        return undefined;
      inverses.push({
        type: "set",
        instanceId: id,
        parameterId: key,
        value: beforeParameters[key],
      });
    }
    if (blendChanged) {
      if (!isRecord(previous.blend) || !isRecord(current.blend))
        return undefined;
      // blend-set derives maskIds from the drawn root reference.
      const previousIds =
        typeof previous.blend.maskId === "string"
          ? [previous.blend.maskId]
          : [];
      if (!equal(previous.maskIds, previousIds)) return undefined;
      inverses.push({
        type: "blend-set",
        instanceId: id,
        value: previous.blend,
      });
    }
  }
  for (const previous of beforeMasks) {
    const id = previous.maskId as string;
    const current = masks.get(id);
    if (
      !current ||
      !equal(remaining(previous, ["value"]), remaining(current, ["value"]))
    )
      return undefined;
    if (!equal(previous.value, current.value)) {
      if (!isRecord(previous.value) || !isRecord(current.value))
        return undefined;
      inverses.push({ type: "mask-set", maskId: id, value: previous.value });
    }
  }
  if (!equal(before.output, after.output)) {
    if (
      !isRecord(before.output) ||
      !isRecord(after.output) ||
      !isRecord(before.output.settings) ||
      !isRecord(after.output.settings) ||
      !equal(
        remaining(before.output, ["settings"]),
        remaining(after.output, ["settings"]),
      )
    )
      return undefined;
    for (const key of new Set([
      ...Object.keys(before.output.settings),
      ...Object.keys(after.output.settings),
    ])) {
      if (equal(before.output.settings[key], after.output.settings[key]))
        continue;
      if (!(key in before.output.settings)) return undefined;
      inverses.push({
        type: "output-set",
        fieldId: key,
        value: before.output.settings[key],
      });
    }
  }
  return inverses;
}
