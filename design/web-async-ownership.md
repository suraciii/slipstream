# Web Async Ownership

The Web client starts asynchronous work whose settlement may outlive the UI
state that initiated it. A stale operation must not revert newer shared data,
write into a newer source or Photo, erase a newer failure notice, or release a
recovery claim it does not own.

Async ownership has two independent dimensions:

1. the operation's **effect**: presentation read, shared-state read, lifecycle
   release, admitted persistence, or another admitted server command;
2. its **supersession policy**: abort transport, detach continuation, commit in
   order, or always settle.

A read is not automatically abortable, and a write is not automatically tied
to the lifetime of its initiating UI. Workflows decompose into child
operations; they never acquire an implicit fifth policy.

## Design Drivers

- Admitted server work is never treated as rolled back by HTTP cancellation.
  Once a persistence mutation or scan command is sent, the server may commit
  or continue it; aborting the fetch only blinds the client.
- Publication and Browse snapshot coherence require one generation at a time.
  A stale Folder or Browse result is never silently reinterpreted as current.
- Cancellation cannot replace ordering. Two successful reads may settle out
  of order, and a newer request may fail while an older valid response remains
  useful.
- Lifecycle cleanup is distinct from both reads and persisted user state: a
  Browse-token release is best-effort cleanup with bounded server expiry as
  its fallback.
- The client remains dependency-free. The required behavior fits one scoped
  task primitive and one write-settlement rule.

## Operation Model

A **scope** owns operations whose UI continuation becomes invalid together.
Halting a scope prevents every child continuation from committing. Depending
on the child's declared policy, halting also aborts the transport or merely
detaches the continuation while transport finishes.

An **ownership key** identifies work that supersedes or coalesces other work.
Source- and Photo-scoped keys include their captured generation so one source
or Photo cannot supersede another accidentally. Intentionally global work
(the overview and Album settlement families) and uniquely identified cleanup
(a Browse-token release) do not add a generation.

A **settlement handle** belongs to non-abortable work. It captures the
initiating generation and presentation surface, runs the request to
settlement, and decides whether the result may still write locally, must
fallback to the Library summary, or stays silent.

A generation is ownership metadata captured by an operation. It is not a
separate operation family.

## Operation Assignment

### Publication status polling

`GET /api/status` is a presentation read owned by the application scope with
the global `publication-status` key. The first committed overview starts one
application-lifetime status monitor whether the Library is published or not.
The monitor continues at a bounded idle cadence so it observes scans triggered
outside the current tab; while scan state is non-idle it may use a foreground
progress cadence. Starting a replacement monitor detaches the preceding timer,
loop, and in-flight continuation; transport need not be aborted. Application
teardown does the same. A detached monitor never writes scan status or starts
an overview load.

An answered HTTP non-success or transport failure is silent, does not create
or release a Recovery claim, and leaves the current monitor running. A
successful response may write scan progress only as a background update
through the Summary Notice Channel. Each individual status request captures a
fresh non-owning background epoch when that request starts; the lifetime
monitor does not reuse one epoch. Transition handling compares the response
with the last scan state observed by this monitor. A first committed overview
whose scan is already `idle` establishes the baseline and does not invent a
completion event because no open snapshot predates that commit.

A semantic `failed` scan retains the prior Published Library, stops that poll,
and claims an actionable `scan-failure` Summary notice with a **Retry Library
Check** action.

When a current poll first observes `idle` after a non-idle scan, it advances
the overview data floor, claims a completion notice that the prior open Browse
Snapshot remains unchanged, and offers **Refresh Current Source**. It also
starts `loadOverview` to refresh shared facts and File Locations, but the
completion notice remains until explicit source refresh or a higher-priority
notice replaces it.

### Library scan retry command

`POST /api/scan` is a non-abortable admitted server command owned by the
application. Duplicate Retry Library Check admission while one command is in
flight is a no-op. Starting the command creates a scan-completion handle and
records a known non-idle transition for that handle before awaiting the
terminal response. The existing status monitor remains running. The first of
(a monitor transition to `idle`) or (the command's successful terminal `idle`
response) consumes the handle, advances the overview floor, and presents the
completion notice; the other path is an exact-handle no-op. This guarantees
exactly-once completion even when the scan finishes between monitor polls.

