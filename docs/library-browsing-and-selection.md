# Library Browsing and Selection

A Photographer opens Slipstream to browse the Photo Library, find Photos, and record decisions. Browsing must become useful before a large Library has transferred every Photo fact, every Album member, or every Original Folder. Slipstream therefore uses one progressively loaded Library Browser for `All Photos`, File Locations, and Albums.

## Library Browser

The Library Browser is Slipstream's primary screen. It must open directly to the `All Photos` Grid rather than an Album landing page. Source navigation must show:

- the Photo Library as the `All Photos` source;
- the read-only Library Folder root and its Original Folders under a `File Locations` section;
- each Album under a separate `Albums` section;
- the Photo count for each displayed source;
- current Library loading or scan status; and
- whether an Album has a saved position.

The active Grid status and every source card must use `1 Photo` for one and `N Photos` for every other count in both visible and accessible text.

A wide viewport must keep compact source navigation beside the Grid. A narrow viewport must place the same navigation in a drawer opened by a visible `Sources` control. The drawer must not consume the Grid's first viewport while closed, must close after the Photographer chooses a source, and must return focus to the `Sources` control when dismissed. Both layouts must preserve the `File Locations` and `Albums` distinction. A Folder and Album with the same name must remain distinguishable by section and source labeling. Changing source must not require entering a separate workflow.

The current source must be visually and programmatically identifiable. Source state must not depend on color alone.

Opening Slipstream must not require the browser to download every Photo fact, every Album member, or the complete Original Folder tree. Album summaries may arrive with the bounded Library Overview. File Locations must show a root labeled `Library Folder` without exposing its absolute server path, then load descendants as bounded direct-child Folder windows. A Folder window must report its real parent, range, direct-child count, and recursive Photo counts without returning complete recursive membership.

All Folder windows retained together must come from the same Published Library. Library summary counts and Albums must also be revalidated against that publication before they replace visible shared facts. If a rescan replaces the publication while an older summary or File Locations are loading, Slipstream must discard the older summary and refresh File Locations rather than append or present facts from different publications. Opening a Folder from an expired publication must refresh navigation and require the Photographer to open the current Folder projection; it must not silently reinterpret the stale Location against a different publication.

The first product uses two views:

- **Grid View** shows progressively loaded thumbnail cells from the current source.
- **Photo View** shows one current Photo with selection, Rating, Preview, navigation, and Detail Review controls.

The Photographer opens Photo View by activating a Grid cell. Photo View must provide a direct return to Grid View. Returning to Grid View must restore the browser-local scroll position and current Photo when those cells remain in the open source. Keyboard focus must return to that Photo cell when it is rendered, or to the Grid viewport when it is not.

Slipstream follows this familiar Library-browser shape without adding desktop editing panels, folder mutation, Album Groups, Smart Albums, keywording, publishing, or RAW adjustment controls.

## Presentation and Control Hierarchy

The Library Browser must use the current Photo and Photo Grid as its dominant surfaces. Source navigation, status, and controls must support those surfaces without competing with them for attention or space.

The first product uses one neutral dark appearance. Color must communicate keyboard focus, Selection State, Rating, failure, or connectivity rather than decorate unrelated containers. The interface must not depend on color alone to communicate a Photo decision or control state.

Required information must meet WCAG AA text contrast against its rendered background. Controls intended for touch must provide at least a 44 by 44 CSS-pixel target on narrow viewports. Focused controls must remain visibly distinguishable.

Grid View must keep its current source name, truthful loading status, and Library refresh action compact so the Photo Grid remains visible. Source rows must read as navigation rather than a collection of promotional cards.

Photo View must keep the Preview larger than any control group when the viewport can display a usable Preview. It must group selection decisions, Rating, Album membership, and navigation by purpose. Select and reject are the primary review actions; clear, undo, Detail Review, and Album membership are supporting actions. Previous and next navigation must remain visible without implying a selection decision.

When a short viewport cannot show a usable Preview and every control at once, Photo View must preserve a usable Preview and provide a vertical path to every existing control. It must not clip controls without a way to reach them. At supported narrow widths, Grid cells must divide each complete row evenly across the available Grid width, leaving no more than the ordinary inter-cell gap at the trailing edge. The `Library Folder` source label must remain fully readable.

