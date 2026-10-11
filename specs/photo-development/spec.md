# Photo Development

A Photographer needs a place to edit a photograph, inspect the result, and keep
files for download or later processing. A [Photo Workspace](workspace/spec.md)
is a real directory containing an independent input snapshot, saved tool state,
previews, and outputs. A tool reads and writes files there directly. The
Photographer must not download, upload, or submit an editing file on every
iteration.

## Scope

Processing Engines are peer tools. darktable provides general image editing and RAW
development; standalone SpektraFilm provides negative, print, and scan simulation.
The SpektraFilm integration inside darktable is not the standalone module.
Slipstream must not impose an order, automatically chain modules, or insert a
conversion. The initial standalone Film Recipe uses Kodak Portra 400 and Kodak
Portra Endura; it does not restrict darktable's editing surface.

[Headless darktable Editing](darktable/editing.md) owns operation instances,
structured controls, masks, resources, reusable settings, and editing history.
Processing must support the qualified headless CPU environment without requiring
a GPU or a desktop catalog. Module discovery must distinguish target capability
from executable support in the selected environment. JPEG and RAW inputs retain
separate support facts; a camera JPEG Preview is never a substitute for RAW input.

This capability must not introduce an embedded Agent, workflow graph editor,
automatic edit planning, or new Photo identities for alternate edits.

## Tool Invocation and Composition

An invocation captures a selected Processing Engine, an identified input file
revision, and that tool's saved parameters. The normal initial input is the
workspace's independent snapshot of the Original. A caller may explicitly select
a completed compatible workspace output as another tool's input, repeat a tool,
or stop after any result. Tools exchange files rather than shared parameter trees.

```text diagram
Photo -> workspace input snapshot
          -> selected tool -> preview files and output files
                               -> explicit compatible input to another tool
```

No preview is required before Export. A file appearing in the workspace does not
start another tool automatically. [Photo Workspace](workspace/spec.md) owns
artifact identity, completion, file discovery, and eligibility for Continue
processing. An ordinary file can be discovered without a tool callback or
manifest. Slipstream must not invent successful Export provenance for it.

A shared extension does not establish compatible precision, color space, transfer
function, or geometry. Each module must disclose admitted inputs, settings,
outputs, and refusal reasons under [Processing Modules](processing/design.md).
Unknown image facts require inspection or explicit refusal, not an implicit
conversion or substitution. Preview files are useful for viewing and download;
they must not be offered as equivalent to full-resolution scene-linear inputs.

Admission must reuse protected managed input bytes. An ordinary externally
writable file that passes compatibility checks must receive one protected input
snapshot before processing; later edits and renders reuse it rather than copy
it on every preview. Verified processing provenance is not required to admit a
compatible ordinary image.

## Development Controls

