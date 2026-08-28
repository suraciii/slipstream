# Library Browsing and Selection

A Photographer opens Slipstream to browse the Photo Library, find Photos, and record decisions. Browsing must become useful before a large Library has transferred every Photo fact or generated every Preview. Slipstream therefore uses one progressively loaded Library Browser for `All Photos` and Photo Sets.

## Library Browser

The Library Browser is Slipstream's primary screen. It must open directly to the `All Photos` Grid rather than a Photo Set landing page. Source navigation must show:

- the Photo Library as the `All Photos` source;
- each Photo Set as another source;
- the Photo count for each source;
- current Library loading or scan status; and
- whether a Photo Set has a saved position.

A wide viewport may keep source navigation beside the Grid. A narrow viewport may present the same sources through a compact control or drawer. Changing source must not require entering a separate workflow.

Opening Slipstream must not require the browser to download every Photo fact or every Photo Set member. The source list and counts must become available from a bounded summary response.

The first product uses two views:

- **Grid View** shows progressively loaded thumbnail cells from the current source.
- **Photo View** shows one current Photo with selection, Rating, Preview, navigation, and Detail Review controls.

The Photographer opens Photo View by activating a Grid cell. Photo View must provide a direct return to Grid View. Returning to Grid View must restore the browser-local scroll position and current Photo when those cells remain in the open source.

Slipstream follows this familiar Library-browser shape without adding desktop editing panels, folder management, keywording, publishing, or RAW adjustment controls.

## Source Order

Each source owns one order:

- `All Photos` uses deterministic Capture Time order.
- A Photo Set uses explicit membership position.

When the Photographer opens or changes a source, Slipstream fixes that source's ordered Photo IDs for the open view. A rescan may refresh availability, Selection State, Rating, and Preview facts, but it must not insert, remove, or reorder Photos in that open view. Reopening or refreshing the source may use a newly published order.

`All Photos` must use this deterministic order:

1. Photos with a valid authoritative Capture Time, ordered by normalized camera-local Capture Time.
2. Photos without a valid authoritative Capture Time.
3. For equal Capture Times and throughout the missing-time partition, the Photo's ordering Location by UTF-8 bytes.
4. Photo ID by UTF-8 bytes when all earlier values tie.

The Photo's ordering Location is its RAW Original Location when the Photo contains RAW. Otherwise it is its JPEG Original Location.

Photo Set order must use membership position only. Capture metadata, availability, Selection State, Rating, Preview state, and rescans must not reorder a Photo Set.

## Progressive Grid Loading

Grid View must become interactive from a bounded first window of Photos. It must not wait for the entire source to transfer, parse, or render.

While a window loads, Grid View must show stable placeholders and a truthful status such as:

```text literal
Loading Photos 1–60 of 36,997…
```

As the Photographer scrolls, Slipstream must load bounded later windows. The browser must keep the number of retained Photo facts and rendered Grid cells bounded independently of total Library size.

A Grid cell must show, when available:

- a cached or progressively generated thumbnail;
- Selection State;
- Rating;
- unavailable state; and
- Preview failure without removing the Photo from its position.

Thumbnail completion must not change source order, Selection State, Rating, or saved Photo Set position.

## Photo View

Photo View must show one current Photo as the primary content. It must also show:

- current position and total Photo count;
- Selection State;
- Rating;
- Preview Source;
- controls for select, reject, clear, undo, and Rating;
- previous and next navigation; and
- whether Preview detail is limited.

The first product does not display Capture Time, timezone availability, missing metadata, or RAW/JPEG capture disagreement in Photo View. These facts affect deterministic Library order only. They must not disable selection, Rating, navigation, or Preview behavior.

The next and previous Photos must remain reachable without recording a decision.

Slipstream must prioritize the current Photo's Preview. After the current Preview is ready, it may prepare the immediately next and previous Previews in the background. This preparation must not change saved position or selection state.

## Loading Feedback

Slipstream must distinguish these user-visible states:

- connecting to the server;
- loading the Library summary;
- preparing the current source order;
- loading a bounded Grid or Photo window;
- preparing a thumbnail or review Preview;
- scanning the Library Folder; and
- disconnected or failed.

Slipstream must show a numeric count or percentage only when it knows the corresponding total and completed amount. Otherwise it must show the current phase without inventing progress.

An existing published Library must remain browsable while an ordinary background rescan checks for changes. The interface must show the current scan phase and real counts when available. Photos discovered by that scan appear after the Photographer refreshes or reopens the source; they must not move the current open view.

On a new state store with no published Library, the browser may show initialization progress until the first scan publishes the Library.

## Saved Position

