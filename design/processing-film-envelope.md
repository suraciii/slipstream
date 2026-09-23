# Qualified Film Memory Envelope

Component inventories explain some Film allocations, while retained cgroup
accounting observes the complete attempt. A whole-attempt peak cannot identify
four independent native, runtime, allocator, and kernel components. Admission
needs a qualified total envelope without turning unresolved component costs into
zero or allowing a measurement request to bypass the gate.

This contract admits only existing, exact registered Film fixtures. It produces
operational receipts, not Exports, and grants neither arbitrary TIFF ingestion
nor Photo/Library access. [Processing Memory](processing-memory.md) owns ordinary
processing policy; [Film Resource Measurement](processing-film-measurement.md)
owns the fixture, numerical procedure, fixed resource limits, sealed staging,
two permits, native validation and result handoff. Those contracts apply here
except for the distinct admission authority defined below.

## Model and Decision

A **qualified envelope** is an operator-approved immutable document binding a
finite set of catalogue fixtures to an empirical whole-attempt ceiling and a
separate safety reserve. It binds the exact executable image, launcher,
numerical procedure and observed host environment. Approval requires independent
review of the referenced qualification evidence; a valid hash is not approval.

A **known inventory** is compiled source-backed reservation arithmetic. Its
missing component diagnostics remain visible even when the independently
qualified total envelope permits an attempt. It is neither an estimate of every
allocation nor a measured lower bound on actual usage.

An **admitted plan** captures the matched envelope, evidence and environment,
known inventory, complete total reservation, local workspace plans and actual
attempt limit. Only this plan can authorize heavy work in this profile.

Two alternatives were considered:

- Filling the measurement model's separate empirical component bins would
  require evidence that isolates those owners. Whole-attempt peaks do not
  provide it, and subtracting asynchronous analytical maxima does not create it.
- A qualified operational total ceiling can cover unresolved active arrays,
  retained arrays, native/runtime workspace, allocator and kernel charges
  together. Comparing that ceiling with known reservations preserves useful
  source checks without inventing a decomposition. This is the selected model.

Configuration and public protocol version 3 select `film-qualified-fixtures`
with root-only peers and a fresh instance. Native qualification version 1 and
Film measurement version 2 retain their existing authority and semantics.
Reinterpreting v2 `fits-model` would hide a changed permission boundary. No
request can switch modes, bypass qualification, change a reserve, or select a
different recipe, executable or resource limit.

## Formula and Provenance

`film-total-envelope-v1` is a compiled formula, not an expression language:

```text
S = full enforced shared temporary-storage capacity
F = exact captured TIFF source byte length, or zero for synthetic input
K = maximum known sequential-stage reservation, already including S + F
E = approved empirical whole-attempt charged-memory ceiling for the fixture
R = separately approved, predeclared positive attempt safety reserve
required = checked_add(max(K, checked_add(E, S, F)), R)
admit iff all qualification checks pass and required <= attempt_limit
```

E includes the entire accounting lifetime from capped bootstrap through input
staging, simulation, encoding, native validation and retained charged storage.
It is an operational ceiling validated for a finite scope, not a proof that an
opaque component can never exceed a particular number. Adding full S and F
above E deliberately overlaps charges already present in observations. This
avoids assuming every measured attempt populated all allowed storage or source
cache. Do not subtract observed residency or borrow R to obtain a fit. R covers
attempt uncertainty; it does not replace separately qualified control-service,
shared-ancestor or host headroom.

The closed inventory revision `film-known-storage-local-v1` uses the measurement
contract's fixed stages, checked geometry and fixed local workspace plans. Its
known stage reservation is S + F plus 65536 bytes for staging and validation,
the respective computed local scratch for gamut, CCTF and JPEG, and no additional
known bytes for other stages. Take the maximum, never the sum of sequential
stages. Owned-array inventory is unknown at every stage except staging and
validation; runtime, native, allocator-retention and kernel terms remain unknown
at every stage. These exact missing stage/term pairs are emitted in stage order,
then term order from the measurement schema. Unknown terms contribute no known
bytes but are never reported as zero-valued or qualified bounds. A later proven
inventory requires a reviewed compiled revision and new qualification.

S remains the enforced 4 GiB cap. The existing local allowance constants and
batch algorithms remain unchanged. The new document supplies no coefficients,
stage programs, batch controls, component byte bins or precision switches.
Every multiply and addition, including E + S + F and the final R, uses checked
unsigned 64-bit arithmetic. Preserve the independent geometry, address-space,
dtype/layout and whole-row checks. Overflow is invalid qualification or input,
never a saturating fit. Equality with the limit fits.

## Qualification and Scope

