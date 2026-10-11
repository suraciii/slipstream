# Native darktable Integration

Darktable's parameter layouts, actual defaults, module ordering and history
construction belong beside the engine. Independent editing also needs durable
state outside Slipstream. The native bridge therefore sits behind an independent
file/state tool; Slipstream supervises that tool rather than owning native
parameter translation or a second edit database.

[darktable Editing](editing.md) owns semantic capabilities,
[darktable Integration](spec.md) owns input qualification,
and [darktable CLI](cli.md) owns public grammar. [Photo Development
Architecture](../design.md) owns workspace integration and conversion of
existing service state. [Development Color Pipeline](../processing/color.md) owns
baseline and numerical image contracts. This document owns the native boundary.

## Model and Ownership

The dependency is one-way: `Slipstream -> darktable-edit -> native darktable`.
`darktable-edit` is the proposed independent executable, not the existing
upstream `darktable-cli` or desktop GUI. It runs with accessible input and edit
state paths, its own resources and pinned engine bundle. It needs no Slipstream
server, HTTP token, Photo ID, database or crate. Optional `slipstream darktable`
resolves the Photo/workspace and invokes the same executable against the same
state directory. Web controls use that same operation boundary.

The tool owns versioned canonical semantic Edit State, saved revisions,
input/content binding, operation-instance/mask identities, immutable resource
references, request replay and per-edit locking. Slipstream keeps pointers and
observed facts, never a second mutable parameter snapshot. The native bridge
owns reflection, actual image initialization, validation/application, history
construction and pixelpipe execution. Native catalogs, handles and blobs are
private attempt state. No persistent desktop catalog is the canonical edit.

