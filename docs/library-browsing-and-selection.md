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

A source name that is visually truncated must expose its complete name on hover. An Original Folder with no descendant Folders must not present an expand control; affected rows must keep their alignment without it.

Opening Slipstream must not require the browser to download every Photo fact, every Album member, or the complete Original Folder tree. Album summaries may arrive with the bounded Library Overview. File Locations must show a root labeled `Library Folder` without exposing its absolute server path, then load descendants as bounded direct-child Folder windows. A Folder window must report its real parent, range, direct-child count, and recursive Photo counts without returning complete recursive membership.

All Folder windows retained together must come from the same Published Library. Library summary counts and Albums must also be revalidated against that publication before they replace visible shared facts. If a rescan replaces the publication while an older summary or File Locations are loading, Slipstream must discard the older summary and refresh File Locations rather than append or present facts from different publications. Opening a Folder from an expired publication must refresh navigation and require the Photographer to open the current Folder projection; it must not silently reinterpret the stale Location against a different publication.

The first product uses two views:

- **Grid View** shows progressively loaded thumbnail cells from the current
  source.
- **Photo View** shows one current Photo with selection, Rating, Preview,
  Album membership, navigation, and Preview zoom controls.

The Photographer opens Photo View by activating a Grid cell. A Grid cell must be enabled only while its activation can immediately open Photo View against the current source. While Slipstream replaces an expired source snapshot, it must make retained Grid cells unavailable until the replacement source is ready. It must not expose an enabled Grid cell whose activation does nothing. Photo View must provide a direct return to Grid View. Returning to Grid View must restore the browser-local scroll position and current Photo when those cells remain in the open source. Keyboard focus must return to that Photo cell when it is rendered, or to the Grid viewport when it is not.

Slipstream follows this familiar Library-browser shape without adding desktop editing panels, folder mutation, Album Groups, Smart Albums, keywording, publishing, or RAW adjustment controls.

## Presentation and Control Hierarchy

The Library Browser must use the current Photo and Photo Grid as its dominant surfaces. Source navigation, status, and controls must support those surfaces without competing with them for attention or space.

The Library Browser must expose exactly one main landmark while it loads and after it becomes usable, so assistive technology identifies one primary screen.

The first product uses one neutral dark appearance. Color must communicate keyboard focus, Selection State, Rating, failure, or connectivity rather than decorate unrelated containers. The interface must not depend on color alone to communicate a Photo decision or control state.

Required information must meet WCAG AA text contrast against its rendered background. Every visible interactive target must provide at least a 44 by 44 CSS-pixel target at supported narrow and short-landscape viewports. Focused controls must remain visibly distinguishable.

Grid View must keep its current source name, truthful loading status, and Library refresh action compact so the Photo Grid remains visible. Source rows must read as navigation rather than a collection of promotional cards. The Library refresh action is a recovery action and must not occupy a permanently prominent primary slot.

Album rename and delete are supporting actions. On a wide viewport whose primary pointer can hover, they may stay concealed until the Photographer hovers or focuses their Album row. They must remain visible without hover at supported touch and narrow viewports, and must remain reachable from the keyboard.

A valid Album name must not widen Grid View or Photo View beyond the Library Browser at a supported viewport. Its visible current-source title may be visually truncated, but assistive technology must retain the complete Album name.

Photo View must keep the Preview larger than any control group when the viewport can display a usable Preview. It must group selection decisions, Rating, Album membership, and navigation by purpose. Select and reject are the primary review actions; clear, undo, Preview zoom, and Album membership are supporting actions. Previous and next navigation must remain visible without implying a selection decision.

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

## Source Ordering Selection

Grid View must expose one explicit order selection for the open source:

- `All Photos` and an Original Folder offer Capture Time, earliest first
  (the default) and Capture Time, latest first.
- An Album offers Album order (the default), Capture Time earliest first, and
  Capture Time latest first.

