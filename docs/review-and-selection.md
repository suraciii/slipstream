# Review and Selection

A Photographer needs to make repeated decisions with minimal delay and without accidental loss. Slipstream therefore makes one-Photo review, touch gestures, visible state, and undo the center of the first product.

## Starting a Review Session

The Photographer may start a Review Session from a Photo Set or a filtered Photo Library view. Each source owns one order:

- A filtered Photo Library Review uses Capture Time order.
- A Photo Set Review uses explicit Photo Set membership order.

A Review Session must snapshot its ordered Photo IDs when it starts. A rescan may refresh availability and Preview facts for Photos already in the Session, but it must not insert, remove, or reorder the active sequence. A later filtered Library Review Session may use new or corrected Capture Time facts from a completed rescan.

Filtered Library Review must use this deterministic order:

1. Photos with a valid authoritative Capture Time, ordered by normalized camera-local Capture Time.
2. Photos without a valid authoritative Capture Time.
3. For equal Capture Times and throughout the missing-time partition, the Photo's ordering Location by UTF-8 bytes.
4. Photo ID by UTF-8 bytes when all earlier values tie.

The Photo's ordering Location is its RAW Original Location when the Photo contains RAW. Otherwise it is its JPEG Original Location.

Photo Set Review must use membership position only. Capture metadata, availability changes, Selection State, Rating, Preview state, and rescans must not reorder a Photo Set.

Slipstream must remember the last reviewed Photo for each Photo Set. Resuming must return to that Photo when it is still a member and available. If it is unavailable, Slipstream must move to the next available member by membership position and wrap once to the first available member. If no member is available, Slipstream must keep the remembered member current. Removing the remembered Photo from the Photo Set clears that Set's saved position. The next Review Session starts at its first available member, or its first member when none are available.

The first product does not persist progress for filtered Photo Library Review.

## Review Surface

The review surface must show one current Photo as the primary content. It must also show:

- current position and total Photo count;
- Selection State;
- Rating;
- Preview Source;
- controls for select, reject, clear, undo, and Rating;
- whether Preview detail is limited.

The first product does not display Capture Time, timezone availability, missing metadata, or RAW/JPEG capture disagreement on the Review surface. These facts affect deterministic Library order only. They must not disable selection, Rating, navigation, or Preview behavior.

The next and previous Photos must remain reachable without recording a decision.

Slipstream may preload nearby Previews. Preloading must not change review progress or selection state.

## Selection State

Each Photo has exactly one Selection State:

- `undecided`;
- `selected`;
- `rejected`.

Selecting a rejected Photo changes it to `selected`. Rejecting a selected Photo changes it to `rejected`. Clearing either state changes it to `undecided`.

A selection action must persist before Slipstream treats the next Photo as safely reviewed. The interface may animate immediately, but a persistence failure must restore or retain the prior visible state and keep the affected Photo recoverable.

## Touch Gestures

When the Preview fits within the review surface:

- a committed right swipe must set `selected`;
- a committed left swipe must set `rejected`;
- a drag below the commit threshold must return the Photo to its starting position without changing state;
- the surface must show the pending direction before release.

A committed swipe advances to the next Photo after the decision is accepted.

Vertical swipe meanings are not part of the first product. Rating uses explicit controls. This avoids direction conflicts and accidental star changes.

Slipstream must provide visible controls equivalent to swipe actions. Gesture use must not be required for accessibility or desktop use.

## Detail Review

Double activation or a pinch gesture may enter Detail Review. The Photographer may zoom and pan within the resolution supplied by the Preview.

While zoomed beyond fit:

- one-finger dragging must pan the Preview;
- horizontal dragging must not select or reject the Photo;
- select and reject remain available through explicit controls.

Returning to fit restores swipe selection.

Slipstream must not upscale a Preview and imply that the added pixels reveal real focus detail. When available Preview resolution is insufficient for the displayed zoom, the interface must identify the limit.

## Rating

A Photo has a Rating from zero through five stars. Zero means no Rating.

Changing Rating must not change Selection State. Selecting or rejecting a Photo must not change Rating.

The Photographer must be able to set Rating through visible controls. Keyboard shortcuts `0` through `5` may provide the same behavior on devices with keyboards.

## Undo

