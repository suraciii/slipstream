# Processing Memory

Photo development loads large native image buffers. A limit on the complete
server can terminate browsing together with a render. A limit checked only by
Python cannot contain allocations in numerical libraries or child processes.
Slipstream needs an enforced task boundary and an engine that uses that boundary
efficiently without changing the photograph.

## Design Drivers

- Preserve browsing, saved Edit Recipes, Originals, and completed Exports when
  processing exhausts its allocation.
- Bound the complete processing workload, including native allocations and
  descendants, before expensive work starts.
- Keep full developed dimensions, numerical precision, and the qualified Film
  Recipe independent of deployment memory policy.
- Keep the Rust service responsible for scheduling and publication. Engines
  remain private, supervised processes, not another Library service.

[Photo Development Architecture](photo-development.md) owns the job lifecycle,
queue fairness, source safety, and publication contracts. This specification
owns memory policy, enforcement, and engine workspace behavior.

## Model and Ownership

The deployment operator owns one finite **processing memory budget** per
Slipstream instance. It is the aggregate hard limit for admitted computation,
not a limit on each thread, a JPEG size limit, or an Edit Recipe setting.
Enabling processing requires an explicit positive byte quantity within a
qualified deployment envelope. Zero, an unlimited value, and an inferred share
of all host RAM are not supported policies. Public rendering requests cannot
override this budget. Configuration syntax belongs in the deployment reference.

The deployment also reserves memory for the **control service**: Rust HTTP and
CLI operations, SQLite, normal Library work, and bounded supervision. This
reserve is established by deployment qualification, not subtracted from the
processing allocation while an attempt is running. For a finite shared ancestor
limit, the processing budget plus the qualified control allowance and deployment
overhead must fit below that ancestor limit. Host capacity planning must also
account for other services; a free-memory snapshot is not a reservation.

An **attempt** owns a private process subtree, workspace, captured settings and
source binding, enforcement identity, and terminal resource evidence. One heavy
attempt runs at a time under the scheduler contract. All of its development,
simulation, validation, and encoding stages share the processing allocation;
these stages run sequentially. Queued work retains bounded metadata rather than
decoded images or preloaded runtimes. Dispatch must not overlap a new attempt
with descendants left by an earlier one.

An engine's **workspace plan** describes required live buffers, temporary
workspace, batch geometry, and qualified runtime headroom for one input geometry,
stage, and exact processing bundle. It is computed from the attempt budget and
the bundle's measured resource model. It is not a hard-limit mechanism and must
not claim that every image can fit every configured budget.

The memory policy and plan belong to the attempt record, not the Edit Recipe.
Changing policy affects subsequent attempts. A running attempt keeps its captured
policy; lowering the deployment allocation requires draining or cancelling it
and confirming that its processes have stopped. Queued work is checked against
the current policy when dispatched, without changing its captured image intent.

## Enforcement Boundary

The supported Linux implementation uses cgroup v2 memory control through the
operator-owned host launcher defined in [Processing Executor](processing-executor.md).
The Rust control service and launcher remain outside the entire finite
processing subtree. A fresh retained attempt slice accounts for the complete
container, including bootstrap charges, while a protected workload leaf groups
engine descendants for OOM termination. The executor contract owns manager
boundaries, placement, private transport, and retained terminal accounting.
Every shared finite ancestor must include qualified control-service and
deployment headroom.

Before a processing executable can allocate image data, its attempt must have:

- `memory.max` set to the captured processing limit;
- `memory.swap.max` set to zero;
- `memory.oom.group` enabled for the attempt workload;
- the qualified CPU, task/thread, time, and temporary-storage limits; and
- confirmed membership for the initial child, with descendants inheriting the
  same containment boundary.

Use launch-time placement or a blocked bootstrap that joins the cgroup before
exec. Starting an unrestricted engine and moving it after allocation is not an
acceptable ordering. Processing children must not be able to write controller
settings, raise limits, or escape into the control-service group.

The budget applies to memory charged by the kernel to the subtree, including
anonymous native buffers, charged file cache, and tmpfs. Process RSS is useful
diagnostic information but is not aggregate enforcement evidence. Staging,
metadata inspection, artifact validation, and file encoding must not move large
allocations outside this boundary. A staging helper may receive a validated
read-only Original descriptor under the source-safety contract; engines receive
only staged input. Supervisor-side IO must use bounded buffers, with its cache
and memory costs covered by the control-service qualification.

`memory.max` is the kernel containment mechanism, not a promise of a perfectly
flat instantaneous peak: the kernel permits temporary excess in some cases.
The workspace plan must leave measured headroom below it. A sampled RSS guard
or `memory.high` reclaim threshold must not be represented as the hard limit.
The initial policy does not use swap or reclaim throttling as a way to make an
otherwise unsupported image fit.

