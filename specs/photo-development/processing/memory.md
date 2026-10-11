# Processing Memory

Photo development loads large native image buffers. An independent tool needs a
finite memory/CPU environment whether invoked directly or supervised by Web.
The bundled Web deployment shares one container allocation with the service;
a standalone environment has its own qualified allocation. This specification
owns those limits and workspace planning without changing the photograph.

## Design Drivers

- Preserve saved tool state, read-only inputs and completed files under resource
  failure; the shared Web deployment may still lose browsing to container OOM.
- Keep full developed dimensions, precision and Film Recipe independent of limits.
- Keep resource planning and native-child supervision in the independent tool.
  Slipstream admits and reconciles only work it starts.
- Serialize supervised Web attempts; do not claim that this also serializes
  independently invoked tools on the host.

[Photo Development Architecture](../design.md) owns captured invocation
and supervised receipts. [Photo Workspace](../workspace/design.md) owns durable files.
This specification owns resource enforcement and engine workspace behavior.

## Model and Ownership

The **execution environment** supplies an effective finite memory and CPU
allocation covering the tool, every native child and required processing IO.
The optional Web deployment uses the application's container allocation shared
with Rust, HTTP, SQLite and Library work. A standalone command uses its own
qualified environment; no service database, queue or task ID is required.
Limits and enforcement facts must be inspectable. Missing/unbounded allocation
refuses image processing, never silently invokes unrestricted host execution.

In the shared Web deployment, the service reserves its **control allowance**
for browsing, persistence and bounded supervision beside one heavy attempt.
The Web scheduler admits one such attempt at a time. Its lock is not a host-wide
lock: concurrent external commands are outside that promise, and their memory
must not be omitted when sharing the same enforced environment. Effective policy
must refuse capacity that cannot be established rather than claiming isolation.

An **attempt** owns captured input/state revision, a fresh native child, private
scratch and terminal outcome. Tool processing/validation/encoding stages are
sequential. Queued Web work retains bounded metadata, not decoded buffers.
Native children must terminate and scratch ownership settle before the owning
supervisor releases its capacity. Independent edit-directory locks protect state,
not all image execution in a workspace.

A **workspace plan** accounts for required live buffers, temporary workspace,
batch geometry and qualified runtime headroom for exact input geometry, stage
and bundle. The independent tool computes and validates it against the effective
allocation. It is not kernel enforcement or a promise every image fits.
The plan belongs to the attempt, not canonical saved intent. Policy changes
affect subsequent attempts; captured processing settings remain unchanged.

## Enforcement Boundary

The supported bundled Web deployment enforces its application's finite Linux
container memory/CPU limit under [Deployment](../../deployment/spec.md). The tool
and native descendants share it with Web/SQLite. No per-attempt cgroup, host
launcher or processing daemon is added. The failure boundary is shared: engine
OOM can stop the service. Serialized Web admission and resource planning reduce
risk; they do not prove service isolation or control external CLI concurrency.

A standalone environment must enforce an advertised finite allocation covering
native allocations and descendants. It may be the same qualified container
invoked directly or another supported bounded environment. Cooperative estimates,
process RSS and per-process address-space limits alone cannot prove that bound.
Unsupported enforcement reports unavailable execution policy before processing;
independent usability does not mean unrestricted execution.

Inputs, stated/decoded geometry and pixel limits are checked before large
allocations. A finite deadline bounds computation; thread policy covers native
libraries as well as the command process. Metadata inspection, validation and IO
use bounded buffers and belong to the same resource plan. Workspace input is
admitted once and reused, not copied from the Library every iteration.
External host OOM, administrator termination and unrelated workloads can still
stop the environment; observed failure evidence must preserve that distinction.

## Deployment Contract

[Local Photo Executor](execution.md) owns tool/native process lifetime.
The optional application image bundles permitted independent executables and
qualified assets. Their manifest/digests are checked per tool; missing assets
leave Library and file access usable. Service startup configuration controls Web
tool availability and deployment sizing, never request fields. The existing
`SLIPSTREAM_PHOTO_DEVELOPMENT` and `SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY` integration
settings do not become dependencies of standalone state/commands.

