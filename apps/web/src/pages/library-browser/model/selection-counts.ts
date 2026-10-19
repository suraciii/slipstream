import type { SelectionCounts, SelectionState } from "../api/contracts.js";

/// One confirmed Selection State transition applied to the open source's
/// counts. The counts come from the server when the source opens, so a
/// confirmed decision moves them by one instead of re-deriving them from
/// loaded windows.
export const adjustedSelectionCounts = (
  counts: SelectionCounts,
  prior: SelectionState,
  next: SelectionState,
): SelectionCounts => {
  if (prior === next) return counts;
  const adjusted = { ...counts };
  // A loaded Photo can be newer than the source-wide counts after an
  // external write. Never expose a negative progress bucket while moving the
  // browser's belief to the observed fact; the next source refresh restores
  // the server's complete counts.
  adjusted[prior] = Math.max(0, adjusted[prior] - 1);
  adjusted[next] += 1;
  return Object.freeze(adjusted);
};
