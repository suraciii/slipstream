# Photo Development

A Photographer needs to process a selected RAW Photo or a compatible
Processing Artifact with an admitted Processing Module without repeatedly
moving between desktop applications. Slipstream exposes peer modules that the
Photographer composes explicitly. It preserves Original Files and the
camera-produced Preview used for selection.

## Scope

The capability provides Processing Modules as peers: darktable performs RAW
development, and standalone SpektraFilm performs negative, print, and scan
simulation. The SpektraFilm integration inside darktable is not a module of
this capability. Slipstream must not define a fixed order between modules,
chain them automatically, or treat one as a stage of the other. The first Film
Recipe uses Kodak Portra 400 and Kodak Portra Endura.

Slipstream must expose exposure and white-balance controls as darktable's
qualified editing surface. It must not require the Photographer to operate a
module's desktop interface. Processing must support the qualified headless
CPU environment; a GPU must not be required.

RAW development applies to supported RAW Photos admitted by the darktable
module. JPEG Photos retain their own browsing and selection behavior. Their
pixels must not substitute for a RAW Photo's development input. Each module
must report its own input support separately from availability of an embedded
camera Preview.

This capability must not introduce crop or retouching controls, masks, batch
editing, a workflow graph editor or automatic step planning, custom Film
Recipes, external editing history import, or user-visible virtual copies. A
module's discovery must not admit engine controls beyond its qualified
product surface. It does not change the qualified 0.1 release boundary.

## Processing Steps and Composition

The object of editing is one selected Processing Step: one Processing Module,
one identified input, and one captured parameter snapshot. The input is the
Photo's Original File or a published Processing Artifact the caller chose
explicitly.

A caller may compose zero, one, or many steps, repeat a module, or select a
different module at any time. Slipstream must not impose a step order, invoke
a module automatically, or insert a conversion between steps. Choosing a
published artifact as the next input is the only coupling between steps:

```text diagram
Original
  -> selected module Edit Preview
  -> selected module Export -> immutable artifact a1
  -> another selected module Edit Preview over a1
  -> another selected module Export -> immutable artifact a2
```

This is one example, not the required path. A caller may stop after any
Preview, use one module only, or repeat a module.

Module compatibility is decided at the module boundary. A shared file
extension or format name must not imply compatible color space, transfer
function, precision, or geometry. A module must refuse an input, parameter,
or output combination it does not support. Slipstream must not convert
between module formats implicitly or answer a refused step with another
module.

