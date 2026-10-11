# darktable CLI Reference

A Photographer or Agent must be able to edit an image through semantic darktable operations, retain that state, and write previews and exports into the directory that holds the input. This document defines two target entry points: standalone `darktable-edit` and the optional Slipstream integration under `slipstream darktable`. `darktable-edit` is a proposed product executable, distinct from the official darktable `darktable-cli` batch renderer and the darktable GUI. Installed help and qualified capabilities determine actual availability; this reference does not claim a candidate implements the target.

[CLI Reference](../../command-line/reference.md) owns Slipstream credentials, envelopes, and Library/workspace grammar. [Photo Workspace](../workspace/spec.md) owns the directory and its files. [darktable Editing](editing.md) owns editing behavior. The semantic grammar below is authoritative for both entry points; engine parameter parsing and native serialization remain private.

## Grammar and Identity

Standalone initialization is:

```text literal
darktable-edit [--output json|text] open INPUT --state STATE_DIR [--preview-directory DIR] [--output-directory DIR]
```

`INPUT` is an existing regular image file accessible on the executing host. `STATE_DIR` is an explicit private edit-state directory, created atomically when absent and reopened when already bound to that exact input. Reopening with another input fails; use explicit rebind or a different state directory. The tool runs on arbitrary supported local files without Slipstream, a Photo ID, service credentials, a server, or its database. It owns its qualified native engine and dependencies. Input files are read-only and are never rewritten, renamed, or given adjacent XMP files implicitly.

Within a workspace, use `input/<filename>` and `.edits/darktable/<edit-name>` under the returned workspace path. Initialization accepts explicit preview/output directories and records them in tool state. The integration supplies the workspace's `previews/` and `outputs/`; a standalone caller supplies them once when choosing the same-directory workflow. Without explicit directories, defaults are `previews/` and `outputs/` beneath `STATE_DIR`. The tool does not detect Slipstream ownership from directory names or read Slipstream metadata/schema. `open` returns `stateDirectory`, `inputPath`, `executionHost`, `revision`, `stateToken`, `previewDirectory`, and `outputDirectory`. Rendering may explicitly override the recorded directory with `--directory`. Directories are created lazily. A tool checks actual filesystem access; a server path returned to a remote client is not a local path on that client.

