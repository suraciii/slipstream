# Web Frontend Architecture

The Web application uses Feature-Sliced Design (FSD) to separate Library Browser lifetimes and enforce dependency direction.

A layer or slice exists only when current behavior gives it a durable responsibility.

[Library Browsing and Selection](../library-browser/spec.md) owns user-visible behavior. [Web Async Ownership](async-ownership.md) owns cancellation, settlement, convergence, recovery, and failure presentation.

The client remains dependency-light Vanilla TypeScript and Vite.

Source structure MUST use the product language in [CONTEXT.md](../../CONTEXT.md).

A source reorganization MUST NOT introduce a UI framework, router, state container, query cache, or dependency-injection system.

Original Files remain read-only.

Browser protocol, Browse Snapshot coherence, admitted-write semantics, and deployment boundaries remain governed by their owning contracts.

## Model

### Layers

The application uses `app` and `pages`, from highest to lowest.

Dependencies MUST point downward. `app` MUST import only a page's public API, and code within a slice MUST use internal relative imports rather than its own public API.

The `pages/library-browser` slice exposes mounting into a supplied root and an idempotent disposal handle.

Application startup, page-hide handling, and global styling belong to `app`; Library Browser state and behavior do not.

Additional layers are introduced only for current responsibilities:

| Layer      | Admission boundary                                                                                                                               |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `shared`   | A domain-neutral mechanism has a current consumer outside the Library Browser; dependency-free code alone does not justify promotion.            |
| `entities` | A Photo, Album, or Original Folder owns behavior or a stable contract consumed outside the page; page-local wire types alone are insufficient.   |
| `features` | A product action has an independent boundary outside the page model; one-off forms, button groups, and handlers remain page-local.               |
| `widgets`  | A self-contained page region hides meaningful behavior behind its interface or is reused; callback-heavy visual wrappers alone are insufficient. |

When needed, layers MUST follow `pages` → `widgets` → `features` → `entities` → `shared`.

Slices in the same layer MUST NOT import one another.

External consumers MUST import a slice only through its `index.ts` public API.

### Library Browser owners

The Library Browser page is composed around the ownership boundaries already defined by Web Async Ownership:

#### Page controller

The **page controller** mounts the page, coordinates its internal owners, and
disposes them. It owns only cross-owner interaction readiness and sequencing;
it does not retain another owner's mutable state or reimplement that owner's
transitions and async policies.

#### Navigation owner

The **navigation owner** owns the page's browser address codec, history-entry metadata, and pending destination identity under [Browser Navigation](../library-browser/experience/design.md). It owns no source facts, HTTP, or mutation state. The controller coordinates its destination intents.

The page-local destination controller resolves destinations, missing targets, expired position lookups, restoration, and Album Resume through existing source, Photo, and navigation operations.

Each establishment has its own session identity. A newer destination invalidates an older continuation even when both use the same Browse Snapshot. Completed work remains fenced until superseded or disposed.

#### Application-lifetime owner

The **application-lifetime owner** owns Library Overview, publication status,
scan command settlement, application Recovery claims, and Summary
presentation state.

#### File Location owner

The **File Location owner** owns root binding, bounded direct-child windows,
retry ranges, and publication rebinding independently of source, Grid, and
Photo changes.

#### Source and Grid owner

The **source and Grid owner** owns the selected source, its view order and Selection State filter, and its per-state Selection counts. It also owns:

- Browse Snapshot lifecycle and bounded Browse Windows;
- visible-range admission, with one in-flight request and one completion notification per bounded window regardless of joined consumers;
- the retained-fact cache bound anchored to the latest visible range;
- Grid position, Thumbnail work, and source-scoped image transfers.

A page-local Browse window controller maps awaited outcomes and no-waiter notifications into Recovery and presentation. It replays captured exact-range retries. It owns no Snapshot, Browse token, fact cache, or retry records.

Awaited work settles through its caller; only unawaited work uses the owner notification.