Per-task containment does not guarantee survival under unrelated host-wide OOM,
administrator termination, or a misconfigured ancestor limit. Capability and
deployment checks must distinguish these conditions from a task's own limit.

## Deployment Contract

The selected execution mechanism is an operator-owned host launcher and a fresh
pinned processing container per attempt, as defined by
[Processing Executor](processing-executor.md). Supported Linux packaging uses
systemd slices and Docker's systemd cgroup driver. The Web receives only the
private bounded launcher socket, not Docker credentials, root privilege, or
controller-write access. The worker receives none of those capabilities.

Startup must verify effective ancestor limits and permission to create, place,
observe, terminate, and remove an owned test workload. Operator tooling must
exercise the same container UID, namespace, retained accounting, finite storage,
and controller boundaries used by real processing. Successfully writing a
configuration value or opening a socket is insufficient.

A deployment without this verified boundary must report processing unavailable
and keep ordinary Library operations available. It must not fall back to an
unrestricted subprocess or advertise a whole-server container limit as task
isolation. The supported operator tooling must exercise the same boundary used
by real processing before enabling the capability.

## Workspace Planning and Admission

Queue acceptance and permission to start image computation are separate checks.
Known unsupported geometry or policy may be rejected before queue acceptance.
Otherwise the accepted Export remains queued until dispatch can revalidate
source, bundle, policy, and capacity. Inspection of an untrusted input, including
obtaining geometry, must itself run under bounded resources. Stated dimensions
must be checked against decoded geometry and pixel limits before large allocation.

For each stage, the plan must account for simultaneously live inputs and
outputs, library and thread workspace, IO and file-cache costs, and runtime
headroom. The batch workspace must fit in what remains. Sequential stages use
the maximum of their individual requirements; buffers carried across stages are
included in each relevant stage. Byte calculations must use checked arithmetic.

Select a batch geometry from the qualified range that fits this plan. The range
and overhead model belong to the pinned bundle and must be validated across
input content, aspect ratios, thread settings, and fresh processes. Reading
current free RAM or simply multiplying pixel count by one RGB buffer is not a
sufficient estimator. The supervisor must validate returned plans against its
own limit and the qualified bundle rather than trusting arbitrary engine advice.

If required live data and minimum workspace do not fit, fail before the expensive
stage with a resource-budget reason. A missing resource model is an unqualified
capability, not permission to guess. Unexpected allocation failure or kernel OOM
still requires supervised failure settlement; preflight is not a guarantee of
success. Do not automatically retry an OOM with the same settings or raise the
budget. Explicit retry creates a new attempt using current policy.

[Qualified Film Memory Envelope](processing-film-envelope.md) defines the
separate operator-only admission authority over exact registered fixtures. It
combines source-backed known reservations with a reviewed whole-attempt empirical
ceiling while retaining unknown component diagnostics. Its fixture gate does not
grant ordinary Photo/Export admission or a broader photographic class.

## Engine Memory Efficiency

### Bounded Pointwise Computation

Pointwise output-gamut conversion must use bounded batches and write into one
owned destination buffer. Preserve pixel order, the upstream mathematical
function, viewing conditions, profile assets, and qualified float precision.
Batch-size changes must produce the same pre-encoding pixels throughout the
supported execution envelope. A budget-dependent image change is a failed
qualification, not a new Film Recipe.

The plan must declare the admitted array layout. Flattening or converting a
strided array must not create an unbudgeted full-frame copy. Either require and
validate a qualified contiguous layout, budget the normalization copy, or use
bounded traversal. Verify one-pixel batches, boundaries on either side of a full
batch, odd dimensions, short final batches, and supported non-contiguous inputs
or their explicit rejection.

The initial adapter supports only the pinned CAM16-UCS compression settings,
viewing conditions, and sRGB output profile. It admits nonempty C-contiguous
native float64 RGB arrays with shape `(..., 3)`. It rejects other compression
settings, output spaces, dtypes, empty inputs, and strided layouts before
allocating the destination. It must not fall back to an unbounded transform. Flattening must remain a view. One owned destination
requires 24 bytes per pixel; caller-owned input remains unchanged, including
read-only input. The caller reserves the destination and all other live stage
data separately before granting this transform its temporary workspace.

A versioned transform model maps a positive workspace byte allowance to a batch
size in the qualified range. Its fixed term covers cold color-table construction
and its per-pixel term covers simultaneous transform temporaries, including the
returned batch. An allowance below the one-pixel requirement fails before image
allocation. This allowance is a local algorithm contract, not total-attempt
admission or a substitute for the kernel limit. The complete workspace planner
must include library headroom and account for both the input and destination.

