# Web Frontend Architecture

Slipstream's Web application is one Library Browser, but that page owns several
different lifetimes: application status, File Location navigation, an open
source and its bounded Grid, the current Photo, and admitted persistence. Those
lifetimes must remain independently understandable as the product grows.

The frontend uses Feature-Sliced Design (FSD) as a dependency and ownership
model. It does not materialize every conventional FSD layer. A layer or slice
exists only when current behavior gives it a durable responsibility.

This design changes source ownership only. User-visible Library Browser
behavior remains defined by
[Library Browsing and Selection](../docs/library-browsing-and-selection.md),
and asynchronous cancellation, settlement, convergence, recovery, and failure
presentation remain defined by [Web Async Ownership](web-async-ownership.md).

## Design Drivers

- One page currently coordinates application-, File Location-, source-, Grid-,
  Photo-, and persistence-owned work. A change in one lifetime must not require
  understanding every other lifetime.
- Source structure must use the product language in [`CONTEXT.md`](../CONTEXT.md)
  so that a maintainer can find Library Browser behavior without knowing an
  implementation taxonomy.
- FSD dependency direction and public APIs must prevent extracted code from
  growing new implicit coupling.
- The application remains a dependency-light Vanilla TypeScript and Vite
  client. A source reorganization does not justify a UI framework, router,
  state container, query cache, or dependency-injection system.
- Original Files remain read-only, and the browser protocol, Browse Snapshot
  coherence, admitted-write semantics, and deployment surface remain unchanged.
- Migration must proceed in independently reviewable, working increments. A
  directory end state is not sufficient evidence of correct behavior.

## Model

### Layers

The initial architecture uses only these layers, from highest to lowest:

```text
app
pages
```

Dependencies point downward. `app` may import only the public API of a page.
Code inside one slice uses its internal relative imports instead of importing
its own public API.

The initial `pages` layer contains one `library-browser` slice. That slice has
one public API: mounting the Library Browser into a supplied root and returning
an idempotent disposal handle. Application startup, page-hide handling, and
global styling belong to `app`; Library Browser state and behavior do not.

`shared`, `entities`, `features`, and `widgets` are intentionally absent
initially:

- A shared library is introduced only for a domain-neutral mechanism with a
  current consumer outside the Library Browser. Being dependency-free alone
  does not require promotion from its product owner.

- A Photo, Album, or Original Folder slice is introduced only when it owns
  behavior or a stable contract consumed outside the Library Browser page.
  Moving page-local wire types into type-only entity folders is not sufficient.
- A feature slice is introduced only for a product action with an independent
  boundary outside the page model. A one-off form, button group, or event
  handler remains page-local.
- A widget slice is introduced only for a self-contained page region whose
  interface hides meaningful behavior or which is reused. Splitting the one
  Library Browser into callback-heavy visual wrappers is not a widget boundary.

If one of these layers becomes necessary, it is inserted in normal FSD order:
`pages` -> `widgets` -> `features` -> `entities` -> `shared`. Slices in the
same layer do not import one another. External consumers import a slice only
through its `index.ts` public API.

### Library Browser owners

The Library Browser page is composed around the ownership boundaries already
defined by Web Async Ownership:

- The **page controller** mounts the page, coordinates its internal owners, and
  disposes them. It owns only cross-owner interaction readiness and sequencing;
  it does not retain another owner's mutable state or reimplement that owner's
  transitions and async policies.
- The **navigation owner** owns the page's browser address codec, history-entry
  metadata, and pending destination identity under
  [Browser Navigation](browser-navigation.md). It owns no source facts, HTTP,
  or mutation state; the controller coordinates its destination intents.
- The **application-lifetime owner** owns Library Overview, publication status,
  scan command settlement, application Recovery claims, and Summary
  presentation state.
- The **File Location owner** owns root binding, bounded direct-child windows,
  retry ranges, and publication rebinding independently of source, Grid, and
  Photo changes.
- The **source and Grid owner** owns the selected Library Browser source, the
  source's view order and Selection State filter, per-state Selection counts
  for the open source, Browse Snapshot lifecycle, bounded Browse Windows, the
  visible-range admission contract (one in-flight request and one completion
  notification per bounded window regardless of joined consumers), the
  retained-fact cache bound anchored to the latest visible range, Grid
  position, Thumbnail work, and source-scoped image transfers.