An answered `4xx` keeps the scan-failure notice without adding a transport
Recovery claim. `5xx` or transport failure adds a scan-command Recovery claim
and keeps the same retry action. Successful terminal settlement releases that
exact claim. Application teardown detaches presentation from the command but
cannot treat accepted scan work as rolled back; the application-owned server
Scan Cycle continues independently of the HTTP waiter.

### Shared overview refresh

`GET /api/overview` is a shared-state read owned by the application scope,
with the global `overview` key and **commit-in-order** policy. A newer request
does not abort or detach an older one. Every request captures both a
monotonically increasing request sequence and the current **overview data
floor**. Successful Album mutations and observed publication replacement
advance that floor before starting their refresh. A response may commit only
when its captured floor still equals the current floor and its sequence is
newer than the last response committed at that floor. Thus an older success
may still commit after a newer request fails when no data-changing boundary
intervened, but a response started before a committed mutation or publication
change can never regress shared data. Application teardown detaches every
overview continuation so no response writes into a destroyed application.

Overview failure has no notice or Recovery policy of its own. Its live parent
supplies Summary and Recovery claim handles and decides whether failure is a
foreground reload failure, a persisted-Album-but-refresh-failed notice, or a
silent recovery probe. The owner action controls only that presentation; it
does not replace commit ordering or the data floor.

### File Location windows and root binding

`GET /api/file-locations` belongs to an independent File Location scope with
key `(fileLocationGeneration, parent)`. Source, Grid, and Photo changes neither
abort nor detach it. Only same-parent supersession, File Location generation
reset, or application teardown may detach it. Its transport may finish after
detachment, but its response cannot render, clear retry state, or replace the
parent's current page.

An answered publication conflict (`409`) advances the overview data floor,
resets the File Location generation, refreshes overview as a silent recovery
probe, rebinds the root, and claims an actionable summary notice only after the
new root is coherent. Any other
answered failure, malformed response, or transport failure for the current
key retains loaded siblings, records the exact failed range, claims the
actionable summary notice, exposes that range's retry control, and changes
connectivity to disconnected. Only success for that same range clears its retry state, releases its Summary
claim, and releases that range's Recovery claim. Success for another File
Location window may prove server reachability but cannot release the failed
range or any current Source/Photo recovery claim, and therefore cannot
re-enable decisions by itself.

The unbound root uses the distinct key
`(fileLocationGeneration, root-binding)`. Exactly one operation owns that key;
other callers await its settlement instead of starting or cancelling a second
bind. Resetting the File Location generation or tearing down the application
releases those waiters with an unbound result. Root binding is not superseded
by ordinary child-window work.

### Browse open and reopen

`POST /api/browse` is a presentation read that allocates a bounded server
snapshot. It is owned by the source scope, keyed by the source generation,
and uses **abort-transport** policy when a newer source supersedes it. If a
completed stale response exposes a token, the client starts a token-release
operation. If aborting loses the response after server allocation, bounded
server expiry remains the cleanup fallback.

A Folder Browse open additionally captures the bound File Location
publication. It never sends without that publication, and publication expiry
starts a new root-binding operation before a retry. A current open or reopen
failure retains the recoverable source, reports on the Grid/source surface,
and changes connectivity to disconnected. A Folder publication conflict also
runs the File Location recovery above; its actionable summary notice is
independent of the source failure message. Superseded failures are silent.

### Browse windows

`GET /api/browse/{token}` is a presentation read keyed by
`(sourceGeneration, alignedStart)` within its owning scope. Grid scrolling and
Grid look-ahead use the source/Grid scope. A boundary window needed to open or
retry the current Photo uses the Photo scope at high priority: starting it
aborts an in-flight low-priority Grid or adjacent request for the same range
and starts a foreground request instead of coalescing with the aborted work.
A source change aborts every window request from the old source. A response
commits only to its captured source generation and token.

