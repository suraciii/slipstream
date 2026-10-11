# Photo Editing Model

A Photographer or Agent needs to open a Photo's persistent directory, adjust its files with independent tools, inspect completed previews, and export a file for download or another compatible tool. The model must make that path direct without transferring every edit or keeping competing parameter stores.

[Photo Workspace](workspace/spec.md) owns observable directory behavior. [Directory Photo Workspace](workspace/design.md) owns preparation, reconciliation, containment, retention, and migration. [Headless darktable Editing](darktable/editing.md) owns general editing capabilities; [darktable CLI](darktable/cli.md) owns standalone and optional supervised grammar.

## Model

A **Photo Workspace** is one persistent real directory associated with a Photo. It contains protected input snapshots, tool-owned editing state, completed previews and outputs. A Photo and its Library decisions remain independent of these files.

An **Edit State** is the current confirmed editing state owned by one independent tool in its supplied state directory. It contains its Processing Engine, explicit input binding, Engine Module Controls, instances, masks, resources, output settings, and saved revision. Web controls, standalone calls, and optional Slipstream integration read and mutate that same canonical state. Separate edits may coexist without a second service parameter store.

A **Processing Engine** is a capability such as darktable or standalone SpektraFilm. An **Engine Module** is an addressable operation inside that engine. A **Control** has a defined meaning, validation, and reset behavior. Readable, writable, and executable capability facts remain separate; discovery does not qualify every input or operation.

A **Workspace Artifact** is a completed contained file with a checked content revision. A **Processing Artifact** is its narrower provenance-bearing case: a validated completed Export with exact input, state, bundle, and image-contract evidence. Ordinary compatible files remain useful without invented producing parameters or successful Export attribution.

Edit Recipe and Processing Step name internal snapshots and invocation records, not additional daily editing objects or a workflow graph. Native parameter layouts, blobs, catalogs, and handles remain private to the independent tool and engine. Slipstream owns Photo identity, workspace association, contained-file discovery, delivery, access, and optional supervised execution.

## Behavior

The normal editing flow is:

1. Open or reopen the Photo Workspace and obtain its real directory and execution/access facts.
2. Select an available tool and explicit protected input, or resume its matching saved state.
3. Inspect Controls and change settings under the observed saved revision.
4. Read the confirmed tool state after an atomic mutation.
5. Request a bounded Edit Preview when needed; the tool finishes its file in the workspace.
6. Export the captured confirmed state directly, without a prior Preview.
7. Download the exact completed file or explicitly select it for another compatible tool.

Only initial preparation or explicit source refresh reads the live Original. Subsequent edits and renders reuse the protected input. A missing or changed live Original does not invalidate intact workspace input, saved state, or completed files. Refresh creates or reuses verified bytes without retargeting existing state.

A direct Agent may invoke the independent file/state tool using paths accessible in its execution environment. Optional Slipstream integration resolves authorized Photo/edit identities to those same paths and state. A remote server pathname never implies local client access. The Agent does not reconstruct engine blobs or use a private native protocol.

Preview captures input, saved revision, resources, bundle, geometry, and display conversion. A later accepted edit makes it stale; late completion cannot replace a newer current preview. Preview files are viewable and downloadable, but must not masquerade as full-resolution scene-linear inputs.

Export captures confirmed state and exact input/resources before execution. The tool validates its actual output contract and atomically finishes a unique file without replacement. File discovery requires no upload, registration, callback, or commit operation. Processing success and indexing readiness are separate facts.

Darktable and standalone SpektraFilm are peer tools. A caller explicitly selects a completed compatible file for another invocation; actual precision, color, transfer, and geometry determine compatibility. No implicit conversion, upstream rerun, or automatic pipeline follows from file arrival or an earlier edit. Each tool owns its own state, and upstream changes never retarget downstream input.

## Failure and Retention

A stale revision, incompatible input, unavailable capability, missing resource, capacity refusal, or failed render must preserve previous confirmed state and completed files. Saving succeeds independently of a subsequent rendering failure. Uncertain writes replay the exact original request identity; current-value equality or a newly observed file does not prove that an uncertain request succeeded.

Independent command lifetime belongs to its process. Optional supervised work may remain queryable after a client departs; neither mode invents surviving computation after machine restart. Reconciliation discovers completed files after missed notifications and does not adopt unknown independent output as a successful supervised job.

Tools own supported persistent history, resources, Presets, and Styles under their specialist contracts. Restore and composite mutations use the same guarded state boundary. Unsupported historical intent remains inspectable with exact missing or incompatible dependencies disclosed; migration never silently resets it.

Inputs, confirmed state, required resources, and kept outputs persist until explicit dependency-safe removal. Receipt or cache expiry must not erase them. Removing the default input does not recreate it on reopen. Whole-workspace deletion requires its reviewed complete loss inventory and exclusion of active or unknown use. Original Files and external XMP Sidecars remain unchanged.

## Terminology

Use Edit State for tool-owned saved intent, Processing Engine for a capability, Engine Module and Control for qualified operations and values, Edit Preview for a bounded rendition, Export for explicit materialization, and Workspace Artifact or Processing Artifact according to the evidence actually available.

Avoid Edit Recipe as a second parameter owner, Processing Step as a user-visible workflow stage, native blobs as public state, and latest result as an input identity. The [canonical glossary](../../CONTEXT.md) owns domain definitions.