Clear must be unavailable while the current Photo is already `undecided`. Undo must be unavailable until the current source has an undoable Selection State or Rating change.

## Source Order

Each source owns one order:

- `All Photos` uses deterministic Capture Time order.
- An Original Folder filters that same order to one recursive Folder subtree.
- An Album uses explicit membership position.

When the Photographer opens or changes a source, Slipstream fixes that source's ordered Photo IDs for the open view. A rescan may refresh availability, Selection State, Rating, Preview facts, and File Location navigation, but it must not insert, remove, or reorder Photos in that open view. Reopening or refreshing the source may use a newly published order or Folder subtree.

`All Photos` must use this deterministic order:

1. Photos with a valid authoritative Capture Time, ordered by normalized camera-local Capture Time.
2. Photos without a valid authoritative Capture Time.
3. For equal Capture Times and throughout the missing-time partition, the Photo's ordering Location by UTF-8 bytes.
4. Photo ID by UTF-8 bytes when all earlier values tie.

The Photo's ordering Location is its RAW Original Location when the Photo contains RAW. Otherwise it is its JPEG Original Location.

Original Folder order must use the `All Photos` order filtered by component-aware Folder ancestry. A Folder named `a` must not include a sibling named `ab`. Folder filtering must count a RAW/JPEG pair once by the parent of its ordering Original Location.

Album order must use membership position only. Capture metadata, availability, Selection State, Rating, Preview state, Original Folder changes, and rescans must not reorder an Album.

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
- Photo unavailability and pairing ambiguity as distinct facts;
- Preview unavailability or failure without removing the Photo from its position; and
- thumbnail delivery failure without replacing the Photo or Preview facts above.

Thumbnail completion must not change source order, Selection State, Rating, or saved Album position.

Thumbnail, Preview, scan, and bounded look-ahead loading must run in the background. Rebuildable or superseded loading must not delay source changes, navigation, already available controls, or returning between Grid View and Photo View. Changing source or view must cancel pending image transfers and requests owned only by the previous source, Photo, or view; hiding obsolete loading is not sufficient when it would continue consuming capacity. Slipstream may wait only when the requested action depends on confirmed facts or persistence, such as opening a source's first bounded window or safely completing a Selection State change before advancing.

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

An existing published Library must remain browsable while an ordinary background rescan checks for changes. The interface must show the current scan phase and real counts when available. If the check fails, Slipstream must retain the prior Published Library and offer **Retry Library Check**. When a replacement publishes, a completion notice must offer **Refresh Current Source**. Shared counts and File Locations may refresh immediately, but Photos discovered by that scan appear in the open source only after the Photographer refreshes or reopens it; they must not move the current open view.

On a new state store with no published Library, the browser may show initialization progress until the first scan publishes the Library.

## Empty Sources

An empty source must identify what is empty and must not imply that Photos or Original Files were removed.

When the Photo Library contains no Photos, the `All Photos` source and Library Folder source must say that no supported Photos were found. They must direct the operator to check the configured Library Folder or add supported files, then offer **Check Library** through the existing rescan workflow. A completed check follows the normal **Refresh Current Source** contract above.

An empty Album must remain openable and manageable. Its Grid must say that the Album contains no Photos and that the Photographer can add Photos from another source's Photo View. Rename and delete remain available through source navigation.

## Saved Position

Slipstream must remember the last Photo shown in Photo View for each Album. Grid scrolling alone does not change durable saved position. Opening that Album must return to the saved Photo when it is still a member and available.

If the saved Photo is unavailable, Slipstream must move to the next available member by membership position and wrap once to the first available member. If no member is available, Slipstream must keep the saved member current.

Removing the saved Photo from the Album clears that Album's saved position. The next opening starts at its first available member, or its first member when none are available.

The first product does not persist an `All Photos` or Original Folder position across browser reloads. Grid scroll position and the current Photo in those sources are browser-local.

## Album Management

The Photographer must be able to create an Album from source navigation. Album names must be nonempty, at most 120 characters, and unique case-insensitively within the flat Album list. A newly created empty Album must open to a usable empty Grid with controls to rename or delete it.

