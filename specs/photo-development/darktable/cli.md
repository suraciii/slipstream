# darktable CLI Contract

`darktable-edit` is an independent executable, distinct from official `darktable-cli` and the darktable GUI. Optional `slipstream darktable` invokes the same editing capability. [Headless darktable Editing](editing.md) owns behavior; [CLI Reference](../../command-line/reference.md) owns shared credentials, envelopes, and workspace grammar.

Installed command help and the command schemas own exhaustive option and field inventories. This contract owns target invocation compatibility, semantic carriers, guards, bounds, and recovery, including requirements not established by installed help.

## Grammar and Identity

Standalone `open` MUST accept an accessible regular image as `INPUT` and an explicit `--state STATE_DIR`. It MUST atomically create absent state or reopen state bound to exactly that input. A different input requires explicit rebind or another state directory.

The tool MUST work on supported local files without a Slipstream server, Photo ID, credentials, or database. It MUST NOT rewrite or rename input files or implicitly write adjacent XMP.

Within a workspace, the input and state paths use `input/<filename>` and `.edits/darktable/<edit-name>` beneath the returned workspace. Integration supplies the workspace's `previews/` and `outputs/` directories.

Standalone callers MAY supply those directories on initialization. Otherwise the defaults MUST be `STATE_DIR/previews/` and `STATE_DIR/outputs/`.

The tool MUST record the chosen directories, create them lazily, and permit explicit render destination override with `--directory`. Open MUST return state and input paths, execution host, revision, state token, and preview and output directories. It MUST NOT infer Slipstream ownership from path names or read Slipstream metadata.

The standalone target is a state directory. The integration target is a Photo ID and optional Edit ID; only the adapter requires Photo identity. Scoped catalog commands use standalone `--state` or authorized integration `--photo` and optional `--edit`; `--edit` requires `--photo`.

IDs MUST contain 1–128 ASCII letters, digits, `.`, `_`, or `-`. Tokens are opaque base64url strings of at most 512 bytes. Paths are not subject to ID restrictions; native field and action IDs MUST be stable semantic mappings obtained through discovery.

JSON command input MUST be at most 64 KiB, reject duplicate and unknown keys, and preserve array order. `--value JSON` accepts one complete JSON value. Source evidence in JSON is opaque UTF-8 of at most 16384 bytes, including escaped NUL, and is not a command-line ID.

| Limit                  | Maximum |
| ---------------------- | ------: |
| Changes per edit       |     100 |
| Instances per edit     |     256 |
| Masks per edit         |     256 |
| Structured value depth |      32 |
| Geometry edge          |   65536 |
| Schema description     |   1 MiB |

Geometry edges MUST be at least 1. Smaller disclosed qualified limits govern admission.

Standalone operational commands MUST default to one JSON envelope on stdout using the shared envelope and exit classes, without service credential options. Progress goes to stderr. File-effect results MUST identify the execution host and actual absolute paths.

