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
//! A change the tool's vocabulary cannot invert truthfully — a removed
//! operation, a mask creation, an applied Style — yields no inverse, and the
//! owner reports that Undo is unavailable rather than substituting an
//! unrelated reset.
//!
//! See [Autosave and Reversible Editing](../../../../../../specs/photo-development/spec.md#autosave-and-reversible-editing).

import { isRecord } from "../api/guards.js";
import type { EditChange } from "./workspace-edit-owner.js";

type Change = EditChange;
type Instance = Readonly<{
  instanceId: string;
  enabled?: unknown;
  order?: unknown;
  parameters?: unknown;
}>;

const instanceId = (value: unknown): string | undefined =>
  typeof value === "string" && value ? value : undefined;

const parameterValue = (instance: unknown, parameterId: string): unknown => {
  if (!isRecord(instance)) return undefined;
  const parameters = instance.parameters;
  if (!isRecord(parameters) || !(parameterId in parameters)) return undefined;
  return parameters[parameterId];
};

const instancesOf = (edit: unknown): Instance[] => {
  if (!isRecord(edit)) return [];
  const instances = edit.instances;
  if (!isRecord(instances) || !Array.isArray(instances.items)) return [];
  return instances.items.filter(isRecord) as Instance[];
};

/**
 * The changes that return one confirmed edit state to another. Only the
 * changes the tool's own vocabulary can express appear here.
 */
export function inverseChanges(
  before: unknown,
  after: unknown,
): Change[] | undefined {
  const beforeInstances = instancesOf(before);
  const afterInstances = instancesOf(after);
  if (!isRecord(before) || !isRecord(after)) return undefined;
  const ids = (instances: Instance[]) =>
    new Set(
      instances
        .map((instance) => instanceId(instance.instanceId))
        .filter((id): id is string => id !== undefined),
    );
  const beforeIds = ids(beforeInstances);
  const afterIds = ids(afterInstances);
  // A newly added operation cannot be removed truthfully: the tool generated
  // its identity, and no change in the tool's vocabulary addresses it. An
  // operation that disappeared cannot be recreated from the edit state alone.
  if (beforeIds.size !== afterIds.size) return undefined;
  for (const id of beforeIds) if (!afterIds.has(id)) return undefined;
  const inverses: Change[] = [];
  for (const beforeInstance of beforeInstances) {
    const id = instanceId(beforeInstance.instanceId);
    if (!id) return undefined;
    const afterInstance = afterInstances.find(
      (candidate) => instanceId(candidate.instanceId) === id,
    );
    if (!afterInstance) return undefined;
    if (beforeInstance.enabled !== afterInstance.enabled)
      inverses.push({
        type: "instance-enable",
        instanceId: id,
        enabled: Boolean(beforeInstance.enabled),
      });
    const beforeParameters = isRecord(beforeInstance.parameters)
      ? beforeInstance.parameters
      : {};
    const afterParameters = isRecord(afterInstance.parameters)
      ? afterInstance.parameters
      : {};
    for (const key of new Set([
      ...Object.keys(beforeParameters),
      ...Object.keys(afterParameters),
    ])) {
      const previous = parameterValue(beforeInstance, key);
      const current = parameterValue(afterInstance, key);
      if (JSON.stringify(previous) === JSON.stringify(current)) continue;
      if (previous === undefined) return undefined;
      inverses.push({
        type: "set",
        instanceId: id,
        parameterId: key,
        value: previous,
      });
    }
  }
  return inverses;
}
