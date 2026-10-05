# Photo Development

The concise domain contract is [Photo Editing Model](photo-editing-model.md).
This document adds the product behavior and retention details behind that
contract.

A Photographer needs to adjust one Photo, inspect the current result,
and request an output without repeatedly moving between desktop applications.
Slipstream owns the Photo's current Edit State and exposes peer Processing
Engines through one guarded editing surface. A completed Export produces an
immutable Processing Artifact that another service can consume. Original Files
and the camera-produced Preview used for selection remain unchanged.

## Scope

The capability provides Processing Engines as peers: darktable performs
RAW development, and standalone SpektraFilm performs negative, print, and scan
simulation. The SpektraFilm integration inside darktable is not a Processing
Engine in this capability. Slipstream must not define a fixed order, chain
engines automatically, or treat one engine as a stage of another. A completed
Export is the handoff boundary; a downstream service starts its own Edit State
from the resulting Artifact.

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
editing, a workflow graph editor or automatic processing planning, custom Film
Recipes, external editing history import, or user-visible virtual copies. A
module's discovery must not admit engine controls beyond its qualified
product surface. It does not change the qualified 0.1 release boundary.

## Edit State and Artifact Handoff

The object of editing is one current Edit State for one Photo. It contains one
selected Processing Engine, one explicit input identity, the Engine Module
Controls admitted for that input, and a guarded edit revision. A successful
control change replaces that state atomically.

Export captures the confirmed Edit State and produces one immutable Processing
Artifact. The Artifact carries its input identity, Engine, concrete Controls,
bundle and schema identity, source revision, output contract, Export identity,
digest, and retention facts. Another service may accept that Artifact as input
and owns its own Edit State. Slipstream does not maintain a cross-service
editing pipeline or a user-visible sequence of steps.

Module compatibility is decided at the Processing Engine boundary. A shared
file extension or format name must not imply compatible color space, transfer
function, precision, or geometry. An Engine must refuse an input, Control, or
output combination it does not support. Slipstream must not convert between
Engine formats implicitly or answer a refusal with another Engine.

