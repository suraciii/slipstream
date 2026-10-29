# Processing Memory

This contract owns finite resource enforcement and engine workspace planning without changing the photograph. [Photo Development Architecture](../design.md) owns captured invocation and supervised receipts. [Photo Workspace](../workspace/design.md) owns durable files, and [Local Photo Executor](execution.md) owns process lifetime.

## Model and Ownership

The execution environment MUST supply an effective finite memory and CPU allocation covering the tool, native descendants, and required processing IO. The bundled Web deployment shares its application container allocation with service, HTTP, SQLite, and Library work. A standalone command uses its own qualified allocation without a service database, queue, or task ID.

Limits and enforcement facts MUST be inspectable. Missing or unbounded allocation MUST refuse image processing rather than invoke unrestricted host execution.

The Web service reserves a control allowance for browsing, persistence, and bounded supervision beside one heavy attempt. Its one-at-a-time admission does not serialize external commands. When external commands share the enforced environment, their memory MUST be accounted for; capacity that cannot be established MUST be refused rather than described as isolated.

An attempt owns a captured input/state revision, fresh native child, private scratch, and terminal outcome. Processing, validation, and encoding stages are sequential.

Queued Web work retains bounded metadata, not decoded buffers. Capacity is released only after native termination and scratch settlement under the [executor contract](execution.md#execution).

A workspace plan accounts for live buffers, temporary workspace, batch geometry, and qualified runtime headroom for the exact input geometry, stage, and bundle. The tool computes and validates the plan against the effective allocation. A plan is neither kernel enforcement nor a promise that every image fits.

The plan belongs to the attempt, not saved intent. Policy changes affect subsequent attempts and MUST NOT change captured processing settings, developed dimensions, precision, or Film Recipe.

## Enforcement Boundary

The bundled Web deployment enforces finite Linux container memory and CPU limits under [Deployment](../../deployment/spec.md). The tool and descendants share this failure boundary with Web and SQLite; engine OOM can stop the service. Serialized admission and planning reduce risk but do not prove service isolation or control external CLI concurrency.

A standalone environment MUST enforce an advertised finite allocation covering native allocations and descendants. It MAY use the same qualified container directly or another supported bounded environment.

Cooperative estimates, process RSS, and per-process address-space limits alone cannot prove that bound. Unsupported enforcement MUST report unavailable execution policy before processing.

Inputs, stated and decoded geometry, and pixel limits MUST be checked before large allocations. A finite deadline bounds computation, and thread policy covers native libraries and the command process. Metadata inspection, validation, and IO use bounded buffers and belong to the resource plan.

External host OOM, administrator termination, and unrelated workloads can stop the environment. Failure evidence MUST preserve that distinction.

## Deployment Contract

The optional application image bundles permitted independent executables and qualified assets. Manifest and digest validation, tool availability, and startup policy follow [Local Photo Executor](execution.md#model). `SLIPSTREAM_PHOTO_DEVELOPMENT` and `SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY` MUST NOT become standalone state or command dependencies.

The operator sizes the shared allocation for its control allowance plus one supervised attempt using qualified measured peaks. Standalone callers provide a qualified finite environment sized for actual command concurrency. Execution reports effective host and allocation honestly rather than assuming a server path or limit exists on a remote client.

## Workspace Planning and Admission

Queue acceptance and permission to start computation are separate checks. Dispatch MUST revalidate captured input, revision, bundle, resources, and current capacity. Standalone execution performs tool admission synchronously without a server queue.

Untrusted metadata and geometry inspection MUST be bounded. Stated dimensions MUST agree with decoded geometry before large allocation.

Each stage plan MUST account for:

- Simultaneously live inputs and outputs.
- Library and thread workspace.
- IO and file-cache costs.
- Runtime headroom.

Batch workspace MUST fit in the remaining allocation. Sequential stages use the maximum stage requirement; buffers carried across stages are included in each relevant stage. Byte calculations MUST use checked arithmetic.

Batch geometry MUST be selected from the pinned bundle's qualified range and fit the plan. Its range and overhead model require qualification across input content, aspect ratios, thread settings, and fresh processes.

Current free RAM or one RGB buffer per pixel is insufficient. The supervisor MUST validate returned plans against its own limit and the qualified bundle.

If live data and minimum workspace do not fit, execution MUST fail before the expensive stage with a resource-budget reason. A missing resource model means unqualified capability, not permission to guess. Preflight does not guarantee success; unexpected allocation failure or kernel OOM requires supervised settlement.

OOM MUST NOT trigger an automatic retry with the same settings or a raised budget. An explicit retry creates a new attempt under current policy.

Every tool/input/parameter/output configuration requires qualified resource evidence; Preview and full-resolution Export require separate exact-bundle evidence. [Processing Modules](design.md#explicit-materialization) owns the standalone Film lower-bound refusal.

## Engine Memory Efficiency

### Bounded Pointwise Computation

Pointwise output-gamut conversion MUST use bounded batches and write to one owned destination buffer. It MUST preserve pixel order, the upstream mathematical function, viewing conditions, profile assets, and qualified float precision. Batch-size changes MUST produce the same pre-encoding pixels throughout the supported envelope; budget-dependent image changes fail qualification.

The plan MUST declare admitted array layout. Flattening or conversion of a strided array MUST NOT make an unbudgeted full-frame copy: validate a qualified contiguous layout, budget the normalization copy, or use bounded traversal. Supported non-contiguous inputs require bounded behavior; unsupported ones require explicit rejection.

The initial adapter admits only pinned CAM16-UCS compression settings, viewing conditions, and the sRGB output profile. It admits nonempty C-contiguous native float64 RGB arrays shaped `(..., 3)`. Other settings, output spaces, dtypes, empty inputs, and strided layouts MUST be rejected before destination allocation, without an unbounded fallback.

Flattening MUST remain a view. One owned destination requires 24 bytes per pixel, and caller-owned input remains unchanged, including read-only input. The caller reserves the destination and all other live stage data separately from temporary transform workspace.

A versioned model maps a positive workspace byte allowance to a qualified batch size. Its fixed term covers cold color-table construction; its per-pixel term covers simultaneous transform temporaries including the returned batch.

An allowance below the one-pixel requirement MUST fail before image allocation. The attempt planner separately accounts for library headroom, input, and destination.

Eligible transfer-function encoding and numeric output conversion follow the same bounded principle. Buffer fusion or reuse MUST preserve operation order, rounding, aliasing, and downstream ownership. IO buffers and the final JPEG encoder remain part of the peak.

Delivery uses a reviewed pinned engine change or narrow versioned adapter with reproducible patch identity, not a runtime monkey patch.

### Buffer Lifetime

An intermediate MUST be released after its last consumer on the actual execution path while retaining the requested collected result. Early collection, intermediate injection, fan-out, and fan-in MUST preserve their semantics. All references, including caller variables, stage locals, views, debug taps, and caches, participate in lifetime accounting.

Caches MAY retain bounded profile/LUT data but MUST NOT retain full-frame intermediate arrays across attempts. Process exit is the final reclamation boundary; reduced live bytes do not imply an equal immediate RSS reduction.

### Ordered Lifetimes and Output Encoding

The lifetime plan MUST mirror the declared-order topology walk, including skipped nodes, overwritten tap names, callbacks, and its first post-node collection check. Planning uses tap metadata, not image values. Earlier tap values remain needed until their last consumer before replacement.

Expired dispatcher references MUST be released before the next node starts. The requested collection MUST be retained before its storage is pruned. Aliased views retain backing arrays, and dropping an internal reference neither transfers nor revokes borrowed caller input.

The runtime MUST reclaim cyclic traceback frames owning completed stage arrays after dispatcher references expire at each handoff, before the next allocation. On return it retains the collected result, drops remaining dispatcher ownership, reclaims cycles, and then returns.

It MUST NOT alter process-global GC enablement or thresholds. Whole-run measurements include reclamation cost; node timings MAY exclude it when reclamation is reported separately.

Same-profile display encoding MUST batch the exact qualified color-library operation, including matrix and transfer-function order. It MUST NOT replace that operation with a superficially equivalent formula. It uses the nonempty contiguous native float64 layout defined for output-gamut conversion.

Finished JPEG conversion admits native float32 or float64 RGB and normalizes strided layouts only within the current row batch. Empty or unsupported input MUST be rejected before opening the output file. Conversion preserves clipping, scaling, and integer truncation.

Encoding writes consecutive full-width row batches using bounded numeric workspace while retaining source samples and identical encoder settings, ICC, and geometry. The numeric allowance MUST admit at least one row before file creation.

Native encoder storage is a separate stage requirement; scanline IO does not prove constant codec memory. Failed open, write, or close MUST fail Export without publishing a partial artifact.

Finite-value checks and pixel identity hashing MUST use bounded chunks. Pixel digests retain exact C-order sample bytes, including non-contiguous inputs through bounded traversal.

### Spatial and Stochastic Operations

Grain, halation, diffusion, blur, and sharpening MUST retain qualified image semantics and MUST NOT be divided into independent tiles under the pointwise rule. Spatial tiling requires explicit boundary/overlap treatment, filter support, physical pixel scale, and random-state ownership, with seam and complete-image comparisons. Precision reduction and memory-mapped spill require separate quality and storage evidence.

## Failure, Cancellation, and Recovery

The owning tool or supervisor distinguishes these outcomes before mapping its structured local or Web result:

- Capability or deployment policy unavailable.
- Input or stage outside the qualified resource envelope.
- Workspace plan exceeds the effective allocation.
- Runtime allocation failure or confirmed engine termination.
- Deadline, cancellation, engine error, failed cleanup, or unconfirmed child termination.

Exit 137 alone is not proof of OOM. Limit failure requires native termination evidence and observed environment events; missing evidence preserves uncertainty. A caught allocation failure is a distinct resource failure, not proof of kernel enforcement.

Failure and cancellation follow the termination, scratch, and capacity rules in [Local Photo Executor](execution.md#execution). Partial outputs MUST NOT become final results; saved state and earlier files remain intact. Service restart and scratch cleanup follow its [restart contract](execution.md#restart).

## Evidence and Qualification

Attempts record bundle and attempt identity, geometry, stage, effective allocation and enforcement, workspace plan, timings, process evidence, and cleanup outcome. Bounded diagnostics are retained for the owning receipt lifetime. Detailed source paths and image contents MUST NOT enter public logs or Issues; an authorized workspace response separately exposes its actual execution path and host.

Errors explain affected work, preserved state and files, and a useful next action. Qualification MUST cover the exact packaged deployment, standalone enforcement, and shared Web mode. The Web supervisor's lock MUST NOT serve as evidence of global CLI serialization.

Qualification MUST establish that supported configurations preserve their image, topology, input-ownership, and reproducibility contracts across their qualified resource envelope. Resource planning MUST cover real-camera Preview and full Export separately, including their dimensions, effects, profiles, and encoding. Passing a synthetic small-image check or a single Export does not establish the supported envelope.

Qualification MUST establish bounded execution, preserved publication and source safety, successful cleanup, and subsequent admission in the applicable environment. Low budgets fail predictably; supported budgets succeed without changing pre-encoding pixel identity. Observer or harness changes MUST be reported separately when comparing aggregate peaks.

Shared Web qualification MUST establish that one serialized attempt fits beside responsive Library reads. Qualification MUST include cold and warm process peaks, time, threads, IO, and browse contention against a common idle baseline and declared thresholds.
