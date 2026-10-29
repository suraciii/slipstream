# Photo Workspace

A Photographer needs one place to take a Photo through independent tools, inspect
previews, download completed files, and continue processing.

Moving each result
out of Slipstream and submitting it back after every edit interrupts that work.

A Photo Workspace MUST therefore be a real persistent directory.

Tools MUST read
its inputs and write their results directly into it.

The Web Edit view and file
commands MUST present those same files.

[Photo Development](../spec.md) owns processing behavior.

[Headless darktable Editing](../darktable/editing.md) owns darktable capabilities.

[CLI Reference](../../command-line/reference.md) owns workspace commands, and
[darktable CLI Reference](../darktable/cli.md) owns independent editing and the
optional `slipstream darktable` integration.

## Open and Resume

The Photo View MUST offer Edit to open or reopen the Photo's workspace.

Opening
MUST create at most one default workspace for that Photo, including concurrent
requests.

The caller MUST NOT choose a storage directory, construct an artifact
record, or initialize a processing recipe before useful work.

First preparation MUST place a verified, protected snapshot of the selected
Original in the workspace.

It MUST show preparation progress and a specific
failure if the Original is unavailable, changed during copying, or cannot fit.

No incomplete input MAY appear ready.

Preparation MUST NOT run darktable or
change the Original, its XMP Sidecar, or Library decisions.

Initially, the only workspace image file MUST be the protected input.

The separate Camera reference is not another workspace file or a render of the RAW snapshot.

Directories for previews, outputs, and tool state MUST appear only when needed.

The input's Original filename MUST remain recognizable.

A subsequent open MUST reuse existing files and tool state without recopying the Original or resetting edits.

After explicit removal of the default input, open MUST disclose that no default is selected.

It MUST keep remaining files and resumable edits available for explicit selection
without reading the Original, recreating removed files, or retargeting an edit.

Prepare input again explicitly refreshes from an available verified Original;
it reuses an intact matching snapshot or creates a new identity, never resurrects
an explicitly removed identity, and leaves removal receipts replayable.

If a workspace input is still intact, work MUST continue when the Original is
later missing or changed.

The workspace MUST disclose that the snapshot differs
from the current Library source when known.

Refreshing the source MUST be an
explicit action that adds a new input version.

It MUST NOT replace bytes under
an existing edit or silently rebind any processing state.

## Directory and Execution Location

Workspace open MUST return its identity, Photo identity, absolute filesystem
path, execution location, prepared input path and checked identity, readiness,
existing tool states, and Web URL.

Paths MUST be directly usable by a process in
the disclosed execution environment.

Container and host paths MUST NOT be
presented as interchangeable.

The independent tool MUST run where it can access that directory.

A remote
Slipstream client MUST NOT be told that a server path exists on its own machine.

The integration MUST either offer an available supervised tool on the workspace
host or report the required execution environment.

Authorized shared filesystem
access MAY also supply that directory.

The product MUST NOT require a new remote
shell service or transfer every preview through the caller's machine.

The following layout illustrates the managed directories; concrete generated
filenames come from command results rather than caller guesses:

```text diagram
photo-workspace/
  input/
    DSC01234.ARW
  previews/
    DSC01234-darktable-r3-preview.png
  outputs/
    DSC01234-darktable-r3.tif
    DSC01234-film-r1.jpg
  .edits/
    darktable/
      main/
  .tmp/
```

`input/` MUST be protected from tool writes. `previews/` and `outputs/` hold
completed files. `.edits/` holds resumable tool state and required resources;
`.tmp/` holds incomplete work.

The latter two MUST stay out of the normal image
list.

These roles MUST NOT require an independent tool to understand Slipstream
identity or implement a Slipstream registration protocol.

A tool MAY receive
explicit input, state, and output paths.

## Adjust and Preview

With one input and no saved state, selecting an available tool MUST use that
input.

With one matching saved state, reopening the tool MUST resume it.

Several
inputs or saved alternatives MUST require a clear choice by recognizable name
and thumbnail; the product MUST NOT guess from the newest file or lowest ID.

The user MAY explicitly start a separate edit without disturbing existing state.

An independent tool MUST save its editing state in the directory supplied for
that edit.

The tool MUST own initialization, operation semantics, validation,
and persistence.

Browser controls and `slipstream darktable` MUST use the same
tool state as independent calls.

A user MUST NOT import, synchronize, submit, or
register a local change for it to appear in the workspace.

A completed adjustment MUST save before it is reported as saved.

It MUST NOT
require a full render.

Preview MUST render a captured saved version and write a
completed image into the preview directory.

The workspace MUST show that version
and whether it still matches the selected edit.

