# Rejected Photo Cleanup Capability

Photographers use `rejected` Photos as a review queue. After checking that queue, they need to remove unwanted Photos without touching a `selected` or `undecided` Photo and without modifying an Original File. This capability defines the product behavior across the subdomains that own that outcome.

## Photo Review

The Library Browser keeps the existing Selection State values: `undecided`, `selected`, and `rejected`. The `Rejected` filter shows only Photos whose current state is `rejected` and reports the filtered result count separately from the source's complete decision counts.

Cleanup never changes a Photo's Selection State as a side effect. A Photo that is no longer `rejected` when cleanup is confirmed remains in the Library and is reported as changed elsewhere.

## Library Browsing

When the `Rejected` filter is active, View options may offer **Clean up rejected Photos**. The action is unavailable for other filters and does not appear as a general delete action. It opens a cleanup review that names the current result count and explains that the operation moves Photos to the Slipstream Recovery Area.

The Photographer may select all Photos in the current rejected result, including Photos outside the currently loaded Grid windows. This cleanup selection is separate from ordinary Grid multi-selection and its 100-Photo limit. It belongs to the current source and rejected filter. If the source changes, the rejected filter changes, or the source is reopened, the cleanup selection is discarded and must be reviewed again.

Before the operation is admitted, Slipstream must show the exact number of Photos and require an explicit **Move to Recovery Area** action. The confirmation must state that Original Files remain in place and read-only. It must not offer permanent deletion in this flow.

If the reviewed result is no longer current before confirmation, Slipstream must discard the pending cleanup selection and ask the Photographer to review the current rejected result again. A transport failure, malformed response, or incomplete response must leave the cleanup retryable without claiming per-Photo outcomes.

## Photo Library and Recovery

After confirmation, Slipstream moves only Photos that are still `rejected` in the reviewed result into the Recovery Area. Moving a Photo removes it from normal Library Browser sources while retaining its Photo identity, Original Location, Rating, Album membership, and prior Selection State for recovery.

The Recovery Area is an application-owned recovery surface. It does not represent a filesystem directory. The Original File is never renamed, moved, overwritten, or deleted by cleanup or restore.

The result must report the number moved and identify Photos that could not be moved because they changed state, disappeared from the Library, or could not be admitted. A partial result must remain retryable. The current rejected view updates from confirmed outcomes only; it must not claim that an uncertain Photo was moved. A missing Photo is reported as no longer available and is not treated as an implicit deletion.

The completed result provides **Undo** while the result remains available. Undo restores every Photo moved by that operation, including its prior Selection State and other retained facts, as one unit. The Recovery Area also provides a durable restore action after the immediate Undo affordance is gone. Restoring a Photo makes it available to normal sources again without changing its Original File.

## Metadata Interoperability

Selection State is an application-owned fact. This capability does not write it to XMP sidecars, infer it from `xmp:Rating`, or change Rating when moving or restoring a Photo. Any future XMP representation requires a separate interoperability capability and an explicit synchronization action.

## Examples

- A source contains 240 rejected Photos. The Photographer filters to `Rejected`, starts cleanup, reviews “240 Photos,” confirms, and receives “240 moved to Recovery Area.” The Grid then reports no rejected Photos.
- One of 40 reviewed Photos is marked `selected` by another operation before confirmation. Cleanup moves the other 39 and reports one “changed elsewhere” outcome; it does not overwrite that Photo.
- Undo restores a cleanup batch. The restored Photos reappear with their previous Selection State, Rating, Album membership, and Photo identity. The Original Files were never changed.
