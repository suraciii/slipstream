# Scalable Library Browsing

Slipstream's Web application must browse a Photo Library whose size exceeds what a mobile browser should download, parse, retain, or render as one response. The Photographer opens `All Photos`, one Original Folder, or one Album, browses its Grid, and opens individual Photos in the same Library Browser. The implementation needs a hidden stable-order boundary so a background rescan cannot move Photos underneath the user.

## Design Drivers

- The Library may contain tens of thousands of Photos and continue growing.
- The first useful screen must not depend on every Photo fact, Album member, or Original Folder.
- Grid and Photo navigation must preserve deterministic Library and Folder order or explicit Album order.
- A rescan may refresh Photo facts but must not reorder an already open source.
- Grid thumbnails and review Previews are rebuildable, persistent derivatives.
- The current Photo must not compete equally with speculative background work.
- The server remains one Rust modular monolith with one SQLite owner and one Photographer.
- Original Files remain descriptor-confined and read-only.
- The browser must expose truthful loading and scan progress without invented percentages.

## Model

### Library Overview

The Library Overview is a bounded summary of the current published Library. It contains:

- total Photo count;
- Album identifiers, names, counts, and saved-position availability;
- current scan state and progress summary; and
- whether a published Library is available.

It contains no complete Photo list, complete Album membership list, or Original Folder tree. Its response size is therefore independent of Library Photo count except for encoded counts and the number of Albums.

### File Location Window

A File Location Window is one bounded range of direct child Original Folders beneath a requested parent. Each Folder summary contains its relative Location, display name, recursive Photo count, and whether known descendant Folders exist.

The server derives Folder summaries from one Published Library according to [Physical File Locations and Virtual Albums](photo-organization.md). A request provides one validated relative parent Location, start position, bounded limit, and the opaque publication value from the first retained Folder window. The first request may omit that value and binds to the current publication. The server returns the publication, parent, requested start, total direct-child count, and only that range.

A newer publication expires the old value. The browser restarts File Location navigation instead of combining windows from different publications. No route returns the complete Folder tree or complete recursive Folder membership. The Library Overview therefore remains small even when directory count grows with the Library.

### Browse Snapshot

A Browse Snapshot is an internal, ephemeral server object. It is not a product or domain concept exposed to the Photographer.

It contains:

- one opaque token;
- source identity: `All Photos`, one Original Folder, or one Album;
- the view order and Selection State filter it was created with;
- an immutable ordered array of Photo IDs;
- total count;
- the source's per-state Selection counts when it was created;
- the Album's initial saved position when applicable; and
- last-access time for bounded cleanup.

Creating a Snapshot copies only ordered Photo IDs and bounded counts. It does not copy Photo facts, thumbnails, or Preview bytes. Current Photo facts are queried from the Library owner when a window is requested.

One process may retain only a bounded number of Snapshots. Explicit close, idle expiration, server restart, and bounded oldest-idle eviction may remove one. Losing a Snapshot never loses Selection State, Rating, Album membership, or saved position because those remain in SQLite.

### Browse Window

A Browse Window is a bounded consecutive range within one Browse Snapshot. Each item contains only the facts needed by Grid View and Photo View:

- position and Photo ID;
- availability of the Photo and its Original;
- Selection State and Rating;
- the single Original's kind;
- the ordering Original filename, because Grid View and Photo View identify a Photo by the Original File the Photographer decides on;
- current thumbnail and review-Preview facts; and
- derivative URLs only when current cache identities are known.

The ordering Original filename is a basename only. The relative Location,
absolute paths, and the Library Folder's absolute path never cross this
boundary.

A request must provide a start position and bounded limit. The server enforces a small maximum. No browser-facing route may use omission of the limit to mean the complete Library. Each Grid item's Photo summary carries one `original` fact with its kind and availability; the retired `ambiguous` and `originals` array fields must not appear in any current response.

### Photo Review Metadata

Photo View obtains a bounded, Photo-scoped metadata view through
`GET /api/photos/{id}/metadata`. The response contains only Capture Time,
Aperture, ISO, Shutter Speed, and Focal Length when the Photo's own
Original File provides them. Missing fields are omitted from the response and are rendered as `—` by
the browser. Reading metadata is read-only and failure does not make the Photo
unavailable or block review actions.