The envelope binds the exact catalogue digest; every case names a unique fixture
in that catalogue. A missing case is outside the envelope. An explicit unknown
case has no E, R or evidence and cannot run. A qualified case supplies positive
E and R plus the qualification evidence digest. Duplicate or foreign cases are
invalid. Multiple fixtures may cite the same reviewed evidence, but membership
and arithmetic remain per fixture. The catalogue's complete source identity,
geometry, decoded-pixel reference, recipe and ICC bindings are unchanged.

This exact-content gate does not imply support for all smaller images, a rotated
shape, another TIFF encoding, another source kind or new photographic content.
A supported finite geometry/content class requires a separately predeclared
calibration and held-out corpus covering its permitted branches, aspects, tails,
source layouts and content. Only explicitly qualified orientations belong to
such a class. Class-wide Photo admission requires its own bounded input and
dispatch contract; a fixture ID is never a substitute for it.

Before collecting acceptance observations, freeze the corpus split, exact
reference outputs, finite repetitions, tested limits, cache/thread procedure,
the rule deriving E from calibration observations, the independent R rule and
all failure criteria. Derive E and freeze it before revealing held-out results.
R cannot be adjusted to hide a failed held-out observation. Qualification must
show every required valid memory-fit observation at or below E itself; neither
R nor the overlapping S/F reservations may conceal an E miss. Each such
observation requires complete reference output, exact retained peak/events,
confirmed terminal cleanup and healthy control-state operations. Low-limit OOM
experiments establish containment; they cannot be discarded or relabeled as
successful samples. A changed scope, eligibility/observer protocol or fitted
rule requires independent review and a fresh campaign; observations from
different protocols cannot be combined. No observation in a syntax example
qualifies a fixture or recommends a deployment budget.

### Campaign Evidence Eligibility

Memory qualification and terminal I/O diagnostics answer separate questions.
The campaign records four independent booleans so that loss of an I/O sample
cannot erase complete memory evidence or be mistaken for a complete campaign
row:

- `memory_fit_eligible` requires exact fixture, source, reference, executable,
  launcher and environment identity; retained attempt-slice memory peak, limits,
  memory events and zero-swap evidence equal to the terminal receipt; successful
  reference-matching output; confirmed cleanup and exact ownership; and every
  predeclared Web/Album and host/ancestor headroom check to pass. Missing memory
  evidence, unresolved identity, OOM, failed output, uncertain cleanup or failed
  headroom makes the row ineligible for E/R fitting.
- `io_diagnostic_eligible` requires complete terminal I/O evidence for the exact
  attempt and its declared scope. Missing per-attempt terminal counters make the
  diagnostic unavailable. Parent I/O counters cannot stand in for attempt
  counters unless contemporaneous evidence proves exclusive membership of the
  attempt in that parent for the entire observation interval.
- `observer_window_eligible` requires the predeclared observer window to have
  complete, attributable Web/Album and host/ancestor observations, with no
  unresolved identity or ordering contradiction that affects memory or
  headroom evidence. It does not require terminal I/O counters when those
  counters are unavailable independently of the complete memory observations.
- `campaign_row_eligible` is true only when memory-fit, I/O-diagnostic and
  observer-window eligibility all pass. It represents a complete campaign row,
  not the input gate for the memory fitter.

An eligibility result that cannot be established from retained evidence is
false and records its reason; missing evidence never implies a pass.
The fitter consumes only `memory_fit_eligible` rows and reports I/O diagnostic
coverage separately. A missing terminal I/O diagnostic may therefore leave
memory fitting valid while the full campaign row remains ineligible; it cannot
be imputed, copied from a parent without the proof above, or presented as
complete release evidence. Per the [Processing Executor](processing-executor.md),
the launcher must establish its I/O release precondition before work is released.
Meeting that precondition does not make terminal I/O diagnostics a memory-fit
input.

The selected separation preserves usable, independently complete memory
observations while keeping missing diagnostics visible. A single composite
eligibility flag is rejected because it either discards valid memory evidence
when only terminal I/O is missing or silently treats incomplete diagnostics as
complete. Low-level observer failures that affect memory identity, memory
accounting or declared headroom still disqualify the memory-fit row.

Evidence retains the immutable image/launcher identities, source and output
hashes, compiled inventory/formula, full captured limits, thread/cache procedure,
environment, raw parent/attempt local and hierarchical events, peak, outcome,
phase timing where observed, cleanup and independent reference provenance. It
also records the predeclared corpus/rules, each eligibility result with its
supporting reason, I/O diagnostic coverage and review decision. Full evidence is
an operator artifact; the launcher does not fetch URLs or implement a review,
signing or approval service.

## Documents and Environment Authority

The [JSON Schema](schemas/processing-film-envelope.schema.json) is the sole new
syntax authority and references unchanged measurement definitions locally.
The measurement contract's strict lexical JSON rules and canonical encoding
apply unchanged. Resolve schema references from packaged files only, without
network access. Canonical conformance inputs are in the
[vectors](schemas/processing-film-envelope-vectors.json).

