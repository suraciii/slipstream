# Photo Processing Modules

An independently usable photo tool must own its editing state and native engine
contract. Slipstream should discover completed files and optionally invoke that
tool, rather than become its parameter database or impose a pipeline. A bounded
Preview and a full-resolution materialized output remain different requests.

[Photo Workspace](../workspace/design.md) owns the real directory and generic file
index. [Photo Development Architecture](../design.md) owns integration,
cutover and supervised work. [Local Photo Executor](execution.md) owns
execution lifetime. This document owns tool invocation and explicit composition;
[darktable CLI](../darktable/cli.md) owns exact command grammar.

## Model and Ownership

A Processing Module is the service-internal integration boundary for one
independent Processing Engine and its input, persistent state, semantic schema,
resources, and output contract. darktable and standalone SpektraFilm are peer
tools; SpektraFilm is not a required darktable operation or pipeline suffix.
A tool may be used without Slipstream. A Processing Step is the internal record
of one captured tool invocation against an identified file and state revision,
not a required centralized database parameter record or user-visible stage.

Tool-owned Edit State directories retain complete editing intent and revisions.
Slipstream indexes their pointers and observed availability without duplicating
mutable parameters. Separate state directories represent independent edits.
The browser selects one edit/input; changing selection does not rewrite state.
A compatible existing recipe surface may project these facts. Unsupported
historical intent remains inspectable under the cutover contract rather than
being coerced into a runnable new state.

The retained Film adapter owns `.edits/spektrafilm/adapter/state.json` inside
the Photo Workspace. It atomically stores the recipe and exact request replay
receipts, while the compatibility recipe response projects a content-derived
revision. External state changes therefore invalidate stale guards. Saves do
not duplicate mutable parameters into Library metadata. Historical darktable
and unsupported steps must be echoed unchanged; omissions and generic writes
to those steps are refused. Explicit darktable migration reviews the complete
retained parameter tree against its exact protected Original capture through
the versioned tool importer, preserving historical identifiers and bytes.

A Workspace Artifact is a completed file with observed content evidence. A
Processing Artifact additionally has verified invocation provenance and actual
image-contract facts. Folder names and file extensions cannot prove it. A later
tool binds explicitly to one completed input revision, never an upstream
"latest result". Changed upstream edits and new exports leave that binding intact.

The tool owns schema validation, local state locks/revisions, native defaults,
resource versions, child lifetime and filesystem completion. Slipstream owns
Photo/workspace resolution, indexing, access and admission/receipts for runs it
starts. No tool callback, registration SDK, Slipstream crate or HTTP dependency
is needed to write a file that becomes visible in a workspace.

## Interface and Discovery

The durable boundary is file-based: inspect a tool and selected input/state,
mutate its persistent state, or render a captured state into the workspace.
Exact command carriers are owned by the tool's grammar. Slipstream invokes a
permitted executable with an argument vector, accessible absolute paths and
bounded structured results. It does not expose arbitrary shell execution.
Standalone invocation accepts caller-owned accessible paths and requires no
Photo IDs or server task lifecycle.

Read-only discovery reports the tool/bundle/schema identity, input/output
contracts, finite limits, image-context initialization requirements, capability
availability and refusal reasons. Missing tools are independent: failure of
SpektraFilm does not disable darktable or browsing. Readable, writable and
executable capabilities are separate. Introspection alone grants no execution
qualification. Actual input, parameter, output and resource combinations must
match the qualified bundle.

State and resources use tool-owned versioned semantic schemas. No shared flat
parameter map, merged module schema or generic output-options language is
introduced. Unsupported fields or versions must be preserved for inspection
and refused for execution, not dropped to make a request runnable. A changed
engine/schema requires fresh qualification; it cannot reinterpret an old revision.

### Concrete Module Shapes

Darktable state retains ordered Operation Instances with stable edit-local IDs,
native operation/version, enabled state, recursive parameters, legal order and
blend/mask references. Native stack indexes and `multi_priority` are not public
identity. Arrays, structures, strings, curves, geometry, masks, actions and
output profile/precision/encoding remain complete schema values. C layouts and
engine history stay private to the native bridge. General editing is not reduced
to exposure and as-shot WB; custom camera WB remains independently qualified.

Read-only inspection can initialize a prospective baseline in private engine
state without saving an edit. Explicit open/create atomically installs an
input-bound baseline; partial mutations preserve omitted settings and validate
complete resulting relationships. Multiple states require an explicit target;
no newest-edit, newest-artifact or unrelated Web selection default is inferred.
Tool-local history and exact replay own state recovery, shared by Web and CLI.

Presets/Styles have immutable versions, explicit fixed versus target-initialized
scope and merge/update versus replace semantics. Application materializes
edit-local instances and masks with typed reference mappings. ICC/LUT/raster-mask
resources have immutable typed versions, bounded import validation and content
identities. Retained state/work protects dependencies. Missing external bytes
remain disclosed; resource staging and semantic validation belong to the tool.
[darktable Editing](../darktable/editing.md) owns the capability details.

Standalone SpektraFilm retains its grouped camera, enlarger, scanner, film/print
and input/output settings. Its fixed qualified configuration retains the pinned
complete required parameter groups: missing groups are structurally invalid and
changed fixed recipe values are refused for execution. Unsupported saved intent
is preserved, not substituted. Runtime-emitted defaults must match the pinned
tree. Admission uses actual linear ProPhoto float32 TIFF and qualified geometry,
including portrait inputs, rather than discovery-example dimensions. Its
advertised 900,000 ms deadline applies to Preview and Export with explicit
cancellation. These are concrete tool contracts, not a universal parameter DSL.