#### Photo owner

The **Photo owner** owns the current Photo, foreground and adjacent Preview
work, Photo View navigation, Selection State and Rating writes, browser-local
undo, and Photo-scoped image transfers.

#### Photo details owner

The **Photo details owner** keeps current-Photo membership and capture-metadata reads fenced to their Photo authority. It presents Album membership independently of decision readiness and delegates every membership write to the Album action owner; the page controller coordinates its lifecycle without duplicating those read policies.

#### Album action owner

The **Album action owner** keeps admitted Album create, rename, delete, and
membership writes alive to settlement and applies their global latest-wins
presentation and shared-data convergence rules.

#### Saved-position coordinator

The **saved-position coordinator** serializes admitted Album-position writes
while validating both the captured Album source and current Photo owners. It
does not become a general persistence service.

#### Removal owner

The **removal owner** owns the reviewed removal. Its responsibilities include:

- the `Rejected` Browse Snapshot under review;
- the operation identity used by confirmation;
- the confirmed operation Undo can still restore; and
- admission of every removal and restoration write.

It never restores an operation or Photo the Library no longer holds. It holds no Photo facts of its own.

The owner also owns Trash inspection, explicit record-cleanup review, and receipt recovery. It fences reads by Trash generation and inspection scope. Every dismissal clears unsubmitted review state.

The owner retains an attempt identity before confirmation. It keeps admitted cleanup settling to its durable database result. Outcome recovery preserves retry access and converges rows, counts, and Undo without issuing another confirmation.

Trash never deletes Original Files; external deletion belongs to the Photographer under the [Trash contract](../photo-library/trash/spec.md).

#### Editor rendition owner

The **Editor rendition owner** owns independent current and baseline request admission, bounded polling, cancellation, retained images, and object-URL lifetime. Both lanes use one transfer lifecycle and the existing rendition validators.

The current lane follows recipe and edit-source identity. The baseline is retained across saved-settings changes for the same Photo, stage, and source revision.

Every asynchronous response or body continuation checks its lane generation and Photo scope before changing presentation or admitting another poll.

The Editor controller provides current session identity and chooses the presented lane. Its save stream waits on the Photo-scoped output controller's admission barrier before sending dependent writes.

#### Page UI

The **page UI** owns Library Browser markup, DOM bindings, semantic rendering, focus, keyboard, pointer, and responsive presentation. It reports user intent to the page model; it does not issue HTTP requests or decide async ownership.

The page-local Grid presenter owns thumbnail size, virtualized cells, merged render scheduling, keyboard focus, and captured Grid restoration. It reports visible ranges as intent; the source and Grid owner alone admits Browse work.

The Grid presenter reuses the DOM nodes of Photos that stay visible. It restores focus to one Tab stop without taking it from another surface. See [Library Browsing and Selection](../library-browser/spec.md).

The page-local Photo presenter owns presented Photo and Selection identities, status surface identity, facts and metadata rendering, and Preview image targets. It reads bounded presentation models without holding mutable Photo facts or issuing HTTP. Reset and disposal invalidate its targets and cancel gestures.

Modal arbitration, keyboard DOM facts, and responsive placement remain in the page UI composition. A pure UI-local keyboard policy chooses precedence and semantic actions.

Native range adjustment and modal cancellation remain with their surfaces. The Grid presenter owns delegated key geometry and prevent-default handling.

The page-local Photo gesture controller owns Preview pointer capture and the pending hold. It owns horizontal decision and vertical navigation accelerators, transient direction feedback, and the handoff to zoom panning or pinch.

The gesture controller reads the current Photo surface and decision readiness without owning Photo facts. It emits only semantic mutation or navigation intents.

Resetting a Photo, leaving its surface, or disposing the view cancels the hold and releases capture. A canceled or stale gesture never writes a decision or navigates.

Fit-state vertical movement that does not commit navigation remains native Photo View scrolling, including movement in letterbox regions. Manual zoom owns Preview dragging and never turns it into a decision or navigation action.