The following grammar uses `COMMAND` for either `darktable-edit` or `slipstream darktable`. `TARGET` means `STATE_DIR` for standalone calls, or `PHOTO_ID [--edit EDIT_ID]` for Slipstream calls. `GUARD` means optional `--if-state TOKEN [--request REQUEST_ID]`; `--request` is also accepted without a guard. These are grammar metavariables, not shell values. Slipstream obtains Photo IDs through [Photo Queries](../../command-line/reference.md#photo-queries); workspace open is the one Library-to-file handoff in the independent workflow. Only the integration adapter needs Photo identity thereafter.

For catalog scope and shared record ownership, see [Catalog Scope](editing.md#catalog-scope). Catalog-only commands accept `[--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID]`: standalone uses `--state`, integration uses authorized `--photo`/`--edit`, and omission selects the executing tool's disclosed user/configured catalog. Targeted editing, save, and apply default to the target's edit-local catalog. Optional `--catalog local|global` on save, preset/style apply, and atomic apply/validate explicitly selects that local catalog or the executing tool's user/configured catalog. There is no fallback between catalogs. Resource bytes required by a revision are pinned privately in its state directory or by immutable retained references with explicit availability. Tools with other names have the same independence boundary; a directory does not make them depend on Slipstream.

IDs use 1–128 ASCII letters, digits, `.`, `_`, or `-`; tokens are opaque base64url strings of at most 512 bytes. Paths are paths and do not use the identifier restriction. Native field/action IDs must be stable semantic mappings and obtained through discovery. `--value JSON` accepts one complete JSON value. JSON command inputs are bounded to 64 KiB, reject duplicate/unknown keys, and preserve array order. Source evidence, when carried in JSON, is opaque UTF-8 up to 16384 bytes, including escaped NUL; it is never a command-line ID. Edit bounds are 100 changes, 256 instances, 256 masks, depth 32, and geometry edges 1–65536; schema descriptions are at most 1 MiB. Smaller disclosed qualified limits govern admission.

Standalone operational commands default to one JSON envelope on stdout, using the common `schemaVersion`, `status`, `data`, and `error` shape and exit classes in [CLI Reference](../../command-line/reference.md#output-envelope), without service credential options. Progress goes to stderr. Every result identifies execution host and actual absolute paths for file effects. Mutations generate and return a request identity when omitted. Ordinary users need not assemble artifact IDs or provenance manifests before a simple command. GUI read-modify-write, Agent read-dependent writes, atomic apply, reorder, rebind, history restore, and mapped preset/style application require observed `--if-state` evidence. No-guard convenience is limited to focused commands expressing a fresh intent validated entirely under the current-state lock; it cannot overwrite a caller-constructed full state. Callers needing recovery after an uncertain response supply a stable `--request` before submission and retain the exact command/input.

## Discover and Inspect

```text literal
COMMAND modules list [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID] [--if-state TOKEN] [--search TEXT] [--limit N]
COMMAND modules describe MODULE_ID [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID] [--if-state TOKEN]
COMMAND edit show TARGET
COMMAND edit instances list TARGET [--if-state TOKEN] [--limit N]
COMMAND edit instances show TARGET INSTANCE_ID [--if-state TOKEN]
COMMAND edit masks list TARGET [--if-state TOKEN] [--limit N]
COMMAND edit masks show TARGET MASK_ID [--if-state TOKEN]
COMMAND edit actions list TARGET [--if-state TOKEN] [--instance INSTANCE_ID] [--limit N]
COMMAND edit actions describe TARGET ACTION_ID [--if-state TOKEN] [--instance INSTANCE_ID]
COMMAND edit output show TARGET [--if-state TOKEN]
```

`--state` is standalone-only; `--photo`/`--edit` on catalogs are Slipstream-only and `--edit` requires `--photo`. `modules` discovers native darktable operations rather than the peer Processing Module catalog. Global discovery returns schemas; input-scoped discovery returns initialized/reset values, frames, and capability availability for that input. Reads never create state. Standalone requires `open` before state-scoped editing. Slipstream `edit show` may inspect a prospective workspace input without saving an edit. One existing darktable edit is selected regardless of Web selection; multiple edits require an explicit `--edit` and otherwise return `edit_ambiguous` with candidates. It never chooses the first or newest edit.

Unavailable initialization exposes unknown values as absent, output settings as `{}`, and image contract as null. It does not substitute static defaults, camera JPEGs, or another tool. Retained incompatible state remains readable. A supplied token selects and validates that exact input and state before returning contextual values; changed evidence returns `state_conflict` rather than another baseline.

`EditView` contains `stateDirectory`, `executionHost`, `input`, `revision`, `baselineRevision`, `stateToken`, `engineId: "darktable"`, `bundleId`, `schemaVersion`, `availability`, and `initialization: {state: "ready"|"pending"|"unavailable", reason: string|null}`. Slipstream adds `photoId`, `workspaceId`, `editId`, and `webUrl`; these are integration references rather than tool dependencies. `input` identifies the absolute path, fingerprint, byte length, availability, and bound versus observed input revision. Input changes require explicit rebind; Library Original changes do not invalidate an unchanged workspace snapshot.

The view also exposes bounded `instances`, `masks`, and `actions` windows; complete output settings; actual input `frames: {frameId, width, height, coordinateSchema, transform}[]`; `previewCapabilities` with nullable region/statistics schemas and a positive file-byte bound; latest preview identity/revision/staleness; and admitted limits. Each embedded window has `items`, `total`, and `nextCursor`, with at most 60 items. Lists default to 50, accept 1–60, and alternatively accept `--cursor CURSOR` without filters or target arguments. Lists pin one state revision or bundle/schema identity; history pins retained revision IDs. Opaque cursors retain captured windows across command processes and service restarts, expire after 15 minutes idle or bounded eviction, and fail explicitly rather than mixing pages. Embedded view cursors continue through the corresponding instances, masks, or actions list operation. Continuations retain their selected state/catalog and authorized Photo/Edit scope.

`CapabilityStatus` is `{read: boolean, write: boolean, execute: boolean, reason: string|null, recovery: string|null}`. A `ModuleSummary` contains `moduleId`, `name`, `schemaVersion`, `multipleInstances`, and `availability`. Describe adds parameter/blend schemas, actions, ordering and relationship constraints. Each parameter exposes stable ID, name, type, unit, bounds/enums, recursive fields/items, required status, JSON Schema 2020-12 `valueSchema`, availability, and actual initialized/reset values when known. Schemas own deterministic validation; clients never decode native blobs.

`InstanceSummary` contains `instanceId`, `moduleId`, `name`, `enabled`, `order`, and availability. Show adds parameter values and their initialized/reset values, blend settings, and mask IDs. `MaskSummary` contains `maskId`, `kind`, `frameId`, and `referencedBy`; show adds value and schema. `ActionSummary` contains `actionId`, nullable `instanceId`, name, availability, and affected targets; describe adds its input schema. Headless pickers require an advertised region/frame schema. Unsupported GUI actions are disclosed as unavailable.

## Atomic Editing

```text literal
slipstream darktable edit create PHOTO_ID [--input FILE] GUARD
COMMAND edit set TARGET INSTANCE_ID PARAMETER_ID --value JSON GUARD
COMMAND edit validate TARGET --input FILE [--catalog local|global] [--if-state TOKEN]
COMMAND edit apply TARGET --input FILE [--catalog local|global] GUARD
COMMAND edit reset TARGET --all GUARD
COMMAND edit reset TARGET --instance INSTANCE_ID [--parameter PARAMETER_ID] GUARD
COMMAND edit rebind TARGET --input FILE GUARD
COMMAND edit instances add TARGET MODULE_ID [--name NAME] GUARD
COMMAND edit instances clone TARGET INSTANCE_ID [--name NAME] GUARD
COMMAND edit instances enable TARGET INSTANCE_ID --value true|false GUARD
COMMAND edit instances remove TARGET INSTANCE_ID GUARD
COMMAND edit instances reorder TARGET --input FILE GUARD
COMMAND edit masks create TARGET --input FILE GUARD
COMMAND edit masks set TARGET MASK_ID --value JSON GUARD
COMMAND edit masks remove TARGET MASK_ID GUARD
COMMAND edit actions run TARGET ACTION_ID [--instance INSTANCE_ID] --input FILE GUARD
COMMAND edit output set TARGET FIELD_ID --value JSON GUARD
```

Standalone `open` creates the native initialized baseline and output settings together. Slipstream `edit create` or the first valid mutation resolves/reopens the Photo Workspace and initializes the same tool state; optional input is exactly `{input: {kind: "workspace-artifact", artifactId: string, revision: string}}`, or the retained adapter representation `{input: {kind: "original", sourceRevision: string}}`. Original-backed requests resolve to the read-only workspace snapshot once; they never keep editing the live Original or require its later availability. Explicit create allows another edit state. Artifact-backed state binds that exact file revision; already protected managed files are reused, while mutable external file admission takes one verified byte-independent protected snapshot and reuses it across edits/previews. A standalone rebind input is `{input: {path: string}}`; the adapter accepts the same checked workspace-artifact binding as create. Rebind preserves compatible settings, validates frames/resources, and creates a new revision rather than silently changing the source. Standalone arbitrary inputs remain read-only to the tool; any observed external mutation fails input validation rather than feeding new bytes to a saved revision.

The retained Original carrier resolves only a checked prepared snapshot whose source revision exactly matches `sourceRevision`. Missing or changed prepared evidence refuses creation; a refreshed input or workspace default cannot replace it. Creation journals the selected artifact identity and checked revision, and holds its common input lease through initialization and receipt publication.

Every mutation obtains a per-state exclusive filesystem lock shared by standalone, Web, and supervised integration. No whole-workspace lock blocks unrelated edits. Busy acquisition fails after a bounded disclosed interval. Under lock, supplied state evidence is checked before computing changes. Commands requiring observed evidence fail if no guard is supplied; focused no-guard convenience validates a fresh intent against current locked state. It never protects an earlier read or replaces a constructed full state. Atomic durable state replacement saves one complete candidate, with no partial instances/masks/output or history. Automatic actions compute under the same lock or recheck the captured revision before commit. A conflict or failed computation saves nothing. Standalone execution owns its process; client termination does not promise a detached computation.

Identical request ID and canonical payload replay the original committed result before considering today's guard; changed intent under the same identity returns `request_conflict`. Receipt settlement and state commit must be recoverable together after interruption. Receipts disclose `retainUntil` and survive restart for at least seven days after settlement; needed state and files do not expire with them. An uncertain request must be replayed exactly with its original identity before dependent work. If an auto-generated identity was not received, inspect state/files and report attribution as unknown; generating another ID cannot recover that lost admission. Service-supervised actions may continue after transport disconnect and expose `outcome_unknown` while pending; only a durable supervised receipt justifies that guarantee.

`edit validate` checks the same candidate schema and relationships without saving, initializing prospective state, creating history/receipts, importing resources, or executing automatic actions. It returns validity, deterministic effects, and actions requiring execution. It reserves nothing.

Apply accepts `{changes: Change[]}`, nonempty and within the disclosed bound. Focused commands lower to these same types:

- `{type: "set", instanceId, parameterId, value}`;
- `{type: "reset", scope: "all"|"instance"|"parameter", instanceId?, parameterId?}`;
- `{type: "instance-add", moduleId, ref, name}` or `{type: "instance-clone", instanceId, ref, name}`;
- `{type: "instance-enable", instanceId, enabled}`, `{type: "instance-remove", instanceId}`, or `{type: "instance-reorder", instanceIds}`;
- `{type: "mask-create", ref, kind, frameId, value}`, `{type: "mask-set", maskId, value}`, or `{type: "mask-remove", maskId}`;
- `{type: "blend-set", instanceId, value}`;
- `{type: "action", actionId, instanceId, input}`;
- `{type: "output-set", fieldId, value}`;
- `{type: "preset-apply", presetId, version, instanceId, mode: "update"|"replace", initialization: "literal"|"target-camera", maskPolicy: "omit"|"compatible"}`; or
- `{type: "style-apply", styleId, version, mode: "merge"|"replace", mapping: {entryId, instanceId: string|null}[], initialization: "literal"|"target-camera", maskPolicy: "omit"|"compatible", includeOutput}`.

Fields above denote typed JSON keys: IDs/names/refs are strings, values are schema-admitted JSON, enabled/includeOutput are booleans, input/blend/mask values are objects, and instanceIds is a string array. Null names and instance IDs are accepted where initialization/mapping requires them. Unknown keys or irrelevant reset fields fail. A request-local `ref` uses 1–64 letters/digits/`_`/`-`; later changes refer to it as `@ref`, including blend mask references. Existing IDs never begin with `@`. Changes evaluate in order against one candidate, and the result maps refs to generated stable IDs. Reorder names every final instance once. Focused reorder accepts `{instanceIds: string[]}`, mask create accepts `{kind: string, frameId: string, value: object}`, and action input follows its schema.

Assignment is absolute, including structured curves/arrays. Relative adjustment requires an advertised action. Unmentioned state is preserved. Reset uses actual input-context defaults; `--all` restores initialized instances, masks, blend, and output for the acknowledged input without changing its binding or another state. Removing referenced objects, illegal ordering, duplicate chromatic adaptation, invalid ranges/units, and missing dependencies reject the whole group unless its final state explicitly resolves them.

`MutationResult` returns state path, revision, state token, request ID, outcome (`saved`, `unchanged`, `replayed`), retention deadline, changed targets, complete concrete computed effects, generated IDs, and current revision/token. Slipstream adds integration IDs and Web URL. No-op writes create no history. A replay's committed snapshot may be older than current state; use `current.stateToken` or a fresh show for another decision.

## Output Contract

`OutputState` contains complete `settings`, JSON Schema 2020-12 `schema`, availability, nullable `imageContract`, and a separately identified `previewConversion`. Output fields describe actual format, precision, color space, transfer, encoding, profile/resource version, geometry, and valid combinations. `ImageContract` contains those facts, including `profile: {id, version}|null` and positive width/height. File extensions never prove compatibility. Crop and rotation are honored.

The qualified target advertises scene-linear float32 ProPhoto TIFF and supported display outputs. This does not force every edit to linear TIFF. Incompatible tone mapping with a scene-linear contract is an explicit conflict, never automatic module disablement. Preview display conversion is a separate branch and cannot become an export source. Exports have no hidden format, quality, geometry, or profile overrides.

## Managed Resources, Presets, and Styles

```text literal
COMMAND resources kinds [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID]
COMMAND resources list [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID] [--kind KIND] [--limit N]
COMMAND resources show RESOURCE_ID --version VERSION [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID]
COMMAND resources import --kind KIND --file PATH [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID] [--request REQUEST_ID]
COMMAND resources remove RESOURCE_ID --version VERSION [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID] [--request REQUEST_ID]
COMMAND presets list [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID] [--module MODULE_ID] [--limit N]
COMMAND presets show PRESET_ID --version VERSION [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID]
COMMAND presets save TARGET INSTANCE_ID --input FILE [--catalog local|global] GUARD
COMMAND presets apply TARGET PRESET_ID --version VERSION --instance INSTANCE_ID --mode update|replace --initialization literal|target-camera --mask-policy omit|compatible [--catalog local|global] GUARD
COMMAND presets import --file PATH [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID] [--request REQUEST_ID]
COMMAND presets export PRESET_ID --version VERSION --file PATH [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID]
COMMAND presets remove PRESET_ID --version VERSION [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID] [--request REQUEST_ID]
COMMAND styles list [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID] [--limit N]
COMMAND styles show STYLE_ID --version VERSION [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID]
COMMAND styles save TARGET --input FILE [--catalog local|global] GUARD
COMMAND styles apply TARGET STYLE_ID --version VERSION --input FILE [--catalog local|global] GUARD
COMMAND styles import --file PATH [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID] [--request REQUEST_ID]
COMMAND styles export STYLE_ID --version VERSION --file PATH [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID]
COMMAND styles remove STYLE_ID --version VERSION [--state STATE_DIR | --photo PHOTO_ID] [--edit EDIT_ID] [--request REQUEST_ID]
```

`--state` is standalone-only; `--photo`/`--edit` is integration-only, with `--edit` requiring `--photo`. Scoped operations require an existing edit and use the selection and `edit_ambiguous` behavior in [Discover and Inspect](#discover-and-inspect); no scoped catalog read creates state. Integration never accepts an arbitrary server state path. Paginated commands also accept cursor-only continuation, which retains the selected catalog identity. Kinds advertise media types, maximum bytes, availability, value schemas, and frames, covering ICC profiles, LUTs, raster, drawn and parametric masks. Actual dimensions/transforms come from input-scoped discovery. Typed imports validate regular-file bytes, length, type, structure and SHA-256 into immutable versions. Standalone reads a local file; the Slipstream adapter streams the caller-local resource into the selected tool catalog. This optional resource import is not a per-edit input/output synchronization step. No shell directives, native executable plugin, ambient desktop catalog, or untyped native-state import is accepted.

Catalog discovery, reads, imports, saves, exports, removals, validation, and application return `catalog: {catalogId, scope: "edit-local"|"user"|"configured", executionHost, stateDirectory: string|null}`. Edit-local `stateDirectory` is the actual absolute directory; global scope returns null. Catalog IDs are opaque tool-owned identities; authorized integration adds `photoId`/`editId` when scoped. Discovery and edit views disclose the target's local catalog and the separately configured global catalog, so callers can inspect the intended scope before a save/apply. Lists return only records from the selected catalog. `catalog_not_found` reports an absent catalog; `catalog_record_not_found` reports an absent ID/version in that catalog and includes its scope. Neither error triggers a second lookup.

Definition IDs/versions and newly introduced resource references resolve within the command's one selected catalog. Existing unchanged references resolve through the revision's pinned bytes or immutable retained references, not through another catalog search. Save writes one record in the selected catalog and retains its complete dependency closure there; already pinned source bytes may be retained without copying a source catalog record. Focused editing commands without `--catalog` resolve newly introduced resource references in the target's local catalog; intentional global references use atomic `edit apply --catalog global` with the same `set`, `blend-set`, or `output-set` change. This allows ordinary subsequent edits to preserve globally sourced pinned resources without another selector or lookup. Atomic apply/validate selects one catalog for every newly introduced reference, including preset/style definitions and their resources; per-change scope overrides are not accepted. Request identity includes the resolved catalog identity and canonical scope selection. Replay returns that original catalog and result, never a record with the same name from another catalog. Missing-record errors include the selected scope and recovery: select the intended existing local edit with `--state` or `--photo`/`--edit`, explicitly use `--catalog global` for targeted operations, or import the missing record into the selected catalog.

Resource records retain kind, version, name, media type, digest, size, creation time, availability and reference counts for current edits, retained revisions, catalogs and tasks. Schema-admitted values reference `{resourceId, version}`. Removal refuses referenced versions. Saved state must disclose unavailable dependencies; it must not claim a moved state directory is portable without its pinned bytes/engine.

Preset save selects `{name, parameterIds, includeBlend, initialization, maskPolicy}`. Style save selects `{name, instances: {instanceId, parameterIds, includeBlend}[], includeOutput, initialization, maskPolicy}`. Selection is explicit/nonempty. `literal` captures values; `target-camera` captures schema-declared initialization directives rather than source-camera numbers. Unsupported directives refuse capture. Presets retain module/schema, selected values, blend, masks and immutable resources. Styles retain identified entries and their relationships plus optional output; application does not match by name or position.

`DefinitionSummary` is `{id: string, version: string, kind: "preset"|"style", name: string, bundleId: string, schemaVersion: string, createdAt: string, availability: CapabilityStatus}`; `createdAt` is a UTC RFC 3339 timestamp. The returned `id` supplies `PRESET_ID` or `STYLE_ID`, and `version` supplies `VERSION`. `schemaVersion` identifies the definition representation, not one operation's parameter schema. Definition IDs are stable within the disclosed catalog; versions are immutable. List data is `{catalog, items: DefinitionSummary[], total: integer, nextCursor: string|null}` using the common catalog cursor bounds. Show, save, and import data is `{catalog, record: PresetRecord|StyleRecord}`; save/import additionally return `requestId: string`, `outcome: "created"|"replayed"`, and `retainUntil: string` (UTC RFC 3339). Exact replay returns the same created definition identity/version. Application uses MutationResult and never returns a new reusable definition merely because it created edit-local instances.

Both record kinds contain all DefinitionSummary fields plus `initialization: "literal"|"target-camera"`, `maskPolicy: "omit"|"compatible"`, `resources: {resourceId: string, version: string}[]`, and `masks: {schema: object, value: object}|null`. The masks schema is the bundle's JSON Schema 2020-12 for the complete captured mask graph; its value carries the local mask identities, typed references, frames, source-stage evidence, and selected source-input facts required by that schema. Null means no captured masks. Native blobs, executable schemas, arbitrary paths, and dangling references are not valid graph values. `Settings` is `{moduleId: string, parameterSchemaVersion: string, parameters: {parameterId: string, value: JSON}[], blend: object|null}`. Each parameter appears once; its value is a discovered schema-admitted value or, under target-camera initialization, an explicitly schema-admitted initialization directive. Blend null means omitted; otherwise it is the discovered structured blend value with typed mask references. The record preserves these selections rather than filling omitted fields from another image.

`PresetRecord` adds `settings: Settings`. `StyleRecord` adds `entries: ({entryId: string} & Settings)[]` and `output: object|null`. Here `&` means fields in the same JSON object. Entries are nonempty and in captured processing order; entryId is a unique record-local identity, never an edit instance ID or array index. Each definition version preserves those IDs/order. The complete saved output settings are present only when included; null means preserve the target output. Show always returns every entry, selected parameter, and dependency within the advertised definition byte bound; it must not silently truncate a definition. If the scope exceeds that bound, save/import refuses before creating a record. The caller constructs mapping from these returned entries and inspected target instances; it never invents entry IDs from names. Remove data is `{catalog, id: string, version: string, requestId: string, outcome: "removed"|"replayed"}`. Export data is `{catalog, id: string, version: string, path: string, sha256: string, byteLength: integer, fileCommitted: true}`.

Captured masks include the complete referenced closure, native schema, coordinate/frame and source-stage evidence. IDs are local to the catalog record; typed references are rewritten consistently, including Style entry IDs or reserved `self` for a Preset. Cycles, dangling dependencies or exceeded limits refuse capture/import. Raster dependencies remain immutable resources. Source evidence permits compatibility validation without reopening the source Photo or source workspace.

`maskPolicy: "compatible"` validates actual target frames/input, allocates target masks once, preserves shared references, and rewrites all typed relationships atomically; it never guesses coordinate transforms. Results include `catalogMappings` per applied change with stable instance/mask mappings; replay allocates nothing new. `omit` stores no captured masks and excludes mask-dependent fields, preserves existing target relationships, and refuses invalid results. Omitted fields are absent rather than null/source IDs.

Style apply input is `{mode, mapping: {entryId, instanceId: string|null}[], initialization, maskPolicy, includeOutput}`. Exact Merge/Replace reset scope, mapping validity, target defaults, native ordering, and preserved masks are authoritative in [Presets and Styles](editing.md#presets-and-styles). Preset update/replace follows that section as well. `edit validate` and application results expose `catalogMappings` and `effects: {resetInstanceIds: string[], preservedInstanceIds: string[], createdInstances: {entryId, instanceId: string|null, moduleId, order}[], changedTargets, relationships, output}` per application. Validate uses null generated IDs and reports planned ordering and concrete field/reset effects; apply returns committed IDs. No field describes an implicit contiguous replacement range. Captures create catalog records rather than edit history; applying them creates a normal guarded revision.

Interchange JSON is `{schemaVersion: 1, kind: "preset"|"style", record: PresetRecord|StyleRecord}`, with matching outer and record kinds. It reuses the complete show record, including selected values, ordered entry IDs, masks, relationships and resource versions. Imports validate schema/bundle and dependency closure, allocate a new definition identity/version, and cannot overwrite by descriptive file identity. Record-local entry/mask IDs and their internal references remain consistent in the imported record returned to the caller; the caller uses that returned record for later mapping. Descriptive creation/availability fields do not grant authority to import external files or bypass current qualification. Export uses no-replace publication. Import/export streams files under the separately advertised positive byte maximum rather than the mutation JSON bound.

## Preview and Inspection

```text literal
darktable-edit preview render STATE_DIR [--revision REV] [--directory DIR] [--name NAME] [--input FILE] [--request REQUEST_ID]
slipstream darktable preview render PHOTO_ID [--edit EDIT_ID] [--revision REV] [--request REQUEST_ID] [--input FILE]
slipstream darktable preview status PREVIEW_ID
slipstream darktable preview wait PREVIEW_ID
slipstream darktable preview download PREVIEW_ID --file PATH [--comparison]
slipstream darktable preview proxy get PHOTO_ID
slipstream darktable preview proxy create PHOTO_ID --input FILE [--if-state TOKEN] [--request REQUEST_ID]
slipstream darktable preview proxy remove PHOTO_ID [--if-state TOKEN] [--request REQUEST_ID]
```

Standalone render is synchronous and writes a completed image under the preview directory. It captures current state atomically, or the exact retained revision when supplied, then renders from the immutable capture without blocking subsequent unrelated edits. A default unique filename incorporates render identity; explicit names never replace existing files. Optional input is `{comparison: {revision: string}|"baseline"|null, region: object|null}`; Slipstream may additionally name a comparison edit ID. Baseline means native initialization under the same input/schema, never camera JPEG. Region/detail support follows discovered schemas. Preview does not save state.

Files are written privately, closed and validated, then renamed without replacement into their final directory. Prior complete previews remain visible during work and after cancellation/failure. A late completion from an older revision is marked stale; it cannot replace the latest relevant preview. Results identify actual paths, input/revision, bundle/schema, conversion, frame/scale, dimensions, detail limit, digest/length and nullable native statistics. Statistics distinguish working/display scope and never label display clipping as sensor clipping. Full-resolution export provides detail when bounded preview cannot.

Only the Slipstream adapter may admit service-supervised jobs, capturing the current or explicitly requested saved revision before queueing and promptly returning a durable `previewId` with `accepted`, `executing`, `succeeded`, `failed` or `cancelled` state. Its job record retains captured input/state, request, timestamps, retention deadline, image/comparison file evidence, statistics and failure. A succeeded image is the same workspace file that Slipstream indexes. Status/wait read that job, not a new render; disconnect/timeout does not cancel admitted service work. Wait is bounded by client timeout, returns the handle with `wait_timeout` if pending, and reports terminal failure honestly. Standalone has no detached status/wait promise: an invoking process owns execution, interruption leaves no partial ready file, and its durable request receipt is reconciled on exact replay.

Download is optional for remote image inspection/delivery. Same-host callers read the completed path directly; they do not download/upload/register/commit each iteration. Comparison download only retrieves the already captured comparison image. Both downloads use bounded verified no-replace transfer.

Development Proxy commands retain legacy read/remove access and explicitly bounded rebuilding where qualified. They are caches, not the workspace input snapshot, and cannot silently replace a missing input or authorize an Original-backed export. Proxy replies disclose source revision, dimensions, byte evidence, status and failures. Workspace editing survives loss of a live Original when its own snapshot remains unchanged.

## Export and Download

```text literal
darktable-edit export create STATE_DIR [--revision REV] [--directory DIR] [--name NAME] [--request REQUEST_ID]
darktable-edit export state-file STATE_DIR [--revision REV] --file PATH [--request REQUEST_ID]
slipstream darktable export create PHOTO_ID [--edit EDIT_ID] [--revision REV] [--request REQUEST_ID]
slipstream darktable export list PHOTO_ID [--edit EDIT_ID] [--limit N]
slipstream darktable export status PHOTO_ID REQUEST_ID
slipstream darktable export wait PHOTO_ID REQUEST_ID
slipstream darktable export cancel PHOTO_ID REQUEST_ID [--request CANCEL_REQUEST_ID]
slipstream darktable export retry PHOTO_ID REQUEST_ID [--request NEW_REQUEST_ID]
slipstream darktable export download ARTIFACT_ID --file PATH
slipstream darktable export state-file PHOTO_ID [--edit EDIT_ID] [--revision REV] [--request REQUEST_ID]
slipstream darktable export state-file-download STATE_FILE_ID --file PATH
slipstream darktable export historical-download EXPORT_ID --file PATH
```

Standalone export is synchronous. It captures the saved current or named retained revision, input fingerprint, dependencies, engine/bundle and complete output settings. Preview is optional. It validates that capture and writes the final image into the output directory by private write/validate/close/no-replace rename. Default filenames are unique; explicit name collision refuses. A completed result returns absolute path, revision, input evidence, output image contract, digest/length, request identity and `fileCommitted: true`. An export failure leaves saved adjustments and earlier files intact. Disk-full is actionable and never silently lowers precision/resolution.

The Slipstream adapter supervises the same executable on the execution host against the same state directory. It retains durable jobs and captured revisions, and offers status/wait/cancel/retry with their existing lifecycle meanings. First terminal decision wins cancellation. Retry creates a new attempt from the original failed/cancelled capture, never today's state. Exact request replay attaches to the original attempt before checking today's state; changed payload conflicts. Capacity refusal does not evict other files. Only accepted service work continues after the client exits. A local caller that closes its owning process has no equivalent guarantee.

Final workspace files become visible through normal Slipstream reconciliation. Tools have no required callback or registration API. Optional verified invocation metadata enriches provenance; arbitrary discovered images remain workspace files until facts are verified, never fabricated successful Exports. Workspace reads recover missed notifications. Indexed artifact identity/revision is bound to completed bytes; subsequent in-place changes invalidate old evidence. Same-host next tools take the returned output path directly after checking their own input contract. There is no forced publication or synchronization action, hidden conversion, automatic chaining, or new Library Photo.

The adapter `ExportRecord` retains request, integration IDs, captured revision/input/output/bundle/schema, lifecycle timestamps, nullable failure, retry source, and nullable artifact. Indexed artifacts expose path, filename, output contract, dependencies and exact byte evidence plus Web URL. Current workspace artifacts/state do not have a seven-day expiry. Transient receipts disclose their own deadline independently; explicit file removal follows workspace dependency guards. Legacy artifacts keep truthful historical retention facts.

Downloads verify bound revision, decoded image contract, structure, length and SHA-256 before no-replace local publication; resumable transfer follows [Photo Development](../../command-line/reference.md#photo-development). Existing files/symlinks are never overwritten. A post-publication reporting failure preserves the file and reports its committed effect. State-file export writes complete confirmed native XMP state as an explicit interchange file, including dependency identities and disclosed optional Camera Raw mappings/omissions; it does not require engine execution or modify an Original/sidecar. It is not the authoritative resumable state directory. Historical download retains original metadata/bytes and never invents complete current provenance.

## Saved History and Recovery

```text literal
COMMAND history list TARGET [--limit N]
COMMAND history show TARGET --revision REV
COMMAND history restore TARGET --revision REV GUARD
COMMAND edit migration show TARGET --bundle BUNDLE_ID --schema SCHEMA_VERSION
COMMAND edit migrate TARGET --migration MIGRATION_TOKEN --if-state STATE_TOKEN --request REQUEST_ID --confirm
```

History contains retained successful revisions, parent, time, request identity, changed targets, availability and pinned status. Current/baseline and at least the latest 100 successful revisions remain; dependencies referenced by retained files/jobs pin their captures. Reads, no-ops, previews, exports and catalog captures do not create edit revisions. Full history inspection returns canonical semantic parameters, masks/blend, output and input evidence without decoding native blobs, even after restart or loss of compatible execution.

Restore creates a new guarded revision rather than decreasing revision identity. It validates saved settings/resources/frames against the acknowledged binding and never substitutes versions or recreates deleted files. After explicit rebind, restore applies compatible historical adjustments against the newly acknowledged input instead of reviving its old input binding. Incompatible coordinates refuse; no silent transform occurs. Neither entry point changes Library decisions, Album membership or another tool's edit state.

Migration retains historical semantic state, XMP receipts and image bytes non-destructively. An adapter projection may retain old step/recipe identities, but the on-disk tool state is the canonical parameter source. Reading old unsupported state cannot silently convert it; explicit supported migration preserves the original retained record and dependencies.

`edit migration show` is read-only. It returns source revision/bundle/schema,
target bundle/schema, availability and refusal reason, a bounded complete list
of semantic changes and identity mappings, `stateToken`, and `migrationToken`.
The latter binds the exact disclosed conversion, converter identity, target
assets, and source state; it is null when no supported complete conversion is
available. Inspectable incompatible state must still supply these guards.
Inspection never installs an engine, mutates state, or renders an image.

`edit migrate` requires that reviewed conversion and explicit confirmation.
Under the same per-state lock, it checks the tokens and target dependencies,
validates the complete converted state, retains the original state and its
dependencies, and atomically makes a new revision current. Changed source,
converter, or target assets refuse without refreshing approval. Lossy conversion
or an incomplete change disclosure is unavailable. Success returns
`MutationResult` plus previous revision/bundle/schema, target bundle/schema, and
the applied identity mapping. Exact request replay recovers the original result
without converting twice. Failure leaves the prior state current; migration
never rewrites completed images or retargets accepted work. Both entry points use
this grammar, including when ordinary mutations refuse the old schema.

## darktable Errors

Common envelopes/exit classes apply. Domain refusals disclose target, reason and recovery. Invalid settings/input or unsupported schema exit 2; missing state/instance/mask/module/action/resource/preset/style/revision/file/request exits 3. Stale state, changed bound input, ambiguous edit, request reuse, referenced resources, output incompatibility, destination collision and bounded lock-busy exit 4. Unavailable execution/dependencies, expired receipts/revisions, disk-full and supervised wait/execution failure exit 6. Possibly admitted writes with unconfirmed settlement use `outcome_unknown`, exit 7; confirmed atomic refusals have effect `none`. No state conflict refreshes a guard and retries automatically.

Standalone errors name paths and execution host rather than fabricated Photo IDs. Integration errors retain Photo/workspace/edit and job identities for inspection. A receipt expiry is not proof of non-admission. A stale preview is not a lost edit. Download/local I/O errors do not claim an unknown semantic write. Explicit replay uses the exact payload and identity; new intent uses a new identity and fresh decision.

## End-to-End Example

This is a sequence template, not a shell script: uppercase operands are values copied from the identified results. All file/state operations below run on the disclosed execution host or in an environment that can access its paths. Library lookup and final download may run remotely.

```text literal
slipstream photos resolve --location '2026/trip/IMG_2042.CR3'
slipstream workspace open PHOTO_ID
darktable-edit open INPUT_PATH --state STATE_DIR --preview-directory PREVIEW_DIR --output-directory OUTPUT_DIR
darktable-edit edit show STATE_DIR
darktable-edit modules list --state STATE_DIR
darktable-edit modules describe MODULE_ID --state STATE_DIR
darktable-edit edit set STATE_DIR INSTANCE_ID PARAMETER_ID --value '0.5' --if-state TOKEN --request edit-001
darktable-edit preview render STATE_DIR --request preview-001
darktable-edit edit output show STATE_DIR
darktable-edit export create STATE_DIR --request export-001
slipstream workspace files WORKSPACE_ID
slipstream workspace download WORKSPACE_ID ARTIFACT_ID --file FINAL_PATH
```

Resolve supplies `PHOTO_ID`; open supplies `WORKSPACE_ID`, `INPUT_PATH`, `PREVIEW_DIR`, `OUTPUT_DIR`, execution facts and `.edits/darktable/main` as a suggested `STATE_DIR` under the actual directory. Tool show supplies the current token and discovered instance IDs. Choose a numeric parameter whose schema admits `0.5`; do not assume a fixed universal catalog. Preview returns a readable path in that directory. Save a discovered valid output combination through `edit output set`/apply when the current contract is unsuitable; export uses those saved settings and returns `OUTPUT_PATH`. Files reconciliation supplies the indexed artifact ID for optional final download; choose the destination filename from that result so its extension matches the exported format. A same-host next independent tool instead receives `OUTPUT_PATH` directly using its own documented grammar after verifying compatibility. The same darktable sequence works on any supported local input and explicit state directory outside Slipstream. The Photographer may use optional `slipstream darktable` supervision for remote Web/CLI editing without changing directory, state or file ownership.
