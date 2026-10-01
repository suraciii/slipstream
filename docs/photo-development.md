# Photo Development

A Photographer needs to correct a selected RAW Photo and carry the result into
film simulation without repeatedly moving between desktop applications.
Slipstream provides a fixed development path while preserving Original Files
and the camera-produced Preview used for selection.

## Scope

The supported path is RAW development with darktable, followed by Spektrafilm
negative, print, and scan simulation. The first Film Recipe uses Kodak Portra
400 and Kodak Portra Endura.

Slipstream must expose exposure and white-balance controls. It must not require
the Photographer to operate either engine's desktop interface. Development
must support the qualified headless CPU environment; a GPU must not be required.

The capability applies to supported RAW Photos. JPEG Photos retain their own
browsing and selection behavior. Their pixels must not substitute for a RAW
Photo's development input. Supported RAW development must be reported separately
from availability of an embedded camera Preview.

This capability must not introduce crop or retouching controls, masks, batch
editing, arbitrary processing graphs, custom Film Recipes, external editing
history import, or user-visible virtual copies. It does not change the qualified
0.1 release boundary.

The [darktable Integration](darktable-integration.md) specification owns the
product relationship with the engine. Engine discovery must not expand this
editing scope or grant support to unqualified controls.

## Development Controls

Exposure must represent compensation in EV against a documented processing
baseline. White balance must offer as-shot settings, temperature, and tint.
Controls must expose current values, valid ranges, and individual reset actions.
As-shot must use the Photo's own camera information. An unavailable value must
not be presented as a known camera setting.

Reset exposure must restore the processing baseline. Reset white balance must
restore as-shot settings. Reset all must restore both. Resets must be undoable.
The Original's orientation must be respected without requiring a rotation tool.

The Film Recipe must remain fixed. Viewing the Development Result bypasses the
film stage for inspection; it must not change the saved Edit Recipe or redefine
the Film Result. A Finished JPEG must always include the fixed Film Recipe.

## Editing Workspace

Photo View must offer an Edit entry point when development is supported. Edit
opens one workspace for the selected Photo, showing the current result with
the development controls. Camera, Develop, and Film remain distinct in
provenance within one Photo context. Choosing a reference or result never
changes settings or the independent output targets:

- The workspace opens on an Edit Preview of the Development Result. This is
  the default editing view.
- A Film control previews the fixed Film Result. It must be available only
  when the service reports the film capability ready and the current Photo is
  admitted; while Film is unavailable, the control must be hidden or disabled
  with a short plain-language explanation, and it must not appear enabled
  before Film is qualified. If Film becomes unavailable after the workspace
  opens, the workspace must keep editing and Development TIFF export
  available and replace the Film control with the unavailable explanation.
- An Original reference action shows the existing camera-produced Preview. It
  is a distinct action, not an editing view.

A Film Preview or Finished JPEG must never fall back to the camera Preview or
the Development Result, and Slipstream must not label either of them as a
successful Film Result. Entering Edit must restore the current Edit Recipe.
It must not reset settings merely because the Photographer previews Film,
checks the Original reference, or leaves an Album.

The workspace must distinguish edit-state saving, saved, conflict, and failure
from preview updating, current, stale, and failure. It must report saved only
after confirmation. A stale preview remains visible as the last result while
the new result is being generated. Technical evidence such as recipe revisions,
request identities, and processing reasons must remain behind an optional
details affordance. The primary flow must use editing state, edit state file,
editing TIFF, and output rather than recipe or artifact terminology.

Edit availability rests on three independent facts: whether the deployment's
processing engine is usable, whether the Library is still scanning or
recovering, and whether this Photo's Original File currently reads. A
recovering Library or a busy engine must not be presented as an unreadable
Original. While a Photo's source read is pending or waiting for read
capacity, the workspace must keep that Photo's settings read-only, say the
Photo is waiting, and offer to check again; a later read that publishes
current source facts must resume editing without losing confirmed settings.
A confirmed missing or unreadable Original must retain saved editing state,
prior results, and downloadable outputs. New image rendering and exports require
the Original. Exporting an already confirmed edit state file does not require
the Original or a processing engine.

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
A local draft must not override newer server settings silently. If browser recovery storage is unavailable, the workspace must keep the draft
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
different source, stage, or settings snapshot is not the requested preview.

The workspace must continue checking an admitted Preview long enough for a
full RAW development. If the bounded wait ends before settlement, it must say
the result is still unknown and offer a fresh check. It must not keep showing
"rendering" as though it is still following the request.