This metadata is intentionally loaded on demand instead of being added to
every bounded Grid window. The first product does not expose a general EXIF
tree, metadata editor, or unbounded metadata response.

### Photo Album Membership Query

Photo View obtains the Albums that contain one Photo through
`GET /api/photos/{id}/albums`. The response lists each containing Album's
identifier and name in Album-list order. The query is resolved server-side
from the Album membership tables; the browser must not derive membership by
traversing every Album's members. The response is bounded by the number of
Albums and contains no member lists. An unknown Photo identifier is a
distinct not-found failure. Reading membership is read-only; mutations
continue through the existing Album routes with their admitted-write
contracts.

The position lookup accepts one stable Photo ID through
`GET /api/browse/{token}/position?photoId={id}`. It returns that Photo's
position in the same immutable Browse Snapshot, or `null` when the Photo is
not in the source. It returns no Photo facts and has no unbounded form. An
expired or unknown Snapshot remains a distinct not-found failure.

### Published Library

The Published Library is the most recent complete scan committed by the Library owner. The browser may use it while an ordinary rescan builds a replacement. A root binding, schema, confinement, or state admission failure remains fail-closed and prevents service admission.

### Loading Status

Loading Status reports real phases and counts. It distinguishes:

- opening admitted persisted state;
- discovering supported Original Files;
- inspecting Capture Time facts;
- recovering relocated Original Files;
- applying a completed scan;
- enrolling content fingerprints;
- idle;
- failed with the prior Published Library retained; and
- initializing when no Published Library exists.

A phase may omit a total until that total is known. The protocol must not manufacture a percentage from elapsed time.

## Semantics

### Application Startup

Startup must first admit the Library Folder binding, SQLite state, cache layout, schema, and sidecar boundary. Admission failure remains a hard startup failure.

When a compatible Published Library exists, the server may bind HTTP and serve it before an ordinary full rescan completes. The rescan runs in the background and exposes Loading Status. Its successful result atomically replaces the Published Library for future Browse Snapshots.

When no Published Library exists, the server serves an initializing Library Overview while the first scan runs. The first Browse Snapshot cannot be created until that scan publishes a Library.

A Library Expansion retains its stricter offline contract. The expansion command must still complete its required post-commit scan before reporting success. Background startup does not weaken expansion admission or rollback behavior.

### Source Opening

The Web application loads the Library Overview first. It may concurrently request the first `All Photos` Browse Snapshot so Grid placeholders appear immediately, but source navigation must not wait for that request.

Conceptual protocol surfaces are:

```text literal
GET    /api/overview
GET    /api/status
POST   /api/scan
GET    /api/file-locations?publication={opaque}&parent={folder}&start={position}&limit={count}
POST   /api/browse
GET    /api/browse/{token}?start={position}&limit={count}
GET    /api/browse/{token}/position?photoId={id}
GET    /api/photos/{id}/albums
GET    /api/recovery/unavailable
POST   /api/recovery/propose
POST   /api/recovery/apply
DELETE /api/browse/{token}
```

Loading Status also reports the committed recovery result of the most recent completed scan as `lastRecovery {relocatedPhotos, fingerprintedOriginals, unavailablePhotos}` and the enrollment counters as `fingerprints {enrolled, pending}`. The scan phase `recovering` covers fingerprint comparison for candidate locations. The recovery routes serve the bounded manual recovery contract in [Photo Library and Albums](../docs/photo-library.md): `unavailable` lists remembered facts, `propose` returns inspectable mappings without writing, and `apply` commits one approved batch of relocations with per-mapping revalidation. A proposal carries a per-mapping outcome of `matched`, `content-mismatch`, `missing`, `kind-mismatch`, `unreadable`, `occupied`, or `colliding`, and marks the mapping verified only when a persisted fingerprint matched the candidate digest.

When a Published Library exists, both `GET /api/overview` and
`GET /api/status` include its opaque publication generation. The browser uses
that generation to revalidate an Overview immediately before committing
shared counts and Album summaries, so a response captured before replacement
cannot cross the publication boundary. An unpublished Library omits it.

