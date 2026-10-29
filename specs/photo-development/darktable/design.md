# Native darktable Integration

The native bridge keeps parameter layouts, image defaults, legal ordering, and history construction beside darktable. An independent file/state tool owns durable editing intent; Slipstream supervises it without translating native parameters or maintaining a second edit database.

[darktable Editing](editing.md) owns semantic behavior. [darktable CLI](cli.md) owns public invocation and compatibility. [Photo Development Architecture](../design.md) owns workspace integration and service-state conversion; [Development Color](../processing/color.md) owns image contracts.

## Model and Ownership

The dependency is `Slipstream -> darktable-edit -> native darktable`. The tool MUST operate on accessible input and state paths with its own qualified bundle and resources, without Slipstream or a desktop catalog. Optional Web and CLI integration MUST resolve authorized identities to the same tool and state directory.

The tool owns semantic Edit State, revisions, input binding, instance and mask identities, immutable resource references, request replay, and per-edit locking. Slipstream MAY retain pointers and observed facts but MUST NOT retain a second mutable parameter snapshot.

The native bridge owns reflection, image initialization, validation and application, history construction, and pixelpipe execution. Native catalogs, handles, and blobs are private attempt state, not canonical editing state.

[Catalog Scope](editing.md#catalog-scope) owns resource, Preset, and Style catalog selection. The edit-local catalog owns its persistent identity; edit views MUST read its descriptor instead of retaining another catalog identity in edit-state storage.

The bundle MUST pin the engine commit, native protocol, semantic schemas and mappings, baseline policy, resources, and color assets. Discovery MUST distinguish readable, writable, and executable capability without claiming GUI parity or qualifying every input and output combination.

## Interaction

A direct caller supplies accessible input and state paths. Slipstream resolves authorized workspace, file, and edit identities on the execution host and invokes the same tool.

For an attempt, the tool MUST capture the selected revision and input, check native metadata, initialize the qualified baseline, and apply complete intent before the requested action or rendition. A mutation returns its atomically saved revision; a render returns evidence of its completed file.

The tool MUST use a deterministic typed client over private stdio to its pinned native MCP bridge. The native protocol MUST NOT become a public HTTP API or an LLM-driven session. Tool completion MUST NOT require a Slipstream callback or SDK registration.

## Discovery and Parameter Contract

Bundle construction MUST obtain actual module identities, parameter versions, structures, and headless operations. Bounded approved metadata MAY support inspection without launching an engine for every read.

Each native attempt MUST check protocol metadata and used schemas against the bundle. A mismatch MUST report engine incompatibility without changing state or installing output.

The bridge MUST expose one recursive semantic contract for schema, decoding, validation, and application. It MUST disclose array shapes, nested structures, string encoding and capacity, finite scalar ranges, exact integer rules, booleans, enums, and unsupported kinds.

Metadata MUST describe multiplicity, legal order, relationships, masks, coordinate frames, resources, actions, and output combinations. Semantic catalog identities MUST NOT depend on stack position or UI labels. Large catalogs MUST be bounded or paginated.

Image-context inspection MUST return actual initialized and reset values and operation relationships. Static schema defaults MUST NOT substitute for camera initialization. Prospective inspection MUST NOT save state.

Add and reset MUST initialize for the bound image; clone MUST allocate a distinct stable instance identity. Partial updates MUST preserve omitted values and produce a complete validated revision. Native handles MUST NOT become durable identities.

[Presets and Styles](editing.md#presets-and-styles) owns application semantics. The bridge supplies actual defaults, compatibility, relationship validation, and native legal ordering; the tool resolves mappings and computes the candidate. Slipstream and Web MUST NOT implement another reset or replacement algorithm.

Validation MUST reject wrong types, non-finite or unrepresentable values, invalid integers or enums, overlong strings, wrong array dimensions, unknown fields, and invalid stack or reference structure. Values MUST NOT be coerced, clamped, truncated, or ignored.

Private binary parameters MUST carry their source version and checked current size. Equal size MUST NOT authorize an older layout. Older semantic intent requires an explicit qualified upgrade; desktop binary-history import is not implied.

## Execution and State Lifetime

Each native attempt MUST have fresh configuration, catalog, cache, and scratch. It MUST receive read-only bound input, pinned assets, and confined copies of immutable resources, without Library write access, Slipstream database access, or desktop configuration.

Ambient XMP loading and writing, desktop presets, crawler startup, and display dependencies MUST be disabled. Private catalog writes MAY be allowed; read-only flags alone are not confinement.

The tool MUST map captured state against the qualified baseline and actual bound-image defaults. [RAW Development](../processing/color.md#raw-development) owns technical corrections and artistic-default restrictions. Input bytes MUST be checked for stability and MUST NOT be silently refreshed or recopied from the Library for each render.

The bridge MUST enforce admitted instances and legal ordering. Removing referenced masks or instances MUST fail unless an atomic composite leaves valid state. Duplicate chromatic adaptation, invalid geometry, invalid relationships, and incompatible resources MUST fail before save; failed attempts MUST discard partial native history.

A headless action MUST identify its native algorithm, input schema, and affected targets. The bridge MUST evaluate it against isolated initialized state and return concrete semantic values; the tool MUST validate and atomically save those values. Region pickers and automatic geometry require native headless support, not GUI event simulation or a competing Slipstream algorithm.

Preview and Export MUST share state preparation. Export MUST NOT require a preliminary Preview or persistent native catalog commit. An 8-bit native render MUST NOT become a scene-linear Development TIFF or Film input.

[Local Photo Executor](../processing/execution.md) and [Processing Memory](../processing/memory.md) own deadlines, cancellation, and resource admission. EOF or exit before a complete native result MUST fail the attempt.

The tool MUST validate output and install it using private staging and atomic no-replace publication before reporting completion. A failed render MUST preserve a confirmed save. Native attempt state MUST be discarded after confirmed termination while semantic editing state remains available.

## Integration With Existing Workloads

Standalone SpektraFilm consumes an explicitly selected compatible file and MUST NOT cause darktable to rerun upstream state. Development Proxy limitations remain governed by [Photo Development Architecture](../design.md#development-proxy).

Standalone and supervised access MUST share per-edit guards without a workspace-global state lock. Service restart MUST reconcile only service-owned receipts; it MUST NOT adopt or cancel direct CLI children. Independent file discovery does not itself prove Processing Artifact provenance.

[Existing service state at cutover](../design.md#existing-service-state-at-cutover) governs preservation and conversion. Manual parameter packing, XMP history generation, and post-import SQLite validation MUST NOT remain a second production native execution path or fallback. Public namespace compatibility remains owned by the CLI and wire contracts.
