# Library Browser Experience

Photographers need room to inspect Photos and a predictable way to return to
where they were. A narrow screen must not become a stack of every desktop
control. Browser Back must follow destinations inside the Library Browser
before it leaves the site.

This document owns screen composition, disclosure, and browser navigation.
[Library Browsing and Selection](library-browsing-and-selection.md) owns
Photo actions, source order, progressive loading, gestures, persistence,
failures, and Undo. Moving a control must preserve those rules.

```text diagram
Library Browser
├── Grid: Sources · View options · Select mode
│   └── Select mode: count · Done / Select · Reject · Add to Album
├── Photo: source return / Preview / Previous · position · Next
│   └── Quick Action Dock: Reject · Rating · Select · More
└── One supporting surface: Sources, options, Rating, tools, or recovery
```

## Screen Structure

The Library Browser has two views: Grid View and Photo View. Sources, View
options, Rating choices, and Photo tools are supporting surfaces, not new
pages or new sources.

The interface must use one neutral dark appearance. Color communicates focus,
Selection State, Rating, connectivity, or failure. State must also have text,
an icon, or another non-color indication. Required text must meet WCAG AA
contrast. There must be exactly one main landmark.

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

## Grid View

The normal header must contain the source title, a compact count or current
loading status, View options, and Select mode. The Photo Grid fills the
remaining area. Routine progress must not create another toolbar row.

View options must group these existing controls:

- Selection State filter;
- source order;
- Small, Medium, and Large thumbnail size;
- complete source decision counts, distinct from the filtered result count;
- source-specific actions, including Add Folder for an Original Folder; and
- Refresh Current Source when it is available.

The header must indicate an active nondefault filter or order and the visible
result count. Complete progress remains available in View options with the
existing server-authoritative count rules. A consequential scan completion or
failure must remain discoverable under Status and Recovery, even while the
options surface is closed.

Changing View options uses an explicit Apply action. Closing without Apply
must discard uncommitted changes. Applying only a thumbnail-size change must
not reopen the source. Applying filter and order together must open one view
with both choices and the existing identity-anchor rules.

