# Photo Development

A Photographer needs a place to edit a photograph, inspect the result, and keep
files for download or later processing.

A [Photo Workspace](workspace/spec.md)
is a real directory containing an independent input snapshot, saved tool state,
previews, and outputs.

A tool reads and writes files there directly.

The
Photographer MUST NOT download, upload, or submit an editing file on every
iteration.

## Scope

Processing Engines are peer tools. darktable provides general image editing and RAW
development; standalone SpektraFilm provides negative, print, and scan simulation.

The SpektraFilm integration inside darktable is not the standalone module.

Slipstream MUST NOT impose an order, automatically chain modules, or insert a
conversion.

The initial standalone Film Recipe uses Kodak Portra 400 and Kodak
Portra Endura; it does not restrict darktable's editing surface.

[Headless darktable Editing](darktable/editing.md) owns operation instances,
structured controls, masks, resources, reusable settings, and editing history.

Processing MUST support the qualified headless CPU environment without requiring
a GPU or a desktop catalog.

Module discovery MUST distinguish target capability
from executable support in the selected environment.

JPEG and RAW inputs retain
separate support facts; a camera JPEG Preview is never a substitute for RAW input.

Capability discovery MUST distinguish readable, writable, and executable facts.

Discovery alone does not qualify every input or operation.

This capability MUST NOT introduce an embedded Agent, workflow graph editor,
automatic edit planning, or new Photo identities for alternate edits.

## Tool Invocation and Composition

An invocation captures a selected Processing Engine, an identified input file
revision, and that tool's saved parameters.

The normal initial input is the
workspace's independent snapshot of the Original.

A caller MAY explicitly select
a completed compatible workspace output as another tool's input, repeat a tool,
or stop after any result.

Tools exchange files rather than shared parameter trees.

No preview is required before Export.

A file appearing in the workspace does not
start another tool automatically.

[Photo Workspace](workspace/spec.md) owns
artifact identity, completion, file discovery, and eligibility for Continue
processing.

An ordinary file can be discovered without a tool callback or
manifest.

Slipstream MUST NOT invent successful Export provenance for it.

A shared extension does not establish compatible precision, color space, transfer
function, or geometry.

Each module MUST disclose admitted inputs, settings,
outputs, and refusal reasons under [Processing Modules](processing/design.md).

Unknown image facts require inspection or explicit refusal, not an implicit
conversion or substitution.

Preview files are useful for viewing and download;
they MUST NOT be offered as equivalent to full-resolution scene-linear inputs.

Admission MUST reuse protected managed input bytes.

An ordinary externally
writable file that passes compatibility checks MUST receive one protected input
snapshot before processing; later edits and renders reuse it rather than copy
it on every preview.

Verified processing provenance is not required to admit a
compatible ordinary image.

## Development Controls