`POST /api/scan` takes no semantic request fields; an empty JSON object is
accepted. A same-service request admits one application-owned Scan Cycle and
waits for its terminal Loading Status. Concurrent accepted requests join that
cycle within the bounded waiter capacity and each receives the one terminal
status captured by its leader; the browser suppresses a duplicate Retry
Library Check while its own command is in flight. Success is `200` with the
bounded Loading Status shape used by `GET /api/status` and no Photo facts.
`Origin` does not determine scan admission; an unavailable, saturated, or
failed scan is `5xx`. Wrong-method handling remains the shared `405`
protocol rule.

### Scan Cycle Ownership

The Application owns at most one Scan Cycle across background startup and
explicit `POST /api/scan` callers. The first admission starts an
application-owned leader task. Later admissions attach bounded waiters to that
task rather than invoking independent publication lifecycles. The leader runs
one physical Library scan, publishes its result exactly once, completes status
accounting exactly once, captures the resulting terminal Loading Status, and
fans that same result out to every remaining waiter.

Dropping an HTTP request removes only that request's waiter. It does not cancel
the admitted leader, skip publication, or leave status accounting elevated.
Application shutdown may drain or terminate the leader through the ordinary
server shutdown contract, but cleanup still completes status accounting.
After the leader reaches terminal state, the next admission may create a new
cycle. This single-flight boundary prevents one physical scan from producing
multiple publication generations.

`POST /api/browse` accepts one source:

```json
{ "source": "library" }
```

or:

```json
{ "source": "folder", "folderPath": "RAW/26-spring", "publication": "opaque" }
```

or:

```json
{ "source": "album", "albumId": "opaque-id" }
```

It optionally accepts one explicit view order. For `library` and `folder`
sources the values are `"capture-time-asc"` (the default when omitted) and
`"capture-time-desc"`. For `album` sources the values are `"album-order"`
(the default when omitted), `"capture-time-asc"`, and
`"capture-time-desc"`. An order value that is invalid for the source is
rejected before any Snapshot is created.

It optionally accepts one Selection State filter. The values are `"all"`
(the default when omitted), `"undecided"`, `"selected"`, and `"rejected"`. An
unknown value is rejected before any Snapshot is created. The filter is
applied once, server-side, to the complete ordered source before the Snapshot
is frozen, so a filtered Snapshot is one frozen bounded sequence with its own
total and positions. The source's per-state Selection counts describe the
unfiltered source order and are reported even for an unfiltered open.

It returns the opaque token, total count, initial position, per-state
Selection counts, and optionally one bounded first window. The exact JSON
belongs to the protocol compatibility fixtures; database rows and absolute
Original Locations do not cross this boundary.

The protocol has no route that materializes every Photo fact, every Album member, every Original Folder, or complete recursive Folder membership. The legacy unbounded complete-Photo and complete-membership routes remain retired. Album mutations return bounded Album summaries in the same shape as the Library Overview's Album list, never member lists. Legacy Photo Set routes and source values are retired rather than aliased. A triggered scan reports Loading Status and returns no Photo facts. Operator verification uses bounded traversal or an explicit offline state projection from the owned SQLite state rather than any production route that materializes every Photo fact.

### Order Ownership

An `All Photos` Browse Snapshot copies Photo IDs from the current Published Library's deterministic Capture Time order. An Original Folder Browse Snapshot filters that order by the recursive component-aware Folder rule. An Album Browse Snapshot copies IDs by persisted membership position.

The requested view order is applied once, server-side, to the complete
source before the Snapshot is frozen:

- `capture-time-desc` reverses only the Capture Time direction. Photos
  without a valid authoritative Capture Time stay in the trailing partition,
  and the ordering Location and Photo ID tie-breakers keep their existing
  direction, so a descending view is never produced by reversing a
  sequence that contains missing-time Photos.
- An Album time view orders members by the same Capture Time authority
  through the Published Library's Photo facts while leaving persisted
  membership positions untouched.

After creation, a Snapshot's ID order never changes. A rescan may change facts returned for those IDs, including availability and Preview state, but cannot insert, remove, or reorder them. Reopening the source creates a new Snapshot from the latest Published Library, and an explicit refresh reuses the currently selected order and Selection State filter.

### Selection Filtering

The Selection State filter is a view option of one open source. It is applied
after the view order has been resolved to the complete source order and before
the Snapshot is frozen, so it selects from that order and never rewrites it.
Filtering writes no persisted state: Album membership, Album member position,
Selection State, and Original Files are unaffected.

