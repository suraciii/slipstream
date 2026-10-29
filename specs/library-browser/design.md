# Scalable Library Browsing

Slipstream's Web application MUST browse a Photo Library whose size exceeds what a mobile browser SHOULD download, parse, retain, or render as one response.

The Photographer opens `All Photos`, one Original Folder, or one Album, browses its Grid, and opens individual Photos in the same Library Browser.

The implementation needs a hidden stable-order boundary so a background rescan cannot move Photos underneath the user.

## Design Drivers

- The Library MAY contain tens of thousands of Photos and continue growing.
- The first useful screen MUST NOT depend on every Photo fact, Album member, or Original Folder.
- Grid and Photo navigation MUST preserve deterministic Library and Folder order or explicit Album order.
- A rescan MAY refresh Photo facts but MUST NOT reorder an already open source.
- Grid thumbnails and review Previews are rebuildable, persistent derivatives.
- The current Photo MUST NOT compete equally with speculative background work.
- The server remains one Rust modular monolith with one SQLite owner and one Photographer.
- Original Files remain descriptor-confined and read-only.
- The browser MUST expose truthful loading and scan progress without invented percentages.

## Model

### Library Overview

The Library Overview is a bounded summary of the current published Library.

It contains:

- total Photo count;
- Album identifiers, names, counts, and saved-position availability;
- current scan state and progress summary; and
- whether a published Library is available.

It contains no complete Photo list, complete Album membership list, or Original Folder tree.

Its response size is therefore independent of Library Photo count except for encoded counts and the number of Albums.

### File Location Window

A File Location Window is one bounded range of direct child Original Folders beneath a requested parent.

Each Folder summary contains its relative Location, display name, recursive Photo count, and whether known descendant Folders exist.

The server derives Folder summaries from one Published Library according to [Physical File Locations and Virtual Albums](../photo-library/design.md).

A request provides one validated relative parent Location, start position, bounded limit, and the opaque publication value from the first retained Folder window.

The first request MAY omit that value and binds to the current publication.

The server returns the publication, parent, requested start, total direct-child count, and only that range.

A newer publication expires the old value.

The browser restarts File Location navigation instead of combining windows from different publications.

No route returns the complete Folder tree or complete recursive Folder membership.

The Library Overview therefore remains small even when directory count grows with the Library.

The File Location owner retains at most 32 parent windows, including the root, across expanded and collapsed Folders. Collapse changes disclosure without discarding a retained page.

Reopening uses the retained page immediately unless the parent has an unresolved failure. Retention evicts an older non-root window; publication reset discards every window.

Explicit count refresh MUST replace the pending request scope without changing publication authority or exact-range failure identities. It MUST detach earlier Folder continuations without waiting for their non-abortable transports.

The refresh plan MUST contain only visible retained windows and already requested visible windows. It MUST preserve the latest requested page and expansion intent.

Refresh MUST discard hidden windows, including descendants hidden by an ancestor or by paging.

The captured requests MUST start together before another interaction can change the plan. A later collapse MUST prevent a pending response from reopening that Folder. Newer same-parent requests MUST supersede earlier requests.

An obsolete response MUST NOT change counts or replace current retry state. Tree traversal and cache cleanup MUST NOT continue after the captured requests settle.

Read-state changes and waiter resolution MUST occur in one synchronous settlement. Summary and Recovery claims MUST match the current exact-range failure identities, even when refresh supersedes a completed read's presentation.

The controller owns each pending expansion and its loading presentation:

| Event                                 | Expansion presentation                                                             |
| ------------------------------------- | ---------------------------------------------------------------------------------- |
| Repeated expansion                    | Join the pending operation.                                                        |
| Reset or teardown                     | Invalidate the presentation.                                                       |
| Superseding refresh or page request   | Release the pending expansion feedback.                                            |
| Retry                                 | Replace expansion feedback with feedback for the retry request.                    |
| Obsolete, non-abortable transport     | Do not keep the disclosure busy or prevent collapse after its replacement settles. |
| Load joining the initial root binding | Keep the existing expansion feedback.                                              |

Refresh, paging, and retry still request authoritative windows. Bounded reuse avoids a network round trip for every disclosure without retaining the complete tree or bypassing failure recovery.

### Browse Snapshot