[Discovering Editing Capabilities](darktable/editing.md#discovering-editing-capabilities)
owns the meanings of darktable's controls, input-dependent initialization, reset
targets, and interactions.

Controls MUST NOT be restricted to exposure and white
balance.

The exposure baseline and preservation of camera white-balance
coefficients follow [Development Color Pipeline](processing/color.md).

Input orientation MUST be respected without requiring a rotation edit.

Parameters belong to the selected tool's on-disk state.

Slipstream MAY index its
location and present a projection for the Web, but MUST NOT maintain a competing
parameter source of truth.

Inspecting another tool's result MUST NOT change the
saved state.

Optional Web controls MUST invoke the same tool against that same
state and preserve settings they cannot display.

Automatic adjustments are explicit actions.

The native tool computes and saves
concrete values under its mutation guard; later Preview and Export reuse those
values without independently rerunning the detector.

A failed action or stale
revision leaves the saved state unchanged.

## Editing Workspace

[Photo Workspace](workspace/spec.md) owns entry, file presentation, input selection,
tool launch, downloads, and Continue processing.

A module choice MUST explain
unavailable input support, engine dependencies, and execution capacity separately.

Editing gestures MUST NOT change Library selection or navigate Photos.

The
Original reference shows the camera-produced Preview, not a tool result.

Entering Edit MUST NOT require a Preview, reset saved state, or require a
file-manager sequence.

Browser controls MUST remain usable by keyboard and touch.

[Photo Workspace](workspace/spec.md#web-interaction) owns their presentation.

The workspace MUST distinguish save status from Preview status.

A previous
Preview MAY remain visible while rendering, but MUST be marked stale.

Current
status depends on the input and saved tool revision, not filename or arrival time.

When settings are saved while a Preview is rendering, the editor MUST request a
Preview of the newest confirmed revision after the older render settles.

An
older completion MUST NOT replace that current result.

A failed Preview MUST
remain an explicit failure with a retry action rather than trigger repeated
automatic execution.

After a render completes, a temporary workspace lease can delay file discovery.

The editor MUST keep checking the completed file until discovery can proceed,
without reporting an engine failure or submitting another render.

Library scanning, live Original availability, workspace input availability, and
tool availability are independent.

Once the input snapshot is complete and
unchanged, a missing or changed live Original MUST NOT disable its editing or
Export.

An unavailable workspace input MUST preserve state and earlier outputs
and explain the unavailable dependency.

Browsing thumbnails, Selection State,
Rating, Album membership, and browsing position remain independent of editing.

The Grid's saved-edit indicator reflects retained tool state, even if it equals
the baseline or cannot currently execute.

The workspace identifies the current Engine, input, and Controls in plain
language.

It distinguishes edit-state saving, saved, conflict, and failure
from Preview updating, current, stale, and failure.

Technical evidence such as revision tokens, request identities, and diagnostic processing reasons stays behind an optional details affordance.

Actionable status and refusals remain visible as specified by [Web Interaction](workspace/spec.md#web-interaction).

The primary flow uses editing state, editing TIFF,
and output language.

The current image and comparison controls MUST stay visible while the
Photographer operates or scrolls editing controls.

Wide layouts place image
and controls beside each other; narrow layouts retain an image region above
independently scrolling controls.

When selected and available, the Original reference MUST show the camera Preview inside this workspace at normal visibility, explicitly labelled as reference.

No saved Edit State is a start state, not an Edit Preview failure.

Controls and explanations MUST fit without horizontal scrolling or clipping.

Undo, Redo, save feedback, and conflict recovery belong beside the primary
controls rather than inside a technical disclosure.

Edit availability reports these independent facts:

- Whether the selected Processing Engine admits the workspace input.
- Whether the Library is scanning or recovering.
- Whether the Photo's live Original File currently reads.

A recovering Library or a busy Engine MUST NOT be presented as an unreadable Original.

The live Original fact does not disable editing or Export when the workspace input snapshot remains available.

When a service accepts a retained Processing Artifact, that Artifact is the explicit input identity for the selected tool's canonical Edit State.

The tool uses the Artifact's actual image contract and workspace retention state; it MUST NOT reopen or re-stage the upstream Original, mutate upstream state, or assume a latest result.

If the workspace input itself is missing or unreadable, the workspace preserves saved Edit State and prior Artifacts and reports the unavailable dependency.

## Autosave and Reversible Editing

One completed pointer drag, committed numeric entry, or settled keyboard
adjustment is one Web edit action.

The Web MUST save it through the tool without
a Save button or a write for every intermediate pointer position.

Saved means
the tool confirmed the corresponding atomic on-disk revision.

A late response
MUST NOT mark later settings saved.

A navigation MUST retain ownership of an
already submitted save without changing another Photo's controls.

Unconfirmed browser settings remain a draft.

Bounded browser recovery MAY retain
it, but MUST disclose when reload recovery is unavailable.

A draft MUST NOT
silently override the on-disk state.

Reopening restores the tool's confirmed
state, not a separately maintained Web recipe.

The Web MUST submit the revision it observed.

A stale write stops autosave for
that state and retains the draft.

The Photographer MAY use the newly saved state
or explicitly reapply the draft against its newly observed revision; this can
conflict again.

A busy state MUST report bounded waiting or refusal.

Separate
state directories MUST NOT conflict merely because they share a workspace.

Recovered settings remain local drafts until the selected tool confirms them.

Undo and redo in the browser submit new guarded updates for edit actions; the tool's retained history and restore semantics remain authoritative across sessions.

Primary exposure changes, individual resets, and compatibility edits share one chronological edit history per Photo through the selected tool's saved state.

An external revision invalidates history entries that can no longer be applied safely; it MUST NOT erase a local action merely because that action was confirmed by the tool.

Undo and redo apply completed actions through the same guarded state history.

[History and Recovery](darktable/editing.md#history-and-recovery) owns darktable's
retained revisions and restore.

An uncertain mutation MUST be reconciled with
its original request identity before dependent writes proceed; equality of
current values is not proof that the request completed.

Two reversible operations share the editing surface and MUST NOT be conflated:

- **Session Undo and Redo** are browser-local over the drafted Edit State and
  available without reopening the tool. Each submits a guarded update with a new
  request identity through the per-edit queue and appends to the chronological
  edit history.
- **Retained revision restore** selects an earlier confirmed tool-owned revision.
  It MAY select a revision the current draft never visited.

Both stay beside the primary controls so the Photographer does not leave the image to reverse a change.

A revision restore MUST NOT be labeled Undo or Redo. Session Undo and Redo
MUST NOT be labeled as history restore; their owners, scope, and outcomes differ.

## Preview Behavior

Controls MUST respond immediately; preview generation follows confirmed edits.

The previous complete preview stays visible while a replacement is pending or
fails.

Rendering failure MUST NOT discard saved state or a recoverable draft.

Late, cancelled, or obsolete results MUST NOT replace the current view.

An Edit Preview captures a saved tool revision, input revision, rendition
geometry, processing assets, and display conversion.

It is a real workspace file
with disclosed dimensions.

Rendering MUST NOT change the edit or create a
full-resolution handoff merely to provide a small preview.

A stale preview can
be inspected explicitly with its status and MUST NOT be called current.

Comparison MUST identify both operands and hold display conversion and geometry
constant when comparing to the disclosed processing baseline.

The camera
Preview remains a separately labeled reference.

Scene-referred previews follow
[Development Color Pipeline](processing/color.md); other outputs MUST
disclose their own display conversion.

Reduced-resolution inspection MUST NOT
claim full-resolution grain, halation, or spatial-effect equivalence.

A detail
request MUST execute a qualified region or refer to a matching full-resolution
output, rather than enlarge a small preview.

Effects MUST NOT be silently
disabled to make previews faster.

The initial standalone SpektraFilm warm inclusive-render 95th-percentile target
is at most 4.0 seconds for approximately 1 MP landscape and portrait renditions
(1225 × 816 and 816 × 1225), including its simulation wrapper after
initialization and all Film effects.

Queueing, admission, startup, and image
transfer are separate disclosed request-latency facts; cold startup and
full-resolution qualification remain separate.

## Export

An Export is an explicit tool execution from a captured confirmed state.

It
captures the input revision, complete settings, resources, engine bundle, and
output contract before execution.

Later edits MUST NOT retarget it.

Export MUST
work without a prior preview.

Unconfirmed settings, save failure, or conflict
MUST NOT silently produce an output from older settings.

Formats, precision, color, transfer, profile, geometry, and encoding belong to
the tool's output contract.

There is no universal product output.

The module
MUST verify the claimed result facts before reporting a successful Processing
Artifact.

Extension alone is insufficient.

The output MUST respect admitted
crop and rotation without an unrequested resize.

Downloaded images MUST include
correct color/orientation and basic capture metadata; GPS and private device
identifiers MUST be omitted.

External XMP history MUST NOT be copied as a claim
of supported editing interchange.

Tools write complete files into the workspace and return their actual paths.

Default generated output names MUST be unique; existing destinations MUST NOT
be overwritten.

The workspace can discover files without the tool's response.

Concurrent completions MUST retain both outputs and MUST NOT attribute one run's
file to another run.

Atomic completion and external file-change handling follow
[Photo Workspace](workspace/spec.md).

Export states are queued, exporting, ready to download, failed, or cancelled.

Accepted work follows the durable settlement rules.

A confirmed later edit marks an earlier Artifact as based on older settings but does not remove or retarget it.

A failed or cancelled Export does not publish partial output.

Workspace retention is governed by [Photo Workspace](workspace/spec.md), explicit removal, and reviewed whole-workspace deletion.

A run receipt or cache lifetime MUST NOT expire or delete a confirmed output, saved state, or required input.

If a retained file is removed, the primary Download location MUST identify that it is unavailable and MUST NOT offer an enabled Download action.

The primary Download location shows whether an output uses earlier settings.

Provenance identifiers and image-contract details remain available through an optional disclosure.

The Web uses plain status such as Exporting and Ready to download.

Output cards
identify available filename, size, time, tool, dimensions, and verified image
facts; missing provenance stays unknown.

An earlier-state output remains
available with that fact shown.

New failures MUST NOT hide previous files.

Download and Continue processing are actions on a completed identified file;
Continue processing checks the chosen next tool's compatibility and does not
implicitly rerun an upstream edit.

Outputs MUST NOT create Photos or Albums.

A standalone command owns its synchronous process lifetime.

Returning or losing
its output does not imply a service-owned background task.

Slipstream MAY
supervise that same tool for Web execution; its accepted-work tracking MUST
truthfully distinguish running, interrupted, completed, and uncertain outcomes.

A browser departure need not stop supervised work.

Neither mode MAY claim that
an interrupted process survives a machine restart.

### Edit State File

The authoritative editing state is already saved in the tool's private state
directory.

Editing MUST NOT require exporting that state, uploading it, or
committing a submitted copy to Slipstream.

Tool-state files are not image outputs
and remain separate from the workspace's normal result list.

A tool MAY offer a supported state export for backup or interchange.

Such an
export MUST preserve complete instances, masks, output settings, input identity,
resource versions, and schema/engine identity, and disclose dependencies it does
not package.

JSON state is not automatically a portable bundle or a desktop XMP
history.

A native interchange format MAY be advertised only for its validated
supported semantics.

No Lightroom compatibility or bidirectional XMP
synchronization follows from exporting a file.

Reading or copying confirmed state requires neither the live Original nor an
available rendering engine.

A state export MUST NOT write beside the Original or
become a second authoritative state store.

Before claiming an editing image's
compatibility with another editor, qualification MUST open it in a named target
and check color, dimensions, orientation, and sample format.

## Shared Human and Programmatic Use

The independent `darktable-edit` tool reads local inputs and saves local state
without requiring Slipstream.

The optional `slipstream darktable` integration
resolves Photos, workspace files, and state directories and invokes that same
tool.

[darktable CLI Reference](darktable/cli.md) owns the command grammar.

The
integration MUST NOT introduce a download/edit/upload bridge or mirror of tool
parameters.

Compatible generic processing interfaces MAY project the canonical
file state; historical user state and outputs MUST remain retained.

A returned workspace path belongs to a stated execution host and mode.

A remote
caller MUST run in an authorized environment that can access that directory;
a server pathname MUST NOT be presented as a file on the caller's machine.

Downloads remain useful for delivery, not mandatory per-edit synchronization.

The workspace MUST NOT provide an arbitrary remote-shell endpoint.

## Original Availability and Recovery

The workspace begins with a guarded, independent read-only snapshot.

It MUST NOT
use a writable hardlink or a symlink to an Original.

Tools MUST NOT modify the
Original or its associated external XMP Sidecar.

Workspace files remain outside
the Library Folder.

Exact-content Location Recovery preserves the Photo's workspace association.

A missing or changed live Original does not invalidate unchanged snapshot bytes,
saved edits, or completed outputs.

No Development Proxy is needed to edit that
snapshot offline.

Conversely, a modified or missing workspace input invalidates
its old revision for future rendering; it MUST NOT silently resolve to the live
Original.

An explicit source refresh creates another input version and does not
retarget existing states or prior results.

Reusing settings with another input
requires the tool's explicit binding and compatibility rules.

A Photo with retained workspace state or referenced outputs remains referenced
for Retire and Bind eligibility.

Recovery MUST NOT discard edits as unused scan
records.

Library deletion and workspace deletion are separate reviewed actions
under [Photo Workspace](workspace/spec.md).

## Processing Capacity

Each execution environment has finite memory and computing capacity.

Supervised
Web work MUST obey configured admission and report waiting separately from
running; standalone execution MUST disclose its own execution limits.

A lock on
one edit state MUST NOT serialize all independent workspace edits or unrelated
file outputs.

Resource scheduling and state-write guards are different concerns.

Capacity is separate from input support and engine availability.

A preview can
fit while a full Export cannot.

A refusal MUST identify the affected operation
and allowance, preserve saved state and previous outputs, and leave browsing
usable.

Processing MUST NOT silently shrink dimensions, disable Film effects,
change numerical quality, or repeatedly restart a failed attempt.

Explicit retry
MUST identify its intended captured revision and current dependencies.

Increasing
capacity MUST NOT change a successfully rendered look.

## Failure and Retention

Workspace retention, cleanup, download integrity, and explicit deletion follow
[Photo Workspace](workspace/spec.md).

Needed inputs, saved state, referenced
resources, and kept outputs have no fixed seven-day expiry.

A short-lived run
receipt or cache MUST NOT determine a file's lifetime or delete confirmed state.

State and retained files require backup; a tool's state is only reproducible
while its declared input, resources, and engine dependencies remain available.

Invalid parameters, missing assets, unsupported inputs, cancellation, engine
failure, and insufficient disk space MUST leave earlier confirmed state and
completed results intact.

A failed atomic state save MUST retain the prior
revision.

Incomplete outputs MUST remain private and MUST NOT be offered for
download or later processing.

Disk-full errors MUST explain how to free space
without silently deleting needed files or reducing output quality.

A lost response MUST NOT imply failure.

The original tool request MAY be
reconciled under its exact-request rules; independently discovered files prove
file availability, not that an uncertain mutation or Export request succeeded.

Restart reconciliation MUST discover complete ordinary outputs even when a
watcher missed their creation.

It MUST preserve old complete files, classify
interrupted work honestly, and not rerun an uncertain action automatically.