Each module must report its own availability, refusal reasons, compatible
inputs and outputs, editing parameters, and limits through the read-only
module discovery operation defined in
[Processing Modules](../design/processing-modules.md#interface-and-discovery).
There is no separate Film capability state, and one module's availability must
not be reported as another's. Discovery describes what a module can do; it must
not by itself expose new controls or admit engine-private parameters into the
product.

The [darktable Integration](darktable-integration.md) specification owns the
product relationship with the engine. Engine discovery must not expand this
editing scope or grant support to unqualified controls.

## Development Controls

The darktable module's qualified editing surface is exposure and white
balance. Exposure must represent compensation in EV against a documented
processing baseline. White balance must offer as-shot settings, temperature,
and tint. Controls must expose current values, valid ranges, and individual
reset actions. As-shot must use the Photo's own camera information. An
unavailable value must not be presented as a known camera setting.

Reset exposure must restore the processing baseline. Reset white balance must
restore as-shot settings. Reset all must restore both. Resets must be
undoable. The Original's orientation must be respected without requiring a
rotation tool.

Parameters belong to their module and stay captured in each Processing Step.
Inspecting another module's result must not change the saved Edit Recipe or
redefine another step's result. A Film Recipe remains fixed only when the
standalone SpektraFilm module is selected; it is not a global output concept.

Automatic adjustments are explicit actions on the current Processing Step.
When a module admits one, the Photographer supplies the module's automatic
instruction and Slipstream asks the native engine for the concrete values it
computed. The saved step contains those concrete values; Preview and Export
reuse them and do not independently rerun the automatic algorithm. A failed
automatic action or a stale guarded Recipe leaves the saved step unchanged.

## Editing Workspace

Photo View must offer an Edit entry point when a Processing Module admits the
Photo. Edit opens one workspace for the selected Photo, showing the current
Processing Step's result with that module's editing controls. The workspace
must not present separate stage, module, or result views or tabs the
Photographer must choose between. The results remain distinct in provenance,
but the Photographer acts on one current step:

- The workspace opens on the selected current step's Edit Preview. With no
  selected step, it waits for the caller to choose a module and compatible input.
- Selecting a saved step restores that step's captured module, input, and
  parameters. Creating another step may repeat the same module with a different
  input or parameter snapshot. A module choice must be
  available only when that module reports itself ready and admits the current
  Photo and input; while a module is unavailable, its choice must be hidden
  or disabled with a short plain-language explanation, and it must not appear
  enabled before the module is qualified. If a module becomes unavailable
  after the workspace opens, the workspace must keep editing and Export
  available for the remaining modules and replace the unavailable module's
  choice with the unavailable explanation.
- An Original reference action shows the existing camera-produced Preview. It
  is a distinct action, not an editing view.

The workspace must identify the current step's module and input in plain
language. An Edit Preview or Export must never fall back to the camera
Preview, another module's result, or a display-only derivative, and
Slipstream must not label any of them as a successful result of the requested
step. Entering Edit must restore the current Edit Recipe. It must not reset
settings merely because the Photographer selects another module, checks the
Original reference, or leaves an Album.

The workspace must distinguish edit-state saving, saved, conflict, and failure
from preview updating, current, stale, and failure. It must report saved only
after confirmation. A stale preview remains visible as the last result while
the new result is being generated. Technical evidence such as recipe revisions,
request identities, and processing reasons must remain behind an optional
details affordance. The primary flow must use editing state, edit state file,
editing TIFF, and output rather than recipe or artifact terminology.

Edit availability rests on three independent facts: whether the selected
Processing Module is usable for the selected input, whether the Library is
still scanning or recovering, and whether the Photo's Original File currently
reads. A recovering Library or a busy module must not be presented as an
unreadable Original. While an Original-backed step's source read is pending or
waiting for read capacity, the workspace must keep that Photo's settings
read-only, say the Photo is waiting, and offer to check again; a later read
that publishes current source facts must resume editing without losing
confirmed settings.

A step whose input is a retained, compatible Processing Artifact may continue
while its Original is unavailable, subject to that artifact's lease, expiry,
module availability, and compatibility checks. It must not reopen or restage
the missing Original. A confirmed missing or unreadable Original disables
Original-backed Preview and Export for the current source and must be explained
as permanent. Artifact-backed Preview and Export use only the retained artifact
and its captured image contract; they fail explicitly when that artifact or the
selected module is unavailable. A refusal caused by the Photo's source state
must name the same reason the Edit read reports.
Confirmed missing or unreadable Originals must retain saved editing intent,
prior results, and downloadable outputs. Exporting an already confirmed edit
state file requires neither the Original nor a processing engine.

The Grid must retain its camera-produced thumbnails and indicate Photos with
saved edits. Its bounded Photo summaries must report whether a saved Edit
Recipe exists. This fact remains true when the recipe matches the processing
baseline or its Original File is unavailable; it does not imply that processing
is currently available. Selection State, Rating, Album membership, and
browsing position must remain independent of Edit Recipe changes.

Desktop controls must sit beside the image. On a narrow screen, the image and
controls must remain reachable without horizontal page scrolling. Labels,
values, reset, comparison, and undo must support keyboard and touch use. Editing
and comparison gestures must not select, reject, or navigate a Photo.

## Autosave and Reversible Editing

One completed pointer drag, committed numeric entry, or settled keyboard
adjustment is one edit action. Slipstream must automatically save the resulting
Edit Recipe. It must not require a Save button or save continuously at every
intermediate pointer position.

The workspace must distinguish saving, saved, and save failure. It must report
saved only after the service confirms the matching settings. A late response
must not mark later settings saved. Normal navigation to another Photo must
retain ownership of a pending save without a confirmation dialog.

Pending settings must have bounded recovery in the current browser. Recovered
settings must be identified as a local draft until the service confirms them.
A local draft must not override newer server settings silently. If browser
recovery storage is unavailable, the workspace must keep the draft
for the current session, disclose the lack of reload recovery, and continue
guarded online saving. It must not claim that such a draft will survive browser
closure or evict another unconfirmed draft to make room.

Undo and redo must operate on edit actions in the current browser editing
session. They must save the resulting settings under the same conflict rules.
Reopening a Photo must restore saved settings; it need not restore a durable
history of edit actions.

If another client changes the Edit Recipe, autosave must stop for that Photo
and retain the local settings. The Photographer must be able to use the saved
recipe or explicitly reapply the local settings against the newly observed
revision. The latter may conflict again. Neither action may silently overwrite
a later update. An uncertain save outcome must be reconciled before dependent
writes or exports proceed.

## Preview Behavior

Controls must respond immediately to input. A preview request must follow a
completed edit action. The workspace must retain the last successful image
while a new one is pending and visibly identify that it is out of date.

Saved settings and completed previews are separate facts. Rendering failure
must not discard a saved recipe or a recoverable draft. Late, cancelled, or
obsolete results must not replace the current view. A successful image for a
different source, module, or settings snapshot is not the requested preview.

An Edit Preview is a bounded execution of the current Processing Step. It must
use that step's input and parameter snapshot and a bounded rendition geometry.
It must not create a full-resolution handoff artifact. A preview identity must
cover the input identity, selected module, parameter snapshot, rendition
geometry, processing bundle, and display conversion. If the bounded wait ends
before settlement, the workspace must say the result is still unknown and
offer a fresh check. It must not keep showing "rendering" as though it is
still following the request.

Comparison must hold the displayed result and its display conversion constant
while comparing the current settings with the module's as-shot or baseline
settings. The Original reference must remain a separately labeled action and
must not serve as a comparison image. If either comparison image is pending,
the workspace must say so rather than compare unrelated images.
An Edit Preview display uses the fixed conversion and clipping behavior in the
[Development Color Pipeline](../design/development-color.md#display-and-comparison).
That display rendition must not feed an Export or another Processing Step.

An Edit Preview must identify its actual dimensions. A reduced-resolution
rendition is suitable for overall color and tone; it must not be presented as
proof of full-resolution grain or halation detail. Full-detail inspection must
use a qualified full-resolution Processing Artifact and identify its settings.
Slipstream must not silently disable effects to make a preview appear faster.

The SpektraFilm module's initial Edit Preview responsiveness target is a warm
inclusive-render 95th-percentile latency of at most 4.0 seconds for both
approximately 1 MP orientations (1225 × 816 and 816 × 1225). The measurement
includes the simulation wrapper after initialization, uses the qualified
thread policy, and must retain the complete Film effects. Queueing, admission,
process startup, and image transfer are measured separately as complete
request latency and disclosed to the Photographer; they must not be hidden by
the simulation target. Cold startup and full-resolution Export distributions
remain separate qualification evidence.

## Export

An Export is a separate explicit execution of one Processing Step. It must
capture the confirmed input identity, module, parameters, processing bundle,
and output contract, and it must validate and publish an immutable Processing
Artifact before that artifact can become the input of a later step. Output
format and its precision, color, transfer, geometry, and encoding options are
ordinary parameters of the selected module. The product has no universal image
target or fixed module pair.

The selected module owns the output label and contract shown to the
Photographer. A module must state whether its output is full-resolution,
scene-referred, display-encoded, or otherwise bounded; the service must
validate the actual geometry, samples, color contract, transfer function,
profile, metadata, and encoding before publication. An output contract is not
implied by a file extension or a module name. Output options outside the
qualified module surface are refused.

The workspace must offer one Export action for the current step and identify
the selected module and output contract in plain language. It must not present
an ambiguous universal format choice or silently invoke another module.
Programmatic clients address the Processing Step explicitly; the module
discovery and direct-call contract owns those parameters.

Export states must use the same plain language as the workspace status:
`Exporting` while accepted work is unfinished, `Ready to download` on
completion, and `This export has expired. Export again` after expiry.
Retained output cards identify their module and concrete output contract, task
state, generation time, filename, size, and download action. TIFF details also
show dimensions, orientation, color space, embedded ICC information, and sample
format. Queued, generating, downloadable, failed, and expired are distinct states.
A confirmed edit marks an older output as based on earlier settings without
refusing its download. Pending or failed newer work must not hide earlier output.
Reopening the Photo restores retained outputs and unfinished work from the
service. Exporting current confirmed settings creates a new task and file.

Export must capture the step's input, module, and control settings when the
Photographer invokes it. Those settings must be confirmed by the service
before the Export is accepted. A save failure or conflict must not cause older
settings to be exported silently. Later adjustments must not retarget an
accepted Export. A published Processing Artifact must stay bound to the input,
module, parameters, bundle, and image contract it was produced from, and a
later change to an upstream step must not retarget it.

The Photographer must be able to inspect Export state, cancel unfinished work,
and download a completed artifact. Accepted work must survive browser departure.
Downloads must identify their type, size, and expiry when available. Output
filenames must distinguish the outputs and avoid collisions.
The browser must verify the complete artifact length and SHA-256 before offering
the file. Large-file integrity checks must leave the interface responsive on
both HTTP and HTTPS.

Exports and intermediate files must remain outside the Library Folder. They
must not create new Photos automatically or overwrite Original Files. Downloads
must include correct color/orientation information and basic capture metadata;
GPS and private device identifiers must be omitted. External XMP edit history
must not be copied into output as a claim of supported editing interchange.

### Edit State File

The Photographer must be able to export the current confirmed editing state
as an XMP parameter file from an explicitly selected darktable step. Saving or
unresolved local settings must block this action with an explanation. The file
must retain the complete recipe, selected step, semantic exposure and white-balance
intent, and Photo/source/state provenance. A Film Recipe is included when it is
present in the saved steps. The action must refuse a selection that cannot provide
unambiguous semantic exposure and white balance. Standard Camera Raw fields may
carry only parameters with matching semantics. Unqualified temperature/tint
mappings and Slipstream effects must remain in the Slipstream namespace; the
output must disclose that other editors cannot reproduce all effects. A file
does not establish complete Lightroom interchange.

An edit state file is a service-owned output, not an associated XMP Sidecar.
It must never write beside an Original or become another source of truth.
XMP import and bidirectional synchronization remain outside scope. Its
confirmed snapshot must be downloadable without an available Original or
processing engine, including after departure and service restart. The existing
seven-day output retention policy applies.

Repeated download and retry of one confirmed edit state file must return the
same file and integrity evidence throughout retention, even after a later edit.
After expiry the service must explain that the file is unavailable and require
a new export request. An unavailable state store must be reported as a service
failure rather than as an unknown Photo or missing file.

Before claiming external-editing compatibility, the editing TIFF must be
opened in at least one named target editor, checking color, pixel dimensions,
orientation, and sample format. Download success alone does not prove this.

## Shared Human and Programmatic Use

Web and programmatic clients must use the same Photo identity, Edit Recipe,
Processing Step, revision checks, Export state, and error semantics. The
processing service must be able to discover each module's availability and its
supported inputs, outputs, parameters, and limits through the read-only module
discovery operation. Clients receive only the controls and combinations the
product surface admits; the closed CLI and HTTP profile is not a workflow DSL
or a general engine-discovery command. Clients can inspect settings, change
supported controls, request an Edit Preview for a Processing Step, submit an
Export, inspect or cancel it, and download its completed result.

Programmatic changes must be visible when the Photographer opens the Photo in
the Web. A client must not need the service's filesystem paths, SQLite database,
engine settings files, or browser automation to perform these operations.
Command syntax and wire details must have one authoritative client reference.

## Original Availability and Recovery

Edits must remain associated with their Photo through exact-content Location
Recovery. A changed Original's contents must invalidate processing results;
Slipstream must retain the earlier settings and require explicit confirmation
before applying them to different content. Missing Originals must not cause
saved settings to be deleted.

A Photo with saved editing intent or retained Export references must count as
referenced when assessing Retire and Bind eligibility. Recovery must not retire
editing state as though it were an unused scan record.

When a valid Development Proxy exists for the last observed source revision,
Slipstream must expose that fact as `editSource: "development-proxy"` while the
Original is unavailable. An Edit Preview may use the proxy only within the
current step's module boundary: a qualified module may apply its admitted
parameters against the proxy's compatible image contract without reopening the
Original. A module that cannot produce its Preview from the proxy must report
failure; it must not fall back to another input or module. The proxy must never
make a full-resolution Export admissible. An Original-backed Export requires
the Original; an artifact-backed Export uses only its retained compatible
artifact and its captured image contract.

## Processing Capacity

Slipstream runs as one application on the Photographer's machine. The Web
interface and the development engine share that application's finite memory
and computing allowance, and only one development request runs at a time; a
second request waits its turn instead of competing for memory. The allowance
is deployment configuration, not an exposure, white-balance, Film Recipe, or
per-Photo control.

Slipstream must report resource availability separately from RAW support and
module availability. A Photo may support an Edit Preview while its full Export
cannot fit the configured allowance. Queued work must be identified as waiting;
the workspace must not imply that image computation has started.

When an operation cannot fit, Slipstream must identify the affected module or
output and explain that the processing allowance is insufficient. A runtime
memory failure must preserve saved edits and previously completed outputs. The
Photographer must be able to continue browsing and inspect the failure. A
previously completed Processing Artifact remains available according to its
retention policy even when a later step fails.

Because the allowance is shared, a severe engine failure can stop the whole
application; restarting Slipstream recovers it, and saved edits and completed
outputs survive. Processing must not silently reduce Export dimensions,
disable film effects, change numerical quality, or repeatedly restart the same
failed attempt to obtain a result. Explicit retry must keep the captured image
intent and check current resource availability. Increasing the allowance must
not change a successfully rendered look.

## Failure and Retention

An unavailable module must leave selection and browsing usable and explain
which Processing Module is unavailable. Unsupported input, resource rejection,
render interruption, insufficient storage, and missing processing assets must
produce actionable failures without changing Originals or losing recipes.

A completed artifact must not be exposed before it is fully written and
validated. An interrupted Export must have an explicit recoverable outcome.
Cancellation must settle to the actual terminal result if completion races it.
A dropped response alone must not imply that the request failed to take effect.

In the browser, an Export submission whose response is lost or cannot establish
acceptance retains its Photo's request identity and captured settings. Reconcile
repeats that exact request; a new Export or a dependent edit of that Photo waits
until the service resolves it. Other Photos remain editable. This in-session
recovery does not survive a browser reload; retained Exports can be inspected
after reopening, but an unconfirmed request cannot be inferred from a list.

Edit Recipes require backup. Intermediate images and Edit Previews may be
reconstructed while their source and processing assets remain available.
Every accepted Export's status receipt and captured snapshot must remain
available until terminal settlement and for seven days afterward. Repeating an
accepted request with the same identity and payload must resolve to that Export
without starting another attempt. Reusing its identity with a different
payload must be refused. An explicit retry must use a new request identity and
may use the captured snapshot only while it remains retained. After a receipt
expires, repeating that request must return an explicit expired outcome and
must not create an Export. Expiry must not make the old identity available for
new work. A new Export after expiry requires a new request identity and
confirmation of the current source and settings.

A successful Processing Artifact, its captured Export snapshot, and its request
receipt must remain available for seven days after publication. The interface
must disclose the expiry. An active download must hold a lease that keeps the
artifact, snapshot, and receipt available until the response stream settles.

The deployment must enforce a finite retained-output allowance. If the service
cannot reserve enough space for a complete selected-module output within that
allowance, it must refuse the Export before accepting the work. It must not
evict a retained artifact before its disclosed expiry or while a download lease
is active. A storage refusal must leave saved settings and completed Exports
available and explain that retained-output capacity is insufficient.

After expiry, regeneration must identify missing input or module requirements
rather than silently change the result. Module upgrades must not silently
change saved looks.

## Examples

- A Photographer drags exposure once and opens the next Photo. The first
  Photo's matching save completes independently. Its result must not change the
  controls or preview for the second Photo.
- A Photographer clicks Export at +0.5 EV and then changes exposure to +1 EV.
  The accepted Export must remain at +0.5 EV; the Photo may subsequently save
  +1 EV as its current recipe.
- A CLI changes white balance while a browser has pending exposure changes.
  The browser must retain its draft and report a conflict instead of overwriting
  the CLI's recipe.
- A Photographer previews one selected module only and closes the Photo. No
  artifact must be created, and another module must not be invoked.
- A Photographer Exports one module's result and explicitly selects the
  published artifact as the next module's input. If the next module fails, the
  first artifact remains downloadable and the second result remains failed.
- After an artifact is published, the Photographer changes an upstream step.
  The artifact remains bound to its captured input, parameters, bundle, and
  image contract; the change does not retarget it.
- A module Preview succeeds but its full selected output exceeds the configured
  processing allowance. The Export must fail with a resource explanation
  while the Edit Recipe and successful Preview remain available. A retry after
  the operator changes capacity must use the Export's captured settings.

[Photo Development Architecture](../design/photo-development.md) owns execution,
concurrency, and storage contracts.
[Processing Modules](../design/processing-modules.md) owns module identity,
the execution boundary, and interface compatibility.
[Development Color Pipeline](../design/development-color.md)
owns the engine and color boundary. [Photo Previews](previews.md) continues to
own camera Preview behavior.
