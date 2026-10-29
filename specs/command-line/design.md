# Command-Line Architecture

The CLI exposes shared domain capabilities without coupling Agents to browser state or duplicating persistence rules. Transport failure MUST NOT manufacture certainty about a write.

[Command-Line Use](spec.md) owns product behavior. [CLI Reference](reference.md) owns common grammar and machine results. [darktable CLI](../photo-development/darktable/cli.md) owns its standalone and integrated language.

## Model and Ownership

The `slipstream` Rust client owns parsing, connection selection, input validation, bounded HTTP exchange, local output, and result presentation. `slipstream-server` remains the service and offline operator executable; its startup and expansion commands remain there.

Web and CLI converge on the existing application, Library, Preview, Album, and persistence owners. Domain modules MUST NOT depend on CLI arguments, HTTP JSON, or Agent runtimes.

```text diagram
Photographer -> Web --------------------+
                                        |
Photographer -> external Agent -> CLI ---+-> HTTP -> domain owners -> SQLite
                                                       |
                                                       +-> read-only Originals
```

A query continuation is a temporary transport reference.

A Library mutation version belongs to the Library persistence owner.

A darktable state token guards canonical tool state, a saved revision identifies retained intent, and a request ID identifies an attempted effect; none is Photo identity.

[Photo Workspace Architecture](../photo-development/workspace/design.md) owns tool state, file access, supervision, and retention. [darktable CLI](../photo-development/darktable/cli.md) owns standalone execution and its optional workspace adapter.

## Client Implementation Boundary

The client MUST remain independent of LibRaw, libvips, the persistence owner, and server startup code. It uses established Rust command and HTTP/TLS libraries and typed JSON.

The client MUST validate complete mutation input before sending it. The service MUST independently validate it before admitting a write; client validation is not enforcement.

