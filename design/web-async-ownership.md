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
A key always includes the generation needed to prevent one source or Photo
from superseding another accidentally.

A **settlement handle** belongs to non-abortable work. It captures the
initiating generation and presentation surface, runs the request to
settlement, and decides whether the result may still write locally, must
fallback to the Library summary, or stays silent.

A generation is ownership metadata captured by an operation. It is not a
separate operation family.

## Operation Assignment

### Publication status polling

`GET /api/status` is a presentation read owned by the publication-status
scope. Starting a new publication poll detaches the preceding loop and its
in-flight response; transport need not be aborted. A detached poll never
writes scan status or starts an overview load. Background scan status writes
only while no write-failure notice owns the Library summary. Reaching `idle`
starts the `loadOverview` workflow described below.

### Shared overview refresh

`GET /api/overview` is a shared-state read with the global `overview` key and
**commit-in-order** policy. A newer request does not abort or detach an older
one. Every request receives a monotonically increasing client sequence, and a
response commits only when its sequence is newer than the last committed
response. A newer request that fails does not invalidate an older successful
response. The owner action, when present, controls only summary-notice and
connectivity presentation; it does not replace commit ordering.

### File Location windows and root binding

`GET /api/file-locations` is a presentation read with key
`(fileLocationGeneration, parent)`. A same-parent request detaches the
preceding continuation; its transport may finish, but its response cannot
render, clear a retry state, or replace the parent's current page. Changing
File Location publication generation detaches every request from the old
publication.

The unbound root uses the distinct key
`(fileLocationGeneration, root-binding)`. Exactly one operation owns that key;
other callers await its settlement instead of starting or cancelling a second
bind. Resetting the File Location generation releases those waiters with an
unbound result. Root binding is not superseded by ordinary child-window work.

### Browse open and reopen

`POST /api/browse` is a presentation read that allocates a bounded server
snapshot. It is owned by the source scope, keyed by the source generation,
and uses **abort-transport** policy when a newer source supersedes it. If a
completed stale response exposes a token, the client starts a token-release
operation. If aborting loses the response after server allocation, bounded
server expiry remains the cleanup fallback.

A Folder Browse open additionally captures the bound File Location
publication. It never sends without that publication, and publication expiry
starts a new root-binding operation before a retry.

### Browse windows

`GET /api/browse/{token}` is a presentation read owned by the source/grid
scope with key `(sourceGeneration, alignedStart)`. Requests with the same key
coalesce. A source change aborts every window request from the old source.
Current-window work has higher priority than adjacent work, but priority does
not alter ownership. A response commits only to its captured source
generation and token.

### Browse-token release

`DELETE /api/browse/{token}` is a lifecycle release keyed by token. It is
best-effort detached cleanup: UI teardown does not abort it, it presents no
success or failure notice, and it never changes connectivity. Bounded server
expiry is the fallback if the request cannot complete.

### Current Preview and adjacent Preview

The current Preview read is owned by the Photo scope with key
`(requestGeneration, photoId, current)`. Leaving the Photo, changing Photo, or
changing source aborts its transport and prevents every continuation after an
`await` from rendering, persisting position, or updating controls.

Adjacent Preview reads use key `(requestGeneration, photoId, adjacent)`, run
at low priority, and are best effort. The Photo scope aborts them on Photo or
source change. Their failures are silent.

### Thumbnail reads

Thumbnail reads are owned by the source/grid scope with key
`(sourceGeneration, photoId)`. Requests with the same key coalesce. Source
change aborts the old source's work. Only the captured generation may attach a
thumbnail or mark it unavailable.

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

Answered conflicts (`404` and `409`) do not change connectivity. Transport
failures and service failures (`5xx`) change connectivity only while the
operation still owns the global Album settlement family.

### Photo-state persistence and Undo

Selection, Rating, and Undo are admitted writes serialized by the current
Photo interaction (`busy` permits one at a time). Their settlement ownership
key is `(sourceGeneration, photoId, field)`. They always settle after send.
If source generation or Photo identity changes, their success and failure
continuations are silent; they do not fallback to the Library summary. A
current answered conflict reports on the Photo surface and may require a
refresh; a current transport failure changes connectivity. Undo follows the
same ownership rule as the Photo-state write it reverses.

### Saved Album position

Saved-position writes are admitted writes serialized by the progress queue
and keyed by `(albumId, photoId)`. They always settle after send. An answered
`404` or `409` is a stale position write and does not change connectivity. A
transport failure changes connectivity only while the same Album source
generation remains current. A stale failure is silent; saved-position writes
never fallback to the Library summary.

## Composite Workflows

A workflow is only composition; each child retains its assigned policy.

- `loadOverview` and Retry consist of a shared overview refresh, optional root
  binding, and an optional Browse open. Repeating the workflow supersedes its
  presentation failure surface but does not cancel an overview response that
  may still commit.
- Opening a Photo consists of a Browse-window read, a current Preview read,
  and—only if the operation is still current before send—a saved-position
  write. Once sent, that write always settles.
- Reopening an expired source consists of optional root binding, Browse open,
  Browse-window read, optional Preview read, and conditional saved-position
  write.
- Retrying a Photo consists of a current Preview read followed, while still
  current, by a saved-position write.
- An Album mutation consists of the admitted write followed after success by
  an owner-tagged shared overview refresh.
- A form submit consists of form-continuation ownership plus the admitted
  Album write. Destroying or replacing the form detaches the form
  continuation but never aborts the write.

## Notice and Teardown Rules

A standing Album failure owns the Library summary until a newer Album action
settles successfully, a newer Album failure replaces it, or the user
explicitly reloads. Background polling and unowned overview refreshes do not
erase it. Actionable File Location recovery and failed-range retry messages
explicitly retake the summary because they require immediate user action.

If the initiating UI disappears after an Album write is sent, success stays
locally silent and shared data still refreshes; failure falls back to the
Library summary. If the initiating UI disappears after a Photo-state, Undo,
or saved-position write is sent, both success and failure are locally silent,
while the write still settles. These different failure destinations are
intentional product contracts.

## Scope and Settlement Primitive

The implementation provides one dependency-free task primitive with these
capabilities:

- create and halt a scope;
- start a keyed child with `abort-transport`, `detach-continuation`, or
  `commit-in-order` policy;
- pass an `AbortSignal` only to operations declared abortable;
- coalesce identical keys where specified;
- run cleanup exactly once;
- expose a non-abortable settlement handle for admitted writes;
- attach generation metadata without defining feature-specific task types.

The primitive owns no DOM or protocol knowledge. Source, Photo, Folder parent,
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

- abortable source, Browse-window, Preview, and thumbnail work cannot render
  after owner teardown;
- detached status and File Location responses may settle but cannot write
  after their key is superseded;
- an older overview success commits when a newer request fails, while an
  older response cannot revert a newer committed response;
- an in-flight root bind is shared by waiters and reset releases all waiters;
- a stale Browse-open token is released when known, with server expiry as the
  fallback when the token is unknown;
- admitted Album writes settle after source, Photo, or form teardown with the
  notice routing defined above;
- stale Photo-state, Undo, and saved-position failures remain silent, while
  current failures retain their endpoint-specific connectivity behavior;
- admission keys prevent duplicate same-key writes across re-renders without
  blocking independent keys;
- snapshot and publication coherence remain unchanged.

The complete local verification gate must remain green after implementation.
