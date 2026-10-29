# Headless darktable Editing

The independent `darktable-edit` tool edits accessible image files through a persistent state directory. [darktable Integration](spec.md) owns its relationship to Slipstream; [darktable CLI](cli.md) owns invocation, guards, and structured results. [Photo Workspace](../workspace/spec.md) owns workspace files and [Photo Development](../spec.md) owns shared processing and Original safety.

## Scope and Ownership

The tool MUST provide general semantic editing, including:

- scalar and structured parameters and curves;
- multiple Operation Instances and legal ordering;
- blending, masks, and automatic actions;
- Module Presets, Styles, and resources;
- preview, history, and output choices.

Supported operations MUST use a general language rather than a bespoke command for each parameter. Exposure, white balance, lens correction, crop, and rotation are examples, not a closed control list.

The tool owns on-disk state, validation, mutation guards, history, rendering, and local file completion. darktable owns algorithms and valid operation relationships. Slipstream owns Library identity, workspace association, file indexing, access, and supervised invocation.

An external Agent owns editing decisions and workflow order. Neither component MUST embed a model, chat interface, planner, or Agent runtime. Web and CLI MUST use the same tool-owned JSON state, retained resources, and revisions.

A partial Web control MUST preserve objects and settings it does not edit and identify state it cannot present. Editing MUST NOT change Rating, Selection State, Album membership, Photo identity, or browsing position.

Discovery MUST distinguish readable settings, supported mutations, and executable results. Missing mappings, actions, assets, or output capabilities MUST have specific reasons and recovery actions. A complete schema alone does not qualify a control.

## Finding a Photo