Slipstream must provide undo for the most recent Selection State or Rating change made in the current browser Review Session.

Undo must restore the previous value and return to the affected Photo when the original action advanced away from it.

The first product requires one-level undo. A durable, multi-step action history is not required.

Undo remains available until another Selection State or Rating change occurs, the Photographer leaves the Review Session, or the browser reloads. The browser holds the one undo description; the server does not persist an undo history. Undo must fail without changing state when the Photo's current value no longer matches the value produced by the action being undone.

## Keyboard Behavior

On a device with a keyboard:

- Right Arrow moves to the next Photo without changing it.
- Left Arrow moves to the previous Photo without changing it.
- `P` selects the current Photo.
- `X` rejects the current Photo.
- `U` clears the current Photo's Selection State.
- `0` through `5` set Rating.
- `Ctrl+Z` or `Command+Z` performs undo.

Keyboard actions must follow the same persistence and undo rules as visible controls and gestures.

## Failure Behavior

If the current Preview cannot load, Slipstream must keep the Photo in the review order, identify the failure, and allow navigation without forcing a selection decision.

If a selection or Rating change cannot persist, Slipstream must identify the affected action. It must not silently advance as if the decision were saved.

A disconnected browser must stop accepting new decisions until the server confirms the connection and current Photo state. An already confirmed decision must not be duplicated after reconnect.

## Examples

The Photographer drags a Photo to the right. A selected indicator grows with the drag. The Photographer releases after the commit threshold. Slipstream records `selected` and advances to the next Photo.

The Photographer pinches to inspect a face. A horizontal one-finger drag pans across the enlarged Preview and does not reject or select it.

The Photographer rejects a Photo by mistake, then chooses Undo. Slipstream restores its prior state and returns to that Photo.

A filtered Library contains `shoot/A.JPG` captured at `2026:01:01 10:00:00` and `shoot/Z.JPG` captured at `2026:01:01 09:00:00`. The expected Review order is `Z`, then `A`, even though the filenames sort in the opposite order.

A filtered Library contains `shoot/a.JPG` and `shoot/b.JPG` with the same Capture Time. The expected order is `a`, then `b`. If Capture Time and ordering Location also tie, Photo ID `1a...` sorts before Photo ID `2b...`.

A filtered Library contains `shoot/C.JPG` with a valid Capture Time, `shoot/A.JPG` with malformed `DateTimeOriginal` and no valid fallback, and `shoot/B.JPG` with no recognized capture field. The expected order is `C`, `A`, `B`.

A filtered Library contains RAW-only `shoot/A.ARW` captured at `09:00`, JPEG-only `shoot/B.JPG` captured at `08:00`, and pair `shoot/C.ARW` plus `shoot/C.JPG` captured at `07:00`. The expected order is `C`, `B`, `A`.

Pair `shoot/D.ARW` plus `shoot/D.JPG` reports `11:00` in RAW and `10:00` in JPEG. Slipstream records the disagreement and uses `11:00`. If `shoot/E.JPG` reports `10:30`, the expected order is `E`, then `D`.

`shoot/F.JPG` reports `10:00` with no timezone. `shoot/G.JPG` reports `09:30+01:00`. Capture ordering compares their camera-local values and does not apply the offset. The expected order is `G`, then `F`. Slipstream must not describe `F` as UTC.

`shoot/H.JPG` reports `10:00:00` with no subsecond value. `shoot/I.JPG` reports `10:00:00.001`. Missing subseconds normalize to zero, so the expected order is `H`, then `I`.

`shoot/K.JPG` was captured at `09:00`, was indexed successfully, and later becomes unavailable. `shoot/L.JPG` is available and was captured at `10:00`. A later filtered Library Review keeps `K` before `L`; `K` remains navigable and is identified as unavailable.

A Photo Set explicitly contains `shoot/A.JPG` at position 0 and `shoot/Z.JPG` at position 1, while `Z` has the earlier Capture Time. Photo Set Review must show `A`, then `Z`. A filtered Library Review of the same Photos must show `Z`, then `A`.

A Photo Set contains unavailable `A`, available `B`, and available `C` in that order, and its saved progress points to `A`. Resuming starts at `B`. The unavailable `A` remains in position 0 and remains reachable through Previous.
