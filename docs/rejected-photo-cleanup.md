# Rejected Photo Cleanup

Photographers use `rejected` Photos as a review queue. After checking that queue, they need to remove the unwanted Photos without touching a `selected` or `undecided` Photo and without modifying an Original File. Cleanup therefore has its own confirmation and recovery flow instead of reusing ordinary Grid multi-selection.

## Behavior

The Library Browser keeps the existing Selection State values: `undecided`, `selected`, and `rejected`. The `Rejected` filter shows only Photos whose current state is `rejected` and reports the filtered result count separately from the source's complete decision counts.

When the `Rejected` filter is active, View options may offer **Clean up rejected Photos**. The action is unavailable for other filters and does not appear as a general delete action. It opens a cleanup review that names the current result count and explains that the operation moves Photos to the Slipstream Recovery Area.

The Photographer may select all Photos in the current rejected result, including Photos outside the currently loaded Grid windows. This selection is tied to the current source, order, and rejected filter. It is not the ordinary 100-Photo Grid multi-selection. If the source changes, the rejected filter changes, or the source is reopened, the cleanup selection is discarded and must be reviewed again.

Before the operation is admitted, Slipstream must show the exact number of Photos and require an explicit **Move to Recovery Area** action. The confirmation must state that Original Files remain in place and read-only. It must not offer permanent deletion in this flow.

After confirmation, Slipstream moves only Photos that are still `rejected` in the reviewed result into the Recovery Area. Moving a Photo removes it from normal Library Browser sources and keeps its Photo identity, Original Location, Rating, Album membership, and prior Selection State available for recovery. The Original File is never renamed, moved, overwritten, or deleted.

The result must report the number moved and any Photos that could not be moved because they changed state, disappeared from the Library, or could not be admitted. A partial result must identify each outcome and remain retryable. The current rejected view updates from confirmed outcomes only; it must not claim that an uncertain Photo was moved.

The completed result provides **Undo** while the result remains available. Undo restores every Photo moved by that operation, including its prior Selection State and other retained facts, as one unit. The Recovery Area also provides a durable restore action after the immediate undo affordance is gone. Restoring a Photo makes it available to normal sources again without changing its Original File.

## Failure Behavior

The cleanup review must be based on one coherent Browse Snapshot. If that snapshot expires or the Library publication changes before confirmation, Slipstream must discard the pending cleanup selection and ask the Photographer to review the current rejected result again.

If a Photo is no longer `rejected` when the operation commits, Slipstream must leave it in place and report that it changed elsewhere. If the Photo is no longer in the Library, Slipstream must report that it is no longer available. Neither outcome counts as moved or as an implicit deletion.

If transport fails before the server returns an admitted result, Slipstream must keep the cleanup retryable and must not invent per-Photo outcomes. A malformed or incomplete result must be treated the same way. Original Files and retained Photo facts must remain unchanged by a failed operation.

## Metadata boundary

Selection State is an application-owned fact. This capability does not write it to XMP sidecars, does not infer it from `xmp:Rating`, and does not change Rating when moving or restoring a Photo. Any future XMP representation requires a separate interoperability specification and an explicit synchronization action.

## Examples

- A source contains 240 rejected Photos. The Photographer filters to `Rejected`, starts cleanup, reviews “240 Photos,” confirms, and receives “240 moved to Recovery Area.” The Grid then reports no rejected Photos.
- One of 40 reviewed Photos is marked `selected` by another operation before confirmation. Cleanup moves the other 39 and reports one “changed elsewhere” outcome; it does not overwrite that Photo.
- Undo restores a cleanup batch. The restored Photos reappear with their previous Selection State, Rating, Album membership, and Photo identity. The Original Files were never changed.