Slipstream must remember the last Photo shown in Photo View for each Photo Set. Grid scrolling alone does not change durable saved position. Opening that Photo Set must return to the saved Photo when it is still a member and available.

If the saved Photo is unavailable, Slipstream must move to the next available member by membership position and wrap once to the first available member. If no member is available, Slipstream must keep the saved member current.

Removing the saved Photo from the Photo Set clears that Set's saved position. The next opening starts at its first available member, or its first member when none are available.

The first product does not persist an `All Photos` position across browser reloads. Grid scroll position and the current `All Photos` Photo are browser-local.

## Selection State

Each Photo has exactly one Selection State:

- `undecided`;
- `selected`;
- `rejected`.

Selecting a rejected Photo changes it to `selected`. Rejecting a selected Photo changes it to `rejected`. Clearing either state changes it to `undecided`.

A selection action must persist before Slipstream treats navigation caused by that action as safely completed. The interface may animate immediately, but a persistence failure must restore or retain the prior visible state and keep the affected Photo recoverable.

## Touch Gestures

When the Preview fits within Photo View:

- a committed right swipe must set `selected`;
- a committed left swipe must set `rejected`;
- a drag below the commit threshold must return the Photo to its starting position without changing state; and
- the surface must show the pending direction before release.

A committed swipe advances to the next Photo after the decision is accepted.

Vertical swipe meanings are not part of the first product. Rating uses explicit controls. Slipstream must provide visible controls equivalent to swipe actions.

## Detail Review

Double activation or a pinch gesture may enter Detail Review. The Photographer may zoom and pan within the resolution supplied by the Preview.

While zoomed beyond fit:

- one-finger dragging must pan the Preview;
- horizontal dragging must not select or reject the Photo; and
- select and reject remain available through explicit controls.

Returning to fit restores swipe selection. Slipstream must not upscale a Preview and imply that added pixels reveal real focus detail.

## Rating

A Photo has a Rating from zero through five stars. Zero means no Rating.

Changing Rating must not change Selection State. Selecting or rejecting a Photo must not change Rating.

The Photographer must be able to set Rating through visible controls. Keyboard shortcuts `0` through `5` may provide the same behavior on devices with keyboards.

## Undo

Slipstream must provide undo for the most recent Selection State or Rating change made since the Photographer opened the current source.

Undo must restore the previous value and return to the affected Photo when the original action advanced away from it.

The first product requires one-level undo. Undo remains available until another Selection State or Rating change occurs, the Photographer changes source, or the browser reloads. The browser holds the one undo description; the server does not persist undo history.

Undo must fail without changing state when the Photo's current value no longer matches the value produced by the action being undone.

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

If a Grid or Photo window fails to load, Slipstream must retain already loaded content, identify the failed range, and offer retry without returning to an empty application screen.

If the current Preview cannot load, Slipstream must keep the Photo in source order, identify the failure, and allow navigation without forcing a selection decision.

If a selection or Rating change cannot persist, Slipstream must identify the affected action. It must not silently advance as if the decision were saved.

A disconnected browser may continue displaying already loaded thumbnails and Previews, but it must stop accepting new decisions until the server confirms the connection and current Photo state. Reconnect must refresh only the current source window and affected state; it must not require a full-Library transfer.

If an ephemeral server-side browse snapshot expires or is lost after server restart, Slipstream must reopen the current source from the latest published Library and identify that newly completed scans may affect its order.

## Examples

A Library contains 36,997 Photos. Slipstream displays the Library count and first Grid window without transferring all 36,997 Photo facts. Scrolling loads later windows while the source order remains stable.

The Photographer opens `26春节`, activates its fourth Grid cell, and later returns to the source list. Reopening `26春节` returns to that saved Photo when it remains available.

A rescan discovers 100 new Photos while the Photographer is viewing the Library. The open Grid does not insert them or move existing cells. A completion notice offers refresh; reopening `All Photos` includes the new Photos in Capture Time order.

The Photographer opens Photo 100. Slipstream prepares its review Preview first, then prepares Photos 101 and 99 with lower priority. Moving to Photo 101 normally reuses the completed cache entry.

The Photographer drags a Photo to the right. A selected indicator grows with the drag. The Photographer releases after the commit threshold. Slipstream records `selected` and advances to the next Photo.

A Library contains `shoot/A.JPG` captured at `2026:01:01 10:00:00` and `shoot/Z.JPG` captured at `2026:01:01 09:00:00`. Grid and Photo navigation show `Z`, then `A`, even though the filenames sort in the opposite order.

A Photo Set explicitly contains `shoot/A.JPG` at position 0 and `shoot/Z.JPG` at position 1, while `Z` has the earlier Capture Time. That Photo Set shows `A`, then `Z`. `All Photos` shows `Z`, then `A`.