- The **Photo owner** owns the current Photo, foreground and adjacent Preview
  work, Photo View navigation, Selection State and Rating writes, browser-local
  undo, and Photo-scoped image transfers.
- The **Album action owner** keeps admitted Album create, rename, delete, and
  membership writes alive to settlement and applies their global latest-wins
  presentation and shared-data convergence rules.
- The **saved-position coordinator** serializes admitted Album-position writes
  while validating both the captured Album source and current Photo owners. It
  does not become a general persistence service.
- The **removal owner** owns the reviewed removal: the `Rejected` Browse
  Snapshot under review, the operation identity its confirmation uses, the
  confirmed operation Undo can still restore, and the admission of every
  removal and restoration write. It never restores an operation or a Photo the
  Library no longer holds, and it holds no Photo facts of its own.
- The **page UI** owns Library Browser markup, DOM bindings, semantic rendering,
  focus, keyboard, pointer, and responsive presentation. It reports user intent
  to the page model; it does not issue HTTP requests or decide async ownership.
  Grid rendering is presentational: it never initiates loading, merges to at
  most one update per animation frame, and reuses the DOM nodes of Photos that
  stay visible. Grid keyboard focus is presentation state: one rendered cell
  holds the Tab stop, arrow keys move cell focus and report the target row's
  range through the same merged render, and the page UI owns the focus
  restoration required by
  [Library Browsing and Selection](../docs/library-browsing-and-selection.md).
  It yields Fit-state vertical touch panning to native Photo View
  scrolling, retains Fit-state horizontal decision gestures, and takes full
  Preview drag ownership whenever the zoom state is manual. The page UI also
  owns the transient mobile Quick Action Dock, Photo tools surface, pending
  Rating Wheel gesture, Wheel candidate, and disclosure focus. These
  values are presentation state and do not become Photo owner state.
- The **page API** owns Library Browser HTTP calls, wire response types, and
  response decoding. It accepts cancellation inputs from the calling owner but
  does not choose which operation supersedes another.

These are responsibility boundaries, not a requirement for one file per bullet.
An extracted module must hide meaningful state or policy behind a smaller
interface. Otherwise the responsibility stays with its nearest existing owner.

### Async ownership module

The four mechanisms defined by Web Async Ownership remain one cohesive,
dependency-free module inside the Library Browser page: scoped tasks,
settlement handles, Summary notice arbitration, and Recovery claims. Splitting
only its most reusable-looking classes would contradict that existing contract
and create a second ownership boundary.

The module is not promoted to `shared`: its Recovery types currently interpret
source and Photo ownership, and it has no consumer outside the Library Browser.
Dependency-free implementation is valuable but is not, by itself, evidence of
shared ownership. Browse tokens, Preview state, mutation reconciliation, and
the actual Recovery and Summary presentation state also remain page-owned.

### State and communication

Each mutable state value has one owner. Another page-internal module interacts
with that state through an explicit command, query, or result; it does not
retain a writable reference to the owner's state.

The page controller may coordinate internal owners, but owners do not call each
other through cyclic imports. When coordination is necessary, the controller
passes a narrow callback or consumes an operation result. Interfaces use
Library Browser language and avoid exposing DOM element collections, request
sequence counters, or other owner internals.

The application-lifetime owner is the sole writer of the Library Overview data
floor and committed Album summaries. File Location conflicts, scan completion,
Album mutation success, and saved-position confirmation return typed outcomes
to that owner through the page controller; they do not mutate its state or call
it through reverse imports.

Cross-owner connectivity and decision readiness are derived once from transport
reachability, Recovery claims, and active interaction ownership. The
application-lifetime owner reports the status probe's transport outcome, and the
page controller applies it to that shared reachability state; a probe answer
without usable content reports neither a reachable nor a lost transport. Local
Preview inspection readiness is separate: an already loaded Preview may zoom and
pan without transport or persistence readiness, while fit-mode decision gestures
remain governed by decision readiness. Individual owners cannot independently
declare the page connected or clear another owner's busy state. A presentation
surface that must reject late settlement is represented by an opaque ownership
token, not by an unguarded `setStatus` callback.

An operation's semantic owner is explicit at a module boundary. Transport
objects such as `AbortSignal` carry cancellation only and are not used as
cross-module identity tags.

The source structure does not introduce a global frontend store. Browser-local
state stays with the lifetime that invalidates it, while server-authoritative
state continues to converge through the existing API and ordering contracts.

### Preview presentation state

