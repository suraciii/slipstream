import { describe, expect, test } from "bun:test";
import {
  createSessionEditHistory,
  MAXIMUM_SESSION_HISTORY,
  recordSessionEdit,
  redoableEntry,
  retireForRedo,
  retireForUndo,
  undoableEntry,
  type SessionEditEntry,
} from "./session-edit-history.js";
import { inverseChanges } from "./session-edit-inverse.js";
import type { EditChange } from "./workspace-edit-owner.js";

const editState = (
  instances: ReadonlyArray<Readonly<Record<string, unknown>>>,
) => ({ instances: { items: instances } });

const set = (
  instanceId: string,
  parameterId: string,
  value: unknown,
): EditChange => ({ type: "set", instanceId, parameterId, value });

const entry = (
  changes: readonly EditChange[],
  inverses: readonly EditChange[] | undefined,
  revision: string,
): SessionEditEntry => ({
  changes,
  inverses,
  baseRevision: "r0",
  revision,
});

describe("session edit history", () => {
  test("records the newest action and clears everything reversed after it", () => {
    const history = createSessionEditHistory();
    recordSessionEdit(
      history,
      entry([set("i", "a", 1)], [set("i", "a", 0)], "r1"),
    );
    recordSessionEdit(
      history,
      entry([set("i", "b", 2)], [set("i", "b", 0)], "r2"),
    );
    expect(undoableEntry(history)?.revision).toBe("r2");
    retireForUndo(history);
    expect(undoableEntry(history)?.revision).toBe("r1");
    expect(redoableEntry(history)?.revision).toBe("r2");
    retireForRedo(history);
    expect(undoableEntry(history)?.revision).toBe("r2");
    expect(redoableEntry(history)).toBeUndefined();
  });

  test("an action with no truthful inverse reports Undo unavailable", () => {
    const history = createSessionEditHistory();
    recordSessionEdit(history, entry([{ type: "reset" }], undefined, "r1"));
    expect(undoableEntry(history)?.inverses).toBeUndefined();
    expect(undoableEntry(history)?.changes).toEqual([{ type: "reset" }]);
  });

  test("an empty session has no Undo or Redo", () => {
    const history = createSessionEditHistory();
    expect(undoableEntry(history)).toBeUndefined();
    expect(redoableEntry(history)).toBeUndefined();
  });

  test("retiring for Undo keeps the entry available to Redo", () => {
    const history = createSessionEditHistory();
    recordSessionEdit(
      history,
      entry([set("i", "a", 5)], [set("i", "a", 0)], "r1"),
    );
    const retired = retireForUndo(history);
    expect(retired.changes).toEqual([set("i", "a", 5)]);
    expect(history.past).toHaveLength(0);
    expect(redoableEntry(history)).toBe(retired);
  });

  test("a recorded action clears the Redo of everything reversed after it", () => {
    const history = createSessionEditHistory();
    recordSessionEdit(
      history,
      entry([set("i", "a", 1)], [set("i", "a", 0)], "r1"),
    );
    retireForUndo(history);
    expect(history.future).toHaveLength(1);
    recordSessionEdit(
      history,
      entry([set("i", "b", 2)], [set("i", "b", 0)], "r2"),
    );
    expect(history.future).toHaveLength(0);
    expect(redoableEntry(history)).toBeUndefined();
  });

  test("the session history stays bounded", () => {
    const history = createSessionEditHistory();
    for (let index = 0; index < MAXIMUM_SESSION_HISTORY + 10; index++)
      recordSessionEdit(
        history,
        entry([set("i", "a", index)], [], `r${index}`),
      );
    expect(history.past).toHaveLength(MAXIMUM_SESSION_HISTORY);
    expect(undoableEntry(history)?.revision).toBe(
      `r${MAXIMUM_SESSION_HISTORY + 9}`,
    );
  });
});