A time order belongs to the open view only. It must never rewrite persisted
Album member positions, and an Album's persisted order remains the default
when it is reopened.

The server applies the selected order to the complete source before it
paginates windows, so Grid and Photo navigation share one globally ordered
view. Photos without a valid authoritative Capture Time sort last in both
directions. Equal Capture Times keep the deterministic ordering Location and
Photo ID tie-breakers; reversing the time direction never reverses those
tie-breakers and never moves missing-time Photos ahead of timed Photos. Time
ordering reuses the existing RAW/JPEG Capture Time authority and camera-local
normalization; it does not guess time zones and does not use file modification
time.

Changing the order must keep the current Photo, resolved by Photo ID, and
reposition the view around it. Opening or reopening a different source uses
that source's default order. The first product does not persist the selected
order across reloads. Album saved positions restore by Photo identity and are
unaffected by the selected view order.

## Progressive Grid Loading

Grid View must become interactive from a bounded first window of Photos. It must not wait for the entire source to transfer, parse, or render.

While a window loads, Grid View must show stable placeholders and a truthful status such as:

```text literal
Loading Photos 1–60 of 36,997…
```

As the Photographer scrolls, Slipstream loads bounded later windows. The
browser must keep the number of retained Photo facts and rendered Grid cells
bounded independently of total Library size.

Loading is range-based and rendering never initiates it:

- scrolling or Grid keyboard movement computes the visible range plus a
  bounded buffer and reports that one range; the Grid requests only the
  bounded windows still missing for that range, and one completion
  notification updates everything the range needs — neither scrolling nor
  keyboard movement creates per-cell loading work;
- Grid updates are merged to at most one per animation frame while scrolling,
  and each update reuses the DOM nodes of Photos that remain visible: only
  cells that enter, leave, or change are added, removed, or updated;
- already loaded Photos keep their fixed-size cells while later positions
  load, so stopping the scroll converges to a complete view without repeated
  placeholder churn; and
- retained Photo facts are capped at a bound that covers the actual viewport
  and buffer at supported large viewports; eviction protects the current
  range and buffer and always uses the latest visible range, never a position
  captured when an older request started.

A Grid cell must show, when available:

- a cached or progressively generated thumbnail;
- the Original filename;
- Selection State;
- Rating;
- Photo unavailability and pairing ambiguity as distinct facts;
- Preview unavailability or failure without removing the Photo from its position; and
- thumbnail delivery failure without replacing the Photo or Preview facts above.

The Original filename is the basename of the Photo's ordering Original Location
defined under Source Order. A Grid cell must keep the position number visible
and must truncate a long filename instead of breaking the cell layout.

A Grid cell must carry a Selection State badge only while that Photo is `selected` or `rejected`. An `undecided` Photo must show no badge, so the badge always marks a recorded decision.

Thumbnail completion must not change source order, Selection State, Rating, or saved Album position.

## Grid Composition and Orientation

Grid cells remain uniform, virtualizable units. The Photo image inside each
cell must display complete and unstretched at its true aspect ratio:
landscape Photos render wider than tall, portrait Photos taller than wide,
and square Photos square. A panoramic Photo must read as one long
composition rather than a cropped strip.

Letterboxing must stay quiet. A cell must not crop every Photo to one
composition and must not stretch an image to fill its area. Selection State,
Rating, and Photo state indicators must not overlay the Photo image; they
render beside or beneath the image area.

Orientation follows the EXIF-corrected derivative dimensions. A Photo whose
Preview derivative applies an EXIF rotation must display with the corrected
orientation exactly once, and a RAW/JPEG pair remains one Photo with one
cell.

While a thumbnail is not yet loaded or its dimensions are unknown, the cell
must keep a stable placeholder. The placeholder must not fake an orientation
and must not cause Grid layout to jump when the thumbnail arrives.

