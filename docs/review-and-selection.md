# Review and Selection

A Photographer needs to make repeated decisions with minimal delay and without accidental loss. Slipstream therefore makes one-Photo review, touch gestures, visible state, and undo the center of the first product.

## Starting a Review Session

The Photographer may start a Review Session from a Photo Set or a filtered Photo Library view.

Slipstream must use a stable review order for the Session. The first product uses capture time when available, then relative path as a deterministic tie-breaker. A missing capture time sorts by relative path after Photos with capture times.

Slipstream must remember the last reviewed Photo for each Photo Set. Resuming must return to that Photo when it is still a member and available. If it is unavailable, Slipstream must move to the next available member without deleting its state. Removing the remembered Photo from the Photo Set clears that Set's saved position; the next Review Session starts at its first available member.

## Review Surface

The review surface must show one current Photo as the primary content. It must also show:

- current position and total Photo count;
- Selection State;
- Rating;
- Preview Source;
- controls for select, reject, clear, undo, and Rating;
- whether Preview detail is limited.

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
