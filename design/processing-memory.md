# Processing Memory

Photo development loads large native image buffers. Slipstream is a personal,
single-machine application: one container hosts the Web service and the
development engine together, and they share that container's finite memory and
CPU. This specification states what that shared boundary guarantees, what it
deliberately does not, and how the engine stays within it without changing the
photograph.

## Design Drivers

- Preserve browsing, saved Edit Recipes, Originals, and completed Exports when
  processing exhausts the shared container allocation.
- Keep full developed dimensions, numerical precision, and the qualified Film
  Recipe independent of deployment memory policy.
- Keep the Rust service responsible for scheduling and publication. The engine
  remains a private child process inside the application, not another Library
  service.
- Serialize heavy work: at most one development attempt runs at a time inside
  the one application container.

[Photo Development Architecture](photo-development.md) owns the job lifecycle,
queue fairness, source safety, and publication contracts. This specification
owns memory policy, enforcement, and engine workspace behavior.

## Model and Ownership

The deployment's one **application container** owns a finite memory and CPU
allocation shared by the Rust service, Web delivery, SQLite, and the
development engine. This shared limit is the documented resource boundary of
the personal single-machine product: there is no separate per-attempt
processing budget, and public rendering requests cannot change it. Sizing the
container belongs to [Deployment](../docs/deployment.md).

Inside that boundary the service keeps its **control allowance**: Rust HTTP and
CLI operations, SQLite, normal Library work, and bounded supervision must
continue to fit beside one running development attempt. Serialization is what
makes this plannable. The scheduler admits at most one heavy attempt at a
time, so the engine's measured peak is the dominant term and Library work
competes only with bounded supervision, staging copies, and validation.

An **attempt** owns a fresh engine child, a private scratch workspace, captured
settings and source binding, and its terminal outcome. All of its development,
simulation, validation, and encoding stages run sequentially in the container.
Queued work retains bounded metadata rather than decoded images or preloaded
runtimes. Dispatch must not overlap a new attempt with an engine child left by
an earlier one; the processing lock is released only after termination and
scratch cleanup are confirmed.

An engine's **workspace plan** describes required live buffers, temporary
workspace, batch geometry, and qualified runtime headroom for one input
geometry, stage, and exact processing bundle. It is computed from the container
allocation and the bundle's measured resource model. It is not a hard-limit
mechanism and must not claim that every image can fit every deployment.

The workspace plan belongs to the attempt record, not the Edit Recipe. Changing
the container allocation affects subsequent attempts; a running attempt keeps
the environment it started in, and queued work is checked against the current
allocation when dispatched, without changing its captured image intent.

## Enforcement Boundary

The supported Linux implementation is the one application container's finite
memory and CPU limit, as sized by [Deployment](../docs/deployment.md). The
engine runs as an ordinary child process of the Rust service inside that
container. The container limit bounds the Web service, SQLite, the engine, and
every native allocation the engine's libraries make, including descendant
processes. There is no per-attempt cgroup, no processing subtree, and no
swap policy to tune inside the product: that per-attempt machinery was the
retained trade of the local model, accepted for a single-user product.

The consequence an operator must understand is that the boundary is shared,
not isolated. A runaway engine allocation can exhaust the container and
terminate browsing together with the render, and an engine OOM can stop the
whole application. Serialized execution is the mitigation: at most one heavy
attempt runs, so the failure surface is one request at a time, and the
recovery path is the ordinary application restart with its startup scratch
cleanup. The engine must not be able to grow beyond the plan above: staged
inputs are bounded, stated dimensions are checked against decoded geometry
and pixel limits before large allocation, and a deadline bounds total engine
time.

Supervisor-side work — staging, metadata inspection, artifact validation, and
file encoding — shares the same container allocation and must use bounded
buffers so it cannot crowd out Library work by itself. Process RSS figures
are diagnostics, not enforcement evidence. An unusably small container limit
is a deployment defect to report through the capability contract, not a reason
to fall back to unrestricted host execution.

Per-attempt containment is not claimed. Unrelated host-wide OOM, administrator
termination, or a sibling workload on the same host can still stop the
application. Capability and deployment checks must distinguish those
conditions from a development failure.

## Deployment Contract

The selected execution mechanism is the in-process PhotoExecutor defined by
[Local Photo Executor](processing-executor.md): one application image contains
the server, the optional bundled engine extension, and the qualified runtime.
A normal start requires no host launcher, processing systemd unit, worker
container provisioning, processing socket, or processing Compose overlay.