Each Engine must report its availability, refusal reasons, compatible inputs and
outputs, product Controls, and limits through the read-only discovery operation
defined in [Processing Modules](../design/processing-modules.md#interface-and-discovery).
Discovery describes what an Engine can do; it must not by itself expose native
history, catalog state, host paths, or unqualified parameters.

The [darktable Integration](darktable-integration.md) specification owns the
product relationship with the darktable Engine. Engine discovery must not
expand this editing scope or grant support to unqualified Controls.

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

Controls belong to the current Edit State and remain owned by their
Engine Module. Inspecting another Engine or requesting an Artifact must not
change the confirmed Edit State.

Automatic adjustments are explicit actions on the current Edit State. When an
Engine admits one, the Photographer supplies the Engine's instruction and
Slipstream asks the native engine for the concrete values it computed. Those
values are saved into the Edit State before Preview or Export uses them. A
failed automatic action or a stale guarded edit revision leaves the confirmed
state unchanged.

## Editing Workspace

Photo View must offer an Edit entry point when a Processing Engine admits the
Photo. Edit opens one workspace for the selected Photo and its current Edit
State. The workspace shows the current Edit Preview beside the admitted
Controls; it must not present separate stage, module, or result views that make
the Photographer manage an internal pipeline.

When no Edit State exists, the Photographer chooses one available Processing
Engine and compatible input. A later Engine choice starts a separate service
operation or requires an explicit Artifact input; it does not silently append a
second invocation to the Photo's existing state. If an Engine becomes unavailable, the
workspace keeps the confirmed Edit State and explains which operation can no
longer run.

An Original reference action shows the existing camera-produced Preview. It is
a distinct action, not an editing view. An Edit Preview or Export must never
fall back to the camera Preview, another Engine, or a display-only derivative.

The workspace identifies the current Engine, input, and Controls in plain
language. It distinguishes edit-state saving, saved, conflict, and failure
from Preview updating, current, stale, and failure. Technical evidence such as
revision tokens, request identities, and processing reasons stays behind an
optional details affordance. The primary flow uses editing state, editing TIFF,
and output language.

Edit availability rests on three independent facts: whether the selected
Processing Engine admits the input, whether the Library is still scanning or
recovering, and whether the Photo's Original File currently reads. A recovering
Library or a busy Engine must not be presented as an unreadable Original.

When a service accepts a retained Processing Artifact, that Artifact is the
explicit input identity for the service's own Edit State. The service uses the
Artifact's actual image contract and retention state; it must not reopen or
re-stage the upstream Original, mutate the upstream state, or assume a latest
result. A confirmed missing or unreadable Original disables Original-backed
Preview and Export while preserving the saved Edit State and prior Artifacts.

## Autosave and Reversible Editing

One completed pointer drag, committed numeric entry, or settled keyboard
adjustment is one edit action. Slipstream automatically saves the resulting
Edit State. It must not require a Save button or save continuously at every
intermediate pointer position.

The workspace reports saving, saved, conflict, and save failure only from the
matching service response. A late response must not mark a later state saved.
Normal navigation retains ownership of a pending update without a confirmation
dialog.

Pending settings have bounded recovery in the current browser. Recovered
settings are local drafts until the service confirms them. A draft must not
silently override newer service state. Undo and redo operate on edit actions in
the current browser session and submit new guarded updates; the product does
not promise a durable action history.

If another client changes the Edit State, autosave stops for that Photo and
retains the local draft. The Photographer can keep the confirmed state or
explicitly retry the draft against the newly observed revision. Neither action
silently overwrites a later update. An uncertain save outcome is reconciled
before dependent Preview or Export requests proceed.

## Preview Behavior

Controls respond immediately to input. An Edit Preview request follows a
confirmed edit action. The workspace retains the last successful image while a
new one is pending and visibly identifies it as stale after a later accepted
edit.

An Edit Preview executes the current Edit State against its captured input and
Controls at bounded rendition geometry. Its identity covers Photo, input,
Engine, Controls, bundle, geometry, and display conversion. It must not create
a full-resolution handoff Artifact, invoke another Engine, or substitute a
Camera Preview. Late, cancelled, failed, or obsolete results must not replace
the current view.

Comparison keeps the displayed result and display conversion constant while
comparing current Controls with the Engine's as-shot or baseline Controls. The
Original reference remains a separately labelled action. Reduced Preview
resolution is not proof of full-resolution detail.

## Export

Export is a separate explicit execution of the confirmed Edit State. It
captures the input identity, Engine, concrete Controls, bundle/schema, and
output contract at acceptance. It does not capture settings that become
current after queueing.

Slipstream validates actual output type, geometry, precision, color and
transfer contract, metadata, byte size, and content digest before publishing
one immutable Processing Artifact. The Artifact is the service handoff
object; a receiving service validates its actual image contract and starts its
own Edit State. Export never modifies an Original File or silently invokes
another Engine.

Export states are queued, exporting, ready to download, failed, cancelled, or
expired. Accepted work survives browser departure under the durable settlement
rules. A confirmed later edit marks an earlier Artifact as based on older
settings but does not remove or retarget it. A failed or cancelled Export
does not publish partial output.

## Edit State File

The Photographer may export the current confirmed Edit State as an XMP
parameter file when the selected Engine has a qualified semantic mapping.
Saving or unresolved local settings blocks the action. The file records the
Photo and source identity, Engine, Controls, bundle/schema, and edit revision.
It is an output for the current state, not an XMP Sidecar, a cross-service plan,
or another source of truth.

The file must remain downloadable without an available Original or Engine once
its confirmed output has been retained. Repeated download and retry of one
confirmed file returns the same bytes and integrity evidence through retention.
A file does not establish complete Lightroom or darktable interchange.

## Shared Human and Programmatic Use

Web and programmatic clients use the same Photo identity, Edit State revision,
source checks, Engine availability, Preview state, Export state, and Artifact
facts. Clients receive only the Controls and combinations that the product
surface admits. The closed CLI and HTTP profile is not a workflow DSL or a
general Engine-discovery command.

Programmatic changes are visible when the Photographer opens the Photo in the
Web. A client does not need filesystem paths, SQLite data, engine settings
files, or browser automation. Command syntax and wire details have one
authoritative client reference.

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
selected Engine Module boundary: a qualified module may apply its admitted
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
retention policy even when a later Export fails.

Because the allowance is shared, a severe engine failure can stop the whole
application; restarting Slipstream recovers it, and saved edits and completed
outputs survive. Processing must not silently reduce Export dimensions,
disable film effects, change numerical quality, or repeatedly restart the same
failed attempt to obtain a result. Explicit retry must keep the captured image
intent and check current resource availability. Increasing the allowance must
not change a successfully rendered look.

## Failure and Retention

An unavailable Engine must leave selection and browsing usable and explain
which Processing Engine is unavailable. Unsupported input, resource rejection,
render interruption, insufficient storage, and missing processing assets must
produce actionable failures without changing Originals or losing the saved Edit
State.

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

The saved Edit State and its internal recovery snapshot require backup.
Intermediate images and Edit Previews may be reconstructed while their source
and processing assets remain available.
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
  +1 EV as its current Edit State.
- A CLI changes white balance while a browser has pending exposure changes.
  The browser must retain its draft and report a conflict instead of overwriting
  the CLI's current Edit State.
- A Photographer previews one selected Engine Module only and closes the Photo.
  No Artifact must be created, and another Engine must not be invoked.
- A Photographer Exports one Engine's result and explicitly selects the
  published Artifact as the next service's input. If the next service fails, the
  first artifact remains downloadable and the second result remains failed.
- After an Artifact is published, the Photographer changes the upstream Edit
  State.
  The artifact remains bound to its captured input, parameters, bundle, and
  image contract; the change does not retarget it.
- A module Preview succeeds but its full selected output exceeds the configured
  processing allowance. The Export must fail with a resource explanation
  while the Edit State and successful Preview remain available. A retry after
  the operator changes capacity must use the Export's captured settings.

[Photo Development Architecture](../design/photo-development.md) owns execution,
concurrency, and storage contracts.
[Processing Modules](../design/processing-modules.md) owns module identity,
the execution boundary, and interface compatibility.
[Development Color Pipeline](../design/development-color.md)
owns the engine and color boundary. [Photo Previews](previews.md) continues to
own camera Preview behavior.