Thumbnail, Preview, scan, and bounded look-ahead loading must run in the background. Rebuildable or superseded loading must not delay source changes, navigation, already available controls, or returning between Grid View and Photo View. Changing source or view must cancel pending image transfers and requests owned only by the previous source, Photo, or view; hiding obsolete loading is not sufficient when it would continue consuming capacity. Slipstream may wait only when the requested action depends on confirmed facts or persistence, such as opening a source's first bounded window or safely completing a Selection State change before advancing.

## Photo View

Photo View must show one current Photo as the primary content. It must also show:

- current position and total Photo count;
- the Original filename when available;
- Selection State;
- Rating;
- Preview Source;
- the Albums that contain the current Photo;
- controls for select, reject, clear, undo, and Rating;
- previous and next navigation; and
- whether Preview detail is limited; and
- review-relevant capture metadata when available: Capture Time, Aperture,
  ISO, Shutter Speed, and Focal Length.

Capture metadata is read-only. A missing or unreadable field must display an
explicit `—` value. The displayed values follow the same RAW-first, JPEG
fallback authority used for Capture Time. Metadata loading failure must not
disable selection, Rating, navigation, or Preview behavior. Slipstream does
not provide a general EXIF editor or an unbounded metadata browser.

Capture Time must display as camera-local `YYYY-MM-DD HH:MM`. Sub-second
precision and the timezone-free normalized form are transport detail and must
not appear in the interface. Slipstream must not convert, re-interpret, or
invent a timezone for this value.

Limited Preview detail must be identified together with the Preview Source
fact, and must not be presented as a separate fact row. The complete
explanation of the limit must remain available to assistive technology.

The next and previous Photos must remain reachable without recording a decision.

Slipstream must prioritize the current Photo's Preview. After the current Preview is ready, it may prepare the immediately next and previous Previews in the background. This preparation must not change saved position or selection state.

Photo View must keep the Preview inside the available viewport width. The
Preview area may be shorter than the full Photo View when the remaining
controls need more room, but it must remain large enough to inspect the image
and must never create horizontal overflow. The Photo View may scroll
vertically on a short viewport so every existing control remains reachable.

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

When the current source is an Original Folder, the Grid header must offer one
explicit **Add Folder** action. The Photographer chooses an Album and confirms
the action. Slipstream adds every Photo in that Folder and its descendants in
the same order as the open Folder source. The browser sends the Folder
Location and its Published Library value, not the complete Photo list.

The action is one bounded, idempotent operation. A Photo already in the Album
is skipped without changing its membership position. The response must report
the matched, added, and already-member counts. While it is running, the
action is disabled and reports that the Folder is being added. A failed or
expired operation must remain visibly incomplete and may be retried after the
current Folder source is refreshed. A Folder with more than the server's
bounded operation limit is rejected before any membership is committed.

A confirmed membership addition appends the Photo after existing members. Removing a Photo must compact later membership positions without changing their relative order. Removing the saved or current Photo must apply the saved-position rules before the Album is next opened.

Album mutations must persist before Slipstream presents them as complete. Changing source or Photo must not cancel an admitted persistence operation, but a late response from an obsolete UI generation must not overwrite the current source, current Photo, or current error state.

The first Album-management interface does not require Grid multi-select, drag-and-drop, a visual bulk reorder surface, Album covers, sharing, Album Groups, or Smart Albums. Adding an entire current Folder is supported separately from Grid multi-select.

## Photo Album Membership

Photo View must show which Albums contain the current Photo. The membership
list is the server's answer for that Photo, distinct from the browsing
source and from recent management actions: viewing the same Photo from any
source must present the same membership, and it must remain correct after a
reload.

A Photo in no Album must say so explicitly. Membership loading and failure
are displayed independently of the Preview and capture metadata; a failed
membership load must not disable selection, Rating, navigation, or Preview
behavior and must offer a retry that reloads only the membership facts.