Configuration remains at most 16 KiB. It binds the existing `catalogue.json`
(32 KiB, at most 16 fixtures) and a fixed `envelope.json` (128 KiB, at most
16 cases) beneath the sealed root. There is no resource-model file in this
profile. Use the existing confined, bounded immutable-document read rules and
exact-byte SHA-256. Document/config changes require a drained restart. An
operator provisions the approved envelope digest only after independent review;
the presence of a well-formed document does not perform that approval.

The envelope binds a resolved immutable local image ID, the SHA-256 of the actual
launcher executable, the catalogue digest, the closed formula/inventory revisions
and an observed environment. The catalogue and packaged image additionally bind
the numerical bundle, recipe, ICCs, reference procedure and fixed thread policy.
Construct the envelope after building the image and launcher. Neither executable
contains its own final envelope digest, avoiding a circular build identity.

The fixed environment collector observes Linux x86-64, kernel release, system
page size, CPU identity and the selected Docker runtime identity. Missing,
duplicate, oversized or unparseable observations make admission unavailable;
operator strings cannot replace them. Observations are read with finite existing
manager deadlines and at most 1 MiB per source:

- Kernel release and machine come from `uname`; page size from `sysconf`.
- Parse `/proc/cpuinfo` into one record per logical processor, in numeric
  processor order. Each record contains that processor number and the exact
  trimmed values of `vendor_id`, `cpu family`, `model`, `stepping`, `microcode`,
  `physical id` and `core id`, plus the unique whitespace-separated `flags`
  sorted by UTF-8 byte order. Require each field once per record, unique
  processor numbers, ASCII nonempty values and at most 4096 records. Hash the
  canonical array of objects using those source field names; the flags field
  is an array, processor is an unsigned integer, other values are strings.
  Dynamic frequency and utilization do not enter identity.
- Select the same explicit `runc` runtime used by container creation. From
  Docker's bounded `version` result capture the unique Engine, containerd and
  runc component Version and GitCommit. From `info.Runtimes.runc` require path
  `runc`, parse the `org.opencontainers.runtime-spec.features` status JSON and
  hash the canonical object `{engine, containerd, runc, runtime_features}`;
  the first three values are closed `{version, commit}` objects. Require the
  runtime features' runc version/commit annotations to match that component
  after removing only their trailing newline. A different selected runtime or
  missing selected-runtime evidence is unavailable; an unrelated installed
  executable's version does not establish the identity.

The complete environment object is canonical-hashed for the plan. The stored
object and all compiled identities must equal current observations at admission
and dispatch. Rechecking does not turn host root interference into a supported
concurrent mutation; executor ownership and uncertainty rules still apply.

## Admission, Replay and Execution

Retain the executor's identity, watermark, exclusive slot, durable manager
intent, quarantine, cancellation and settlement rules. Handle an existing
accepted identity before consulting current qualification: the same captured
request returns its old receipt; changed intent conflicts. Expired identity
cannot relaunch. Changing approval never discards pending ownership or rewrites
an old result.

For a new Start, check instance/incarnation, policy/bundle/document identities,
capacity and exact runtime ownership as usual. Then validate fixture membership,
geometry, observed environment, qualified case, checked plan and fit before
reserving the sequence/slot or making any manager/workspace/staging effect.
Refusal consumes no sequence and creates no receipt. The response error codes
retain their existing meanings, with these additions:

| Code                    | Meaning                                                     |
| ----------------------- | ----------------------------------------------------------- |
| `incompatible-envelope` | Requested digest differs from the captured document.        |
| `outside-envelope`      | Registered fixture has no envelope case.                    |
| `unqualified-envelope`  | Case explicitly lacks a qualified total ceiling.            |
| `resource-budget`       | Valid qualified plan requires more than the captured limit. |

Unknown catalogue IDs remain `unknown-fixture`; unsupported geometry remains
`unsupported-fixture`; malformed documents or overflow are invalid input.
Missing observation authority, environment/executable mismatch or unavailable
containment is `unavailable`, never a budget refusal. Reconcile reports the
distinct `film-qualified-fixtures-only` capability. Availability is `blocked`
when observation, ownership or containment is unavailable, `unqualified` when
that boundary is usable but the envelope has no qualified case, and `available`
otherwise. It does not promise that every catalogue case fits. A usable boundary
with an unknown selected case returns `unqualified-envelope` even if no other
case is qualified; an absent selected case returns `outside-envelope`.
An invalidated case also returns `unqualified-envelope`; it is excluded when
determining whether any qualified case remains available.