An answered `404` starts current-source Browse recovery. If recovery fails,
the owning Grid or Photo surface identifies that the source expired and
remains retryable. A current-generation transport failure changes connectivity
to disconnected and presents range retry on the owning Grid or Photo surface.
Another answered failure or malformed response retains connectivity and
loaded content, identifies the failed range on the owning surface, and permits
that range to be requested again. An aborted or superseded settlement is
silent. A speculative adjacent-window failure follows the same read contract;
an adjacent Preview failure remains separately silent.

### Browse-token release

`DELETE /api/browse/{token}` is a lifecycle release keyed by token. It is
best-effort detached cleanup: UI teardown does not abort it, it presents no
success or failure notice, and it never changes connectivity. Bounded server
expiry is the fallback if the request cannot complete.

### Current Preview and adjacent Preview

The current Preview read is owned by the Photo scope with key
`(requestGeneration, photoId, current)`. Leaving the Photo, changing Photo, or
changing source aborts its transport and prevents every continuation after an
`await` from rendering, persisting position, or updating controls. An answered
non-ready Preview keeps the Photo navigable, reports the server's Preview
message on the Photo surface, and does not change connectivity. A transport or
malformed-response failure reports Photo retry and changes connectivity to
disconnected. A superseded failure is silent.

Adjacent Preview reads use key `(requestGeneration, photoId, adjacent)`, run
at low priority, and are best effort. The Photo scope aborts them on Photo or
source change. Their answered and transport failures are silent and never
change connectivity. When that neighbor becomes current, the browser starts a
foreground current-Preview request for the same Photo and derivative identity.
The Preview service deduplicates that identity and raises the existing native
job to current priority rather than running duplicate work; browser-request
cancellation does not cancel an admitted reusable server job.

### Thumbnail reads

Thumbnail reads are owned by the source/Grid scope with key
`(sourceGeneration, photoId)`. Requests with the same key coalesce. Source
change or application teardown aborts old work. Only the captured generation
may attach a thumbnail or mark it unavailable. Answered and transport failures
leave a stable unavailable placeholder; they present no summary or
connectivity failure.

### Browser image transfers

Assigning a URL to a Grid or Review `<img src>` starts a browser-managed byte
transfer distinct from `fetch`. Each Grid image transfer belongs to the
source/Grid scope and each Review image transfer belongs to the Photo scope.
Before its scope is halted or its element is replaced, the client removes the
pending `src` and its completion/error handlers. A completion or error may
write only while the element, URL, and captured generation still match; a
detached image error cannot mark a replacement cell or Photo unavailable.

### Album persistence

Album create, rename, delete, membership add, and membership remove are
admitted writes. They share one global settlement family key,
`album-mutation`, because a newer Album action owns Album notices and
connectivity presentation regardless of which Album it targets.

The write always settles. Immediately after success it advances the overview
data floor. Its shared-data refresh is a separate owner-tagged overview
operation captured at that new floor and therefore follows overview commit
order without permitting any pre-mutation response to regress Album state.
A current success may update its initiating form or Photo controls; a
superseded success is locally silent and the committed overview is its
confirmation. A current failure writes to its initiating surface. A
superseded failure falls back to the Library summary unless a newer Album
failure already owns it.

Membership duplicate suppression is a separate admission key,
`(verb, albumId, photoId)`. Repeating the same key while in flight is a no-op;
a different verb or Album is independently admitted. Form continuation
ownership is keyed by stable form ID, so an older settlement never clears a
newer form or draft.

Answered `404`, `409`, and other `4xx` failures do not change connectivity.
Transport failures and service failures (`5xx`) change connectivity only while
the operation still owns the global Album settlement family.

### Photo-state persistence and Undo

Selection, Rating, and Undo are admitted writes serialized by the current
Photo interaction (`busy` permits one at a time). Their settlement ownership
key is `(sourceGeneration, photoId, field)`. They always settle after send.
If source generation or Photo identity changes, their success and failure
continuations are silent; they do not fallback to the Library summary.

For a current Selection or Rating write, answered `409` reports a concurrent
change on the Photo surface and changes connectivity to disconnected so Retry
must refresh current facts. Any other answered non-success—including `404`,
other `4xx`, and `5xx`—reports that the change was not saved but does not
change connectivity. A transport failure reports uncertainty and disconnects.
Undo uses the same classification: `409` reports stale Undo and disconnects;
other answered non-success reports that Undo was not saved without
disconnecting; transport failure disconnects.