A mutation MUST generate and return a request identity when omitted. A caller requiring uncertain-response recovery MUST choose and retain an identity and exact request before submission. [Atomic Editing](#atomic-editing) owns guard requirements.

## Discover and Inspect

Global module discovery MUST expose native darktable operation schemas, not the peer Processing Module catalog. Input-scoped discovery MUST disclose initialized and reset values, frames, and capability availability. Reads MUST NOT create state.

Standalone state-scoped editing requires `open`. Integration `edit show` MAY inspect a prospective workspace input without saving an edit. One existing edit is selected independently of Web selection; multiple edits MUST return `edit_ambiguous` with candidates unless explicitly selected.

Unavailable initialization MUST expose unknown values as absent, output settings as `{}`, and the image contract as null. It MUST NOT substitute static defaults, camera JPEGs, or another tool. A supplied state token MUST select and validate that exact input and state; changed evidence returns `state_conflict`.

Edit inspection MUST expose these facts:

| Subject              | Required facts                                                                                                                        |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| State                | State directory, execution host, baseline and current revisions, state token, engine, bundle, schema, and initialization availability |
| Input                | Absolute path, fingerprint, byte length, availability, bound and observed revisions, and actual frames                                |
| Output               | Complete settings                                                                                                                     |
| Preview capabilities | Nullable region and statistics schemas, and a positive file-byte bound                                                                |
| Latest Preview       | Identity, revision, and staleness                                                                                                     |
| Admission            | Applicable limits                                                                                                                     |

Each input frame MUST identify its `frameId`, width, height, coordinate schema, and transform.

Integration adds Photo, workspace, edit, and Web references. Capability status MUST distinguish `read`, `write`, and `execute`, with a reason and recovery where unavailable.

Operation descriptions MUST provide deterministic JSON Schema 2020-12 validation and initialized/reset values when known. Instance, mask, and action inspection MUST identify their semantic targets and relationships. Headless pickers require an advertised region/frame schema.

Embedded instance, mask, and action windows MUST carry items, total, and continuation, with at most 60 items. Lists default to 50 and accept 1–60. Cursor-only continuation MUST NOT accept new filters or target arguments.

A cursor MUST pin the state revision, bundle/schema, or retained history IDs as appropriate. It MUST survive command processes and service restarts, retain authorized state and catalog scope, and expire after 15 minutes idle or bounded eviction. Expiration MUST fail explicitly rather than mix pages; embedded windows continue through their corresponding list operation.

## Atomic Editing

Standalone `open` initializes baseline and output settings together. Integration `edit create`, or its first valid mutation, MUST resolve or reopen the Photo Workspace and initialize that same tool state. Explicit create permits another edit state.

Integration creation accepts `{input: {kind: "workspace-artifact", artifactId: string, revision: string}}` or the retained `{input: {kind: "original", sourceRevision: string}}` carrier. Artifact-backed state binds exactly that file revision. Original-backed requests MUST resolve once to a checked prepared snapshot matching `sourceRevision`; missing or changed evidence MUST refuse rather than substitute a refreshed input or default.

Creation MUST journal the selected artifact and checked revision and hold its input lease through initialization and receipt publication. Managed inputs are reused; mutable external inputs receive one verified independent protected snapshot under [Photo Development](../spec.md#tool-invocation-and-composition).

Standalone rebind accepts `{input: {path: string}}`; integration accepts the checked workspace-artifact carrier. Rebind MUST validate frames, resources, and compatible settings and create a revision. Observed mutation of an external input MUST fail its old binding.

Every mutation MUST acquire a shared per-state exclusive filesystem lock with bounded disclosed waiting. Under that lock it MUST check supplied state evidence before computing changes and atomically install one complete validated candidate. Conflicts and failed calculations MUST save nothing.

GUI read-modify-write, Agent read-dependent writes, atomic apply, reorder, rebind, history restore, and mapped Preset or Style application MUST require observed `--if-state` evidence. Focused no-guard commands MAY express fresh intent validated entirely against locked current state; they MUST NOT overwrite a caller-constructed full state or protect an earlier read.

Automatic actions MUST calculate under the lock or recheck the captured revision before commit. Standalone execution owns its process lifetime and MUST NOT promise detached computation after termination.

The same request ID and canonical payload MUST replay the original committed result before checking the current guard. A different intent under that identity MUST return `request_conflict`. State commit and receipt settlement MUST recover together after interruption.

Receipts MUST disclose `retainUntil` and survive restart for at least seven days after settlement. Receipt expiry MUST NOT permit reuse of an old identity as new work or expire needed state and files. A caller MUST reconcile an uncertain request exactly before dependent work.

If an auto-generated identity was not received, the caller MAY inspect state and files but MUST report attribution as unknown. Another ID cannot recover the lost admission. Only a durable supervised receipt justifies continuation after disconnect; pending settlement reports `outcome_unknown`.

`edit validate` MUST check the same candidate schema and relationships without saving, initializing prospective state, creating history or receipts, importing resources, or executing automatic actions. It MUST report validity, deterministic effects, and actions needing execution; it reserves nothing.

Atomic apply accepts a nonempty `{changes: Change[]}`. Focused commands use the same change semantics. The supported typed changes are:

| `type`                                                   | Semantic fields                                                                                                 |
| -------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `set`                                                    | `instanceId`, `parameterId`, `value`                                                                            |
| `reset`                                                  | `scope: all\|instance\|parameter`, optional `instanceId`, `parameterId`                                         |
| `instance-add`, `instance-clone`                         | module or source instance, request-local `ref`, `name`                                                          |
| `instance-enable`, `instance-remove`, `instance-reorder` | target and enabled value, or final `instanceIds`                                                                |
| `mask-create`, `mask-set`, `mask-remove`                 | request-local `ref`, kind, frame and value, or existing mask target                                             |
| `blend-set`, `action`, `output-set`                      | target and schema-admitted value or action input                                                                |
| `preset-apply`                                           | definition and version, instance, `mode: update\|replace`, initialization and mask policy                       |
| `style-apply`                                            | definition and version, `mode: merge\|replace`, entry mapping, initialization, mask policy and output selection |

IDs, names, and refs are strings; values follow discovered schemas. Null names or instance IDs are allowed where initialization or mapping requires them. Unknown keys and irrelevant reset fields MUST fail.

The exact Preset carrier is `{type: "preset-apply", presetId, version, instanceId, mode: "update"|"replace", initialization: "literal"|"target-camera", maskPolicy: "omit"|"compatible"}`.

The exact Style carrier is `{type: "style-apply", styleId, version, mode: "merge"|"replace", mapping: {entryId, instanceId: string|null}[], initialization: "literal"|"target-camera", maskPolicy: "omit"|"compatible", includeOutput}`.

The add and clone carriers use `{type: "instance-add", moduleId, ref, name}` and `{type: "instance-clone", instanceId, ref, name}`. Mask creation uses `{type: "mask-create", ref, kind, frameId, value}`. The remaining carrier fields follow their semantic targets in the table.

A request-local `ref` MUST contain 1–64 letters, digits, `_`, or `-`. Later changes refer to it as `@ref`, including blend mask references; existing IDs MUST NOT start with `@`.

Changes evaluate in order against one candidate, and results MUST map refs to stable generated IDs. Reorder MUST name each final instance exactly once.

Focused reorder accepts `{instanceIds: string[]}`; mask creation accepts `{kind: string, frameId: string, value: object}`. Action input follows its advertised schema. Reset `--all` restores initialized instances, masks, blending, and output for the acknowledged input without changing binding or other state.

The mutation result MUST identify the request, outcome, saved revision and token, retention deadline, changed targets, concrete effects, generated IDs, and current revision and token. No-op writes MUST NOT create history. A replayed committed revision MAY precede current state; the result MUST distinguish them.

## Output Contract

Output inspection MUST expose complete settings, JSON Schema 2020-12, availability, a nullable image contract, and a separately identified preview conversion. Image facts MUST describe actual format, precision, color, transfer, encoding, profile version, and positive geometry; extensions do not establish compatibility.

The qualified target includes scene-linear float32 ProPhoto TIFF and supported display outputs. It MUST NOT force every edit to TIFF.

An incompatible scene-linear and tone-mapping combination MUST conflict without disabling effects. Export MUST NOT apply hidden format, quality, geometry, or profile overrides.

## Managed Resources, Presets, and Styles

[Catalog Scope](editing.md#catalog-scope) owns catalog selection and pinned dependency behavior. Scoped operations require an existing edit and MUST NOT accept arbitrary server state paths. Cursor continuation retains selected catalog identity.

Resource kinds MUST advertise media types, maximum bytes, availability, value schemas, and frames. Import MUST validate regular-file bytes, length, type, structure, and SHA-256 and create immutable versions. Standalone reads a local file; integration streams a caller-local resource to the selected tool catalog.

Import MUST NOT accept shell directives, executable plugins, ambient desktop catalogs, or untyped native state. Removal MUST refuse versions referenced by current edits, retained revisions, catalogs, or tasks.

All catalog discovery and operations MUST return the selected catalog identity, scope (`edit-local`, `user`, or `configured`), execution host, and state directory when edit-local. Scoped integration also identifies the Photo and edit. Discovery and edit views MUST disclose both target local and configured global catalogs.

An absent catalog returns `catalog_not_found`; an absent ID or version returns `catalog_record_not_found` with selected scope. Neither permits fallback. The error MUST identify recovery through selecting the intended edit, explicitly selecting global scope, or importing the missing record into the selected catalog.

Definitions and newly introduced resources MUST resolve in the single selected catalog. Focused commands without `--catalog` use local scope; new global references use atomic `edit apply --catalog global`. Existing pinned global references MUST survive subsequent local-scope edits without another lookup.

Save MUST retain the complete dependency closure in the selected catalog. Per-change catalog overrides are forbidden. Request identity MUST include resolved catalog identity and canonical scope; replay returns the original catalog and result.

Preset and Style capture MUST use an explicit nonempty selection. `literal` captures values; `target-camera` captures schema-declared initialization directives rather than source-camera values. Unsupported directives MUST refuse capture.

Preset save accepts `{name, parameterIds, includeBlend, initialization, maskPolicy}`. Style save accepts `{name, instances: {instanceId, parameterIds, includeBlend}[], includeOutput, initialization, maskPolicy}`.

Interchange representations MUST retain these semantic carriers:

| Carrier            | Required meaning                                                                                      |
| ------------------ | ----------------------------------------------------------------------------------------------------- |
| Resource reference | `{resourceId, version}` identifies an immutable version.                                              |
| Definition         | Initialization, mask policy, resource versions, and a complete mask graph or null for no masks.       |
| `Settings`         | `{moduleId, parameterSchemaVersion, parameters: {parameterId, value}[], blend}`.                      |
| Preset             | `settings: Settings` selects one operation's fields.                                                  |
| Style              | Nonempty `entries: ({entryId} & Settings)[]` and nullable `output`; null preserves the target output. |
| Mask graph         | `{schema, value}` with JSON Schema 2020-12 and typed references.                                      |

Native blobs, executable schemas, arbitrary paths, and dangling references MUST NOT appear in mask graph values. Each parameter MUST occur once and use a schema-admitted value or initialization directive. Null blend means omitted, not a source-image default.

Definition results MUST expose these facts:

| Result         | Required facts                                                                                       |
| -------------- | ---------------------------------------------------------------------------------------------------- |
| Summary        | ID, version, kind, name, bundle, representation schema, UTC RFC 3339 creation time, and availability |
| List           | Catalog, items, total, and next cursor                                                               |
| Show           | Selected catalog and complete record                                                                 |
| Save or import | Selected catalog, complete record, request identity, created/replayed outcome, and receipt deadline  |

Removal results MUST identify catalog, definition ID and version, request ID, and removed/replayed outcome. Export results MUST identify catalog, ID, version, path, SHA-256, byte length, and `fileCommitted: true`.

Definition IDs MUST remain stable within their catalog and versions immutable. Definition `schemaVersion` identifies the representation, not an operation's parameter schema. Style entries MUST have unique record-local IDs in captured processing order, independent of instance IDs and array indices.

A complete show MUST return every selected entry, parameter, and dependency within the advertised definition byte bound. Save or import MUST refuse an oversized definition rather than truncate it. Exact replay of save or import MUST return the same definition identity and version.

Captured mask graphs MUST include their complete referenced closure, schema, frames, source-stage evidence, and source-input facts needed for compatibility validation without reopening the source workspace. Record-local IDs and typed references MUST be consistent, including Style entry IDs or reserved `self` for a Preset. Cycles, dangling references, and exceeded limits MUST refuse capture or import.

`maskPolicy: compatible` MUST validate actual target frames and input, allocate target masks once, preserve shared references, and rewrite typed relationships atomically without guessing transforms. `omit` MUST exclude captured masks and mask-dependent fields, preserve target relationships, and refuse invalid results. Omitted fields MUST be absent rather than null or source IDs.

[Presets and Styles](editing.md#presets-and-styles) owns mapping and Merge/Replace semantics. Validation and application MUST report catalog mappings and reset, preserved, created, changed, relationship, and output effects.

Validation uses null generated IDs and reports planned ordering; application returns committed IDs. Neither reports an implicit contiguous replacement range.

The Style apply carrier is `{mode, mapping: {entryId, instanceId: string|null}[], initialization, maskPolicy, includeOutput}`. Effects use `{resetInstanceIds: string[], preservedInstanceIds: string[], createdInstances: {entryId, instanceId: string|null, moduleId, order}[], changedTargets, relationships, output}` per application.

Interchange JSON uses `{schemaVersion: 1, kind: "preset"|"style", record: PresetRecord|StyleRecord}` with matching outer and record kinds. It MUST contain the complete shown record, ordered entry IDs, masks, relationships, and resource versions. Import MUST validate schema, bundle, and dependency closure, allocate a new identity/version, and preserve record-local references without overwriting by descriptive file identity.

Interchange MUST NOT authorize arbitrary external file access or bypass qualification. Export MUST use no-replace publication. Import and export stream under a separately advertised positive byte maximum, not the mutation JSON limit.

## Preview and Inspection

Standalone Preview MUST synchronously capture current or explicitly selected retained state and render from that immutable capture without blocking later unrelated edits. Default filenames MUST be unique; explicit names MUST NOT replace existing files.

Optional preview input accepts `{comparison: {revision: string}|"baseline"|null, region: object|null}`. Integration MAY also select a comparison edit.

Baseline means native initialization under the same input and schema, never the camera JPEG. Preview MUST NOT save state.

Preview results MUST identify the captured input and revision, bundle and schema, conversion, frame, scale, dimensions, detail limit, digest, length, and available native statistics. Statistics MUST distinguish working and display scope and MUST NOT label display clipping as sensor clipping.

Only the Slipstream adapter MAY admit supervised jobs. It MUST capture current or explicitly requested revision before queueing and promptly return a durable `previewId` with `accepted`, `executing`, `succeeded`, `failed`, or `cancelled` state. The receipt MUST retain request, timestamps, deadline, image and comparison evidence, statistics, and failure.

Status and wait MUST inspect the accepted job rather than render again. Client disconnect or timeout MUST NOT cancel admitted work. Bounded wait returns `wait_timeout` and the handle while pending; terminal failures MUST be reported.

Standalone interruption MUST NOT publish partial files. Exact replay MUST reconcile its durable receipt.

Comparison download retrieves only the already captured comparison. Downloads use bounded verified no-replace transfer. Development Proxy read, removal, and qualified rebuilding remain available under [Development Proxy](../design.md#development-proxy); a proxy MUST NOT substitute for a missing workspace input or authorize Original-backed Export.

## Export and Download

Standalone Export MUST synchronously capture current or named retained revision, input fingerprint, dependencies, bundle, and complete output settings. It MUST validate the capture and complete a unique file without replacement. An explicit name collision MUST refuse.

A completed result MUST identify the absolute path, revision, input evidence, output contract, digest, length, request identity, and `fileCommitted: true`. Failure MUST preserve saved adjustments and earlier files; disk exhaustion MUST NOT silently lower precision or resolution.

The adapter MAY supervise durable jobs against the same state directory. Cancellation follows the first terminal decision. Retry MUST create a new attempt from the original failed or cancelled capture, never current state.

Exact replay MUST attach to the original attempt before checking current state; changed payload MUST conflict.

Only accepted service work MAY continue after the client exits. Workspace indexing and provenance follow [Photo Workspace](../workspace/spec.md#export-download-and-continue); a discovered file alone MUST NOT establish an uncertain Export's success.

The adapter Export record MUST retain request and integration identities, captured revision and input, output contract, bundle and schema, lifecycle timestamps, failure, retry source, and artifact when known. Receipt expiry MUST NOT expire current workspace artifacts or state. Historical artifacts retain truthful historical retention facts.

Downloads MUST verify the bound revision, decoded image contract, structure, length, and SHA-256 before no-replace publication. Existing files and symlinks MUST NOT be overwritten. A reporting failure after publication MUST preserve the file and report the committed effect.

Resumable transfer follows [CLI Photo Development](../../command-line/reference.md#photo-development).

State-file export MUST write complete confirmed native XMP state as explicit interchange, including dependency identities and disclosed optional Camera Raw mappings or omissions. It MUST NOT require engine execution, modify an Original or Sidecar, or claim to be the authoritative resumable state directory. Historical download MUST preserve original bytes and metadata without inventing provenance.

## Saved History and Recovery

[History and Recovery](editing.md#history-and-recovery) owns retention and restore behavior. History inspection MUST expose parent, time, request identity, changed targets, availability, pinned status, and complete canonical semantic state without decoding native blobs.

Restore MUST validate settings, resources, and frames against the acknowledged binding. After explicit rebind, it applies compatible historical adjustments to the new input rather than reviving the old binding. Incompatible coordinates MUST refuse without a silent transform.

Reading unsupported state MUST NOT silently convert it. Migration MUST retain historical semantic state, XMP receipts, image bytes, and dependencies. An adapter projection MAY retain old step or recipe identities without becoming the parameter owner.

`edit migration show` MUST be read-only and return source and target bundle/schema identities, availability, refusal reason, a bounded complete change list and identity mappings, `stateToken`, and `migrationToken`. The migration token MUST bind the disclosed conversion, converter, target assets, and source state; it is null when no supported complete conversion exists. Inspectable incompatible state MUST still supply these guards.

`edit migrate` MUST require the reviewed migration token, state token, request identity, and explicit confirmation. Under the state lock it MUST validate the conversion and target dependencies, retain the original state and dependencies, and atomically install a new revision. Changed source, converter, or assets MUST refuse without refreshing approval.

Lossy conversion or incomplete disclosure MUST be unavailable. Success MUST report previous and target revision, bundle, schema, and identity mappings. Exact replay MUST NOT convert twice; failure MUST leave prior state current.

Migration MUST NOT rewrite completed images or retarget accepted work. Both entry points MUST support this grammar even when ordinary mutation refuses the old schema.

## darktable Errors

Domain refusals MUST identify target, reason, and recovery. Common envelopes and exit classes apply:

| Exit | Conditions                                                                                                                                       |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| 2    | Invalid settings or input; unsupported schema                                                                                                    |
| 3    | Missing state, object, resource, revision, file, or request                                                                                      |
| 4    | Stale state, changed input, ambiguous edit, request reuse, referenced resource, incompatible output, destination collision, or bounded lock-busy |
| 6    | Unavailable execution or dependency, expired receipt or revision, disk exhaustion, or supervised wait/execution failure                          |
| 7    | Possibly admitted write with unconfirmed settlement (`outcome_unknown`)                                                                          |

Confirmed atomic refusals MUST report effect `none`. State conflicts MUST NOT automatically refresh guards or retry. A receipt expiry is not proof of non-admission.

Standalone errors MUST identify paths and execution host rather than fabricate Photo IDs. Integration errors MUST retain Photo, workspace, edit, and job identities as applicable. Download or local I/O errors MUST NOT claim an unknown semantic write.
