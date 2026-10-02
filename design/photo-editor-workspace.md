# Photo Editor Workspace Design

A Photo Editor spans Library browsing, durable Edit Recipe state, asynchronous
Processing Step results, and retained Exports. Those parts have different
owners. Without a workspace boundary, a late preview or save response can
change the wrong Photo, and a later step can silently use a different input
than the one the Photographer saw.

[Photo Development](../docs/photo-development.md) owns the user-visible
behavior. [Photo Development Architecture](photo-development.md) owns recipe,
source, processing, and Export service contracts.
[Processing Modules](processing-modules.md) owns the module boundary, step
identity, and artifact handoff. This document defines the browser workspace
boundary that composes those contracts.

## Design Drivers

- A Photo is the stable user context. A Processing Step or preview is not a new
  Photo.
- One Photo has one current Edit Recipe containing zero or more Processing
  Steps. The browser selects one active step; a module is not a second source or
  a virtual copy, and the Film Recipe stays fixed for an admitted Film step.
- Original Files and XMP Sidecars remain outside browser and processing writes.
- Expensive work is asynchronous and may settle after the Photographer leaves
  the Photo, changes the recipe, or loses the connection.
- Camera Preview, an Edit Preview, and a published Processing Artifact have
  different provenance and color contracts. The workspace must never substitute
  one for another.
- Web and programmatic clients use the same service facts and revision guards.
- The product contract presents one Edit workspace that opens on the selected
  current Processing Step's result. The camera Preview is a separate reference
  action. Choosing a result view never changes saved parameters or silently
  replaces the step's input.

## Model

The workspace owns one active Photo binding and a browser-local editing session.
The service owns durable facts and processing lifecycle.

### Photo binding

A binding contains the stable Photo identity, the observed Original revision,
the current Edit Recipe revision (or no recipe), and the module availability
snapshot used to open the workspace. It remains the address of every read,
preview, comparison, and Export action in the session.

A source revision change invalidates source-dependent results. It does not
remove the Edit Recipe. Applying the old recipe to changed content requires the
existing explicit rebind rules; the workspace must not perform that action as a
side effect of reopening or previewing.

### Browser session

The browser owns:

- the active Photo binding and the selected current Processing Step;
- the local draft and its relationship to the observed recipe revision;
- session undo/redo entries for completed edit actions;
- pending view interaction, comparison choice, zoom, and keyboard/touch focus;
- request ownership keys used to ignore obsolete responses.

The browser does not own the confirmed recipe, source identity, processing
bundle, Export receipt, or artifact retention.

### Durable service facts

The service owns:

- the current Edit Recipe and guarded revision;
- source binding and source revision evidence;
- per-module availability and refusal reasons;
- Edit Preview and comparison results;
- Export request identity, captured snapshot, state, artifact, and receipt;
- queue admission, resource checks, cancellation settlement, and retention.

Engine-private history, Python objects, and filesystem paths remain behind the
processing boundary defined by [Photo Development Architecture](photo-development.md).

### Result relationship

The workspace composes the camera reference with the current Processing Step's
results, each with distinct provenance:

- **Camera:** the existing camera-produced Preview. It is a reference only,
  reached through the Original reference action.
- **Current step:** the bounded Edit Preview of the selected Processing Step
  against its captured input and parameter snapshot. It is the default editing
  view.

Each available module contributes a selectable Processing Step only when it
reports readiness, input compatibility, and an admitted parameter/output
combination. The workspace identifies the selected module and its explicit
input. Peer modules have no required order, and selecting one never invokes
another. There is no separate Film capability: availability and refusal reasons
belong to the selected module.