The Snapshot's per-state Selection counts are computed over the same complete
ordered source before the filter is applied. A filtered Snapshot therefore
reports the filtered total while its counts still describe the whole source,
which is what makes progress readable inside a filtered view. Both are plain
Library facts read at creation time: a Photo that the Published Library cannot
resolve is not counted, and no count is derived from requested windows.

Membership is frozen with the Snapshot order. A decision that changes a
Photo's Selection State does not add that Photo to, or remove it from, an open
Snapshot; the Photographer sees the change after reopening the source, which
applies the filter to the latest facts and re-anchors the current Photo by
identity. Positions, `GET /api/browse/{token}/position`, windows, and Previous
and Next navigation all resolve inside the frozen filtered sequence, so a
filtered view stays one bounded sequence with one meaning for every position.

The server resolves Album saved position when it creates the Snapshot. It applies the unavailable-member fallback defined by the Product Spec. The browser does not download all members to reproduce this rule. Durable saved position changes only when a Photo becomes current in Photo View and the position write is confirmed; Grid scrolling remains browser-local. Saved position and view order are independent: the position resolves by Photo identity whichever order the open view uses.

The saved position applies only to an open that supplies no explicit anchor. A view change (a new filter or order) anchors on the browser's current Photo by identity; when that Photo does not match the new view, the open starts at the view's first Photo rather than at the durable saved position.

Every source open requires the published Library. An Album open reads the same Published snapshot as `library` and `folder` sources for its anchor, filter membership, and counts, and `album-order` is no exception even though its order comes from persisted membership position. An Album open therefore fails with the not-published response before any Snapshot exists instead of failing later at its first window.

### Grid Loading

The source/Grid owner owns one range admission path. The visible range plus
a bounded buffer is reported as one unit; the owner computes the aligned
bounded windows still missing for that range, starts at most one in-flight
request per window, coalesces every concurrent demand for the same window
onto that request, and settles exactly one completion notification per
completed window no matter how many consumers joined. Rendering is
presentational: it never initiates loading and never subscribes per cell.

The rendered Grid must use virtualization or an equivalent bounded-DOM
mechanism. The number of rendered cells must be proportional to the viewport
and look-ahead, not the total source size. Grid DOM updates merge to at most
one per animation frame, and each update reuses the nodes of Photos that
stay visible: only entering, leaving, or changed cells are touched.

Retained Photo facts are capped at a bound that covers the actual viewport
and buffer at supported large viewports. Eviction is anchored to the latest
reported visible range and protects that range and its buffer; a late
response for an older position must never evict current-viewport data. A
window request captured before a source change must not commit into the new
source; correctness never depends on cancellation succeeding.

Duplicate or overlapping window requests share server work through the
single in-flight coalescing above; a general response cache is not required.
SQLite queries must run through the existing owner boundary and must
preserve request order.

### Current Photo Facts

Browse Snapshot order and Photo facts have separate owners:

```text literal
Browse Snapshot -> stable ordered Photo IDs
Library owner    -> current Photo facts
Preview service  -> current derivative facts and bytes
```

A Selection State or Rating mutation updates SQLite through the existing transaction boundary. The affected loaded window is updated locally after confirmation. A conflict or reconnect refreshes only the affected Photo or current bounded window.

Preview completion must become visible to subsequent window or Photo queries immediately. A stale process-wide scan snapshot must not remain the only source of Preview state after the Preview service has persisted newer facts.

### Batch Selection Writes

Grid multi-selection applies one Selection State to many Photos through
`POST /api/photos/state`. The request carries one bounded item per selected
Photo and one `selectionState` value:

```json
{
  "selectionState": "selected",
  "photos": [{ "photoId": "photo-1", "expectedCurrent": "undecided" }]
}
```

The server accepts at most 100 unique Photo identifiers and rejects an empty,
duplicate, over-limit, malformed, incomplete, unknown-field, or invalid-state
request before any write, as an invalid request rather than a conflict. The
browser mirrors that bound, so a batch it builds is always one the server can
admit. `selectionState` is exactly `selected` or `rejected`, and every item
has exactly `photoId` and `expectedCurrent`, with `expectedCurrent` exactly
`undecided`, `selected`, or `rejected`. Batch Rating is not part of this
contract, and the route carries no source, Album, or position: the browser
names the Photos it multi-selected.