The page UI owns one transient Preview zoom state for the current Photo. It
is presentation state, not Photo state, and is reset when the current Photo
or view changes. The page UI exposes the state through Fit Window, zoom out,
a continuous slider, zoom in, a live percentage, and a 100% action, and
keeps the state available to assistive technology.

The state is one zoom percentage plus a Fit marker rather than a mode list:

- `Fit` recomputes from the Preview area and the image's natural size, owns
  the existing horizontal decision and vertical scrolling gestures, and
  reports the percentage it produces.
- A manual zoom percentage in `[10%, 800%]` is anchored to the Preview's
  natural pixels: `100%` maps one Preview pixel to one CSS pixel. Pointer
  wheel and pinch zoom keep the gesture point stationary; one-finger drag
  pans within bounded overflow and never records a decision.
- Detail inspection is the manual zoom state rather than a separate mode,
  so `D` toggles a 200% detail zoom inside the same continuous state.

This shape is selected because the Photographer must reach any
magnification between complete composition and honest pixel inspection
without a second control model. The retired `Fill` cropping mode is not
reintroduced: cropping hid composition rather than inspecting it. Explicit
Select and Reject controls stay available in every zoom state.

### Mobile Photo View interaction state

The mobile Photo View adds a transient action hierarchy without adding a new
frontend lifetime. The page UI owns the Quick Action Dock, the Photo tools
surface, and the Rating Wheel's pointer state. The Photo owner remains
the sole owner of the current Photo, Rating and Selection State persistence,
Undo, async authority, and settlement classification.

The transient gesture state has three meaningful values:

- no pending gesture;
- a pending touch hold bound to one current Photo and its Preview surface; and
- an open Wheel with one presentation-only Rating candidate.

The pending hold uses the product contract's 12 CSS-pixel movement boundary and
450-millisecond hold threshold. Before the threshold, the page UI hands the
pointer to horizontal decision movement, native vertical scrolling, or Pinch
Zoom when those gestures take ownership. After the threshold, the Wheel owns
the pointer until release or cancellation. The state is invalidated when the
Photo surface, current Photo, connection readiness, or UI generation changes.

A Wheel release emits the same Rating intent as an explicit Rating control. It
does not issue HTTP itself, retain a second Rating value, advance the Photo, or
change Selection State. A late settlement is accepted only by the Photo owner
and the current Photo presentation token, exactly like an explicit Rating
mutation.

