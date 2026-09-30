# Native darktable Integration

Slipstream must not duplicate darktable's parameter layouts, module
initialization, or history construction for each engine module. That knowledge
belongs with the engine. The service must still own semantic editing intent,
source authority, qualification, and result publication.

[darktable Integration](../docs/darktable-integration.md) owns observable product
behavior. [Photo Development Architecture](photo-development.md) owns recipes,
source guards, work admission, and publication.
[Development Color Pipeline](development-color.md) owns the processing baseline
and image contract. This document owns the interaction with the native engine.

## Model and Ownership

The dependency is one-way: Slipstream calls darktable. darktable does not call the
service to resolve Photos, save recipes, or publish results. It is an executor,
not a separate bounded context that owns product state.

The Rust service owns the Edit Recipe and captured processing snapshots. A
recipe contains semantic exposure and white-balance intent and the fixed Film
Recipe reference, not a darktable history, parameter blob, catalog image ID,
or module ordering value. The existing recipe model remains unchanged.

The Photo worker owns one generic native MCP client and the mapping of admitted
semantic controls to engine operations. That mapping may name operations and
fields, but must not reconstruct C layouts, write XMP, or edit darktable SQLite
history. Complete automatic engine discovery does not imply automatic mapping
of every module to a product control.

The native darktable bridge owns parameter reflection, image-context
initialization, parameter application, module instances, version checking,
history construction, and pixelpipe execution. The engine owns the native
algorithms and legal module ordering. A baseline policy selects technical
processing and suppresses unrequested artistic defaults under the color spec.

The processing bundle binds the exact engine source commit and worker image,
MCP contract, parameter schemas, semantic mappings, baseline policy, and color
assets. A discovered schema is engine metadata, not permission to execute it.
A deployed capability admits only qualified combinations. Discovery does not
loosen the closed [Photo processing protocol](processing-photo-protocol.md).

This engine cutover must preserve the closed execution payload's qualified
exposure range and as-shot white balance. Stored temperature/tint intent remains
readable but processing-unavailable under that protocol. The product's adjustable
white-balance target does not grant execution authority: admission requires its
independent camera mapping qualification and an explicit update of the governing
Photo processing protocol and operator checks. Native image initialization must
support inspecting actual defaults without granting an unqualified custom mode.

## Interaction

```mermaid
sequenceDiagram
    participant S as Slipstream service
    participant W as Isolated Photo worker
    participant D as Native darktable-mcp
    Note over W,D: Bundle construction and qualification
    W->>D: Discover modules, parameter schemas, and capabilities
    D-->>W: Complete versioned engine metadata
    Note over S: Admit qualified semantic controls for the bundle
    Note over S,D: One processing attempt
    S->>W: Captured semantic intent and staged source via existing executor
    W->>D: Initialize staged image under the pinned baseline
    D-->>W: Image-context values and available module instances
    W->>D: Apply complete captured intent and request an output
    D-->>W: Artifact or explicit failure
    W-->>S: Validated worker outcome through existing executor
    Note over S: Apply source and freshness guards, then publish
```

The worker starts darktable-mcp as a private child process and exchanges MCP
messages over stdio. It must use a deterministic typed client, not an LLM. The
native MCP interface is not an HTTP endpoint, a service exposed to clients, or a
replacement for the host launcher's private socket. Tool availability and tool
metadata must be checked against the pinned contract before execution.

Only the worker may pass its attempt-local paths to darktable. The browser and
CLI address Photos through Slipstream; the service and launcher retain the
source-descriptor authority defined by the existing processing protocol.

## Discovery and Parameter Contract

Bundle construction must query the actual engine for module identities,
parameter versions, structures, and supported execution operations. The
resulting bounded metadata must be bound to that bundle and available to the
service without launching an engine for every UI read. A runtime mismatch with
the approved metadata must fail the affected capability.

The bridge must expose a recursive parameter representation with unambiguous
field identity. Arrays must retain dimensions and element types; structures
must retain nesting; supported strings must expose their capacity and encoding
constraints. Scalar metadata must cover numeric ranges, boolean types, enum
symbols and values, and available field and enum descriptions. Unsupported
parameter kinds must be explicit and must not disappear from a seemingly
complete schema. Schema, decode, and apply must use the same representation.

An image-context read must provide actual initialized values, reset defaults,
module instance identities, and ordering information needed by the worker.
Static introspection defaults must not stand in for camera initialization.
Omitted fields in a partial parameter update must preserve the initialized or
current values. Inspection must not change service-owned editing intent.

All input must be validated before mutation: correct JSON types, finite and
representable numbers, exact integers, enum membership, string limits, array
shape, known fields, and valid stack structure. The bridge must reject invalid
input rather than coerce, truncate, clamp, or ignore it. The worker must not
reimplement a second parameter validator based on C layouts.

Binary parameters, when used privately, must carry an explicit source parameter
version. Current-version size checks remain required but do not prove version
compatibility. This boundary must reject non-current versions explicitly; it
must not reinterpret equal-sized older layouts. Desktop history import is not
part of this contract, so automatic legacy-blob migration is not required.

## Execution and State Lifetime

