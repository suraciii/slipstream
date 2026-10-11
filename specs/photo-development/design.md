# Photo Development Architecture

Photo editing needs persistent intent and expensive native processing, while a
Photo Workspace must remain an ordinary directory that independent tools can
read and write. Keeping parameters in Slipstream's database would make an
external editor depend on the service or require a second state store and a
synchronization protocol. The selected boundary puts canonical edit state in
tool-owned directories and makes Slipstream a workspace file index and optional
execution supervisor.

[Photo Development](spec.md) owns product behavior.
[Photo Editor Workspace](workspace/web.md) owns browser lifecycle and asynchronous presentation.
[Photo Workspace](workspace/design.md) owns workspace creation, confinement,
file indexing, ready-file semantics, accessibility, deletion, and retention.
[darktable Editing](darktable/editing.md) owns semantic capabilities;
[darktable CLI](darktable/cli.md) owns command grammar and the optional
`slipstream darktable` integration surface. [Processing Modules](processing/design.md)
owns file-based invocation and composition. [Development Color Pipeline](processing/color.md)
owns concrete output and display contracts.

## Model and Ownership

A Photo Workspace is a real persistent directory associated with a Photo. It
starts with one visible input snapshot. Opening it returns its actual absolute
execution path and the host/environment in which that path is accessible; a
remote client must not treat a server path as a local path. Workspace outputs
are files, not new Photos or Library imports.

Each independent tool owns its persistent Edit State in a directory such as
`.edits/darktable/<edit>/`. The directory holds the versioned canonical state,
input identity, saved revisions, typed resource references, and local operation
receipts. A darktable state captures operation instances, recursive parameters,
ordering, masks/blending, curves, geometry, output settings, presets/styles and
action results. It is general editing intent, not a fixed exposure/WB recipe.
The native engine's private catalog and binary history are execution details,
not canonical saved state.

Slipstream owns Photo identity, workspace association, guarded Original access,
file-index metadata, Web URLs, and receipts for work it supervises. It may retain
an edit-directory pointer, tool identity, observed revision and derived status.
Those observations are caches: the tool's directory is the authority. Slipstream
must not keep a second writable parameter tree in SQLite or regenerate state
from an outdated cached tree. Existing recipe/step read models may project the
tool state where semantics match; they do not introduce a second state owner.
Browser drafts and session undo/redo remain local pending intent.

A Workspace Artifact is a completed indexed file. A Processing Artifact adds
validated input, edit revision/parameter digest, engine bundle/schema, output
contract and byte provenance. A file without that evidence remains an ordinary
workspace file, even when placed in `outputs/`. Observing it must not invent an
Export success, an edit revision, or a relationship to a concurrent Web run.

Composition selects one completed file as another tool's explicit input.
Upstream edits never retarget that input or rerun downstream work. darktable and
standalone SpektraFilm remain independent peers. There is no workflow scheduler,
plugin registration framework, mandatory ordering, or automatic conversion.

## Application Boundary

The dependency is `Slipstream -> independent editing tool -> native engine`.
The standalone `darktable-edit` executable depends on accessible files,
its state/resources and its own qualified darktable bundle. It requires no
Slipstream server, database, HTTP token, Photo ID, or Slipstream crate.
It can be invoked directly in the workspace's execution environment. Downloading,
editing elsewhere and submitting files back is not the primary editing flow.

Web controls and optional `slipstream darktable` commands resolve the same
workspace and state directory, then invoke the same tool using argument vectors
and bounded structured results. They do not implement a second parameter
validator, compute native actions themselves, or require tool callbacks/SDK
registration. No public arbitrary-shell or arbitrary-server-path endpoint is
introduced. Standalone callers may use their own accessible paths; the service
accepts authorized workspace/file identities and resolves confined paths itself.

The tool owns validation, atomic edit persistence, captured-revision execution,
native-child lifetime and filesystem output completion. Slipstream adds bounded
admission, browser/run receipts and process supervision for work it starts.
Tool availability, input compatibility, resource admission and Library readiness
remain separate facts. Missing engines leave browsing, file downloads and saved
state inspection usable.

## Original Access

Workspace creation resolves the Original through the guarded Library descriptor
boundary, copies or reflinks its bytes into an independent read-only input
snapshot, hashes them, and checks source stability before installing that
snapshot. A hardlink or symlink that exposes writable Library content is not an
independent snapshot. Engines never reopen an unchecked Library pathname.

This stable copy happens once per input version, not for every preview, export
or edit. Rendering verifies the bound input's identity and byte stability.
Later Original disappearance or change does not invalidate unchanged workspace
input. Explicit source refresh adds an input version; it never replaces bytes
beneath an existing edit or silently rebinds intent.

