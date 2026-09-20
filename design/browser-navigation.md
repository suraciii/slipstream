# Browser Navigation and Responsive Surfaces

The Library Browser needs addressable Grid and Photo destinations without
creating another owner of source data, Photo mutations, or Browse Snapshots.
History restoration must remain bounded even when the Library contains tens
of thousands of Photos. Responsive presentation must not reproduce every
control in a second mobile tree.

[Library Browser Experience](../docs/library-browser-experience.md) owns the
observable layout and navigation rules. [Web Async Ownership](web-async-ownership.md)
owns transport, settlement, recovery, and stale-result policy. This design
connects those contracts without changing the server's HTTP API.

## Model and Ownership

A destination is a validated source reference, view order, Selection State
filter, and optional stable Photo ID. Absence of a Photo ID means Grid. It is
not a Browse token, Photo array, or saved mutation.

A browser entry has an opaque page-generated identifier and bounded
restoration metadata. A Grid anchor contains a stable Photo ID, an index hint,
and a CSS-pixel offset relative to its row. A focus target is a Photo ID or the
Grid itself. A Photo entry may identify the immediately preceding Grid entry
that opened it. That relationship is evidence for in-app return, not permission
to infer history from its length or the referrer.

A page-local navigation owner owns URL decoding/encoding, the current browser
entry, pending destination identity, and history transitions. It reports a
destination intent to the page controller. It issues no HTTP calls and owns no
Photo facts or mutable source projection. The controller coordinates existing
source/Grid and Photo owners and reports a committed or failed transition.

The source/Grid owner remains authoritative for source identity, options,
Snapshot readiness, range loading, and anchor lookup. The Photo owner remains
authoritative for Photo commitment and mutation settlement. The UI owns
geometry, focus, panels, and pointer state. Panels emit existing semantic
intents and never become another API client or persistence coordinator.

## Browser Address Contract

Addresses use the existing application path `/` and query parameters. The
server continues to serve the same application document, so copied addresses
and refresh do not require a new path fallback or a deployment route change.
The codec uses URL and URLSearchParams, with decoding performed exactly once.

The recognized parameters are:

- `source`: `library`, `folder`, or `album`; omission means `library`;
- `folderPath`: required for `folder`, including an empty value for the
  Library Folder; forbidden for other sources;
- `albumId`: required for `album` and forbidden for other sources;
- `photoId`: optional stable Photo identity; omission means Grid;
- `order`: `capture-time-asc` or `capture-time-desc`, with `album-order` also
  allowed for Albums; omission uses that source's default; and
- `selection`: `all`, `undecided`, `selected`, or `rejected`; omission means
  `all`.

IDs and Folder Locations must satisfy the existing API validators. A Location
must remain relative and component-valid; an encoded slash is a separator
after the single decode, not an escape from containment. Absolute Locations,
`.` or `..` components, duplicate recognized parameters, empty required IDs,
an empty value for an optional parameter, and source-incompatible options
are invalid. An encoded reserved character
inside a valid Location must round-trip without double decoding.

Unknown query keys are ignored and removed on canonicalization; startup
canonicalization performs that same single replacement of the current URL.
Known invalid values produce an invalid-link explanation and the All Photos
Grid, with one replacement of the current URL. This occurs before any request
for the invalid source. The encoder omits defaults and emits recognized parameters
in the order above. It must not place filenames, absolute paths, transient
publication tokens, arbitrary return URLs, or UI panels in an address.

Examples use illustrative IDs that satisfy the identifier shape:

```text literal
/
/?source=folder&folderPath=
/?source=folder&folderPath=RAW%2F26-spring&selection=undecided
/?source=album&albumId=00000000-0000-4000-8000-000000000001
/?source=album&albumId=00000000-0000-4000-8000-000000000001&photoId=00000000-0000-4000-8000-000000000004&order=capture-time-desc
/?photoId=00000000-0000-4000-8000-000000000009
```

