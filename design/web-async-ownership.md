# Web Async Ownership

The Web client starts asynchronous work whose settlement may outlive the UI
state that initiated it. A stale operation must not revert newer shared data,
write into a newer source or Photo, erase a newer failure notice, or change
connectivity after a newer operation proved the server reachable.

Async ownership has two independent dimensions:

1. the operation's **effect**: presentation read, shared-state read, lifecycle
   release, or admitted persistence;
2. its **supersession policy**: abort transport, detach continuation, commit in
   order, or always settle.

A read is not automatically abortable, and a write is not automatically tied
to the lifetime of its initiating UI. Workflows decompose into child
operations; they never acquire an implicit fifth policy.

## Design Drivers

- Admitted persistence is never aborted. Once a mutation request is sent, the
  server may commit it; aborting the fetch blinds the client without rolling
  the server back.
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
the global `publication-status` key. Starting a new publication poll detaches
the preceding timer, loop, and in-flight continuation; transport need not be
aborted. Application teardown does the same. A detached poll never writes
scan status or starts an overview load.

An answered non-success or transport failure is silent, does not change
connectivity, and leaves the current loop polling. A successful response may
write scan progress only as a background update through the Summary Notice
Channel. Reaching `idle` starts `loadOverview` only while the poll still owns
its key.

### Shared overview refresh

`GET /api/overview` is a shared-state read owned by the application scope,
with the global `overview` key and **commit-in-order** policy. A newer request
does not abort or detach an older one. Every request receives a monotonically
increasing client sequence, and a response commits only when its sequence is
newer than the last committed response. A newer request that fails does not
invalidate an older successful response. Application teardown detaches every
overview continuation so no response writes into a destroyed application.

Overview failure has no notice or connectivity policy of its own. Its live
parent supplies a Summary Notice ticket and decides whether failure is a
foreground reload failure, a persisted-Album-but-refresh-failed notice, or a
silent recovery probe. The owner action controls only that presentation; it
does not replace commit ordering.

### File Location windows and root binding

`GET /api/file-locations` belongs to an independent File Location scope with
key `(fileLocationGeneration, parent)`. Source, Grid, and Photo changes neither
abort nor detach it. Only same-parent supersession, File Location generation
reset, or application teardown may detach it. Its transport may finish after
detachment, but its response cannot render, clear retry state, or replace the
parent's current page.

An answered publication conflict (`409`) resets the File Location generation,
refreshes overview as a silent recovery probe, rebinds the root, and claims an
actionable summary notice only after the new root is coherent. Any other
answered failure, malformed response, or transport failure for the current
key retains loaded siblings, records the exact failed range, claims the
actionable summary notice, exposes that range's retry control, and changes
connectivity to disconnected. Only success for that same range clears its
retry state and releases its notice; success for any File Location window may
restore connectivity but cannot clear another range's notice.

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
change connectivity.

### Thumbnail reads

Thumbnail reads are owned by the source/Grid scope with key
`(sourceGeneration, photoId)`. Requests with the same key coalesce. Source
change or application teardown aborts old work. Only the captured generation
may attach a thumbnail or mark it unavailable. Answered and transport failures
leave a stable unavailable placeholder; they present no summary or
connectivity failure.

### Album persistence

Album create, rename, delete, membership add, and membership remove are
admitted writes. They share one global settlement family key,
`album-mutation`, because a newer Album action owns Album notices and
connectivity presentation regardless of which Album it targets.

The write always settles. Its successful shared-data refresh is a separate
owner-tagged overview operation and therefore follows overview commit order.
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
and keyed by `(albumId, photoId)`. They always settle after send. Answered
`404` and `409` are stale-position results and do not change connectivity. Any
other answered non-success—including other `4xx` and `5xx`—and transport
failure changes connectivity only while the same Album source generation
remains current. A stale failure is silent; saved-position writes never
fallback to the Library summary.

## Composite Workflows

A workflow is only composition; each child retains its assigned policy. A
workflow captures its parent owner before starting and rechecks that owner
after every `await` and before sending each later child. Superseded workflows
start no further children.

