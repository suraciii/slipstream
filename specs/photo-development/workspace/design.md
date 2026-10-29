# Directory Photo Workspace

File-oriented tools already accept inputs and produce files.

Making each tool
speak Slipstream's Photo, recipe, publication, and download protocols adds a
second editing state and a transfer cycle.

The selected boundary is a persistent
Photo Workspace directory with protected input snapshots, tool-owned state, and
completed files.

Slipstream owns preparation, containment, indexing, delivery,
and optional execution supervision.

Independent tools own editing and rendering.

[Photo Workspace](spec.md) owns the interaction and retention
contract.

[Photo Development Architecture](../design.md) owns processing
integration.

[Native darktable Integration](../darktable/design.md) owns the
independent editor and its native engine boundary.

Workspace commands are in
[CLI Reference](../../command-line/reference.md).

## Model and Ownership

A Photo has one default persistent workspace association.

Creation is idempotent
under concurrent opens.

The directory lives in application-owned durable storage,
outside the Library and outside disposable execution scratch.

Opening reuses it.

Its stable workspace identity is not an engine image ID, working-directory
basename, file digest, or new Photo.

The workspace record contains its Photo association, directory binding, prepared
input evidence, and an index of contained files and tool-state locations.

It
MUST NOT own a second canonical copy of tool parameters.

The independent tool's
persisted Edit State owns parameters, instance and mask identities, resources,
and revisions.

The service MAY cache a projection, but MUST invalidate it when
the canonical revision changes and invoke the same tool to read or mutate it.

Workspace reconciliation also discovers edit directories for installed qualified
tools beneath `.edits/<tool>/`.

It asks that tool's read-only state interface for
input, revision, dependencies, and retained completed-render evidence.

This MUST
work for states created by a direct CLI call while Slipstream was stopped, without
registration or a tool callback.

It MUST NOT parse private state with a second
schema implementation.

Unsupported or unreadable tool state stays retained and
is shown as unavailable; unknown private directories are never presumed to be
disposable cache.

Missing dependency knowledge blocks automatic cleanup rather
than authorizing deletion.

Unrelated tools can still exchange ordinary files
without providing state inspection integration.

A Workspace Artifact is a completed contained file with a stable artifact
identity, relative path, checked content revision, role, byte size, digest, and
verified available file facts.

Content revisions distinguish replacement at the
same path.

Paths are usable filenames; IDs support Web addressing and bounded
queries.

Neither substitutes for verified byte identity at use time.

A Processing Artifact is the narrower case with validated processing evidence
for the exact bytes, input, saved state, tool bundle, and output contract.

A
workspace file can be a useful input or downloadable output without qualifying
as a Processing Artifact.

Unknown producer, edit version, or color contract MUST
remain unknown.

Filename, directory, modification time, and native executable
exit status alone are insufficient proof of that provenance.

Slipstream MUST NOT schedule another tool when a file appears.

The caller owns
composition by selecting an exact file as another invocation's input.

No graph,
conversion registry, or pipeline scheduler is needed for this boundary.

## Directory Preparation

Preparation MUST resolve the Photo through the existing Library read boundary.

It MUST capture a stable Original revision, copy and hash an already validated
read descriptor, verify source stability, and atomically place a read-only input
snapshot in `input/`.

A proven copy-on-write clone MAY replace copying where it
preserves byte independence.

A writable hard link or a symlink back to the
Library does not meet the boundary.

Preparation MUST reserve a finite storage allowance and fail without a ready
input when it cannot fit or the source changes.

Concurrent requests MUST share
or reconcile the same preparation, rather than creating competing snapshots.

The returned path MUST name the completed snapshot, not the Original pathname.

Later operations MUST check that snapshot's identity rather than require the
live Original to remain readable or unchanged.

Source refresh creates a separately identified input snapshot.

Existing tool
states and downstream bindings keep their captured input until an explicit
rebind initializes and validates the new input.

Source location recovery and
Library revision changes do not implicitly replace workspace files.

Create `previews/`, `outputs/`, `.edits/<tool>/<edit>/`, and `.tmp/` lazily.

These
managed locations express roles and protection, not a mandatory protocol for
all external tools.