Grid cells must retain complete, unstretched composition, a position number,
filename, Selection State, Rating, and independent availability facts. The
uniform cell geometry, placeholders, keyboard behavior, and bounded loading
contract remain in [Grid Composition and Orientation](library-browsing-and-selection.md#grid-composition-and-orientation).
At narrow widths, complete rows must divide the available Grid width evenly,
leaving no more than the ordinary inter-cell gap at the trailing edge.

## Multi-Selection

Select mode must replace the normal header controls with the source name,
`N / 100 Photos`, and Done. Done is the visible clear exit: it empties the
multi-selection and leaves Select mode without changing a Photo decision.
Escape has the same effect. Source changes and source reopening retain their
existing clear behavior.

The selection tray must occupy a bottom action region with Select, Reject,
and Add to Album. It remains visible with zero selected Photos, when batch
actions are unavailable. The normal View controls must not remain stacked
above the tray. Desktop modifier selection must show this same selection
surface whenever the multi-selection is nonempty.

A completed batch must retain the selection and the visible indication
Selection remains active. Partial, uncertain, changed-elsewhere, and missing
outcomes must follow the existing batch contract. A compact persistent result
must expose result details and applicable Review N or Remove added Photos
actions. Detailed results may use a bounded disclosure, but must not be
replaced by an expiring toast or an unexplained success icon. Review N closes
that disclosure and focuses the affected Grid cells; it does not open another
modal. The existing single live-region announcement rule remains in force.

## Photo View

Photo View must contain a compact source-return and filename row, the Preview,
and a primary action region. It must communicate current position, Selection
State, and Rating without requiring Photo tools to be open. The primary
region must present one Reject action, one Rating entry with an explicit
current value including zero, one Select action, and More.

Previous and Next must remain visible in a separate navigation group with the
position and total. They must not share an activation target with Select or
Reject. Portrait layouts may place navigation immediately above the main
actions; short landscape must consolidate them without shrinking touch
targets. There must be only one visible Previous/Next pair.

The Preview must fit its complete composition. Empty space caused by image
aspect ratio is acceptable. Persistent zoom controls, capture facts, and Album
management must not cover the mobile Preview or consume closed-panel space.
A manual zoom must expose its current percentage and a direct Fit return.

Wide Photo View may show its bounded neighbor strip. Narrow and short layouts
must move that strip behind a Nearby Photos entry in Photo tools. This is the
same bounded navigation surface, not a second timeline. Hiding it must release
its image demand under the existing filmstrip lifecycle. The explicit
Previous/Next pair remains available when the strip is closed.

The mobile Quick Action Dock consists of Reject, Rating, Select, and More.
Its closed Secondary Sheet must occupy no space and expose no focusable
controls. More opens these supporting actions:

- Clear and Undo, with their existing availability rules;
- Photo Album membership and management;
- Capture Details, Preview Source, and the combined detail-limit explanation;
- the complete Preview Zoom controls; and
- Nearby Photos and Sources.

Rating opens a focused set of explicit 0–5 choices, rather than exposing every
Photo tool. Only the entry or its active choice surface owns explicit Rating
interaction at one time. Rating saves keep the current Photo open. The Rating
Wheel, swipe decisions, pinch, pan, keyboard shortcuts, and their persistence
rules remain unchanged.

Details and membership may load when requested. Their loading or failure must
remain local to that surface. Closing a surface while a write settles must
not claim cancellation or success; the existing operation owner determines
where its outcome may appear.

## Supporting Surfaces

Sources, View options, Rating, Photo tools, Album forms, and recovery review
must have an explicit name and Close or Cancel action. A compact supporting
surface opens from the bottom and scrolls internally when necessary. A wide
surface may be a centered dialog or an appropriate source drawer.

At most one modal surface may be active. Moving from Photo tools to Details,
Albums, Zoom, or Nearby Photos replaces its content and provides a local
return to Photo tools. That local return is not a browser history entry.
Leaving a form with an unsubmitted draft discards the draft after an explicit
cancel or close; an admitted operation still follows its settlement rules.

Opening a modal must cancel a pending Rating Wheel and move focus into the
surface. Background content must not receive pointer input, keyboard focus,
or Photo shortcuts. Tab and Shift+Tab remain within the modal. Close, Escape,
a scrim activation, or a supported platform close request dismisses it and
returns focus to its invoker, or the nearest valid control if that invoker no
longer exists. An already admitted operation must not be cancelled by closing
its form. A consequential failure must remain recoverable after closing.

Temporary panels must not add browser history entries. Android system Back
may issue a platform close request; browser toolbar Back and browser history
swipes navigate through history. A history traversal must close temporary
surfaces and render its destination. Forward must not reopen an old tools
panel. These mechanisms must not be described as universally equivalent.

Album creation remains available in Sources. Each Album must have a visible,
keyboard-accessible action entry for rename and delete on touch screens;
actions must not depend on hover. Photo membership management retains its
true per-Album checkboxes, including removal. Original Folder Add Folder
remains an explicit confirmable action distinct from Add to Album for selected
Photos. Recovery review retains its proposal and explicit apply workflow.

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
completed scan with an outstanding action must expose one compact status entry
from either view. Opening it reveals the current owning notice and action;
background progress must not erase it. Expanding detailed recovery uses a
scrolling surface rather than an unlimited header stack.

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

Desktop verification must cover 1280 by 800; touch-tablet verification must
cover 1024 by 768. Rotation must preserve source, Photo, committed decisions,
and applicable focus while recomputing layout and Fit. Browser support remains
owned by the [0.1 Support and Release Contract](0.1-support-and-release.md#supported-scope).

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
