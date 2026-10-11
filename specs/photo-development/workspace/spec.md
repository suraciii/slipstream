# Photo Workspace

A Photographer needs one place to take a Photo through independent tools, inspect
previews, download completed files, and continue processing. Moving each result
out of Slipstream and submitting it back after every edit interrupts that work.
A Photo Workspace must therefore be a real persistent directory. Tools must read
its inputs and write their results directly into it. The Web Edit view and file
commands must present those same files.

[Photo Development](../spec.md) owns processing behavior.
[Headless darktable Editing](../darktable/editing.md) owns darktable capabilities.
[CLI Reference](../../command-line/reference.md) owns workspace commands, and
[darktable CLI Reference](../darktable/cli.md) owns independent editing and the
optional `slipstream darktable` integration.

## Open and Resume

The Photo View must offer Edit to open or reopen the Photo's workspace. Opening
must create at most one default workspace for that Photo, including concurrent
requests. The caller must not choose a storage directory, construct an artifact
record, or initialize a processing recipe before useful work.

First preparation must place a verified, protected snapshot of the selected
Original in the workspace. It must show preparation progress and a specific
failure if the Original is unavailable, changed during copying, or cannot fit.
No incomplete input may appear ready. Preparation must not run darktable or
change the Original, its XMP Sidecar, or Library decisions.

Initially, the only visible image must be that input. Directories for previews,
outputs, and tool state must appear only when needed. The input's original
filename must remain recognizable. A subsequent open must reuse existing files
and tool state, without recopying the Original or resetting edits. After explicit
removal of the default input, open must disclose that no default is selected.
It must keep remaining files and resumable edits available for explicit selection
without reading the Original, recreating removed files, or retargeting an edit.
Prepare input again explicitly refreshes from an available verified Original;
it reuses an intact matching snapshot or creates a new identity, never resurrects
an explicitly removed identity, and leaves removal receipts replayable.

If a workspace input is still intact, work must continue when the Original is
later missing or changed. The workspace must disclose that the snapshot differs
from the current Library source when known. Refreshing the source must be an
explicit action that adds a new input version. It must not replace bytes under
an existing edit or silently rebind any processing state.

## Directory and Execution Location

Workspace open must return its identity, Photo identity, absolute filesystem
path, execution location, prepared input path and checked identity, readiness,
existing tool states, and Web URL. Paths must be directly usable by a process in
the disclosed execution environment. Container and host paths must not be
presented as interchangeable.

The independent tool must run where it can access that directory. A remote
Slipstream client must not be told that a server path exists on its own machine.
The integration must either offer an available supervised tool on the workspace
host or report the required execution environment. Authorized shared filesystem
access may also supply that directory. The product must not require a new remote
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

`input/` must be protected from tool writes. `previews/` and `outputs/` hold
completed files. `.edits/` holds resumable tool state and required resources;
`.tmp/` holds incomplete work. The latter two must stay out of the normal image
list. These roles must not require an independent tool to understand Slipstream
identity or implement a Slipstream registration protocol. A tool may receive
explicit input, state, and output paths.

## Adjust and Preview

With one input and no saved state, selecting an available tool must use that
input. With one matching saved state, reopening the tool must resume it. Several
inputs or saved alternatives must require a clear choice by recognizable name
and thumbnail; the product must not guess from the newest file or lowest ID.
The user may explicitly start a separate edit without disturbing existing state.

An independent tool must save its editing state in the directory supplied for
that edit. The tool must own initialization, operation semantics, validation,
and persistence. Browser controls and `slipstream darktable` must use the same
tool state as independent calls. A user must not import, synchronize, submit, or
register a local change for it to appear in the workspace.

A completed adjustment must save before it is reported as saved. It must not
require a full render. Preview must render a captured saved version and write a
completed image into the preview directory. The workspace must show that version
and whether it still matches the selected edit. During an update, the last
completed preview remains visible with an Updating status. A late result for an
older version must not replace the preview of a newer version.