Apply the same principle to eligible transfer-function encoding and numeric
output conversion. Fuse or reuse buffers only where operation order, rounding,
aliasing, and downstream ownership remain correct. IO buffers and the final
JPEG encoder remain part of the measured peak. A temporary runtime monkey patch
is not the delivery mechanism: ship a reviewed, pinned engine change or a narrow
versioned adapter with a reproducible patch identity in the processing bundle.

### Buffer Lifetime

The pipeline must release an intermediate after its last consumer on the actual
execution path, retaining the requested collected result. Early collection,
intermediate injection, fan-out and fan-in must preserve their existing
semantics. Blindly deleting the preceding stage's input is insufficient when a
later node still needs it.

All owners must participate: dispatcher state, caller variables, stage locals,
NumPy views, debug taps, and caches. Removing a dictionary entry cannot free a
buffer still referenced elsewhere. Caches may retain bounded profile/LUT data;
they must not retain full-frame intermediate arrays across attempts. Process
exit remains the final reclamation boundary. Reduced live bytes do not imply an
equal immediate reduction in RSS; resource acceptance measures the actual peak.

### Ordered Lifetimes and Output Encoding

The lifetime plan must mirror the existing declared-order topology walk,
including skipped nodes, overwritten tap names, callbacks, and its first
post-node collection check. Planning uses tap metadata, not image values.
An earlier tap value is needed only until its last consumer before a replacement
write. Release expired dispatcher references before the next node starts;
return the requested collection before pruning its storage. Aliased views retain
their backing arrays through ordinary ownership. Caller input remains borrowed,
and dropping an internal reference does not transfer or revoke caller ownership.
Cold numerical compilation can leave cyclic traceback frames that own completed
stage arrays. The runtime must reclaim these cycles after expiring dispatcher
references at each stage handoff, before another stage allocates. On return, it
first retains the collected result, drops the remaining dispatcher ownership,
reclaims cycles, then returns that result. It must not alter process-global GC
enablement or thresholds. Whole-run measurements include this reclamation cost;
node computation timings may continue to exclude it, while reclamation time is
reported separately.

For sublayer grain processing, retain the interpolated layer densities and
reclaim completed interpolation compiler cycles before particle sampling
allocates its buffers. This handoff must preserve every live caller, layer, and
view owner, and it must not depend on automatic collection occurring at a useful
time. Skipped grain and non-sublayer paths retain their existing behavior.
Reclamation must not change interpolation execution or random-state ownership.
A node duration that includes this work is inclusive elapsed time, not measured
computation-only time. Use the existing
[measurement timing contract](processing-film-measurement.md)
when reporting separate computation and reclamation observations.

Same-profile display encoding must batch the exact qualified color-library
operation, including its matrix and transfer-function order. It must not replace
that operation with a superficially equivalent transfer-function formula. It
uses the same nonempty contiguous native float64 layout as output-gamut
conversion. Finished JPEG conversion admits native float32 or float64 RGB and
normalizes strided layouts only within the current row batch. It rejects empty
or unsupported inputs before opening the output file. Finished JPEG conversion preserves clipping, scaling, and integer truncation.
It writes consecutive full-width row batches using bounded numeric workspace,
retaining the source samples and identical encoder settings, ICC, and geometry.
The numeric allowance must admit at least one row before creating the file.
Native encoder storage is a separate, measured stage requirement; scanline IO
must not be described as proof that the codec uses constant memory. Failed open,
write, or close operations must fail the Export rather than publish a partial
artifact.

Finite-value checks and pixel identity hashing must also use bounded chunks.
Pixel digests retain the exact C-order sample bytes of the existing identity,
including non-contiguous inputs through bounded traversal. Qualification must
report observer/harness changes separately when comparing aggregate peaks.

### Spatial and Stochastic Operations

Grain, halation, diffusion, blur, and sharpening must retain the qualified image
semantics. They must not be divided into independent tiles using the pointwise
rule. Any later spatial tiling needs explicit boundary/overlap treatment, filter
support, physical pixel scale, and random-state ownership, with seam and complete
image comparisons. Precision reduction and memory-mapped spill are separate
algorithm/resource decisions and require their own quality and storage evidence.

## Failure, Cancellation, and Recovery

The supervisor must distinguish these semantic outcomes before mapping them to
the shared Web/CLI error contract:

- enforcement unavailable or invalid deployment policy;
- input or stage outside the qualified resource envelope;
- workspace plan exceeds the captured budget;
- runtime allocation failure or confirmed attempt OOM;
- deadline, cancellation, engine error, and interrupted supervisor ownership; and
- failed cleanup or unconfirmed process termination.