A Browse Snapshot is an internal, ephemeral server object.

It is not a product or domain concept exposed to the Photographer.

It contains:

- one opaque token;
- source identity: `All Photos`, one Original Folder, or one Album;
- the view order and filter conditions it was created with;
- an immutable ordered array of Photo IDs;
- total count;
- the source's per-state Selection counts when it was created;
- the Album's initial saved position when applicable; and
- last-access time for bounded cleanup.

Creating a Snapshot copies only ordered Photo IDs and bounded counts.

It does not copy Photo facts, thumbnails, or Preview bytes.

Current Photo facts are queried from the Library owner when a window is requested.

One process MAY retain only a bounded number of Snapshots.

Explicit close, idle expiration, server restart, and bounded oldest-idle eviction MAY remove one.

Losing a Snapshot never loses Selection State, Rating, Album membership, or saved position because those remain in SQLite.

### Browse Window

A Browse Window is a bounded consecutive range within one Browse Snapshot.

Each item contains only the facts needed by Grid View and Photo View:

- position and Photo ID;
- availability of the Photo and its Original;
- Selection State and Rating;
- the single Original's kind;
- the ordering Original filename, because Grid View and Photo View identify a Photo by the Original File the Photographer decides on;
- current thumbnail and review-Preview facts; and
- derivative URLs only when current cache identities are known.

The ordering Original filename is a basename only.

The relative Location, absolute paths, and the Library Folder's absolute path never cross this boundary.

A request MUST provide a start position and bounded limit.

The server enforces a small maximum.

No browser-facing route MAY use omission of the limit to mean the complete Library.

Each Grid item's Photo summary carries one `original` fact with its kind and availability; the retired `ambiguous` and `originals` array fields MUST NOT appear in any current response.

### Photo Review Metadata

Photo View obtains a bounded, Photo-scoped metadata view through `GET /api/photos/{id}/metadata`.

The response contains only Capture Time, Aperture, ISO, Shutter Speed, and Focal Length when the Photo's own Original File provides them.

Missing fields are omitted from the response and are rendered as `—` by the browser.

Reading metadata is read-only and failure does not make the Photo unavailable or block review actions.

This metadata is intentionally loaded on demand instead of being added to every bounded Grid window.

The first product does not expose a general EXIF tree, metadata editor, or unbounded metadata response.

### Photo Album Membership Query

Photo View obtains the Albums that contain one Photo through `GET /api/photos/{id}/albums`.

The response lists each containing Album's identifier and name in Album-list order.

The query is resolved server-side from the Album membership tables; the browser MUST NOT derive membership by traversing every Album's members.

The response is bounded by the number of Albums and contains no member lists.

An unknown Photo identifier is a distinct not-found failure.

Reading membership is read-only; mutations continue through the existing Album routes with their admitted-write contracts.

The position lookup accepts one stable Photo ID through `GET /api/browse/{token}/position?photoId={id}`.

It returns that Photo's position in the same immutable Browse Snapshot, or `null` when the Photo is not in the source.

It returns no Photo facts and has no unbounded form.

An expired or unknown Snapshot remains a distinct not-found failure.

### Published Library

The Published Library is the most recent complete scan committed by the Library owner.

The browser MAY use it while an ordinary rescan builds a replacement.

A root binding, schema, confinement, or state admission failure remains fail-closed and prevents service admission.

While any scan or recovery runs — including a saturated one — the last Published Library remains the authority for existing Photos: availability, metadata, source facts, and derivative eligibility continue to be served from it.

Only a completed scan atomically replaces it.

A deferred, failed, or interrupted scan publishes nothing, and a per-Original read that is empty, failed, or deferred never overwrites that Original's known published facts.

### Loading Status

Loading Status reports real phases and counts.

It distinguishes:

- opening admitted persisted state;
- discovering supported Original Files;
- inspecting Capture Time facts;
- recovering relocated Original Files;
- applying a completed scan;
- enrolling content fingerprints;
- idle;
- failed with the prior Published Library retained; and
- initializing when no Published Library exists.

A phase MAY omit a total until that total is known.

The protocol MUST NOT manufacture a percentage from elapsed time.

The status includes the scanner's last progress update in Unix milliseconds; zero means no scan has started in this process.

Recovery progress counts cover eligible candidates examined during the current scan, not an estimate.