Startup discovers the optional extension, verifies its manifest identity and
every named asset digest, and reports development available only when those
checks pass. Missing or invalid assets leave Library operations available and
development reported `bundle-unavailable`. Operator control is the documented
startup environment — `SLIPSTREAM_PHOTO_DEVELOPMENT` to force the capability
on or off, and `SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY` to select a locally built
bundle for development or smoke runs — never a request field.

The operator's resource obligation is to size the one container's memory and
CPU for the Web service plus one serialized development attempt, using the
bundle's measured resource model, and to keep that sizing with the deployment
record. Local verification and the real-camera smoke must exercise a complete
development, cancellation, deadline, engine failure, and restart inside those
limits before a deployment is treated as processing-enabled.

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

Each Processing Module requires independently qualified resource evidence before
admission to this shared boundary. [Processing Modules](processing-modules.md#explicit-materialization)
owns the standalone Film Export lower-bound refusal. Passing that check is not
complete-attempt qualification; bounded Preview and full-resolution Export require
separate evidence for the exact bundle, input, parameters, and geometry.

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

- capability or deployment policy unavailable;
- input or stage outside the qualified resource envelope;
- workspace plan exceeds the container allocation;
- runtime allocation failure or a confirmed engine termination; and
- deadline, cancellation, engine error, failed cleanup, or unconfirmed child
  termination.

Exit 137 alone is not proof of OOM. Classify a confirmed limit failure from
the engine child's termination evidence and the container's observed events,
and preserve uncertainty when evidence is missing. An allocation failure
caught before the kernel intervenes remains a resource failure, with its
distinct evidence.

On failure or cancellation, terminate the engine child's complete process
group, reap the direct child, remove the private scratch workspace, and
withhold publication of incomplete artifacts. Do not release the processing
lock until termination and cleanup are confirmed. If termination is
unconfirmed, keep heavy work stopped while browsing remains available.
Existing validated Development Results follow the architecture's retention
rule.

After an application restart there are no owned cgroups or journals to
reconcile: startup never attaches to an engine process from the previous
lifetime. It removes abandoned scratch under its exclusive lock, resolves
unfinished work from the durable Export snapshot, and admits new work through
ordinary serialization. Orphaned work must not publish independently.

The existing single terminal-result and cancellation/publication race rules
remain authoritative. Changing the resource policy cannot rewrite an already
successful Export or its captured settings.

## Evidence and Qualification

Record the attempt identity, bundle identity, geometry, stage, the container
allocation and workspace plan, stage timings, observed process outcome, and
cleanup outcome. Retain bounded diagnostics outside the attempt before
releasing the lock.
Private source paths and image contents must not enter public logs or Issues.
Administrative diagnostics may expose detailed resource facts; Photographer
errors must explain the affected operation, saved-state preservation, and next
action without exposing kernel or Python internals.

Qualification must separately prove:

- one serialized engine attempt stays inside the sized container allocation
  while Library reads stay responsive, and the next valid attempt can run
  after cleanup;
- engine failure and cancellation terminate the child process group, scratch
  is removed, and no partial artifact is published;
- queue, cancellation, OOM/complete races, application restart, and failed
  cleanup preserve one terminal result, source safety, and publication
  integrity;
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
- the exact packaged deployment proves the same resource and recovery contract.

Real-camera smoke evidence is separate from simulated unit tests and ordinary
repository gates. A successful small image or one full-size export cannot
establish the supported deployment envelope. The retired kernel-containment
evidence and its operator harness were removed with the launcher path.

## Options

### Selected: One Container with Serialized Local Execution

The application container's finite memory and CPU limit is the resource
boundary, and the single processing lock serializes engine work. It matches
the personal single-machine deployment: no privileged host components and no
second operational lifecycle. Its documented cost is the shared failure
domain — an engine OOM can stop the whole application — accepted because at
most one request runs at a time and the ordinary application restart recovers.
Engine workspace planning still minimizes live data so the sized container
stays sufficient.

### Rejected: Per-Attempt Task Cgroups with a Host Launcher

Kernel containment per attempt would isolate an engine OOM from the Web
service, but it required a privileged host launcher, Docker authority, cgroup
management, and a durable journal — operational weight with no single-user
product value. Issue #490 removed it from the production path together with
its harness and operator workflows.

### Rejected: Python Checks or Address-Space Limits Alone

Cooperative checks cannot stop opaque native allocations. A process
address-space limit measures a different resource and does not aggregate
independent child processes. Neither replaces the container limit plus
serialization.

## References

- [Local Photo Executor](processing-executor.md)
- [Deployment](../docs/deployment.md) for the container resource contract
- [Docker memory and swap constraints](https://docs.docker.com/engine/containers/resource_constraints/)