A step's input is the Photo's Original binding or a published Processing
Artifact the caller selected explicitly. Only a published, immutable Export
artifact can become a later step's input; an Edit Preview, a display rendition,
and a Camera Preview never can. Composition is caller-controlled: zero, one, or
many steps, in any module order the caller chooses, as defined by
[Processing Modules](processing-modules.md#model-and-ownership).

Entering the workspace selects the caller's current step, if one exists. With
no current step, the workspace waits for an explicit module and compatible
input selection; it must not invent an engine or pipeline. The Camera reference
remains independently reachable in every case. Selecting a result never changes
the recipe or source binding.

## Workspace lifecycle

### Open

1. Resolve the Photo by stable identity and obtain the current source, recipe,
   module availability, and retained result facts.
2. Create the Photo binding from that response.
3. Select the caller's current step, or wait for an explicit module and
   compatible input selection when none exists.
4. Restore the confirmed recipe or the baseline/as-shot values when no recipe
   exists.
5. Render only a result whose identity matches the binding, selected step,
   recipe revision, processing bundle, and display conversion.

An unavailable Original or Processing Module leaves the Photo and its saved
facts inspectable. It does not cause the browser to invent a result or disable
unrelated Library decisions.

### Edit action

A completed pointer drag, committed numeric entry, or settled keyboard
adjustment is one edit action. The browser updates the local draft immediately,
then sends one guarded recipe write and requests a preview for the resulting
recipe snapshot. Intermediate pointer positions must not create a sequence of
service writes.

The service confirms the expected source and recipe revisions before committing.
A successful response may settle after the browser has accepted a newer action;
that response can mark only its own matching snapshot as saved. It cannot
replace newer draft values or step results.

Reset exposure, reset white balance, and reset all are ordinary edit actions.
They are undoable and do not delete an existing recipe row.

### Preview action

A preview request carries the Photo identity, source revision, recipe snapshot,
selected step, processing bundle, and display conversion. The browser may
retain the last successful image while the requested one is pending, but must
label it out of date. It must not present it as current.

A result may update the visible canvas only while all of these still match the
active request owner. A late, cancelled, failed, or different-step result is
settled as its own outcome and cannot overwrite the canvas or status of a newer
request.

A preview executes only the selected current step against its captured input
and parameters, bounded to the admitted rendition geometry. It never creates a
full-resolution handoff, substitutes the Camera Preview, or invokes another
module to prepare input or finish output; a missing compatible input is a
refusal, not permission to run an upstream step. A later edit to the current
step makes its prior previews stale. It never retargets an accepted Export or
a downstream step bound to a published artifact.

### Comparison

Comparison is owned by the selected step. It pairs the current settings with
that step's as-shot/baseline settings while holding Photo identity, geometry,
processing bundle, and display conversion constant. Camera Preview is never a
comparison operand for a step comparison.

If either operand is pending, stale, unavailable, or from a different identity,
the workspace reports that comparison is unavailable. It does not fall back to
an unrelated image.

### Leave, navigation, and reopen

Leaving the Photo does not cancel a guarded recipe write or an accepted Export.
The Photo scope keeps ownership of its response, while the visible workspace
moves to the next Photo. A response for the prior scope cannot change the new
Photo's controls, canvas, selected step, or notices.

Reopening the Photo reads current service facts again. It restores the confirmed
recipe, not a durable browser undo history. A recovered local draft is marked
local until the service confirms it and must not silently overwrite a newer
server revision.

## State projection

The workspace presents independent state axes. A success on one axis must not
stand in for success on another.

- **Module availability:** each Processing Module reports `ready`,
  `unsupported`, or `unavailable`, with the service's reason. Resource
  availability and engine readiness remain separate from source support, and
  one module's state is never reported as another's.
- **Recipe:** no recipe, draft, saving, saved, conflict, or save failure.
- **Preview:** current, pending, stale, unavailable, or failed, bound to its
  identity tuple of input, module, parameters, geometry, bundle, and display
  conversion ([Processing
  Modules](processing-modules.md#identity-compatibility-and-failure)).
- **Export:** accepted, queued, running, succeeded, failed, cancelled,
  outcome-unknown, or expired, using the service receipt and captured snapshot.
- **Connection:** transport reachability is separate from the Photo's current
  facts and operation settlement.

The UI may combine these facts into one concise status message, but it must
retain their meanings. For example, a saved recipe with a failed Edit Preview
is still saved; a ready module with a queued preview is not a current result; a
lost response is not proof that a write failed.

## Ownership and ordering rules

- Every browser continuation carries a Photo-scope generation. Leaving the
  Photo, replacing its source binding, or changing the selected step
  invalidates only the continuations it owns.
- Recipe writes are ordered by the captured expected recipe revision and source
  revision. The service's guarded result is authoritative.
- Preview settlement is ordered by the captured identity tuple, not by network
  completion time.
- Export submission is admitted only after the captured settings are confirmed.
  The accepted snapshot is immutable even if the browser later changes its
  draft.
- A response with an uncertain effect must be reconciled through the existing
  request identity before a dependent write or Export is accepted.
- Browser recovery data cannot create a new Photo, source binding, or Export.
- Step changes do not reset controls, selection state, Rating, Album
  membership, browsing position, or the Photo binding.

## Failure behavior

- **Unsupported source or mode:** keep browsing and saved facts usable; disable
  the affected step/control and state the qualification boundary.
- **Original unavailable or unreadable:** preserve the recipe and prior results;
  show the source reason and do not substitute a Camera Preview for a step
  result.
- **Recipe conflict:** stop autosave for the Photo, retain local intent, show
  the observed revision, and offer use-current or explicit reapply.
- **Preview failure:** retain the confirmed recipe and last successful result,
  label the current request failed, and provide a retry that captures current
  facts. Do not show an old result as current.
- **Resource refusal:** distinguish queued work from refusal and identify the
  module or target. Do not reduce dimensions, disable module effects, change
  numerical quality, or retry indefinitely.
- **Export cancellation race:** settle to the actual terminal state. A dropped
  response alone is outcome-unknown and requires reconciliation.
- **Expired receipt or artifact:** report explicit expiry. Repeating an old
  request identity must not create new work.

Original Files, XMP Sidecars, Photo identity, Selection State, Rating, Album
membership, and unrelated recipes remain unchanged in every failure path.

## Options

### Selected: One Photo-scoped workspace with one Edit surface

A single Photo scope owns the binding, draft, revision guards, async
generations, comparison, and Export entry. The current step, its peer module
views, and the camera reference are views over that scope, not separate owners.
This hides step-ordering and prevents a module view from owning a competing
recipe or source identity.

### Rejected: Independent editor pages per module

Separate Camera and per-module pages would duplicate recipe, source, and async
ownership. Their responses could disagree about the current Photo or let one
module view retain a stale input from another step. The current product needs
one durable recipe per Photo and explicit artifact handoff, so separate page
owners add change and failure paths without product value.

### Selected: Service-confirmed facts with browser-local reversible intent

The service remains authoritative for identity, revisions, processing, and
retention. The browser keeps immediate draft feedback and session undo/redo.
This gives responsive controls without making reload recovery or a late local
response authoritative over a newer server revision.

### Rejected: Browser-owned recipe and result cache as source of truth

A browser cache cannot protect against another client, source recovery, engine
bundle changes, or uncertain write outcomes. It would make Web and CLI behavior
diverge and could present stale pixels as current.

## Verification

An implementation is conforming when focused checks and a real supported-client
run demonstrate all of the following:

- The workspace opens on the caller's current step; with no current step it
  waits for explicit module and compatible-input selection. Each module choice
  is offered only when that module reports readiness and its own reason.
- A change in one Photo cannot update another Photo after navigation.
- One completed edit action creates one guarded save and one matching Preview
  request, while intermediate drag positions remain local.
- A late save, Preview, or comparison response cannot replace newer local or
  server-confirmed state.
- An Edit Preview executes only the selected step at admitted geometry: it
  creates no full-resolution handoff, substitutes no Camera Preview, and
  invokes no other module.
- Only a published, immutable Processing Artifact can be selected as a later
  step's input; an Edit Preview and a Camera Preview cannot.
- A changed Processing Step makes only matching Previews stale without
  retargeting an accepted Export or a downstream step bound to an artifact.
- A previously completed Processing Artifact remains available when another
  selected module fails.
- An accepted Export keeps its captured settings after a later edit and
  survives browser departure.
- Original/XMP bytes and independent Library decisions remain unchanged.
- Web and CLI observe the same Photo identity, recipe revision, module
  availability, Preview identity, Export receipt, and terminal outcome.

The governing Product Spec, service Design Spec, module contract, color
contract, and CLI reference remain the authoritative sources for their
respective details. This workspace spec is the implementation boundary between
them; it does not add a new processing API, storage model, or user-visible
editing feature.