The Quick Action Dock is a responsive presentation of existing intents. It
must not become a second page controller or a second persistence admission. The
Photo tools surface presents existing Details, Album, Zoom, Clear, and Undo
controls; each retains its existing owner, failure isolation, and focus
restoration rules. Previous/Next remain in the primary navigation group.
[Browser Navigation and Responsive Surfaces](browser-navigation.md#responsive-presentation-boundary)
owns shared modal and disclosure lifecycle.

### Membership presentation state

The page UI also presents the current Photo's Album membership from a
per-Photo server query, with loading, empty, and failed members of its own.
The page controller owns that read like capture metadata: it is fenced to
the current Photo, a late response for another Photo is discarded, and
failure does not affect decision readiness. Membership mutations continue
to run through the Album action owner's admitted-write contract.

### Grid presentation state

The page UI owns the Grid's thumbnail size as presentation state of the open
Grid. One step set in the page UI is the single source for both the CSS cell
box and the virtualized layout: column count, row and column pitch, cell
positions, scroll anchors, canvas height, and keyboard row movement all
derive from the chosen step, so a size change cannot leave the layout and its
cells disagreeing about geometry.

A size change re-anchors the Grid on the topmost visible row, re-renders
through the same merged render as scrolling, and reports the new range
through the normal admission path. It is not a source open: it changes no
wire contract, keeps the current Browse Snapshot, and leaves Selection State,
Rating, and Album state untouched.

Thumbnail requests keep the single `thumbnail-512` derivative at every step.
The largest step still displays within that derivative at 1× and 2× device
pixel ratios; above 2× the Grid scales the derivative up inside the cell
instead of requesting another rendition: per-step renditions would fragment
the rebuildable cache and add derivative work for no visible gain. The Preview
scheduling priority
in [Scalable Library Browsing](library-browsing.md) is unchanged.

### Styling

Global tokens, reset rules, and the application mount surface belong to `app`.
Library Browser layout and component styles remain within the page slice. CSS
is split by ownership only when doing so creates a navigable boundary; the
migration does not create one stylesheet per element and does not change the
selected neutral dark appearance.

## Semantics

Application startup mounts exactly one Library Browser into the configured root.
Disposal is idempotent. It detaches presentation continuations, stops owned
application and page scopes, removes browser-managed image transfers, and
starts or preserves Browse-token cleanup and admitted-write settlement exactly
as required by Web Async Ownership.

Moving an operation between source modules never changes its ownership key,
generation, transport policy, settlement rule, ordering rule, Recovery claim,
or presentation surface. If an extraction reveals that one of those contracts
must change, that behavior change stops and receives its own governing design
and review instead of entering the architecture migration.

Wire routes, request and response values, persistence, server boundaries, and
deployment verification remain unchanged. Composite responses used only by the
Library Browser stay in its `api` segment. A wire type moves to an entity only
when an entity slice already has an independent consumer and can expose a
stable domain-facing contract rather than the server response wholesale.

Architecture transitions do not preserve obsolete import paths. A migration
increment first changes all consumers, verifies them, and then removes the old
module or re-export in the same increment.

## Options

### Selected: Incremental page-first FSD

Establish the application/page boundary, keep the cohesive async ownership
module page-local, and then move one async owner at a time behind a deep
page-internal interface. Every increment keeps a working application and the
existing behavioral gates.

This approach directly reduces change amplification while keeping temporary
and permanent abstractions to a minimum.

### Rejected: Full textbook FSD tree in one migration

Creating `widgets`, `features`, and `entities` immediately would classify the
current page without proving independent ownership. It would also combine many
file moves with the highest-risk async boundaries and make regressions harder
to localize.

### Rejected: Technical `components`, `services`, and `stores` split

That structure separates file types but leaves Library Browser ownership and
lifetimes implicit. It tends to move domain state into a shared store and HTTP
policy into broad services, so a maintainer still needs cross-directory context
for one product change.

### Rejected: Framework or state-management migration

A framework could change rendering mechanics but would not define Browse,
Photo, File Location, or persistence ownership. It adds a second migration,
new runtime concepts, and new failure modes without solving the current
boundary problem.

### Mobile Photo View interaction state options

#### Selected: page-local transient interaction state

Keep the Wheel, Dock, and disclosure state in the existing Library Browser page
UI, and send only semantic Rating, Selection State, navigation, and Album
intents to the page controller. This fits the current ownership model because
only Photo View consumes the state, the state is invalidated by the current
Photo surface, and no external consumer needs the interaction protocol.

#### Rejected: move Wheel state into the Photo owner

The Photo owner owns durable Photo mutations and async authority, not pointer
geometry or responsive presentation. Moving a pending hold or candidate into it
would make a transient gesture survive the wrong UI lifetime and would couple
Photo persistence to DOM timing.

#### Rejected: create a reusable gesture feature or mobile state store

The Rating Wheel and Dock have one current consumer and depend on Preview zoom,
Photo identity, focus, and existing page gestures. A new feature slice or
frontend store would add an ownership boundary without an independent contract
or second consumer. Reuse can be considered only when another current product
surface has the same complete interaction semantics.

#### Rejected: replace explicit Rating controls with the Wheel

A Wheel is efficient for touch but is a poor keyboard and assistive-technology
surface. Keeping explicit controls as the fallback preserves discoverability,
programmatic state, and non-gesture operation while the Wheel remains an
accelerator.

## Risks and Trade-offs

- A page slice may remain large during migration. That is preferable to
  inventing shallow layers; the next extraction occurs only when one owner can
  present a smaller interface than the state and policy it hides.
- The page controller could become a renamed monolith. It may coordinate owner
  results, but it does not retain their mutable state, request bookkeeping, or
  error policy.
- File movement can conceal behavioral edits. Each increment isolates one
  owner, keeps protocol and presentation behavior unchanged, and is reviewed
  against the async ownership matrix before merge.
- Layer direction and page public-API rules are enforced with existing tooling
  as soon as the skeleton exists. The final audit adds owner-internal cycle and
  API-boundary checks; another dependency is justified only by a concrete rule
  the existing toolchain cannot express.
- Omitting conventional FSD layers makes the tree less symmetrical. This is an
  intentional trade-off: fewer concepts are easier to navigate than empty or
  type-only slices, and a missing layer can be added when an owner appears.

## Migration

1. Establish an `app` entrypoint and a `pages/library-browser` public mounting
   API while keeping the current page implementation intact. Add the minimum
   existing-ESLint rules that prevent higher-layer imports and public-API
   bypasses in the new structure.
2. Move the existing four-mechanism ownership module and its focused tests
   intact into the Library Browser page model. Update the unit-test command so
   all seventeen ownership characterization cases remain discovered.
3. Extract the independent File Location owner, preserving root binding,
   publication coherence, bounded windows, and exact-range retry behavior.
4. Extract the application-lifetime owner, preserving Library Overview,
   publication status, scan settlement, Recovery, and Summary semantics.
5. Extract the source/Grid owner, including Browse Snapshot release, bounded
   windows, Thumbnail work, and browser-managed Grid image transfers.
6. Extract the Album action owner without changing its admitted-write,
   latest-wins notice, connectivity, or overview-convergence behavior.
7. Extract the Photo owner without changing current Preview, navigation,
   Selection State, Rating, undo, or image-transfer behavior.
8. Extract saved-position coordination without changing its source-and-Photo
   ownership, serialization, Recovery, or stale-settlement behavior.
9. Move remaining page markup, DOM binding, and styles behind page-internal UI
   interfaces; remove the obsolete monolithic entrypoint.
10. Audit the resulting downward import direction and public APIs with the
    existing lint toolchain. Add a dedicated architecture dependency only if
    the existing tooling cannot express a concrete rule that has already been
    violated.

Each owner extraction is a vertical, behavior-preserving integration increment:
it moves that owner's model, API adapter, and smallest necessary UI adapter
together. The shared source-navigation UI remains page-owned until a later
owner can consume a semantic interface without element bags or callback soup.
Each increment updates tests only where ownership or import paths move, runs
focused checks and the full repository gate, and is independently revertible.
There is no data, protocol, or deployment migration.

## Verification

Verification must prove:

- application startup imports only the Library Browser public API and contains
  no Library Browser state or HTTP behavior;
- no `shared`, entity, feature, or widget layer exists without a demonstrated
  current owner or consumer;
- page-internal owners have no cyclic imports or shared writable state;
- disposing one mounted page twice is harmless, performs each cleanup at most
  once, suppresses late presentation, and starts at most one release for a
  known Browse token;
- every operation named by Web Async Ownership retains its scope, key,
  cancellation or settlement policy, ordering, Recovery, and failure behavior
  as a characterized rule of its model owner, proven by the page-model unit
  tests rather than by a browser scenario;
- the browser smoke suite proves the assembled real stack end to end against
  the Rust server and the built Web assets: the access boundary, startup scan
  and Grid rendering, Photo View, decisions persisted through the real write
  path, Album management, a responsive surface, the CLI-to-Web flow, and the
  opt-in real-camera scenario. The suite stays smoke-sized and runs fully
  parallel; it is not the home of behavior or race regression, and
  [`CONTRIBUTING.md`](../CONTRIBUTING.md#browser-suite-policy) owns that rule;
- presentational and browser-real rules keep required coverage wherever they
  live. A behavior domain leaves the browser suite only when it keeps
  required coverage with an explicit owner and destination: a page-model unit
  assertion, a dedicated slower suite, or the open Issue that owns the work
  until one of those exists. Until
  issue [#410](https://github.com/suraciii/slipstream/issues/410) closes, the
  Zoom, Dock, and Rating Wheel interaction rules, pointer and gesture state,
  Grid geometry and focus movement, filmstrip presentation, viewport and
  200%-text space budgets, browser history/bfcache/pagehide semantics,
  40,000-Photo in-browser boundedness, and the rendered-DOM halves of the
  recovery and windowing families have no executing coverage; they remain
  required coverage, and #410 owns restoring each of them;
- mobile Photo View checks prove Quick Action Dock and Photo tools focus,
  safe-area, and ownership behavior, and Rating Wheel checks prove the
  450-millisecond hold, 12 CSS-pixel handoff, release-only mutation, explicit
  fallback, cancellation, and stale-settlement rules — as unit tests of the
  extracted interaction module once it exists, and through issue
  [#410](https://github.com/suraciii/slipstream/issues/410)'s dedicated
  slower suite until then;
- the complete repository verification gate passes with nonzero test discovery;
  and
- an independent read-only review checks FSD placement, dependency direction,
  public APIs, obsolete paths, and accidental behavioral changes before merge.