A recovering phase without an established total MAY still represent active planning.

The timestamp changes only when the scanner reports phase or count progress, not on reads.

Loading Status is the Library axis only: it never reports processing capability or a Photo's source-read state, and a `recovering` phase changes neither.

A status answer preserves the latest observed phase and counts, so a follow-up `status` after an unknown `library check` outcome reports progress or the terminal result rather than a reset.

## Semantics

### Application Startup

Startup MUST first admit the Library Folder binding, SQLite state, cache layout, schema, and sidecar boundary.

Admission failure remains a hard startup failure.

When a compatible Published Library exists, the server MAY bind HTTP and serve it before an ordinary full rescan completes.

The rescan runs in the background and exposes Loading Status.

Its successful result atomically replaces the Published Library for future Browse Snapshots.

When no Published Library exists, the server serves an initializing Library Overview while the first scan runs.

The first Browse Snapshot cannot be created until that scan publishes a Library.

A Library Expansion retains its stricter offline contract.

The expansion command MUST still complete its required post-commit scan before reporting success.

Background startup does not weaken expansion admission or rollback behavior.

### Source Opening

The Web application loads the Library Overview first.

It MAY concurrently request the initial destination's Browse Snapshot so Grid placeholders appear immediately, but source navigation MUST NOT wait for that request, for a destination whose open needs no pending publication binding.

The bare application address selects All Photos.