Photo View must offer one membership management panel that lists Albums with
a checkbox reflecting the Photo's true membership. Checking an Album adds
the Photo through the same persistence rules as any membership addition,
and unchecking removes it through the same rules as membership removal,
including the browsing-position contract when removing from the open Album.
Creating a new Album reuses the existing Album creation entry. An Album list
too large for one view must load on demand rather than transferring every
member of every Album.

A successful add or remove must update the membership list and the Album
counts. A failed toggle must keep the prior true state, identify the failed
Album and action, and leave the toggle retryable. Adding a Photo that
already belongs must not create a duplicate membership.

Switching Photos quickly must never present another Photo's membership. A
membership response that arrives after its Photo is no longer current must
be discarded.

## Selection State

Each Photo has exactly one Selection State:

- `undecided`;
- `selected`;
- `rejected`.

Selecting a rejected Photo changes it to `selected`. Rejecting a selected Photo changes it to `rejected`. Clearing either state changes it to `undecided`.

A selection action must persist before Slipstream treats navigation caused by that action as safely completed. The interface may animate immediately, but a persistence failure must restore or retain the prior visible state and keep the affected Photo recoverable.

## Touch Gestures

While the Preview zoom state is Fit:

- a committed right swipe must set `selected`;
- a committed left swipe must set `rejected`;
- a drag below the commit threshold must return the Photo to its starting position without changing state; and
- the surface must show the pending direction before release.

The direction labels are a touch affordance. A device whose primary pointer
can hover must not show them outside an in-progress decision drag.

A committed swipe advances to the next Photo after the decision is accepted.

Vertical swipes do not record a decision. At supported narrow or short-landscape touch viewports, while the zoom state is Fit, a vertical gesture that begins on the Preview must scroll Photo View naturally. A two-finger pinch always zooms the Preview, and a manual zoom state gives one-finger dragging to bounded panning instead of decisions. Rating uses explicit controls. Slipstream must provide visible controls equivalent to swipe actions.

## Preview Zoom and Fit

Photo View must keep the complete Preview visible by default. **Fit** shows
the entire Preview without cropping, as large as the available Preview area
allows, and is the state every Photo opens in.

Photo View must expose explicit zoom controls:

- a **Fit Window** action that restores the complete composition;
- a zoom out action, a continuous zoom slider, and a zoom in action;
- a live zoom percentage; and
- a **100%** action.

The percentage is relative to the Preview image's own pixels. `100%` maps one
Preview pixel to one CSS pixel regardless of device pixel ratio; it reports
the honest derivative resolution and never implies RAW sensor precision. The
zoom range is 10% to 800% of the Preview's natural size. The buttons and
wheel step zoom multiplicatively, and the slider moves continuously within
the same range. Fit is reported with the percentage it actually produces.

On a device with a pointer wheel, wheel zoom must keep the image point under
the pointer stationary while zooming. On a touch device, a two-finger pinch
zooms around the gesture midpoint. Once the Photo is larger than the Preview
area, dragging pans within bounded overflow so the image cannot be dragged
out of view; while it fits, panning is not available and the image stays
centered.

Zoom and pan must never record a Selection State or Rating. While the zoom
state is manual, one-finger dragging pans the Preview and swipe decisions are
unavailable; explicit Select and Reject controls stay available. In Fit,
horizontal dragging keeps the decision gesture and vertical dragging scrolls
a short Photo View naturally.

Every zoom control must be operable from the keyboard, and the current zoom
state must be exposed programmatically. Keyboard shortcuts `+` and `-` step
zoom, `F` restores Fit, and `D` toggles a 200% detail zoom.

Changing Photo or returning to Grid View must reset the zoom state to Fit.
When the viewport or Preview area changes size, Fit must recompute, and a
manual zoom must keep its percentage while its pan is constrained again.
Zooming above 100% is allowed but is always reported truthfully; the interface
must not suggest the source contains detail beyond the Preview derivative.

## Rating

A Photo has a Rating from zero through five stars. Zero means no Rating.