During an update, the last
completed preview remains visible with an Updating status.

A late result for an
older version MUST NOT replace the preview of a newer version.

Changing settings and previewing MUST remain repeatable without regenerating a
full-resolution TIFF, restaging the RAW, or asking the caller to reconstruct all
parameters.

Native tool capabilities and unsupported operations MUST remain
explicit.

An unavailable tool MUST NOT prevent viewing and downloading existing
files.

## Export, Download, and Continue

Export MUST generate a file from the selected input and captured saved edit
version.

It MUST NOT require a prior Preview or upscale a preview file.

The tool
MUST provide suitable discoverable output choices, including actual format,
precision, color space, transfer function, and dimensions.

Repeated exports MUST
use distinct generated names and preserve previous completed outputs.

A tool MUST finish the file before publishing its final path.

Slipstream MUST
automatically discover it and show it after file validation.

The user MUST NOT
upload it again or call a separate artifact registration or commit operation.

Successful tool processing and workspace file visibility are separate facts: if
indexing is still underway, the command MUST return the existing final path and
show Checking file rather than claim the render failed or rerun it.

Each completed output MUST offer Download and Continue processing.

Download
MUST deliver the selected file's checked bytes with its recognizable filename.

It MUST NOT render again or return whatever output is now newest.

Continue
processing MUST carry the exact selected file into the next tool without asking
the user to browse for it, copy it, or re-enter an artifact ID.

With several
compatible tools, the user chooses one.

With none, the file remains downloadable
and the product MUST explain the missing compatible capability.

The next tool MUST check the actual input contract.

A TIFF filename alone does
not establish linear color, floating-point precision, or compatible geometry.

No conversion or additional processing tool MAY run merely because a file
appeared.

Changing an earlier edit MUST NOT rerun downstream work or replace its
input.

Download and Continue processing remain independent choices.

Preview files MUST be viewable and downloadable as previews.

They MUST NOT be
offered as full-resolution or scene-linear handoff inputs.

Tool state is not an
image result.

A file written by an unrelated tool MAY be shown and downloaded,
but Slipstream MUST NOT invent the producing parameters, editing version, or
successful Export attribution.

Such a file becomes an admitted next input only
after its actual contract passes the selected tool's checks.

## Web Interaction

The main Edit view MUST show the selected image, relevant tool controls, the input name, and concise save and processing status.

A Files disclosure MUST group inputs, previews, and outputs without requiring a separate workflow screen.

The initial view MUST NOT be a general file manager or a blank stage editor.

With no saved edit, the prepared input and tool choice MUST be visible together with an explicitly labeled **Camera reference** when that Photo's Preview is available.

The Camera reference is Library display evidence, not an Edit Preview or proof of workspace input readiness.

When it is unavailable, the view MUST show **Camera reference unavailable** without substituting a sibling image or disabling an otherwise intact input.

Once an applicable Edit Preview is selected, the Camera reference requires explicit selection and MUST NOT replace its pending, stale, or failed result automatically.

The Camera reference MUST NOT be substituted as a comparison operand.

No saved edit is a normal start state, not a Preview failure.

Opening or reopening MUST NOT implicitly initialize a tool or start rendering.

Tool choices MUST show known availability and refusal reasons beside the input before creation is offered.

Unanswered discovery MUST remain loading or retryable, not supported or unsupported by assumption.

Engine availability, selected-input compatibility, and execution capacity remain independent; an available engine MAY still refuse authoritative admission.

A missing tool MUST NOT disable other tools, Files, or existing downloads.

At default text size, the initial image or reference and the next available editing action or actionable blocker MUST share the first view at 375 by 667 and 390 by 844.

Routine technical status MUST NOT displace them.

Short layouts MUST retain an image or reference region and primary actions while controls scroll independently, yielding supporting details before clipping controls.

At 200% text enlargement, controls and text MUST remain usable without horizontal page overflow; vertical reflow MAY be necessary.

### Primary action strip

One primary action strip carries the actions that belong to every editing task:

- save state, with its current saving, saved, conflict, failure, or unknown
  status;
- session Undo;
- session Redo;
- Preview; and
- Export.

These five are the strip's peers.

The strip stays with the image rather than
inside a tool panel, so the Photographer never scrolls editing controls or opens
a technical disclosure to save, reverse an edit, render a Preview, or start an
Export.

A narrow layout MAY compress or regroup the strip, but it MUST NOT
remove an action from the primary level.

Files, jobs, catalogs, resources, and raw technical details are supporting
disclosures.

They have a clear name and invoker, but they are not peers of the
strip: moving between them MUST NOT replace the strip, and the strip MUST NOT be
represented as one more disclosure beside them.

