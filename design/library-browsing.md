# Scalable Library Browsing

Slipstream's Web application must browse a Photo Library whose size exceeds what a mobile browser should download, parse, retain, or render as one response. The Photographer opens `All Photos` or a Photo Set, browses its Grid, and opens individual Photos in the same Library Browser. The implementation needs a hidden stable-order boundary so a background rescan cannot move Photos underneath the user.

## Design Drivers

- The Library may contain tens of thousands of Photos and continue growing.
- The first useful screen must not depend on every Photo fact or Photo Set member.
- Grid and Photo navigation must preserve deterministic Library order or explicit Photo Set order.
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
- Photo Set identifiers, names, counts, and saved-position availability;
- current scan state and progress summary; and
- whether a published Library is available.

It contains no complete Photo list and no complete Photo Set membership list. Its response size is therefore independent of Library Photo count except for encoded counts and the number of Photo Sets.

### Browse Snapshot

A Browse Snapshot is an internal, ephemeral server object. It is not a product or domain concept exposed to the Photographer.

It contains:

- one opaque token;
- source identity: `All Photos` or one Photo Set;
- an immutable ordered array of Photo IDs;
- total count;
- the Photo Set's initial saved position when applicable; and
- last-access time for bounded cleanup.

Creating a Snapshot copies only ordered Photo IDs. It does not copy Photo facts, thumbnails, or Preview bytes. Current Photo facts are queried from the Library owner when a window is requested.

One process may retain only a bounded number of Snapshots. Explicit close, idle expiration, server restart, and bounded oldest-idle eviction may remove one. Losing a Snapshot never loses Selection State, Rating, Photo Set membership, or saved position because those remain in SQLite.

### Browse Window

A Browse Window is a bounded consecutive range within one Browse Snapshot. Each item contains only the facts needed by Grid View and Photo View:

- position and Photo ID;
- availability and ambiguity;
- Selection State and Rating;
- Original kinds and availability;
- current thumbnail and review-Preview facts; and
- derivative URLs only when current cache identities are known.

A request must provide a start position and bounded limit. The server enforces a small maximum. No browser-facing route may use omission of the limit to mean the complete Library.

### Published Library

The Published Library is the most recent complete scan committed by the Library owner. The browser may use it while an ordinary rescan builds a replacement. A root binding, schema, confinement, or state admission failure remains fail-closed and prevents service admission.

### Loading Status

Loading Status reports real phases and counts. It distinguishes:

- opening admitted persisted state;
- discovering supported Original Files;
- inspecting Capture Time facts;
- applying a completed scan;
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
POST   /api/browse
GET    /api/browse/{token}?start={position}&limit={count}
DELETE /api/browse/{token}
```

`POST /api/browse` accepts one source:

```json
{ "source": "library" }
```

or:

```json
{ "source": "photo-set", "photoSetId": "opaque-id" }
```

It returns the opaque token, total count, initial position, and optionally one bounded first window. The exact JSON belongs to the protocol compatibility fixtures; database rows and absolute Original Locations do not cross this boundary.

The existing unbounded complete-Photo response is not part of the target browser contract. Operator verification must use bounded traversal or an explicit offline state check rather than rely on a production route that materializes every Photo fact.

### Order Ownership

An `All Photos` Browse Snapshot copies Photo IDs from the current Published Library's deterministic Capture Time order. A Photo Set Browse Snapshot copies IDs by persisted membership position.

After creation, a Snapshot's ID order never changes. A rescan may change facts returned for those IDs, including availability and Preview state, but cannot insert, remove, or reorder them. Reopening the source creates a new Snapshot from the latest Published Library.

The server resolves Photo Set saved position when it creates the Snapshot. It applies the unavailable-member fallback defined by the Product Spec. The browser does not download all members to reproduce this rule. Durable saved position changes only when a Photo becomes current in Photo View and the position write is confirmed; Grid scrolling remains browser-local.

### Grid Loading

The browser requests only the first Grid window needed for the viewport and a small look-ahead. Scrolling requests later windows. It may discard distant facts while retaining enough identity and measurements to preserve scroll position.

The rendered Grid must use virtualization or an equivalent bounded-DOM mechanism. The number of rendered cells must be proportional to the viewport and look-ahead, not the total source size.

Duplicate or overlapping window requests may share server work, but a general response cache is not required. SQLite queries must run through the existing owner boundary and must preserve request order.

### Current Photo Facts

Browse Snapshot order and Photo facts have separate owners:

```text literal
Browse Snapshot -> stable ordered Photo IDs
Library owner    -> current Photo facts
Preview service  -> current derivative facts and bytes
```

A Selection State or Rating mutation updates SQLite through the existing transaction boundary. The affected loaded window is updated locally after confirmation. A conflict or reconnect refreshes only the affected Photo or current bounded window.

Preview completion must become visible to subsequent window or Photo queries immediately. A stale process-wide scan snapshot must not remain the only source of Preview state after the Preview service has persisted newer facts.

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

### Loading Feedback

The Web application owns presentation of asynchronous phases. It must remain responsive while requests run.

- Overview loading shows connection and summary phases.
- Browse creation shows source-order preparation without a fake percentage.
- Grid loading reports the real requested range and total.
- Thumbnail loading uses stable cell placeholders and real completed/visible counts when useful.
- Preview loading reports cache lookup or preparation phases without claiming native extraction percentages that are not measured.
- Background scan status comes from the server's current real phase and counters.

Polling `GET /api/status` at a modest interval is sufficient for one Photographer. WebSocket or a distributed event service is not required.

### Reconnect and Expiration

Already loaded facts and derivative bytes remain visible after disconnection. Mutations remain disabled until the server confirms current state.

If a Browse Snapshot still exists, reconnect reloads only the current bounded window. If it expired or the process restarted, the browser creates a new Snapshot for the same source, moves to the same Photo when it still exists, and tells the Photographer that the latest published order is now in use.

The first product does not promise durable `All Photos` position across browser reload. Photo Set saved position remains durable SQLite state.

## Failure Behavior

A failed window request does not clear successfully loaded windows. The browser identifies the failed range and retries that range.

A per-Photo thumbnail or Preview failure remains attached to that Photo and does not block sibling cells, navigation, Selection State, or Rating.

A background rescan failure retains the prior Published Library and reports an actionable failed status. It must not publish a partial order. Root binding mismatch, schema rejection, sidecar admission failure, and invalid storage layout remain hard failures rather than background warnings.

Browse Snapshot eviction returns a distinct not-found or expired response. The browser must not reinterpret an arbitrary token or silently continue with a different order.

Cache write failure may serve a valid stale derivative under the existing stale-truth contract. It must not remove a prior valid derivative before replacement completes and must never modify an Original File.

## Options

### Selected: Bounded Server-Owned Browse Snapshot and Windows

This preserves a stable open-source order while keeping browser transfer, memory, and DOM bounded. It also lets the server resolve Photo Set resume behavior without sending all members.

### Rejected: Add Progress to the Existing Complete Response

A progress indicator would make waiting visible but would still transfer, parse, retain, and map every Photo fact before browsing. Cost would continue growing with the Library.

### Rejected: Cursor Pagination Without a Stable Snapshot

Rows inserted or reordered by a rescan could be duplicated, skipped, or moved between pages while the Photographer scrolls. Stable ordering requires one explicit hidden snapshot boundary.

### Rejected: Stream Every Photo Fact

Streaming could show early rows sooner, but total transfer and browser memory would still grow with the entire Library. It also complicates reconnect and partial-order publication without solving the underlying boundary.

### Rejected: Send Every Ordered ID to the Browser

Sending only IDs is smaller than sending every fact, but it still makes startup transfer and browser memory proportional to the whole Library and leaves Snapshot lifecycle and Photo Set resume rules in the client.

### Selected: Persistent Demand Cache with Adjacent Prefetch

This makes repeat and next-Photo browsing fast while keeping I/O proportional to actual use.

### Rejected: Precompute Every Derivative

A full-Library warmup may consume tens of gigabytes and many hours of mounted-storage I/O while competing with the current Photo. The product has no current requirement for every Photo to be instantly available before browsing.

### Selected: Status Polling

A small status query is sufficient for one local Photographer and keeps process lifecycle simple.

### Rejected: WebSocket, Message Broker, or Separate Worker Service

These add reconnection protocols, deployment units, and coordination state without a demonstrated need. Existing HTTP, bounded native work, and the SQLite owner remain adequate.

## Verification

Verification must include a generated Library projection with at least 40,000 Photos and prove:

- Library Overview size does not grow with Photo count except encoded counts and Photo Set summaries;
- the first Grid becomes interactive without a complete Photo transfer;
- every Browse Window respects the enforced maximum;
- browser-retained facts and rendered cells remain bounded while scrolling from the first to a late position;
- `All Photos` order matches Capture Time rules and Photo Set order matches membership position;
- a rescan refreshes facts but cannot reorder or insert into an open Browse Snapshot;
- reopening the source after rescan uses the new complete order;
- Photo Set saved-position and unavailable-member fallback work without complete membership transfer;
- Selection State, Rating, undo, and saved Photo Set position mutations refresh only affected facts and survive restart;
- current Preview work outranks adjacent and Grid work under the shared capacity-two budget;
- a generated thumbnail and review Preview are reused from server cache after process restart and from browser HTTP cache when identity is unchanged;
- a source revision change cannot reuse an old derivative as current;
- cache removal rebuilds derivatives without changing SQLite user state or Original File hashes;
- background scan status reports real phases and counts and never publishes a partial Library;
- disconnect, failed window, expired Snapshot, and retry behavior preserve already loaded content; and
- mobile Chromium remains usable under throttled network and CPU conditions.