Changing Rating must not change Selection State. Selecting or rejecting a Photo must not change Rating.

The Photographer must be able to set Rating through visible controls. Keyboard shortcuts `0` through `5` may provide the same behavior on devices with keyboards.

Exactly one visible Rating control from zero through five must communicate the current Rating visually and programmatically. Zero must remain an explicit current value when the Photo has no Rating.

Photo View must present a Photo with no Rating as `No rating` in its Rating
fact. A rated Photo must present `<N> star` for one and `<N> stars`
otherwise. This wording applies to the fact only; the Rating control keeps
zero as an explicit value.

## Undo

Slipstream must provide undo for the most recent Selection State or Rating change made since the Photographer opened the current source.

Undo must restore the previous value and return to the affected Photo when the original action advanced away from it.

The first product requires one-level undo. Undo remains available until another Selection State or Rating change occurs, the Photographer changes source, or the browser reloads. The browser holds the one undo description; the server does not persist undo history.

The undo description must identify its affected Photo by stable Photo ID. Any position retained with that description is only a hint for the current Browse Snapshot. When the same source is reopened with a replacement Snapshot, Slipstream must resolve the Photo ID against that Snapshot before loading or applying Undo. The resolution must use one bounded position lookup and must not transfer the complete source or create browser-global state.

If the affected Photo is no longer in the current source, Slipstream must clear the undo description and identify that Undo is no longer available. If the bounded lookup fails, Slipstream must keep the undo description and current Photo unchanged, identify that the target could not be located, and allow the Photographer to retry. It must not send the Undo mutation until the target identity and current position are confirmed.

Undo must fail without changing state when the Photo's current value no longer matches the value produced by the action being undone.

## Keyboard Behavior

On a device with a keyboard, Photo View provides:

- Right Arrow moves to the next Photo without changing it.
- Left Arrow moves to the previous Photo without changing it.
- `P` selects the current Photo.
- `X` rejects the current Photo.
- `U` clears the current Photo's Selection State.
- `0` through `5` set Rating.
- `Ctrl+Z` or `Command+Z` performs undo.

Keyboard actions must follow the same persistence and undo rules as visible controls and gestures.

## Grid View Keyboard

Grid View must be operable from a keyboard without opening Photo View.

- The Grid is entered once by Tab: the Grid viewport is its entry and fallback Tab stop, and at most one rendered cell is a Tab stop at a time (the cell the keyboard owns). Arrow keys move cell focus. Left and Right move one Photo; Up and Down move one complete row.
- Moving cell focus must load the bounded window that contains the target row, exactly as scrolling does. Cell focus must not stop at the edge of a loaded window.
- While the focused Photo's bounded window is loading, the Grid keeps keyboard focus and returns it to the cell once that cell renders.
- The focused cell must keep its visible focus ring when a window replacement or a merged render rebuilds it.
- `Enter` opens the focused Photo in Photo View.
- `P`, `X`, `U`, and `0` through `5` apply to the focused Photo, with the same persistence and undo rules as their Photo View equivalents. A decision must not move cell focus, and `U` must perform no write while the focused Photo is already `undecided`.
- A failed Grid decision must report on the Grid's status line and must keep the affected Photo recoverable. It must not open Photo View.
- Grid keys must act only while the Grid owns keyboard focus. Another surface with focus, such as an Album name input, must receive its own keys unchanged.

While the Grid is open, undo of a change that did not advance away from the affected Photo must restore the value in place, keep the Grid open, and return cell focus to the affected Photo. Undo of a change that advanced away must return to the affected Photo in Photo View. The browser holds one undo description, shared with Photo View.

## Failure Behavior

If a File Location window fails to load, Slipstream must retain already loaded Folder nodes, identify the failed range, and offer retry without collapsing unrelated navigation. If its publication expired, Slipstream must replace the retained File Locations with one coherent current publication and identify that scan results changed them.

If a Grid or Photo window fails to load, Slipstream must retain already loaded content, identify the failed range, and offer retry without returning to an empty application screen.