A completed regular file elsewhere beneath the workspace
can be indexed conservatively as a generic file. `.edits/` and `.tmp/` are
excluded from normal artifact discovery and download; tool-controlled state
export is the route for portable state delivery.

Required resources live with
that state or have retained exact references.

A tool invocation receives explicit input, state, and output locations.

Defaults
for output directories MAY be recorded by the tool when its state is opened;
the independent tool MUST NOT read a Slipstream database or infer ownership from
an ambient directory name.

Paths MUST remain meaningful in the execution
namespace disclosed by workspace open, including container mount translations.

## Completion and File Discovery

Files, not a client upload or metadata transaction, cross the tool boundary.

Tools MUST write unfinished bytes to a private temporary file on the target
filesystem, close it, then atomically move it to a unique final path without
replacement.

This can be implemented entirely with ordinary filesystem
operations; a Slipstream SDK, manifest, callback, or register command is not a
prerequisite.

The independent editor additionally returns structured result
facts that an integrating caller can validate for richer provenance.

Filesystem events are invalidation hints.

Workspace open, file listing, inspect,
and download MUST reconcile relevant files with bounded scanning and cached
observations.

Startup MUST repair missed observations.

Event loss MUST NOT make
a completed file permanently invisible.

Scans MUST be confined to the workspace,
exclude private state/temp storage, and refuse symbolic links, special files,
and traversal.

A bounded listing reports continuation rather than pretending
its first page is the entire directory.

A new final image remains Checking until a confined descriptor yields a stable
size and digest and bounded format inspection validates its actual structure.

Check stability before and after reading; a changed observation restarts
validation or leaves the file unavailable.

Large or unsafe-to-parse files MUST
produce an explicit limit or type refusal rather than blocking discovery of
other files.

An unknown safe regular file MAY be delivered as an attachment with
an unknown processing role, but cannot become an admitted image input on the
strength of its extension.

Direct writes into a final filename are not an atomic completion signal.

An
integrated tool MUST use the temporary-and-rename contract.

For externally
written files, final-close or stable scanning plus format validation can only
establish the observed complete revision, not prove that the producer will never
write again.

Any later change invalidates the old revision and provenance.

A
truncated known image MUST remain unavailable, never be relabeled a generic
ready output to bypass image validation.

Invocation acceptance and artifact indexing are independent.

A tool can have
finished while the service is restarting or indexing is delayed.

Report the
known completed path and an indexing-pending state; do not rerun rendering to
repair discovery.

A supervised job can settle successfully only after its exact
returned file and provenance pass validation.

Reconciliation MAY adopt a file
for a job only when retained invocation evidence identifies that attempt; it
MUST NOT choose the newest matching filename.

## Stable Bytes and Containment

Downloads and processing admission bind to one artifact content revision and
open the file through the confined read boundary.

A different revision refuses
that binding.

Download uses size and digest evidence so a truncated, extra, or
changed transfer is not committed by the CLI.

Browser delivery MUST use the
same validated descriptor and disclosed identity; caching never substitutes a
new version for a previously named revision.

Managed completed outputs are no-replace and read-only to later tool runs.

Inputs and selected existing artifacts MUST be mounted or otherwise exposed
read-only to supervised children.

Each run receives writable private scratch
and its intended output destination; no writable Library access is granted.

The system MUST NOT rely on a writable hard link for isolation.

Host-level
filesystem access is a trusted execution capability, not an HTTP authorization
bypass exposed to an arbitrary browser client.

When an untrusted-to-stability external output is admitted for processing, make
one protected byte-independent snapshot and recheck its captured identity.

This
copy is unnecessary for already protected managed files, and MUST NOT recur
on every parameter edit or preview.

Its space and failure MUST be observable.

Concurrent external mutation MUST fail input admission or execution validation;
it MUST NOT silently feed replacement bytes to an old saved state.

The authorized tools retain useful filenames.

Default exports allocate unique
names.

An explicit existing output path MUST fail without replacement.

There
is no mutable `latest.tif` downstream binding.

The service MAY select a current
view, but selection is an index/reference and not a filesystem alias promising
immutable content.

## Editing and Execution

The independent editor needs a readable input, its state directory, output
locations, and compatible native resources.