Tool-owned resource/Preset/Style catalogs are distinct from private native
attempt catalogs. [Catalog Scope](editing.md#catalog-scope)
owns their scope and entry-point behavior; the
[CLI contract](cli.md#managed-resources-presets-and-styles)
owns selectors, resolution, and result fields. Slipstream resolves authorized
Photo/Edit identities to the same state directory and invokes that tool-owned
catalog. It keeps no duplicate record store and must not redirect a local
catalog lookup to configured global records. Retained revision dependencies
resolve through their pinned bytes or immutable retained references, independent
of later command catalog selection.

The edit-local catalog owns its persistent identity. Edit views read the catalog's
descriptor directly; the edit-state database does not retain a second catalog
identity.

A headless action names an admitted native algorithm with its input schema and
affected targets. The bridge evaluates it against isolated initialized state
and returns concrete semantic values. The tool validates and saves those values
atomically; later renders reuse ordinary saved intent. Exposure deflicker, for
example, captures computed EV as manual exposure while preserving other settings.
Region pickers and automatic geometry require native headless support, not GUI
event simulation or a second algorithm implemented by Slipstream.

The bundle pins engine commit, native protocol, semantic schemas/mappings,
baseline policy, resources and color assets. General editing includes recursive
parameters, multiple instances, curves, masks/blending, geometry, actions,
presets/styles and output settings. Readable/writable/executable capabilities and
missing support are explicit. Discovery does not claim GUI parity or qualify a
source/settings/output combination. Unsupported saved intent stays inspectable;
new writes refuse unavailable capabilities rather than guessing them.

## Interaction

A direct caller supplies input/state paths to the tool. A Web caller supplies
authorized workspace/file/edit identities; Slipstream resolves actual paths in
its execution environment and starts the same tool. The tool reads its canonical
revision, captures the selected input/settings, starts a fresh private engine,
checks native metadata, initializes the qualified baseline, applies complete
intent, and executes the requested action or rendition. Mutation returns an
atomically saved revision; render returns exact completed file evidence.

The tool uses a deterministic typed client over private stdio to its pinned
native MCP bridge. The native protocol is not a client HTTP API or an LLM-driven
session. No callback or SDK registration with Slipstream is required. The tool
can atomically finish output in the workspace while Slipstream is absent.
Service supervision adds run receipts, not parameter authority.

## Discovery and Parameter Contract

Bundle construction queries actual module identities, parameter versions,
structures and headless operations. Bounded approved metadata permits ordinary
inspection without launching an engine for every UI read. Each private attempt
checks protocol/tool metadata and used schemas against that bundle before
applying intent. Mismatch is explicit engine incompatibility, not broader
permission, and installs no output or changed state.

The bridge exposes one recursive semantic representation for schema, decode,
validate and apply. Arrays retain shape and element types; structures retain
nesting; strings retain encoding/capacity; scalars disclose finite representable
ranges, exact integer rules, booleans, enum symbols/values and descriptions.
Unsupported kinds remain explicit rather than disappearing from a supposedly
complete schema. Metadata additionally covers multiplicity, legal order,
relationships, blend/mask kinds and coordinate frames, resource types/limits,
actions/effect targets and output formats/profiles/precision/transfer/geometry.
Catalog identities are semantic targets, never stack positions or UI labels.
Large catalogs are bounded or paginated.

Image-context inspection returns actual initialized/reset values and operation
relationships. Static schema defaults cannot substitute for camera initialization.
Read-only prospective inspection saves no state. Add/reset initializes against
the bound image; clone allocates a distinct stable instance ID. Partial updates
preserve omitted values and produce a complete canonical validated revision.
Engine handles never become durable identity.

[Presets and Styles](editing.md#presets-and-styles) owns mapped
Merge/Replace semantics, including target-input reset defaults and mask omission.
The bridge supplies actual defaults, operation compatibility, relationship
validation, and native legal ordering; the tool resolves catalog records and
entry mappings, computes the complete candidate, and preserves durable instance
identities. Slipstream and Web must not implement another reset or replacement
algorithm. Existing read/validate operations expose the concrete scope and
effects before guarded application, without a separate workflow engine.

Validate exact types, finite representable numbers, integers, enums, string
lengths, array dimensions, known fields and complete stack/reference structure
before committing. Invalid values are refused, not coerced, clamped, truncated
or ignored. The tool uses the bridge's semantic contract; neither it nor
Slipstream independently reconstructs C layouts. Private binary parameters
carry source version and checked current size. Equal size cannot authorize an
older layout. Saved older semantic intent needs an explicit qualified upgrade;
desktop binary history import is not implied.

## Execution and State Lifetime

Each native attempt owns fresh configuration, catalog/cache and scratch. It
receives the stable read-only workspace input, pinned assets and confined copies
of immutable referenced resources. It receives no Library write access,
Slipstream database or desktop configuration. Ambient XMP loading/writing,
desktop presets, crawler startup and display dependencies are disabled. Local
private catalog writes are permitted; read-only flags alone are not confinement.

The tool maps captured state against the qualified baseline and actual bound
image defaults. [RAW Development](../processing/color.md#raw-development) defines
technical corrections and prevention of unintended artistic defaults. Input
hash/stability is checked before use; an unchanged workspace snapshot remains
usable if the live Original later disappears. Rendering does not recopy it from
the Library each iteration or silently refresh a changed binding.

The bridge creates admitted instances and enforces legal ordering. Referenced
masks/instances cannot be removed unless an atomic composite leaves valid state.
Chromatic adaptation cannot be duplicated. Validate mask geometry, relationships
and resource compatibility before save. Failure changes neither current state
nor partial initialized history; a failed engine attempt is discarded.

Preview and Export share the same state preparation path. Export does not
require a preliminary PNG, Preview or persistent native catalog commit. The
requested geometry/display derivative remains distinct from processing intent;
an 8-bit native render cannot become scene-linear Development TIFF or Film input.

The tool owns native process-group cancellation, finite deadlines and resource
admission under [Local Photo Executor](../processing/execution.md) and
[Processing Memory](../processing/memory.md). EOF/exit before a complete native result
is failure. It validates output and performs unique temp+close+atomic no-replace
installation, then returns captured revision/invocation/content facts. State
persistence and file publication are separate outcomes: a saved revision remains
saved when rendering fails. No successful partial file is reported. Native
catalogs, history and blobs are discarded after confirmed engine termination;
tool-owned semantic state persists for reopening.

## Integration With Existing Workloads

The independent tool replaces the native invocation boundary, not the concrete
Film engine or color rules. A standalone SpektraFilm run reads an explicitly
selected compatible completed file. It does not ask darktable or Slipstream to
rerun upstream state. Existing preview-only Development Proxies retain their
limitations; the persistent workspace snapshot removes the need to use a proxy
as a substitute for a missing live Original.

Web supervised work and direct CLI use the same canonical state and per-edit
revision guards. Independent state directories may run separately under their
actual execution capacity; no workspace-global state lock blocks unrelated files.
Service restart reconciles only its own supervised receipts. Direct CLI native
children are not service tasks and are not cancelled/adopted by service restart.
Their completed files are indexed independently; Processing Artifact enrichment
requires verified local or supervised invocation evidence.

At cutover preserve existing recipes/history/resources and convert only
qualified semantic intent under [Existing service state at
cutover](../design.md#existing-service-state-at-cutover). Keep unsupported
intent readable and blocked rather than dropping it. Manual parameter packing,
XMP history generation and post-import SQLite validation cannot remain a second
production native execution path or fallback. Existing compatible public
integration routes project canonical tool state. Public namespace changes are
owned by their CLI/wire specifications, not implied by this boundary redesign.

## Options

### Selected: Native MCP Behind an Independent File/State Tool

The tool concentrates semantic persistence and request safety independently of
Slipstream, while the native bridge concentrates layout/default/history knowledge
beside darktable. A fresh child gives a direct lifetime proof. Slipstream can
supervise the exact executable or merely discover its completed files.

### Rejected: Monolithic Parameters with an Independent Mirror

A service parameter source plus local tool state requires synchronization and
semantic conflict merging. It would make offline editing conditional on import
and commit instead of directly using the same directory.

### Rejected: CLI With a Generic XMP Generator

A generic XML wrapper still makes its external author own binary layouts, real
defaults, history construction and version handling. Process isolation alone
cannot remove that duplication.

### Rejected: Shared Persistent darktable Service or Desktop Catalog

Shared native state introduces cross-image history, identity translation and
reset coordination. Tool-owned semantic directories plus fresh private children
provide persistence without an additional daemon or desktop catalog lifecycle.

## Verification

Future qualification must cover nested parameter/string/array round trips,
wrong-version equal-sized blobs, invalid composites, actual image/reset defaults,
instances and ordering, masks/blending, actions and atomic state failure. Complete
standalone edits must work without Slipstream; supervised Web and integration
CLI must see the same revisions, resources and files. Include uncertain request
replay, stale Preview, history restoration and Export before Preview.
The durable [acceptance scenarios](editing.md#acceptance-scenarios)
include A/B/C/D with only A/C mapped for Replace and Merge, all-null creation,
invalid and replayed mappings, and local catalog access switched to Web/remote
supervision. Qualification must demonstrate the same catalog records and exact
replacement effects across those entry points.

Real supported RAW fixtures must qualify full-size float32 TIFF, exact ICC,
orientation, negative/over-range preservation and Film handoff. Success/failure,
cancellation, interrupted tools and service restart must preserve Original/XMP
bytes, prior outputs and saved intent without partial installation or invented
run attribution. Discovery, synthetic TIFFs and compilation do not establish
native RAW or full editing qualification.
