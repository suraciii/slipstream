# Photo Editor Workspace Design

A Photo Editor spans Library browsing, durable Edit Recipe state, asynchronous
stage results, and retained Exports. Those parts have different owners. Without
a workspace boundary, a late preview or save response can change the wrong Photo,
and Film can silently use a different Edit than the one the Photographer saw.

[Photo Development](../docs/photo-development.md) owns the user-visible
behavior. [Photo Development Architecture](photo-development.md) owns recipe,
source, processing, and Export service contracts. This document defines the
browser workspace boundary that composes those contracts.

## Design Drivers

- A Photo is the stable user context. A stage or preview is not a new Photo.
- One Photo has one current Edit Recipe. Film is derived from the same recipe;
  it is not a second source or a virtual copy.
- Original Files and XMP Sidecars remain outside browser and processing writes.
- Expensive work is asynchronous and may settle after the Photographer leaves
  the Photo, changes the recipe, or loses the connection.
- Camera Preview, Edit Preview, and Film Result have different provenance and
  color contracts. The workspace must never substitute one for another.
- Web and programmatic clients use the same service facts and revision guards.
- The product contract presents one Edit workspace that opens on the Develop
  result. Film is an optional action when the Film stage is ready, and the
  camera Preview is a separate reference action. Choosing a result view never
  changes the recipe or source binding.

## Model

The workspace owns one active Photo binding and a browser-local editing session.
The service owns durable facts and processing lifecycle.

### Photo binding

A binding contains the stable Photo identity, the observed Original revision,
the current Edit Recipe revision (or no recipe), and the capability snapshot
used to open the workspace. It remains the address of every read, write,
preview, comparison, and Export action in the session.

A source revision change invalidates source-dependent results. It does not
remove the Edit Recipe. Applying the old recipe to changed content requires the
existing explicit rebind rules; the workspace must not perform that action as a
side effect of reopening or previewing.

### Browser session

The browser owns:

- the active Photo binding and selected stage;
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
- stage capability and support reasons;
- Edit Preview and comparison results;
- Export request identity, captured snapshot, state, artifact, and receipt;
- queue admission, resource checks, cancellation settlement, and retention.

Engine-private history, Python objects, and filesystem paths remain behind the
processing boundary defined by [Photo Development Architecture](photo-development.md).

### Result relationship

The workspace composes three results with distinct provenance:

- **Camera:** the existing camera-produced Preview. It is a reference only,
  reached through the Original reference action.
- **Develop:** the Edit Preview of the Development Result for the current Edit
  Recipe snapshot. It is the default editing view, and its independent export
  target is `development-tiff`.
- **Film:** the Edit Preview of the Film Result derived from that same Edit
  Recipe snapshot and the fixed Film Recipe. It is reachable through the Film
  action only when the Film capability is `ready`; its export target is
  `film-jpeg`.

Entering the workspace selects Develop. The Film action stays unavailable with
the service's reason while Film is unavailable or unsupported. The Camera
reference remains independently reachable in both cases. Selecting a result
never changes the recipe or source binding.

## Workspace lifecycle

### Open

1. Resolve the Photo by stable identity and obtain the current source, recipe,
   capability, and retained result facts.
2. Create the Photo binding from that response.
3. Select Develop; enable the Film action only when its capability is `ready`.
4. Restore the confirmed recipe or the baseline/as-shot values when no recipe
   exists.
5. Render only a result whose identity matches the binding, stage, recipe
   revision, processing bundle, and display conversion.

An unavailable Original or processing stage leaves the Photo and its saved
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
replace newer draft values or stage results.

Reset exposure, reset white balance, and reset all are ordinary edit actions.
They are undoable and do not delete an existing recipe row.

### Preview action

A preview request carries the Photo identity, source revision, recipe snapshot,
stage, processing bundle, and display conversion. The browser may retain the
last successful image while the requested one is pending, but must label it
out of date. It must not present it as current.

A result may update the visible canvas only while all of these still match the
active request owner. A late, cancelled, failed, or different-stage result is
settled as its own outcome and cannot overwrite the canvas or status of a newer
request.

Film requests must use the matching Development Result from the captured Edit
snapshot. A later Edit action makes prior Film previews stale; it does not
retarget an already accepted Export.

### Comparison

