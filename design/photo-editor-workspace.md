# Photo Editor Workspace Design

A Photo Editor spans Library browsing, one durable Edit State, asynchronous
Preview and Export work, and retained Processing Artifacts. Those parts have
different owners. Without a workspace boundary, a late Preview or save
response can change the wrong Photo or replace a newer confirmed state.

[Photo Development](../docs/photo-development.md) owns user-visible behavior.
[Photo Development Architecture](photo-development.md) owns Edit State, source,
processing, and Export service contracts.
[Processing Modules](processing-modules.md) owns one Engine invocation and the
Artifact handoff boundary. This document defines the browser workspace
boundary around those contracts.

## Design Drivers

- A Photo is the stable user context. An Edit Preview or Processing Artifact is
  not a new Photo.
- One Photo has one current Edit State. The browser does not expose a
  cross-service pipeline, virtual copy, or user-visible recipe object.
- Original Files and XMP Sidecars remain outside browser and processing writes.
- Expensive work is asynchronous and may settle after the Photographer leaves
  the Photo, changes the Edit State, or loses the connection.
- Camera Preview, Edit Preview, and Processing Artifact have different
  provenance and image contracts. The workspace must never substitute one for
  another.
- Web and programmatic clients use the same service facts and revision guards.
- The product presents one Edit workspace for the current Edit State. The
  Camera Preview is a separate reference action, and an Artifact is a
  downloadable output or explicit input to another service.

## Model

The workspace owns one active Photo binding and a browser-local editing session.
The service owns durable facts and the processing lifecycle.

### Photo binding

A binding contains the stable Photo identity, the observed Original revision,
the current Edit State revision, and the Engine availability snapshot used to
open the workspace. It remains the address of every read, Preview, comparison,
and Export action in the session.

A source revision change invalidates source-dependent results. It does not remove
the confirmed Edit State. Applying the old state to changed content requires an
explicit guarded rebind; the workspace must not perform that action while
reopening or requesting a Preview.

### Browser session

The browser owns:

- the active Photo binding and the local Edit State draft;
- session undo/redo entries for completed edit actions;
- pending view interaction, comparison choice, zoom, and keyboard/touch focus;
- request ownership keys used to ignore obsolete responses.

The browser does not own the confirmed Edit State, source identity, processing
bundle, Export receipt, or Artifact retention.

### Durable service facts

The service owns:

- the current Edit State and guarded revision;
- source binding and source revision evidence;
- Engine availability and refusal reasons;
- Edit Preview and comparison results;
- Export request identity, captured input and Controls, state, Artifact, and
  receipt;
- queue admission, resource checks, cancellation settlement, and retention.

Engine-private history, native objects, and filesystem paths remain behind the
processing boundary defined by [Photo Development Architecture](photo-development.md).

### Result relationship

The workspace composes the camera reference with the current Edit State's
result:

- **Camera:** the existing camera-produced Preview, reached through the
  Original reference action.
- **Current edit:** the bounded Edit Preview for the selected Engine, input,
  and Controls.
- **Output:** a completed Export produces an immutable Processing Artifact with
  provenance and an actual image contract.

The workspace identifies the selected Engine, input, and Controls in plain
language. Choosing a different Engine or supplying an Artifact starts a
separate service operation or service-owned Edit State; it does not append a
user-visible stage to the Photo. A downstream service receives the Artifact and
owns its own state. Selecting a result never changes the confirmed Edit State
or source binding.

## Workspace lifecycle

### Open

1. Resolve the Photo by stable identity and obtain the current source, Edit
   State, Engine availability, and retained Artifact facts.
2. Create the Photo binding from that response.
3. Restore the confirmed Edit State, or show the baseline/as-shot controls when
   no state exists.
4. Render only an Edit Preview whose identity matches the binding, current
   revision, input, Engine, Controls, bundle, and display conversion.

An unavailable Original or Processing Engine leaves the Photo and its confirmed
facts inspectable. It does not cause the browser to invent a result or disable
unrelated Library decisions.

### Edit action

A completed pointer drag, committed numeric entry, or settled keyboard
adjustment is one edit action. The workspace sends one guarded update with the
observed source and edit revisions. The service confirms the update before
the workspace reports Saved.

The browser may retain a bounded local draft and session undo/redo entries. A
draft never silently replaces newer service state. Undo and redo submit new
guarded updates; they do not promise a durable action history.

### Preview action

A Preview request carries the Photo identity, current edit revision, input
identity, Engine, Controls, bundle, rendition geometry, and display conversion.
The workspace retains the last successful image while a new request is pending.
A later accepted edit makes the previous Preview stale.