Original Files and photographer-owned XMP Sidecars are outside tool writes.
Native children receive read-only inputs and confined immutable resources, with
private writable configuration/catalog/cache/scratch. Ambient sidecar discovery,
sidecar writing and desktop configuration are disabled. Ordinary workspace
inputs and resources are also checked: changed bytes refuse the old binding,
not a reason to silently recompute with the same revision label.

## Development Proxy

A service-owned Development Proxy is no longer required to edit while the live
Original is unavailable: the persisted workspace input supplies the admitted
bytes. Existing proxies retain their disclosed provenance and preview-only
limitations. A legacy proxy cannot be promoted into a full-resolution Export
source or presented as the Original snapshot. It may remain an explicitly
identified reference or retained legacy derivative; reconstructing saved
creative intent from a baseline proxy is forbidden.

## Edit State Writes and Autosave

Tool mutations acquire a bounded per-edit lock, validate the complete resulting
state against the pinned schema, and atomically install canonical state, its
new revision and the operation receipt. A failed composite operation changes
nothing. Independent state directories do not require a workspace-wide lock.
Captured render revisions remain immutable while later edits advance current
state. Ordinary writes do not rebind inputs.

### Darktable Semantic Mutations and Version Lifetimes

Stable operation-instance and mask IDs belong to the edit directory's lifetime;
engine handles belong only to a private attempt. Focused changes preserve omitted
fields. Actions save concrete native results atomically. Reset uses initialized
image-context defaults, not generic schema defaults. Preset/style application
materializes edit-local instances/masks with explicit mapping and preserves
immutable resource-version meaning. Custom camera WB remains separately qualified.

Web and Agent writes supply checked expected-revision/state evidence and a
caller-retained request identity. The standalone grammar also permits simple
local writes under the tool's locked current observation; that convenience must
not silently refresh an explicitly supplied guard. Exact-body replay is resolved
before current-state guards, returns the recorded operation, and does not repeat
an action. A different payload under the same identity conflicts. A generated
identity returned after a direct command is not sufficient to recover a lost
response; recoverable callers must choose and retain it before invocation.
Exact grammar, guard scope and receipt limits belong to the CLI authority.

Canonical saved revisions carry input, complete semantic state, bundle/schema
and resource identities. Retain current state and the advertised bounded history
of at least 100 latest successful revisions per edit. Reads/renders add no edit
history. Work pins its captured revision and dependencies. Restore creates a
new guarded revision; a different input requires explicit validated rebind.
Unsupported saved versions stay inspectable and unavailable for execution;
no baseline substitution or silent semantic upgrade is permitted.

Browser autosave serializes writes per edit, with one in flight and coalesced
later actions. A save acknowledgement confirms only its own action and cannot
erase newer draft intent. Navigation does not move response ownership to another
Photo. Bounded local recovery drafts identify service, workspace, edit, input,
observed revision and request identity. Storage failure is disclosed while
online saving continues. Unconfirmed drafts are removed only after matching
confirmation or explicit discard. Uncertain mutations are reconciled through
the tool's receipt before dependent writes proceed; inspecting equal settings
alone does not prove which action committed.

## Export Submission and Ordering

Export captures the confirmed edit revision, input byte evidence, bundle/schema,
resources and intended output contract. Web settles prior writes, commits the
captured visible draft if needed, and admits that exact revision before releasing
its submission barrier. Later edits cannot modify the captured output request.
A concurrent conflict preserves the user's captured intent for explicit resolution.

Standalone export is a synchronous local command owning its process; no server
queue, HTTP polling or service task lifecycle is imposed on it. Web may queue and
supervise the same executable. Its receipt records immutable invocation identity
and does not become the canonical edit state. Exact request replay must not start
a duplicate uncertain operation. Receipt expiry never authorizes reuse as new work.

## Preview Scheduling

The tool renders only the selected edit and captured revision at a qualified
bounded geometry. It does not first render a full-resolution handoff or invoke
another tool. Actual decoding may still require full source memory and must be
admitted separately. A finite rendition derivation changes geometry/display
only as qualified; it does not silently alter effects, precision or randomness.

Browser latest-intent ownership includes workspace, edit, revision, input,
bundle, geometry and display transform. Comparison owns its two operands
separately. Prior completed preview files remain available while a replacement
renders; the browser labels stale results and never presents late old-revision
completion as the current canvas. Camera Preview is a separate reference.
Preview files can be downloaded, but that fact does not qualify them as
full-resolution scene-linear inputs. Continue Processing follows actual
compatibility under the workspace contract.

## Export Execution and Recovery