Slipstream callers MAY begin with an Album, Folder, query, Photo ID, or exact Library-relative Original Location. [Finding Photos](../../command-line/spec.md#finding-photos) owns lookup, ambiguity, and publication readiness. RAW and same-basename JPEG are independent Photos.

Workspace opening and execution-host accessibility follow [Photo Workspace](../workspace/spec.md#directory-and-execution-location). The integration MUST NOT use a writable Library path as editing input or imply that a remote server path is local to the caller.

Standalone use starts with a readable local image and explicit state directory. It MUST NOT import a Photo or require Slipstream callbacks or registration.

## Inspecting and Starting an Edit

Opening a new input MUST atomically persist its native initialized baseline. The result MUST identify the state path, input bytes, engine and schema, revision, concrete initialized settings, reset targets, output settings, and relevant capabilities.

Reopening matching state MUST be idempotent and MUST NOT reset or rebind it. A mismatching input requires an explicit decision. Reads MUST NOT mutate state or render.

Unavailable initialization MUST report missing assets or unsupported input rather than invent static defaults. Saved intent MUST remain inspectable without the live Original or a usable engine; input-scoped discovery MUST distinguish unavailable facts and offer a recheck where appropriate.

The caller MUST NOT construct a complete parameter tree or create a desktop catalog. A baseline MAY be exported directly; a Style or Module Preset MAY be the first change.

When several darktable states exist, integration MUST require explicit selection and return bounded summaries. It MUST NOT choose the first, newest, or ambient current state. Variants use separate state directories associated with the same Photo.

Input binding MUST identify actual bytes and image facts. A compatible completed output MAY initialize a state without reading the Original; compatibility MUST NOT depend solely on an extension or a Slipstream Export receipt.

[Photo Development](../spec.md#tool-invocation-and-composition) owns protected managed input reuse and external-file snapshot admission. Changed input evidence MUST invalidate the binding; an explicit refresh MUST NOT retarget existing state.

## Discovering Editing Capabilities

Discovery MUST provide a bounded searchable catalog and detailed operation descriptions. Discovery, validation, mutation, Preview, and Export MUST use the same native contract; callers MUST NOT need private protocols or source code.

Descriptions MUST identify meanings, types, units, ranges, symbolic choices, structured shapes, reset behavior, instance changes, ordering, interactions, and schema versions. Static defaults and input-dependent initialized values MUST be distinguished. Camera, lens, RAW, resource, and execution-capacity support MUST be reported separately.

Geometric controls MUST disclose units, origin, axes, coordinate frame, and whether values represent edges, sizes, or angles. Curves MUST disclose node coordinates, interpolation, and channel meaning.

Exposure MUST disclose its reference baseline and whether values are absolute compensation. White balance MUST disclose its relationship to input color interpretation and adaptation; a second correction MUST NOT be applied silently.

An automatic action MUST disclose inputs, affected objects, preconditions, and result. Offered pickers, region sampling, lens scaling, and perspective estimation require executable headless actions; numeric fields alone do not establish that support.

Incompatible state MUST remain readable with its native schema and engine identities and a specific limitation. Mutation and execution MUST refuse unsupported interpretation. A supported migration MUST disclose changes, retain the previous state, and validate the new state before making it current.

## Changing an Edit

A mutation MUST address an explicit state and intended change. Assignment is absolute; relative adjustment requires an advertised action. Unmentioned settings MUST be preserved.

Related instance, parameter, mask, blending, and output changes MAY form one bounded atomic mutation. Validation MUST evaluate the resulting combination; any failure MUST preserve the complete prior state.

Writes use a per-state lock and revision. Web and other callers acting on observed state MUST supply the expected revision. A stale guard MUST conflict; independent states MUST NOT be locked merely because they share a workspace.

[CLI mutation rules](cli.md#atomic-editing) own bounded waiting, no-guard convenience, and exact-request recovery.

Operation Instances have stable identities independent of position and display name. Callers MUST be able to add, inspect, duplicate, enable, disable, reset, remove, and legally move supported instances.

Add and duplicate MUST return identities; defaults come from the current input. Duplication MUST disclose shared mask and resource references.

Invalid ordering or dangling references MUST fail unless the same atomic change repairs the dependency. Reset MUST disclose scope and initialized values, preserve unrelated state, and leave other variants and Library facts unchanged.

A proposed change MAY be validated without saving or reserving a revision. Such validation does not guarantee later concurrency or capacity admission.

Automatic actions MUST calculate concrete settings against a captured input and revision and save under the same guard. Failure or a state change during evaluation MUST NOT save partial adjustment. Recovery MUST NOT rerun an uncertain action against newer state.

Rebind MUST be explicit and validate retained settings against the newly acknowledged input. A changed workspace file invalidates its old evidence; source refresh MUST NOT automatically rebind existing state.

## Masks and Resources

Callers MUST be able to inspect, create, modify, bind, unbind, and remove supported drawn, parametric, and raster masks and combinations. Discovery MUST describe their representation and limits.

Each mask MUST identify its coordinate frame and source stage. Upstream geometry changes MUST preserve that meaning or refuse incompatibility. A picker or drawn region MUST identify the rendition and edit revision used to choose coordinates.

Blend modes, opacity, channels, and references MUST be discoverable with their operation. Missing raster sources, cycles, dangling references, and incompatible combinations MUST fail before saving.

The tool MUST list supported resource kinds and import local ICC profiles, LUTs, and raster masks. Import MUST validate content, type, size, and integrity and return an immutable version. Resource bytes MUST be retained with the state or carry an explicit dependency with availability evidence; a mutable path alone is not a packaged resource.

Resource content MUST NOT execute code or import ambient engine state. Remote integration MUST use authorized workspace resources rather than arbitrary server paths.

Edits, Styles, previews, and Exports MUST identify exact resource versions. Removal MUST NOT break retained revisions or captured renders. Missing resources MUST remain visible with a recovery action; no fallback profile, LUT, or mask MAY be selected silently.

## Catalog Scope

Resources, Module Presets, and Styles belong to tool-owned catalogs. The state directory owns its edit-local catalog. Standalone and authorized Slipstream access to the same edit MUST use the same records, identities, immutable versions, and reference accounting, without copying records or export/import between entry points.

An unscoped catalog command selects the standalone user catalog or the supervised tool's configured catalog; the selection MUST be disclosed. A scoped command selects only the named edit-local catalog.

Multiple edits require explicit selection; reads MUST NOT initialize or choose an edit by recency or Web selection. Ambient desktop catalogs MUST NOT be read.

Save and apply default to the target edit-local catalog. An explicit global selection resolves definitions and new resource references in the executing tool's disclosed user or configured catalog. Atomic validation and application MUST use one selected catalog for all new references.

Unchanged references MUST resolve through the revision's pinned bytes or immutable retained references. Missing IDs, versions, or names MUST NOT trigger cross-catalog search. Application MUST pin required dependencies without duplicating catalog records.

Every catalog discovery and operation result MUST identify the selected scope and catalog identity. [darktable CLI](cli.md#managed-resources-presets-and-styles) owns selectors and result semantics.

## Presets and Styles

A Module Preset selects settings for one operation; a Style selects instances and relationships. Callers MUST be able to list, inspect, save, apply, import, export, and remove supported representations by identity and immutable version. Export does not imply universal desktop-history import or bidirectional XMP synchronization.

Saving MUST disclose included controls, instances, masks, resources, and output settings. Definitions MUST distinguish literal values from target initialization; camera-dependent values, lens facts, and source-bound geometry MUST NOT be treated as universal defaults.

Style application MUST map each captured entry exactly once to a compatible existing instance or null. Duplicate destinations, unknown or missing entries, unknown destinations, and incompatible operation or schema mappings MUST refuse the entire application. Names and positions MUST NOT imply mappings.

Merge MUST change only captured fields and relationships of mapped existing instances. Replace MUST reset exactly the non-null mapped instances to actual target-input defaults, preserve their IDs and positions, then apply captured fields and relationships.

When masks are omitted, reset and application MUST preserve existing mask-dependent fields and relationships. Both modes MUST preserve unrelated instances and existing shared masks. Compatible captured masks MUST receive target-local identities rather than overwrite existing masks.

A null destination MUST create one occurrence initialized for the target input before applying captured fields and relationships. All-null Replace MUST NOT reset or delete an existing instance.

Existing mapped positions MUST be preserved. New occurrences MUST follow captured relative order where legal; when multiple insertions are legal, use the earliest position that satisfies native constraints without reordering existing instances. Impossible ordering or relationships MUST refuse the whole application.

Module Preset update MUST change included fields only. Replace MUST reset only its explicitly identified instance, using the same target-default and mask-omission rules.

Optional validation MUST report mapped reset scope, preserved targets, new occurrences, and relationship and output effects without saving or reserving state. Guarded application MUST validate the same effects against the observed revision and return them. Exact replay MUST return the original mapping and effects without allocating again; changed intent MUST conflict.

Definitions MUST preserve included masks and complete relationships. Compatible application MUST allocate target-local identities and rewrite references without dangling source identities. Omitted masks MUST preserve target relationships.

A Style without output settings MUST preserve the current output choice.

Application becomes concrete saved state. Later modification or deletion of a definition MUST NOT change that state or accepted outputs. Saving or applying a Style MUST NOT require a GUI-authored preset or modify unrelated states.

## Preview and Inspection

Preview MUST capture saved state and input revisions and write a completed image in the chosen directory. The result MUST report the actual path, requested and actual dimensions, rendition identity, engine and resource versions, display conversion, and current or stale status. Rendering MUST NOT mutate or select the edit.

Standalone execution is synchronous and MUST NOT invent a service-owned background job. Slipstream MAY supervise the same executable with an inspectable run identity; inspection MUST NOT rerender a newer revision.

Failure or cancellation MUST preserve saved state and complete previews. A late result MUST remain tied to its captured revision and be marked stale when state advances. Comparison MUST identify both operands and transforms; a camera JPEG MUST NOT replace a developed operand.

Statistics MUST disclose region, resolution, color, transfer, and clipping stage; display clipping is not proof of lost RAW highlights. Detail inspection MUST disclose scale and quality and use a qualified coordinate frame. It MUST NOT enlarge a small preview as full-detail evidence.

Unsupported detail modes MUST explain why and MAY refer to a matching full Export. Preview success does not establish full-resolution feasibility or spatial-effect equivalence. [Photo Workspace](../workspace/spec.md#export-download-and-continue) owns delivery and file discovery.

## Output and Export

Saved output settings MUST describe discoverable compatible format, sample precision, color space, profile, transfer, geometry, and encoding. Selecting darktable does not imply a universal format or fixed module pipeline.

Qualified inputs MUST support full-resolution IEEE float32 RGB TIFF with a genuine linear ProPhoto RGB profile under [Development Color](../processing/color.md). Full resolution uses edited crop and rotation without unrequested resize or upscale.

A scene-referred contract conflicting with tone mapping MUST fail before execution. Artistic settings MUST NOT be silently disabled, and tone-mapped pixels MUST NOT be called scene-referred merely because their ICC profile is linear. Preview conversion MUST NOT become the final render source.

Export MUST capture the confirmed revision, input bytes, complete settings, resources, engine identity, and output contract. It MUST NOT require a Preview or silently capture a later revision. An explicitly selected retained revision MAY be used while its dependencies remain available.

The result MUST report the completed path, captured revision, byte identity, and verified output facts. Publication and workspace discovery follow [Photo Workspace](../workspace/spec.md#export-download-and-continue). An uncertain response MUST use exact-request reconciliation, not filename coincidence.

## History and Recovery

Successful mutations MUST retain local revisions with initialized baseline, changed targets, and input provenance. Reads, no-op writes, previews, Exports, and catalog captures MUST NOT add editing history. History MUST survive restart and remain inspectable without the live Original or a usable engine.

Each state MUST retain its current revision, initialized baseline, and at least its latest 100 successful saved revisions. The tool MUST advertise its finite limit and oldest retained revision. Revisions and resources referenced by accepted work or kept outputs MUST survive independently of the browsing limit; an unavailable revision MUST NOT resolve to current state.

Restore MUST create a new guarded revision and preserve the current input binding. Different-input history requires explicit supported rebind and validation; incompatible settings or coordinates MUST NOT be silently migrated or transformed. Restore MUST NOT recreate unavailable input files or retarget prior outputs.

Atomic state writes MUST preserve the prior revision on interruption or disk exhaustion. Recovery MUST identify affected objects and available actions without requiring reconstruction of native blobs or edits to the Slipstream database. [Photo Development](../spec.md#failure-and-retention) owns shared file recovery and retention.