An operator sizes the shared application allocation for its control allowance
plus one supervised attempt using qualified measured peaks. Standalone callers
provide a qualified finite environment sized for the actual command/concurrency.
Execution reports effective host/allocation honestly rather than assuming a
server path or limit exists on the remote client's machine. Neither mode needs a
host launcher, processing socket, worker provisioning or separate daemon.

## Workspace Planning and Admission

Web queue acceptance and permission to start computation are separate checks.
Dispatch revalidates captured input, revision, bundle, resources and current
capacity. Standalone execution performs the same tool admission synchronously,
without a server queue. Untrusted metadata/geometry inspection is itself bounded;
stated dimensions must agree with decoded geometry before large allocation.

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

Each tool/input/parameter/output configuration needs qualified resource evidence.
[Processing Modules](design.md#explicit-materialization) owns the
standalone Film lower-bound refusal. Passing it does not qualify the whole
attempt; Preview and full-resolution Export require separate exact-bundle evidence.

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

The owning tool/supervisor distinguishes these outcomes before mapping them to
its structured local or Web result:

- capability or deployment policy unavailable;
- input or stage outside the qualified resource envelope;
- workspace plan exceeds the effective allocation;
- runtime allocation failure or a confirmed engine termination; and
- deadline, cancellation, engine error, failed cleanup, or unconfirmed child
  termination.

Exit 137 alone is not proof of OOM. Confirm a limit failure from native
termination evidence and the execution environment's observed events; preserve
uncertainty when evidence is missing. A caught allocation failure is a distinct
resource failure rather than proof that the kernel enforced a limit.

Failure/cancellation terminates all owned native descendants, reaps children and
settles private scratch before releasing the owning execution slot. Partial
outputs are never installed as final results; earlier complete files and saved
state remain intact. Unconfirmed termination stops affected heavy admission.

Service restart reconciles only its own receipts under
[Local Photo Executor](execution.md#restart). It neither cancels nor
adopts independent CLI work. Tool-local state/receipts reconcile interrupted
commands. Scratch cleanup requires established ownership and no live tool user;
startup cannot indiscriminately delete concurrent external work. Completed files
are independently indexed, but cannot be attributed to a run without exact
validated evidence. Policy changes do not rewrite completed output or intent.

## Evidence and Qualification

Record attempt/bundle identity, geometry, stage, effective allocation/enforcement,
workspace plan, timings, process evidence and cleanup outcome. Retain bounded
diagnostics through the owning receipt lifetime. Detailed source paths/image
contents do not enter public logs or Issues. The authorized workspace response
does expose its actual execution path/host; this is separate from diagnostic logs.
Errors explain affected work, preserved saved state/files and a useful next action.

Qualification must separately prove:

- standalone attempts stay within their qualified finite allocation, and one
  serialized Web attempt fits beside responsive Library reads in the shared
  container, with successful subsequent admission after cleanup;
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

Real-camera evidence is separate from simulated tests and ordinary repository
gates. One small image or one Export cannot establish the supported envelope.
Qualification must cover standalone enforcement as well as the shared Web mode;
the Web supervisor's lock cannot serve as evidence of global CLI serialization.

## Options

### Selected: Finite Tool Execution with Serialized Web Admission

The independent tool owns workspace planning and native children under its
qualified finite environment. Web may share one container limit and admit one
heavy command at a time. This avoids a second service and preserves standalone
use. Shared Web OOM remains an explicit accepted failure domain, not a claim of
per-attempt isolation. External concurrency requires its own capacity facts.

### Rejected: Per-Attempt Task Cgroups with a Host Launcher

Privileged launcher, Docker authority, dynamic cgroup management and a durable
launcher journal add an operational lifecycle unneeded by this product. A
standalone tool still needs qualified finite enforcement without that framework.

### Rejected: Python Checks or Address-Space Limits Alone

Cooperative checks cannot stop opaque native allocations. A process
address-space limit measures a different resource and does not aggregate
independent descendants. Neither replaces qualified finite environmental
enforcement and honest capacity admission.

## References

- [Local Photo Executor](execution.md)
- [Deployment](../../deployment/spec.md) for the container resource contract
- [Docker memory and swap constraints](https://docs.docker.com/engine/containers/resource_constraints/)
