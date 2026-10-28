//! The Edit view's two reversible operations over the tool-owned Edit State,
//! kept apart from one another because they have different owners.
//!
//! Session Undo and Redo are browser-local over the drafted Edit State. Each
//! one is an ordinary edit action: the owner submits it as its own guarded
//! update with a new request identity, so the tool's confirmed state records
//! the reversal exactly as it records a slider change.
//!
//! Retained revision restore is a separate tool-owned operation against the
//! tool's saved history. It is never the inverse of a session edit and it may
//! select a revision the current draft never visited.
//!
//! See [Autosave and Reversible Editing](../../../../../../specs/photo-development/spec.md#autosave-and-reversible-editing).

import type { EditChange } from "./workspace-edit-owner.js";

/** The bounded number of completed edit actions this session remembers. */
export const MAXIMUM_SESSION_HISTORY = 64;

/** One completed edit action of this browser session. */
export type SessionEditEntry = Readonly<{
  /** The changes this action submitted, oldest first. */
  changes: readonly EditChange[];
  /**
   * The change list that returns the confirmed state to what this action
   * replaced, or undefined when the tool's change vocabulary cannot express
   * that inverse truthfully. A session Undo is then unavailable for this
   * action rather than approximated by an unrelated reset.
   */
  inverses: readonly EditChange[] | undefined;
  /** The revision confirmed before this action was applied. */
  baseRevision: string;
  /** The revision this action produced. */
  revision: string;
}>;

/** The chronological session history of one selected edit. */
export type SessionEditHistory = {
  /** Completed actions, oldest first. */
  past: SessionEditEntry[];
  /** Reversed actions available to Redo, newest first. */
  future: SessionEditEntry[];
};

export const createSessionEditHistory = (): SessionEditHistory => ({
  past: [],
  future: [],
});

/** Records one completed action and clears everything reversed after it. */
export function recordSessionEdit(
  history: SessionEditHistory,
  entry: SessionEditEntry,
): void {
  history.past.push(entry);
  if (history.past.length > MAXIMUM_SESSION_HISTORY) history.past.shift();
  history.future = [];
}

export const undoableEntry = (
  history: SessionEditHistory,
): SessionEditEntry | undefined =>
  history.past.length ? history.past.at(-1) : undefined;

export const redoableEntry = (
  history: SessionEditHistory,
): SessionEditEntry | undefined =>
  history.future.length ? history.future.at(-1) : undefined;

/**
 * Moves the newest completed action to the reversed side for Redo. The
 * caller submits the reversal before calling this, so an unsettled reversal
 * never loses the entry it would restore.
 */
export function retireForUndo(history: SessionEditHistory): SessionEditEntry {
  const entry = history.past.pop()!;
  history.future.push(entry);
  if (history.future.length > MAXIMUM_SESSION_HISTORY) history.future.shift();
  return entry;
}

export function retireForRedo(history: SessionEditHistory): SessionEditEntry {
  const entry = history.future.pop()!;
  history.past.push(entry);
  if (history.past.length > MAXIMUM_SESSION_HISTORY) history.past.shift();
  return entry;
}