### Saved Album position

Saved-position writes are admitted writes serialized by the progress queue
and keyed by `(sourceGeneration, requestGeneration, albumId, photoId)`. They
always settle after send. Their settlement handle captures both the Album
source owner and current Photo owner. Answered `404` and `409` are
stale-position results and do not change recovery state. Any other answered
non-success—including other `4xx` and `5xx`—and transport failure adds a
saved-position Recovery claim only while the same source generation, Photo
generation, Album, and Photo remain current. A settlement from an older Photo
is silent; saved-position writes never fallback to the Library summary.

## Composite Workflows

A workflow is only composition; each child retains its assigned policy. A
presentation workflow captures its parent owner before starting and rechecks
that owner after every `await` and before sending each later child. Superseded
presentation workflows start no further children. A shared overview that
successfully commits may separately elect a bootstrap workflow as described
below; that is a new child of the committed shared state, not continuation of
a superseded presentation workflow.

- `loadOverview` and Retry own a foreground presentation ticket in the
  application scope. They start a shared overview refresh without cancelling
  older overview work. Only the newest pending load presents load failure or
  creates a reload Recovery claim. Startup follow-up, however, belongs to the
  newest successfully **committed overview**, not the newest request: when a
  response commits and the application still lacks its initial source, that
  commit elects a bootstrap owner which starts or awaits independent root
  binding and then may start source-scoped Browse open. A newer failed request
  cannot strand an older valid committed response. Every first committed
  overview ensures the single application-lifetime publication-status monitor
  is running; an unpublished commit withholds Browse opening until publication,
  while a published non-idle commit remains browsable and the same monitor
  reports its background scan. Every follow-up rechecks the elected commit
  sequence and application lifetime before starting its next child.
- Opening a Photo creates a new Photo scope and aborts Grid-speculative work.
  If the Photo lies outside retained facts, it starts a high-priority,
  Photo-owned Browse-window read; it never coalesces with an aborted adjacent
  request. If still current, it renders facts and starts current Preview. It
  sends saved position only while still current immediately before send; once
  sent, that write always settles.
- Reopening an expired source is source-owned. It may await independent root
  binding, then starts Browse open, a high-priority source window, optional
  Photo-owned Preview, and conditional saved position. Each child starts only
  while the source and optional Photo owner remain current.
- Retrying a Photo creates a new Photo owner, invalidates the current aligned
  bounded Browse-window facts, reloads that range at high priority in the
  Photo scope, and verifies source generation and Photo identity. Only then
  does it revalidate current Preview and conditionally persist Album position.
  It reports Connected only if the window, Preview, and required position
  write all succeed under the same owners.
- An Album mutation consists of the admitted write followed after success—and
  only while the application still exists—by an owner-tagged shared overview
  refresh. UI-surface supersession does not prevent shared overview commit.
- A form submit consists of form-continuation ownership plus the admitted
  Album write. Destroying or replacing the form detaches the form continuation
  but never aborts the write.

## Notice and Teardown Rules

The Library summary has one **Summary Notice Channel** shared by Album,
File Location, overview, status, scan failure/completion, failed-range
recovery, and explicit reload. An operation that may own a notice receives an
opaque claim handle containing a monotonically increasing ticket, owner kind,
and exact owner key (for example a failed Folder range). A non-owning
background HTTP request captures an opaque **background epoch** from the same
monotonic sequence when that request starts. A lifetime monitor therefore uses
a fresh epoch for every status request, and each overview request does the
same. Persistent claims are identified by their full handle; within the same
priority, only a newer ticket replaces the visible owner.