The operation is one transaction with per-Photo outcomes, so one unsuccessful
Photo never rolls back confirmed Photos. The response reports exactly one
outcome per requested Photo:

- `applied` with the Selection State the Photo held before the write;
- `changedElsewhere` with the Photo's current Selection State when its current
  value differs from `expectedCurrent`; and
- `missing` when the current Library no longer holds the Photo.

A `changedElsewhere` or `missing` Photo is not written. On the batch
settlement, the browser moves loaded Photo facts and source decision counts
only for `applied` outcomes. During `Review N`, it reconciles each refreshed
Photo's contribution from the belief already held by the source counts to the
observed fact, so a retry cannot move a count twice. A Photo whose write was
not confirmed is never presented as decided. The browser keeps unsuccessful
Photos selected for review or retry. A missing
Photo remains a selected, non-retryable result until clear or source change;
its ID is excluded from later requests. The browser does not invent a fact for
a missing Photo. `Review N` refreshes the current facts for N changed Photos
and replaces their expected states before a retry. Before treating a refreshed
Photo as missing, the browser resolves its identity against the current Browse
Snapshot position; bounded local fact eviction is retryable cache pressure, not
proof that the Photo left the Library.

The response is valid only when its three arrays contain exactly one
non-overlapping outcome for every requested Photo and each outcome has the
fields defined above. A missing array, duplicate or invented ID, obsolete
`conflicts` field, or extra outcome field makes the whole response malformed.
A malformed response moves no facts, counts, or Undo entries; the selection
remains retryable and the Grid reports a failed batch.

Batch Undo reuses the single-Photo compare-and-set write: the browser sends
one bounded write per confirmed Photo, each naming the prior value the server
reported and the batch's value as `expectedCurrent`. One Photo that changed
does not block the others, and the browser retires only the Photos whose Undo
cannot apply. A batch Undo route was rejected: it would add a second
compare-and-set surface for a workflow the existing route already expresses,
and the per-Photo conflict truthfulness is identical either way.

Batch **Add to Album** reuses the bounded Album membership route with the
multi-selected identifiers. The successful membership result identifies the
newly added Photo IDs and the IDs that were already members. A separate
bounded compensation operation removes only the newly added IDs; it is not
global Selection State Undo and does not claim to restore historical Album
positions. Existing membership rules, the membership bound, and duplicate
suppression remain authoritative.

The Grid selection tray is source-owned presentation. It shows `Visible
results` for the currently filtered Browse Snapshot and `Source progress` for
the complete source's Selection State counts. These labels must not reuse one
number with two meanings. A batch Selection State decision or batch Album
addition does not write Album saved position; an Album result explicitly
reports that the durable Photo View resume position is unchanged. A
**Remove added Photos** compensation is an ordinary Album removal and reports
the resulting saved position when it removes the saved Photo.

### Persistent Derivative Cache

The existing cache identity and atomic publication contracts remain authoritative. Both `thumbnail-512` and `review-2560` derivatives persist in the configured cache directory and may be reused across server restart.

A current cache hit must not reopen or reprocess the Original File. Derivative delivery uses immutable identity-bearing URLs, `ETag`, and long-lived immutable browser caching. A changed source revision creates a different cache identity and cannot be presented as current under the previous identity.

The cache is rebuildable and not authoritative for Selection State, Rating, membership, or saved position. Removing cache bytes may cause regeneration but must not change SQLite user state or Original Files.

### Preview Scheduling

Scheduling priority remains:

1. current Photo review Preview;
2. immediately next and previous Photo review Previews;
3. visible Grid thumbnails;
4. bounded Grid look-ahead thumbnails.

Current Preview completion triggers adjacent prefetch. Prefetch is limited to immediate neighbors and remaining shared native-work capacity. Moving to a prefetched neighbor promotes that request to current priority.

Slipstream must not automatically prepare every Library Preview. Full precomputation would create unbounded storage and Original-file I/O relative to actual browsing.

Duplicate requests for one cache identity share one in-flight job. Leaving Photo View may leave a nearly complete reusable job running, but queued speculative work with no consumer may be dropped.

### Filmstrip Neighbors

