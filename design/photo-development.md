# Photo Development Architecture

Photo development introduces expensive native processing and durable editing
intent into a Library Browser that must remain responsive. The service must
coordinate these operations without giving image engines ownership of Original
Files, Photo identity, or client state.

[Photo Development](../docs/photo-development.md) owns product behavior.
[Processing Modules](processing-modules.md) owns single-module invocation,
discovery, composition, and Preview/Export identity.
[Development Color Pipeline](development-color.md) owns the qualified
concrete image contracts of module outputs and their display derivatives.
Existing Library confinement, camera Preview, and selection contracts
remain authoritative for their boundaries.

## Model and Ownership

The Photo owns one current Edit Recipe containing zero or more Processing Step
records. Each record has a stable opaque `step_id`, one selected module, one
identified input binding, and one complete parameter snapshot under the
[module contract](processing-modules.md#model-and-ownership). A repeated module
uses a different step identity. A persisted recipe, including an empty recipe,
is saved editing intent; absence of a recipe is the unsaved state. Opening an
admitted RAW development step uses the processing baseline and as-shot white
balance. Saving baseline settings still creates intent, and reset does not
delete a step. Recipe revisions remain independent of Rating and Selection
State. The active browser step is a selection over this collection, not a hidden
pipeline position.

The Agent-facing stateful API calls this current projection `Edit State`: the
selected step, input binding, Engine Modules, qualified control values, and
revision. `Edit Recipe` remains the complete saved snapshot and advanced
composition representation. Stateful `set` and `reset` mutations are guarded
compare-and-set writes over that same durable recipe; they do not introduce a
Session, Workflow, Preset, or engine-history store.

A step's input binding is one of three guarded kinds: the Photo's Original
File under its guarded source revision, one retained immutable Processing
Artifact under its captured image contract, or — as a bounded preview
source only — the Photo's [Development Proxy](#development-proxy). The
kind fixes admission: an Original-backed Export requires the Original; an
artifact-backed step continues while the Original is unavailable, within
that artifact's lease, expiry, and module compatibility; a proxy never
admits an Export. Upstream edits never retarget an artifact or proxy input.

Exposure and white balance are semantic controls of the admitted darktable
configuration. Custom temperature and tint ranges and engine mapping must be
qualified before execution. A Film Recipe is one concrete standalone
SpektraFilm configuration, not a mandatory suffix of every Edit Recipe.
Development Result and Film Result name module-specific concrete results; an
Edit Preview names the current step's bounded result, not an implicit pair of
stages.

An Export records an immutable input/module/parameter/bundle snapshot, output
contract, request identity, state, and artifact reference. Retaining that snapshot
is not a user-visible edit-history feature. Only current recipes, retained
artifacts, and active work require snapshots. Selecting an Export artifact as a
later step's input is explicit; changing upstream intent does not retarget it.

Rust and SQLite own recipes, source bindings, Export state, queue admission,
concurrency, and artifact publication. Browser state owns pending drafts,
session undo/redo, and view interaction. Engine-private history and Python
objects remain behind processing adapters.

The service's Export manager owns admission of its workloads onto the
shared PhotoExecutor. It supplies the captured workload, source, and recipe
identity, starts the local engine attempt, and validates the returned artifact
identity. Callers retain validation, task-failure handling, cancellation,
discard, and publication order. Sharing the executor does not merge ephemeral
preview and durable Export policy.

The private Preview executor owns ephemeral attempt identity, cancellation,
abandonment, discard and temporary staging. It uses the Export manager's single
heavy-work admission and confined workspace through narrow resource operations.
The Preview registry owns rendition admission, retained render deadlines and
sweeping; the executor creates no durable Export row or publication claim.
Export publication, restart adoption and download leases remain with the
durable manager. Initial and post-derivation Preview identity use one construction
rule; post-derivation publication still rereads current Library facts.

## Application Boundary

The existing Rust modular monolith owns development lifecycle inside its
one application container. Each selected module runs through the in-process
PhotoExecutor under [Local Photo Executor](processing-executor.md). [Native
darktable Integration](darktable-integration.md) owns execution through a
fresh private MCP child. Standalone SpektraFilm uses a narrow Python entry point
calling its pinned runtime. Neither peer invokes another module or publishes a
Processing Artifact. Engines must not expose an independent public API or start
the GUI.

Processing remains opt-in. Discovery reports availability and refusal reasons
per module, separately from input support, resource admission, and Library
readiness. Missing module availability must not prevent browsing, inspection of
saved intent, or work with another independently available module.

The runtime must pin Python, engine builds, adapter schemas, profiles and native
IO dependencies. Importing the package must not be equated with successful
headless processing. Qualification must run without display variables and GPU
device access. Installation of GUI dependencies is not proof that a display is
required, nor proof that every native import is headless-safe.

The first execution model starts a fresh engine child for each serialized
request, which gives a direct state-lifetime proof. Reusing one long-lived
engine child is optional follow-up work and requires an executable test
proving image, history, and module state cannot leak between requests. It must
not silently become an additional service-owned Library.

## Original Access

All inputs must be resolved by Photo identity through the existing validated
Library read boundary. An API must not accept an arbitrary server path.

For path-oriented engines, create an immutable staging copy from the validated
Original descriptor in application-owned storage. Capture the source revision,
copy and hash its bytes, and verify source stability before admitting the staged
input. A revision change must reject the operation. Do not reopen a pathname
that can escape confinement between validation and use.

The processing child must receive read-only staged input and writable private
work/configuration/output directories. It must not have a writable Library bind,
access to a photographer's desktop configuration, or an ambient Sidecar
association. Staging by hard link is insufficient when it allows source mutation
to change the admitted bytes. A descriptor-based alternative requires explicit
engine compatibility and confinement proof before replacing the copy boundary.

Argument vectors must avoid shell interpretation. Engine-private editing state
must follow [Native darktable Integration](darktable-integration.md); it must
never reuse external Rating XMP or overwrite a photographer-owned file. Child
output paths and manifests must be validated before the service accepts them.

## Development Proxy

The service may retain one bounded, scene-linear Development Proxy per Photo
outside the Library Folder. A proxy is published only after the Original has
been resolved through the confined Library boundary, copied and byte-hashed,
processed with the baseline recipe (zero exposure and as-shot white balance),
and the source revision has been checked again immediately before atomic
publication. Its identity includes the Photo, source revision, staged-byte
hash,
approved profile, pipeline, geometry, and processing bundle.

The proxy row is committed after its complete artifact is durably installed.
Replacement publishes a new identity before removing the prior artifact; a
failed replacement therefore leaves the prior valid proxy available. Startup
reconciliation validates recorded artifacts and removes incomplete rows and
unclaimed files. Proxy-backed editing derives the current step's bounded
Preview from the scene-linear artifact by applying the saved numeric
exposure and display conversion locally. A module step that must re-execute
its engine against the proxy uses a qualified proxy workload whose
zero-exposure input cannot double-apply the captured parameters, and a
module that cannot produce its Preview from the proxy refuses rather than
fall back to another input or module.

The proxy is a preview source only. It must not satisfy an Original-required
Export or permit an arbitrary filesystem path into the engine. The service
reports proxy provenance separately from source support and retains guarded
recipe revisions against the proxy's recorded source revision while the
Original is unavailable.

## Recipe Writes and Autosave

A save command must include semantic settings, an expected Edit Recipe revision,
the expected Library source revision, and a stable request identity. Validation
must compare both the recipe and source preconditions. A changed published
source revision must refuse an old draft even when the recipe revision itself
has not changed. Validation and compare-and-set persistence must be atomic. The
result must identify the committed revision or an explicit conflict, refusal,
or unknown outcome. Identical retries with the same request identity must
resolve to the recorded operation; a different payload under that identity
must be rejected.

The Library source revision is an opaque token from the current Library
observation. It guards state writes against a changed published source; it is
not proof of exact byte identity. Processing admission must resolve the Photo
through the confined descriptor boundary, copy and hash the input, and verify
source stability before use. A processing operation must refuse an input whose
current Library source revision no longer matches the recipe binding. Source
byte verification remains required even when the saved revision matches.

Each browser Photo owner must serialize writes. It may retain one in-flight
save and coalesce later editing actions into the latest pending recipe. An
acknowledgement updates the confirmed baseline only for its own operation. It
must not erase later pending intent or mark it saved. An unrelated Photo's
navigation must not cancel or rebind this ownership.

Attempt to persist a bounded local pending draft before sending a browser save.
Draft identity includes the service/Library, Photo, source binding, observed
recipe revision and request identity. If local storage is blocked or exhausted,
retain an in-memory draft under the same Photo owner, disclose that reload or
browser closure can lose it, and continue guarded online saving. Do not block a
healthy service save solely because local recovery storage is unavailable.
Do not evict an unconfirmed draft to make room for another. Remove a draft only
after matching confirmation or explicit discard. An expired receipt or changed
source/revision requires reconciliation, not silent replay.

A request that may have reached the service but lost its response must be
resolved through its receipt or current state before further writes advance.
Reading matching settings alone must not authorize an unrelated overwrite.
CLI commands retain explicit mutation and uncertainty semantics; autosave is
browser orchestration of the same service operation.

Stored intent is never rewritten to match execution availability. A saved
setting the current module qualification does not admit stays readable as
retained intent and reports processing unavailable for that Photo; the
service must not rewrite it or silently substitute baseline execution. A
save carrying such a setting is committed like any other save.

Save receipts are public and bounded. A request identity is 1 to 128 characters
of ASCII letters, digits, `.`, `_`, and `-`; the caller chooses it, its
uniqueness scope is the Photo, and a retry never regenerates it. The service
retains each settled save receipt for the same seven-day reconciliation period
the [Product Spec](../docs/photo-development.md#failure-and-retention) defines
for accepted Export receipts. Within that period, an identical retry with the
same request identity and payload resolves to the recorded outcome and
committed revision and starts no new work. The same identity with a different
payload is refused with `request_conflict`. After that period, a replay of the
identity returns the explicit `receipt_expired` outcome, and expiry never frees
the identity for new work; a new save requires a new identity.

Undo/redo must submit a new guarded recipe write. A whole pointer drag is one
history entry. External changes invalidate assumptions behind local history;
the browser must reconcile before using that history to write over newer state.

## Export Submission and Ordering

Export click captures an immutable copy of the visible control settings and
selected output contract. The browser must place an ordering barrier in that Photo's write stream:
settle prior writes, commit the captured recipe if needed, and submit the Export
against that exact confirmed revision and source binding before sending later edits.

The browser output controller owns one Photo-scoped admission record containing
the immutable submission and its write barrier. Uncertain results retain both
for exact replay; confirmed acceptance or refusal settles only the captured
record. Response and body continuations recheck that record before changing
output state, so an older replay cannot release a later submission's barrier.
The Editor write stream queries this barrier without keeping a second map.

The service must atomically validate the expected current recipe revision and
source binding, capture input/settings/bundle/output contract, persist the
Export and its idempotency receipt, and admit its work. An output contract
outside the selected module's admitted set is refused before acceptance and
must not create an Export or a receipt. A concurrent client can cause a
conflict between save and submission; the service must not substitute a
newer or older recipe. The browser
must preserve the captured intent and resolve the conflict explicitly.

After Export acceptance, subsequent edits may advance the Photo's recipe. They
must not mutate the captured snapshot. Cancellation, polling, reconnect and
download address the Export identity, not the current Edit Recipe.

Retain every accepted Export receipt and its captured snapshot through the
active operation and for the reconciliation period defined by the [Product
Spec](../docs/photo-development.md#failure-and-retention) after terminal
settlement. Repeating the same request identity and payload returns the
existing Export and must not start work; a different payload with that
identity conflicts. An explicit retry creates a new attempt with a new request
identity against the same snapshot, while that snapshot remains retained. After
receipt expiry, the old identity returns an explicit expired outcome and cannot
be submitted as new work. A new Export requires a new identity and a newly
confirmed source/settings snapshot.

Export request identities and receipts follow the guarded-save receipt rules:
the same identity format and Photo scope, retention through the active
operation and for seven days after settlement, and an explicit expired outcome
that never frees an identity for new work. An expired identity must never be
interpreted as new work.

## Preview Scheduling

[Processing Modules](processing-modules.md#preview-and-export) owns bounded
current-step execution and explicit Export materialization.
Preview work is ephemeral and latest-intent-wins within its Photo/step owner.
Pending requests with the same complete identity may coalesce. Completion must
recheck the current owner and full identity before delivery.

Cancelling computation and ignoring obsolete output are distinct operations.
Even a process that cannot stop immediately must not publish its stale result.
Temporary comparison requests own their admission, supersession, and delivery
separately from the main preview and do not change saved intent.

A comparison requests the selected module's published default tree under the
[module comparison contract](processing-modules.md#bounded-current-step-preview).
For darktable this baseline uses 0 EV and as-shot white balance from the input.
Every Preview admission settles: completion, failure, and cancellation free its
identity, a later request cannot report work that no longer exists as still
running, and an attempt the service abandons is cancelled rather than left for
deadline cleanup.
Each response is a current rendition, a queued/running admission result, or
a refusal naming the selected module or incompatible input and reason.

Changing a step's input or parameters makes its preview stale. A downstream step
bound to an immutable Export artifact remains bound to that artifact, not to the
new upstream intent. Display-only changes may reuse a matching bounded result.
No cache hit may be claimed from a filename, modification time, or visible
similarity alone.

## Export Execution and Recovery

Export state is queued, running, succeeded, failed, or cancelled. Browser
disconnection must not cancel admitted work. Cancellation must settle exactly
once against the actual completion state; it cannot undo an already successful
publication.
Queued work must survive restart. During startup, reconcile unfinished running
work from its durable Export snapshot. Mark interrupted work failed with an
actionable reason unless its fully validated published artifact can be recovered. Do not
blindly repeat an expensive interrupted operation or claim success from a
partial output. Explicit retry creates a new attempt on the captured snapshot
and validates source/bundle availability again.

Each attempt owns its temporary files and isolated workload subtree. Write to temporary
output, validate its type, dimensions, profile and expected identity, then
atomically publish. Database/artifact recovery must handle a crash between file
publication and state commit by validating and reconciling the recorded attempt.
Neither orphan files nor a database flag alone establish success.

The attempt owns a validated published file until durable settlement records its
artifact claim. Retention and orphan cleanup must not delete the file during
that interval. An unconfirmed state commit keeps the file protected until the
persistence owner establishes whether the artifact was committed or the attempt
ended without publication.

Remove an attempt's private workspace only after its workload has settled and
terminal evidence is durable. [Local Photo Executor](processing-executor.md)
owns settlement and cleanup; retained Processing Artifacts follow the retention
rules below.

Failed module work must not mark its requested artifact successful or discard an
earlier completed artifact. A previously completed Export remains bound to its
captured input even if the Original or upstream editing intent later changes.

## Resources and Retention

The application must use bounded queue admission, processing concurrency,
threads, decoded pixels, memory, temporary disk and retained output. The initial
scheduler admits at most one heavy processing job at a time per instance.
Queued interactive previews have priority over unstarted exports, with bounded
fairness so neither class starves. Running exports need not be preempted; the
client must expose queueing accurately.

Thread-pool limits must cover NumPy/BLAS, Numba, FFTW and darktable rather than
only the Rust scheduler. Limits and overload errors must be documented and
qualified for the supported deployment. Missing required limits must prevent
processing admission, not normal Library operation.

[Processing Memory](processing-memory.md) owns the shared container
allocation, serialized enforcement, engine workspace plans, buffer lifetimes,
and memory failure evidence. These are execution policies independent of the
Edit Recipe. The deployment must qualify that boundary before processing is
enabled.

[Local Photo Executor](processing-executor.md) defines admission between this
service boundary and the engine child. It keeps Photo/source/recipe and Export
authority in the service and stages a confined copy rather than exposing a
Library path. [Processing Modules](processing-modules.md#interface-and-discovery)
owns each adapter's complete parameter tree and input/output contracts.
Discoverability cannot qualify a new module/input/parameter/output combination.

A cache entry's identity includes content evidence, step settings, exact bundle,
geometry and stochastic policy. Active inputs, outputs and downloads require
leases so eviction cannot remove them mid-operation. Disk exhaustion must leave
saved intent and published artifacts coherent.

The [Product Spec](../docs/photo-development.md#failure-and-retention) owns
retention periods. A successful Processing Artifact and its captured snapshot
remain one retention unit through that disclosed period. An active download
lease delays their removal until its response stream settles. The accepted
Export receipt follows the Product Spec's terminal-reconciliation period for
every output contract and outcome, including failed, cancelled, and
interrupted work. Before accepting a new Export, reserve capacity for the
complete artifact within the deployment's finite retained-output allowance.
If that reservation fails, refuse the new Export without evicting an
unexpired or leased artifact. After expiry and release of all leases, the
artifact and snapshot may be removed; regeneration must validate the captured
source and bundle and fail explicitly when either is unavailable. Each
module output contract carries its own disclosed bounded artifact-retention
period.

Edit Recipe state belongs in backup. Rebuildable derivatives need not. A saved
recipe must retain its processing bundle identity; an engine update must either
keep the compatible bundle available or report that explicit recipe upgrade is
required. Re-rendering with unqualified replacement assets is forbidden.

## Library and Client Integration

Location Recovery with equal content preserves editing intent after validation.
A changed published source revision invalidates use of the bound recipe until an
explicit rebind. That operation must identify both the previously observed
recipe revision and the newly observed source revision. It must compare both
guards atomically and return a new recipe revision. Rebind updates only Original
input source bindings; it preserves step identities, current selection, complete
parameter trees, and artifact bindings. Ordinary saves and processing must not
rebind as a side effect. Exact input bytes are independently verified when
processing stages the Original.

Photo read models must expose whether a saved recipe exists and processing
availability without eagerly rendering every Photo. The saved-edit fact is
true even when saved settings match the baseline or the Original is unavailable.
Saved editing intent and retained Export references must prevent automatic
retirement as an unreferenced record in Retire and Bind.

The service must offer bounded operations to discover admitted module support,
read/change Edit Recipe intent, request the current step's Preview,
submit/list/inspect/cancel/retry Exports, and obtain a validated artifact. Web and
CLI share identity and guards. Per-Photo work and artifact lists each show at
most 64 records, newest first with a deterministic identity tie order. They
restore unfinished work and retained terminal metadata after navigation and
restart; a newer failed or pending request must not hide an earlier downloadable
output. Artifact responses expose module, complete concrete output contract,
identity, publication time, filename, type, size, expiry, TIFF color/orientation/
sample facts, and matching content metadata. Filenames distinguish the module,
step, and immutable artifact identity. Partial downloads must not be reported
complete. These read-view bounds must not evict unexpired receipts or artifacts.

Historical fixed-stage image Exports remain inspectable and downloadable through
read-only history. Their captured metadata, bytes, expiry, and active download
leases remain authoritative. They must not be converted into Processing Artifacts
when complete module/schema/parameter provenance is absent. The bounded history
view is not a retention limit. Retired fixed-stage mutations must not execute
again on restart; unfinished records settle through interrupted-work recovery
without invoking the retired pipeline.

Migration preserves existing composable recipes. Legacy two-control intent becomes
a darktable-owned snapshot preserving exact semantic exposure and white balance,
including unqualified values; it must not substitute baseline values. Settled
save receipts without recorded settlement time begin their seven-day retention
at migration. Legacy request identities whose payload cannot be replayed under
the new contract remain reserved in the Photo's namespace and report expiry,
instead of authorizing a different write.

### Edit state file snapshots

An edit state file captures a confirmed Edit Recipe and its caller-selected
darktable step in one transaction, with Photo, source, recipe, and complete
module parameter provenance. The selected step must provide unambiguous semantic
exposure and white-balance intent; a non-darktable selection or an unsupported
semantic extraction is refused without inventing baseline values. Schema v14
owns the XMP export table. The service persists exact document bytes, filename,
byte length, SHA-256, and creation/expiry times alongside the request identity
and captured recipe. Regenerating a document on read is rejected: generator
changes would alter previously acknowledged download evidence. Restart, later
recipe changes, migration, and unavailable Originals or processing engines must
leave retained historical documents and their evidence unchanged.

The closed request identity uses the same syntax as image Export requests.
The Photo scopes the identity; a different payload conflicts, the same payload
replays its captured record, and an expired identity returns `export_expired`
without starting another snapshot. A new identity with obsolete recipe/source
expectations returns HTTP 409 `stale_edit`. A known Photo without a confirmed
recipe is also HTTP 409 `stale_edit`, while an unknown Photo is HTTP 404
`unknown_photo`. A client receiving `stale_edit` must read the confirmed recipe
again before choosing a new snapshot identity. Read failures return HTTP 503
`resource_unavailable`; an unconfirmed create returns HTTP 500 `outcome_unknown`.
Storage failures must never become missing records.

The XMP uses Camera Raw `Exposure2012` only for semantic EV and `WhiteBalance`
only for As Shot. Custom temperature/tint intent, selected step identity, complete
recipe provenance, and module parameter snapshots stay in the Slipstream
namespace. A Film Recipe is captured only when present in that recipe, never
invented as an implicit successor. The opaque UTF-8 source revision contains NUL
separators and is encoded losslessly as lowercase hexadecimal with the sibling
property `SourceRevisionEncoding` set to `hex-utf8`; raw revision bytes must never
appear as forbidden XML characters.

`RecipeSnapshot` carries the complete canonical stored recipe as XML-escaped
UTF-8 JSON, with `RecipeSnapshotEncoding` set to `json-utf8`. Its field names
and Original/Artifact variants use the durable recipe record shape. It preserves
every step, parameter tree, input contract, and current selection; JSON escaping
preserves embedded NUL bytes without placing forbidden characters in the XML.

POST returns root creation and expiry times and artifact filename, content
type, length and SHA-256. GET list returns at most 64 snapshots, including expired
metadata, in descending creation time, then descending identity order. The
periodic output sweep releases expired document bytes while keeping metadata and
request identities to report expiry after reopening and refuse identity reuse.
GET artifact serves the captured bytes with matching content and Slipstream
artifact headers,
attachment filename and `no-store`. Expired downloads return HTTP 410 with
`export_expired`. No operation writes a photographer-owned Sidecar or Original.

Wire and CLI syntax outside the Photo Development service surface belong in
their authoritative references, not a second DSL inside this specification; the
[wire contract](#wire-contract) of that surface is owned by its Service Surface
section. The new surface must preserve shared error and
transport-uncertainty conventions. It must not expose executable expressions,
engine-private blobs, arbitrary filesystem paths, or a general task engine.

## Service Surface

This section owns the availability, outcome, and refusal semantics every
client surface — Web, CLI, or programmatic — preserves for the operations
above. It defines no HTTP routes, request fields, response fields, or
universal parameter schemas: the dynamic module interface belongs to
[Processing Modules](processing-modules.md#interface-and-discovery), and
wire and CLI syntax belong to their authoritative references under
[Wire contract](#wire-contract) below.

### Availability and diagnostics

Availability composes three independent axes. The module axis is each
Processing Module's own discovery report: availability and refusal reasons
per module, separately from input support and resource admission. The
Library axis is the scan and recovery phase of the Published Library
([Scalable Library Browsing](library-browsing.md#loading-status)). The
Photo axis is one Photo's source-read state. A ready module does not imply
that the Library scan is idle or that any Photo source is readable, and a
recovering Library does not change the module axis. While a scan or
recovery runs, the last Published Library remains the authority for
existing Photos: a Photo whose publication holds current readable source
facts stays `supported` with that source revision, and a Photo without
current source facts reports a retryable wait state, never a confirmed read
failure. Editing is offered only when the selected module reports itself
ready and admits the step's input; an artifact-backed step substitutes its
own retained-input and module-compatibility checks for the Photo axis.

The Photo axis reports one closed reason set. `original-missing` is a
confirmed absence from the remembered Location; `original-unreadable` is a
confirmed read or parse failure for the current source revision; both are
permanent for that source revision. `read-pending` means current source
facts have not been inspected and published yet; `resource-unavailable`
means bounded observation, native-work admission, or attempt
reconciliation could not complete; both are retryable without a restart. A
transient condition must never surface as a confirmed read failure. Recipe
reads derive their source facts from the same Published Library that serves
Photo reads, so one response is coherent with them; classification comes
from committed inspection evidence for the current source revision, never
synthesized from stale metadata, another Photo, or a scan that has not
published. A refusal that follows from the Photo's source state carries the
same closed reason the read reports, so one reason maps to one Web/CLI
behavior.

The selected module's discovery report names the control modes and ranges
its qualification admits for the Photo's source class against the observed
bundle. That report is the only source of truth an editor may enable; a
mode or range outside it is never executed and never substituted with
another mode. Bounded diagnostics compose exactly these axes — the module
report, the Loading Status, and the Photo's recipe read together name the
reason an edit is disabled — and expose no host path, secret, or
engine-private setting.

### Guarded save outcomes

A guarded save returns exactly one outcome:

| Outcome            | Meaning                                                                                             |
| ------------------ | --------------------------------------------------------------------------------------------------- |
| `saved`            | The settings were committed as a new recipe revision.                                               |
| `unchanged`        | The same caller request identity already committed these settings.                                  |
| `unknown`          | The response was lost or the commit is unconfirmed; admission is unproven.                          |
| `receipt_expired`  | The request identity's receipt retention has expired; see the save-receipt rules.                   |
| `recipe_conflict`  | The expected recipe revision is no longer current; the response carries current facts.              |
| `source_changed`   | The expected source revision is not the current published revision; the draft is refused.           |
| `requires_rebind`  | The stored binding is stale and no ordinary save may adopt the new source.                          |
| `request_conflict` | The request identity was already used with a different payload.                                     |
| `missing_recipe`   | A write requiring an existing recipe found none.                                                    |
| `unsupported`      | The Photo's source class is not admitted by the selected module's qualification.                    |
| `invalid_settings` | The settings have a malformed or incomplete structural shape.                                       |
| `unavailable`      | Current source facts cannot be read — confirmed or still pending — so no guarded write is possible. |

The service evaluates these guards in one closed order: request-identity
replay, then the stored source binding, then the expected source revision,
then the expected recipe revision. A stale stored binding produces
`requires_rebind` even when the expected recipe revision is also stale,
because a rebind invalidates the recipe's source-dependent meaning; the
service never applies a payload captured against the old binding. Every
conflict outcome carries the current source revision and the current recipe
revision, or `null` when no recipe exists, so one response lets the client
reconcile both: the current source revision is what a fresh save must
expect or a rebind must name as the newly observed revision, and the
current recipe revision is what the next save must expect and what a rebind
must name as the previously observed revision.

An `unknown` outcome reports a lost response or an unconfirmed commit; the
client retains its draft and reconciles through the same request identity
or current state, treating it as neither refusal nor success. No Export may
start from an unconfirmed save: the client reconciles it before
submitting, and submission validates committed revisions only.

### Failures and refusals

Every refusal carries one structured code, and a code is authoritative;
clients must not parse messages. Three refusal families are distinct. A
processing refusal names the selected module or an admitted input,
parameter, or output combination that cannot execute. A resource refusal
names missing facts, capacity, or admission resources — including exhausted
retained-output capacity — not capability. A permanent `unsupported`
refusal names a source class the selected module's qualification does not
admit. Guarded saves and rebinds admit no engine work, so they never report
a processing refusal; their resource refusal is `unavailable`. A retryable
source-read reason is therefore always a resource refusal, never a
processing failure and never a confirmed read outcome; the retry becomes
meaningful when the named condition clears, not when a module changes.

### Wire contract

Wire and CLI syntax belong to their authoritative references, not to a
second DSL. [Command Line](command-line.md#photo-development-surface) owns
the current deployment's closed HTTP facade and its request/response
fixtures; the [CLI Reference](../docs/cli-reference.md#photo-development)
owns the public command grammar and normalized results. A deployment
facade's selector values name concrete Processing Module configurations:
they must not be reinterpreted as product pipeline stages, universal output
targets, or a composition language, and changing a facade's HTTP contract
requires its own caller and deployment verification. The general module
contract does not silently reinterpret a deployment facade, and a facade
does not expand merely because an adapter describes more controls: the
dynamic module interface is owned by [Processing
Modules](processing-modules.md#interface-and-discovery). Every surface must
preserve the outcome, error, and transport-uncertainty conventions above,
and no surface accepts executable expressions, arbitrary filesystem paths,
or a general task engine.

## Options

### Selected: Rust Ownership with In-Process Execution

This extends the existing service while keeping heavy Python/native failures
inside disposable engine children. It keeps identity, persistence,
scheduling and recovery under one lifecycle, matching the single-container
deployment.

### Rejected: Embed Python in HTTP Handlers

Embedding couples native crashes, interpreter lifetime and memory pressure to
service availability. It offers no necessary product capability for this path.

### Selected: Current Recipe with Captured Export Snapshots

Autosave supports continuous editing while immutable Export snapshots preserve
intent. It needs neither user-visible versions nor an event-sourced history.

### Rejected: Export Whatever Settings Are Current at Execution

Queue delay would change output after the Photographer submitted it. That makes
retry, CLI composition and visual inspection unreliable.

### Selected: Bounded Artifact Retention with Admission Reservation

The Product Spec's fixed retention window keeps the published artifact and
exact request intent available for retries and later download while
bounding retained growth. Capacity is reserved before accepting new work.
An active download lease protects a valid artifact from expiry cleanup
until that transfer settles.

### Rejected: Unbounded Retention or Pressure-Driven Early Eviction

Unbounded retention cannot uphold finite storage. Early eviction under pressure
breaks the disclosed download window, so the service refuses new Exports when
the retained-output allowance cannot admit them.

## Verification

Tests must derive expected behavior from the Product Spec and prove:

- descriptor-confined staging and Original/external-XMP invariance;
- actual headless CPU execution through both engines and validated artifacts;
- late save acknowledgement, navigation during save, draft recovery, revision
  conflicts and uncertain-write reconciliation;
- undo/redo, reset and export ordering with later edits and concurrent clients;
- latest-preview ownership, comparison isolation and source invalidation;
- queue limits, fairness, memory/disk failures and browsing responsiveness;
- native-work saturation, deferred inspection, and interrupted recovery leave
  the Published Library's source facts, source revision, and recipe bindings
  intact, publish retryable (`read-pending`, `resource-unavailable`) rather
  than confirmed-failure reasons, and map one reason to one Web/CLI behavior;
- request deduplication, receipt expiry and export snapshot identity;
- cancel/complete races, process cleanup, restart and publication recovery;
- active-download retention, backup/restore and engine-upgrade behavior;
- editing-aware Location Recovery and Retire and Bind eligibility; and
- equivalent Web/CLI effects and real browser/server completion.

Generated fixtures must be redistributable. Real-camera qualification follows
CONTRIBUTING.md and must be reported separately from deterministic local gates.
The deployed image must run the same complete scenario before live acceptance.