A late, cancelled, failed, or identity-mismatched result cannot replace the
current view. Preview bytes are display renditions and cannot become an
Artifact or a downstream service input.

### Comparison

Comparison uses the current Edit State and the same input, geometry, and display
conversion as the baseline or as-shot operand. The Original reference remains a
separately labelled action and is not a comparison substitute.

### Leave, navigation, and reopen

Leaving a Photo does not cancel a guarded state update or an accepted Export.
A pending local draft retains its Photo binding until it is confirmed, refused,
or expires under its bounded recovery policy. Reopening reads the service's
current Edit State and retained Artifact facts; it does not infer success from
a browser draft or rebind a changed Original.

## State projection

The workspace presents these independent status facts:

- **Edit State:** no state, draft, saving, saved, conflict, or save failure.
- **Edit Preview:** unavailable, current, stale, pending, failed, or unknown.
- **Export:** queued, exporting, ready to download, failed, cancelled, or
  expired.
- **Engine:** ready or unavailable with an actionable refusal reason.
- **Source:** ready, waiting, changed, missing, or unreadable.

Revision tokens, request identities, bundle IDs, and processing reasons remain
available through an optional details affordance. The primary flow uses
editing state, controls, Preview, and output language.

## Ownership and ordering rules

The browser owns presentation, local draft, session undo/redo, comparison,
zoom, focus, and request ownership keys. The service owns Photo identity,
confirmed Edit State, source guards, Engine admission, Preview identity,
Export state, Artifact publication, leases, and retention.

A state update compares the observed source and edit revisions atomically. A
changed Original never becomes a new input by reopening, Preview, or Export.
Rebind is an explicit guarded operation.

Export captures the confirmed Edit State and output contract at acceptance.
Later edits create a new revision and never retarget an accepted Export or its
Artifact. Another service that receives the Artifact owns its own state and
must validate the Artifact's actual image contract.

Every operation with a caller request ID is idempotent. A lost response is an
uncertain outcome; the client replays the exact request identity and body.
Obsolete responses are ignored by the owning Photo binding and operation
identity.

## Failure behavior

- **Engine unavailable:** preserve the confirmed Edit State and explain the
  Engine-specific refusal. Do not substitute another Engine.
- **Unsupported Control:** refuse the update without rounding, dropping, or
  replacing the confirmed state.
- **Source changed:** retain the prior state and require explicit guarded
  rebind before applying it to new content.
- **Original unavailable:** preserve the state and earlier Artifacts. A
  compatible Artifact may remain readable by a receiving service under its
  lease; Camera Preview is not a substitute.
- **Edit conflict:** stop the local save, retain the draft, and show the latest
  confirmed state so the Photographer can choose whether to retry.
- **Preview failure:** retain the confirmed state and last successful Preview.
- **Export failure or cancellation:** do not publish a partial Artifact; retain
  earlier Artifacts and their download facts.
- **Expired Artifact:** report expiry and require a new explicit Export.
- **Library decisions:** Selection State, Rating, Album membership, and Original
  bytes remain unchanged in every editing failure path.

## Options

### Selected: One Photo-scoped workspace with one Edit surface

One Edit State keeps Photo identity, controls, guards, Preview, and Export in
one understandable workspace. Artifact transfer lets another service continue
without making the first service own a workflow or a second editor page.

### Rejected: Independent editor pages per Engine

Separate pages duplicate source, state, conflict, and asynchronous result
ownership. They make it easy for one page to retain stale input from another
and imply a product-level pipeline that does not exist.

### Selected: Service-confirmed facts with browser-local reversible intent

The service remains authoritative for confirmed state and retained output. The
browser can provide responsive drafts and session undo/redo without becoming a
second persistence system.

### Rejected: Browser-owned state and output cache

A browser cache cannot safely coordinate multiple clients, source changes,
Export settlement, Artifact leases, or uncertain network outcomes.

## Verification

Acceptance must prove:

- first Edit State creation and repeated guarded Control updates;
- source and edit revision conflicts leave confirmed state unchanged;
- Preview becomes stale after a later accepted edit and late results are ignored;
- Export works without a preceding Preview and publishes only validated output;
- lost save and Export responses replay with the same request identity;
- unavailable Engines and unsupported Controls preserve confirmed state;
- an immutable TIFF Artifact can be validated and consumed by another service;
- upstream edits do not retarget an existing Artifact;
- Original and XMP bytes, Selection State, Rating, and Album membership remain
  unchanged; and
- the workspace remains usable at narrow widths without horizontal overflow.

The expected behavior is derived from the product model in
[Photo Editing Model](../docs/photo-editing-model.md). Implementation may keep
internal snapshot records for replay, but those records are not additional
product objects.