Its package MUST NOT require
Slipstream's service, database, Photo types, credentials, or HTTP API.

Its native
engine protocol remains private to that tool.

The service integration depends
on the independent editor, not the reverse.

`slipstream darktable` MAY resolve Photo and edit identities, pass the workspace
paths, and supervise the same executable used directly.

It MUST NOT copy inputs
out to a separate checkout, persist a parallel parameter tree, or require
submitting edits back.

Browser controls operate through the same integration.

Existing generic recipe interfaces are projections onto those canonical states,
not competing parameter stores.

A single edit's mutations MUST serialize within the tool.

Each successful
mutation atomically creates a saved revision.

Requests based on an inspected
revision, including browser drafts, MUST carry that revision and reject stale
state.

The standalone CLI MAY expose focused convenience mutations that inspect
and validate under one lock.

It MUST NOT silently refresh an explicitly
supplied guard or use that convenience to replace a stale complete snapshot.

Explicit request identities support reconciliation of uncertain outcomes;
[darktable CLI Reference](../darktable/cli.md) owns their syntax and scope.

First state publication MUST validate before creating tool-private state.

A
refused initial save MUST NOT leave an empty state directory that prevents
workspace inspection or deletion.

Concurrent first writers MUST recheck the
published state under the tool's mutation lock: identical requests replay the
committed receipt; competing intent MUST preserve the existing state and report
the appropriate conflict.

Rendering captures one immutable saved revision before execution or supervised
queue admission.

It need not hold the state mutation lock throughout rendering.

Later edits mark that render stale without changing its input.

Distinct edit
state directories and output files MUST NOT contend on one editing lock; shared
processing admission MAY serialize expensive runs for resource safety.

An accepted capture or persisted job dependency remains active during queue
waiting, execution, and terminal settlement.

Workspace deletion MUST inspect
these retained admissions as well as live process leases.

A settled capture
alone is retained loss inventory; it MUST NOT make the whole workspace
permanently undeletable.

Standalone command lifetime belongs to its process.

It MUST NOT promise durable
service jobs after it exits or dies.

The Slipstream supervisor MAY retain
accepted work through client disconnection and provide bounded wait/status/
cancel operations.

Restart reconciles that supervisor's attempts using retained
evidence; it does not adopt arbitrary external processes or successful files as
its own jobs.

Local read-only workspace access and downloads remain available
when an engine is unavailable.

## Retention and Cutover

Workspace durability follows the [product lifetime contract](spec.md#lifetime-and-recovery).

Task receipt expiration, display caches, and disposable previews MUST NOT expire
inputs, saved tool state, required resources, or kept outputs.

The file index is
rebuildable; its mapping and provenance evidence needed for stable identities
and jobs MUST remain durable.

[Deployment Backup](../../deployment/spec.md#backup)
defines the complete recovery point and independent-writer exclusion.

A stopped
service does not freeze tool-owned files.

Copy under established quiescence or
use a consistent snapshot; independent live file copies cannot preserve a
revision and its dependency closure reliably.

Selective removal follows the [product dependency and active-use rules](spec.md#lifetime-and-recovery)
and [CLI removal contract](../../command-line/reference.md#photo-workspace).

Protected
inputs are read-only to tools, not permanently exempt from explicit removal.

Removal and new binding admission share protection: either the use pins the
revision first or it observes removal and refuses.

Unverified independent use
refuses removal.

Removing the default input clears that association without
retargeting an edit; reopening never recreates it or reads the Original.

Explicit
refresh MAY create a new artifact identity, while exact removal replay retains
the original settled result.

Directory cleanup MUST NOT follow symlinks, erase
the Library, or delete state as though it were cache.

Missing externally deleted
files remain explicit missing inputs until rebound or the dependent state is
removed; they never resolve to a newer same-name file.

Whole-workspace deletion checks the reviewed complete loss inventory and
dependencies again before withdrawing the workspace from use.

It MUST coordinate
with each known tool's state and active-work protection, including independent
processes.

The service queue alone is not proof of quiescence.

Unknown state,
unverified external use, or an active dependency refuses deletion until resolved.

The retained deletion receipt owns resumable reclamation after visibility is
removed; partial reclamation MUST NOT be reported as completed deletion.
