# Browser Navigation and Responsive Surfaces

Addressable Grid and Photo destinations use the existing source, Photo, and Browse Snapshot owners.

History metadata stays bounded independently of Library size.

[Library Browser Experience](spec.md) owns the observable layout and navigation rules.

[Web Async Ownership](../../web-application/async-ownership.md) owns transport, settlement, recovery, and stale-result policy.

This design connects those contracts without changing the server's HTTP API.

## Model and Ownership

A destination is a validated source reference, view order, filter conditions, and optional stable Photo ID.

Absence of a Photo ID means Grid.

It is not a Browse token, Photo array, or saved mutation.

A browser entry has an opaque page-generated identifier and bounded restoration metadata.

A Grid anchor contains a stable Photo ID, an index hint, and a CSS-pixel offset relative to its row.

A focus target is a Photo ID or the Grid itself.

A Photo entry MAY identify the immediately preceding Grid entry that opened it.

That relationship is evidence for in-app return, not permission to infer history from its length or the referrer.

A page-local navigation owner owns URL decoding/encoding, the current browser entry, pending destination identity, and history transitions.

It reports a destination intent to the page controller.

It issues no HTTP calls and owns no Photo facts or mutable source projection.

The controller coordinates existing source/Grid and Photo owners and reports a committed or failed transition.

The source/Grid owner remains authoritative for source identity, options, Snapshot readiness, range loading, and anchor lookup.

The Photo owner remains authoritative for Photo commitment and mutation settlement.

The UI owns geometry, focus, panels, and pointer state.

Panels emit existing semantic intents and never become another API client or persistence coordinator.

## Browser Address Contract

Addresses use the existing application path `/` and query parameters.

The server continues to serve the same application document, so copied addresses and refresh do not require a new path fallback or a deployment route change.

The codec uses URL and URLSearchParams, with decoding performed exactly once.

The recognized parameters are:

- `source`: `library`, `folder`, or `album`; omission means `library`;
- `folderPath`: required for `folder`, including an empty value for the
  Library Folder; forbidden for other sources;
- `albumId`: required for `album` and forbidden for other sources;
- `photoId`: optional stable Photo identity; omission means Grid;
- `order`: `capture-time-asc` or `capture-time-desc`, with `album-order` also
  allowed for Albums; omission uses that source's default;
- `selection`: `all`, `unflagged`, `picked`, or `rejected`; omission means
  `all`;
- `rating-min` and `rating-max`: inclusive integers from 0 through 5; omission
  applies no Rating bound;
- `kind`: `raw` or `jpeg`; omission applies no Original kind condition;
- `available`: `true` or `false`; omission applies no availability condition;
- `captured-from` and `captured-before`: camera-local
  `YYYY-MM-DDTHH:MM:SS`; omission applies no Capture Time bound.

IDs and Folder Locations MUST satisfy the existing API validators.

A Location MUST remain relative and component-valid; an encoded slash is a separator after the single decode, not an escape from containment.

Absolute Locations, `.` or `..` components, duplicate recognized parameters, empty required IDs, an empty value for an optional parameter, and source-incompatible options are invalid.

An optional filter parameter is additionally invalid when its value fails its own fact validation: an out-of-range or inverted Rating range, an unknown `kind` or `available` value, or a malformed or reversed Capture Time bound.

An encoded reserved character inside a valid Location MUST round-trip without double decoding.

Unknown query keys are ignored and removed on canonicalization; startup canonicalization performs that same single replacement of the current URL.

Known invalid values produce an invalid-link explanation and the All Photos Grid, with one replacement of the current URL.

This occurs before any request for the invalid source.

The encoder omits defaults and emits recognized parameters in the order above.

It MUST NOT place filenames, absolute paths, transient publication tokens, arbitrary return URLs, or UI panels in an address.

Addresses define browser destinations, not new HTTP endpoints.