Changing settings and previewing must remain repeatable without regenerating a
full-resolution TIFF, restaging the RAW, or asking the caller to reconstruct all
parameters. Native tool capabilities and unsupported operations must remain
explicit. An unavailable tool must not prevent viewing and downloading existing
files.

## Export, Download, and Continue

Export must generate a file from the selected input and captured saved edit
version. It must not require a prior Preview or upscale a preview file. The tool
must provide suitable discoverable output choices, including actual format,
precision, color space, transfer function, and dimensions. Repeated exports must
use distinct generated names and preserve previous completed outputs.

A tool must finish the file before publishing its final path. Slipstream must
automatically discover it and show it after file validation. The user must not
upload it again or call a separate artifact registration or commit operation.
Successful tool processing and workspace file visibility are separate facts: if
indexing is still underway, the command must return the existing final path and
show Checking file rather than claim the render failed or rerun it.

Each completed output must offer Download and Continue processing. Download
must deliver the selected file's checked bytes with its recognizable filename.
It must not render again or return whatever output is now newest. Continue
processing must carry the exact selected file into the next tool without asking
the user to browse for it, copy it, or re-enter an artifact ID. With several
compatible tools, the user chooses one. With none, the file remains downloadable
and the product must explain the missing compatible capability.

The next tool must check the actual input contract. A TIFF filename alone does
not establish linear color, floating-point precision, or compatible geometry.
No conversion or additional processing tool may run merely because a file
appeared. Changing an earlier edit must not rerun downstream work or replace its
input. Download and Continue processing remain independent choices.

Preview files must be viewable and downloadable as previews. They must not be
offered as full-resolution or scene-linear handoff inputs. Tool state is not an
image result. A file written by an unrelated tool may be shown and downloaded,
but Slipstream must not invent the producing parameters, editing version, or
successful Export attribution. Such a file becomes an admitted next input only
after its actual contract passes the selected tool's checks.

## Web Interaction

The main Edit view must show the selected image, relevant tool controls, the
input name, and concise save and processing status. A Files disclosure must group
input files, previews, and outputs without requiring a separate workflow screen.
The initial view must not be a general file manager or a blank stage editor.
The user must see a prepared RAW and the available editing action immediately.

A file card must show a thumbnail when displayable, filename, file role, size,
and readiness. Dimensions and relevant output characteristics must be available
without exposing internal IDs. Provenance, full execution paths, revisions, and
diagnostic details belong in an optional details view. An output that cannot be
displayed directly must still offer its valid Download and compatible Continue
processing actions; any display conversion must be labeled as such.

Selecting a file for inspection must not change the active edit or bind a tool
to a different input. Continue processing is the explicit transition. Download
must not remove the file. Leaving Edit or returning from the Library must
preserve selected state and completed files. Keyboard actions and narrow-screen
presentation must retain the same actions and status meanings.

Files must offer Remove for an explicitly selected protected input as well as
other eligible files. Protection means read-only to tools, not forever retained.
The reviewed file revision must be checked at removal. A refusal must name the
dependent saved edits, jobs, outputs, resources, or active tools and offer the
relevant explicit rebind/remove/settle action; unknown external use must be
resolved before removal. No force action may break those dependencies. Removing
an unused older RAW must preserve unrelated edits/results and the Library
Original. Removing the default/last input must disclose that no default is
selected while preserving access to remaining files and edits. Offer explicit
selection of a compatible retained file, Prepare input again when the Original
is available, or source recovery otherwise.

The primary statuses must distinguish Preparing input, Saving settings,
Rendering preview, Exporting, Checking file, and Ready. Failures must name the
failed operation and a concrete recovery action. A successful settings save must
remain saved if the following render fails. Files must remain visible while an
unrelated operation is pending or fails.

## Lifetime and Recovery