Exit 137 alone is not proof of OOM. Classify a confirmed limit failure using
attempt-local kernel events and terminal process/container evidence. Preserve
uncertainty when evidence is missing. An allocation failure caught before the
kernel intervenes remains a resource failure, with its distinct evidence.

On failure or cancellation, settle the complete attempt subtree, collect terminal
evidence, and withhold publication of incomplete artifacts. Do not release its
scheduler slot until the owned subtree is empty. If termination is unconfirmed,
quarantine the attempt and stop new heavy work while keeping browsing available.
Existing validated Development Results follow the architecture's retention rule.

After supervisor restart, reconcile persisted attempt ownership with the actual
owned cgroups before admitting new processing. Kernel policy remains effective
while the supervisor is absent. Orphaned work must not publish independently.
Settle or recover it through the existing guarded artifact-publication contract;
never kill an unrelated process based only on a recycled PID. Confirm process
exit and preserve evidence before removing cgroups or temporary work.

The existing single terminal-result and cancellation/publication race rules
remain authoritative. Changing the resource policy cannot rewrite an already
successful Export or its captured settings.

## Evidence and Qualification

Record the attempt and instance identity, bundle/patch identity, geometry, stage,
captured limit and workspace plan, effective enforcement settings, stage timings,
aggregate memory peak and events, process outcome, and cleanup outcome. Retain
bounded diagnostics outside the dying workload before releasing ownership.
Private source paths and image contents must not enter public logs or Issues.
Administrative diagnostics may expose detailed resource facts; Photographer
errors must explain the affected operation, saved-state preservation, and next
action without exposing kernel or Python internals.

Qualification must separately prove:

- synthetic native and descendant allocation is contained at both attempt and
  aggregate-parent limits, with no control-service termination, and the next
  valid attempt can run after cleanup;
- invalid/missing delegation, restrictive ancestors, and attempted child escape
  fail without unrestricted execution;
- queue, cancellation, OOM/complete races, supervisor restart, and failed cleanup
  preserve one terminal result, source safety, and publication integrity;
- batch-boundary, odd-aspect, negative, over-range, saturated, and neutral inputs
  match the reference, including full seeded small-image pipelines;
- topology lifetimes preserve injection, collection, shared views, and branching,
  without mutating caller-owned input;
- representative real-camera previews and full exports stay inside the proposed
  envelope, with original dimensions, effects, profiles, and encoding preserved;
- cold/warm process runs record aggregate peaks, time, threads, IO, and browse
  contention against the same idle baseline and declared acceptance thresholds;
- low budgets fail predictably, supported budgets succeed, and successful budget
  changes leave pre-encoding pixel identity unchanged; and
- the exact packaged deployment proves the same isolation and recovery contract.

Kernel/cgroup evidence is separate from simulated unit tests, ordinary repository
gates, and camera qualification. A successful small image, one full-size export,
or a finite container setting cannot establish the supported deployment envelope.

## Options

### Selected: Task Cgroups with Engine Workspace Planning

The kernel contains native and descendant allocations; Rust owns admission and
settlement; the engine minimizes live data within that budget. This extends the
existing process-per-job architecture and avoids a second public job service.

### Rejected: Whole-Server Container Limit as Task Isolation

It bounds deployment consumption but shares the failure boundary with HTTP,
SQLite, and browsing. It remains an outer deployment constraint, not the task
memory contract.

### Rejected: Python Checks or Address-Space Limits Alone

Cooperative checks cannot stop opaque native allocations. A process address-space
limit measures a different resource and does not aggregate independent child
processes. Neither replaces the selected workload boundary.

### Selected: Restricted Host Launcher and Fresh Containers

The host launcher enforces and observes each attempt outside its OOM boundary.
Its operational journal does not own the Library or Export lifecycle. Retained
systemd accounting closes the terminal-evidence gap when Docker removes a scope.
[Processing Executor](processing-executor.md) defines the authority and lifecycle.

### Rejected: Writable Delegation Inside the Web Container

Safe in-container delegation would require additional migration authority,
controller ownership, and engine identity separation. The supported Web image
remains unprivileged and has no writable controller mount. An unavailable
launcher cannot silently fall back to this different execution mechanism.

## References

- [Linux cgroup v2 memory interfaces](https://docs.kernel.org/admin-guide/cgroup-v2.html#memory-interface-files)
- [systemd cgroup delegation and controller ownership](https://systemd.io/CGROUP_DELEGATION/)
- [Docker memory and swap constraints](https://docs.docker.com/engine/containers/resource_constraints/)
