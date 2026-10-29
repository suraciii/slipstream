# Library Browser Experience

Photographers need room to inspect Photos and a predictable way to return to
where they were. A narrow screen must not become a stack of every desktop
control. Browser Back must follow destinations inside the Library Browser
before it leaves the site.

This document owns screen composition, observable disclosure behavior, and
browser navigation; the shared modal lifecycle mechanism is owned by
[Browser Navigation](design.md#responsive-presentation-boundary).
[Library Browsing and Selection](../spec.md) owns
Photo actions, source order, progressive loading, gestures, persistence,
failures, and Undo. Moving a control must preserve those rules.

```text diagram
Library Browser
├── Grid: Sources · View options (incl. Album Resume) · Select mode
├── Photo: Back · Preview · position/state badge · Pick / Reject / Undo · Rating · More
│   └── Touch accelerators in Fit: ← Reject · → Pick · ↑ Next · ↓ Previous
└── One supporting surface: Sources, options, Rating, tools, or recovery
```

## Screen Structure

The Library Browser has two views: Grid View and Photo View. Sources, View
options, Rating choices, and Photo tools are supporting surfaces, not new
pages or new sources.

Use **Pick** for the keep decision, **Reject** for the reject decision, and
**Unflagged** for no decision. **Select** is reserved for choosing Photos or
sources; **Select mode** names the bounded multi-selection. These terms are
defined in [CONTEXT.md](../../../CONTEXT.md).

The interface must use one neutral dark appearance. Color communicates focus,
Selection State, Rating, connectivity, or failure. State must also have text,
an icon, or another non-color indication. Required text must meet WCAG AA
contrast. There must be exactly one application-level main landmark. The
Access entry screen renders inside that landmark and must not create a nested
main landmark. The current Grid or Photo view supplies the single `h1` inside
it, as defined by [Instance Access](../../access/spec.md#browser-entry).

A narrow layout applies at widths up to 760 CSS pixels. Photo View also uses
compact controls at heights up to 480 CSS pixels. Layout must follow available
space rather than identifying a device by its user agent.

A wide Grid must keep resizable source navigation beside the Photos. A narrow
Grid must expose Sources through its current-source title and a disclosure
indicator. Its accessible name must identify both Sources and the current
source. Opening Sources must not discard the current Grid or Photo context.
There must be no dedicated brand or connected-status row on narrow screens.

Source navigation must retain separate All Photos, Folders, and Albums
sections, their counts, and Library status. The read-only root must retain the
visible name Library Folder. Long names must truncate without widening the
screen and remain available to assistive technology; a pointer hover must
reveal the complete name. Selecting a source closes Sources.

A valid Album name must not widen Grid View or Photo View beyond the Library
Browser at a supported viewport. Its visible current-source title may be
visually truncated, but assistive technology must retain the complete Album
name. Source rows must read as navigation rather than a collection of
promotional cards.

## Grid View

The normal header must contain the source title, a compact count or current
loading status, View options, and Select mode. The Photo Grid fills the
remaining area. Routine progress must not create another toolbar row. The
Library refresh action is a recovery action and must not occupy a
permanently prominent primary slot.

View options must separate staged choices from immediate actions:

- **Filter:** Selection State first, then Rating, Original kind, Original
  availability, and Capture Time, with Reset filters. Advanced conditions may
  use a named disclosure; active advanced conditions must be exposed on open.
- **Order and display:** source order and Small, Medium, or Large thumbnail size.
- **Browse actions:** Open a random Photo and Album Resume when available.
- **Source actions:** Add Folder for an Original Folder, Refresh Current Source,
  the current Rejected result's removal review, and the Library-wide Trash entry.

Committed matching count and complete-source decision counts must stay distinct
from each other and from draft choices. Conditions, exact Capture Time input,
Reset filters, and zero-result recovery follow [Source-Scoped Library
Filters](../filters/spec.md#applying-and-reading-a-filter).

The header must show the result count and identify an active filter and
non-default order. Wide layouts must name each active condition in text. Narrow
layouts must use a compact **Filters: N** summary rather than a scrollable
condition strip. Count the five condition groups: Selection State, Rating,
kind, availability, and Capture Time. Either or both bounds of one range count
as one group; defaults count as zero. Order and thumbnail size are not filters.
Non-default order must have a separate text indication, **Custom order**.

The summary belongs to the View options entry, not a second filter button or
header row. Its accessible description must name every committed condition
with its values and the actual non-default order. Opening View options exposes
those values as readable, keyboard-operable controls; no hover or tiny nested
scroll area may be the only complete-condition path. With no active filter,
the count summary may be omitted.

View options and Select mode must retain their complete visible labels. The
source name and result count share one Sources disclosure: only a long source
name may truncate, with its full name available to assistive technology and
on pointer hover. No Grid control may be clipped or relabeled at 320, 360,
390, 420, or 760 CSS pixels wide. The normal header retains its space budget.

On a wide Grid, **Open a random Photo** must appear in the normal header's
secondary action group without creating another toolbar row. On a narrow Grid,
it must appear in View options under a named Browse actions group and must not
add a permanent header row. Its accessible name must identify that it opens a
random Photo from the current source and filter.

The action is enabled only when the current Browse Snapshot is ready and has a
nonzero result count, and while no source or Photo transition is pending. While
it resolves, the button must expose a pending state and must not admit a second
attempt. Grid cells must present as unavailable until that attempt settles;
they must not appear actionable when opening them would be refused. A successful
range Retry must clear the failed status while keeping the Grid, not reopen the
unresolved Photo automatically. The action's placement and pending presentation
follow [Random Photo](../spec.md#random-photo); it must not change the current
Grid into a different source or filter.

Complete progress remains available in View options with the existing
server-authoritative count rules. A consequential Library check completion or
failure must remain discoverable under Status and Recovery while options are
closed.

Trash and the current Rejected result's removal review belong in View options, beside source management actions. They must not add a permanent row to the narrow Grid header. Trash remains reachable regardless of the active source or filter. Removal review must retain its current-source and reviewed-result scope.

Changing View options uses an explicit Apply action. Closing without Apply
must discard uncommitted changes. Applying only a thumbnail-size change must
not reopen the source. Applying filter and order together must open one view
with both choices and the existing identity-anchor rules.

Browse and Source actions must use the committed source, filter, and order,
never unsubmitted choices. Their activation discards the options draft before
opening the next surface or admitting the action; it must not implicitly Apply.
When a draft differs, the action group must say that it uses the current view
and discards unapplied choices. Returning to options starts from committed
values. Removal confirmation remains separate and retains its reviewed scope.

Grid cells must retain complete, unstretched composition, a position number,
filename, Selection State, Rating, and independent availability facts. The
uniform cell geometry, placeholders, keyboard behavior, and bounded loading
contract remain in [Grid Composition and Orientation](../spec.md#grid-composition-and-orientation).
An unavailable or failed Thumbnail must use an explicit text placeholder in the image region, retaining the Photo's facts and interaction. A failed image delivery must not leave a browser broken-image icon visible.
At every width, complete rows must divide the available Grid width evenly,
leaving no more than the ordinary inter-cell gap at the trailing edge. An
incomplete last row keeps the complete-row cell width and leading alignment;
it must not stretch its few Photos. The selected size step and Medium default
remain unchanged; filling a row never crops or stretches its images.

## Multi-Selection

Select mode must replace the normal header controls with the source name,
`N / 100 selected`, and Done. Done is the visible clear exit: it empties the
multi-selection and leaves Select mode without changing a Photo decision.
Escape has the same effect only while the Grid owns it. A supporting surface
handles its own Escape first. Source changes and reopening retain their clear
behavior.

The selection tray occupies a bottom action region with Pick, Reject, and Add
to Album. It appears only while the Grid multi-selection is nonempty. Desktop
modifier selection must show this same selection surface whenever the
multi-selection is nonempty.

Add to Album must open one named chooser instead of an always-visible Album
selector. Opening or cancelling it writes nothing. The chooser identifies the
selected count and source, requires an explicit target Album choice, and names
that Album before **Add to Album** confirms membership. It must not silently
submit a default Album. The existing New Album form is available even when no
Albums exist; creation returns to the chooser without adding Photos.

The chooser and creation form share the one supporting surface and preserve
the captured selection through local transitions. Cancelling the form returns
to the chooser; closing the chooser returns focus to Add to Album without
clearing selection. A source change invalidates an unsubmitted chooser. At
submission, capture the eligible stable Photo IDs and target Album ID once;
later UI changes must not retarget an admitted write.

A completed batch must retain the selection and the visible indication
Selection remains active. Partial, uncertain, changed-elsewhere, and missing
outcomes must follow the existing batch contract. A compact persistent result
must expose result details and applicable Review N or Remove added Photos
actions. Detailed results may use a bounded disclosure, but must not be
replaced by an expiring toast or an unexplained success icon. Review N closes
that disclosure and focuses the affected Grid cells; it does not open another
modal. The existing single live-region announcement rule remains in force.

The next action, the persistent result, and compensation must be visually
distinct. Normal compact outcomes and first-level actions must fit the
Select-mode region budget; detailed outcomes may expand explicitly. Closing a
chooser or result disclosure must not cancel admitted work, erase uncertainty,
or claim success. Compensation expires only under its existing owning rules;
a persistent outcome does not promise indefinite compensation availability.

## Photo View

Photo View must contain a compact source-return and filename row, the Preview,
and a primary action region. It must communicate current position, Selection
State, and Rating without requiring Photo tools to be open. The primary region
must present one Reject action, one Rating entry with an explicit current value
including zero, one Pick action, one Undo action, and More.

Previous and Next must remain visible in a separate navigation group with the
position and total. They must not share an activation target with Pick or
Reject. Portrait layouts may place navigation immediately above the main
actions; short landscape must consolidate them without shrinking touch
targets. There must be only one visible Previous/Next pair.

Photo View is the continuous review surface. The Photographer must be able to
review the current Photo and move to the previous or next Photo without
opening a supporting surface, relying on a gesture, or memorizing a keyboard
shortcut.

One composition satisfies that review at every supported viewport. The context
row, one navigation group, and one review dock stay together inside the safe
viewport at 375 by 667, 390 by 844, and the named short-landscape viewports.
Selection State is presented with the Photo identity in that context row and
must not occupy its own forced row. Short landscape consolidates the layout by
moving content, never by shrinking a 44 by 44 CSS pixel target below that size.
Pick, Reject, Rating, the gestures, zoom, history, and their writes keep their
existing behavior in every layout.

Undo must remain visible in the primary action region. Its availability follows
the Photo action owner; when no undoable change exists, the control may be
disabled but must expose that fact without implying that a decision was
reverted.

On touch input in Fit Preview, horizontal gestures are decision accelerators:
right Picks and left Rejects, then advances to the next Photo after the
accepted decision. Vertical gestures are navigation accelerators: up opens
the next Photo and down opens the previous Photo. They never change Selection
State, Rating, Album membership, saved Album position, or Undo. The mapping is
fixed across source kinds and Selection State filters. The visible Previous,
Next, Pick, and Reject controls remain the equivalent non-gesture paths.

Manual Zoom gives one-finger panning priority over all four swipe actions.
Vertical movement that does not commit navigation must retain the natural
Photo View scroll path, including scrolling from Preview letterbox regions.
The Preview's direction feedback follows the [gesture lifecycle](../spec.md#touch-gestures).
It must name the candidate action and must not rely on color, haptics, or
animation alone. Static gesture instructions belong in Help, not on the image.

More opens the supporting action groups:

- **Review:** Clear flag with its existing availability rule, and Rating using
  the same choice surface as the primary Rating entry;
- **Photo:** Edit, Album membership, Capture Details, Preview Source, Preview
  Zoom, and Nearby Photos;
- **Library:** Sources, View options, and Review unavailable Originals; and
- **Help:** a named disclosure for the combined Preview detail-limit explanation
  and gesture guidance.

The combined Preview detail evidence must be programmatically available beside
the Preview without opening the Help disclosure. Opening Help may repeat the
explanation and add gesture guidance, but Help must not be the only way to
read it.

The Preview must fit its complete composition. Empty space caused by image
aspect ratio is acceptable. Closed supporting groups must occupy no space and
expose no focusable controls. The bounded neighbor strip remains available
through Photo > Nearby Photos, following the existing lifecycle and source
order. Rating saves keep the current Photo open.

Rating opens a focused set of explicit 0–5 choices, rather than exposing every
Photo tool. Only the entry or its active choice surface owns explicit Rating
interaction at one time. Rating saves keep the current Photo open. The Rating
Wheel, swipe decisions, pinch, pan, keyboard shortcuts, and their persistence
rules remain unchanged.

Photo View does not expose **Open a random Photo** in the first product. Its
navigation group remains the single visible Previous/Next pair, and the More
surface does not create a second random-navigation path. Random selection
starts from Grid View so the source, committed order, and Selection State
filter remain explicit context.

Details and membership may load when requested. Their loading or failure must
remain local to that surface. Closing a surface while a write settles must not
claim cancellation or success; the existing operation owner determines where
its outcome may appear.

## Supporting Surfaces

Sources, View options, Rating, Photo tools, Album forms, and recovery review
must have an explicit name and Close or Cancel action. A compact supporting
surface opens from the bottom and scrolls internally when necessary. A wide
surface may be a centered dialog or an appropriate source drawer.

At most one modal surface may be active. Moving from Photo tools to Details,
Albums, Zoom, or Nearby Photos replaces its content and provides a local
return to Photo tools. That local return is not a browser history entry.
Each active subview must have one visible primary title naming the current
task and one clear local return action. Shell and body must not repeat that
title as adjacent headings. Distinct subsection headings and programmatic
group names remain available; reducing repetition must not remove labels.
Leaving an Album form with an unsubmitted draft discards it after an explicit
cancel or close. Metadata drafts follow the [metadata editing contract](../../photo-library/metadata/spec.md#human-and-agent-access).
An admitted operation still follows its settlement rules.

Opening a modal must cancel a pending Rating Wheel and move focus into the
surface. Background content must not receive pointer input, keyboard focus,
or Photo shortcuts. Tab and Shift+Tab remain within the modal. At its root,
Close, Escape, scrim activation, and a supported platform close request dismiss
the surface and return focus to its invoker or nearest valid control.

In contextual New Album, Cancel, Escape, and a supported platform close request
discard the unsubmitted name and return locally to the initiating membership
panel or batch chooser, focusing New Album. They must not clear Grid selection.
The shell's Close and scrim activation dismiss the entire supporting workflow
and restore its outer invoker. A standalone Album form has no local parent;
all close paths dismiss it. These transitions never cancel an admitted write,
claim success, or hide a consequential failure's recovery path.

Temporary panels must not add browser history entries. Android system Back
may issue a platform close request; browser toolbar Back and browser history
swipes navigate through history. A history traversal must close temporary
surfaces and render its destination. Forward must not reopen an old tools
panel. These mechanisms must not be described as universally equivalent.

Album creation remains available in Sources and in the contextual membership
or batch chooser under [Album Management](../spec.md#album-management).
Each Album must have a visible, keyboard-accessible action entry for rename
and delete on touch screens; actions must not depend on hover. Photo membership
management retains true per-Album checkboxes, including removal. Original
Folder Add Folder remains an explicit confirmable action distinct from Add to
Album for selected Photos. Recovery retains its proposal and explicit apply.

Album creation and rename failures must appear inside the active form with
the submitted name retained. Delete Album confirmation must name the Album
and explain that Photos and Original Files remain unchanged. An unknown
membership read must not show editable unchecked boxes as confirmed absence;
Retry remains available. Folder expansion and paging must retain keyboard
focus on the corresponding control after the source list changes.

## Destinations and Browser History

A destination consists of a source, its order and filter, and either its Grid
or one Photo identified independently of its position. It must be represented
by a same-site URL. Opening a link in a new tab, bookmarking it, or refreshing
it must resolve that destination against current Library facts.

The source title must show the source resolved for that URL. The browser must
not label a retained Photo from another destination as its current content.
A destination URL is a reference, not an archival snapshot or a shared grant
of access. Sharing remains subject to the deployment's reachability boundary.

Navigation must obey these rules:

- A bare application URL opens the All Photos Grid with default options.
- Choosing a source creates a Grid destination with that source's default
  order and All filter. It never implicitly replaces the Grid with Photo View.
- An Album with a saved position exposes Resume separately. Resume resolves
  that position under the existing saved-position rules and opens Photo View.
  From another source it establishes the Album Grid before opening the Photo,
  so returning from the Photo has a meaningful source destination.
- Opening a Photo from Grid creates one new destination. Repeated activation
  of the current destination must not create duplicate history entries.
- Previous, Next, neighbor-strip navigation, decision-driven advancement, and
  Undo-driven Photo return replace the current Photo destination after their
  existing readiness and persistence prerequisites succeed.
- Applying filter or order changes replaces the current destination. The URL
  preserves the committed choices across reload and direct entry. Changing
  source normally uses defaults; traversing history restores that entry's
  choices. Thumbnail size remains page-local under its existing contract.
- Back from Photo View returns to the prior Grid context. Forward reopens the
  last Photo represented by that Photo entry.
- The in-app source-return action may traverse only a known matching Grid
  entry. A directly loaded Photo with no such entry must open its source Grid
  without risking navigation to another site.
- At the original application entry, browser Back must retain its ordinary
  ability to leave the site. The application must not trap the browser with
  dummy entries or an unconditional back handler.

Selection State, Rating, membership, scans, recovery apply, zoom, panel
visibility, and multi-selection must not become browser history. Traversal
must not replay a decision or an old mutation request. A Photo that becomes
current in an Album still follows the normal saved-position contract; restoring
a Grid alone must never write an Album position.

## Returning to a Grid

A Grid history entry must restore its source, filter, order, top visible Photo
and offset, and keyboard focus when those facts remain meaningful. A different
screen size must restore by Photo identity using the current cell geometry.
It must not depend on retaining every previously displayed cell or loading the
whole source. The existing bounded position lookup must suffice.

Within one live source, Grid and Photo share the current fixed source order.
Returning to its Grid must not refilter after every decision. After an entry
can no longer reuse its source, it resolves against the latest published view
and identifies that the view was refreshed. Missing anchors fall back to the
nearest valid position, or the empty source state. Browser reload preserves
the destination; exact Grid placement after a full reload is best effort and
is not a new durable All Photos or Folder resume feature.

## Status and Recovery

A normal state must keep status compact. Initial Library loading, source
opening, and absent thumbnails use truthful labels and stable placeholders.
Preview loading uses the Preview region and must not expose another Photo's
controls. No total or completion percentage may be invented.

An empty Library, empty Album, no filter matches, unavailable Photo, unavailable
Original, and failed request must remain distinguishable. Their existing
recovery actions must remain reachable without exposing every management
control during ordinary review.

A blocking connection or decision failure must be visible beside the affected
primary action, not only inside a closed panel. A Library-level failure or
completed Library check with an outstanding action must expose one compact status entry
from either view. Opening it reveals the current owning notice and action;
background progress must not erase it. Opening the compact status entry must
not discard the current Photo, its zoom state, or the Grid anchor. Expanding
detailed recovery uses a scrolling surface rather than an unlimited header
stack.

A removed Album or Folder destination must return to All Photos with an
explanation. A Photo no longer in the requested source or current filter must
return to that source's Grid with an explanation and preserve the requested
filter. An unavailable Original whose Photo still exists must keep its Photo
and remembered facts available. A temporary transport failure is not proof
that any source or Photo was removed.

A failed in-app next-Photo load retains the prior committed Photo, address,
position, and Photo Retry target. A direct load or history traversal has
already chosen a destination: if that load fails, keep its address and show a
retryable destination state and a source-return action. Do not silently replace
it with the previous destination or programmatically reverse browser history.

## Accessibility and Space Acceptance

Every interactive target must provide at least 44 by 44 CSS pixels on narrow
and short layouts. Safe-area insets must add protection around controls rather
than shrink their targets. Keyboard focus must remain visible, including after
virtualized Grid restoration, a panel close, or a re-render. Reduced motion
must not remove state feedback or require an animation to complete an action.

At default text size with zero safe-area insets and no exceptional notice:

- at 375 by 667 and 390 by 844, the Grid header must occupy at most 88 pixels;
- in Select mode, the header and action tray together must occupy at most
  168 pixels, leaving at least 499 pixels for the Grid at 375 by 667;
- Photo View must reserve at least 480 pixels for the Preview at 375 by 667
  and 650 pixels at 390 by 844; and
- at 667 by 375 and 844 by 390, Photo View must reserve at least 240 pixels
  for the Preview.

These are region budgets, not image cropping requirements. They prevent
control accumulation from passing solely because no element overflows.
Explicitly expanded panels and exceptional notices may exceed normal-state
budgets, but must retain a reachable close/recovery action. With text enlarged
to 200%, content may reflow vertically and need not meet those pixel budgets;
controls and required facts must remain reachable without page-level
horizontal overflow.

A Preview minimum must not push the primary review controls below the safe
viewport. The budgets rank the Preview above supporting content, not above the
controls that operate it. When a Preview minimum and the primary review
controls cannot both hold at a supported viewport, the primary review controls
and Preview inspection remain and supporting content yields first.

Desktop verification must cover 1280 by 800; touch-tablet verification must
cover 1024 by 768. Rotation must preserve source, Photo, committed decisions,
and applicable focus while recomputing layout and Fit. Browser support remains
owned by the [0.1 Support and Release Contract](../../deployment/support.md#supported-scope).

## Examples

The Photographer opens an Album's Grid, scrolls to Photo 400, opens it, and
views another 20 Photos. Back restores the Album Grid at Photo 400. Forward
opens the last Photo viewed. None of those history operations reverses a
selection decision.

A copied Photo link opens without prior history. Its source-return action
opens the source Grid. The browser's own Back retains the ability to leave
the site.

The Photographer opens Rating and closes it without selecting a value. The
Photo address and history are unchanged. Opening tools and using browser Back
closes the tools and follows the prior destination; a supported native close
request instead closes only the tools.

The Photographer opens the Album `26春节`, which has a saved position. Its
Grid opens with Resume available. Resume opens the saved Photo, and Back
returns to the Album Grid.

On a narrow Grid, a Picked filter with both Rating bounds, RAW kind, unavailable
Originals, and both Capture Time bounds shows Filters: 5. A custom order is
identified separately. Keyboard activation of View options exposes every
value without enlarging the closed header or adding a second filter surface.

The Photographer changes a draft filter and opens Trash from Source actions.
The draft is discarded, the committed source remains unchanged, and Trash
still covers the entire Library. Returning to options does not revive the draft.

The Photographer opens Add to Album for selected Photos, creates an Album,
and returns to the chooser. Nothing joins the Album until explicit addition.
Closing after addition was sent leaves that write settling for its captured
IDs and Album; switching source cannot transfer its result to a new selection.
