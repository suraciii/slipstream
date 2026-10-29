# Remove Rejected Photos

[Removal and Restore](../spec.md) owns eligibility, retained facts, outcomes, and recovery. This contract owns the browser review of a rejected result. [Trash](../../trash/spec.md) owns the recovery destination and record cleanup.

## Marking and filtering

A Photographer MAY mark each Photo `unflagged`, `picked`, or `rejected`. Marking records a Selection State decision; it does not remove the Photo or change its Original File, Rating, or Album membership.

The `Rejected` filter shows Photos with current Selection State `rejected`. Its result count MUST remain distinct from the source's complete Selection State counts.

Selection State is application-owned. This capability MUST NOT write it to XMP sidecars, infer it from `xmp:Rating`, or change Rating when removing or restoring a Photo.

## Entry and review

When the `Rejected` filter is active, the Library Browser MAY offer **Remove rejected Photos** from View options. It MUST NOT offer this action for other filters or as a general Grid delete action. The review MUST be offered only while removal could be admitted against the open Snapshot.

The review MUST show the current rejected result count. The Photographer MAY select all Photos in that result, including those outside loaded Grid windows. This removal selection is separate from ordinary Grid multi-selection and its 100-Photo limit; it belongs to the current source and `Rejected` filter.

The following events MUST discard the removal selection and require a new review:

- a source change;
- a filter change; or
- reopening the source.

The Photographer MUST see the exact selected count before confirmation. Opening the review MUST NOT remove Photos.

## Confirmation and result

The confirmation action is **Remove from Library**. It MUST state that selected Photos leave normal Library views, remain recoverable, and leave Original Files in place. It MUST NOT offer physical file deletion.

The outcome MUST report the number removed and identify Photos that:

- changed state;
- disappeared;
- were already removed; or
- could not be admitted.

A partial result MUST remain retryable. The rejected view MUST update only from confirmed removals and MUST NOT claim an uncertain Photo was removed.

The completed result MUST offer **Undo** while available if the operation removed Photos. Undo MUST restore the operation's still-eligible Photos as one unit under the [generic Undo contract](../design.md#undo-and-restore). An operation that removed nothing MUST NOT offer Undo.

## Failure behavior

The review MUST remain tied to the result the Photographer reviewed. If that result is no longer current before confirmation, Slipstream MUST discard the selection and require a new review.

A transport failure, malformed response, or incomplete response MUST leave the operation retryable without inventing per-Photo outcomes. The browser MUST retain the review and operation identity until the result is confirmed. A failed operation MUST NOT change visible Selection State, Rating, Album membership, Photo identity, or Original Files.
