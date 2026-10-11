# Command-Line Architecture

Slipstream needs a machine interface that exposes its domain capabilities
without coupling an external Agent to browser state or duplicating persistence
rules. Human and machine clients can act concurrently, and a failed transport
must not manufacture certainty about a committed write.

[Command-Line Use](spec.md) owns product behavior.
[CLI Reference](reference.md) owns common grammar, envelopes, and
Library/peer commands. [darktable CLI](../photo-development/darktable/cli.md) alone owns
darktable grammar and its machine contract;
[darktable Editing](../photo-development/darktable/editing.md) owns product editing behavior.

## Model and Ownership

The `slipstream` client owns command parsing, connection selection, input
validation, bounded HTTP exchange, local Preview output, and presentation of
results. It is a new Rust workspace binary, independent of the server's native
image-processing dependency graph. `slipstream-server` remains the service and
offline operator executable; its existing startup and expansion commands do
not move into the user client.

The existing Rust application, Library, Preview, Album, and persistence owners
remain authoritative. Web and CLI commands converge on those owners. Domain
modules do not depend on CLI argument types, HTTP JSON, or Agent runtimes.

```text diagram
Photographer -> Web --------------------+
                                        |
Photographer -> external Agent -> CLI ---+-> HTTP -> domain owners -> SQLite
                                                       |
                                                       +-> read-only Originals
```

A query continuation is a temporary transport reference. A Library mutation version remains owned by the Library persistence owner. A darktable state token is a precondition on canonical tool state; saved revision identifies retained intent; request ID identifies an attempted effect. None is Photo identity. No Agent task model or general workflow engine is required.

The optional full `slipstream darktable` namespace resolves a Photo Workspace and invokes the independent proposed `darktable-edit` tool against its actual input/state directory. Standalone semantic operations need no Slipstream IDs, HTTP, credentials or database and may edit arbitrary local inputs. Native parameter parsing/serialization stays private to the tool. Exact Library Locations resolve to Photo IDs only for opening a workspace; paths become the independent tool interface thereafter. Generic processing routes remain qualified adapter projections and historical access, never a second canonical parameter store or darktable bypass.

The [Photo Workspace Architecture](../photo-development/workspace/design.md) owns preparation, containment, file revisions/indexing and retention/cutover. The tool owns initialized parameters, instances/masks/actions, output settings, catalogs/dependencies, durable state revisions and its per-state lock. Slipstream indexes a state pointer/status, invokes qualified adapters and supervises service jobs; it does not mirror editable parameters as an independent database authority.

## Client Implementation Boundary

Use established Rust libraries for command parsing and HTTP/TLS, with serde
for typed JSON. Select narrow features and verify the resulting client links
without LibRaw, libvips, the persistence owner, or server startup code. Avoid
creating a shared SDK or code-generation pipeline before a second real client
requires it. Small wire types may be client-owned; executable fixtures detect
contract drift.

The CLI must validate complete mutation input before sending it. The service
must independently validate input before admitting a write. Client validation
is convenience, not enforcement. Both consume the same contract examples in
their tests; this does not require sharing domain implementation code.

Transport performs no transparent write retry and follows no redirects. A
whole-command deadline bounds all requests except the resumable Processing
Artifact transfer, which instead bounds each control request and forward
progress through its idle timeout. Signal handling cancels reads and
local downloads and reports mutation uncertainty when admission is possible;
it never equates a dropped connection with server cancellation.

## HTTP Capability Boundary

Preserve the existing Web HTTP routes and deployment health boundary. Add only
the missing representations and checked operations. The target additions are:

- `GET /api/capabilities`: supported CLI contract versions and published limits;
- `GET /api/album-summaries`: bounded Album listing and exact-name or Photo
  membership lookup, with continuation;
- `GET /api/albums/{id}`: one current Album summary and mutation version;
- `POST /api/photo-queries` and `GET /api/photo-queries/{token}`: create and page
  a fixed matching set with current facts;