Actionable scan failure/completion, File Location and range-retry notices, and
explicit reload own the highest priority. Album failures are fallback notices.
Ordinary overview and in-progress status are background updates. A background
write is accepted only while no persistent owner exists, its epoch is not
older than `reloadBarrierTicket`, and it is not older than the last accepted
background epoch. Rejection returns a stale/no-op result and performs no DOM,
owner, pending, barrier, or Recovery change. An accepted background write
updates the last background epoch but never acquires ownership. A
lower-priority settled failure
blocked by a higher-priority owner is retained as pending and is presented
when that owner releases, unless an explicit reload ticket invalidated all
older pending notices. This prevents a late Album failure from erasing a newer
File Location retry while still preserving admitted-write failure visibility.

A persistent owner is released only by presenting its exact claim handle or by
a newer operation authorized for that owner kind. A File Location retry
success therefore releases only its exact failed-range owner. A newer Album
success may release an already displayed older Album notice, but it does not
predeclare an in-flight older write successful; if that older write later
fails, it may claim an otherwise free channel.

An explicit reload atomically advances a persistent `reloadBarrierTicket`,
invalidates and removes every active or pending handle older than that barrier,
and then claims the channel before its first request. Every later claim,
pending fallback, background write, and release is rejected when its ticket is
older than the barrier, even after the reload's visible notice is released.
No invalidated owner remains present to block background writes or await an
impossible release. Application teardown invalidates all claim handles.

If the initiating source, Photo, or form disappears after an Album write is
sent, success stays locally silent and shared data still refreshes; failure
falls back through the Summary Notice Channel. If those surfaces disappear
after a Photo-state, Undo, or saved-position write is sent, both success and
failure are locally silent, while the write still settles. These different
failure destinations are intentional product contracts.

### Connectivity and recovery ownership

Transport reachability and permission to make Photo decisions are separate.
The application has one **Recovery Gate** containing opaque claims keyed by the
operation and UI owner that requires revalidation (Folder range, source
snapshot, Photo generation, write, or scan command). Every failure described
as disconnected adds a blocking exact claim. The retry action carries that
handle: exact-range retry for File Locations, source Retry for source/Album
failures, Photo Retry for current-window/Preview/Photo-write failures, and
Retry Library Check for scan admission. An unrelated successful request may
prove that the server is reachable, but only the designated recovery under the
same current owner releases the claim. Decision controls are enabled only when
no blocking current claim remains and the current bounded source and Photo
facts have been confirmed.
Changing source or Photo creates a transition lineage. Claims from owner A
become predecessor claims for the in-progress owner B: they cannot present into
B, but they keep decisions blocked during establishment. If B establishes its
bounded current facts successfully, that exact transition retires A's
predecessor claims and any B establishment claim. If B establishment fails, a
current B claim replaces the predecessor blocker and A's claims retire; a
later B retry or later successor releases only the then-current lineage. An
old A claim never reactivates if the UI later returns to the same logical Photo
under a new generation. Independent File Location claims remain outside this
lineage until their own range succeeds. Claim creation, replacement, and
release are generation-gated, so stale failure and success cannot change the
current gate.

Application teardown halts the application, File Location, source/Grid, and
Photo scopes; cancels timers; aborts abortable transports; removes pending
browser image sources; detaches every remaining read continuation and
root-binding waiter; invalidates Summary and Recovery handles; and starts
best-effort release for a known Browse token. Admitted writes continue to
settlement but perform no DOM, connectivity, notice, or follow-up overview
work after the application itself is gone.

## Ownership Module Contracts

The implementation provides one dependency-free ownership module containing
four orthogonal mechanisms:

- scoped tasks create and halt scopes; start keyed children with
  `abort-transport`, `detach-continuation`, or `commit-in-order` policy; pass an
  `AbortSignal` only to abortable operations; coalesce keys where specified;
  and run cleanup exactly once;
- settlement handles run admitted writes without cancellation and decide
  whether their captured surface may still present the result;
- the Summary Notice Channel issues opaque owning `(ticket, kind, key)` handles
  and a fresh non-owning background epoch per HTTP request from one monotonic
  sequence; atomically invalidates pre-barrier active and pending handles;
  retains a reload-barrier floor and last accepted background epoch;
  arbitrates persistent owners by priority and ticket; reports stale/no-op
  background writes; performs exact-handle release; and retains at most the
  newest eligible pending fallback per owner kind;