The tool writes unique private temporary output, closes and validates it,
then atomically renames it into its final workspace path without replacing an
existing file. Default names are unique and returned; an explicit existing name
is refused. Installation must use an atomic no-replace operation rather than a
racy existence check followed by overwriting rename. Each completed file is
independently indexable without a manifest, registration, callback or commit.

Structured completion identifies the exact invocation, captured state and file
content evidence. Slipstream validates that evidence before claiming a supervised
run succeeded or annotating a Processing Artifact. A generic file index can see
a ready file even when a tool response was lost; that alone cannot settle the run.
Concurrent same-directory runs must be attributed by their unique invocation and
output evidence, never by newest filename or watcher-event timing.

Web-run state is queued, running, succeeded, failed or cancelled; an uncertain
observation is reported as outcome-unknown until reconciled. Browser departure
does not cancel accepted work. Cancellation settles against actual completion;
it cannot undo an already completed file. Failure preserves saved state and
prior completed outputs. Private partial files never appear as downloadable
results. Disk-full and close/encoding failures do not install a final file.

On service restart, reconcile only service-owned run receipts. Recover success
only with matching complete file and validated invocation provenance. Interrupted
unproven work fails explicitly; do not automatically rerun it. Queued service
work revalidates pinned input, revision and bundle before dispatch. Never attach
to an unidentified old process or delete scratch still owned by a live tool.
Standalone processes have their own lifetime: service restart neither cancels
nor adopts them. Their completed files are discovered through workspace
reconciliation; their local receipts can prove provenance when present and
verified, but absent evidence leaves a generic file. The workspace specification
owns file-index reconciliation independently from this run lifecycle.

## Resources and Retention

[Local Photo Executor](processing/execution.md) owns process containment,
cancellation, finite deadlines and Web admission. [Processing Memory](processing/memory.md)
owns memory/CPU enforcement and unchanged-image resource planning. The Web
supervisor initially admits one heavy attempt at a time, with bounded queue
fairness between preview and export. That limit covers its own runs, not all
external commands on the host. Standalone execution must advertise its own
finite environment/policy rather than claim the service serialized it.

Resource imports have independent bounded bytes, kind and content limits.
The tool owns immutable typed versions, validation and confined native staging.
Current state, retained revisions and active work protect referenced dependencies.
Missing external resources are explicit, not fabricated. A state directory is
portable only with its input/resources and compatible qualified bundle.

