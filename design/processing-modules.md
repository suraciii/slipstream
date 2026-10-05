# Photo Processing Modules

An Edit Preview and a materialized image serve different purposes. Slipstream
must run exactly the selected Processing Engine against the selected input and
publish only validated Export output. This boundary is about one invocation
and Artifact transfer; it does not create a user-visible processing pipeline.

[Photo Development](../docs/photo-development.md) owns user-visible behavior.
[Photo Development Architecture](photo-development.md) owns guarded editing,
source resolution, Export lifecycle, and publication.
[Local Photo Executor](processing-executor.md) owns execution containment and
settlement. This document owns the Engine boundary. Discovery does not widen
the qualified input, Control, output, or resource combinations admitted by a
verified bundle.

## Model and Ownership

A Processing Module is the service-internal adapter boundary for one Processing
Engine. darktable and standalone SpektraFilm are peer Engines. SpektraFilm here
means the standalone runtime, not a darktable image-operation module.

An Edit State contains the current Engine, explicit input identity, qualified
Controls, and guarded edit revision. A service may persist a complete internal
snapshot of that state for recovery and replay. It must not expose that
snapshot as a second daily editing object.

One invocation selects one Engine, one input, and one captured set of Controls.
The Engine owns its parameter mapping and private intermediate state. Slipstream
owns Photo identity, saved Edit State, source guards, input leases, serialized
heavy-work admission, cancellation, deadlines, output validation, and
publication. The Engine cannot resolve Library paths, choose another Engine,
publish an Export, change an Original, or schedule a dependent invocation.

A Processing Artifact is an immutable Export result. Its provenance identifies
the input, Engine, concrete Controls, bundle/schema, output contract, and
validated byte identity. Another service may consume the Artifact as an
explicit input and owns its own Edit State. A later Export creates a new
Artifact; no mutable latest-result relationship is implied.

## Agent Stateful Surface

The Agent-facing Edit State is the current service state, not a projection over
a complete parameter snapshot. Stateful edits use one canonical Control grammar
and the Engine-owned mapping. The current MVP qualifies only
darktable.exposure.ev in the 0..=1 EV range.

Discovery publishes Controls with their value schema, defaults, reset values,
readability, editability, executability, and refusal reason. A discoverable
Control is not automatically executable: white balance, color calibration,
highlight recovery, and mutable SpektraFilm Controls remain explicit refusals
until their native mapping and qualification evidence are complete. An
unsupported request leaves the retained Edit State unchanged.

Complete native parameter trees, compatibility snapshots, and legacy
processing-recipe routes remain outside the normal Agent surface. They may be
retained during migration, but they do not define the product model.

## Interface and Discovery

The module boundary has one execution operation and one discovery operation:

```text literal
run(module, input, parameters) -> result | error
describe(module) -> module-description | error
```

`describe` is read-only. It returns only the selected module's bounded
description: admitted input and output contracts, the module-owned parameter
schema and versions, finite limits, and current availability/refusal reasons.
It does not return engine history, catalog state, executable data, or a host
path. A description is not product admission; the requested invocation must still
pass the service's source, bundle, qualification, and resource guards.

`run` selects one module and one input. The input is the confined read-only input
provided by Slipstream, with its verified identity and concrete image contract;
it is not an arbitrary client pathname. `parameters` is the selected module's
complete, bounded, versioned parameter tree. Output format, precision, color
space, transfer function, geometry, and encoding options are ordinary parameters
of that tree. There is no additional output argument or host-wide image-option
schema. `result` identifies one private completed output and its actual image
facts, or a module-specific refusal/failure. It is not a published artifact.
Slipstream validates it before delivery or publication.

Discovery describes each module's supported inputs, outputs, parameter schema,
defaults, versions, finite limits, and availability with refusal reasons.
Availability belongs to each module: for example, darktable may be ready while
SpektraFilm is unavailable. There is no separate Film capability. Input support,
qualified parameter combinations, and current resource admission are distinct
from engine availability; a ready module does not imply that every Photo or
invocation is admitted.

The host must preserve module-owned trees, not flatten them into a shared map,
merge schemas across modules, or discard unknown fields to obtain a runnable
request. The adapter validates its own schema and the complete combination
before computation. Unknown or unsupported versions and combinations fail
explicitly. A discoverable engine control does not grant product admission:
Slipstream admits only the input, parameter, output, and resource combinations
qualified for the selected bundle. Neither discovery nor dynamic parameters
permit arbitrary engine history, catalog access, commands, or executable data.
Discovery and execution use the same pinned adapter/schema identity; a changed
bundle requires a fresh availability and admission check.

### Concrete Module Shapes

For darktable, ordered image-operation instances remain an ordered stack.
An entry preserves `operation`, `multi_priority`, `enabled`, and structured
`params` or a versioned parameter blob. Parameter version and explicit
`before`/`after` ordering constraints remain module-owned when supported.
Repeated operations must not collapse into one key. Native introspection may
describe recursive structs, arrays, enums, and scalars without making every
operation or blob admissible. The native integration boundary selects the
qualified subset; this module contract does not expose catalog or history tools.