Photo navigation must commit a new current Photo only after its bounded Photo facts are available. If navigation cannot commit its target, Slipstream must retain the prior visible current Photo and position as the Photo Retry target. The displayed Photo, recovery target, and retried bounded window must refer to the same Photo.

If the current Preview cannot load, Slipstream must keep the Photo in source order, identify the failure, and allow navigation without forcing a selection decision.

If an Album creation, rename, delete, or membership change cannot persist, Slipstream must identify the affected Album and action. It must retain a recoverable current source and must not present the failed change as complete.

If the current Photo's Album membership cannot load, Slipstream must keep the prior or empty membership presentation truthful, identify the failure beside the membership panel, and offer a retry. The failure must not affect selection, Rating, navigation, or Preview behavior.

If a selection or Rating change cannot persist, Slipstream must identify the affected action. It must not silently advance as if the decision were saved.

A disconnected browser may continue displaying already loaded thumbnails and Previews. An already loaded Preview must remain available for local Detail Review zoom and pan while disconnected, but the browser must stop accepting new decisions until both the connection and the current Photo state are confirmed. Fit-mode decision gestures and persisted controls remain unavailable while disconnected. Success from an unrelated request, such as another File Location range, does not confirm the current Photo state or re-enable decisions. Reconnect must refresh only the current source window and affected state; it must not require a full-Library transfer.

Slipstream must not present the Library Browser as connected while the server is unreachable. The browser must probe server reachability while the Photographer is idle, and it must report a lost connection within a bounded delay and without a Photographer action. A probe that cannot reach the server must mark the browser disconnected and withhold new decisions.

A probe that receives a usable status answer may confirm the connection again, because that probe is the designated reachability signal. It does not confirm the current Photo state: only the recovery that owns an affected operation confirms that state, so a confirmed connection must not clear an unfinished recovery or release a decision that still waits for its own confirmation. A status answer that arrives without usable content reports neither outcome: the server is reachable, and the condition belongs to the server rather than to connectivity.

If an ephemeral server-side browse snapshot expires or is lost after server restart, Slipstream must reopen the current source from the latest published Library and identify that newly completed scans may affect its order. A successful replacement snapshot also replaces browser-local Album membership facts from the retired snapshot; a failed reopen retains the recoverable current view.

## Examples

A Library contains 36,997 Photos. Slipstream displays the Library count and first Grid window without transferring all 36,997 Photo facts. Scrolling loads later windows while the source order remains stable.

The Photographer opens the Album `26春节`, activates its fourth Grid cell, and later returns to the source list. Reopening that Album returns to the saved Photo when it remains available.

The filesystem also contains the Original Folder `RAW/26春节`. Opening it shows Photos from that Folder and its descendants in Capture Time order. Its matching name does not connect it to the Album or change Album membership.

A rescan discovers 100 new Photos while the Photographer is viewing the Library. The open Grid does not insert them or move existing cells. A completion notice offers refresh; reopening `All Photos` includes the new Photos in Capture Time order.

The Photographer opens Photo 100. Slipstream prepares its review Preview first, then prepares Photos 101 and 99 with lower priority. Moving to Photo 101 normally reuses the completed cache entry.

The Photographer drags a Photo to the right. A selected indicator grows with the drag. The Photographer releases after the commit threshold. Slipstream records `selected` and advances to the next Photo.

A Library contains `shoot/A.JPG` captured at `2026:01:01 10:00:00` and `shoot/Z.JPG` captured at `2026:01:01 09:00:00`. Grid and Photo navigation show `Z`, then `A`, even though the filenames sort in the opposite order.

An Album explicitly contains `shoot/A.JPG` at position 0 and `shoot/Z.JPG` at position 1, while `Z` has the earlier Capture Time. That Album shows `A`, then `Z`. `All Photos` and the `shoot` Original Folder show `Z`, then `A`.