Transport MUST NOT transparently retry writes or follow redirects. [Invocation](reference.md#invocation) owns deadlines and transfer exceptions.

Signals cancel reads and local downloads; possible write admission remains uncertain and a dropped connection does not imply server cancellation.

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

These routes MUST map to shared owners, not a parallel CLI business layer.
Album creation, Folder windows, scan status/check, capture metadata, Preview
requests, and derivative download reuse their existing routes. Preserve retired
routes as retired; do not revive the unbounded `GET /api/albums` representation.

### Photo Development Surface

[Photo Workspace Architecture](../photo-development/workspace/design.md) owns the actual directory, independent tools, state locks, file discovery, and safe removal. [Photo Workspace](reference.md#photo-workspace) owns the CLI carriers and truthful host/access facts.

Service supervision captures the requested or current retained state revision, protected input evidence, resources, engine, bundle, and output before queue admission.

Later edits MUST NOT retarget that job, and stale late previews MUST NOT replace the latest relevant rendition.

Only an accepted durable supervised job or receipt justifies continued service work after HTTP disconnect. Standalone render and export remain synchronous process-owned work.

The existing generic route names remain where compatible:

- `/api/processing/modules` discovers qualified tools/adapters;
- `/api/photos/{id}/processing-recipe` and its save/rebind forms project retained identities and canonical on-disk state without dual writes;
- `/api/photos/{id}/processing-preview/{step_id}` projects a captured workspace preview and optional byte transfer;
- `/api/photos/{id}/processing-exports` and request-specific status/cancel/retry expose supervised jobs and retained snapshots;
- `/api/processing-artifacts/{id}` and `/bytes` expose verified completed artifact evidence and bound bytes;
- `/api/exports/{id}` and `/artifact` retain truthful historical metadata/bytes;
- Development Proxy read/create/remove remains an explicitly bounded cache surface, never the workspace input or authority to substitute a missing binding.

The [retained exchange contract](reference.md#retained-processing-adapter-exchanges) owns the generic bodies and results.

Original, immutable Processing Artifact, and revision-bound workspace-artifact bindings remain distinct; this prevents changed external bytes from masquerading as the old immutable artifact.

Generic adapters MUST preserve unrelated canonical state and selections and MUST refuse unsupported atomic changes. darktable mutations and execution remain in the full darktable namespace. [Workspace retention](../photo-development/workspace/design.md#retention-and-cutover) owns non-destructive migration.

### Contract Negotiation

Every CLI request identifies contract version 2 through `Slipstream-CLI-Contract: 2`. A version 2 server MUST validate that header before domain admission, including on reused mutation routes. An unsupported value fails without a mutation.

Version 2 retires the physical-deletion commands; a version 1 client is incompatible. The client checks capabilities before its operational request. A service without capabilities is incompatible; the client MUST NOT guess from HTML, health, or version strings.

Capability decoding preserves valid advertised versions even if a required field is missing or invalid. Version membership and representation validity are separate checks. Incompatible representations are refused before admission with the offending field identified.

Missing limits MUST NOT be filled with defaults to support an older partial service. A matching client/service candidate is the supported pairing.

HTTP does not expose the CLI process envelope on existing Web routes. The client normalizes typed route results into that envelope. New routes MUST return structured error codes and domain references.

When a reused legacy route cannot distinguish necessary cases, add structured fields for CLI requests. Do not parse English messages or silently change Web shapes.

Route additions MUST NOT bypass existing method, header-size, body-size, or shutdown admission rules. Retired fixed Photo Development routes MUST NOT have compatibility aliases.

### Reviewed Recovery Surface

Location Recovery uses one Web and CLI protocol. [Physical File Locations and Virtual Albums](../photo-library/design.md#reviewed-location-recovery-access) owns reviewed inspection, proposals, confirmation, retirement, atomic application, and outcomes.

The [CLI Reference](reference.md#reviewed-location-recovery) owns command grammar, input documents, limits, result schemas, and exit codes. Retired unbounded recovery shapes MUST NOT be revived.

Reviews are bounded process state. They expire when idle, undergo least-recently-used eviction under pressure, and refuse excess scope instead of returning partial or truncated results.

An unavailable review retains ordered Original and Photo identities and resolves current facts on each page. A Folder-prefix review retains evaluated mappings so collision detection covers the complete scope. Later files MUST NOT join a reviewed set.

Cursors validate kind, process epoch, offset, and page size. Failed or expired reviews are reported explicitly rather than silently re-evaluated.

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

A Photo query evaluates source, filters, and global order against one coherent published membership and decision read. It retains ordered matching Photo IDs and bounded query metadata, not all facts or image bytes.

Later windows fetch current facts and decision versions in one serialized owner read. Missing IDs retain their positions as specified by the CLI reference.

Use the existing Browse Snapshot mechanism where its ownership matches, with an explicit query kind separating Web source snapshots from CLI filter queries. Query creation MUST NOT set Album progress.

Rating and Selection State remain independent filter conditions. Addressed Destinations MAY carry both under the [filter contract](../library-browser/filters/spec.md); Rating MUST NOT be encoded as a Selection State.

Album-list queries use the fixed-ID/current-fact principle over Album IDs. Folder navigation retains its publication-bound contract and creates no retained query collection.

Folder continuations carry the publication, parent, range, and page size opaquely. They have no idle deadline and do not participate in query eviction. They expire when the bound publication is no longer current, including after restart publishes a new identity.

Photo and Album continuations do not expire merely because a new scan publishes.

All temporary query collections, including Web snapshots, share a bounded process budget. Preserve the existing per-window bound and process snapshot count. The aggregate maximum is 1,000,000 retained IDs across Web, Photo, and Album collections.

Expire idle collections and evict least-recently-used collections under pressure. A single new result exceeding the budget fails before publishing a continuation. Construction MUST stop at the budget rather than allocate an unbounded result first.

The [CLI Reference](reference.md#service-and-discovery) owns observable error and recovery behavior for this failure.

Photo and Album cursors encode an opaque retained-query reference and bounded next position. The server validates kind, process epoch, offset, and page size.

Folder cursors encode an opaque publication reference, parent, bounded next position, and page size. No cursor contains an absolute path or mutable query text as an executable expression.

Tampered or wrong-kind cursors fail as invalid input. Replaced Folder publications and expired or evicted query references produce `cursor_expired`. The CLI reference owns time expiry and `expiresAt`.

Read-only query POST retries are semantically safe but can consume resources;
the client does not retry automatically. A lost query response leaves only a
bounded expiring server object. No persistent query store or query DSL is added.

## Mutation Versions

The persistence owner issues opaque versions containing a random process epoch and per-object counter. These counters are process-local guards, not durable Photo facts.

A fresh epoch invalidates pre-restart versions, so lost counters cannot make an old token valid again. Reading fresh facts is the recovery path; no schema migration is required for the guards.

A Photo counter changes after any effective Selection State or Rating change. An Album counter changes after any effective rename, membership, or order change.

A no-op does not change a counter. Saved position, Preview facts, metadata inspection, and scan progress do not change counters.

Every effective Photo or Album writer MUST participate in owner-controlled version invalidation, including Web routes, batch decisions, Undo, Folder addition, Album compensation, and recovery-related retirements. A read returning a decision or Album version MUST use a real version reflecting that participation.

Record deletion returns missing; recreation uses a new opaque domain ID. Clients cannot supply a new version.

Precondition validation, prior-value reads, mutation, and version decisions run under the serialized persistence owner. Counter changes become visible only after successful SQLite commit and before the next owner command can read. Rollback preserves versions.

Reads return facts and versions from the same owner operation, never an independently updated HTTP cache. Counter overflow MUST fail closed before a write rather than wrap.

Token comparison precedes no-op detection. Changing a value away and back still invalidates an older command. An expired process epoch is an ordinary conflict requiring fresh facts.

The guard covers observed decision or Album state; it does not claim historical identity of Original bytes.

## Mutation Settlement

Photo batches validate the complete request first. One database transaction classifies each requested Photo and changes matching items. Conflicts and missing records are domain results, not transaction failures.

Storage failure rolls back all changes. Returned results are the transaction's facts, not a later query that could observe another writer.

An Album mutation validates its version and all requested identities before member changes. Rename, deletion, add, remove, and reorder are atomic. Web and CLI share member-order and saved-position rules.

A CLI Photo decision cannot write Album progress as a side effect. Neither a successful command nor its output starts an undo session.

Prior Photo values help formulate a correction, but the caller MUST obtain current versions before the new write. Album compensation uses ordinary checked member removal and cannot promise to restore old order.

Photo decisions and Album mutations add no durable command ledger, idempotency-key table, or task transaction. Membership addition avoids duplicates, but its version guard still rejects replay with an outdated version.

Album name uniqueness prevents duplicate names; it does not prove who created an Album after a lost response. [Photo Development Architecture](../photo-development/design.md) owns separate Edit Recipe and Export receipt rules.

A complete valid server error can establish refusal or rollback. A truncated, malformed, unexpected, or missing response after a possible send cannot; report `outcome_unknown`, including for untrusted non-JSON data.

Return confirmed partial results only when every item validates against the request without duplicate or missing outcomes.

## Preview Transfer and Local Files

The CLI obtains Preview metadata and a revision-bearing derivative reference, then downloads that exact derivative. The response MUST repeat typed Photo identity, source, source revision, and dimensions for comparison with the admitted request.

Before publication, the client requires JPEG content type, enforces the streaming byte bound, validates complete JPEG structure, reads encoded dimensions, and compares repeated facts. A maintained pure-Rust JPEG parser owns this bounded validation.

The client does not decode pixels, buffer RAW, or run native image processing. Existing service image byte and pixel limits remain authoritative. If the source changes before delivery, fail or explicitly request current metadata; never label old bytes with new facts.

Download into an exclusively created temporary file beside the requested path. Publish without replacement relative to the admitted parent directory, so racing files or symlinks cannot be overwritten. Final no-replace publication is the local commit point.

Before commit, cancellation or failure removes only this invocation's temporary file. After commit, output or interruption failure preserves the JPEG and reports the committed local effect under the CLI reference. It never retries into an existing or different path.

The output path is local client data; it is not sent as a server Original Location.

### Resumable Processing Artifact Transfer

A verified artifact content revision binds one exact SHA-256/length pair. `GET /api/processing-artifacts/{id}/bytes` and workspace download expose that strong `ETag` and `Accept-Ranges: bytes`.

A caller MAY request one contiguous range.

Matching `If-Range` permits `206` with `Content-Range`; an invalid/unsatisfiable range returns `416` with full length and no stream.

A stale resumable request MAY receive complete `200` bytes only for the same admitted revision, allowing the client to restart that transfer.

If the path now holds a different workspace revision, return `artifact_changed` instead of silently switching its bytes.

Protected managed files can reuse their stable descriptor/identity; mutable external files require stability checks and pinned evidence throughout transfer.

Headers repeat the admitted revision and verified available provenance for successful responses.

The service acquires one finite artifact lease before opening the retained file.
The lease is renewed while the response body is live and released when the body
drains or is dropped. Ranges do not create a second Artifact, a Download Job,
or a server-side copy.

The CLI hashes the response incrementally. An interrupted Artifact download keeps only a destination-private partial file and a small identity sidecar. On retry, matching state is rehashed and resumed with one range; malformed or mismatched state is discarded.

Final no-replace publication remains the local commit point. Partial, short, extra, corrupt, expired, or interrupted transfers never appear at the requested path. Artifact transfer uses an idle-progress timeout rather than a fixed total deadline.

Buffering the complete artifact is rejected because full-resolution TIFFs can
exceed client memory. A server-side copy or durable Download Job is rejected
because the retained immutable bytes and lease already provide the required
identity and lifecycle without another Artifact owner.

CLI Preview work uses the shared bounded scheduler as background demand below an active Web Photo View. One CLI process sends one Preview request at a time; server-side queue bounds apply across clients.

Web interaction MUST NOT wait for an entire Agent analysis batch. The CLI adds no full-Library warmup or speculative neighbor generation.

## Web Handoff

Compose relative Destination URLs under the configured service origin using the [Browser Address Contract](../library-browser/experience/design.md#browser-address-contract). Return a canonical Photo URL rooted in All Photos and an Album Grid URL.

Do not copy arbitrary server-provided origins into links. Unsupported CLI filters have no matching Grid URL and MUST NOT be silently omitted from a supposed query link.

Links expose current state. Page refresh and ordinary requery reveal CLI changes;
this contract does not add live subscriptions or promise replacement of an open
Web source's fixed ID order. Fresh Web actions and CLI writes continue to share
the same serialized storage owner.