Rating controls own the transient Wheel presentation and candidate. The page UI also owns the Quick Action Dock, Photo tools, and disclosure focus. None of these values become Photo owner state.

#### Page API

The **page API** owns Library Browser HTTP calls, wire response types, and response decoding.

It accepts cancellation inputs from the calling owner but does not choose which operation supersedes another.

Browse, removal/restoration, and Trash share one HTTP/JSON classifier.

It distinguishes a failed transfer, an HTTP refusal, and unreadable JSON without trusting the decoded body.

Each client retains its own identity, count, and Snapshot validators and maps those classifications into its existing result types.

Thumbnail, Preview, and other APIs retain their distinct protocols.

These are responsibility boundaries, not a requirement for one file per owner.

An extracted module MUST hide meaningful state or policy behind a smaller interface.

Otherwise the responsibility stays with its nearest existing owner.

Sharing the two rendition lifecycles inside the page model hides their transport and image-resource rules without adding a second editing session.

Keeping two copies in the Editor controller duplicates cancellation and body-settlement policy.

A general poller spanning rendition, Export and Development Proxy work would erase their different effects and settlement contracts and is not used.

### Async ownership module

The four mechanisms defined by Web Async Ownership remain one cohesive, dependency-free module inside the Library Browser page: scoped tasks, settlement handles, Summary notice arbitration, and Recovery claims.

Splitting only its most reusable-looking classes would contradict that existing contract and create a second ownership boundary.

The module stays page-local: its Recovery types interpret source and Photo ownership, and it has no consumer outside the Library Browser.

Dependency-free implementation is valuable but is not, by itself, evidence of shared ownership.

Browse tokens, Preview state, mutation reconciliation, and the actual Recovery and Summary presentation state also remain page-owned.

### State and communication

Each mutable state value has one owner.

Another page-internal module interacts with that state through an explicit command, query, or result; it does not retain a writable reference to the owner's state.

The page controller MAY coordinate internal owners, but owners do not call each other through cyclic imports.

When coordination is necessary, the controller passes a narrow callback or consumes an operation result.

Interfaces use Library Browser language and avoid exposing DOM element collections, request sequence counters, or other owner internals.

The application-lifetime owner is the sole writer of the Library Overview data floor and committed Album summaries.

File Location conflicts, scan completion, Album mutation success, and saved-position confirmation return typed outcomes to that owner through the page controller; they do not mutate its state or call it through reverse imports.

Cross-owner connectivity and decision readiness are derived once from transport reachability, Recovery claims, and active interaction ownership.

The application-lifetime owner reports the status probe's transport outcome, and the page controller applies it to that shared reachability state; a probe answer without usable content reports neither a reachable nor a lost transport.

Local Preview inspection readiness is separate: an already loaded Preview can zoom and pan without transport or persistence readiness, while fit-mode decision gestures remain governed by decision readiness.

Individual owners cannot independently declare the page connected or clear another owner's busy state.

A presentation surface that MUST reject late settlement is represented by an opaque ownership token, not by an unguarded `setStatus` callback.

An operation's semantic owner is explicit at a module boundary.

Transport objects such as `AbortSignal` carry cancellation only and are not used as cross-module identity tags.

The source structure does not introduce a global frontend store.

Browser-local state stays with the lifetime that invalidates it, while server-authoritative state continues to converge through the existing API and ordering contracts.

### Preview presentation state

The page UI owns one transient Preview zoom state for the current Photo.

It is presentation state, not Photo state, and is reset when the current Photo or view changes.

The page UI keeps zoom state available to assistive technology. It exposes:

- Fit Window;
- zoom out and zoom in;
- a continuous slider;
- a live percentage; and
- a 100% action.

The state is one zoom percentage plus a Fit marker rather than a mode list:

| State             | Behavior                                                                                                                                                                                                                                              |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Fit               | Recompute from Preview area and natural image size. Horizontal movement owns decision accelerators; vertical movement owns navigation accelerators. Uncommitted vertical movement remains native Photo View scrolling.                                |
| Manual            | `[10%, 800%]` is anchored to natural Preview pixels; `100%` maps one Preview pixel to one CSS pixel. Pointer wheel and pinch keep the gesture point stationary. One-finger drag pans within bounded overflow without a decision or navigation action. |
| Detail inspection | Manual zoom rather than a separate mode; `D` toggles 200% detail zoom.                                                                                                                                                                                |

Fit gestures set `picked` to the right and `rejected` to the left. Up opens the next Photo and down opens the previous Photo; visible controls remain equivalent fallbacks.

This shape is selected because the Photographer MUST reach any magnification between complete composition and honest pixel inspection without a second control model.

The presentation MUST NOT use a `Fill` cropping mode because cropping hides composition rather than inspecting it.

Explicit Pick and Reject controls stay available in every zoom state.

### Mobile Photo View interaction state

The mobile Photo View adds a transient action hierarchy without adding a new frontend lifetime.

The page UI owns the Quick Action Dock, the Photo tools surface, and the Rating Wheel's pointer state.

The Photo owner remains the sole owner of the current Photo, Rating and Selection State persistence, Undo, async authority, and settlement classification.

The transient gesture state has three meaningful values:

- no pending gesture;
- a pending touch hold bound to one current Photo and its Preview surface; and
- an open Wheel with one presentation-only Rating candidate.

The pending hold uses the product contract's 12 CSS-pixel movement boundary and 450-millisecond hold threshold.

Before the threshold, the page UI hands the pointer to horizontal decision movement, vertical navigation or native Photo View scrolling according to the touch region and dominant axis, or Pinch Zoom when those gestures take ownership.

After the threshold, the Wheel owns the pointer until release or cancellation.

The state is invalidated when the Photo surface, current Photo, connection readiness, or UI generation changes.

A Wheel release emits the same Rating intent as an explicit Rating control.

It does not issue HTTP itself, retain a second Rating value, advance the Photo, or change Selection State.

A late settlement is accepted only by the Photo owner and the current Photo presentation token, exactly like an explicit Rating mutation.

The Quick Action Dock is a responsive presentation of existing intents.

It MUST NOT become a second page controller or a second persistence admission.

The Photo tools surface presents existing Details, Album, Zoom, and Clear controls; each retains its existing owner, failure isolation, and focus restoration rules.

Undo is presented once in the primary Photo action region and retains the same Photo action owner.