Photo View presents the current Photo's immediate neighbors from the same
Browse Snapshot. The strip adds no route, no request shape, and no server
contract. While Photo View owns the UI the browser admits no window work for
a hidden surface, and the strip admits none itself: it presents the facts
the current Photo's loaded window already holds. Photo-scoped work the Photo
surface already runs — the current Preview and its adjacent prefetch — can
load a neighbor's window too, and the strip presents those facts as they
arrive. A neighbor no loaded window covers stays a placeholder until the
Photographer navigates to it, which admits that Photo's window through the
normal Photo path and no more.

The strip is presentation state of the Photo surface, bounded at five
entries: two Photos on each side of the current Photo, clamped at the
source's ends. It renders only while Photo View is visible and draws its
thumbnails from the single `thumbnail-512` derivative. It deliberately does
not reuse the Grid's cell composition: an entry crops that derivative to a
square and places the Selection State badge over it, where a Grid cell keeps
the complete image at its true aspect ratio and carries its indicators in
the footer. An entry whose window has not loaded yet presents a quiet
placeholder rather than a guessed Photo, and a strip failure cannot change
the current Photo's transitions.

Five entries is the bound chosen here: more neighbors would enlarge the
strip's thumbnail demand without changing which Photos the Photographer can
reach, and an unbounded strip would contradict the bounded window contract
this design protects. The strip is navigation, not decision: activating a
neighbor opens that Photo, and only the open Photo carries a decision. The
current entry is marked as current and presents no activation, so it can
never re-open the Photo the Photographer is already viewing and discard
their zoom.

### Rejected: Let the strip admit its neighbors' windows

A strip that admitted a neighbor's window itself would make a hidden Grid
range take on work while Photo View owns the UI. The first attempt did
exactly that and regressed two recovery scenarios: an expired-source reopen
pulled the tail window behind a hidden Grid, and an expired reopen plus a
failed adjacent prefetch stopped reaching `Disconnected`. The deferred
window rule and those recovery paths are worth more than a strip that never
shows a placeholder, so a neighbor outside the loaded window stays a
placeholder.

### Client Work Scheduling

The browser treats source control and current-Photo requests as foreground work. Grid thumbnail transfer, bounded look-ahead, adjacent Preview preparation, and scan progress are background work. Background work must not occupy browser network or rendering capacity in a way that delays a new source request or an already available control.

Each source opening and current Photo owns a client request generation. Starting a newer generation aborts fetch-based work from the prior generation and rejects any late response or failure from changing current state. Before a superseded Grid or Photo View is detached or hidden, the browser explicitly removes every pending image source so an already-started transfer cannot continue owning an HTTP connection. A stale Browse Snapshot opened after supersession is explicitly closed.

Grid thumbnail transfer and adjacent Preview preparation use lower browser priority than source, window, and current-Photo requests. Grid and Photo View image decoding is asynchronous, while Photo View's current Preview retains foreground network priority. Scroll events report one visible range per coalesced update; window admission runs immediately while Grid DOM reconstruction is coalesced to at most one render per animation frame that reuses the nodes of Photos still visible.

### Loading Feedback

The Web application owns presentation of asynchronous phases. It must remain responsive while requests run.

- Overview loading shows connection and summary phases.
- File Location loading reports the real requested direct-child range and total.
- Browse creation shows source-order preparation without a fake percentage.
- Grid loading reports the real requested range and total.
- Thumbnail loading uses stable cell placeholders and real completed/visible counts when useful.
- Preview loading reports cache lookup or preparation phases without claiming native extraction percentages that are not measured.
- Background scan status comes from the server's current real phase and counters.

Polling `GET /api/status` at a modest interval is sufficient for one Photographer. WebSocket or a distributed event service is not required.

### Reconnect and Expiration

Already loaded facts and derivative bytes remain visible after disconnection. Mutations remain disabled until the server confirms current state.

The status poll is also the browser's continuous reachability probe. A poll that cannot reach the server marks transport lost, so connectivity and decision readiness stop claiming a live server without a Photographer action. A poll that returns a usable status answer may restore transport reachability, because this probe is the designated reachability signal rather than an unrelated request. A poll that receives an answer without a usable status reports neither transition: the server is reachable, and that answer is a server-side condition.