Opening Album creation or rename must focus the Album name input and select its current value. Closing an Album form after cancellation or a completed action must return focus to the control that opened it when that control remains available, or to the nearest stable Album action. A validation or persistence failure must keep the form and its input recoverable; closing that form follows the same focus rule.

The Photographer must be able to rename an Album and delete an Album after a confirmation that Photos and Original Files remain unchanged. Deleting the open Album must return to `All Photos` or another valid source without leaving an unusable current source.

Photo View must let the Photographer add the current Photo to one or more Albums. Adding a Photo that already belongs to an Album must not create a duplicate membership. When the current source is an Album, Photo View must let the Photographer remove the current Photo from that Album.

A confirmed membership addition appends the Photo after existing members. Removing a Photo must compact later membership positions without changing their relative order. Removing the saved or current Photo must apply the saved-position rules before the Album is next opened.

Album mutations must persist before Slipstream presents them as complete. Changing source or Photo must not cancel an admitted persistence operation, but a late response from an obsolete UI generation must not overwrite the current source, current Photo, or current error state.

The first Album-management interface does not require Grid multi-select, drag-and-drop, a visual bulk reorder surface, Album covers, sharing, Album Groups, or Smart Albums.

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

Exactly one visible Rating control from zero through five must communicate the current Rating visually and programmatically. Zero must remain an explicit current value when the Photo has no Rating.

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

If a File Location window fails to load, Slipstream must retain already loaded Folder nodes, identify the failed range, and offer retry without collapsing unrelated navigation. If its publication expired, Slipstream must replace the retained File Locations with one coherent current publication and identify that scan results changed them.

If a Grid or Photo window fails to load, Slipstream must retain already loaded content, identify the failed range, and offer retry without returning to an empty application screen.

If the current Preview cannot load, Slipstream must keep the Photo in source order, identify the failure, and allow navigation without forcing a selection decision.

If an Album creation, rename, delete, or membership change cannot persist, Slipstream must identify the affected Album and action. It must retain a recoverable current source and must not present the failed change as complete.

If a selection or Rating change cannot persist, Slipstream must identify the affected action. It must not silently advance as if the decision were saved.

A disconnected browser may continue displaying already loaded thumbnails and Previews, but it must stop accepting new decisions until the server confirms the connection and current Photo state. Success from an unrelated request, such as another File Location range, does not confirm that state or re-enable decisions. Reconnect must refresh only the current source window and affected state; it must not require a full-Library transfer.

If an ephemeral server-side browse snapshot expires or is lost after server restart, Slipstream must reopen the current source from the latest published Library and identify that newly completed scans may affect its order.

## Examples

A Library contains 36,997 Photos. Slipstream displays the Library count and first Grid window without transferring all 36,997 Photo facts. Scrolling loads later windows while the source order remains stable.

The Photographer opens the Album `26春节`, activates its fourth Grid cell, and later returns to the source list. Reopening that Album returns to the saved Photo when it remains available.

The filesystem also contains the Original Folder `RAW/26春节`. Opening it shows Photos from that Folder and its descendants in Capture Time order. Its matching name does not connect it to the Album or change Album membership.

A rescan discovers 100 new Photos while the Photographer is viewing the Library. The open Grid does not insert them or move existing cells. A completion notice offers refresh; reopening `All Photos` includes the new Photos in Capture Time order.

The Photographer opens Photo 100. Slipstream prepares its review Preview first, then prepares Photos 101 and 99 with lower priority. Moving to Photo 101 normally reuses the completed cache entry.

The Photographer drags a Photo to the right. A selected indicator grows with the drag. The Photographer releases after the commit threshold. Slipstream records `selected` and advances to the next Photo.

A Library contains `shoot/A.JPG` captured at `2026:01:01 10:00:00` and `shoot/Z.JPG` captured at `2026:01:01 09:00:00`. Grid and Photo navigation show `Z`, then `A`, even though the filenames sort in the opposite order.

An Album explicitly contains `shoot/A.JPG` at position 0 and `shoot/Z.JPG` at position 1, while `Z` has the earlier Capture Time. That Album shows `A`, then `Z`. `All Photos` and the `shoot` Original Folder show `Z`, then `A`.
