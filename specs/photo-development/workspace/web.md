# Photo Editor Workspace Design

Editing combines a real Photo Workspace directory, tool-owned saved state,
asynchronous previews and completed files. Browser controls must make that
simple without letting late responses change another Photo or implying that
an external file proves a supervised run succeeded.

[Photo Development](../spec.md) owns product behavior;
[Photo Workspace](design.md) owns directory/files/accessibility and
retention; [Photo Development Architecture](../design.md) owns tool
integration and supervised work. This document owns the browser interaction
boundary. [Processing Modules](../processing/design.md) owns input compatibility.

## Design Drivers

- A Photo remains the Library context; outputs are workspace files, not new Photos.
- Users can edit and render in one persistent directory without download/submit
  on every iteration or manually choosing internal step IDs.
- The same tool-owned state serves Web and independently invoked CLI.
- Saved state, pending rendering, ready files and connection reachability have
  independent meanings.
- Original/XMP and camera Preview remain separate from workspace writes/results.
- External file changes and asynchronous completion must never be silently
  relabeled as current or attributed to an unrelated Web run.

## Model

The browser binds one Photo to its workspace, selected input file revision and
explicit edit directory. Slipstream supplies authorized file URLs and supervised
run facts; the tool supplies canonical state/revision and capability facts.
The actual absolute execution path and execution host are available for local
Agent use. Remote browsers use authorized URLs; a displayed server path is not
promised to exist on the user's machine.

### Photo binding

Binding includes Photo/workspace identity, selected input content revision,
selected tool/edit, observed canonical revision and bundle/schema identity.
A changed Library Original does not invalidate unchanged workspace input.
Explicit source refresh creates another input version; choosing/rebinding it is
a separate action that validates saved settings. Reopening never retargets an edit.

### Browser session

The browser owns selection, local draft, pending action/request identities,
session undo/redo, comparison, zoom and response generations. It keeps no durable
parameter authority. Recovered local drafts remain pending until confirmed by
the tool's guarded operation. Tool state survives browser closure independently
of browser storage.

### Durable service facts

Slipstream owns directory association, file-index observations/download URLs,
and bounded receipts/admission for runs it supervises. It projects tool state
through the same independent executable and caches pointers/revisions only.
A file index and supervised run are separate owners: a newly ready generic file
can be displayed even when no Web run is known. Exact validated completion
evidence is required to claim Processing Artifact provenance or run success.

### Result relationship

The central canvas shows the latest relevant completed Preview for the selected
edit and revision, retaining the previous image with an out-of-date label while
new work runs. The camera-produced Preview remains an explicit Original reference,
never a hidden fallback or comparison operand.

The Files drawer groups input, previews and outputs, with generic files/resources
available through inspection. The initial workspace has one visible image:
`input/<original filename>`. Private edit directories and temporary partials do
not clutter result cards. Selecting a file does not mutate saved settings.
Outputs offer Download and Continue Processing when actual input compatibility
is verified. Previews are viewable/downloadable renditions; they are not assumed
full-resolution scene-linear sources. Unknown types remain generic files.

## Workspace lifecycle

### Open

Photo View's Edit action creates or reopens the same workspace idempotently.
Select the existing edit when the caller identifies it; otherwise show saved
edits without guessing among ambiguous states. With no edit, choose the one
initial input and offer compatible available tools. A tool choice initializes
its state in the workspace through the same tool used by CLI. The initial input
card and camera reference make the page useful before a processed Preview exists.

Reopen reads actual file/state facts and reconciles missed events. Missing live
Original, unavailable tool or resource preserves already staged input, saved
state and completed downloads. It disables only the affected action with a reason.
The UI must not automatically run all saved edits just to populate the page.

### Edit action

A completed drag, numeric commit or settled keyboard change is one action.
Intermediate positions stay local. Send one guarded tool mutation, preserve
unexposed settings, and request a Preview of its confirmed revision. One in-flight
save per edit coalesces later draft actions. An acknowledgement confirms only its
own action and cannot erase newer draft values.

Reset is an ordinary undoable mutation using tool-initialized image defaults;
it does not delete saved state. External revision conflicts preserve the draft,
stop blind autosave and offer inspect-current or explicit guarded reapply.
Unknown action outcomes retain request identity for exact reconciliation.

### Preview action

Capture workspace/edit, input content evidence, canonical revision, bundle,
geometry and display conversion. Only a matching active owner may update the
canvas. Old revision completion may add a labeled file to the Files list, but
cannot replace the current image/status. Cancelling computation and ignoring
late obsolete output are separate duties.

Render only the selected tool at bounded qualified geometry. Do not invoke an
upstream tool, make a full-size handoff or silently use Camera Preview. A failed
render preserves saved state and the previous image, labeled stale. Retry is
explicit and captures current confirmed facts.

### Comparison

Pair current settings with the selected tool's initialized baseline or explicit
retained revision while fixing input, geometry, bundle and display transform.
Each operand has independent ownership. Pending/stale/incompatible operands
make comparison unavailable, without substitution by an unrelated camera image.

### Leave, navigation, and reopen

Navigation moves visible ownership; pending saves and accepted supervised exports
retain their original Photo/edit owner. Late results cannot change another Photo.
Reopen reads canonical tool state and indexed files, not a durable browser recipe.
Local drafts require conflict reconciliation. Closing the browser does not cancel
accepted supervised exports or erase tool state. Direct CLI process ownership
is independent of browser navigation and service restart.