Restoring transport reachability is not itself a recovery. It does not retire an active Recovery claim, so a decision that still waits for its own confirmation stays unavailable after the probe answers again. Reachability and operation recovery remain separate axes: reachability follows transport evidence, and claims follow operation outcomes.

If a Browse Snapshot still exists, reconnect reloads only the current bounded window. If it expired or the process restarted, the browser creates a new Snapshot for the same source, moves to the same Photo when it still exists, and tells the Photographer that the latest published order is now in use. Browser-local facts scoped to the retired Snapshot, including the temporary memory that keeps a removed Album member visible, are discarded only after the replacement Snapshot commits successfully. A failed reopen retains those facts with the recoverable current view.

The first product does not promise durable `All Photos` or Original Folder position across browser reload. Album saved position remains durable SQLite state.

## Failure Behavior

A failed File Location request does not clear successfully loaded Folder nodes. The browser identifies and retries only the failed direct-child range. An expired publication clears and reloads File Location navigation from the current Published Library rather than appending incompatible children.

A failed Browse Window request does not clear successfully loaded windows. The browser identifies the failed range and retries that range.

A bounded Grid Photo fact retains the server-supplied Photo availability, Original availability, and Preview state independently. A browser-owned thumbnail delivery failure remains bounded and attached to that Photo without replacing those server facts. None of these outcomes blocks sibling cells, navigation, Selection State, or Rating.

A background rescan failure retains the prior Published Library and reports an actionable failed status. It must not publish a partial order. Root binding mismatch, schema rejection, sidecar admission failure, and invalid storage layout remain hard failures rather than background warnings.

Browse Snapshot eviction returns a distinct not-found or expired response. The browser must not reinterpret an arbitrary token or silently continue with a different order.

Cache write failure may serve a valid stale derivative under the existing stale-truth contract. It must not remove a prior valid derivative before replacement completes and must never modify an Original File.

## Options

### Selected: Bounded Server-Owned Browse Snapshot and Windows

This preserves a stable open-source order while keeping browser transfer, memory, and DOM bounded. It also lets the server resolve Album resume behavior without sending all members.

### Rejected: Add Progress to the Existing Complete Response

A progress indicator would make waiting visible but would still transfer, parse, retain, and map every Photo fact before browsing. Cost would continue growing with the Library.

### Rejected: Cursor Pagination Without a Stable Snapshot

Rows inserted or reordered by a rescan could be duplicated, skipped, or moved between pages while the Photographer scrolls. Stable ordering requires one explicit hidden snapshot boundary.

### Rejected: Stream Every Photo Fact

Streaming could show early rows sooner, but total transfer and browser memory would still grow with the entire Library. It also complicates reconnect and partial-order publication without solving the underlying boundary.

### Rejected: Send Every Ordered ID to the Browser

Sending only IDs is smaller than sending every fact, but it still makes startup transfer and browser memory proportional to the whole Library and leaves Snapshot lifecycle and Album resume rules in the client.

### Selected: Freeze the Filter into the Browse Snapshot

The Selection State filter is resolved with the view order against the same
complete source and stored in the Snapshot. Every position, window, and
identity lookup keeps one meaning for the life of the open view, a filter
change reuses the existing reopen and anchor paths, and the browser never
derives membership from loaded windows.

### Rejected: Filter as a Per-Window Request Parameter

Letting each `GET /api/browse/{token}` request carry its own filter would avoid
reopening the source on a filter change. It fails the Snapshot contract: two
requests for one token could then return different memberships, totals, and
positions, and the browser would have to fence its retained windows against a
parameter instead of against the frozen Snapshot. Positions, saved-position
anchoring, and Previous and Next navigation would all need a filter argument to
stay coherent. Freezing the filter into the Snapshot keeps one meaning for
"the open view" and reuses the existing reopen, anchor, and retry paths.

### Selected: Persistent Demand Cache with Adjacent Prefetch

This makes repeat and next-Photo browsing fast while keeping I/O proportional to actual use.

### Rejected: Precompute Every Derivative

A full-Library warmup may consume tens of gigabytes and many hours of mounted-storage I/O while competing with the current Photo. The product has no current requirement for every Photo to be instantly available before browsing.

### Selected: Generation-Scoped Cancellation and Foreground Request Priority

This immediately retires obsolete fetches and prevents rebuildable thumbnail transfer from owning the browser connection pool ahead of a new source request. It preserves demand-driven derivatives and the existing bounded HTTP protocol without another service or transport.