Each attempt must use fresh engine configuration and an attempt-private catalog
in the existing supervised container. The engine receives only the staged
source and bundle assets, never the Library Folder or the service state store.
Configuration must suppress ambient XMP loading, sidecar writing, desktop
presets, and display dependencies. Local catalog/cache writes are allowed;
Original File writes are not. `--read-only` alone is not the containment proof.

The worker must map the captured semantic intent against the qualified baseline,
not against history left by a previous request. As-shot white balance and other
camera-dependent initialization must be obtained from the staged Photo.
[Development Color Pipeline](development-color.md#raw-development) determines
which initialized modules and corrections may execute.

The bridge must create an absent requested instance when the module supports
multiple instances and the approved request requires it. It must preserve
instance identity and enforce darktable's legal ordering; unsupported instances
or moves must fail. The public product does not expose arbitrary ordering.

Recipe application and output generation must share one preparation path.
Export must accept the same applied intent as preview without requiring an
intermediate PNG render or a persistent catalog commit. Output size and display
conversion remain distinct requests, not distinct correction semantics. An
8-bit MCP render must not become the Development TIFF or Spektrafilm source.

Before applying a request, the bridge must validate the complete request against
an isolated development state. Failure must leave the prior request state
unchanged, including auto-initialized history and module instances; no successful
artifact may be reported for partially applied intent. A failed attempt must
not be reused for another Photo or request.

After a successful output, the worker returns through the existing executor
contract. The service remains the only publisher and must apply the existing
source, recipe, and attempt guards. Cancellation must terminate the engine child
with the attempt; process exit or EOF before a complete result is a failure, not
an empty success. Private catalogs, histories, and blobs are discarded with the
attempt, not synchronized into durable recipe state.

## Integration With Existing Workloads

The native bridge replaces development execution, not Spektrafilm or local
proxy transforms. Original-backed development and baseline Development Proxy
construction must use the same qualified darktable baseline. Proxy construction
uses the existing preview-class `development-tiff` admission with zero exposure,
as-shot white balance, and an opaque preview attempt identity. It does not create
a user Export. The service derives the bounded proxy from the validated
scene-linear output, applies the existing source guards, and releases the attempt
artifact. A missing Original cannot build a new proxy. The fixed Film stage
consumes the validated scene-linear result under the color contract.

[Development Proxy](photo-development.md#development-proxy) remains authoritative
for local exposure and proxy-backed Film. No darktable invocation or new module
capability may be implied by that local transform. A future admitted control
must specify whether a proxy can represent it before processing that control.

At production cutover, the worker must remove manual binary parameter packing,
XMP-history construction, and post-import SQLite validation from its development
path. Every production caller must use the native boundary. An engine failure
must not fall back to the retired adapter. Engine qualification references may
retain independent CLI comparisons, but must not become a production fallback.

No route or wire value is renamed by this design. A future protocol extension
must update its authoritative spec, every caller, and deployment operator
verification before admission. Engine or schema upgrades require a new qualified
bundle; unsupported saved semantic intent remains readable but not executable.

## Options

### Selected: Native MCP Inside the Existing Photo Worker

A thin fork of darktable's native MCP exposes engine-owned introspection and
execution through one generic client. It concentrates layout and lifecycle
knowledge beside the engine while preserving existing process and deployment
boundaries. Native bridge fixes and a pinned source build are the maintenance
cost. The service owns a small semantic mapping, not per-module codecs.

### Rejected: CLI With a Generic XMP Generator

CLI execution preserves process isolation, but an external generator still
owns binary layouts, real defaults, history construction, and version handling.
It cannot remove the demonstrated knowledge duplication merely by making the
XML wrapper generic.

### Rejected: Shared Persistent darktable Service or Desktop Catalog

A shared catalog introduces cross-request state, image identity translation,
reset coordination, and additional service lifecycle. No demonstrated need
justifies it. Fresh attempts reuse the established executor boundary.

## Verification

Independent review must derive the discovery and attempt interaction from these
specs. Contract coverage must exercise malformed types and stacks, equal-sized
wrong-version blobs, nested array/string round trips, actual image defaults,
second-instance creation, legal ordering refusal, and failure atomicity.

An end-to-end scenario must save exposure intent through the service, request
an Export before any preview, reopen the Photo, and request a matching Edit
Preview. It must prove guarded persistence and correction equivalence against
a reference engine run, not pixel equality between different output encodings.

Representative engine probes must cover scalar exposure, RAW as-shot camera
coefficients, RGB curve nodes, a string field, and a second exposure instance
without Slipstream-side binary layouts. These probes do not grant product support
to curve, string, or multi-instance controls.

Real supported RAW fixtures must qualify baseline interpretation, full-size
float32 TIFF, exact ICC identity, orientation, negative/over-range preservation,
and Film handoff under the color spec. Success, invalid input, engine failure,
cancellation, and restart must prove unchanged Original and external XMP bytes,
no partial recipe saves, no stale publication, and settled executor outcomes.
The deployed host's operator verification must exercise the new worker bundle.
Synthetic TIFFs, discovery success, and a compilation pass alone are insufficient.
