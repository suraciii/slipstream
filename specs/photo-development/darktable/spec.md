# darktable Integration

A Photographer needs saved corrections to produce the intended photograph in
preview and output while keeping the files available for later use. darktable
editing must work independently on files; Slipstream adds a convenient Photo
Workspace and optional controls without creating another settings store.
Engine upgrades must not silently reinterpret corrections or expose unsupported
controls.

[Photo Workspace](../workspace/spec.md) owns directory and file behavior.
[Photo Development](../spec.md) owns shared editing and output behavior.
[Headless darktable Editing](editing.md) owns the general semantic
editing surface. This specification owns the engine relationship; published
support remains subject to the [release boundary](../../deployment/support.md).

## Relationship

The proposed independent `darktable-edit` executable owns local image editing.
It reads an input file, persists native JSON state in an explicit state directory,
and writes preview and output files to accessible directories. Its dependencies
are its own qualified darktable bundle, input files, and declared resources. It
must not require a Slipstream installation, server, Library, Photo ID, database,
HTTP token, or personal desktop catalog. This command is distinct from official
`darktable-cli` and from darktable's GUI.

Slipstream owns Photos and their workspace association, file indexing, access,
and optional supervised execution. Its `slipstream darktable` integration may
resolve workspace input/state paths and invoke the independent tool. Web controls
must invoke that same tool on the same on-disk state. Slipstream may index a state
pointer and project settings for inspection, but must not mirror authoritative
parameters in its database. [darktable CLI Reference](cli.md) owns the
integration grammar and standalone invocation contract.

There is no download/edit/upload/submit cycle or state-copy commit between the
tool and Slipstream. Completed ordinary files become visible through workspace
reconciliation without a tool callback. Optional structured results can establish
verified provenance but are not a requirement for file discovery. The native
engine protocol remains an internal tool concern rather than a public engine
catalog escape hatch.

Existing compatible processing interfaces may remain projections over canonical
file state. A cutover must preserve historical edits and files and disclose any
explicit supported migration; it must not discard them or invent compatibility
aliases. Library identity and decisions remain independent of editing.

## Local Availability

Independent operation must not require a separately operated Slipstream service.
A local user runs the executable against accessible paths. Slipstream may package
or supervise it in the application's execution environment for Web use without
requiring a personal darktable desktop catalog, processing control socket, or
separate engine service in the normal local flow.

Workspace opening must disclose the actual execution host and directory
accessibility. A remote caller needs an authorized environment with access to
those paths; the integration must not imply that server files exist on the
caller's computer or introduce an arbitrary remote-shell endpoint. A final
download does not become a mandatory synchronization step for later edits.

Missing or invalid engine assets leave the Library, files, and saved state usable
and identify which editing operation is unavailable. CPU-only headless execution
must be qualified; a GPU is not mandatory. Standalone SpektraFilm has separate
availability and must not be inferred from darktable availability.

Per-state write guards prevent concurrent corrupting edits. They must not lock
the entire workspace during unrelated rendering or file completion. Supervised
execution observes its configured processing capacity, separately from state
locks. Cancellation must settle the affected process truthfully; standalone work
must not be advertised as a surviving service-owned job after its process ends.

## Available Controls