Comparison must hold the displayed result and its display conversion constant
while comparing the current settings with the as-shot/baseline development
settings. The Original reference must remain a separately labeled action and
must not serve as a comparison image. If either comparison image is pending,
the workspace must say so rather than compare unrelated images.
The Development Result display uses the fixed conversion and clipping
behavior in the
[Development Color Pipeline](../design/development-color.md#display-and-comparison).
That display rendition must not feed the Development TIFF or Film stage.

An Edit Preview must identify its actual dimensions. Reduced-resolution film
simulation is suitable for overall color and tone; it must not be presented as
proof of full-resolution grain or halation detail. Full-detail inspection must
use a qualified full-resolution result and identify its settings. Slipstream
must not silently disable effects to make a preview appear faster.

The initial Film Preview responsiveness target is a warm inclusive-render
95th-percentile latency of at most 4.0 seconds for both approximately 1 MP
orientations (1225 × 816 and 816 × 1225). The measurement includes the
simulation wrapper after initialization, uses the qualified thread policy, and
must retain the complete Film effects. Queueing, admission, process startup,
and image transfer are measured separately as complete request latency and
disclosed to the Photographer; they must not be hidden by the simulation
target. Cold startup and full-resolution Export distributions remain separate
qualification evidence.

## Export Targets

A Development TIFF is the independent handoff after exposure and white balance.
It must contain full developed dimensions, RGB channels with 32-bit floating
point samples, scene-linear ProPhoto RGB pixels, and a matching embedded ICC
profile. It must not include film simulation or a display/look transform.

A Finished JPEG must contain the Film Result at full developed dimensions,
encoded for sRGB with the pinned destination profile and fixed JPEG quality 85. Finished TIFF and expert output options are outside this capability.

The workspace must show separate output cards for the edit state file and
editing TIFF, plus a separate Finished JPEG output when one exists. Viewing
Film must not retarget the editing TIFF action. Each card must show its task
state, generation time, filename, size, and download action. TIFF details must
also show pixel dimensions, orientation, color space, embedded ICC information,
and sample format. Queued, generating, downloadable, failed, and expired are
distinct outcomes.

A confirmed change in editing state marks an older output as based on earlier
settings. This is a warning, not a download refusal. A pending or failed newer
task must not hide the previous successful output. Reopening the Photo must
restore these outputs and unfinished task states from the service. Choosing
Export again captures current confirmed settings under a new task identity;
it must not overwrite an older file.

Export must capture the control settings when the Photographer invokes it.
Those settings must be confirmed by the service before the Export is accepted.
A save failure or conflict must not cause older settings to be exported silently.
Later adjustments must not retarget an accepted Export.

The Photographer must be able to inspect Export state, cancel unfinished work,
and download a completed artifact. Accepted work must survive browser departure.
Downloads must identify their type, size, and expiry when available. Output
filenames must distinguish the two targets and avoid collisions.
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
as an XMP parameter file. Saving or unresolved local settings must block this
action with an explanation. The file must retain exposure, white-balance
intent, the fixed Film Recipe, and its Photo/source/state provenance. Standard
Camera Raw fields may carry only parameters with matching semantics. Unqualified
temperature/tint mappings and Slipstream effects must remain in the Slipstream
namespace; the output must disclose that other editors cannot reproduce all
effects. A file does not establish complete Lightroom interchange.

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
revision checks, Export state, and error semantics. Clients must be able to
discover development support, inspect settings, change supported controls,
request a stage preview, submit an Export, inspect or cancel it, and download
its completed result.

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
Original is unavailable. Develop Edit Previews may apply the numeric exposure
against the proxy's scene-linear baseline and the pinned display transform
without reopening the Original. Film Edit Previews must use the qualified Film
worker over the proxy and must report failure when that worker is unavailable.
The proxy must never make a full-resolution Export admissible: Development TIFF
and Finished JPEG exports require the Original.

## Processing Capacity

The deployment operator must be able to set a finite memory allowance for image
processing. The allowance must apply across active processing work. It must not
be an exposure, white-balance, Film Recipe, or per-Photo control.

Slipstream must report resource availability separately from RAW support and
engine availability. A Photo may support an Edit Preview while its full Export
cannot fit the configured allowance. Queued work must be identified as waiting;
the workspace must not imply that image computation has started.

When an operation cannot fit, Slipstream must identify the affected stage or
output and explain that the processing allowance is insufficient. A runtime
memory failure must preserve saved edits and previously completed outputs. The
Photographer must be able to continue browsing and inspect the failure. A valid
Development TIFF remains available according to its retention policy even when
the Film stage fails.

Processing must not silently reduce Export dimensions, disable film effects,
change numerical quality, raise its allowance, or repeatedly restart the same
failed attempt to obtain a result. Explicit retry must keep the captured image
intent and check current resource availability. If the deployment cannot enforce
its allowance, processing must be unavailable while normal browsing remains
usable. Increasing the allowance must not change a successfully rendered look.

## Failure and Retention

An unavailable engine must leave selection and browsing usable and explain
which editing capability is unavailable. Unsupported input, resource rejection,
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

A successful Development TIFF, its captured Export snapshot, and its request
receipt must remain available for seven days after publication. The interface
must disclose the expiry. An active download must hold a lease that keeps the
artifact, snapshot, and receipt available until the response stream settles.

The deployment must enforce a finite retained-output allowance. If the service
cannot reserve enough space for a complete new Development TIFF within that
allowance, it must refuse the Export before accepting the work. It must not
evict a Development TIFF before its disclosed expiry or while a download lease
is active. A storage refusal must leave saved settings and completed Exports
available and explain that retained-output capacity is insufficient.

Other Export targets must also have a bounded, disclosed retention period.
After expiration, regeneration must identify missing source or engine
requirements rather than silently change the result. Engine upgrades must not
silently change saved looks.

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
- The Film engine fails after a Development TIFF has completed. That TIFF may
  remain downloadable. The Film Result must remain failed, not successful.
- A Film Edit Preview succeeds but the full Finished JPEG exceeds the configured
  processing allowance. The Export must fail with a resource explanation while
  the Edit Recipe and successful preview remain available. A retry after the
  operator changes capacity must use the Export's captured settings.

[Photo Development Architecture](../design/photo-development.md) owns execution,
concurrency, and storage contracts. [Development Color Pipeline](../design/development-color.md)
owns the engine and color boundary. [Photo Previews](previews.md) continues to
own camera Preview behavior.