- `loadOverview` and Retry own a foreground load ticket in the application
  scope. They start a shared overview refresh without cancelling older
  overview work. If still current after it commits, a published Library starts
  or awaits independent root binding and then may start a source-scoped Browse
  open. An unpublished Library starts the keyed publication-status poll. Only
  the newest load presents load failure or changes connectivity.
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
File Location, overview, status, failed-range recovery, and explicit reload.
Every operation that may write it receives a monotonically increasing ticket
when initiated. Persistent claims have an owner kind and ticket; within the
same priority, only a newer ticket replaces the owner.

Actionable File Location and range-retry notices and explicit reload own the
highest priority. Album failures are fallback notices. Overview and status are
background updates and may write only while no persistent owner exists.
Background writes never acquire ownership. A lower-priority settled failure
blocked by a higher-priority owner is retained as pending and is presented
when that owner releases, unless an explicit reload ticket invalidated all
older pending notices. This prevents a late Album failure from erasing a newer
File Location retry while still preserving admitted-write failure visibility.

A persistent owner is released only by its own successful recovery or by a
newer operation authorized for that owner kind. A File Location retry success
releases only its exact failed-range owner. A newer Album success may release
an already displayed older Album notice, but it does not predeclare an
in-flight older write successful; if that older write later fails, it may
claim an otherwise free channel. An explicit reload claims the channel before
its first request and acts as a barrier against every earlier ticket.

If the initiating source, Photo, or form disappears after an Album write is
sent, success stays locally silent and shared data still refreshes; failure
falls back through the Summary Notice Channel. If those surfaces disappear
after a Photo-state, Undo, or saved-position write is sent, both success and
failure are locally silent, while the write still settles. These different
failure destinations are intentional product contracts.

Application teardown halts the application, File Location, source/Grid, and
Photo scopes; cancels timers; aborts abortable transports; detaches every
remaining read continuation and root-binding waiter; and starts best-effort
release for a known Browse token. Admitted writes continue to settlement but
perform no DOM, connectivity, notice, or follow-up overview work after the
application itself is gone.

## Ownership Module Contracts

The implementation provides one dependency-free ownership module containing
three orthogonal mechanisms:

- scoped tasks create and halt scopes; start keyed children with
  `abort-transport`, `detach-continuation`, or `commit-in-order` policy; pass an
  `AbortSignal` only to abortable operations; coalesce keys where specified;
  and run cleanup exactly once;
- settlement handles run admitted writes without cancellation and decide
  whether their captured surface may still present the result;
- the Summary Notice Channel issues monotonic tickets, arbitrates persistent
  owners by priority and ticket, and retains at most the newest eligible
  pending fallback per owner kind.

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
  releases root-binding waiters, suppresses late DOM/connectivity writes, and
  attempts release of a known Browse token;
- abortable source, Browse-window, Preview, and thumbnail work cannot render
  after owner teardown;
- detached status and File Location responses may settle but cannot write
  after their key is superseded, while source and Photo changes leave the
  independent File Location scope running;
- status non-success and transport failure remain silent and polling, and the
  unpublished `loadOverview` branch starts only one current poll;
- every File Location, Browse open/window, current/adjacent Preview, and
  thumbnail failure follows its specified notice, retry, and connectivity
  route;
- an older overview success commits when a newer request fails, while an older
  response cannot revert a newer committed response;
- an in-flight root bind is shared by waiters and reset releases all waiters;
- a stale Browse-open token is released when known, with server expiry as the
  fallback when the token is unknown;
- Summary Notice tickets prevent stale cross-family overwrite, preserve
  actionable recovery precedence, reveal eligible pending Album failures, and
  make explicit reload a barrier;
- Photo opening promotes an aborted adjacent boundary window to a new
  high-priority Photo-owned request;
- Photo Retry invalidates and reloads the current bounded window before
  Preview revalidation and position persistence;
- superseded composite workflows start no child after losing their parent
  owner;
- admitted Album writes settle after source, Photo, or form teardown with the
  notice routing defined above;
- current and stale Album, Photo-state, Undo, and saved-position responses each
  retain the specified `404`, `409`, other `4xx`, `5xx`, and transport
  connectivity behavior;
- admission keys prevent duplicate same-key writes across re-renders without
  blocking independent keys;
- snapshot and publication coherence remain unchanged.

The complete local verification gate must remain green after implementation.
