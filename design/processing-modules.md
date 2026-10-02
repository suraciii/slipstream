# Composable Photo Processing Modules

A current-result Edit Preview and a materialized image serve different purposes.
Treating them as one pipeline can run an unselected engine, render a full-size
handoff during interactive editing, or silently change a later step's input.
Slipstream separates single-module execution from caller-controlled composition.

[Photo Development](../docs/photo-development.md) owns user-visible behavior.
[Photo Development Architecture](photo-development.md) owns guarded editing,
source resolution, Export lifecycle, and publication.
[Local Photo Executor](processing-executor.md) owns execution containment and
settlement. This document owns the module boundary. Discovery does not widen
the qualified input, parameter, output, or resource combinations admitted by
the selected module's verified bundle.

## Model and Ownership

A Processing Module owns its input contracts, parameter tree, output contracts,
and engine mapping. darktable and standalone SpektraFilm are peer modules.
SpektraFilm here means the standalone runtime, not a darktable image-operation
module. Neither peer has a required position in a Slipstream pipeline.

An Edit Recipe contains zero or more Processing Step records. Each record has
an opaque `step_id` unique within the Photo's current recipe, one selected
module, one input binding, and one complete parameter snapshot. The input
binding is either the guarded Original identity or an explicit immutable
Processing Artifact identity plus its image contract. A repeated module uses a
different `step_id`; two steps with different artifacts remain distinct even
when their module and parameters match.

The browser or programmatic caller selects one record as the current step.
Updating a step creates a new guarded recipe snapshot and Preview identity;
it does not mutate an accepted Export or a published artifact. A caller may
save zero steps, one step, or any finite set of individually admitted steps.
There is no predecessor, planner, or hidden ordering field: an artifact input
is the only composition edge. Slipstream does not accept or execute a workflow
graph.

A Processing Artifact is an immutable Export result. Its provenance identifies
its input, module, complete parameter snapshot, bundle, output contract, and
validated byte identity. A later step binds to that artifact, not to a mutable
"latest result" of an upstream step. Re-exporting upstream creates a new
artifact; selecting it downstream is a separate explicit action.

Slipstream owns Photo identity, saved intent, source guards, input leases,
serialized heavy-work admission, cancellation, deadlines, output validation,
and publication. A module owns only one invocation and its private intermediate
state. It cannot resolve Library paths, choose another module, publish an Export,
change an Original, or schedule a dependent step.

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
path. A description is not product admission; the selected step must still
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

The fixed standalone Film adapter publishes every complete recipe group as a
required property with its pinned `const` value in `parameterSchema`. Callers
construct the executable default tree from those values. Saving or executing a
tree with a missing group or changed recipe value is refused before engine work;
bundle verification also compares the runtime-emitted defaults with that same
pinned tree. Film input admission uses the concrete linear ProPhoto float32 TIFF
contract and the runtime's geometry bounds, including portrait handoffs, rather
than requiring the dimensions of a discovery example. Its advertised 900,000 ms
deadline is the same bound enforced for both Preview and Export; explicit caller
cancellation remains available.

These shapes are examples of module-owned schemas, not a second universal
parameter language. Module versions and qualified mappings belong to their
adapters and bundles.

## Preview and Export

### Bounded Current-Step Preview

An Edit Preview executes only the selected current step against its captured
input and parameters. It must not select another module to prepare input or
finish output. A missing compatible input is a refusal, not permission to run
an upstream step. Camera Preview cannot substitute for the result.

The Preview has an explicit finite geometry and resource bound. The adapter
must compute the selected module's result at that admitted geometry and may
apply a separately identified display conversion for delivery. It must not
first produce a full-resolution handoff and downsample it. A decoder may need
to read or decode full source data; bounded output geometry is not evidence of
bounded decode memory, and resource admission must cover that work independently.
Private module intermediates remain within the attempt's finite workspace.

The captured step parameters include the intended Export output contract.
Preview geometry is a disclosed rendition choice, not a silent edit to that
intent. When an adapter expresses Preview geometry through its parameter tree,
Slipstream freezes both the step snapshot and the exact bounded invocation
parameters. Only the qualified geometry/display derivation may differ from the
captured intent; it must not silently change precision, processing quality,
effects, randomness policy, color interpretation, or encoding defaults.
A Preview does not establish full-resolution spatial-detail equivalence.

Preview output is ephemeral and cannot become a later step's input, an Export,
or a downloadable full-resolution handoff. A completed Export may supply a
bounded display rendition for an exactly matching captured identity without
another module invocation. This reuse does not promote a Preview into an Export.

### Explicit Materialization