Workspace inputs, current edit directories and kept outputs remain durable
until explicit deletion under [Photo Workspace](workspace/design.md). Transient
receipts, caches, unkept previews and abandoned private scratch have separately
disclosed limits; expiring a receipt does not delete a user's required files.
Capacity refusal preserves earlier results and saved intent. Cleanup excludes
live work and referenced dependencies. Downloads pin the exact indexed revision;
changed external bytes invalidate old evidence, never silently replace a stream.
[Deployment Backup](../deployment/spec.md#backup) owns the consistent recovery
point across service state, canonical tool files, and retained dependencies.

## Library and Client Integration

Saved-edit and retained-workspace facts remain independent of Rating, Selection
State, Albums and browsing position. They prevent automatic retirement as an
unreferenced Photo. Location Recovery and later Library revisions do not retarget
workspace inputs. Photo read models report saved-state existence and availability
without eagerly rendering every Photo. Bounded paginated lists preserve access to
older files and receipts; list bounds are not eviction policy.

### Edit state file snapshots

Existing acknowledged Edit State File snapshots remain exact retained bytes with
their filename, hash, receipt and provenance. They are historical exchange
artifacts, not a second writable source of truth. New snapshots, where supported,
capture the tool's confirmed canonical revision/resources through a bounded
read. They neither modify external XMP nor establish desktop-history import.
No per-edit upload/submit bridge is required: the actual state directory is
already the persisted editing state.

### Existing service state at cutover

Before making file state authoritative, take a non-destructive snapshot of
existing service recipes, revisions, resources and receipts. Convert supported
intent into tool-owned directories with explicit source binding, instance/mask
identity mappings, exact semantic values, output settings, dependencies and
original bundle/schema provenance. Validate the complete converted state before
switching its service pointer. A crash leaves either the old authoritative
record or a fully installed directory; repeat conversion cannot create extra
edits. After successful switch, old records are read-only migration evidence,
not another mutation store.

Do not silently apply a new baseline, discard unknown fields, reinterpret blobs
or reduce unsupported settings to exposure/WB. Unsupported intent remains
readable with its original saved data and an explicit blocked-conversion reason.
User-approved semantic upgrade is a separate operation. If the original source
is unavailable, preserve intent and disclose that executable input binding is
missing; a camera JPEG or proxy cannot fabricate it. New workspace-based editing
must not require conversion of unrelated unavailable history.

Historical image and state-snapshot exports retain acknowledged bytes, exact
metadata, download leases and previously promised retention. They may be indexed
as historical workspace files when their linkage is known. Incomplete processing
provenance remains incomplete; it cannot be invented to promote them to new
Processing Artifacts. Old request identities remain reserved with their recorded
replay or explicit expiry result. Unfinished retired work settles as interrupted
without executing a changed pipeline. Existing compatible service routes may
project canonical file state; any public retirement requires coordinated caller
and deployment-contract changes in its owning specification.

## Service Surface

Slipstream's surface manages authorized workspaces/files, resolves tool states,
and supervises permitted tool commands. The exact `slipstream darktable` grammar
remains in [darktable CLI](darktable/cli.md); the independent grammar
addresses paths/state directories and needs no service transport. Existing
compatible generic processing surfaces are projections, not a universal editing
schema. No arbitrary shell, executable expression or workflow graph is accepted.

### Availability and diagnostics

Separate tool/bundle readiness, selected input compatibility, resource admission,
workspace file availability and Published Library loading status. Initial
Original snapshot admission uses current guarded Published Library facts.
Afterward workspace input facts govern processing. Transient resource or scan
conditions remain retryable; they never masquerade as confirmed Original parse
failure. Native discovery alone does not qualify a capability.

### Guarded save outcomes

Report saved, replayed/unchanged, conflict, invalid settings, unsupported,
unavailable, expired receipt or outcome-unknown with structured reasons and
current revision evidence when available. Validate exact replay before guards;
explicit guards are never auto-refreshed. Input mismatch requires explicit
rebind. Lost responses preserve draft/request identity and authorize no duplicate
action. Carrier names and compatibility mapping belong to the CLI/wire authority.

### Failures and refusals

Distinguish unavailable tool, unqualified input/settings/output, missing resource,
capacity refusal, changed input, stale state, bounded busy, native failure,
cancellation and unknown effect. Messages explain the preserved state/files and
next action; clients use codes rather than parse prose. No failure falls back to
another tool, a camera preview, a reduced output or guessed defaults.

### Wire contract

The [Command Line design](../command-line/design.md#photo-development-surface),
[CLI Reference](../command-line/reference.md#photo-development) and
[darktable CLI](darktable/cli.md) own public carriers. Integration facts
include workspace identity, actual execution host/path and indexed file URLs.
Authorized local execution paths are distinct from remote download URLs.
The service cannot imply that a path is mounted in a remote caller's environment.

## Options

### Selected: Tool-Owned Persistent File State

One canonical edit directory serves standalone and supervised execution.
Slipstream indexes files and optionally supervises the same executable. This
keeps engine/schema knowledge with the tool and makes each edit iteration local
to the same directory. The cost is explicit workspace backup and honest
reconciliation when external tools change files or disappear.

### Rejected: Monolithic Parameters with a Download/Submit Bridge

Keeping the database authoritative either prevents independent local edits or
creates a second writable state copy that needs semantic conflict merging.
Download/edit/submit on every iteration adds copies, transport failure and
uncertain ownership to a directory that tools can already access directly.

### Rejected: Export Whatever Settings Are Current at Execution

Queue delay would change submitted intent. Captured immutable revisions and
exact output contracts make later edits and retries predictable.

### Rejected: Shared Desktop Catalog or Generic Workflow Framework

A shared desktop catalog leaks engine lifetime and ambient image/history state.
A scheduler/plugin framework introduces discovery, ordering and recovery policy
unneeded for explicit file-based invocations. Fresh native children and direct
caller-selected artifacts provide the required capability.

## Verification

Implementation evidence must establish standalone operation without any
Slipstream service/database dependency and supervised Web/CLI edits using the
same directory and canonical revision. Cover instance/curve/mask/action edits,
resources, presets/styles, saved-history restore, concurrent guarded writes and
exact uncertain-action replay. Include Export without a preceding Preview.

Exercise guarded one-time input staging, Original/XMP invariance, explicit
refresh, same-directory preview/output discovery without registration, no-replace
publication, partial-file/crash/disk-full handling, cancelled native descendants,
late canvas results and concurrent run attribution. Restart must distinguish
supervised interrupted work from external CLI work. Qualify actual color,
geometry, finite resources and native schema combinations independently.

Cutover evidence must preserve supported semantic intent and acknowledged
historical bytes, block unsupported conversion visibly, retain request identities,
and recover a conversion crash without a second authority. Workspace recovery
must satisfy [Deployment Backup](../deployment/spec.md#backup), including writers
independent of service lifetime. These are required future implementation
checks, not claims that the design or proposed CLI is implemented.