Capture the admitted plan and exact canonical manifest before ordinary durable
acceptance. Before both staging and engine release, revalidate the current
environment, identities, actual limit and exact owned topology against the
captured plan. Drift prevents release and settles the accepted attempt as
interrupted with null detail; uncertain cleanup retains the slot. Never change
its plan, grant new time or fall back to measurement. Completed execution evidence
still has the existing precedence over a later control interruption.

The v3 `EngineGrant` selects the qualified validator. Native Rust recomputes the
known inventory, missing diagnostics, source reserve, local plans and complete
checked formula, requires equality with every captured value and checks fit
before either heavy permit. Envelope approval remains the launcher's authority;
the grant carries a sealed bounded summary, not an engine-editable evidence
document. Python validates the authenticated v3 grant and exact local plans
before pixel allocation; it cannot raise the allowance or authorize admission.
No v3 request can be executed using the v2 measurement grant path.

The unchanged version-2 stage messages and producer/native result formats are
reused by reference. Their versions identify exchange syntax, not permission to
bypass admission. Their grant, manifest and plan digests must match the v3
capture; a successful format-v2 result alone cannot establish v3 admission.
The public v3 instance rejects public v1/v2 requests. Its bundle identity uses
the canonical object `{profile, image, numerical_bundle}` with profile
`slipstream-film-qualified-fixtures-v1` and the resolved immutable image ID.

Unexpected allocation failure, OOM, storage failure or output mismatch retains
the existing supervised outcome/evidence rules. Do not retry, escalate limits,
weaken validation or silently revise the qualification.

### Durable Qualification Invalidation

Verified attempt peak greater than captured E, confirmed OOM attributed to the
attempt or its exact owned processing parent, or a confirmed `allocation-failed`
outcome invalidates the exact (envelope digest, fixture ID) pair for subsequent
admission. Record the distinct reason `peak-exceeded`, `processing-oom` or
`allocation-failed`, in that precedence if several are established. Allocation
failure is an operational qualification failure, not evidence that measured peak
exceeded E. Unrelated ancestor/host pressure, unavailable observation or detected
limit/environment tampering cannot fabricate these findings. Storage/output
faults and cancellation alone do not invalidate memory qualification.

The receipt's nullable `qualification_failure` reports this assessment while
preserving the execution outcome: a complete reference-matching image whose
verified peak exceeds E still has outcome `completed`. It cannot authorize
another execution. Missing peak remains missing and cannot establish a peak
contradiction. Record this assessment, terminal evidence/outcome and invalidation
in one durable registry transition before cleanup can remove their retained
accounting or result sources, and before releasing the slot. Recovery completes
the same assessment from
captured evidence before any new admission; a persistence failure retains
ownership and blocks admission rather than losing the invalidation.

Keep a bounded set of at most 256 invalidations in the operational registry,
including the first proving attempt's incarnation/sequence and reason. It has no
expiry, automatic deletion or request mutation. Receipt expiry, restart and an
ordinary switch from envelope A to B and back to A never remove A's fence.
Replays still return captured receipts, and other non-invalidated cases remain
eligible. Every new acceptance requires capacity for its one potential new
invalidation; the exclusive active slot prevents another attempt consuming that
space concurrently. At set capacity, refuse new admission with `capacity`;
settlement and inspection continue. Registry-loss and instance-claim rules remain unchanged.
An independently reviewed, explicitly revised envelope digest is required to
admit the affected fixture again. Capacity exhaustion requires explicit operator
reconciliation or a fresh instance with reviewed qualification; ordinary startup
must not reset the registry or reuse withdrawn evidence.

## Bounded Records and Verification

Keep the existing 16 KiB grant/stage bounds, 64 KiB public responses, 4 MiB
registry and 256 retained-receipt cap. Evidence corpora and environment source
records do not enter each receipt; only their bounded digests and the admitted
plan do. Validate maximal legal grant, response and retained-record capacity
before enabling the profile. Admission must reserve final-record capacity, not
discover an unpersistable receipt after execution. No limit is raised by this
contract.

Required automated checks include exact-fit and one-byte-below arithmetic,
overflow at every sum/product, canonical cross-language identity, unknown-term
preservation, sequential maxima, TIFF/synthetic reserves and portrait/landscape
membership. Validate all syntax examples with local schema resolution and reject
unknown/duplicate fields, integer coercion and forged plans. A changed image,
launcher, environment or procedure refuses new work while old replay and cleanup
remain intact. Prove refusal has no sequence/slot/manager side effects, and drift
before each permit prevents execution.

Actual packaged qualification must show one approved accepted fixture, a genuine
below-plan policy refusal before heavy work, contained unexpected failure,
cancellation/restart and subsequent success with complete cleanup and preserved
control-state operations. Carry forward native-v1 and measurement-v2 behavior.
This proves the finite fixture gate; supported photographic classes and ordinary
Photo/Export integration require their separate evidence and contracts.