A revision restore from the
tool's retained history remains a separate operation with its own label, as
[Autosave and Reversible Editing](../spec.md#autosave-and-reversible-editing)
requires.

A file card MUST show a thumbnail when displayable, filename, file role, size,
and readiness.

Dimensions and relevant output characteristics MUST be available
without exposing internal IDs.

Provenance, full execution paths, revisions, and
diagnostic details belong in an optional details view.

An output that cannot be
displayed directly MUST still offer its valid Download and compatible Continue
processing actions; any display conversion MUST be labeled as such.

Selecting a file for inspection MUST NOT change the active edit or bind a tool
to a different input.

Continue processing is the explicit transition.

Download
MUST NOT remove the file.

Leaving Edit or returning from the Library MUST
preserve selected state and completed files.

Keyboard actions and narrow-screen
presentation MUST retain the same actions and status meanings.

Files MUST offer Remove for an explicitly selected protected input as well as
other eligible files.

Protection means read-only to tools, not forever retained.

The reviewed file revision MUST be checked at removal.

A refusal MUST name the
dependent saved edits, jobs, outputs, resources, or active tools and offer the
relevant explicit rebind/remove/settle action; unknown external use MUST be
resolved before removal.

No force action MAY break those dependencies.

Removing
an unused older RAW MUST preserve unrelated edits/results and the Library
Original.

Removing the default/last input MUST disclose that no default is
selected while preserving access to remaining files and edits.

Offer explicit
selection of a compatible retained file, Prepare input again when the Original
is available, or source recovery otherwise.

The primary statuses MUST distinguish Preparing input, Saving settings,
Rendering preview, Exporting, Checking file, and Ready.

Failures MUST name the failed operation and a concrete recovery action when known.

A successful settings save MUST
remain saved if the following render fails.

Files MUST remain visible while an
unrelated operation is pending or fails.

Routine diagnostics, revision identifiers, and execution paths MAY move into Details.

Saving, conflict, failed or unknown writes, Preview staleness, and actionable refusal MUST remain visible beside the affected primary action.

Primary messages MUST name the condition, its effect, and a valid next action when known.

Raw error codes and operating-system messages are diagnostic detail.

The interface MUST NOT invent a retry or configuration fix for an unclassified failure.

## Lifetime and Recovery

The workspace is durable working storage.

Inputs, saved edit state, required
resources, and completed outputs MUST remain until explicitly removed.

They
MUST NOT inherit a temporary download or task receipt expiry.

Disposable
previews and abandoned temporary files MAY be offered for cleanup with an
estimated size; cleanup MUST preserve the currently displayed preview and any
file required by a retained edit or active operation.

An individually selected, unreferenced preview MAY be explicitly removed; if it
was displayed, the view MUST clear it and disclose that removal.

This explicit
action is distinct from bulk cleanup, which preserves the current preview.

Removal MUST check the selected revision, dependencies, and active use under
the same admission protection as binding a new edit or job.

A new binding either
pins the input first and blocks removal, or observes that it has been removed
and refuses; it cannot retain a dangling dependency.

Current or retained state,
captured jobs, kept outputs, declared dependencies, active independent tool use,
or unverified use MUST refuse removal with named blockers.

An unused protected
snapshot MAY be explicitly removed without discarding other work.

Deleting an entire workspace MUST require a separate
confirmation of its complete current loss inventory.

A new file, changed state,
or changed dependency invalidates that confirmation.

Active operations and
references from outside the workspace MUST refuse deletion.

Accepted deletion
removes the workspace from use before reclaiming its files and reports any
unfinished reclamation honestly.

It MUST NOT delete the Library Original.

Removing a Photo from the Library MUST NOT implicitly erase this work.

[Trash Record Cleanup](../../photo-library/trash/spec.md) MUST refuse a Photo
with retained workspace, edit, or artifact references, or active or unresolved
work. The separate workspace removal flow remains responsible for that work.

Cancellation, a crashed tool, or disk exhaustion MUST preserve previous complete
files and saved settings.

Partial files MUST NOT appear ready or downloadable.

Closing a supervised client MUST leave service-owned accepted work queryable;
closing an independent process has only the standalone tool's documented
lifetime.

A later workspace read MUST discover completed files even if the
service missed a filesystem notification or restarted during processing.

Concurrent writers MUST NOT overwrite a completed output.

Conflicting edits to
one tool state MUST return Busy or an edit-version conflict, retaining the
caller's intended change.

The product MUST NOT silently reload a newer revision
and overwrite it.

Separate output files MUST NOT conflict merely because they
share the workspace.

External replacement or in-place modification of a file
MUST invalidate its old content revision; old downloads and downstream bindings
MUST NOT silently continue against new bytes.