describe("session edit inverse", () => {
  test("a changed parameter inverts to its previous value", () => {
    const before = editState([
      { instanceId: "i", enabled: true, parameters: { ev: 0, temp: 5500 } },
    ]);
    const after = editState([
      { instanceId: "i", enabled: true, parameters: { ev: 0.5, temp: 5500 } },
    ]);
    expect(inverseChanges(before, after)).toEqual([set("i", "ev", 0)]);
  });

  test("a blend change restores the complete previous blend", () => {
    const before = editState([
      { instanceId: "i", blend: { opacity: 0.2 }, maskIds: [] },
    ]);
    const after = editState([
      { instanceId: "i", blend: { opacity: 0.8 }, maskIds: [] },
    ]);
    expect(inverseChanges(before, after)).toEqual([
      { type: "blend-set", instanceId: "i", value: { opacity: 0.2 } },
    ]);
  });

  test("a mask change restores the complete previous mask value", () => {
    const before = {
      ...editState([]),
      masks: { items: [{ maskId: "m", value: { x: 1 } }] },
    };
    const after = {
      ...editState([]),
      masks: { items: [{ maskId: "m", value: { x: 2 } }] },
    };
    expect(inverseChanges(before, after)).toEqual([
      { type: "mask-set", maskId: "m", value: { x: 1 } },
    ]);
  });

  test("an output change restores the previous output field", () => {
    const before = {
      ...editState([]),
      output: { settings: { format: "jpeg" } },
    };
    const after = { ...editState([]), output: { settings: { format: "png" } } };
    expect(inverseChanges(before, after)).toEqual([
      { type: "output-set", fieldId: "format", value: "jpeg" },
    ]);
  });

  test("a mixed action restores every change, or has no inverse", () => {
    const before = {
      ...editState([
        {
          instanceId: "i",
          parameters: { ev: 0 },
          blend: { opacity: 0.2 },
          maskIds: [],
        },
      ]),
      output: { settings: { format: "jpeg" } },
    };
    const after = {
      ...editState([
        {
          instanceId: "i",
          parameters: { ev: 1 },
          blend: { opacity: 0.8 },
          maskIds: [],
        },
      ]),
      output: { settings: { format: "png" } },
    };
    expect(inverseChanges(before, after)).toEqual([
      set("i", "ev", 0),
      { type: "blend-set", instanceId: "i", value: { opacity: 0.2 } },
      { type: "output-set", fieldId: "format", value: "jpeg" },
    ]);
  });

  test("an uninvertible change makes the entire mixed action unavailable", () => {
    const before = {
      ...editState([{ instanceId: "i", parameters: { ev: 0 } }]),
      masks: { items: [] },
    };
    const after = {
      ...editState([{ instanceId: "i", parameters: { ev: 1 } }]),
      masks: { items: [{ maskId: "m", value: {} }] },
    };
    expect(inverseChanges(before, after)).toBeUndefined();
  });

  test("a partial state window cannot produce a truthful inverse", () => {
    const before = {
      instances: {
        items: [{ instanceId: "i", parameters: { ev: 0 } }],
        nextCursor: "more",
      },
    };
    const after = {
      instances: {
        items: [{ instanceId: "i", parameters: { ev: 1 } }],
        nextCursor: "more",
      },
    };
    expect(inverseChanges(before, after)).toBeUndefined();
  });

  test("a disabled operation inverts to its previous enablement", () => {
    const before = editState([
      { instanceId: "i", enabled: true, parameters: {} },
    ]);
    const after = editState([
      { instanceId: "i", enabled: false, parameters: {} },
    ]);
    expect(inverseChanges(before, after)).toEqual([
      { type: "instance-enable", instanceId: "i", enabled: true },
    ]);
  });

  test("a parameter that appeared has no inverse to remove it", () => {
    const before = editState([
      { instanceId: "i", enabled: true, parameters: {} },
    ]);
    const after = editState([
      { instanceId: "i", enabled: true, parameters: { ev: 1 } },
    ]);
    expect(inverseChanges(before, after)).toBeUndefined();
  });

  test("an operation that disappeared has no inverse to recreate it", () => {
    const before = editState([
      { instanceId: "i", enabled: true, parameters: {} },
      { instanceId: "j", enabled: true, parameters: {} },
    ]);
    const after = editState([
      { instanceId: "i", enabled: true, parameters: {} },
    ]);
    expect(inverseChanges(before, after)).toBeUndefined();
  });

  test("an added operation has no inverse to remove it", () => {
    const before = editState([
      { instanceId: "i", enabled: true, parameters: {} },
    ]);
    const after = editState([
      { instanceId: "i", enabled: true, parameters: {} },
      { instanceId: "j", enabled: true, parameters: {} },
    ]);
    expect(inverseChanges(before, after)).toBeUndefined();
  });

  test("an unchanged state has no inverse changes", () => {
    const state = editState([
      { instanceId: "i", enabled: true, parameters: { ev: 0 } },
    ]);
    expect(inverseChanges(state, state)).toEqual([]);
  });

  test("multiple operations invert to their previous values", () => {
    const before = editState([
      { instanceId: "i", enabled: true, parameters: { ev: 0 } },
      { instanceId: "j", enabled: false, parameters: { b: 1 } },
    ]);
    const after = editState([
      { instanceId: "i", enabled: false, parameters: { ev: 1 } },
      { instanceId: "j", enabled: true, parameters: { b: 1 } },
    ]);
    expect(inverseChanges(before, after)).toEqual([
      { type: "instance-enable", instanceId: "i", enabled: true },
      set("i", "ev", 0),
      { type: "instance-enable", instanceId: "j", enabled: false },
    ]);
  });

  test("a non-object edit state reports no inverse", () => {
    expect(inverseChanges(null, editState([]))).toBeUndefined();
    expect(inverseChanges(editState([]), null)).toBeUndefined();
  });
});