Addressed startup and its single bootstrap election follow [Browser Navigation](experience/design.md#destination-establishment).

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

Loading Status also reports the committed recovery result of the most recent completed scan as `lastRecovery {relocatedPhotos, fingerprintedOriginals, unavailablePhotos}` and the enrollment counters as `fingerprints {enrolled, pending}`.

`relocatedPhotos` counts only associations committed by that scan; a candidate match, an occupied destination, an unresolved mapping, or a file found outside the admitted Library Folder does not increment it.

`unavailablePhotos` is the active unavailable-Photo count: `available=0` and not Removed, excluding Removed Photos in Trash.

The same active-only predicate applies to the remaining count returned by manual recovery.

The scan phase `recovering` covers fingerprint comparison for candidate locations.

The recovery routes serve the bounded manual recovery contract in [Photo Library and Albums](../photo-library/spec.md):

- `unavailable` lists remembered facts.
- `propose` returns inspectable mappings without writing.
- `apply` commits one approved batch of relocations with per-mapping revalidation.

A proposal carries one per-mapping outcome:

- `matched`;
- `content-mismatch`;
- `missing`;
- `kind-mismatch`;
- `unreadable`;
- `occupied`; or
- `colliding`.

It carries an explicit Retire and Bind candidate only when the destination Photo has no protected state or reference.

When a Published Library exists, both `GET /api/overview` and `GET /api/status` include its opaque publication generation.

The browser uses that generation to revalidate an Overview immediately before committing shared counts and Album summaries, so a response captured before replacement cannot cross the publication boundary.

An unpublished Library omits it.

`POST /api/scan` takes no semantic request fields; an empty JSON object is accepted.

A same-service request admits one application-owned Scan Cycle and waits for its terminal Loading Status.

Concurrent accepted requests join the cycle within the bounded waiter capacity.

Each receives the one terminal status captured by its leader.

The browser suppresses a duplicate Retry Library Check while its own command is in flight.

Success is `200` with the bounded Loading Status shape used by `GET /api/status` and no Photo facts.

`Origin` does not determine scan admission; an unavailable, saturated, or failed scan is `5xx`.

Wrong-method handling remains the shared `405` protocol rule.

A waiter whose client command times out or disconnects receives no terminal claim: the outcome of the admitted cycle is unknown to that client, which reconciles through `GET /api/status`.

A timeout is not a cancellation, a terminal failure, or a safe duplicate-retry signal; reissuing the request joins the still-running cycle and starts no second physical scan.

### Scan Cycle Ownership

The Application owns at most one Scan Cycle across background startup and explicit `POST /api/scan` callers.

The first admission starts an application-owned leader task.

Later admissions attach bounded waiters to that task rather than invoking independent publication lifecycles.

The leader runs one physical Library scan.

It publishes the result and completes status accounting exactly once.

The leader captures the resulting terminal Loading Status and fans the same result out to every remaining waiter.

Dropping an HTTP request removes only that request's waiter.

It does not cancel the admitted leader, skip publication, or leave status accounting elevated.

Application shutdown MAY drain or terminate the leader through the ordinary server shutdown contract, but cleanup still completes status accounting.

After the leader reaches terminal state, the next admission MAY create a new cycle.

This single-flight boundary prevents one physical scan from producing multiple publication generations.

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

For an Album, an explicit Resume request adds `"resume": true`.

An ordinary Album source open omits this field and always creates the Album Grid at its initial position.

Resume is never inferred from the absence of `photoId`.

It optionally accepts one explicit view order.

For `library` and `folder` sources the values are `"capture-time-asc"` (the default when omitted) and `"capture-time-desc"`.

For `album` sources the values are `"album-order"` (the default when omitted), `"capture-time-asc"`, and `"capture-time-desc"`.

An order value that is invalid for the source is rejected before any Snapshot is created.

It optionally accepts one Selection State filter.

The values are `"all"` (the default when omitted), `"unflagged"`, `"picked"`, and `"rejected"`.

An unknown value is rejected before any Snapshot is created.

The request MAY also carry the other defined filter facts:

| Fields                           | Values                                     |
| -------------------------------- | ------------------------------------------ |
| `ratingMinimum`, `ratingMaximum` | Inclusive integers from 0 through 5.       |
| `kind`                           | `"raw"` or `"jpeg"`.                       |
| `available`                      | `true` or `false`.                         |
| `capturedFrom`, `capturedBefore` | Camera-local `YYYY-MM-DDTHH:MM:SS` bounds. |

Every value MUST be validated against its fact before any Snapshot exists.

The server MUST reject:

- unknown enumeration values;
- out-of-range Ratings;
- inverted Rating ranges;
- reversed Capture Time ranges; and
- malformed timestamps.

The `capturedFrom` bound is inclusive and the `capturedBefore` bound is exclusive; a Photo without a Capture Time never matches a supplied bound.

All supplied facts combine with AND and are applied once, server-side, to the complete ordered source before the Snapshot is frozen.

A filtered Snapshot is therefore one frozen bounded sequence with its own total and positions, while its per-state Selection counts still describe the unfiltered source order and are reported even for an unfiltered open.

It returns the opaque token, total count, initial position, per-state Selection counts, and optionally one bounded first window.

The exact JSON belongs to the protocol compatibility fixtures; database rows and absolute Original Locations do not cross this boundary.

The protocol has no route that materializes every Photo fact, every Album member, every Original Folder, or complete recursive Folder membership.

The legacy unbounded complete-Photo and complete-membership routes remain retired.

Album mutations return bounded Album summaries in the same shape as the Library Overview's Album list, never member lists.

Legacy Photo Set routes and source values are retired rather than aliased.

A triggered scan reports Loading Status and returns no Photo facts.

Operator verification uses bounded traversal or an explicit offline state projection from the owned SQLite state rather than any production route that materializes every Photo fact.

### Order Ownership

An `All Photos` Browse Snapshot copies Photo IDs from the current Published Library's deterministic Capture Time order.

An Original Folder Browse Snapshot filters that order by the recursive component-aware Folder rule.

An Album Browse Snapshot copies IDs by persisted membership position.

The requested view order is applied once, server-side, to the complete source before the Snapshot is frozen:

- `capture-time-desc` reverses only the Capture Time direction. Photos
  without a valid authoritative Capture Time stay in the trailing partition,
  and the ordering Location and Photo ID tie-breakers keep their existing
  direction, so a descending view is never produced by reversing a
  sequence that contains missing-time Photos.
- An Album time view orders members by the same Capture Time authority
  through the Published Library's Photo facts while leaving persisted
  membership positions untouched.

After creation, a Snapshot's ID order never changes.

A rescan MAY change facts returned for those IDs, including availability and Preview state, but cannot insert, remove, or reorder them.

Reopening the source creates a new Snapshot from the latest Published Library, and an explicit refresh reuses the currently selected order and Selection State filter.

### Selection Filtering

Filtering is a view option of one open source.

The server resolves the view order against the complete source.

It then applies the combined conditions and freezes the resulting sequence into the Snapshot.

Filtering therefore selects from the order and never rewrites it.

It writes no persisted state: Album membership, Album member position, Selection State, Rating, and Original Files are unaffected.

The defined conditions and their meanings belong to [Source-Scoped Library Filters](filters/spec.md#defined-conditions).

A fact MUST NOT be supplied twice in contradictory ways.

The Snapshot's per-state Selection counts are computed over the same complete ordered source before the filter is applied.

A filtered Snapshot therefore reports the filtered total while its counts still describe the whole source, which is what makes progress readable inside a filtered view.

Both are plain Library facts read at creation time: a Photo that the Published Library cannot resolve is not counted, and no count is derived from requested windows.

Membership is frozen with the Snapshot order.

A decision that changes Selection State does not add or remove the Photo from an open Snapshot.

Reopening applies the filter to the latest facts and re-anchors the current Photo by identity.

Positions, `GET /api/browse/{token}/position`, windows, and Previous and Next navigation all resolve inside the frozen filtered sequence, so a filtered view stays one bounded sequence with one meaning for every position.

The server resolves Album saved position when it creates the Snapshot.

It applies the unavailable-member fallback defined by the Product Spec.

The browser does not download all members to reproduce this rule.

Durable saved position changes only when a Photo becomes current in Photo View and the position write is confirmed; Grid scrolling remains browser-local.

Saved position and view order are independent: the position resolves by Photo identity whichever order the open view uses.

The saved position applies only to an Album open that requests Resume and supplies neither an addressed Photo nor a restored Grid anchor.

A view change supplies the browser's current Photo by identity as its open anchor.

If it does not match the new filter or order, the view starts at its first Photo rather than the durable saved position.

A restored Grid anchor is browser-history restoration state, not an open anchor.

Every source open requires the published Library.

An Album open reads the same Published snapshot as `library` and `folder` sources for its anchor, filter membership, and counts, and `album-order` is no exception even though its order comes from persisted membership position.

An Album open therefore fails with the not-published response before any Snapshot exists instead of failing later at its first window.

### Random Position Selection

The random action samples one position from the current Browse Snapshot rather than creating a second source query.

For a Snapshot with total `N`, the target position is one integer in the inclusive range `0` through `N - 1`, and every position has equal probability.

The source/Grid owner admits the target through the same bounded window path used by Grid range loading and Photo navigation.

The random operation therefore has these boundaries:

- it reads the Snapshot's total and frozen order, not the rendered Grid cells;
- it does not copy every Photo ID or Photo fact to the browser;
- it keeps the source, order, and Selection State filter of the current
  Snapshot;
- it MAY select a Photo whose Original or Preview is unavailable; and
- it does not retain a no-repeat set, random seed, or random history.

The target's current bounded Photo facts come from the Library owner.

A target MAY become the ordinary Photo destination once those facts are ready; Preview bytes are not an additional random-selection boundary.

If the Snapshot expires before the target is admitted, the operation returns the distinct expired outcome and the browser keeps the Grid for the existing refresh or retry path.

It MUST NOT silently draw from a replacement Snapshot.

The source/Grid owner owns the Snapshot and bounded target read.

The navigation owner owns the ordinary Photo destination.

The Photo owner owns Photo commitment and any normal Album saved-position settlement.

No random-selection state is durable.

### Grid Loading

The Sources width and its separator MUST read the same width owner, so their values cannot diverge.

The source/Grid owner owns one range admission path.

The visible range plus a bounded buffer is reported as one unit.

The owner computes the aligned bounded windows missing from that range.

The owner MUST admit at most one in-flight request per window and coalesce concurrent demands for that window.

Each completed window MUST settle exactly one completion notification, regardless of how many consumers joined.

Rendering is presentational: it never initiates loading and never subscribes per cell.

The rendered Grid MUST use virtualization or an equivalent bounded-DOM mechanism.

The number of rendered cells MUST be proportional to the viewport and look-ahead, not the total source size.

Grid DOM updates merge to at most one per animation frame, and each update reuses the nodes of Photos that stay visible: only entering, leaving, or changed cells are touched.

Retained Photo facts are capped at a bound that covers the actual viewport and buffer at supported large viewports.

Eviction is anchored to the latest reported visible range and protects that range and its buffer; a late response for an older position MUST never evict current-viewport data.

A window request captured before a source change MUST NOT commit into the new source; correctness never depends on cancellation succeeding.

Duplicate or overlapping window requests share server work through the single in-flight coalescing above; a general response cache is not required.

SQLite queries MUST run through the existing owner boundary and MUST preserve request order.

### Current Photo Facts

Browse Snapshot order and Photo facts have separate owners:

```text literal
Browse Snapshot -> stable ordered Photo IDs
Library owner    -> current Photo facts
Preview service  -> current derivative facts and bytes
```

Each Photo's Selection State or Rating mutation MUST update SQLite in one transaction.

The mutation MAY update that Album's saved position in the same transaction.

The browser's one-level Undo description MUST identify the Photo, changed field, prior value, and expected current value.

Undo MUST use compare-and-set so a newer change is not overwritten.

The browser MUST reconcile failed mutations rather than treating an animation as proof of persistence.

The affected loaded window is updated locally after confirmation.

A conflict or reconnect refreshes only the affected Photo or current bounded window.

Preview completion MUST become visible to subsequent window or Photo queries immediately.

A stale process-wide scan snapshot MUST NOT remain the only source of Preview state after the Preview service has persisted newer facts.

### Batch Selection Writes

Grid multi-selection applies one Selection State to many Photos through `POST /api/photos/state`.

The request carries one bounded item per selected Photo and one `selectionState` value:

```json
{
  "selectionState": "picked",
  "photos": [{ "photoId": "photo-1", "expectedCurrent": "unflagged" }]
}
```

The server accepts at most 100 unique Photo identifiers and rejects an empty, duplicate, over-limit, malformed, incomplete, unknown-field, or invalid-state request before any write, as an invalid request rather than a conflict.

The browser mirrors that bound, so a batch it builds is always one the server can admit.

`selectionState` is exactly `picked` or `rejected`, and every item has exactly `photoId` and `expectedCurrent`, with `expectedCurrent` exactly `unflagged`, `picked`, or `rejected`.

Batch Rating is not part of this contract, and the route carries no source, Album, or position: the browser names the Photos it multi-selected.

The operation is one transaction with per-Photo outcomes, so one unsuccessful Photo never rolls back confirmed Photos.

The response reports exactly one outcome per requested Photo:

- `applied` with the Selection State the Photo held before the write;
- `changedElsewhere` with the Photo's current Selection State when its current
  value differs from `expectedCurrent`; and
- `missing` when the current Library no longer holds the Photo.

A `changedElsewhere` or `missing` Photo is not written.

On the batch settlement, the browser moves loaded Photo facts and source decision counts only for `applied` outcomes.

During `Review N`, it reconciles each refreshed Photo's contribution from the belief already held by the source counts to the observed fact, so a retry cannot move a count twice.

A Photo whose write was not confirmed is never presented as decided.

The browser keeps unsuccessful Photos selected for review or retry.

A missing Photo remains a selected, non-retryable result until clear or source change; its ID is excluded from later requests.

The browser does not invent a fact for a missing Photo.

`Review N` refreshes the current facts for N changed Photos and replaces their expected states before a retry.

Before treating a refreshed Photo as missing, the browser resolves its identity against the current Browse Snapshot position; bounded local fact eviction is retryable cache pressure, not proof that the Photo left the Library.

The response is valid only when its three arrays contain exactly one non-overlapping outcome for every requested Photo and each outcome has the fields defined above.

A missing array, duplicate or invented ID, obsolete `conflicts` field, or extra outcome field makes the whole response malformed.

A malformed response moves no facts, counts, or Undo entries; the selection remains retryable and the Grid reports a failed batch.

Batch Undo reuses the single-Photo compare-and-set write: the browser sends one bounded write per confirmed Photo, each naming the prior value the server reported and the batch's value as `expectedCurrent`.

One Photo that changed does not block the others, and the browser retires only the Photos whose Undo cannot apply.

Per-Photo compare-and-set preserves conflict truthfulness without a second batch Undo surface.

Batch **Add to Album** reuses the bounded Album membership route with the multi-selected identifiers.

The successful membership result identifies the newly added Photo IDs and the IDs that were already members.

A separate bounded compensation operation removes only the newly added IDs; it is not global Selection State Undo and does not claim to restore historical Album positions.

Existing membership rules, the membership bound, and duplicate suppression remain authoritative.

The Grid selection tray is source-owned presentation.

It shows `Visible results` for the currently filtered Browse Snapshot and `Source progress` for the complete source's Selection State counts.

These labels MUST NOT reuse one number with two meanings.

A batch Selection State decision or batch Album addition does not write Album saved position; an Album result explicitly reports that the durable Photo View resume position is unchanged.

A **Remove added Photos** compensation is an ordinary Album removal and reports the resulting saved position when it removes the saved Photo.

### Persistent Derivative Cache

The existing cache identity and atomic publication contracts remain authoritative.

Both `thumbnail-512` and `review-2560` derivatives persist in the configured cache directory and MAY be reused across server restart.

A current cache hit MUST NOT reopen or reprocess the Original File.

Derivative delivery uses identity-bearing URLs and `ETag`; [Instance Access Architecture](../access/design.md#private-content-and-transport) owns authentication and the private response cache policy.

A changed source revision creates a different cache identity and cannot be presented as current under the previous identity.

After a scan publishes a new or changed usable Photo, the server MAY warm its `review-2560` derivative at background priority.

This bounded warmup MUST NOT delay publication or make browsing depend on completing the derivative.

It MUST NOT backfill every existing Photo, and it does not warm `thumbnail-512`.

The cache is rebuildable and not authoritative for Selection State, Rating, membership, or saved position.

Removing cache bytes MAY cause regeneration but MUST NOT change SQLite user state or Original Files.

### Preview Scheduling

Scheduling priority remains:

1. current Photo review Preview;
2. immediately next and previous Photo review Previews;
3. visible Grid thumbnails;
4. bounded Grid look-ahead thumbnails;
5. new or changed `review-2560` warmup.

Current Preview completion triggers adjacent prefetch.

Prefetch is limited to immediate neighbors and remaining shared native-work capacity.

Moving to a prefetched neighbor promotes that request to current priority.

Duplicate requests for one cache identity share one in-flight job.

Leaving Photo View MAY leave a nearly complete reusable job running, but queued speculative work with no consumer MAY be dropped.

### Filmstrip Neighbors

Photo View presents the current Photo's immediate neighbors from the same Browse Snapshot.

The strip adds no route, no request shape, and no server contract.

While Photo View owns the UI the browser admits no window work for a hidden surface, and the strip admits none itself: it presents the facts the current Photo's loaded window already holds.

Photo-scoped work the Photo surface already runs — the current Preview and its adjacent prefetch — can load a neighbor's window too, and the strip presents those facts as they arrive.

A neighbor no loaded window covers stays a placeholder until the Photographer navigates to it, which admits that Photo's window through the normal Photo path and no more.

The strip is presentation state of the Photo surface, bounded at five entries: two Photos on each side of the current Photo, clamped at the source's ends.

The strip's responsive placement follows [Library Browser Experience](experience/spec.md#photo-view).

It renders only while its owning Photo surface or Nearby Photos disclosure is visible and draws its thumbnails from the single `thumbnail-512` derivative.

The strip does not reuse Grid cell composition.

An entry crops the derivative to a square and overlays the Selection State badge.

A Grid cell instead keeps the complete image at its true aspect ratio and carries its indicators in the footer.

An entry whose window has not loaded yet presents a quiet placeholder rather than a guessed Photo, and a strip failure cannot change the current Photo's transitions.

Five entries bound thumbnail demand without reducing navigation reach. An unbounded strip would violate the bounded window contract.

The strip is navigation, not decision: activating a neighbor opens that Photo, and only the open Photo carries a decision.

The current entry is marked as current and presents no activation, so it can never re-open the Photo the Photographer is already viewing and discard their zoom.

### Client Work Scheduling

The browser treats source control and current-Photo requests as foreground work.

Grid thumbnail transfer, bounded look-ahead, adjacent Preview preparation, and scan progress are background work.

Background work MUST NOT occupy browser network or rendering capacity in a way that delays a new source request or an already available control.

Each source opening and current Photo owns a client request generation.

Starting a newer generation aborts fetch-based work from the prior generation and rejects any late response or failure from changing current state.

Before a superseded Grid or Photo View is detached or hidden, the browser explicitly removes every pending image source so an already-started transfer cannot continue owning an HTTP connection.

A stale Browse Snapshot opened after supersession is explicitly closed.

Grid thumbnail transfer and adjacent Preview preparation use lower browser priority than source, window, and current-Photo requests.

Grid and Photo View image decoding is asynchronous, while Photo View's current Preview retains foreground network priority.

Scroll events report one visible range per coalesced update, and window admission runs immediately.

Grid DOM reconstruction is coalesced to at most one render per animation frame and reuses nodes of Photos still visible.

### Loading Feedback

The Web application owns presentation of asynchronous phases.

It MUST remain responsive while requests run.

- Overview loading shows connection and summary phases.
- File Location loading reports the real requested direct-child range and total.
- Browse creation shows source-order preparation without a fake percentage.
- Grid loading reports the real requested range and total.
- Thumbnail loading uses stable cell placeholders and real completed/visible counts when useful.
- Preview loading reports cache lookup or preparation phases without claiming native extraction percentages that are not measured.
- Background scan status comes from the server's current real phase and counters.

Polling `GET /api/status` at a modest interval is sufficient for one Photographer.

WebSocket or a distributed event service is not required.

### Reconnect and Expiration

Already loaded facts and derivative bytes remain visible after disconnection.

Mutations remain disabled until the server confirms current state.

The status poll is also the browser's continuous reachability probe.

A poll that cannot reach the server marks transport lost, so connectivity and decision readiness stop claiming a live server without a Photographer action.

A poll that returns a usable status answer MAY restore transport reachability, because this probe is the designated reachability signal rather than an unrelated request.

A poll that receives an answer without a usable status reports neither transition: the server is reachable, and that answer is a server-side condition.

Restoring transport reachability is not itself a recovery.

It does not retire an active Recovery claim, so a decision that still waits for its own confirmation stays unavailable after the probe answers again.

Reachability and operation recovery remain separate axes: reachability follows transport evidence, and claims follow operation outcomes.

If a Browse Snapshot still exists, reconnect reloads only the current bounded window.

If the Snapshot expired or the process restarted, the browser creates a new Snapshot for the same source.

It moves to the same Photo when present and tells the Photographer that the latest published order is now in use.

Browser-local facts scoped to the retired Snapshot, including the temporary memory that keeps a removed Album member visible, are discarded only after the replacement Snapshot commits successfully.

A failed reopen retains those facts with the recoverable current view.

Browser URLs restore addressed Photos and view options under [Browser Navigation](experience/design.md).

Exact Grid placement after reload remains browser-local best effort; it is not a durable All Photos or Folder resume feature.

Album saved position remains durable SQLite state.

## Failure Behavior

A failed File Location request does not clear successfully loaded Folder nodes.

The browser identifies and retries only the failed direct-child range.

An expired publication clears and reloads File Location navigation from the current Published Library rather than appending incompatible children.

A failed Browse Window request does not clear successfully loaded windows.

The browser identifies the failed range and retries that range.

A bounded Grid Photo fact retains the server-supplied Photo availability, Original availability, and Preview state independently.

A browser-owned thumbnail delivery failure remains bounded and attached to that Photo without replacing those server facts.

None of these outcomes blocks sibling cells, navigation, Selection State, or Rating.

A background rescan failure retains the prior Published Library and reports an actionable failed status.

It MUST NOT publish a partial order.

Root binding mismatch, schema rejection, sidecar admission failure, and invalid storage layout remain hard failures rather than background warnings.

Browse Snapshot eviction returns a distinct not-found or expired response.

The browser MUST NOT reinterpret an arbitrary token or silently continue with a different order.

Cache write failure MAY serve a valid stale derivative under the existing stale-truth contract.

It MUST NOT remove a prior valid derivative before replacement completes and MUST never modify an Original File.

## Design Rationale

Server-owned Snapshots and windows keep source order stable while bounding browser transfer, memory, and DOM.

Resolving filters before freezing gives every position one meaning.

Sampling a position from the current Snapshot preserves its source and filter without transferring the complete sequence or adding durable random state.

Persistent demand caching and adjacent prefetch keep I/O proportional to use.

Generation-scoped cancellation and foreground priority prevent obsolete transfers from delaying new source work.

One status poll carries both the Library scan phase and the designated reachability signal.