### Rejected: Let Background Requests Drain Before Switching

Allowing old thumbnail or window requests to finish makes source-switch latency proportional to the previous viewport's remaining bytes and browser connection limits. It violates the requirement that background loading not block interaction.

### Rejected: Serialize All Grid Thumbnail Requests

Serial transfer would leave connection capacity available, but it would unnecessarily slow ordinary Grid completion and still would not define cancellation or stale-response ownership. Browser priority plus generation-scoped cancellation preserves useful concurrency.

### Selected: Status Polling

A small status query is sufficient for one local Photographer and keeps process lifecycle simple. One poll carries both the Library scan phase and the browser's reachability signal, so connectivity needs no second timer, channel, or lifecycle.

### Rejected: WebSocket, Message Broker, or Separate Worker Service

These add reconnection protocols, deployment units, and coordination state without a demonstrated need. Existing HTTP, bounded native work, and the SQLite owner remain adequate.

## Verification

Verification must include a generated Library projection with at least 40,000 Photos and prove:

- Library Overview size does not grow with Photo count except encoded counts and Album summaries;
- every File Location Window respects an enforced maximum, retained windows share one publication, expiration reloads rather than mixes generations, and no route returns the complete Folder tree or complete recursive membership;
- File Location counts count each Photo once and include remembered unavailable Photos at their last known Locations;
- a moved Original File is restored before any new Photo is allocated, and an ambiguous or unprovable group stays unavailable without state transfer;
- the first Grid becomes interactive without a complete Photo transfer;
- every Browse Window respects the enforced maximum;
- browser-retained Folder nodes, Photo facts, and rendered cells remain bounded while navigating and scrolling from the first to a late position;
- `All Photos` and Original Folder order match Capture Time rules, while Album order matches membership position;
- a Selection State filter returns exactly the matching Photos of the source order, its total and positions describe the filtered sequence, its counts describe the unfiltered source, an unknown filter value is rejected before a Snapshot exists, and no filter value changes Album membership, member position, or Original Files;
- each source's default order is used when no order is requested, `capture-time-desc` and Album time views order the complete source before pagination, missing-time Photos stay last in both directions, tie-breakers keep their direction, and persisted Album positions are unchanged;
- the per-Photo Album membership query answers from membership tables without materializing member lists, and an unknown Photo is a distinct not-found failure;
- a rescan refreshes facts and File Location navigation but cannot reorder or insert into an open Browse Snapshot;
- reopening the source after rescan uses the new complete order and current Folder subtree;
- Album saved-position and unavailable-member fallback work without complete membership transfer;
- Selection State, Rating, undo, and saved Album position mutations refresh only affected facts and survive restart;
- a batch Selection State write compares every existing requested Photo with its expected state in one transaction, reports exactly one non-overlapping `applied`, `changedElsewhere`, or `missing` outcome per request item, never overwrites a changed Photo, rejects an over-limit or malformed request before any write, moves the source's decision counts only for confirmed Photos, and undoes as one unit through per-Photo compare-and-set;
- current Preview work outranks adjacent and Grid work under the shared capacity-two budget;
- the manual recovery routes list remembered unavailable facts, return inspectable proposals without writing, commit one approved batch atomically, and refuse a stale or colliding batch without partial association;
- a generated thumbnail and review Preview are reused from server cache after process restart and from browser HTTP cache when identity is unchanged;
- a source revision change cannot reuse an old derivative as current;
- cache removal rebuilds derivatives without changing SQLite user state or Original File hashes;
- background scan status reports real phases and counts and never publishes a partial Library;
- disconnect, failed window, expired Snapshot, and retry behavior preserve already loaded content;
- a source switch reaches its bounded ready state while prior Grid derivative responses remain held, cancels superseded fetches, and ignores late responses;
- thumbnail requests retain lower browser priority than current source and Photo work, and repeated scroll events cause at most one Grid reconstruction per animation frame while overlapping cells keep their DOM node identity;
- one coalesced request serves every concurrent demand for a bounded window, each completed window settles exactly one completion notification, a late response for an older position never evicts current-viewport facts, and retained facts and rendered cells stay bounded at supported large viewports; and
- mobile Chromium remains usable under throttled network and CPU conditions.
