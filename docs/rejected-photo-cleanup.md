# Library Management: Remove Rejected Photos

`Library Management` owns how Slipstream organizes Photos inside the application. It does not own the Photographer's Original Files. This capability specification separates Photo decisions from file deletion and defines the safe removal of rejected Photos from the Library.

## Capability map

### Mark Photos

A Photographer may mark each Photo as `undecided`, `selected`, or `rejected`. Marking a Photo records a Selection State decision. It does not remove the Photo, change its Original File, change its Rating, or change its Album membership.

The `Rejected` filter shows Photos whose current Selection State is `rejected`. The filtered result count is distinct from the source's complete Selection State counts. Marking and filtering are separate from deletion.

### Delete Photos

Deleting a Photo is a separate capability from marking it. It has two distinct outcomes:

- **Remove from Library** removes a Photo from normal Slipstream Library views while preserving a recoverable Photo record and its Original File. This is the capability specified here.
- **Delete Original Files** physically deletes the Photographer's Original File only through [Trash and Permanent Deletion](library-management-trash.md). It is separate from reversible removal. Marking a Photo or removing it from the Library must never trigger it implicitly.

## Remove Rejected Photos

The browser review below is one invocation of the removal capability.
[Composable Removal and Restore](library-management-removal-and-restore.md)
also permits explicitly identified Photo sets without a browser view. Both forms
must enforce the same removal eligibility and preservation rules.

### Entry and review

The user-facing destination for removed Photos is **Trash**, governed by [Trash and Permanent Deletion](library-management-trash.md). Removal places Photos there; it never deletes their Original Files.

When the `Rejected` filter is active, the Library Browser may offer **Remove rejected Photos**. The action is unavailable for other filters and does not appear as a general delete action.

The removal review must show the current rejected result count. The Photographer may select all Photos in that result, including Photos outside the currently loaded Grid windows. This removal selection is separate from ordinary Grid multi-selection and its 100-Photo limit. It belongs to the current source and `Rejected` filter.

If the source changes, the filter changes, or the source is reopened, Slipstream must discard the removal selection and require a new review. The Photographer must be able to see the exact number of Photos before confirming.

### Confirmation and result

The confirmation action is **Remove from Library**. The confirmation must state that the selected Photos will leave normal Library views, remain recoverable, and leave Original Files in place. It must not offer physical file deletion as part of this action.

Slipstream removes only Photos that are still `rejected` in the reviewed result. A removed Photo keeps its Photo identity, Original Location, Rating, Album membership, and prior Selection State for recovery. The Original File is never renamed, moved, overwritten, or deleted.

The result must report the number removed and identify Photos that could not be removed because they changed state, disappeared from the Library, or could not be admitted. A partial result remains retryable. The current rejected view updates from confirmed removals only; it must not claim that an uncertain Photo was removed.

### Restore

The completed result provides **Undo** while the result remains available. Undo restores every Photo removed by that operation, including its prior Selection State and other retained facts, as one unit.

Durable single and batch Restore follow [Trash and Permanent Deletion](library-management-trash.md#selection-and-restore).

### Failure behavior

The removal review must remain tied to the result the Photographer reviewed. If that result is no longer current before confirmation, Slipstream must discard the pending selection and ask the Photographer to review the current rejected result again.

If a Photo is no longer `rejected` when removal is confirmed, Slipstream must leave it in the Library and report that it changed elsewhere. If the Photo is no longer in the Library, Slipstream must report that it is no longer available. Neither outcome counts as removed or as an implicit physical deletion.

A transport failure, malformed response, or incomplete response must leave the operation retryable without inventing per-Photo outcomes. A failed operation must not change visible Selection State, Rating, Album membership, Photo identity, or the Original File.

## Metadata boundary

Selection State is an application-owned fact. This capability does not write it to XMP sidecars, infer it from `xmp:Rating`, or change Rating when removing or restoring a Photo. Any future XMP representation and synchronization action requires a separate capability specification.

## Examples

- A source contains 240 rejected Photos. The Photographer filters to `Rejected`, starts removal, reviews “240 Photos,” confirms **Remove from Library**, and receives “240 Photos removed.” The Grid then reports no rejected Photos.
- One of 40 reviewed Photos is marked `selected` before confirmation. Slipstream removes the other 39 and reports one “changed elsewhere” outcome; it does not overwrite that Photo.
- Undo restores a removal batch. The restored Photos reappear with their previous Selection State, Rating, Album membership, and Photo identity. The Original Files were never changed.