[Discovering Editing Capabilities](darktable/editing.md#discovering-editing-capabilities)
owns the meanings of darktable's controls, input-dependent initialization, reset
targets, and interactions. Controls must not be restricted to exposure and white
balance. The exposure baseline and preservation of camera white-balance
coefficients follow [Development Color Pipeline](processing/color.md).
Input orientation must be respected without requiring a rotation edit.

Parameters belong to the selected tool's on-disk state. Slipstream may index its
location and present a projection for the Web, but must not maintain a competing
parameter source of truth. Inspecting another tool's result must not change the
saved state. Optional Web controls must invoke the same tool against that same
state and preserve settings they cannot display.

Automatic adjustments are explicit actions. The native tool computes and saves
concrete values under its mutation guard; later Preview and Export reuse those
values without independently rerunning the detector. A failed action or stale
revision leaves the saved state unchanged.

## Editing Workspace

[Photo Workspace](workspace/spec.md) owns the exact entry flow, file presentation,
input selection, tool launch, downloads, and Continue processing. Photo View's
Edit action creates or reopens the Photo's persistent directory. Initially its
only visible image is the input snapshot. The Photographer chooses an available
tool, sees its latest relevant preview, and can inspect its saved outputs in
Files. Technical paths and revision evidence belong in optional details.

A module choice must explain unavailable input support, engine dependencies, or
execution capacity separately. Entering Edit must not reset saved state, require
a preview to exist, or require a file-manager sequence. The Original reference
shows the independent camera-produced Preview and must not masquerade as the
tool's result. Browser controls, when offered, sit beside the image and remain
usable by keyboard and touch; narrow screens must not require horizontal page
scrolling. Editing gestures must not change Library selection or navigate Photos.

Saving and rendering are separate facts. The workspace must distinguish saving,
saved, conflict, and failure from preview updating, current, stale, and failure.
It may retain the previous preview while rendering, but must identify it as out
of date. A result's current status depends on its input and saved tool revision,
not its filename, arrival time, or being the most recent image found.

Library scanning, live Original availability, workspace input availability, and
tool availability are independent. Once the input snapshot is complete and
unchanged, a missing or changed live Original must not disable its editing or
Export. An unavailable workspace input must preserve state and earlier outputs
and explain the unavailable dependency. Browsing thumbnails, Selection State,
Rating, Album membership, and browsing position remain independent of editing.
The Grid's saved-edit indicator reflects retained tool state, even if it equals
the baseline or cannot currently execute.

## Autosave and Reversible Editing

One completed pointer drag, committed numeric entry, or settled keyboard
adjustment is one Web edit action. The Web must save it through the tool without
a Save button or a write for every intermediate pointer position. Saved means
the tool confirmed the corresponding atomic on-disk revision. A late response
must not mark later settings saved. A navigation must retain ownership of an
already submitted save without changing another Photo's controls.

Unconfirmed browser settings remain a draft. Bounded browser recovery may retain
it, but must disclose when reload recovery is unavailable. A draft must not
silently override the on-disk state. Reopening restores the tool's confirmed
state, not a separately maintained Web recipe.

The Web must submit the revision it observed. A stale write stops autosave for
that state and retains the draft. The Photographer may use the newly saved state
or explicitly reapply the draft against its newly observed revision; this can
conflict again. A busy state must report bounded waiting or refusal. Separate
state directories must not conflict merely because they share a workspace.

Undo and redo apply completed actions through the same guarded state history.
[History and Recovery](darktable/editing.md#history-and-recovery) owns darktable's
retained revisions and restore. An uncertain mutation must be reconciled with
its original request identity before dependent writes proceed; equality of
current values is not proof that the request completed.

## Preview Behavior

Controls must respond immediately; preview generation follows confirmed edits.
The previous complete preview stays visible while a replacement is pending or
fails. Rendering failure must not discard saved state or a recoverable draft.
Late, cancelled, or obsolete results must not replace the current view.

An Edit Preview captures a saved tool revision, input revision, rendition
geometry, processing assets, and display conversion. It is a real workspace file
with disclosed dimensions. Rendering must not change the edit or create a
full-resolution handoff merely to provide a small preview. A stale preview can
be inspected explicitly with its status and must not be called current.

Comparison must identify both operands and hold display conversion and geometry
constant when comparing to the disclosed processing baseline. The camera
Preview remains a separately labeled reference. Scene-referred previews follow
[Development Color Pipeline](processing/color.md); other outputs must
disclose their own display conversion. Reduced-resolution inspection must not
claim full-resolution grain, halation, or spatial-effect equivalence. A detail
request must execute a qualified region or refer to a matching full-resolution
output, rather than enlarge a small preview. Effects must not be silently
disabled to make previews faster.

The initial standalone SpektraFilm warm inclusive-render 95th-percentile target
is at most 4.0 seconds for approximately 1 MP landscape and portrait renditions
(1225 × 816 and 816 × 1225), including its simulation wrapper after
initialization and all Film effects. Queueing, admission, startup, and image
transfer are separate disclosed request-latency facts; cold startup and
full-resolution qualification remain separate.

## Export

An Export is an explicit tool execution from a captured confirmed state. It
captures the input revision, complete settings, resources, engine bundle, and
output contract before execution. Later edits must not retarget it. Export must
work without a prior preview. Unconfirmed settings, save failure, or conflict
must not silently produce an output from older settings.

Formats, precision, color, transfer, profile, geometry, and encoding belong to
the tool's output contract. There is no universal product output. The module
must verify the claimed result facts before reporting a successful Processing
Artifact. Extension alone is insufficient. The output must respect admitted
crop and rotation without an unrequested resize. Downloaded images must include
correct color/orientation and basic capture metadata; GPS and private device
identifiers must be omitted. External XMP history must not be copied as a claim
of supported editing interchange.

Tools write complete files into the workspace and return their actual paths.
Default generated output names must be unique; existing destinations must not
be overwritten. The workspace can discover files without the tool's response.
Concurrent completions must retain both outputs and must not attribute one run's
file to another run. Atomic completion and external file-change handling follow
[Photo Workspace](workspace/spec.md).

The Web uses plain status such as Exporting and Ready to download. Output cards
identify available filename, size, time, tool, dimensions, and verified image
facts; missing provenance stays unknown. An earlier-state output remains
available with that fact shown. New failures must not hide previous files.
Download and Continue processing are actions on a completed identified file;
Continue processing checks the chosen next tool's compatibility and does not
implicitly rerun an upstream edit. Outputs must not create Photos or Albums.

A standalone command owns its synchronous process lifetime. Returning or losing
its output does not imply a service-owned background task. Slipstream may
supervise that same tool for Web execution; its accepted-work tracking must
truthfully distinguish running, interrupted, completed, and uncertain outcomes.
A browser departure need not stop supervised work. Neither mode may claim that
an interrupted process survives a machine restart.

### Edit State File

The authoritative editing state is already saved in the tool's private state
directory. Editing must not require exporting that state, uploading it, or
committing a submitted copy to Slipstream. Tool-state files are not image outputs
and remain separate from the workspace's normal result list.

A tool may offer a supported state export for backup or interchange. Such an
export must preserve complete instances, masks, output settings, input identity,
resource versions, and schema/engine identity, and disclose dependencies it does
not package. JSON state is not automatically a portable bundle or a desktop XMP
history. A native interchange format may be advertised only for its validated
supported semantics. No Lightroom compatibility or bidirectional XMP
synchronization follows from exporting a file.

Reading or copying confirmed state requires neither the live Original nor an
available rendering engine. A state export must not write beside the Original or
become a second authoritative state store. Before claiming an editing image's
compatibility with another editor, qualification must open it in a named target
and check color, dimensions, orientation, and sample format.

## Shared Human and Programmatic Use

The independent `darktable-edit` tool reads local inputs and saves local state
without requiring Slipstream. The optional `slipstream darktable` integration
resolves Photos, workspace files, and state directories and invokes that same
tool. [darktable CLI Reference](darktable/cli.md) owns the command grammar. The
integration must not introduce a download/edit/upload bridge or mirror of tool
parameters. Compatible generic processing interfaces may project the canonical
file state; historical user state and outputs must remain retained.

A returned workspace path belongs to a stated execution host and mode. A remote
caller must run in an authorized environment that can access that directory;
a server pathname must not be presented as a file on the caller's machine.
Downloads remain useful for delivery, not mandatory per-edit synchronization.
The workspace must not provide an arbitrary remote-shell endpoint.

## Original Availability and Recovery

The workspace begins with a guarded, independent read-only snapshot. It must not
use a writable hardlink or a symlink to an Original. Tools must never modify the
Original or its associated external XMP Sidecar. Workspace files remain outside
the Library Folder.

Exact-content Location Recovery preserves the Photo's workspace association.
A missing or changed live Original does not invalidate unchanged snapshot bytes,
saved edits, or completed outputs. No Development Proxy is needed to edit that
snapshot offline. Conversely, a modified or missing workspace input invalidates
its old revision for future rendering; it must not silently resolve to the live
Original. An explicit source refresh creates another input version and does not
retarget existing states or prior results. Reusing settings with another input
requires the tool's explicit binding and compatibility rules.

A Photo with retained workspace state or referenced outputs remains referenced
for Retire and Bind eligibility. Recovery must not discard edits as unused scan
records. Library deletion and workspace deletion are separate reviewed actions
under [Photo Workspace](workspace/spec.md).

## Processing Capacity

Each execution environment has finite memory and computing capacity. Supervised
Web work must obey configured admission and report waiting separately from
running; standalone execution must disclose its own execution limits. A lock on
one edit state must not serialize all independent workspace edits or unrelated
file outputs. Resource scheduling and state-write guards are different concerns.

Capacity is separate from input support and engine availability. A preview may
fit while a full Export cannot. A refusal must identify the affected operation
and allowance, preserve saved state and previous outputs, and leave browsing
usable. Processing must not silently shrink dimensions, disable Film effects,
change numerical quality, or repeatedly restart a failed attempt. Explicit retry
must identify its intended captured revision and current dependencies. Increasing
capacity must not change a successfully rendered look.

## Failure and Retention

Workspace retention, cleanup, download integrity, and explicit deletion follow
[Photo Workspace](workspace/spec.md). Needed inputs, saved state, referenced
resources, and kept outputs have no fixed seven-day expiry. A short-lived run
receipt or cache must not determine a file's lifetime or delete confirmed state.
State and retained files require backup; a tool's state is only reproducible
while its declared input, resources, and engine dependencies remain available.

Invalid parameters, missing assets, unsupported inputs, cancellation, engine
failure, and insufficient disk space must leave earlier confirmed state and
completed results intact. A failed atomic state save must retain the prior
revision. Incomplete outputs must remain private and must not be offered for
download or later processing. Disk-full errors must explain how to free space
without silently deleting needed files or reducing output quality.

A lost response must not imply failure. The original tool request may be
reconciled under its exact-request rules; independently discovered files prove
file availability, not that an uncertain mutation or Export request succeeded.
Restart reconciliation must discover complete ordinary outputs even when a
watcher missed their creation. It must preserve old complete files, classify
interrupted work honestly, and not rerun an uncertain action automatically.

## Examples

- Edit opens the workspace's RAW snapshot. darktable writes a preview and TIFF
  there directly; Files shows both without upload or a publication callback.
- The Photographer downloads the TIFF or chooses a compatible next tool. The
  next tool reads that exact file and writes another output into the workspace.
- The live Original disappears after snapshot creation. Editing and Export
  continue from the unchanged snapshot; Library availability still reports the
  missing Original independently.
- A CLI adds a curve and mask. A Web exposure control edits the same saved state
  and preserves those objects. A stale Web save retains its draft and conflicts.
- Export captures +0.5 EV, then the Photographer saves +1 EV. The completed output
  stays bound to +0.5 EV and the old preview remains marked stale until replaced.
- Two tools finish concurrently with unique output names. Both complete files
  remain visible after restart, even if neither tool returned to Slipstream.
- Disk fills during a later render or state save. Earlier state and downloadable
  outputs remain intact; a partial result is never presented as complete.

[Photo Development Architecture](design.md) owns execution
contracts. [Processing Modules](processing/design.md) owns module
interfaces, and [Development Color Pipeline](processing/color.md) owns
color qualification. [Photo Previews](../photo-library/previews/spec.md) continues to own browsing's
camera Preview.