These examples define browser addresses, not new HTTP endpoints. Wire values
remain those of [Scalable Library Browsing](library-browsing.md#source-opening).
An implementation must test parsing, rejection, canonicalization, and round
trips from these examples.

## History Mechanism

Use the History API with one page-local navigation owner. Initialize the
current valid document entry with replaceState; never push a duplicate entry
on startup. A same-document traversal is observed through one popstate
subscription, removed on disposal.

Navigation controls must use real same-origin anchors where a destination is
known. The handler intercepts only an unmodified primary activation. New-tab,
copy-link, download, and external-link behavior remain native. Buttons remain
appropriate for decisions, panels, and operations whose destination requires
resolution, such as Album Resume.

The browser owns the stack. The application must not mirror it with an
unbounded in-memory history array. State under a versioned `slipstream`
namespace stores only the entry identifier, optional Grid anchor/focus,
optional known parent Grid relationship, and optional Folder publication
provenance. Merge that namespace without overwriting other history.state
fields. Treat absent, malformed, or unknown-version state as a direct entry.

Store no Photo facts, thumbnails, tokens, request objects, pending writes,
multi-selection, or Undo descriptions in history.state. No new localStorage
or server persistence is required. Each entry has constant-size metadata;
there is no per-visited-Photo cache outside existing bounded owners.

Before leaving Grid, capture its anchor and focus from the UI and replace
only its entry metadata. A Photo opened from that Grid receives its own entry
and the parent relationship. Subsequent same-source Photo navigation preserves
that relationship while replacing the Photo URL. In-app return traverses one
entry only when the current Photo has this known parent; otherwise it replaces
the Photo entry with its source Grid. A direct link must not call history.back
based solely on history.length or a same-origin referrer.

Source selection creates one Grid entry. Resume from another source first
commits that source's Grid entry and then resolves and opens its saved Photo;
Resume from its current Grid adds only the Photo entry. A plain Grid open
starts at index zero unless a restoration anchor is being restored; the initial
saved position in the existing Browse response is used for Resume, not as an
instruction to switch every Album open into Photo View. With no available
saved position, keep the Grid and explain that Resume is unavailable. An
empty Album must never create an artificial Photo entry.

## Destination Establishment

The navigation owner serializes presentation through one latest destination
identity. A newer intent or traversal invalidates older continuations. It
passes no mutation through the route decoder.

A source selection or applied view change may establish its address and a
truthful pending Grid shell before the source is ready. Retained content must
be identified as previous content and unavailable for decisions; it must not
appear to satisfy the requested source or filter. A failed establishment
keeps that requested destination retryable.

Opening a Photo from Grid, stepping to a neighbor, and decision-driven
advancement commit their Photo address only after the Photo owner commits the
corresponding bounded facts. Failed steps create no new Photo address and keep
the existing Photo Retry target. Preview-byte completion is not a prerequisite
for addressing a Photo whose bounded facts are ready. Persistence-driven
advancement keeps its existing save-before-advance requirement.

Direct entry and browser traversal have already selected a URL. They must
render a destination shell while resolving it. They must not undo traversal
with history.go, overwrite it with the old Photo, or admit a mutation against
previously visible content. A transport failure leaves the target URL and
Retry plus a source-return action. Only confirmed invalid or missing targets
use the product's explained fallback and replace that current entry.

Startup must elect exactly one source bootstrap from the committed Overview.
The initial destination replaces the unconditional All Photos bootstrap; it
must not race a second source open. An unpublished Library waits for the
existing publication workflow before attempting that destination.

A same-source Grid/Photo traversal reuses the live Browse Snapshot when its
source, filter, and order match. This preserves frozen membership after a
decision. No Snapshot is retained merely because its source has an older
browser entry. Once the source is left, its ordinary release rules apply.

When a new Snapshot is necessary, open it with the selected source/options
and the preferred Photo ID where applicable. Resolve the target using the
existing bounded position endpoint. A preferred Photo is a hint to source
opening, so a fallback position returned by open is not proof that the
requested Photo exists. Confirm the stable ID before presenting it. A null
position produces the specified source-Grid fallback, not another Photo
silently shown under the missing Photo's URL.

For Folder links, a direct entry binds to the current Published Library and
opens only a valid current relative Location. A history entry from a prior
Folder publication must not silently reinterpret that Location: if its
provenance differs, show the changed-Folder explanation and require an
explicit Open current Folder action after current publication binding. That
action replaces the entry's provenance; it does not add a history loop.
A missing current Folder returns to All Photos only after the bounded server
answer confirms it is absent. Neither a Folder tree download nor scanning
client-side windows to discover a Photo is permitted.

## Restoration and Persistence

Set browser scroll restoration to manual while the mounted Library Browser
owns its internal Grid scroller, and restore the previous setting on disposal.
The UI captures a top-row Photo anchor and its offset, not a window scrollY.
After source establishment, resolve that Photo against the active Snapshot,
admit only the bounded window covering its row, and restore geometry after
the current size step determines column count. Then restore cell focus with
preventScroll, or focus the Grid when that cell no longer exists.

An index hint is usable only after a stable-ID check in the same Snapshot.
If the anchor is absent, clamp the prior index hint to the new source and use
that bounded position; an empty source uses its empty-state focus target.
If state is absent or invalid, start at the source's first Grid row. Restoration
must finish once for the current destination and may not continually fight
subsequent user scrolling. Lookup failure is retryable, not evidence that the
Photo disappeared.

A destination change supersedes only the read/presentation scopes it actually
leaves. A Grid/Photo change does not gratuitously reopen the source or discard
valid same-source Undo. A source change or reload keeps existing Undo and
multi-selection invalidation rules. Traversal never reconstructs those states
from browser history.

Admitted Photo, Album, saved-position, and recovery writes retain the exact
settlement policies of Web Async Ownership. A route change may detach their
presentation but must not abort them as a navigation shortcut. Late responses
cannot repaint the new destination or enqueue advancement. History restores
no prior mutation intent. A newly committed Album Photo may admit its ordinary
current saved-position write once; restoring a Grid never does so. Test that
exception separately from the prohibition on replaying decisions.

Application teardown must remove browser listeners and cancel navigation
continuations while preserving the existing release and admitted-write
settlement behavior. For pagehide/pageshow with the browser back-forward
cache, the app must resume or remount exactly one usable Library Browser and
revalidate its destination. It must not leave an inert restored document or
duplicate subscriptions. A full page return is not assumed to be popstate.

## Responsive Presentation Boundary

Use a single semantic action binding for each product intent. A control may
move or be rendered for a different layout, but hidden variants must be
unavailable to accessibility, focus, input, and image-loading paths. There must
not be duplicate mutation admission or independent mobile Rating/Undo state.

Use native dialog.showModal for modal surfaces. One page UI controller owns
which surface is active, its invoker, its local subview, and draft values.
It does not own a stack of routes or server state. A content transition within
Photo tools replaces the modal content and restores a meaningful focus target.
Native close/cancel events, explicit Close, and scrim dismissal converge on
one cleanup path. Use explicit edge focus handling if needed to keep Tab in
the surface. Destroy pending gesture candidates before opening it.

Browser history traversal closes the current modal before destination
rendering. The UI must not add dummy history to emulate native close requests.
The existing Chromium support contract applies; do not add a CloseWatcher
polyfill or a second back-stack implementation for unsupported engines.

A supporting surface may grow to a bounded fraction of dynamic viewport
height and scroll internally. The primary Grid/Photo layout uses dynamic
viewport units, minmax/overflow constraints, and safe-area padding. Normal
state region budgets come from the Product Spec. Larger text and error
content must reflow rather than rely on fixed heights that hide controls.

Loaded Preview inspection remains local during disconnection. Opening a panel
must not change Photo readiness; readiness still derives from existing owners.
Closed Nearby Photos releases strip thumbnails and admits no windows. Opening
it uses the same bounded neighbor facts and five-entry limit as the wide strip.

## Options

### Selected: History API and a bounded page navigation owner

The application has one page slice, two view kinds, and existing source/Photo
owners. Native URL/history calls plus one codec fit that boundary without a
routing dependency or another store. Explicit commit timing preserves the
existing rule that a failed next-Photo read retains the prior current Photo.
Query addresses keep server and deployment routing unchanged.

### Not selected: Navigation API as a required runtime boundary

Navigation API offers centralized interception, entry identity, and transition
coordination. It became Baseline Newly available in January 2026, including
Firefox 147; it must not be dismissed as Chromium-only. It is a reasonable
future replacement for the browser adapter, but adopting it here adds a
runtime requirement without removing the source-readiness, virtual Grid
restoration, or admitted-write coordination this design must implement.
Do not ship two parallel navigation implementations to obtain the same behavior.

### Rejected: Framework router migration

React Router and Next.js demonstrate useful gallery semantics, but changing
rendering frameworks to obtain those semantics would broaden the migration
without improving Slipstream's ownership boundary. Hash routing also adds no
value: query addresses already refresh through the existing application path.

### Rejected: Put every panel and Photo step in history

This makes continuous culling require many Back actions and reopens incidental
panels on Forward. It also confuses platform close requests with browser
traversal. Destinations and transient surfaces have separate lifetimes.

### Rejected: Shrink or wrap the existing toolbar stack

Smaller controls violate touch targets. Wrapping every control can preserve
reachability while removing the Photo's usable space. Contextual action
regions and actual closed panels make the space contract testable.

## Verification

Focused automated coverage must derive from the product examples and prove:

- codec round trips, invalid/duplicate values, encoded Locations, defaults,
  canonicalization, and direct URL startup through the real server;
- source/Grid/Photo push and replace behavior, Back, Forward, reload, direct
  entry, known-parent return, duplicate activation, and leaving the site;
- one initial source bootstrap, delayed facts, rapid Back/Forward, stale
  success/failure, expired snapshots, removed targets, and Folder publication
  confirmation without an unbounded read;
- Grid anchor and focus restoration across resize and eviction in a generated
  Library of at least 40,000 Photos, with no retained historical Snapshots;
- no decision or membership replay, no aborted admitted write, no stale
  advancement, and the separate current Album saved-position rule;
- same-source frozen filter membership and Undo through view traversal;
- one modal, no hidden focus or duplicate live announcements, correct close
  focus after re-render, and no primary shortcuts behind modal content;
- region budgets, 44-pixel targets, safe areas, 200% text, reduced motion,
  rotation, desktop/touch input, all batch outcomes, and independent failures;
- external page navigation and browser return do not break mount/disposal;
- query deep links still pass the production host's existing health, static
  asset, and bounded browsing verification without a route migration; and
- the canonical repository gate passes after each implementation slice.

Physical Android Chromium evidence must identify device, browser version,
viewport, system Back versus toolbar traversal, keyboard/safe-area behavior,
and observed results. Emulated Pointer Events alone do not prove platform
close behavior. Other engines are exploratory until the support contract
explicitly expands.

## References

- [Next.js intercepting routes](https://nextjs.org/docs/app/api-reference/file-conventions/intercepting-routes): addressable gallery content and Back/Forward semantics.
- [React Router navigation](https://reactrouter.com/api/hooks/useNavigate): known-history traversal and replacement.
- [React Router scroll restoration](https://reactrouter.com/api/components/ScrollRestoration): restoration keyed by entry.
- [Navigation API](https://developer.mozilla.org/en-US/docs/Web/API/Navigation_API) and [January 2026 platform update](https://web.dev/blog/web-platform-01-2026): modern platform alternative.
- [CloseWatcher](https://developer.mozilla.org/en-US/docs/Web/API/CloseWatcher): native close requests versus navigation.
- [WAI-ARIA modal dialogs](https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/): focus and background interaction.