Wire values remain those of [Scalable Library Browsing](../design.md#source-opening).

## History Mechanism

Use the History API with one page-local navigation owner.

The application MUST NOT ship parallel navigation implementations for the same behavior.

Initialize the current valid document entry with replaceState; never push a duplicate entry on startup.

A same-document traversal is observed through one popstate subscription, removed on disposal.

Navigation controls MUST use real same-origin anchors where a destination is known.

The handler intercepts only an unmodified primary activation.

New-tab, copy-link, download, and external-link behavior remain native.

Buttons remain appropriate for decisions, panels, and operations whose destination requires resolution, such as Album Resume.

The browser owns the stack.

The application MUST NOT mirror it with an unbounded in-memory history array.

State under a versioned `slipstream` namespace stores only the entry identifier, optional Grid anchor/focus, optional known parent Grid relationship, and optional Folder publication provenance.

Merge that namespace without overwriting other history.state fields.

Treat absent, malformed, or unknown-version state as a direct entry.

Store no Photo facts, thumbnails, tokens, request objects, pending writes, multi-selection, or Undo descriptions in history.state.

No new localStorage or server persistence is required.

Each entry has constant-size metadata; there is no per-visited-Photo cache outside existing bounded owners.

Before leaving Grid, capture its anchor and focus from the UI and replace only its entry metadata.

A Photo opened from that Grid receives its own entry and the parent relationship.

Subsequent same-source Photo navigation preserves that relationship while replacing the Photo URL.

In-app return traverses one entry only when the current Photo has this known parent; otherwise it replaces the Photo entry with its source Grid.

A direct link MUST NOT call history.back based solely on history.length or a same-origin referrer.

Source selection creates one Grid entry.

Resume from another source first commits that source's Grid entry and then resolves and opens its saved Photo; Resume from its current Grid adds only the Photo entry.

A plain Grid open starts at index zero unless a restoration anchor is being restored; the initial saved position in the existing Browse response is used for Resume, not as an instruction to switch every Album open into Photo View.

With no available saved position, keep the Grid and explain that Resume is unavailable.

An empty Album MUST NOT create an artificial Photo entry.

## Destination Establishment

The navigation owner serializes presentation through one latest destination identity.

A newer intent or traversal invalidates older continuations.

It passes no mutation through the route decoder.

A source selection or applied view change MAY establish its address and a truthful pending Grid shell before the source is ready.

Retained content MUST be identified as previous content and unavailable for decisions; it MUST NOT appear to satisfy the requested source or filter.

A failed establishment keeps that requested destination retryable.

Opening a Photo from Grid, stepping to a neighbor, and decision-driven advancement commit their Photo address only after the Photo owner commits the corresponding bounded facts.

Failed steps create no new Photo address and keep the existing Photo Retry target.

Preview-byte completion is not a prerequisite for addressing a Photo whose bounded facts are ready.

Persistence-driven advancement keeps its existing save-before-advance requirement.

Direct entry and browser traversal have already selected a URL.

They MUST render a destination shell while resolving it.

They MUST NOT undo traversal with history.go, overwrite it with the old Photo, or admit a mutation against previously visible content.

A transport failure leaves the target URL and Retry plus a source-return action.

Only confirmed invalid or missing targets use the product's explained fallback and replace that current entry.

Startup MUST elect exactly one source bootstrap from the committed Overview.

The initial destination replaces the unconditional All Photos bootstrap; it MUST NOT race a second source open.

An unpublished Library waits for the existing publication workflow before attempting that destination.

A same-source Grid/Photo traversal reuses the live Browse Snapshot when its source, filter, and order match.

This preserves frozen membership after a decision.

No Snapshot is retained merely because its source has an older browser entry.

Once the source is left, its ordinary release rules apply.

When a new Snapshot is necessary, open it with the selected source/options and the preferred Photo ID where applicable.

Resolve the target using the existing bounded position endpoint.

A preferred Photo is a hint to source opening, so a fallback position returned by open is not proof that the requested Photo exists.

Confirm the stable ID before presenting it.

A null position produces the specified source-Grid fallback, not another Photo silently shown under the missing Photo's URL.

For Folder links, a direct entry binds to the current Published Library and opens only a valid current relative Location.

A history entry from a prior Folder publication MUST NOT silently reinterpret that Location: if its provenance differs, show the changed-Folder explanation and require an explicit Open current Folder action after current publication binding.

That action replaces the entry's provenance; it does not add a history loop.

A missing current Folder returns to All Photos only after the bounded server answer confirms it is absent.

Neither a Folder tree download nor scanning client-side windows to discover a Photo is permitted.

## Restoration and Persistence

Set browser scroll restoration to manual while the mounted Library Browser owns its internal Grid scroller, and restore the previous setting on disposal.

The UI captures a top-row Photo anchor and its offset, not a window scrollY.

After source establishment, resolve that Photo against the active Snapshot, admit only the bounded window covering its row, and restore geometry after the current size step determines column count.

Then restore cell focus with preventScroll, or focus the Grid when that cell no longer exists.

An index hint is usable only after a stable-ID check in the same Snapshot.

If the anchor is absent, clamp the prior index hint to the new source and use that bounded position; an empty source uses its empty-state focus target.

If state is absent or invalid, start at the source's first Grid row.

Restoration MUST finish once for the current destination and MUST NOT continually fight subsequent user scrolling.

Lookup failure is retryable, not evidence that the Photo disappeared.

A destination change supersedes only the read/presentation scopes it actually leaves.

A Grid/Photo change does not gratuitously reopen the source or discard valid same-source Undo.

A source change or reload keeps existing Undo and multi-selection invalidation rules.

Traversal never reconstructs those states from browser history.

Admitted Photo, Album, saved-position, and recovery writes retain the exact settlement policies of Web Async Ownership.

A route change MAY detach their presentation but MUST NOT abort them as a navigation shortcut.

Late responses cannot repaint the new destination or enqueue advancement.

History restores no prior mutation intent.

A newly committed Album Photo MAY admit its ordinary current saved-position write once; restoring a Grid never does so.

Application teardown MUST remove browser listeners and cancel navigation continuations while preserving the existing release and admitted-write settlement behavior.

For pagehide/pageshow with the browser back-forward cache, the app MUST resume or remount exactly one usable Library Browser and revalidate its destination.

It MUST NOT leave an inert restored document or duplicate subscriptions.

A full page return is not assumed to be popstate.

## Responsive Presentation Boundary

Use a single semantic action binding for each product intent.

A control MAY move or be rendered for a different layout, but hidden variants MUST be unavailable to accessibility, focus, input, and image-loading paths.

There MUST NOT be duplicate mutation admission or independent mobile Rating/Undo state.

Use native dialog.showModal for modal surfaces.

One page UI controller owns which surface is active, its invoker, its local subview, and draft values.

It does not own a stack of routes or server state. A content transition within Photo tools replaces the modal content and restores a meaningful focus target.

The modal owner classifies close requests under the [experience contract](spec.md#supporting-surfaces). Contextual Album form cancellation returns locally; whole-surface dismissal uses one cleanup and outer-focus restoration path.

Handle native cancel before the dialog closes when it means local return. History traversal always dismisses the whole surface before destination rendering.

Use edge focus handling if needed to keep Tab contained, and destroy pending gesture candidates before opening. The UI MUST NOT add dummy history to emulate native close requests.

The existing Chromium support contract applies; do not add a CloseWatcher polyfill or a second back-stack implementation for unsupported engines.

A supporting surface MAY grow to a bounded fraction of dynamic viewport height and scroll internally. The primary Grid/Photo layout uses dynamic viewport units, minmax/overflow constraints, and safe-area padding.

Normal state region budgets come from the Product Spec. Larger text and error content MUST reflow rather than rely on fixed heights that hide controls.

Loaded Preview inspection remains local during disconnection. Opening a panel MUST NOT change Photo readiness; readiness still derives from existing owners.

Closed Nearby Photos releases strip thumbnails and admits no windows. Opening it uses the same bounded neighbor facts and five-entry limit as the wide strip.

### Filter Drafts and Summaries

View options owns one draft of the committed filter, order, and size. Reset filters changes only that draft's filter; direct zero-result Clear filters is an applied-view intent to the existing source/navigation owner.

Counts and header summaries project committed choices, never unsent drafts. The compact summary counts condition groups from that same filter; it has no parallel query model.

Immediate Browse or Source actions discard the draft and capture the committed scope before leaving options. No URL parameter or endpoint is added.

### Contextual Album Creation and Choice

The single modal owner retains a bounded local return context: its initiating surface and focus target, source/Photo generation, and, for a batch chooser, the existing at-most-100 selected stable IDs. It is not browser history or a new Album store.

New Album replaces the chooser or membership content locally. Create uses the existing Album owner and form identity; successful creation returns only while that initiating context remains current. Membership is a later explicit action, never a continuation automatically sent after create.

Closing or changing destination detaches the local return context, not the admitted write. Late creation can update shared Album facts through existing settlement but cannot reopen a surface or restore selection.

At batch-add admission, the existing Album owner captures the chosen Album and eligible selected IDs. A selector or late response MUST NOT change that captured target.

Known missing Photos remain in the result/count under the batch contract but are excluded from a new request.

Unknown creation is reconciled through Album inspection, not an automatic create retry or inferred membership.

## Design Rationale

Native URL and history calls with one bounded page navigation owner preserve source and Photo ownership.

Explicit commit timing preserves the current Photo when a next-Photo read fails; query addresses keep server and deployment routing unchanged.

Destinations and temporary surfaces have separate lifetimes: putting panels or every Photo step in history would impede continuous review and reopen incidental surfaces on Forward.