Previous/Next remain in the primary navigation group. [Browser Navigation and Responsive Surfaces](../library-browser/experience/design.md#responsive-presentation-boundary) owns shared modal and disclosure lifecycle.

### Membership presentation state

The Photo details owner presents the current Photo's Album membership from a per-Photo server query, with independent loading, empty, and failed states.

Its read is fenced to the current Photo; a late response for another Photo is discarded.

Membership read failure does not affect decision readiness.

Membership mutations run through the Album action owner's admitted-write contract.

### Grid presentation state

The page UI owns the chosen Grid thumbnail step as presentation state of the open Grid. One layout calculation uses available Grid width, step target width, gap, current font metrics, text scale, and selection gutter for both CSS and virtualization geometry.

The selected step determines column count; remaining row width MUST be distributed evenly across those columns without changing the chosen step on resize. An incomplete row MUST use the same widths. This fills available width without changing density or cropping.

Column count, actual cell width, caption height, row and column pitch, positions, canvas height, visible ranges, keyboard movement, and restoration MUST use that calculation. Caption height MUST fit required state and Rating presentations and apply uniformly to every cell and placeholder.

Caption height MUST NOT depend on each Photo's text, orientation, availability, or image completion. Width, text scale, font metric, or selection-gutter changes MUST recompute geometry and preserve the top Photo anchor and applicable focus. Content updates MUST NOT create variable-height rows or make a loaded image shift the Grid.

A size change re-anchors the Grid on the topmost visible row, re-renders through the same merged render as scrolling, and reports the new range through the normal admission path.

It is not a source open: it changes no wire contract, keeps the current Browse Snapshot, and leaves Selection State, Rating, and Album state untouched.

Thumbnail requests keep the single `thumbnail-512` derivative at every step.

Actual cell width and device pixel ratio MAY require display upscaling; that does not establish extra source detail. The Grid MUST NOT request per-step renditions or upscale stored derivatives to fill a row. Per-step renditions would fragment the rebuildable cache without changing the composition contract.

Preview scheduling priority in [Scalable Library Browsing](../library-browser/design.md) is unchanged.

### Destination and restoration policy

One page-local model module answers how a browser address relates to the live source.

It owns no state and issues no HTTP: it reads the open source's shape, the open Snapshot's readiness, and the File Location binding through narrow queries, and the page controller remains the only coordinator that commits what it answers.

The module decides:

- the destination the open Snapshot presents;
- the wire order an address requests;
- whether the open Snapshot can serve a traversal without reopening the source; and
- the display name of a Folder Location whose parent window can be unloaded.

An omitted order and an Album's own order both leave the wire order to the server.

The same module resolves captured Grid restoration:

| Condition                           | Result                                                                             |
| ----------------------------------- | ---------------------------------------------------------------------------------- |
| Anchor is available                 | Confirm its stable Photo identity before trusting its index hint.                  |
| Snapshot no longer holds the anchor | Clamp to the captured position and restore geometry without naming a focused cell. |
| Lookup is retryable                 | Keep the resolved position instead of presenting the Photo as gone.                |

### Source opening settlement

One page-local model boundary classifies source-open results for both a new destination and an expired Snapshot reopen.

It binds a Folder request to the current File Location publication, captures the source authority established by the request, and admits the returned Grid position only for that authority.

A publication conflict MAY rebind File Locations only while the source remains current; after rebinding, both source and File Location authorities MUST still match before the page receives a publication notice.

Only a confirmed `404` identifies a missing source.

Other failures remain retryable.

The page coordinates Photo rebinding, the first bounded window, Recovery, presentation, and history.

It checks authority again when consuming an awaited result, because another destination can supersede it between continuations.

The source and Grid owner remains the only owner of Snapshot state and release.

Source replacement and retained-source reopening keep separate orchestration.

Replacement binds Photo to the new source before opening and commits readiness before rendering.

Reopening first detaches Photo from pending work under the old source, retains Grid cells, renders the covering window, then commits readiness and resumes Photo.

Page-local helpers share the Folder publication precondition, covering-window fence, readiness commit, failure mapping and empty-source presentation without owning another source lifecycle.

Reopening returns established authority, confirmed missing, retryable failure or superseded explicitly.

Destination resolution retries only the established authority, preserves a failed destination for Retry, and explains a missing source before falling back.

Every awaited continuation and busy release checks the captured source and destination; disposed work cannot change presentation.

### Styling

Global tokens, reset rules, and the application mount surface belong to `app`.

Library Browser layout and component styles remain within the page slice.

CSS is split by ownership only when doing so creates a navigable boundary. The selected appearance remains neutral and dark.

## Semantics

Application startup mounts exactly one Library Browser into the configured root.

Disposal is idempotent.

It detaches presentation continuations, stops owned application and page scopes, removes browser-managed image transfers, and starts or preserves Browse-token cleanup and admitted-write settlement exactly as required by Web Async Ownership.

Disposal MUST perform each cleanup at most once and start at most one release for a known Browse token.

Composite responses used only by the Library Browser stay in its `api` segment.

A wire type belongs in an entity only when that slice has an independent consumer and exposes a stable domain-facing contract rather than the server response wholesale.

Architecture transitions MUST update all consumers and remove obsolete modules, import paths, and re-exports.
