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

Photo View must offer an Edit entry point when development is supported. The
workspace must expose Camera, Develop, and Film views with clear provenance:

- Camera shows the existing camera-produced Preview.
- Develop shows an Edit Preview of the Development Result.
- Film shows an Edit Preview of the Film Result and is the default editing view
  when the film capability is available.

If a stage is unavailable, its control must explain why. Slipstream must not
label a camera Preview or Development Result as a successful Film Result.
Entering Edit must restore the current Edit Recipe. It must not reset settings
merely because the Photographer changes views or leaves an Album.

The Grid must retain its camera-produced thumbnails and indicate Photos with
saved edits. Selection State, Rating, Album membership, and browsing position
must remain independent of Edit Recipe changes.

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

Comparison must keep the chosen stage and display conversion constant while
comparing the current settings with the as-shot/baseline development settings.
Camera reference must remain a separately labeled view. If either comparison
image is pending, the workspace must say so rather than compare unrelated images.
The Development view uses the fixed conversion and clipping behavior in the
[Development Color Pipeline](../design/development-color.md#display-and-comparison).
That display rendition must not feed the Development TIFF or Film stage.

An Edit Preview must identify its actual dimensions. Reduced-resolution film
simulation is suitable for overall color and tone; it must not be presented as
proof of full-resolution grain or halation detail. Full-detail inspection must
use a qualified full-resolution result and identify its settings. Slipstream
must not silently disable effects to make a preview appear faster.

## Export Targets

A Development TIFF is the independent handoff after exposure and white balance.
It must contain full developed dimensions, RGB channels with 32-bit floating
point samples, scene-linear ProPhoto RGB pixels, and a matching embedded ICC
profile. It must not include film simulation or a display/look transform.

A Finished JPEG must contain the Film Result at full developed dimensions,
encoded for sRGB with a matching profile and a documented fixed quality setting.
Finished TIFF and expert output options are outside this capability.

The export interface must name the intended output and processing stage. It
must not present an ambiguous TIFF choice that could refer to either an
intermediate development image or a finished film image.

Export must capture the control settings when the Photographer invokes it.
Those settings must be confirmed by the service before the Export is accepted.
A save failure or conflict must not cause older settings to be exported silently.
Later adjustments must not retarget an accepted Export.

The Photographer must be able to inspect Export state, cancel unfinished work,
and download a completed artifact. Accepted work must survive browser departure.
Downloads must identify their type, size, and expiry when available. Output
filenames must distinguish the two targets and avoid collisions.

Exports and intermediate files must remain outside the Library Folder. They
must not create new Photos automatically or overwrite Original Files. Downloads
must include correct color/orientation information and basic capture metadata;
GPS and private device identifiers must be omitted. External XMP edit history
must not be copied into output as a claim of supported editing interchange.

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

A successful Development TIFF and its captured snapshot must remain available
for seven days after publication. The interface must disclose the expiry. An
active download must hold a lease that keeps the artifact and snapshot
available until the response stream settles.

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
