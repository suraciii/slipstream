# Command-Line Architecture

Slipstream needs a machine interface that exposes its domain capabilities
without coupling an external Agent to browser state or duplicating persistence
rules. Human and machine clients can act concurrently, and a failed transport
must not manufacture certainty about a committed write.

[Command-Line Use](../docs/command-line.md) owns product behavior.
[CLI Reference](../docs/cli-reference.md) alone owns command syntax, input
shapes, limits, JSON output, and exit codes.

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

A query continuation is a temporary transport reference. A mutation version
is a precondition on current state. Neither is Photo identity, authority, an
operation receipt, or a new product-domain object. No new glossary entry,
Agent task model, event log, or general workflow engine is required.

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
whole-command deadline bounds all requests. Signal handling cancels reads and
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
  member addition/removal, or complete reorder.

These routes must map to shared owners, not a parallel CLI business layer.
Album creation, Folder windows, scan status/check, capture metadata, Preview
requests, and derivative download reuse their existing routes. Preserve retired
routes as retired; do not revive the unbounded `GET /api/albums` representation.

### Photo Development Surface

These routes expose the first development pipeline through the same shared
operations as Web. They obey the existing method, header-size, body-size, and
shutdown admission rules:

- `GET /api/photos/{id}/edit-recipe`: the current recipe or the absence of one,
  the observed source revision, the source/profile support state, and the
  approved control ranges for that source;
- `POST /api/photos/{id}/edit-recipe`: one guarded save carrying a stable
  request identity and both expected revisions;
- `POST /api/photos/{id}/edit-recipe/rebind`: explicit rebinding of saved
  intent to a newly observed source revision;
- `GET /api/photos/{id}/edit-preview/{stage}`: the current Edit Preview
  rendition for one stage, or an admission or refusal result;
- `POST /api/photos/{id}/exports`: capture an immutable Export snapshot and
  admit its work;
- `GET /api/photos/{id}/exports`: bounded list of that Photo's retained Exports
  with their current state;
- `GET /api/exports/{id}`: one Export's state, captured identity, terminal
  outcome, and artifact metadata;
- `POST /api/exports/{id}/cancel`: exactly-once cancellation against the actual
  completion state;
- `POST /api/exports/{id}/retry`: a new attempt identity against the retained
  snapshot;
- `GET /api/exports/{id}/artifact`: the validated artifact of one Export,
  leased for the response stream.

`stage` is the closed value `develop` until the Film capability is enabled. The
Photo facts returned by `GET /api/photos/{id}` and the bounded Photo summaries
in Browse windows and Photo queries carry the saved-edit fact. Artifact
downloads and Edit Preview renditions reuse the existing private derivative
transfer rules. Structured error codes are authoritative for these routes; no
client parses messages. States, outcomes, snapshot identity, receipt expiry and
disclosure rules are owned by
[Photo Development Architecture](photo-development.md#service-surface).

Every CLI request identifies contract version 1 through
`Slipstream-CLI-Contract: 1`. A server that advertises version 1 must validate
that header on CLI requests, including reused mutation routes, before domain
admission. An unsupported value fails without a mutation. The client checks
capabilities before its command's operational request. An older service that
lacks capabilities is incompatible; the client must not guess based on HTML,
a health response, or a version string.

HTTP does not expose the CLI process envelope on existing Web routes. The
client normalizes typed route results into the CLI envelope. New routes must
return structured error codes and domain references. For reused routes whose
legacy response cannot distinguish necessary cases, add structured fields for
CLI requests rather than parse English messages or silently change Web shapes.

The exact request/response fixtures for these routes must be added with their
implementation and executed before any CLI release. No route addition may
bypass the existing method, header-size, body-size, or shutdown admission rules.
No existing route or wire value is retired by this design.

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
[CLI Reference](../docs/cli-reference.md#service-and-discovery) owns the
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
receipt rules remain owned by [Photo Development Architecture](photo-development.md).

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

CLI Preview work uses the shared bounded scheduler as background demand below
an active Web Photo View. One CLI process sends one Preview request at a time;
server-side queue bounds apply across all clients. Web interaction must not
wait for a whole Agent analysis batch to finish. The CLI adds no full-Library
warmup or speculative neighbor generation.

## Web Handoff

Compose relative Destination URLs under the configured service origin using
[Browser Address Contract](browser-navigation.md#browser-address-contract).
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

### Rejected: Direct Database or Filesystem CLI

A local-only tool could avoid HTTP, but it would duplicate containment,
transactions, scan coordination, and version checks. It would also require
access to private storage and fail the remote-client use case.

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

Preview tests must cover actual readable JPEG output, revision consistency,
existing-file/symlink races, bounded bytes, cancellation cleanup, shared queue
pressure, and unchanged Originals. Linux qualification must also prove that a
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
