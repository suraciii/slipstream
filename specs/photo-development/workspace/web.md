# Photo Workspace Web View

The Web Workspace view lets a Photographer inspect, download, and remove files
in a Photo Workspace. It does not edit a Photo or run a tool. Remove follows
workspace removal rules and never rewrites saved tool intent.

[Photo Development](../spec.md) owns processing behavior. [Photo Workspace](design.md)
owns directory, file, accessibility, and retention behavior. [Photo Development
Architecture](../design.md) owns tool integration and supervised work.

## Boundary

The Web Workspace view is a file view for one Photo's Workspace.

- It MUST show the workspace input and completed files.
- It MUST offer Download for each ready file.
- It MUST offer explicit Remove for eligible files under [Photo Workspace](spec.md).
- It MUST NOT expose tool controls, Edit State, a Preview action, Export, or
  Continue processing.
- It MUST NOT initialize a tool, submit a tool command, or render a Preview.
- It MUST NOT present an absolute server filesystem path as client-local.

The camera-produced Preview remains a Photo View fact. It is not a workspace file
and MUST NOT appear as a Workspace result.

## Open and Resume

Photo View MUST offer **Workspace** to open or reopen the Photo's workspace.

Opening MUST create or reuse at most one default workspace and its protected
input. A missing Original or failed input preparation MUST be reported with a
concrete recovery action; the view MUST NOT start a tool to hide the absence.

Opening MUST NOT select, initialize, or render a tool. It MUST NOT require a
Preview before files become visible.

A contemporaneous duplicate open MUST resolve to the same workspace.

## Files

The view MUST group files by role:

- input;
- outputs; and
- other files.

A file MUST show its filename, role, size, readiness, and any known image facts.
A file whose type is directly displayable MAY show a thumbnail or image. An
existing Preview file MAY be inspected and downloaded as a workspace file; that
MUST NOT request or render a new Preview.

Private edit state and temporary partial files MUST NOT appear in the file list.

The file list MUST use the bounded workspace file listing and its cursor. It
MUST NOT assume every file fits one response.

The view MUST NOT generate a display conversion. A file that cannot be displayed
MUST remain a generic file with Download available.

Selecting a file MUST change only the inspected file. It MUST NOT change Library
selection, Rating, Album membership, navigation, or tool state.

Download MUST use the file's checked revision. A changed or unavailable file
MUST refuse the stale download and report the current readiness.

Remove MUST follow [Photo Workspace](spec.md) admission, dependency blockers, and
exact receipt settlement. An unknown outcome MUST be reconciled from workspace
file facts before another attempt. It MUST NOT edit Library selection, Rating,
Album membership, or navigation, or claim a tool run.

## Presentation

The view MUST show the inspected file or a truthful generic-file state, the file
list, and the execution host when available. It MAY show an absolute Workspace
path only as an execution-environment path; it MUST NOT present it as a
client-local path.

The initial view MUST be the Workspace file set, not a blank stage and not a
tool-selection surface.

Wide layouts MAY place the inspected file beside the list. Narrow layouts MUST
keep the file and primary Download action reachable without horizontal overflow.

The view MUST NOT present a save, Undo, Preview, Export, or engine-status
control.

## Failure and Recovery

A missing or changed workspace input MUST remain distinct from an unavailable
file and from a Library Original that is missing or changed.

A failed file read, download, or removal MUST name the affected file or scope
and leave unrelated files usable.

The view MUST NOT claim that a download proves processing success, tool
execution, or file provenance.

## Accessibility

File cards, Download, and Remove MUST be keyboard and touch operable. Focus MUST
remain within the Workspace surface after selection, download start, removal, or
failure.

Text at 200% enlargement MUST remain usable without horizontal page overflow.

## Acceptance

- A Photographer can open a Workspace from Photo View, inspect its files, and
  download a ready file without a tool call.
- A workspace with no displayable output still presents its input and generic
  files with Download.
- A Workspace view exposes no editing, Preview, Export, or Continue action;
  Remove remains an explicit workspace action.
