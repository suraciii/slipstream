# Headless darktable Editing

A Photographer may ask an external Agent to edit an image in many different ways.
The Agent needs to discover operations, change saved settings, inspect results,
and deliver files without opening a desktop editor. A preset-based RAW workflow
is one use of this capability, not its boundary.

The independent `darktable-edit` tool operates on image files and a persistent
state directory. It must work without a Slipstream Library, server, Photo ID,
HTTP connection, database, or token. Slipstream may invoke it through optional
Web controls and the `slipstream darktable` integration using the same files and
state. [darktable CLI Reference](cli.md) owns command syntax and
results. [Photo Workspace](../workspace/spec.md) owns workspace discovery, file
identity, and the customer flow; [Photo Development](../spec.md) owns
shared processing, Original safety, and retention.

## Scope and Ownership

The tool must provide a general editing surface: scalar and structured
parameters, curves, multiple Operation Instances, legal ordering, blending,
masks, automatic actions, Module Presets, Styles, resources, preview, history,
and output choices. Exposure, white balance, lens correction, crop, and rotation
are examples, not a closed control list. New supported operations use the same
general language rather than requiring a bespoke command for every parameter.

The tool owns its on-disk state, native validation, mutation guards, history,
rendering, and local file completion. darktable owns processing algorithms and
valid operation relationships. Slipstream owns Library identity, workspace
association, file indexing, access, and any supervised invocation. An external
Agent owns editing decisions and workflow order. Neither component must embed a
model, chat interface, planner, or Agent runtime.

The canonical state is the tool's local JSON state and its retained resources
and revisions, not Slipstream's database or a browser draft. Web controls and
CLI commands must operate that same state. A partial Web control must preserve
instances, curves, masks, resources, and output settings it does not edit and
identify state it cannot present. Editing must not change Library Rating,
Selection State, Album membership, Photo identity, or browsing position.

Discovery must distinguish readable settings, supported mutations, and executable
results. A missing native mapping, headless action, asset, or output capability
needs a specific reason and recovery action. A complete schema is not proof that
a control works. Deployment limitations must not redefine general editing as a
permanent handful of controls.

## Finding a Photo