Export is a separate explicit execution of the selected step. Acceptance
captures the confirmed input identity, module, complete step parameters,
processing bundle, and intended output contract. It does not capture whichever
settings happen to be current when queued work starts. The finite Preview
geometry/display derivation is not exported as the intended full-size output.
If any captured processing or output intent has changed since the confirmed
result, the caller must confirm that new intent; it cannot be presented as an
Export of the earlier result.

Export reruns the selected module with the captured output parameters and
separately admitted output geometry. Preview success alone does not establish
full-resolution qualification or resource admission. Export must not upscale a
Preview, reuse display-only bytes as scene-referred input, implicitly develop a
RAW for another module, or silently reduce output dimensions to fit.

Before accepting a new standalone Film Export, the service must refuse a source
geometry whose known minimum live processing memory exceeds the effective
finite deployment allowance. An unavailable or unbounded memory allowance must also refuse
admission. Passing this lower-bound check does not establish complete-attempt
resource qualification. Retained Export receipts and artifacts remain readable
and replayable after the deployment's allowance changes.

Slipstream validates actual output type, geometry, precision, color/transfer
contract, metadata, byte size, and content digest before atomically publishing
a Processing Artifact. Publication and durable Export settlement follow
[Export Execution and Recovery](photo-development.md#export-execution-and-recovery).
Only a successful, validated, retained artifact can be selected by a later step.
Failure or cancellation does not publish a partial artifact. An unavailable or
expired input fails rather than resolving a newer upstream result.

## Identity, Compatibility, and Failure

Preview identity includes the exact input binding and byte evidence, selected
module and adapter/schema version, complete step parameter snapshot or canonical
digest, exact Preview invocation parameters when derived, Preview geometry,
processing bundle, and display conversion. An Original input also retains its
Photo and guarded source revision. Artifact input retains the immutable artifact
identity and concrete image contract. Proxy input retains its proxy identity
and provenance under the existing preview-only rules.

Latest-intent-wins ownership is scoped to the Photo and current step, with
comparison in a separate owner. Equal complete identities may coalesce.
Completion must compare both the active owner and full identity before
publication. Cancellation and ignoring stale output are separate duties; late,
failed, superseded, or wrong-module results cannot update a newer view.
An upstream edit invalidates previews of that edited step but never retargets a
downstream step already bound to a published artifact. Explicitly choosing a new
artifact invalidates that downstream step's Preview identity.

Compatibility is checked at each invocation. A shared extension or format name
is insufficient: the selected module must admit the actual color space, profile,
transfer function, sample precision, geometry, and parameter/output combination.
Concrete module color and sample contracts are owned by
[Development Color Pipeline](development-color.md).
A module that does not admit an artifact's actual contract must refuse it.
Slipstream must not insert a conversion module, silently reinterpret samples,
or convert through a display rendition. A caller may explicitly select an
admitted conversion or output configuration, Export its result, and use that
artifact in another step.

All invocations preserve read-only Originals, confined private staged inputs,
finite serialized execution, deadlines, and cancellation settlement. Module
failure leaves saved intent and earlier artifacts intact. A lost response is an
uncertain outcome, not proof of failure or permission to start a second attempt.
The existing receipt, reconciliation, cleanup, and retention owners remain
unchanged; composition does not introduce another scheduler or journal.

## Example

```text diagram
Original -> darktable Preview
         -> explicit darktable Export -> artifact a1
artifact a1 -> standalone SpektraFilm Preview
            -> explicit SpektraFilm Export -> artifact a2
```

The second step starts only after the caller selects compatible artifact `a1`.
Changing darktable parameters later does not change `a1` or the SpektraFilm step.
The caller may instead stop after the first Preview, use one Export, repeat
darktable on an admitted artifact, or select another admitted module. The diagram
is one scenario, not a fixed number or order of steps.

## Options

### Selected: Direct Invocations and Explicit Artifacts

Single-module calls keep validation and engine mappings local. Immutable Export
inputs make composition and failure ownership explicit without a workflow engine.

### Rejected: Fixed darktable-to-SpektraFilm Pipeline

A fixed pair invokes work the caller did not select and makes a Preview depend
on a full-size upstream handoff. It cannot represent repetition or a single step.

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
- explicit artifact handoff permits compatible repeated or different modules;
- changed input, parameters, geometry, bundle, or display conversion changes
  Preview identity, and out-of-order completions never replace current output;
- upstream edits preserve downstream artifact bindings and earlier Exports; and
- Original invariance, deadlines, cancel/complete races, uncertain outcomes,
  finite resource rejection, and publication recovery hold for every invocation.

Example and contract review do not qualify a module or change deployment
admission. Real-engine evidence remains scoped to the tested module, bundle,
input class, parameters, geometry, output contract, and execution environment.