Standalone SpektraFilm retains its runtime's grouped parameters, including
camera, enlarger, scanner, film/print rendering, and input/output settings.
Those groups are not darktable stack entries. The adapter uses the pinned
standalone simulation API and validates its own input/output encoding and
spatial/stochastic behavior. No darktable operation is inserted to emulate
standalone SpektraFilm.

The fixed standalone Film adapter publishes every complete configuration group as a
required property with its pinned `const` value in `parameterSchema`. Callers
construct the executable default tree from those values. Missing required groups
are structurally invalid; execution with changed recipe values is refused before
engine work. Structurally valid unqualified values remain saved intent. Bundle
verification also compares the runtime-emitted defaults with that same
pinned tree. Film input admission uses the concrete linear ProPhoto float32 TIFF
contract and the runtime's geometry bounds, including portrait handoffs, rather
than requiring the dimensions of a discovery example. Its advertised 900,000 ms
deadline is the same bound enforced for both Preview and Export; explicit caller
cancellation remains available.

These shapes are examples of module-owned schemas, not a second universal
parameter language. Module versions and qualified mappings belong to their
adapters and bundles.

## Preview and Export

### Bounded Edit Preview

An Edit Preview executes exactly one current Edit State invocation against its
captured input and Controls. It must not invoke another Engine, prepare an
upstream input, or produce a full-resolution handoff. A missing compatible
input is a refusal. Camera Preview cannot substitute for the requested result.

The Preview has a finite geometry and resource bound. The adapter computes the
selected Engine's result at that admitted geometry and may apply a separately
identified display conversion for delivery. It must not silently change
precision, processing quality, effects, randomness policy, or encoding
defaults. A Preview does not establish full-resolution output equivalence.

Preview identity covers the input identity, Engine, Control values, rendition
geometry, bundle, schema, and display conversion. Preview output is ephemeral:
it cannot become a later service input, an Export, or a downloadable
full-resolution handoff. A later accepted edit makes the previous Preview
stale. Late, cancelled, or obsolete results must not replace the current view.

### Explicit Export

Export is a separate explicit execution of the current Edit State. Acceptance
captures the confirmed input identity, Engine, concrete Controls, processing
bundle, and intended output contract. It does not capture whichever settings
happen to be current when queued work starts.

The Engine reruns the captured invocation with the admitted output contract.
Slipstream validates actual type, geometry, precision, color and transfer
contract, metadata, byte size, and content digest before publishing one
immutable Processing Artifact. Only a complete validated Artifact can be
handed to another service. Export must not upscale a Preview, reuse display
bytes as a scene-referred input, invoke another Engine, or silently reduce
output dimensions to fit.

The Artifact provenance identifies the input identity, Engine, Controls,
bundle/schema, output contract, request identity, and validated byte identity.
The receiving service validates the concrete image contract before creating its
own Edit State. It does not read or mutate the upstream Edit State.

Export state is queued, running, succeeded, failed, or cancelled. Accepted
work survives browser departure and restart under the existing durable
settlement rules. A lost response is an uncertain outcome; the client replays
the exact request identity and does not start a second invocation.

A module that does not admit the input's actual contract must refuse it.
Slipstream must not insert an implicit conversion, silently reinterpret samples,
or use a latest result in place of an explicit Artifact. Module failure leaves
the confirmed Edit State and earlier Artifacts unchanged.

## Options

### Selected: Current Edit State and Artifact Transfer

Single-Engine calls keep validation and engine mappings local. An immutable
Export Artifact makes service handoff and failure ownership explicit without
requiring a workflow engine or cross-service recipe.

### Rejected: Persistent Cross-Service Composition

A cross-service chain would make Slipstream own downstream dependencies,
branching, reruns, and migration of a plan that the product does not promise.
The Artifact contract already transfers the materialized result.

### Rejected: Fixed darktable-to-SpektraFilm Pipeline

A fixed pair invokes work the caller did not select and makes one service own
another service's lifecycle. Each service must remain independently callable.

### Rejected: Workflow DSL and Implicit Conversion Registry

A graph planner adds ordering, conversion, partial-execution, and recovery
semantics not needed for caller-selected invocations. A common raster hierarchy
would also obscure the concrete color and precision contracts already required.

## Verification

Module implementations must exercise these consumer-visible boundaries:

- discovery preserves each module's schema and reports independent availability;
- described but unqualified controls cannot cross product admission;
- unsupported input/parameter/output combinations fail before computation;
- bounded Preview invokes one module and produces no full-size handoff;
- Export is separately admitted and publishes only validated complete output;
- explicit Artifact transfer preserves the receiving service's input contract;
- changed input, parameters, geometry, bundle, or display conversion changes
  Preview identity, and out-of-order completions never replace current output;
- upstream edits preserve completed Artifact bindings and earlier Exports; and
- Original invariance, deadlines, cancel/complete races, uncertain outcomes,
  finite resource rejection, and publication recovery hold for every invocation.

Example and contract review do not qualify a module or change deployment
admission. Real-engine evidence remains scoped to the tested module, bundle,
input class, parameters, geometry, output contract, and execution environment.