- `GET /api/photos/{id}`: one Photo's current facts and decision version;
- `POST /api/photo-decisions`: checked bounded Selection State or Rating changes;
- `POST /api/albums/{id}/changes`: one typed, version-checked rename, delete,
  member addition/removal, or complete reorder;
- `POST /api/recovery/unavailable` and `GET /api/recovery/unavailable/{cursor}`:
  open and page one bounded review of active unavailable Photos with current facts;
- `POST /api/recovery/propose` and `GET /api/recovery/proposals/{cursor}`:
  evaluate one read-only mapping or one Folder-prefix review, with continuation;
- `POST /api/recovery/apply`: commit one explicit batch of reviewed mappings.

These routes must map to shared owners, not a parallel CLI business layer.
Album creation, Folder windows, scan status/check, capture metadata, Preview
requests, and derivative download reuse their existing routes. Preserve retired
routes as retired; do not revive the unbounded `GET /api/albums` representation.

### Photo Development Surface

The [CLI Reference](reference.md#photo-workspace) owns workspace grammar/results and [darktable CLI Reference](../photo-development/darktable/cli.md) owns both entry points' semantic language. Workspace open supplies one actual directory/input snapshot, suggested state path, explicit preview/output directories and truthful execution host/access facts. A remote client cannot execute a standalone process using a server-only path; use an authorized filesystem-accessible environment or available service supervision. No arbitrary shell endpoint, checkout/sync protocol or invisible mount is added.

Workspace transport exposes bounded open/show/list/inspect/download/remove operations on admitted workspace/artifact identities. It never accepts arbitrary host paths as Library identity. Independent tools see ordinary files and explicit directories rather than this transport. Final file discovery does not require callback, registration, manifest, upload or commit. Ordinary folder/type role inference cannot fabricate processing success/provenance. [Completion and File Discovery](../photo-development/workspace/design.md#completion-and-file-discovery) defines reconciliation and optional verified invocation metadata. [Stable Bytes and Containment](../photo-development/workspace/design.md#stable-bytes-and-containment) governs fingerprints, changed external files, protected captures, transfers and safe removal.

Selective removal uses that same workspace admission coordination and canonical
tool-use locks, not a separate cleanup system. Protected snapshots are read-only
to tools but explicitly removable when no retained state/job/output/dependency or
active/unverified use remains. New binding admission and removal serialize, so
neither can create a dangling input. Removing the default clears the default
association; removing the last leaves no input. Reopen must not read the Original,
recreate an explicitly removed snapshot, or select another file. Explicit refresh
may create a new identity without changing the old removal receipt.

Service supervision dispatches the same independent executable/state as local editing, with explicit directories and qualified typed semantic input. It captures the exact current or requested retained state revision, protected input evidence, resources, engine/bundle and output before queue admission. A later edit cannot retarget a job; a stale late preview cannot replace the latest relevant rendition. Standalone render/export is synchronous and owned by its process. Only an accepted durable supervised job/receipt justifies continued service work after HTTP disconnect. Cancellation/timeout cannot publish partial files or discard saved adjustments/previous completed results.

Both paths use the same per-state filesystem lock and atomic state replacement. GUI and Agent read-modify-write, full atomic batches, reorder, rebind, restore and mapped catalog application require observed state evidence. Focused no-guard commands express fresh intent validated wholly under the lock; they cannot overwrite caller-constructed full state. Supplied guards are never refreshed by adapters. Request IDs default to generated returned values; an explicit caller-retained ID permits exact uncertain-outcome replay. Receipts replay the original settled effect before current-state admission and refuse payload reuse. Auto actions commit one validated candidate or nothing and cannot escape the same revision checks.

The existing generic route names remain where compatible:

- `/api/processing/modules` discovers qualified tools/adapters;
- `/api/photos/{id}/processing-recipe` and its save/rebind forms project retained identities and canonical on-disk state without dual writes;
- `/api/photos/{id}/processing-preview/{step_id}` projects a captured workspace preview and optional byte transfer;
- `/api/photos/{id}/processing-exports` and request-specific status/cancel/retry expose supervised jobs and retained snapshots;
- `/api/processing-artifacts/{id}` and `/bytes` expose verified completed artifact evidence and bound bytes;
- `/api/exports/{id}` and `/artifact` retain truthful historical metadata/bytes;
- Development Proxy read/create/remove remains an explicitly bounded cache surface, never the workspace input or authority to substitute a missing binding.

The [retained exchange contract](reference.md#retained-processing-adapter-exchanges) owns exact generic bodies/results. Keep existing Original and immutable Processing Artifact variants and add only `workspace-artifact`, with workspace/artifact identity, observed content revision and verified image contract. Overloading the old immutable artifact variant would erase external revision invalidation; forcing every independent file into verified processing provenance would invent producer facts. Proxy/legacy Export identities remain their own read-only/cache surfaces rather than new binding variants. A discovered compatible image can be selected with unknown provenance. The existing live sourceRevision string observation (empty when unavailable) remains echoable, but does not invalidate artifact-only work when unrelated Original evidence changes; guarded Original bindings remain exact and never retarget. Adapters check canonical state revisions under tool locks, preserve unsupported/unrelated state and selections, and refuse changes outside an atomically supported scope. All darktable mutation/execution stays in its full namespace. Generic projections are optional for peer tools and cannot become an obligatory handoff DSL.

Migration preserves historical identities, native state, XMP/image receipts and retained bytes non-destructively. A supported state is materialized once into tool-owned state, with retained source record/dependencies; unsupported versions remain inspectable rather than silently converted. Current needed input/state/output files persist until explicit safe removal; transient receipt/cache retention is separate. Incomplete historical provenance stays labeled historical. [Retention and Cutover](../photo-development/workspace/design.md#retention-and-cutover) is authoritative. Retired fixed routes remain retired; this boundary change does not delete retained data or purge compatible existing processing adapters.

Every CLI request identifies contract version 1 through
`Slipstream-CLI-Contract: 1`. A server that advertises version 1 must validate
that header on CLI requests, including reused mutation routes, before domain
admission. An unsupported value fails without a mutation. The client checks
capabilities before its command's operational request. An older service that
lacks capabilities is incompatible; the client must not guess based on HTML,
a health response, or a version string.

Capability decoding preserves valid advertised versions even when a required
field is missing or invalid. Version membership and representation validity are
separate checks. Refuse incompatible representations before domain admission
and identify the field in the diagnostic. Supporting an older partial service
by filling missing limits with defaults would hide missing operations; a matching
client/service candidate is the supported pairing.

HTTP does not expose the CLI process envelope on existing Web routes. The
client normalizes typed route results into the CLI envelope. New routes must
return structured error codes and domain references. For reused routes whose
legacy response cannot distinguish necessary cases, add structured fields for
CLI requests rather than parse English messages or silently change Web shapes.

The request/response fixtures for these routes must be derived from the wire
contract in this design and the service semantics in
[Photo Development Architecture](../photo-development/design.md#service-surface),
added with their implementation, and executed before any CLI release. No route
addition may bypass the existing method, header-size, body-size, or shutdown
admission rules.
The retired fixed Photo Development routes have no compatibility aliases.
The reviewed protocol also replaces the earlier unbounded recovery shapes.

### Reviewed Recovery Surface

Location Recovery keeps one Web and CLI protocol. The shared contract —
retained inspection, read-only proposals, confirmation bound to the reviewed
mapping identity, explicit retire choice, atomic batch application, and truthful
outcome rules — is owned by
[Physical File Locations and Virtual Albums](../photo-library/design.md#reviewed-location-recovery-access).
The [CLI Reference](reference.md#reviewed-location-recovery) alone
owns the command grammar, input documents, limits, result schemas, and exit
codes. The earlier Web-only unbounded recovery shapes are retired by this
design; the Web client moves to the reviewed protocol in the same change.

Reviews are bounded process state with the same lifecycle rules as query
collections: idle expiry, least-recently-used eviction under pressure, and an
explicit refusal instead of a partial or truncated result. An unavailable review
retains ordered Original and Photo identities and resolves current facts on each
page. A Folder-prefix review retains its evaluated mappings, because collision
detection needs the complete evaluated scope, and a later file must not join a
reviewed set. Cursors validate kind, process epoch, offset, and page size, and a
failed or expired review is reported as such rather than silently re-evaluated.

Apply revalidates every submitted mapping against current state inside the
existing persistence transaction. Refused batches report one reason per mapping
and change nothing. Applied results identify each recovered Photo and Original,
its previous and current Location, and any retired destination record, with the
Library-wide unavailable count reported separately.

A lost apply response is an unknown outcome. The client presents the submitted
mapping identities and directs the caller to inspect those Photo identities; it
does not replay the request, retry automatically, or report a current-state match
as proof that this attempt committed.

## Query Semantics and Resources

A Photo query evaluates source, filters, and global order against one coherent
published membership and decision read. It retains only the ordered matching
Photo IDs and bounded query metadata. It does not retain all Photo facts or
image bytes. Later windows fetch current facts and decision versions in one
serialized owner read. Missing IDs retain their positions as specified by the
CLI reference.

Use the existing Browse Snapshot mechanism where its ownership matches, with
an explicit query kind so Web source snapshots and CLI filter queries cannot
be confused. Query creation must not set Album progress. Do not encode Rating
filters into the browser's Selection State filter model.

Album-list queries use the same fixed-ID/current-fact principle over Album
IDs. Folder navigation retains the existing publication-bound contract and
creates no retained query collection. Its continuation carries the publication,
parent, range, and page size opaquely. It has no idle deadline and does not
participate in query eviction, but expires whenever the bound publication is no
longer current, including after server restart publishes a new identity. Photo
and Album query continuations do not expire merely because a new scan publishes.

All temporary query collections, including existing Web snapshots, share a
bounded process budget. Preserve the existing per-window bound and process
snapshot count; enforce an aggregate maximum of 1,000,000 retained IDs across
Web, Photo, and Album query collections. Expire idle collections and evict
least-recently-used collections under pressure. A single new result exceeding
the total budget fails before publishing a continuation. Query construction
must stop at the budget instead of allocating an unbounded result first. The
[CLI Reference](reference.md#service-and-discovery) owns the
observable error and recovery behavior for this failure.

Photo and Album cursors encode an opaque retained-query reference and bounded
next position; the server validates kind, process epoch, offset, and page size.
Folder cursors encode an opaque publication reference, parent, bounded next
position, and page size. No cursor contains an absolute path or mutable query
text supplied as an executable expression. Tampered or wrong-kind cursors fail
as invalid input. A replaced Folder publication and expired or evicted query
references produce `cursor_expired`. Time expiry and `expiresAt` follow the CLI
reference.

Read-only query POST retries are semantically safe but can consume resources;
the client does not retry automatically. A lost query response leaves only a
bounded expiring server object. No persistent query store or query DSL is added.

## Mutation Versions

The persistence owner issues opaque versions containing a random process epoch
and a per-object counter. These counters are process-local guards, not durable
Photo facts. A fresh epoch invalidates all pre-restart versions, so losing
counters on restart cannot make an old token valid again. Reading fresh facts
is the recovery path; no schema migration is required for these guards.

A Photo counter changes after any effective Selection State or Rating change.
An Album counter changes after any effective rename, membership, or order
change. A no-op does not change a counter. Saved position, Preview facts,
metadata inspection, and scan progress do not change these counters.

All writers, including existing Web routes, batch decisions, Undo, Folder
addition, Album compensation, and recovery-related retirements, must participate
in the owner-controlled invalidation rules. This participation is a foundation
for any read that returns a decision or Album version: an earlier delivery slice
must not emit placeholder versions or postpone existing-writer invalidation until
a later mutation command ships. Record deletion returns missing; recreation uses
a new opaque domain ID. No client can supply the new version.

Precondition validation, reading prior values, applying mutations, and deciding
new versions run under the serialized persistence owner. Counter changes become
visible only after a successful SQLite commit and before the next owner command
can read. A rollback preserves versions. Reads return facts and versions from
that same owner operation, never from an independently updated HTTP cache.
Counter overflow must fail closed before a write rather than wrap.

Token comparison occurs before no-op detection. Changing a value away and back
still invalidates an older command. An expired process epoch is an ordinary
conflict that requires fresh facts. This contract guards the observed decision
or Album state; it does not claim historical identity of Original bytes.

## Mutation Settlement

Photo batches validate the entire request first. In one database transaction,
each requested Photo is classified and matching items are changed. Conflicts
and missing records are domain results, not transaction failures. Storage
failure rolls back all changes. Returned results are the transaction's facts,
not a later query that might observe another writer.

An Album mutation validates its version and all requested identities before
applying any member change. Rename, deletion, add, remove, and reorder are
atomic. The existing member-order and saved-position rules remain shared with
Web. A CLI command cannot supply an Album progress write as a side effect of
a Photo decision.

Neither a successful command nor its output starts an undo session. Prior
values in a Photo result help the caller formulate a later correction. The
caller must obtain current versions before that new write. Album compensation
uses ordinary checked member removal and cannot promise to restore old order.

This Photo decision and Album contract adds no durable command ledger,
idempotency-key table, or task transaction. Membership addition has set-like
no-duplicate behavior, but its version guard still rejects replay with an outdated
version. Album name uniqueness prevents duplicate names; it does not prove who
created an Album after a lost response. The separate Edit Recipe and Export
receipt rules remain owned by [Photo Development Architecture](../photo-development/design.md).

A complete and valid server error can establish a refusal or rollback. A
truncated, malformed, unexpected, or missing response after a possible send
cannot. Report `outcome_unknown`, including when the service responded with
untrusted non-JSON data. Return confirmed partial results only when every item
can be validated against the request and no duplicate or missing outcome exists.

## Preview Transfer and Local Files

The CLI obtains Preview metadata and a revision-bearing derivative reference,
then downloads that exact derivative. The derivative response must repeat the
Photo identity, source, source revision, and dimensions in typed metadata so the
client can compare them with the admitted request. Before publication, the client
requires JPEG content type, enforces the byte bound while streaming, validates a
complete JPEG structure, reads its encoded dimensions, and compares every repeated
fact. Use a maintained pure-Rust JPEG parser for this bounded validation; the
client does not decode pixels, buffer RAW, or run native image processing. The
service's existing image byte/pixel limits remain authoritative. If the source
changes before delivery, fail or request current metadata explicitly; never label
old bytes with new facts.

Download into an exclusively created temporary file beside the requested path.
Publish with a no-replace operation relative to the admitted parent directory,
so a racing file or symlink cannot be overwritten. Final no-replace publication
is the local commit point. Before it, cancellation or failure removes only the
temporary file owned by this invocation. After it, output or interruption failure
preserves the final JPEG and reports the committed local effect as specified by
the CLI reference; it never retries into an existing or different path. The
output path is local client data; it is not sent as a server Original Location.

### Resumable Processing Artifact Transfer

A verified artifact content revision binds one exact SHA-256/length pair. `GET /api/processing-artifacts/{id}/bytes` and workspace download expose that strong `ETag` and `Accept-Ranges: bytes`. A caller may request one contiguous range. Matching `If-Range` permits `206` with `Content-Range`; an invalid/unsatisfiable range returns `416` with full length and no stream. A stale resumable request may receive complete `200` bytes only for the same admitted revision, allowing the client to restart that transfer. If the path now holds a different workspace revision, return `artifact_changed` instead of silently switching its bytes. Protected managed files can reuse their stable descriptor/identity; mutable external files require stability checks and pinned evidence throughout transfer. Headers repeat the admitted revision and verified available provenance for successful responses.

The service acquires one finite artifact lease before opening the retained file.
The lease is renewed while the response body is live and released when the body
drains or is dropped. Ranges do not create a second Artifact, a Download Job,
or a server-side copy.

The CLI hashes the response incrementally. An interrupted Artifact download
keeps only a destination-private partial file and a small identity sidecar.
On retry, matching state is rehashed and resumed with one range; malformed or
mismatched state is discarded. The final no-replace publication remains the
local commit point, so a partial, short, extra, corrupt, expired, or
interrupted transfer never appears at the requested path. Artifact transfer
uses an idle-progress timeout rather than a fixed total transfer deadline.

Buffering the complete artifact is rejected because full-resolution TIFFs can
exceed client memory. A server-side copy or durable Download Job is rejected
because the retained immutable bytes and lease already provide the required
identity and lifecycle without another Artifact owner.

### Resumable Transfer Verification

The transfer boundary must prove bounded client memory, incremental digest
validation, private partial-state recovery after interruption and server
restart, correct range and `If-Range` behavior, no-replace publication, lease
protection until body settlement, and explicit expiry refusal. Both TIFF and
JPEG artifacts use the same identity and publication rules.

CLI Preview work uses the shared bounded scheduler as background demand below
an active Web Photo View. One CLI process sends one Preview request at a time;
server-side queue bounds apply across all clients. Web interaction must not
wait for a whole Agent analysis batch to finish. The CLI adds no full-Library
warmup or speculative neighbor generation.

## Web Handoff

Compose relative Destination URLs under the configured service origin using
[Browser Address Contract](../library-browser/experience/design.md#browser-address-contract).
Return a canonical Photo URL rooted in All Photos and an Album Grid URL. Do not
copy server-provided arbitrary origins into links. Unsupported CLI filters have
no matching Grid URL and must not be silently omitted from a supposed query link.

Links expose current state. Page refresh and ordinary requery reveal CLI changes;
this contract does not add live subscriptions or promise replacement of an open
Web source's fixed ID order. Fresh Web actions and CLI writes continue to share
the same serialized storage owner.

## Options

### Selected: Thin Rust Client of the Existing Service

This preserves one enforcement boundary and keeps native processing near
storage. The client can run beside an external Agent without the Photo Library
mounted locally. Established command/HTTP libraries keep the implementation small.

### Selected: Independent Tool with Same-Directory Slipstream Supervision

An operation catalog and one bounded semantic mutation primitive preserve multi-instance, curve, mask, action, preset/style and output capability without leaking native parsing. `darktable-edit` owns that deep interface and on-disk state. The complete `slipstream darktable` namespace is an optional Photo/workspace resolution and job-supervision adapter, invoking the same state/executable rather than passing edits between duplicate stores. Files directly connect tools; Slipstream provides browsing/indexing, not a mandatory recipe DSL.

### Rejected: Generic Full-Tree Editing or Per-Control Commands for darktable

Generic tree replacement makes clients initialize camera state and preserve
unseen masks, instances, and output settings. Per-control commands duplicate
native meanings and grow with every engine module. Native identifiers, typed
structured values, and catalogued actions keep the language small while the
engine owns algorithms and layout knowledge.

### Rejected: Database Editing or Per-Iteration Download/Submit Bridge

Direct edits of Slipstream database state would duplicate Library transactions and containment. Downloading RAW/state for every local adjustment and submitting results back would introduce synchronization/conflict work and block same-host composition. Independent tools instead access the real workspace directory and own their state; a thin service client still handles remote Library queries and optional final downloads. Direct filesystem access to a prepared workspace is deliberate, with no write access granted to Originals.

### Deferred: MCP Adapter

MCP can expose the same capabilities when an actual consuming Agent needs it.
Shipping two machine surfaces now doubles qualification and packaging work.
MCP is not a prerequisite for Agents that can invoke commands and read images.

### Rejected: Embedded Agent and Task Infrastructure

Conversation, planning, model choice, and visual reasoning belong to the user's
Agent. Persistent tasks, proposal review queues, and an execution engine do not
solve the selected query-and-organize workflow.

### Selected: Checked Writes and Explicit Reconciliation

Versions detect intervening changes, including changed-away-and-back values.
After transport uncertainty, object reads establish present state and the caller
chooses a next action. This is adequate for bounded photo-management commands.

### Deferred: Receipts for Photo Decisions and Album Mutations

For the selection and organization commands defined here, receipts could prove
historical attribution but introduce storage, retention, replay, and migration
contracts. They are unnecessary for current-state reconciliation rather than
exactly-once tasks or durable Undo. This choice does not replace the distinct
Edit Recipe and Export receipt requirements.

## Verification

The darktable acceptance scenario must resolve a Library Photo, open a real workspace and run standalone open/edit/preview/export on that host using explicit output directories. Exercise multiple instances, curves, masks, actions, resources, presets/styles and output geometry; prove the completed file appears through Slipstream reconciliation without registration and can be downloaded or passed directly to another compatible independent tool. Also run the same executable on arbitrary supported local input without Slipstream. Export before preview, reopen after restart, and preserve snapshot editing after the live Original becomes unavailable.

Exercise optional full `slipstream darktable` supervision against exactly the same state/directory. Cover guarded Web/Agent races, no partial invalid apply, bounded per-state busy refusal, exact request replay, prospective initialization, multiple-edit ambiguity, captured queued revisions, stale late previews and restore after explicit rebind. Terminate a standalone owning process and prove no detached guarantee/partial ready file; disconnect a supervised client and reconcile its durable job. Schema discovery or parser success alone does not prove this loop.

Qualification must execute the real CLI against the real Rust service with a
generated fixture Library. Contract fixtures cover help, parsing, inputs,
envelopes, exit codes, errors, Photo filters/order, continuation, and mutations.
They must be consumed by both transport and client tests where applicable.

Cross-client tests must cover Web changes between CLI read and write, changed-
away-and-back state, rollback preserving versions, restart invalidating versions,
missing objects, partial Photo batches, and atomic Album refusal. A writer
inventory matrix must exercise every effective Photo and Album writer named by
this contract, including Web routes, batch decisions, Undo, Folder addition,
Album compensation, and recovery-related retirements. Each effective writer must
advance the relevant version and make a previously read CLI version conflict.
The matrix must also prove that no-ops, saved-position writes, Preview-only,
metadata-only, and scan-only fact updates, rollbacks, and failed writes do not
advance decision or Album versions. A scan or recovery action that retires
membership is an effective Album writer and must advance the Album version. Lost
responses, malformed responses, interruption, and failed stdout must not produce
false success or automatic retries.

Development command tests must exercise the public composable commands
end to end: contract negotiation, module discovery, guarded recipe save and
rebind, selected-step bounded Preview, prompt Export acceptance, durable
list/status/cancel, captured retry, exact-body replay, and artifact download
validation against immutable provenance and digest. Source revisions must
exercise the separate opaque byte bound, including NUL.
The same tests must prove that a refused or unavailable step
never falls back to the Camera Preview or another module, that no command
converts or chains module inputs implicitly, and that a later edit or source
change makes prior renditions stale without retargeting an accepted Export.

Preview tests must cover readable JPEG Camera Preview and PNG selected-step
output, revision consistency, existing-file/symlink races, bounded bytes,
cancellation cleanup, shared queue pressure, and unchanged Originals. Linux qualification must also prove that a
non-UTF-8 `--file` value fails before any network request or temporary-file
creation. Web handoff tests must open CLI-returned URLs and verify the same
Photo/Album and current decisions. Large-Library and query-pressure tests must
prove bounded response sizes, continuation beyond one page, the aggregate
Web/Photo/Album retained-ID cap, construction that stops at the bound, idle and
least-recently-used eviction, and the specified cursor outcomes without changing
durable state. Tests may parameterize the production budget code with smaller
limits instead of allocating million-item fixtures.

The client release gate must also prove a clean Linux amd64 install without
native server libraries, reject incompatible services before writes, and verify
checksums and offline help. The canonical repository gate remains
`bun run verify`; exact new focused script names must be added to
`CONTRIBUTING.md` with implementation rather than invented as executable commands
in this design-only change.
