# Library Browser Experience

This contract owns responsive composition, disclosure, and browser navigation.

| Dependency | Responsibility |
| --- | --- |
| [Library Browsing and Selection](../spec.md) | Photo actions, ordering, loading, gestures, persistence, failure, and Undo. |
| [Browser Navigation](design.md) | Navigation and modal lifecycle boundaries. |
| [Product Design Language](../../design-language/spec.md) | Shared visual and accessibility rules. |

## Screen Structure

The interface MUST use one neutral dark appearance; shared presentation rules follow the [Product Design Language](../../design-language/spec.md#visual-language).

The Library Browser has Grid and Photo views. Sources, View options, Rating, and Photo tools are supporting surfaces, not new pages or sources.

A narrow layout applies at widths up to 760 CSS pixels. Photo View also uses compact controls at heights up to 480 CSS pixels. Layout MUST follow available space, not user-agent device identification.

A wide Grid MUST keep resizable source navigation beside Photos. A narrow Grid MUST expose Sources through its current-source title and disclosure indicator, with an accessible name identifying both Sources and the current source.

Opening Sources MUST preserve the Grid or Photo context. Narrow screens MUST NOT add a dedicated brand or connected-status row.

Source navigation MUST retain separate All Photos, Folders, and Albums sections, their counts, and Library status. The read-only root MUST be visibly named Library Folder; selecting a source closes Sources.

Long source names MUST truncate without widening the screen while remaining available to assistive technology and on pointer hover. This also applies to valid Album names in Grid and Photo views.

Source rows MUST read as navigation rather than promotional cards.

The application MUST have exactly one application-level main landmark. The Access entry screen MUST render inside it without creating a nested main landmark; the current Grid or Photo view supplies its single `h1`.


## Grid View

The normal header MUST contain the source title, compact count or loading status, View options, and Select mode. The Photo Grid fills the remaining area; routine progress MUST NOT add a toolbar row.

Refresh is a recovery action and MUST NOT occupy a permanently prominent primary slot.

View options MUST separate staged choices from immediate actions:

- **Filter:** Selection State first, then Rating, Original kind, Original availability, and Capture Time, with Reset filters. Advanced conditions MAY use a named disclosure; active advanced conditions MUST be exposed on open.
- **Order and display:** source order and Small, Medium, or Large thumbnail size.
- **Browse actions:** Open a random Photo and Album Resume when available.
- **Source actions:** Add Folder for an Original Folder, Refresh Current Source, the current Rejected result's removal review, and the Library-wide Trash entry.

Trash MUST remain reachable regardless of source or filter. Removal review retains its current-source and reviewed-result scope; neither adds a permanent narrow header row.

View options MUST expose complete-source decision counts separately from the committed matching count and draft choices. Conditions, Capture Time input, Reset filters, and zero-result recovery follow [Source-Scoped Library Filters](../filters/spec.md#applying-and-reading-a-filter).

The header MUST show the result count and identify an active filter and non-default order. Wide layouts MUST name each active condition in text.

Narrow layouts MUST use a compact **Filters: N** summary instead of a scrollable condition strip. The count includes these five condition groups:

- Selection State;
- Rating;
- Original kind;
- Original availability; and
- Capture Time.

Either or both bounds of one range count as one group; defaults count as zero. Order and thumbnail size are not filters; non-default order MUST have a separate text indication, **Custom order**.

The summary belongs to the View options entry, not a second filter button or header row. Its accessible description MUST name every committed condition with its values and the actual non-default order.

Opening View options exposes those values as readable, keyboard-operable controls; neither hover nor a tiny nested scroll area MAY be the only complete-condition path. With no active filter, the count summary MAY be omitted.

View options and Select mode MUST retain their complete labels at every supported width. No Grid control MAY be clipped or relabeled as an acceptable presentation, including at 320, 360, 390, 420, and 760 CSS pixels.

The source title and result count share one disclosure. Only a long source name MAY be visually truncated; its complete name MUST remain available to assistive technology and on pointer hover.

Complete source progress remains available in View options under the server-authoritative count rules.

A consequential Library Check completion or failure MUST remain discoverable under Status and Recovery when View options is closed.

On wide Grid, **Open a random Photo** MUST appear in the normal header's secondary action group without another row. On narrow Grid it MUST appear under a named Browse actions group in View options, without a permanent header row.

Its accessible name MUST identify that it opens a random Photo from the current source and filter. [Random Photo](../spec.md#random-photo) owns selection, readiness, and navigation behavior.

The action MUST be enabled only when the current Browse Snapshot is ready and nonempty, with no source or Photo transition pending.

While a random attempt resolves, the control MUST expose pending state and refuse a second attempt. Grid cells MUST appear unavailable until it settles; a successful range Retry clears failure while keeping Grid and MUST NOT automatically reopen the unresolved Photo.

View options use explicit Apply. Closing without Apply discards drafts; applying only thumbnail size MUST NOT reopen the source. Applying filter and order together opens one view with both choices and the existing identity-anchor rules.

Browse and Source actions MUST use the committed source, filter, and order, never unsubmitted choices. Activation discards the options draft before opening the next surface or admitting the action; it MUST NOT implicitly Apply.

When a draft differs, the action group MUST say that it uses the current view and discards unapplied choices. Returning to options starts from committed values; removal confirmation remains separate and retains its reviewed scope.

[Grid Composition and Orientation](../spec.md#grid-composition-and-orientation) owns cell geometry, composition, keyboard behavior, and bounded loading. A cell retains its position, filename, Selection State, Rating, and independent availability facts.

An unavailable or failed Thumbnail MUST show an explicit text placeholder in the image region while retaining Photo facts and interaction. A failed image delivery MUST NOT leave a broken-image icon visible.

At every width, complete rows MUST divide the available Grid width evenly, leaving no more than the ordinary inter-cell gap at the trailing edge. An incomplete last row keeps the complete-row cell width and leading alignment and MUST NOT stretch its Photos.

The selected size step and Medium default remain unchanged; filling a row MUST NOT crop or stretch images.

## Multi-Selection

Select mode MUST replace normal header controls with the source name, `N / 100 selected`, and Done. Done clears the multi-selection and leaves Select mode without changing a Photo decision.

Escape has the same effect only while Grid owns it; a supporting surface handles its own Escape first. Source changes and reopening retain their existing clear behavior.

A bottom selection tray with Pick, Reject, and Add to Album MUST appear only while Grid multi-selection is nonempty. Desktop modifier selection MUST show the same surface when nonempty.

Add to Album MUST open one named chooser instead of an always-visible Album selector. Opening or cancelling it writes nothing.

The chooser identifies the selected count and source, requires an explicit target Album choice, and names that Album before **Add to Album** confirms membership. It MUST NOT silently submit a default Album.

The existing New Album form is available even when no Albums exist; creation returns to the chooser without adding Photos. The chooser and creation form share one supporting surface and preserve the captured selection through local transitions.

Cancelling the form returns to the chooser; closing the chooser returns focus to Add to Album without clearing selection. A source change invalidates an unsubmitted chooser.

At submission, capture the eligible stable Photo IDs and target Album ID once; later UI changes MUST NOT retarget an admitted write.

A completed batch MUST retain selection and visibly indicate Selection remains active. Partial, uncertain, changed-elsewhere, and missing outcomes follow the [batch contract](../spec.md#grid-multi-selection-and-batch-actions).

A compact persistent result MUST expose details and applicable Review N or Remove added Photos actions. Details MAY use a bounded disclosure, but MUST NOT be replaced by an expiring toast or unexplained success icon.

Review N closes the disclosure and focuses affected Grid cells without opening another modal. The existing single live-region announcement rule remains in force.

The next action, persistent result, and compensation MUST be visually distinct. Normal compact outcomes and first-level actions MUST fit the Select-mode region budget; detailed outcomes MAY expand explicitly.

Closing a chooser or result disclosure MUST NOT cancel admitted work, erase uncertainty, or claim success. Compensation expires only under its existing owning rules; a persistent outcome does not promise indefinite compensation availability.

## Photo View

Photo View MUST contain a compact source-return and filename row, Preview, and primary action region. Position, Selection State, and Rating MUST be understandable without opening Photo tools.

The primary region MUST present one Reject action, one Rating entry with its explicit current value including zero, one Pick action, one Undo action, and More.

Previous and Next MUST remain visible in one separate navigation group with position and total. They MUST NOT share an activation target with Pick or Reject. Portrait layouts MAY place navigation immediately above the main actions; short landscape MUST consolidate without shrinking touch targets.

The Photographer MUST be able to review and navigate without a supporting surface, gesture, or memorized shortcut. The context row, navigation group, and review dock MUST remain together inside the safe viewport at 375 by 667, 390 by 844, and the specified short-landscape viewports.

Selection State MUST be presented with Photo identity in the context row, not in a forced separate row. Short landscape MUST move content rather than reduce a 44 by 44 CSS pixel target.

Undo MUST remain visible. When no undoable change exists it MAY be disabled, but MUST expose that state without implying a reversal.

[Touch Gestures](../spec.md#touch-gestures) owns the Fit swipe mapping, zoom priority, and decision and navigation effects. Manual Zoom gives one-finger panning priority over all four swipe actions.

Vertical movement that does not commit navigation MUST retain the natural Photo View scroll path, including Preview letterbox regions. Direction feedback MUST name the candidate action; static gesture instructions belong in Help, not on the image.

The Preview MUST show transient direction feedback during a swipe. Feedback MUST NOT rely on color, haptics, or animation alone.

More opens these supporting action groups:

- **Review:** Clear flag with its existing availability rule, and Rating using the same choice surface as the primary Rating entry;
- **Photo:** Edit, Album membership, Capture Details, Preview Source, Preview Zoom, and Nearby Photos;
- **Library:** Sources, View options, and Review unavailable Originals; and
- **Help:** a named disclosure for the combined Preview detail-limit explanation
  and gesture guidance.

The combined Preview detail evidence MUST be programmatically available beside the Preview without opening Help. Help MAY repeat it and add gesture guidance.

The Preview MUST fit its complete composition; aspect-ratio empty space is acceptable. Closed supporting groups MUST occupy no space and expose no focusable controls.

Nearby Photos remains available through Photo > Nearby Photos under its existing bounded lifecycle and source order. Rating saves MUST keep the current Photo open.

Rating opens explicit 0–5 choices rather than every Photo tool. Only the entry or its active choice surface owns explicit Rating interaction at one time; the existing Rating Wheel and gesture rules remain unchanged.

Photo View MUST NOT expose **Open a random Photo**. Random selection starts in Grid View with source, committed order, and filter explicit.

Details and membership MAY load on request. Their loading or failure MUST remain local to that surface; closing a surface during a write MUST NOT claim cancellation or success.

## Supporting Surfaces

Sources, View options, Rating, Photo tools, Album forms, and recovery review MUST have an explicit name and Close or Cancel action. A compact surface opens from the bottom and scrolls internally as needed; a wide surface MAY be a centered dialog or appropriate source drawer.

At most one modal surface MAY be active. Moving from Photo tools to Details, Albums, Zoom, or Nearby Photos replaces its content and provides a local return to Photo tools, not a browser history entry.

Each active subview MUST have one visible primary title naming the current task and one clear local return action. Shell and body MUST NOT repeat that title as adjacent headings; distinct subsection headings and programmatic group names remain available.

Explicitly closing or cancelling an Album form discards an unsubmitted draft. Metadata drafts follow [metadata editing](../../photo-library/metadata/spec.md#human-and-agent-access); admitted operations retain their settlement rules.

In contextual New Album, Cancel, Escape, and a supported platform close request discard the unsubmitted name and return locally to the initiating membership panel or batch chooser, focusing New Album. They MUST NOT clear Grid selection.

The shell's Close and scrim activation dismiss the entire supporting workflow and restore its outer invoker. A standalone Album form has no local parent; all close paths dismiss it.

These transitions MUST NOT cancel an admitted write, claim success, or hide a consequential failure's recovery path.

Opening a modal MUST cancel a pending Rating Wheel and move focus inside. Background content MUST NOT receive pointer input, keyboard focus, or Photo shortcuts; Tab and Shift+Tab stay within the modal.

At the modal root, Close, Escape, scrim activation, or a supported platform close request dismisses the surface and returns focus to its invoker or nearest valid control. Closing MUST NOT cancel admitted operations; consequential failures remain recoverable afterward.

Temporary panels MUST NOT add browser history entries. Android system Back MAY issue a platform close request, while browser toolbar Back and history swipes traverse history.

History traversal MUST close temporary surfaces and render its destination. Forward MUST NOT reopen an old tools panel.

Album creation remains available in Sources and in contextual membership panels and batch choosers under [Album Management](../spec.md#album-management). Every Album MUST have a visible, keyboard-accessible rename and delete entry on touch screens, independent of hover.

Membership management retains per-Album checkboxes including removal. Original Folder Add Folder remains an explicit confirmable action distinct from adding selected Photos to an Album; recovery retains proposal and explicit Apply.

Album creation and rename failures MUST remain in the active form with the submitted name retained. Delete confirmation MUST name the Album and explain that Photos and Original Files remain unchanged.

An unknown membership read MUST NOT present editable unchecked boxes as confirmed absence; Retry MUST remain available. Folder expansion and paging MUST retain keyboard focus on the corresponding control after the source list changes.

## Destinations and Browser History

A [Destination](../../../CONTEXT.md#browsing-and-selection) MUST have a same-site URL. Bookmarking, opening in a new tab, or refreshing resolves it against current Library facts; it is not an archival snapshot or shared access grant.

The source title MUST identify the source resolved for the URL. Retained content from another destination MUST NOT be labeled current. Sharing remains subject to deployment reachability.

| Action | Destination and history effect |
| --- | --- |
| Bare application URL | Open All Photos Grid with default options. |
| Choose source | Create its Grid destination with default order and All filter; do not implicitly open a Photo. |
| Album Resume | Resolve the saved position separately from Grid open. From another source, establish the Album Grid before opening the Photo. |
| Open a Photo from Grid | Create one new destination; repeated activation of the current destination creates no duplicate entry. |
| Previous, Next, neighbor, decision advance, or Undo return | Replace the current Photo destination after existing readiness and persistence prerequisites succeed. |
| Apply filter or order | Replace the current destination and preserve committed choices in its URL. Thumbnail size remains page-local. |
| Back and Forward | Back from Photo returns to prior Grid context; Forward reopens the last Photo represented by that entry. |
| In-app source return | Traverse only a known matching Grid entry; a direct Photo entry opens its source Grid without risking another site. |

Source changes normally use defaults; history traversal restores the entry's choices. At the original application entry, browser Back MUST retain its ordinary ability to leave the site; dummy entries and an unconditional back handler MUST NOT trap it.

Selection State, Rating, membership, scans, recovery apply, zoom, panels, and multi-selection MUST NOT become browser history. Traversal MUST NOT replay decisions or old mutation requests.

A Photo becoming current in an Album follows the normal saved-position contract. Restoring a Grid alone MUST NOT write an Album position.

## Returning to a Grid

A Grid history entry MUST restore source, filter, order, top visible Photo and offset, and keyboard focus when meaningful. On a different screen size it MUST restore by Photo identity using current cell geometry and bounded position lookup, without retaining every cell or loading the whole source.

Grid and Photo share the live source's fixed order. Returning MUST NOT refilter after each decision.

An entry that can no longer reuse its source resolves against the latest published view and identifies that the view was refreshed. Missing anchors fall back to the nearest valid position or empty source state.

Reload preserves the destination; exact Grid placement after full reload is best effort, not a durable All Photos or Folder Resume feature.

## Status and Recovery

Initial Library loading, source opening, and absent thumbnails MUST use truthful labels and stable placeholders. Preview loading uses the Preview region and MUST NOT expose another Photo's controls; totals and completion percentages MUST NOT be invented.

Empty Library, empty Album, no filter matches, unavailable Photo, unavailable Original, and failed request MUST remain distinguishable. Their existing recovery actions MUST remain reachable.

A blocking connection or decision failure MUST be visible beside the affected primary action, not only inside a closed panel. A Library-level failure or completed Library Check with an outstanding action MUST expose one compact status entry from either view.

Opening status reveals the owning notice and action; background progress MUST NOT erase it. Opening status MUST preserve the current Photo, zoom, and Grid anchor. Detailed recovery uses a scrolling surface rather than an unlimited header stack.

A removed Album or Folder destination MUST return to All Photos with an explanation. A Photo absent from the requested source or filter MUST return to that source's Grid with an explanation and preserve the requested filter.

An unavailable Original whose Photo exists MUST retain its Photo and remembered facts. A transport failure is not proof of removal.

A failed in-app next-Photo load retains the prior committed Photo, address, position, and Photo Retry target. A failed direct load or traversal keeps its already-selected address and offers Retry and source return; it MUST NOT silently restore the prior destination or reverse browser history.

## Accessibility and Space Acceptance

Every interactive target MUST provide at least 44 by 44 CSS pixels on narrow and short layouts. Safe-area insets MUST protect controls without shrinking targets.

Keyboard focus MUST remain visible after Grid restoration, panel close, and re-render. Reduced motion MUST NOT remove state feedback or make an animation necessary to complete an action.

At default text size, zero safe-area insets, and no exceptional notice, normal-state region budgets are:

| Viewport in CSS pixels | Budget |
| --- | --- |
| 375 × 667 and 390 × 844 | Grid header at most 88 pixels. |
| Normal Select mode | Header and tray together at most 168 pixels; at 375 × 667, Grid retains at least 499 pixels. |
| 375 × 667 | Photo Preview at least 480 pixels. |
| 390 × 844 | Photo Preview at least 650 pixels. |
| 667 × 375 and 844 × 390 | Photo Preview at least 240 pixels. |

These are region budgets, not cropping requirements. Expanded panels and exceptional notices MAY exceed them but MUST keep close and recovery actions reachable.

At 200% text, content MAY reflow vertically without meeting pixel budgets. Controls and required facts MUST remain reachable without page-level horizontal overflow.

A Preview minimum MUST NOT push primary review controls below the safe viewport. If both cannot fit at a supported viewport, primary review controls and Preview inspection remain while supporting content yields first.

Rotation MUST preserve source, Photo, committed decisions, and applicable focus while recomputing layout and Fit. [Support and Release](../../deployment/support.md#supported-scope) owns browser support.