The workspace is durable working storage. Inputs, saved edit state, required
resources, and completed outputs must remain until explicitly removed. They
must not inherit a temporary download or task receipt expiry. Disposable
previews and abandoned temporary files may be offered for cleanup with an
estimated size; cleanup must preserve the currently displayed preview and any
file required by a retained edit or active operation.
An individually selected, unreferenced preview may be explicitly removed; if it
was displayed, the view must clear it and disclose that removal. This explicit
action is distinct from bulk cleanup, which preserves the current preview.

Removal must check the selected revision, dependencies, and active use under
the same admission protection as binding a new edit or job. A new binding either
pins the input first and blocks removal, or observes that it has been removed
and refuses; it cannot retain a dangling dependency. Current or retained state,
captured jobs, kept outputs, declared dependencies, active independent tool use,
or unverified use must refuse removal with named blockers. An unused protected
snapshot may be explicitly removed without discarding other work. Deleting an entire workspace must require a separate
confirmation of its complete current loss inventory. A new file, changed state,
or changed dependency invalidates that confirmation. Active operations and
references from outside the workspace must refuse deletion. Accepted deletion
removes the workspace from use before reclaiming its files and reports any
unfinished reclamation honestly. It must never delete the Library Original.
Removing a Photo from the Library must not implicitly erase this work.

Cancellation, a crashed tool, or disk exhaustion must preserve previous complete
files and saved settings. Partial files must not appear ready or downloadable.
Closing a supervised client must leave service-owned accepted work queryable;
closing an independent process has only the standalone tool's documented
lifetime. A later workspace read must discover completed files even if the
service missed a filesystem notification or restarted during processing.

Concurrent writers must not overwrite a completed output. Conflicting edits to
one tool state must return Busy or an edit-version conflict, retaining the
caller's intended change. The product must not silently reload a newer revision
and overwrite it. Separate output files must not conflict merely because they
share the workspace. External replacement or in-place modification of a file
must invalidate its old content revision; old downloads and downstream bindings
must not silently continue against new bytes.

## Acceptance Scenarios

- Open a Photo twice. Both callers receive the same prepared workspace and one
  intact input snapshot. No engine runs and no Original or Sidecar changes.
- Open darktable, change exposure and a mask, preview twice, then export. The
  Web sees each completed result without upload or registration. No full-size
  TIFF is needed between adjustments. Restart and resume the saved mask.
- Run the independent tool without Slipstream, then run it against workspace
  paths. The file and state behavior is the same and requires no Photo ID,
  service credential, callback, or Slipstream database.
- Download a completed linear float32 TIFF, or select it for another compatible
  tool. Both use the same bytes. The next tool writes a new file in the same
  workspace; the earlier file remains available.
- Write an image with another tool using temporary output and a final rename.
  Slipstream discovers and validates it without a special manifest. Its unknown
  processing provenance stays unknown. An incomplete image is never Ready.
- Change the current edit while an older preview runs. Its file may remain
  inspectable, but the old result cannot replace the current preview.
- Remove the live Original after preparing a snapshot. That workspace continues
  to edit. Refreshing from changed source bytes requires explicit new input.
- Interrupt an export or exhaust storage. The previous preview, input, saved
  state, and completed outputs remain available. Retry does not overwrite them.
- Open the workspace from another host. Its response identifies where the path
  is usable and offers available supervised execution, not a fictitious local
  path or an obligatory per-preview transfer loop.
- Reopen after service restart or a missed file event. Completed files reappear;
  partial files remain hidden, and unrelated files are not attributed to a
  successful supervised job.
- Complete Film processing from a verified compatible TIFF written by an
  independent tool. Its unknown provenance stays unknown, the exact file
  revision is selected, and no upstream edit or Library Original is needed.
- Remove an unused old RAW snapshot while an edit and output use another input.
  They remain intact. A competing new bind either blocks removal or fails after
  removal; an active or unverified independent use also blocks it.
- Remove the last/default unused input, then reopen. No input is shown without
  touching the Original; explicit refresh can prepare a new identity. Existing
  TIFF-bound work continues when the Original is missing.
- Read an edit, change it externally, and save the old draft. The state-version
  conflict keeps both the newer state and intended draft; no guard is refreshed.