The page-local workspace owner keeps one serialized draft queue for each Photo
and selected edit. Native module descriptions drive recursive numeric, enum,
object, array and curve controls, with initialized reset, instance ordering,
blend and mask graph controls. Unsupported schema keywords are shown explicitly
and retain the original saved values. Read-only open never queues a Preview.
Resources, Presets and Styles use the tool's disclosed edit-local or global
catalog, exact immutable versions, explicit capture fields and complete Style
entry mappings. Local import and definition download use bounded binary transfer
handles, never browser-supplied server paths. Export settles the preceding write
queue and submits its captured saved revision before admitting subsequent saves.
The Files disclosure reviews the exact selected revision before Remove, displays
named blockers, and downloads the selected checked revision independently of
current settings. Browser entry URLs use `photoId`, `edit=1` and optional `editId`.

## State projection

Present independent axes with concise actionable labels:

- **Tool:** ready, unsupported or unavailable, separate from input/resources.
- **State:** no edit, local draft, saving, saved, conflict, save failure or unknown.
- **Preview:** pending, current, stale, unavailable or failed with exact identity.
- **File:** checking, ready, changed or unavailable under the workspace contract.
- **Web run:** queued, running, succeeded, failed, cancelled or outcome-unknown.
- **Connection:** service reachability, independent of saved state and file existence.

Saved settings with a failed Preview are still saved. A newly discovered file
is downloadable when ready even if no supervised task exists. It must not be
shown as a completed Export with fabricated provenance. An external in-place
rewrite invalidates old revision/download evidence and shows checking/changed;
old metadata cannot continue describing the new bytes.

## Ownership and ordering rules

Visible continuations carry Photo/workspace/edit generations and full result
identity. Tool writes use explicit expected revision and request identity; Web
does not silently refresh guards. Export creates a write-stream barrier: settle
prior actions, confirm the captured visible settings, and submit that exact
revision before later saves proceed. Its receipt and file remain tied to that
snapshot after edits advance.

Independent input/output selections never mutate upstream state or launch another
tool. Continue Processing explicitly selects a compatible completed file and
creates/opens the next tool's state, with no automatic conversion. Downloads
address an indexed file revision, not the currently selected edit. Duplicate
same-directory runs are associated only by exact completion evidence.

The retained Film surface uses the generic adapter exchanges in
[CLI Reference](../../command-line/reference.md#retained-processing-adapter-exchanges).
Continue processing copies the inspected completed file's workspace identity,
artifact identity, revision and actual image contract into a `workspace-artifact`
binding. Available Film discovery supplies the complete default parameter tree
and structured controls; compatibility checks color, transfer, sample precision
and geometry before presenting the action. Saving echoes the observed guards and
every unrelated retained step unchanged. Preview and Export settle the Film draft
first and execute only that selected saved step. Unknown writes keep their exact
request bytes for reconciliation; a conflict preserves the draft and requires
inspection followed by explicit reapplication. Several saved Film states for the
same file require a choice. Completed files remain in the shared Files drawer for
revision-bound Download and subsequent processing.

## Failure behavior

Unsupported capability/input disables that action while preserving browsing and
state inspection. Missing live Original is distinguished from a missing/changed
workspace input. A guarded conflict retains local intent; an unknown outcome
requires replay/reconciliation rather than duplicate action. Preview failure
leaves the last successful image stale. Resource/disk refusal identifies the
required action and never silently reduces dimensions, precision or effects.

Cancel/complete races report actual settlement and preserve completed files.
Service restart reconciles supervised work; independently invoked CLI may
continue and later contribute ordinary indexed files. The UI never invents a
server task for it. Cleanup explains disposable files and freed space, excluding
inputs, current edit state, dependencies and kept outputs. Explicit deletion is
separate from closing the editor.

## Options

### Selected: One Photo-scoped workspace with one Edit surface

A Photo scope owns visible selection, drafts and async generations over the real
directory. The canvas and grouped Files drawer provide direct editing/download
without exposing private state paths as routine navigation steps. Tools own their
persistent edits; the page does not impose a fixed processing pipeline.

### Rejected: Independent editor pages per Engine

Separate page owners duplicate Photo/input selection and late-response policy.
One workspace can present different tool states without a competing source of
truth or mandatory module ordering.

### Selected: Tool-confirmed facts with browser-local reversible intent

Canonical state provides CLI/Web consistency; local drafts provide immediate
feedback. File-index reconciliation handles external outputs independently from
supervised receipts.

### Rejected: Browser-owned state and output cache

A browser cache cannot preserve external edits, authoritative revision guards,
resource meaning or uncertain operation outcomes. It can show drafts and cached
images only with disclosed identity/staleness.

## Verification

Future browser evidence must show initial input -> tool selection -> saved edits
-> relevant Preview -> Export file -> Download or explicit compatible next tool,
all in the same workspace. Include reopen after browser/service restart, CLI
changes observed by Web, ambiguous saved states, external file addition/change,
and unsupported downstream input without an invented conversion.

Exercise navigation during save, late Preview/comparison, conflicts, unknown
replay, exact Export barriers, partial/disk-full failures and cancellation races.
Verify separate camera reference, saved-state preservation, unchanged Library
facts and no need to navigate internal step IDs, download/submit each iteration,
or register ordinary completed files.