[Discovering Editing Capabilities](editing.md#discovering-editing-capabilities)
owns native control meanings, initialization, reset, instances, ordering,
structured shapes, and capability status. The tool must support general editing
rather than a permanent exposure/white-balance allowlist. Parameter schemas alone
do not qualify camera mappings, GUI actions, masks, or output combinations.

The native engine describes operations and their schema/version identities.
The editing tool validates and persists that meaning using the same declared
contract used for execution. An adapter must not maintain a competing set of
binary layouts or a bespoke command for each new control. Unsupported settings
remain inspectable with specific limitations rather than being omitted or
presented as executable.

Native schema compatibility is part of saved state and discovery. An incompatible
engine must refuse mutation or rendering while preserving readable intent.
An upgrade must not reset or reinterpret settings silently. Supported migration
is explicit, discloses changes, validates the new state, and retains the old one.
Opening a basic Web control must preserve CLI-created instances, curves, masks,
resource references, and output settings it cannot present.

## Automatic Adjustments

An admitted automatic adjustment is an explicit action on one captured state
revision and input. The native engine evaluates its own algorithm and returns
concrete values and dependencies sufficient to reproduce the accepted intent.
The tool atomically saves them under its state guard. Slipstream must not
implement a competing correction algorithm or independently save its own recipe.

Automatic exposure and illuminant detection are examples rather than an
allowlist. Other actions declare inputs, affected instances, region/source
constraints, and results under the same headless contract. Preview and Export
reuse saved concrete settings and do not independently recompute detection.

Failed detection, unavailable resources, changed input evidence, or a stale
revision leaves saved state unchanged. Exact-request recovery must not repeat
an uncertain action against newer state. Reusing a request identity for another
intent must conflict rather than apply a second correction.

## Editing and Outputs

An Edit Preview and an Export from the same captured input and saved settings
apply the same intended corrections against the same processing baseline.
Resolution and display conversion can differ under qualified output contracts.
Preview conversion must not become the final image's input. Neither opening an
existing state nor requesting a render may reset or mutate settings.

Export requires no prior preview. Output format, precision, color, transfer,
profile, and geometry are saved tool choices, not a fixed module pipeline.
Scene-referred and display-encoded outputs must retain their distinct meanings.
An incompatible output/settings combination fails explicitly rather than silently
disabling artistic operations or declaring an image contract from extension.

State saves are atomic and separate from rendering. Rendering captures immutable
settings/input evidence so a later edit cannot retarget accepted work. A completed
preview from an earlier revision stays marked stale and must not replace the
current preview. An older output remains available without being called an
output of the new state.

The tool writes files directly into the workspace's preview/output areas. Unique
default names and no-overwrite completion preserve concurrent results. Private
partial files become ready only through atomic completion. Slipstream discovers
ordinary completed files independently of invocation callbacks and reconciles
missed events after restart. It must not invent tool success or Processing
Artifact provenance for a file whose producer is unknown.

A standalone command owns synchronous execution. Optional Web supervision may
retain bounded run status and cancellation; it must not alter the independent
file/state contract. Outputs can be downloaded or explicitly selected as a
compatible input to another tool. There is no automatic chaining, reprocessing,
new Photo, or Album import.

## Failure Behavior

Invalid settings fail with the affected control and reason, without rounding an
enum, truncating an integer, substituting zero, dropping unsupported objects, or
ignoring the correction. A stale guarded write conflicts and preserves the
newer state; a busy state reports bounded waiting or refusal. Exact-request replay
reconciles the original mutation without duplicates and identifies a newer current
revision separately.

An engine failure after confirmed save leaves the saved state intact and fails
only the render. Cancellation, interruption, insufficient memory, and disk-full
must preserve prior completed files and confirmed state. Partial outputs must not
be offered for download or later processing. No camera Preview, uncorrected
result, alternate engine version, reduced dimensions, or disabled effect may be
substituted as a successful requested result.

Original and external XMP safety follows [Photo Development](../spec.md).
The input is the independent workspace snapshot, not a writable Library link.
Its unchanged bytes remain editable when the live Original disappears or changes.
Explicit source refresh adds another input version and does not retarget saved
state. A missing or changed workspace input produces a dependency failure and
must not silently reopen the live Original.

Workspace files and saved state are durable under
[Photo Workspace](../workspace/spec.md); no fixed seven-day expiry applies to
needed inputs, state, dependencies, or kept outputs. Cleanup and resource removal
must respect retained references. Restart restores confirmed on-disk state and
discovers completed outputs even when no callback was received. Interrupted work
must be reported honestly, not automatically rerun as an uncertain correction.
Normal browsing, Selection State, Rating, and Album membership remain available
independently of engine availability.

## Examples

- An Agent runs `darktable-edit` on a local RAW with an explicit state directory
  and writes a TIFF without a Slipstream server or a prior preview.
- Slipstream opens a Photo Workspace; its CLI integration and optional Web
  exposure control use that same state. A curve and mask created by the standalone
  tool remain present after the Web change.
- A native tool writes a complete uniquely named output without calling
  Slipstream. Files discovers it; after restart it is still downloadable and can
  be explicitly chosen as another tool's compatible input.
- Two output runs finish concurrently. Neither overwrites the other's file or
  gets attributed to the other's request.
- A saved white-balance mode cannot execute after an engine upgrade. The values
  stay readable with their native schema identity; no as-shot fallback is applied.
- Disk fills during a later save or Export. Earlier state and completed outputs
  remain available, and the incomplete file is not presented as a result.