## Preview and Export

### Bounded Current-Step Preview

Preview executes only the selected edit and captured revision with its explicit
input. It never runs another tool to prepare input, performs a hidden full-size
handoff, or substitutes the Camera Preview. Missing compatible input is a refusal.
The geometry is finite and qualified; full-source decoding memory is admitted
separately. Temporary intermediates remain private and bounded.

State captures intended Export settings; Preview records its exact qualified
geometry/display derivation without altering saved precision, effects, quality,
randomness policy or output intent. Reduced simulation does not claim spatial
pixel equivalence to a downsampled full-size result. Comparison fixes input,
tool, geometry, bundle and display conversion while selecting initialized
baseline or retained revision operands; unavailable defaults refuse comparison.

A completed Preview is an ordinary viewable/downloadable workspace image with
its rendition provenance. Downloadability does not establish high-precision,
full-resolution or scene-linear downstream compatibility. Continue Processing
is offered only under the workspace compatibility contract; an explicitly
accepted reduced-quality image flow must disclose those limits. A completed
Export may supply an exact matching bounded display rendition without another
engine invocation. The camera Preview remains a separate reference.

The standalone Film adapter's completed Preview receipt establishes its bounded
PNG display contract after exact byte and decoded geometry checks; generic PNG
discovery alone does not establish color or downstream compatibility. Its
Finished JPEG additionally requires the pinned embedded sRGB profile. Workspace
publication retains the adapter's exact output contract, including writer
encoding, rather than substituting generic file-discovery encoding labels.

### Explicit Materialization

Export captures confirmed input byte identity, canonical state revision/parameter
digest, bundle/schema, resources and intended output contract. Later mutations
cannot alter it. Standalone Export is a synchronous local process; Web may queue
and supervise that same executable with its own run receipt. A bounded Preview
success is not proof of full-resolution qualification or capacity.

The tool revalidates input stability and full-size resource admission, processes
the selected state, then validates actual output type, dimensions, precision,
color/transfer, metadata, bytes and digest. It writes a unique private temporary
file, closes it and atomically installs the final path without overwrite under
[Photo Development Architecture](../design.md#export-execution-and-recovery).
Cancellation or failure offers no partial output and preserves earlier files.
No callback/manifest/commit is required for generic workspace discovery. Verified
structured completion may enrich the index with Processing Artifact provenance.

For standalone Film, known minimum live processing memory above the effective
finite allowance refuses admission before expensive work. Missing/unbounded
allowance also refuses. Passing this lower bound does not establish complete
resource qualification. Already completed files and retained receipts remain
inspectable after resource policy changes.

## Identity, Compatibility, and Failure

Identity includes captured input content evidence, edit revision and complete
parameter digest, tool/bundle/schema, resource versions, exact invocation
geometry/settings and display transform. Photo identity/source revision are
provenance of the initial snapshot, not a live dependency once stable bytes have
been installed. Original refresh never retargets an existing state.

Browser canvas ownership compares full identity rather than completion order.
Old revisions become stale, not relabeled current. Independent completed files
can coexist, including same-directory concurrent outputs. Only exact validated
invocation/file evidence can settle a supervised run; an unmanifested external
output is a generic file and cannot be attributed by timestamp or newest name.

Compatibility checks actual color/profile, transfer, precision, geometry and
qualified settings, not extension alone. [Development Color Pipeline](color.md)
owns the concrete contracts. The service inserts no implicit conversion or
upstream rerun. The caller may explicitly render a compatible conversion/output
and select that completed file as a new input. Changed input bytes invalidate
old evidence and require explicit rebind; no implicit copy each iteration.

Every invocation preserves read-only Original/XMP and bound input, finite
memory/CPU/threads/disk, deadlines and cancellation. Uncertain local actions
are reconciled through exact request replay, not guessed from visible settings.
Service restart reconciles its own runs; independent CLI processes retain their
own lifetime and effects. Workspace reconciliation discovers their completed
files without inventing server task state.

## Example

One explicit composition is: workspace RAW snapshot -> darktable preview and
Export -> completed scene-linear TIFF -> explicitly selected SpektraFilm input
-> SpektraFilm preview and Export. All completed files remain in the workspace.
The user can download any completed image, stop after darktable, select another
compatible tool, or create another state directory. Changing darktable state
later does not change the selected TIFF or the Film state.

## Options

### Selected: Current Edit State and Artifact Transfer

Single-Engine calls keep validation and engine mappings local. An immutable
Export Artifact makes service handoff and failure ownership explicit without
requiring a workflow engine or cross-service recipe.

### Rejected: Persistent Cross-Service Composition

Tool-owned file state and completed inputs make independent execution,
composition and failure ownership clear. Slipstream can supervise the same tool
without an additional editing store or per-iteration transfer.

### Rejected: Fixed darktable-to-SpektraFilm Pipeline

A fixed pair invokes unselected work and makes interactive editing depend on a
full-size upstream handoff. It cannot represent independent or repeated tools.

### Rejected: Workflow DSL and Implicit Conversion Registry

A scheduler/plugin framework adds planning, registration and recovery semantics
unneeded for explicit file selections. A universal raster/parameter schema also
obscures concrete color and native version contracts.

## Verification

Future implementation evidence must show independent file/state invocation,
same-state supervised Web/CLI effects, complete general edits and bounded
single-tool Preview versus separately admitted full-resolution Export. Cover
schema/refusal honesty, resource versions, exact replay, stable identities,
input changes, no-overwrite completion, partial output, concurrent provenance,
and explicit compatible file handoff. Discovery or a synthetic small image
cannot qualify real-engine color, full-size processing or resource limits.