- the Recovery Gate creates and releases exact-owner revalidation claims,
  separately records transport reachability, and reports whether current
  decision facts are ready.

The module attaches generation metadata without defining feature-specific task
types and owns no DOM or protocol knowledge. Source, Photo, Folder parent,
Album, and form identifiers are supplied by callers as ownership keys.

## Options Considered

A query-cache library was rejected. Browse tokens, publication-coherent
File Locations, and bounded windows do not fit a generic cache model, and
query libraries do not solve admitted-mutation settlement races.

A full actor/state-machine runtime was rejected. The contracts require scoped
operations and settlement ownership, not a general workflow engine.

Keeping separate per-feature coordinators was rejected. The same lifecycle
policies would remain encoded in multiple incompatible forms.

## Verification

Focused automated coverage must prove:

- application teardown cancels the status timer, halts every read scope,
  releases root-binding waiters, removes pending Grid and Review image
  sources, suppresses late DOM/Recovery writes, and attempts release of a
  known Browse token;
- abortable source, Browse-window, Preview, thumbnail, and browser-image work
  cannot render or poison replacement elements after owner teardown;
- detached status and File Location responses may settle but cannot write
  after their key is superseded, while source and Photo changes leave the
  independent File Location scope running;
- the first committed overview starts exactly one application-lifetime status
  monitor for both published and unpublished Libraries; idle monitoring
  detects externally triggered scans; HTTP non-success and transport failure
  remain silent; a semantic failed scan retains the prior publication and owns
  Retry Library Check; and the retry command's terminal result and monitor
  transition consume one completion handle exactly once without changing an
  open snapshot;
- concurrent startup/explicit scan admissions run one application-owned leader,
  publish once, return one captured terminal status to all live waiters, and
  finish status accounting even when one or every HTTP waiter disconnects;
- every File Location, Browse open/window, current/adjacent Preview, and
  thumbnail failure follows its specified notice, retry, and Recovery route;
- unrelated File Location or background success cannot release a current
  Photo/source Recovery claim or re-enable decisions before bounded current
  facts are confirmed;
- an older overview success commits when a newer request fails and no data
  boundary intervened, while an overview started before an Album mutation or
  publication replacement cannot commit after that boundary;
- a valid older startup overview response that commits after a newer load
  fails is elected to complete root binding and initial source opening;
- an in-flight root bind is shared by waiters and reset releases all waiters;
- a stale Browse-open token is released when known, with server expiry as the
  fallback when the token is unknown;
- Summary claim handles prevent stale cross-family overwrite, exact-range
  release cannot clear another owner, eligible pending Album failures surface,
  and reload atomically removes active and pending pre-barrier claims before
  rejecting older claims, releases, pending fallbacks, and non-owning
  background epochs after the visible reload notice clears;
- every status and overview HTTP request captures a fresh background epoch, so
  two successive monitor writes remain eligible around an intervening newer
  background writer while genuinely late settlements are stale no-ops;
- Recovery claims are released only by exact current-owner recovery, stale
  failure or success cannot alter the gate, and an A→B transition where B also
  fails replaces A's predecessor blocker with B without leaving an invisible A
  claim or permitting either stale owner to change controls;
- Photo opening promotes an aborted adjacent boundary window to a new
  high-priority Photo-owned request;
- moving an adjacent prefetched Photo to current causes the shared Preview
  service job to be deduplicated and promoted to current priority;
- Photo Retry invalidates and reloads the current bounded window before
  Preview revalidation and position persistence;
- a saved-position failure from Photo A is silent after Photo B becomes
  current in the same Album;
- superseded composite workflows start no child after losing their parent
  owner, except that a successfully committed overview may be elected as the
  bootstrap owner after a newer request fails;
- admitted Album writes and scan commands settle after their initiating
  surface disappears with the routing defined above;
- current and stale Album, Photo-state, Undo, and saved-position responses each
  retain the specified `404`, `409`, other `4xx`, `5xx`, and transport
  Recovery behavior;
- admission keys prevent duplicate same-key writes across re-renders without
  blocking independent keys;
- snapshot and publication coherence remain unchanged.

The complete local verification gate must remain green after implementation.