For Slipstream integration, the caller may begin with an Album, Folder, query,
Photo ID, or exact Library-relative Original Location.
[Finding Photos](../../command-line/spec.md#finding-photos) owns lookup and ambiguity. RAW
and same-basename JPEG are separate Photos. Duplicate filenames must not select
the first match. Lookup must distinguish an unknown indexed Photo from a scan
that has not yet published usable facts.

The integration opens or reopens the Photo Workspace and returns its actual
path, execution-host accessibility, input files, and saved state references.
It must not pass a writable Library path as the editing input. A remote caller
must use an authorized environment that can access those files; a server path
is not automatically present on the caller's computer. Final downloads remain
available, but downloading and uploading are not required editing steps.

Standalone use begins with a readable local image path and an explicit state
directory. The image does not need to be a Slipstream Photo. This does not import
a new Photo or grant a remote caller arbitrary access to server paths. Tools can
read and write ordinary files in an accessible workspace without Slipstream
callbacks or API registration.

## Inspecting and Starting an Edit

Opening an input into a state directory must initialize its native settings and
persist the initialized baseline atomically. It returns the state path, input
identity and byte revision, schema and engine identity, saved revision, concrete
initialized settings, reset targets, output settings, and relevant capability
facts. Opening an existing state reuses it idempotently and must not reset it or
bind it to another input. A mismatching input needs an explicit decision.

A read must not create a mutation or render an image. Missing initialization
assets or unsupported input must be reported rather than represented by static
schema zeros. Existing saved state remains inspectable without the live Original
or a usable engine. Input-specific discovery must distinguish known initialized
values from unavailable facts and offer a concrete recheck path where appropriate.

The caller must not construct a complete parameter tree or create a desktop
catalog. A baseline can be exported directly; applying a Style or Module Preset
can be the first change. The independent tool takes explicit input and state
paths. Integration may resolve those paths from a Photo's workspace, but if
several darktable states exist it must require explicit selection and return
bounded summaries. It must not select the first, newest, or ambient current
state. Variants use separate state directories and remain associated with the
same Photo rather than creating new Photos.

An unchanged workspace input snapshot remains usable when its live Original is
missing or changed. A compatible completed output can initialize another state
without reading the Original. Input binding must identify actual file bytes and
image facts, not an implicit latest result or extension-only compatibility.
Within a Photo Workspace, protected managed inputs are reused. An admitted
mutable external image receives one protected snapshot, then uses that binding
for subsequent editing and rendering. The tool's compatibility checks must not
require a Slipstream Export receipt or invent processing provenance.

## Discovering Editing Capabilities

Discovery must provide a bounded searchable catalog and detailed per-operation
descriptions. The same native contract governs discovery, validation, mutation,
preview, and Export. Callers must not need private engine protocols or source
code to understand the controls.

Descriptions must include stable identifiers, meanings, types, units, ranges,
symbolic choices, structured shapes, reset behavior, supported instance changes,
ordering and interactions, and schema/version identity. Static defaults and
input-dependent initialized values are distinct. Lens matches, camera mapping,
RAW support, resource availability, and execution capacity must be separate facts.

Geometric controls must describe units, origin, axis direction, coordinate frame,
and whether values denote edges, sizes, or angles. Curves must describe node
coordinates, interpolation, and channel meaning. Exposure must disclose its
reference baseline and whether values are absolute compensation. White balance
must disclose its relationship to input color interpretation and adaptation;
a second white-balance correction must not be applied silently.

An automatic action must describe inputs, affected objects, preconditions, and
result. Offered pickers, region sampling, lens scaling, and perspective estimation
need executable headless actions. Numeric fields do not establish that a related
GUI action can run without a GUI.

State carries its native schema and engine identities. Reading an incompatible
state must preserve and expose saved intent with a specific limitation. Mutation
or execution must refuse unsupported schema interpretation; an engine upgrade
must not reinterpret values, reset settings, or use another mode silently.
An explicit supported migration must disclose changes, retain the prior state,
and validate the resulting state before making it current.

## Changing an Edit

A mutation addresses an explicit state directory and intended change. Simple
assignment is absolute; relative adjustment is an explicit advertised action.
Unmentioned settings are preserved. Related instance, parameter, mask, blending,
and output changes may form one bounded atomic mutation. Validation evaluates
the resulting combination; any failure preserves the complete prior state.

Writes use a per-state lock and saved revision. A caller acting on observed
state, including Web controls, must supply its expected revision. A stale guard
fails without overwriting newer state. Direct sequential commands may omit the
guard only under the CLI's locked-current-state rules. Concurrent ownership must
produce bounded busy/waiting or a conflict, not simultaneous partial writes.
Separate states must not be locked merely because they share a workspace.

The CLI owns exact-request identity rules, including generated identities for
ordinary calls and caller-supplied identities for recoverable retries. Replaying
the same identity and exact intent reconciles its original result without
duplicating changes; a different intent conflicts. The response distinguishes
the original resulting revision from a newer current revision. Receipt expiry
must never permit an old identity to execute again.

Operation Instances have stable identities independent of position or display
name. Callers can add, inspect, duplicate, enable, disable, reset, remove, and
legally move them. Add and duplicate return identities; defaults come from the
current input. Duplication discloses shared mask/resource references. Invalid
ordering or dangling references fail unless the same atomic mutation repairs
the dependency.

Reset discloses its scope and initialized values and preserves unrelated state.
Full reset affects only the selected state, not other variants or Library facts.
A proposed change can be validated without saving or reserving a revision; this
cannot guarantee a later write will pass concurrency or capacity checks.

Automatic actions compute concrete settings against a captured input and edit
revision and save them under the same guard. Preview and Export reuse them.
Failure or a state change during evaluation must not save a partial adjustment.
Recovering an uncertain action must not rerun it against newer state.

Input replacement is explicit and separate. A changed workspace file invalidates
old input evidence. Refreshing a live Original creates another workspace input
version and never retargets this state automatically. A supported explicit rebind
must validate retained settings against the newly acknowledged input.

## Masks and Resources

Masks and blending are structured state. Callers can inspect, create, modify,
bind, unbind, and remove supported drawn, parametric, and raster masks and their
combinations. Discovery describes representation and limits. Each mask identifies
its coordinate frame and source stage. Upstream geometry changes must preserve
that meaning or refuse incompatibility, not move the mask silently.

Blend modes, opacity, channels, and references are discoverable with their
operation. Missing raster sources, cycles, dangling references, and incompatible
combinations fail before saving. A picker or drawn region must identify the
rendition and edit revision from which coordinates were selected.

The tool can list supported resource kinds and import local ICC profiles, LUTs,
and raster masks. It validates content, type, size, and integrity and returns a
stable immutable version. Imported bytes are retained with the state or carry an
explicit dependency with availability evidence; a bare mutable path must not be
presented as a packaged resource. Resource content must not execute code or import
ambient engine state. Remote integration uses authorized workspace resources,
not arbitrary caller-supplied server paths.

Edits, Styles, previews, and Exports identify their exact resource versions.
Removal must not break retained revisions or captured renders. Missing resources
remain visible with their recovery action; no fallback profile, LUT, or mask is
selected silently. A state directory is not magically portable when required
resources or engine dependencies are absent.

## Catalog Scope

Resources, Module Presets, and Styles belong to one tool-owned catalog. An edit's
state directory owns its edit-local catalog. Standalone access to that directory
and authorized Slipstream access through its Photo and Edit identities must use
the same records, identities, immutable versions, and reference accounting. Web
and remote integration must not copy them into a Slipstream record store or
require export/import when switching entry points. Integration resolves only
authorized edit identities, never arbitrary caller-supplied server paths.

An unscoped catalog command selects the standalone tool's user catalog or the
supervised tool's configured catalog. These may be different catalogs and must
be disclosed. A scoped catalog command selects only the named edit-local
catalog. Existing multiple-edit ambiguity requires explicit Edit selection;
catalog reads must not initialize an edit or choose the newest or Web-selected
edit. No ambient desktop catalog is read.

Save and apply default to the target edit-local catalog. An explicit global
selection instead saves to or resolves definitions and new resource references
in the executing tool's disclosed user/configured catalog. Atomic application
and validation use one selected catalog for every new reference. Existing
unchanged resource references use the revision's pinned bytes or immutable
retained references. No missing ID, version, or name may trigger cross-catalog
searching. Applying pins the required bytes or immutable retained references
into the saved revision; it does not duplicate catalog records. Every catalog
discovery and operation result must
identify the selected scope and catalog identity. Exact selectors and result
fields are defined in [darktable CLI](cli.md#managed-resources-presets-and-styles).

## Presets and Styles

A Module Preset holds selected settings for one operation. A Style holds selected
instances and relationships. Callers can list, inspect, save, apply, import,
export, and remove supported representations by identity and immutable version.
Export support does not imply acceptance of every desktop history format or
bidirectional XMP synchronization.

Saving discloses controls, instances, masks, resources, and whether output
settings are included. Style application maps every captured entry exactly once
to either a compatible existing Operation Instance or null. An existing
destination must not appear twice; unknown entries, missing entries, unknown
destinations, and incompatible operation/schema mappings refuse the whole
application. Names and stack positions must not supply an implicit mapping.

Merge changes only captured fields and relationships of mapped existing
instances. Replace has exactly the non-null mapped instances as its reset
scope: it preserves their IDs and positions, resets their settings to actual
defaults for the bound target input, then applies the captured fields and
relationships. With omitted masks, reset and application must preserve existing
mask-dependent fields and relationships. Both modes preserve every unrelated
instance and existing shared mask; neither removes an operation range, unnamed
instances, or shared masks. Captured compatible masks receive new target-local
identities rather than overwriting existing masks.

A null destination creates one new occurrence initialized for the target input,
then applies the captured fields and relationships. An all-null Replace resets
or deletes no existing instance. Existing mapped positions are preserved; new
occurrences follow captured relative ordering where compatible with native
ordering constraints. If more than one legal insertion exists, use the earliest
legal position that satisfies those constraints without reordering existing
instances. Impossible ordering or relationships refuse the whole application.
These rules do not permit implicit broad deletion. Module Preset update changes
included fields only; replace resets only its explicitly identified instance
with the same target-default and mask-omission rules.

When requested, validation must report the exact mapped reset scope, preserved
targets, new occurrences, and relationship/output effects without saving or
reserving state. It is optional inspection, not a prerequisite to application.
Guarded application validates the same effects against the observed revision
and returns them with the saved result. Exact request replay returns the
original mapping and effects without allocating another occurrence; a reused
request identity with changed intent refuses the request.

Definitions preserve included masks and complete relationships. Compatible
application allocates target-local identities, rewrites references, and reports
the mapping; it must not retain dangling source-state identities. Omitting masks
preserves the target's relationships. Camera-dependent values, lens facts, and
source-bound geometry must not be treated as universal defaults. Definitions
distinguish literal values from target initialization and validate their resolved
application. Styles without output settings preserve the current output choice.

Application becomes concrete saved state. Later modification or deletion of the
definition cannot change that state or accepted outputs. Saving or applying a
Style must not require a GUI-authored preset or modify unrelated states.

## Preview and Inspection

Preview captures a saved state revision and input revision and writes a completed
image file to the chosen output directory. In a Photo Workspace the destination
is its previews area under the CLI's path rules. Results return actual paths,
requested/actual dimensions, rendition identity, engine/resource versions,
display conversion, and whether the rendition still matches the state. Rendering
must not mutate or select the edit.

The standalone invocation owns its synchronous process. It must not return a
fictional service-owned background job on wait timeout or disconnection.
Slipstream may supervise the same executable and report bounded pending work
with an inspectable run identity. Its supervision must not change the captured
revision or rerender a newer state while inspecting an existing run.

Failure or cancellation preserves saved state and prior complete previews. A
late result stays tied to its captured revision and is explicitly stale when
state has advanced. Comparison identifies both operands and transforms and
must not replace a developed operand with the camera JPEG.

The caller can inspect the actual local image with external tools, or download
its indexed file through Slipstream. Existing destinations must not be
overwritten. Statistics disclose region, resolution, color, transfer, and clipping
stage; display clipping is not proof that recoverable RAW highlights were lost.
Detail inspection states scale and quality, uses a qualified region's coordinate
frame, and must not enlarge a small preview as supposed full-detail evidence.
Unsupported detail modes explain the reason and may point to a matching full
Export. Preview success does not establish full-resolution feasibility or
spatial-effect equivalence.

## Output and Export

The output contract belongs to saved state. Callers discover and change compatible
format, sample precision, color space, profile, transfer, geometry, and encoding
under ordinary guarded mutations. Selecting darktable implies no universal
format or fixed module pipeline.

Qualified inputs must support full-resolution IEEE float32 RGB TIFF with a
genuine linear ProPhoto RGB profile. Display-encoded outputs are described
separately under [Development Color Pipeline](../processing/color.md).
Full resolution refers to the edited crop/rotation geometry without an
unrequested resize or upscale. A scene-referred contract conflicting with tone
mapping must fail before execution; the tool must not disable artistic settings
silently or call tone-mapped pixels scene-referred because they have a linear ICC
profile. Display conversion must not become the final render's source.

Export captures a confirmed saved revision, input bytes, complete settings,
resources, engine identity, and output contract. It does not require a preview
or silently capture a later state. A retained revision may be selected explicitly
while its dependencies remain available. The command writes into the workspace's
outputs area or an explicitly chosen accessible directory and returns the actual
completed path, captured revision, byte identity, and verified output facts.

Default names are unique and existing destinations must not be overwritten,
including during concurrent renders. Files complete atomically from private
staging; partial writes are not published as results. Ordinary native file
completion must be visible to Slipstream independently of a callback. Structured
results can enrich verified provenance but are not required to discover a file.
A discovered unknown output is not automatically a proven Processing Artifact.

Standalone export is synchronous and owns its execution lifetime; optional
Slipstream supervision may retain run status, cancellation, and delivery. An
uncertain response uses exact-request reconciliation, not filename coincidence.
Completed files remain independently available after restart and later edits.
[Photo Workspace](../workspace/spec.md) owns download integrity, retention, and
Continue processing; no seven-day expiry applies to required workspace files.

## History and Recovery

Successful mutations retain local saved revisions, including initialized baseline,
changed targets, and input provenance. Reads, previews, and Exports do not add
editing history entries. History survives closure and restart and is inspectable
without the live Original or available engine. It is not a desktop catalog.

Each state retains its current revision and at least its latest 100 successful
saved revisions. The tool advertises its finite limit and oldest retained
revision. Captured revisions and resources referenced by accepted work or kept
outputs survive independently of that browsing limit. An unavailable historical
revision must not resolve to today's state.

Restore creates a new guarded revision and preserves the current input binding.
Different-input history requires explicit supported rebind and validation, not
silent source replacement. Restore must not migrate unsupported settings,
resurrect unavailable input files, or retarget earlier outputs. A stale restore
conflicts under ordinary mutation rules. Undo and redo use this same history.

Atomic state writes preserve the prior revision on interruption or disk-full
failure. Restart recovers confirmed state and discovers complete outputs after
missed file events. It must classify interrupted runs honestly, not promise that
terminated standalone processes are still running. Missing resources, input
changes, schema incompatibility, and capacity failures preserve saved intent and
complete files. Recovery must be possible by inspecting the affected objects and
advertised action rather than reconstructing private parameter blobs or editing
Slipstream's database.

## Acceptance Scenarios

Acceptance must exercise the proposed file/state CLI against real native
processing and workspace file discovery. Schema reads or a direct renderer alone
do not qualify the semantic editing surface. These scenarios are independent
capabilities, not a prescribed editing order:

- A standalone caller opens a local RAW into an explicit state directory, applies
  a Style, inspects a preview file, and exports float32 linear ProPhoto TIFF.
  No Slipstream server, Photo ID, desktop catalog, or preview prerequisite is
  needed for output.
- Integration finds an exact indexed Original Location despite duplicate
  filenames and a separate sibling JPEG, opens its workspace, and addresses an
  explicit darktable state. Several states require selection and keep variants.
- The same grammar creates a curve, second exposure instance, and blended local
  mask. An optional Web exposure control changes the same disk state, preserving
  these objects. An invalid atomic change leaves the whole state unchanged.
- A CLI and Web observe one revision. The CLI saves first; the Web write conflicts
  and retains its draft. Independent state directories can proceed without a
  whole-workspace mutation lock.
- An Agent inspects an output's verified image contract and initializes a new
  compatible state from that file without reading the live Original. Unknown or
  incompatible facts do not cause automatic conversion.
- A Style with two instances sharing a drawn mask round-trips through supported
  export/import. Application creates one target-local mask and reports reference
  mapping. Replay creates no duplicates; incompatible geometry refuses the whole
  application; omitting masks preserves target references.
- An edit contains instances A, B, C, and D, with B and D sharing a mask also
  referenced by A. A two-entry Style maps to A and C in Replace mode. A and C
  keep their IDs and positions, reset to actual target-input defaults, and receive
  captured fields. B, D, and the shared mask retain their exact state. Mask omit
  preserves A's existing mask-dependent fields and relationship. Merge of the
  same mapping changes included fields only and preserves omitted settings of A
  and C. Validate reports each scope and effect without saving.
- The same Style maps both entries to null in Replace mode. Two legal new
  occurrences are created once, ordered by the native contract; A, B, C, D and
  their masks remain unchanged. All-null resets and deletes nothing existing.
- A repeated entry, missing entry, duplicate existing destination, incompatible
  operation mapping, or impossible ordering refuses the whole Style application
  without changing state. Exact replay after a later edit returns the original
  IDs/effects and current revision without another reset or new occurrence.
  Changed mapping under that request identity returns a request conflict.
- A local caller imports a resource and saves a Preset and Style in a workspace
  edit-local catalog. Web and remote integration select that same Photo/Edit,
  list/show/export the same IDs and versions, and can remove an unreferenced
  version with the removal visible to the local caller. No export/import is
  required for the switch to supervision. Multiple edits require selection.
  Unscoped user/configured catalogs remain separately disclosed; a colliding
  name or missing ID never causes fallback. Explicit global save/apply and
  atomic apply/validate resolve every reference in that selected catalog and
  report it; retained revision dependencies remain protected from removal.
- Target-dependent white balance and lens initialization resolve separately for
  different inputs. Applying one Style does not copy the first camera's values
  to the second input as universal defaults.
- A local ICC or LUT is imported and referenced by immutable version. Export
  works without a preview; changing the source file cannot retarget captured
  rendering or break pinned resource references.
- A preview captures one revision and finishes after a later edit. Its file stays
  inspectable as stale and never replaces the current preview. Scene-referred
  output conflicting with a tone mapper is refused without changing settings.
- Native tools write two complete uniquely named outputs without a Slipstream
  callback. Both are discovered, downloadable, and retained after restart.
  The user explicitly chooses a compatible file for later processing.
- A lost mutation response is replayed with its original identity after restart.
  It resolves the original change once and reports any newer current revision.
  Coincidentally matching values or discovered files are not success evidence.
- The live Original becomes unavailable or changes while the workspace snapshot
  stays unchanged. Preview and Export still use the snapshot. Explicit source
  refresh adds a new input and never retargets earlier state or results.
- Disk fills during save or render. The prior confirmed state and completed
  output remain intact, and no partial file is offered as a complete result.
- An engine upgrade cannot interpret a saved native schema. State remains
  readable with a specific limitation; mutation/render refuse silent conversion.
  A supported explicit migration preserves the prior state.
- A camera JPEG Preview exists but the RAW encoding is unsupported. The tool
  reports unsupported input and never substitutes that JPEG as a RAW result.

Every success, refusal, cancellation, failure, and recovery must preserve Original
and associated external XMP bytes. Qualification covers native operations, input
classes, geometry, and output contracts claimed by the capability, including
structurally different editing tasks through the same language.
