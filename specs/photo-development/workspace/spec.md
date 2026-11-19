# Photo Workspace

A Photographer needs one place to take a Photo through independent tools, inspect
previews, download completed files, and continue processing.

Moving each result
out of Slipstream and submitting it back after every edit interrupts that work.

A Photo Workspace MUST therefore be a real persistent directory.

Tools MUST read
its inputs and write their results directly into it.

The Web Workspace view and file
commands MUST present those same files.

[Photo Development](../spec.md) owns processing behavior.

[Headless darktable Editing](../darktable/editing.md) owns darktable capabilities.

[CLI Reference](../../command-line/reference.md) owns workspace commands, and
[darktable CLI Reference](../darktable/cli.md) owns independent editing and the
optional `slipstream darktable` integration.

## Open and Resume

The Photo View MUST offer Workspace to open or reopen the Photo's workspace.

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

A subsequent open MUST reuse existing files and tool state without recopying the
Original or resetting tool state.

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

`slipstream darktable` and other tool callers MUST use the same
tool state as independent calls.

A user MUST NOT import, synchronize, submit, or
register a local change for it to appear in the workspace.

A completed adjustment MUST save before it is reported as saved.

It MUST NOT
require a full render.

Preview MUST render a captured saved version and write a
completed image into the preview directory.

The preview result MUST identify that version and whether it still matches the selected edit.

A replacement MUST NOT overwrite the last completed preview until it completes.

A late result for an older version MUST NOT replace the preview of a newer version.

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

The caller MUST NOT upload it again or call a separate artifact registration or commit operation.

Successful tool processing and workspace file visibility are separate facts: if
indexing is still underway, the command MUST return the existing final path and
show Checking file rather than claim the render failed or rerun it.

Each completed output MUST offer Download. Continue processing is a CLI or Agent
action.

Download MUST deliver the selected file's checked bytes with its recognizable
filename.

It MUST NOT render again or return whatever output is now newest.

Continue
processing MUST carry the exact selected file into the next tool without asking
the user to browse for it, copy it, or re-enter an artifact ID.

With several compatible tools, a CLI or Agent caller chooses one.

With none, the file remains downloadable
and the product MUST explain the missing compatible capability.

The next tool MUST check the actual input contract.

A TIFF filename alone does
not establish linear color, floating-point precision, or compatible geometry.

No conversion or additional processing tool MAY run merely because a file
appeared.

Changing an earlier edit MUST NOT rerun downstream work or replace its
input.

Download and Continue processing are independent choices; Continue processing
remains a CLI or Agent action.

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

## Web Workspace View

The Web Workspace view is the non-editing file view defined by
[Photo Workspace Web View](web.md).

It MUST present the workspace input and completed files with Download and
Remove. It MUST NOT present tool controls, Edit State, a Preview action, Export,
or Continue processing. An existing Preview file MAY be inspected and downloaded
as a workspace file; that is not a Preview action.

Opening, file inspection, download, removal, failure, and accessibility follow
[Photo Workspace Web View](web.md).

The view MUST NOT edit Library selection, Rating, Album membership, navigation,
or saved tool intent. Remove follows workspace removal rules.

## Lifetime and Recovery

The workspace is durable working storage.

Inputs, saved edit state, required
resources, and completed outputs MUST remain until explicitly removed.

They
MUST NOT inherit a temporary download or task receipt expiry.

An individually selected, unreferenced preview MAY be explicitly removed. A
presenting client that displayed it MUST clear it and disclose the removal.

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