Comparison is owned by the selected stage. It pairs the current settings with
the stage's as-shot/baseline settings while holding Photo identity, geometry,
processing bundle, and display conversion constant. Camera Preview is never a
comparison operand for a Develop or Film comparison.

If either operand is pending, stale, unavailable, or from a different identity,
the workspace reports that comparison is unavailable. It does not fall back to
an unrelated image.

### Leave, navigation, and reopen

Leaving the Photo does not cancel a guarded recipe write or an accepted Export.
The Photo scope keeps ownership of its response, while the visible workspace
moves to the next Photo. A response for the prior scope cannot change the new
Photo's controls, canvas, stage, or notices.

Reopening the Photo reads current service facts again. It restores the confirmed
recipe, not a durable browser undo history. A recovered local draft is marked
local until the service confirms it and must not silently overwrite a newer
server revision.

## State projection

The workspace presents independent state axes. A success on one axis must not
stand in for success on another.

- **Capability:** `ready`, `unsupported`, or `unavailable`, with the service's
  reason. Resource availability and engine readiness remain separate from source
  support.
- **Recipe:** no recipe, draft, saving, saved, conflict, or save failure.
- **Preview:** current, pending, stale, unavailable, or failed, bound to its
  identity tuple.
- **Export:** accepted, queued, running, succeeded, failed, cancelled,
  outcome-unknown, or expired, using the service receipt and captured snapshot.
- **Connection:** transport reachability is separate from the Photo's current
  facts and operation settlement.

The UI may combine these facts into one concise status message, but it must
retain their meanings. For example, a saved recipe with a failed Film Preview
is still saved; a ready Film capability with a queued preview is not a current
Film result; a lost response is not proof that a write failed.

## Ownership and ordering rules

- Every browser continuation carries a Photo-scope generation. Leaving the
  Photo, replacing its source binding, or changing stage invalidates only the
  continuations it owns.
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
- Stage changes do not reset controls, selection state, Rating, Album
  membership, browsing position, or the Photo binding.

## Failure behavior

- **Unsupported source or mode:** keep browsing and saved facts usable; disable
  the affected stage/control and state the qualification boundary.
- **Original unavailable or unreadable:** preserve the recipe and prior results;
  show the source reason and do not substitute a Camera Preview as Edit/Film.
- **Recipe conflict:** stop autosave for the Photo, retain local intent, show
  the observed revision, and offer use-current or explicit reapply.
- **Preview failure:** retain the confirmed recipe and last successful result,
  label the current request failed, and provide a retry that captures current
  facts. Do not show an old result as current.
- **Resource refusal:** distinguish queued work from refusal and identify the
  stage or target. Do not reduce dimensions, remove Film effects, change
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
generations, comparison, and Export entry. Develop, Film, and the camera
reference are views over that scope, not separate owners. This hides
cross-stage ordering and prevents a Film view from owning a competing recipe
or source identity.

### Rejected: Independent editor pages per stage

Separate Camera, Develop, and Film pages would duplicate recipe, source, and
async ownership. Their responses could disagree about the current Photo or let
Film retain a stale Development Result. The current product needs one durable
recipe and one cross-stage dependency, so separate page owners add change and
failure paths without product value.

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

- The workspace opens on the Develop result; the Film action is enabled only
  when the capability reports `ready` and otherwise states why Film is
  unavailable.
- A change in one Photo cannot update another Photo after navigation.
- One completed edit action creates one guarded save and one matching preview
  request, while intermediate drag positions remain local.
- A late save, preview, or comparison response cannot replace newer local or
  server-confirmed state.
- A Film Preview and Finished JPEG use the same captured Edit snapshot and fixed
  Film Recipe; changing the recipe makes prior Film results stale.
- Camera Preview never satisfies an Edit or Film result requirement.
- A conflict retains local intent and requires explicit use-current or reapply.
- A successful Development TIFF remains available when Film fails.
- An accepted Export keeps its captured settings after a later edit and survives
  browser departure.
- Original/XMP bytes and independent Library decisions remain unchanged.
- Web and CLI observe the same Photo identity, recipe revision, capability,
  preview identity, Export receipt, and terminal outcome.

The governing Product Spec, service Design Spec, color contract, and CLI
reference remain the authoritative sources for their respective details. This
workspace spec is the implementation boundary between them; it does not add a
new processing API, storage model, or user-visible editing feature.
