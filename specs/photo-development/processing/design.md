# Photo Processing Modules

[Photo Development Architecture](../design.md) owns tool integration and captured run receipts. [Photo Workspace](../workspace/design.md) owns files and their index. [Local Photo Executor](execution.md) owns supervision, and [darktable CLI](../darktable/cli.md) owns command grammar.

## Model and Ownership

A Processing Module is the service-internal boundary for one independent Processing Engine and its input, state, schema, resources, and output contracts. darktable and standalone SpektraFilm are peers, not a required sequence.

A tool MAY operate without Slipstream. Its state directories own editing intent and revisions; Slipstream indexes pointers and observed availability without duplicating mutable parameters. Separate directories represent independent edits, and selecting an edit or input MUST NOT rewrite state.

A compatible recipe surface MAY project tool state. Unsupported historical intent remains inspectable under the [cutover contract](../design.md#existing-service-state-at-cutover), not coerced into runnable state.

The Film adapter owns `.edits/spektrafilm/adapter/state.json` and MUST atomically retain its recipe and exact request replay receipts there. The compatibility recipe response projects a content-derived revision, so external changes invalidate stale guards.

Historical darktable and unsupported steps MUST be echoed unchanged. Omissions and generic writes to those steps MUST be refused. Explicit darktable migration reviews the complete retained parameter tree against its exact protected Original capture through the versioned importer, preserving historical identifiers and bytes.

The tool owns schema validation, local state locks and revisions, native defaults, resource versions, child lifetime, and file completion. Slipstream owns Photo/workspace resolution, indexing, access, and admission and receipts for runs it starts. File discovery requires no tool callback, registration SDK, Slipstream crate, or HTTP dependency.

A later tool MUST bind explicitly to one completed input revision, never an upstream latest result. Changes to upstream edits or new exports MUST NOT retarget that binding. A [Processing Artifact](../../../CONTEXT.md#development) requires verified invocation provenance and actual image-contract facts, not a suggestive folder or extension.

## Interface and Discovery

The file-based boundary supports inspecting a tool and selected input/state, mutating persistent state, and rendering captured state into the workspace. The service MUST invoke permitted executables with argument vectors, accessible absolute paths, and bounded structured results; it MUST NOT expose arbitrary shell execution. Standalone invocation accepts accessible caller-owned paths without Photo IDs or a server task lifecycle.

Read-only discovery MUST report tool, bundle, and schema identity; input/output contracts; finite limits; image-context initialization requirements; capability availability; and refusal reasons. Readable, writable, and executable capabilities are separate. A missing tool MUST NOT disable unrelated tools or browsing.

Introspection alone MUST NOT qualify execution. Actual input, parameter, output, and resource combinations MUST match the qualified bundle.

State and resources use versioned tool-owned semantic schemas. Unsupported fields or versions MUST remain inspectable and MUST be refused for execution rather than dropped. A changed engine or schema requires fresh qualification and MUST NOT reinterpret an old revision.

### Concrete Module Shapes

Darktable state retains ordered Operation Instances with stable edit-local IDs, native operation/version, enabled state, recursive parameters, legal order, and blend/mask references. Native stack indexes and `multi_priority` are not public identity. Complete schema values include arrays, structures, strings, curves, geometry, masks, actions, and output profile, precision, and encoding.

C layouts and native history remain private to the bridge. General editing MUST NOT be reduced to exposure and as-shot white balance; custom camera white balance remains independently qualified.

Read-only inspection MAY initialize a prospective baseline privately without saving an edit. Explicit open/create atomically installs an input-bound baseline. Partial mutations preserve omitted settings and validate the complete resulting relationships.

When multiple states exist, the caller MUST select the target explicitly. The tool MUST NOT infer the newest edit, newest artifact, or unrelated Web selection. Tool-local history and exact replay own state recovery for both Web and CLI.

[darktable Editing](../darktable/editing.md) owns Preset, Style, and resource semantics. Versions are immutable; applications distinguish fixed from target-initialized scope and merge/update from replace. Applications materialize edit-local instances and masks with typed reference mappings.

ICC, LUT, and raster-mask resources have immutable typed versions, bounded import validation, and content identities. Retained state and work protect dependencies; missing external bytes remain disclosed.

Standalone SpektraFilm retains grouped camera, enlarger, scanner, film/print, and input/output settings. Its qualified fixed configuration MUST retain the pinned complete required parameter groups. Missing groups are structurally invalid, changed fixed recipe values are refused for execution, and unsupported saved intent is preserved rather than substituted.

Runtime-emitted defaults MUST match the pinned parameter tree. Film admission uses actual linear ProPhoto float32 TIFF and qualified geometry, including portrait inputs, not discovery-example dimensions. Its advertised 900,000 ms deadline applies to both Preview and Export with explicit cancellation.

## Preview and Export

### Bounded Current-Step Preview

Preview MUST execute only the selected edit and captured revision with its explicit input. It MUST refuse missing compatible input and MUST NOT run another tool, perform a hidden full-size handoff, or substitute the camera Preview.

Preview geometry MUST be finite and qualified. Full-source decoding memory is admitted separately, and temporary intermediates remain private and bounded.

Preview records its exact qualified geometry and display derivation without changing saved Export precision, effects, quality, randomness policy, or output intent. Reduced simulation MUST NOT claim spatial pixel equivalence to a downsampled full-size result.

Comparison holds input, tool, geometry, bundle, and display conversion fixed while selecting initialized baseline or retained revision operands. Unavailable defaults MUST refuse comparison.

A completed Preview is a viewable and downloadable workspace image with rendition provenance. Downloadability alone MUST NOT establish full-resolution, high-precision, or scene-linear compatibility. Continue Processing follows the [workspace compatibility contract](../workspace/spec.md); an explicitly accepted reduced-quality flow MUST disclose its limits.

A completed Export MAY supply an exact matching bounded display rendition without another engine invocation. The camera Preview remains a separate reference.

The standalone Film adapter's completed Preview receipt establishes its bounded PNG display contract only after exact byte and decoded geometry checks. Generic PNG discovery alone MUST NOT establish color or downstream compatibility.

Its Finished JPEG additionally requires the pinned embedded sRGB profile. Publication retains the adapter's exact output contract, including writer encoding, rather than generic file-discovery labels.

### Explicit Materialization

Export captures confirmed input byte identity, canonical state revision and parameter digest, bundle/schema, resources, and intended output contract. Later mutations MUST NOT alter that capture. Standalone Export is synchronous; Web MAY queue and supervise the same executable with its own receipt.

A successful bounded Preview MUST NOT establish full-resolution qualification or capacity. The tool MUST revalidate input stability and full-size resource admission before processing.

The tool MUST validate actual output type, dimensions, precision, color/transfer, metadata, bytes, and digest. Publication follows the atomic no-replace and failure rules in [Export execution](../design.md#export-execution-and-recovery). Generic workspace discovery needs no callback, manifest, or commit; validated structured completion MAY enrich the index with Processing Artifact provenance.

For standalone Film, a known minimum live processing memory above the effective finite allowance MUST refuse admission before expensive work. A missing or unbounded allowance MUST also refuse.

Passing this lower bound does not establish full resource qualification. Completed files and retained receipts remain inspectable after resource policy changes.

## Identity, Compatibility, and Failure

Invocation identity includes input content evidence, edit revision and complete parameter digest, tool/bundle/schema, resource versions, exact geometry/settings, and display transform. Photo identity and source revision are provenance of the initial snapshot, not a live dependency after stable bytes are installed. Original refresh MUST NOT retarget existing state.

Browser canvas ownership compares full identity rather than completion order. Old revisions become stale, not relabeled current.

Concurrent completed files can coexist; only exact validated invocation/file evidence settles a supervised run. Timestamp or newest filename MUST NOT attribute an unmanifested external output.

Compatibility checks actual color/profile, transfer, precision, geometry, and qualified settings rather than extension. [Development Color Pipeline](color.md) owns concrete image contracts.

The service MUST NOT insert implicit conversion or rerun upstream work. A caller MAY explicitly render a compatible output and select it as a new input.

Changed input bytes invalidate old evidence and require explicit rebind. Input MUST NOT be implicitly copied on each iteration.

Invocations preserve read-only Original/XMP and bound input under finite memory, CPU, thread, and disk limits, deadlines, and cancellation. Uncertain local actions reconcile through exact request replay rather than inferred visible settings. [Local Photo Executor](execution.md#restart) owns service-run reconciliation and independent process lifetime.
