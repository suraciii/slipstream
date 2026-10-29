# darktable Integration

[Photo Development](../spec.md) owns shared editing, composition, and Original safety. [Photo Workspace](../workspace/spec.md) owns file discovery and retention. [Headless darktable Editing](editing.md) owns editing behavior; [darktable CLI](cli.md) owns invocation and compatibility semantics.

## Relationship

`darktable-edit` MUST edit accessible files independently of Slipstream. It is distinct from official `darktable-cli` and the desktop GUI. It MUST NOT require a Slipstream server, Library, Photo ID, database, token, or personal desktop catalog.

The tool owns its saved state and qualified engine bundle. Optional `slipstream darktable` and Web controls MUST invoke the same tool against that state, without maintaining another authoritative parameter store.

Existing compatible processing interfaces MAY project canonical file state. Historical edits and files MUST be preserved during an explicit supported migration; compatibility aliases MUST NOT be invented. [Photo Development Architecture](../design.md) owns service-state conversion.

## Local Availability

Independent operation MUST work without a separately operated service. Slipstream MAY package or supervise the tool without requiring a personal desktop catalog, processing control socket, or separate engine service.

The execution host and directory accessibility MUST be disclosed. A remote caller MUST use an authorized environment with access to the files; the integration MUST NOT expose an arbitrary remote shell.

Missing or invalid engine assets MUST leave Library facts, files, and saved intent usable and identify the unavailable operation. CPU-only headless execution MUST be qualified; a GPU MUST NOT be required. Standalone SpektraFilm availability is independent of darktable availability.

State guards and execution capacity are separate. A state lock MUST NOT block unrelated workspace renders or file completions. Cancellation MUST report the affected process outcome truthfully; a terminated standalone process MUST NOT be advertised as a surviving service job.

## Available Controls

[Editing capability discovery](editing.md#discovering-editing-capabilities) owns control meanings and capability status. General editing MUST NOT become a permanent exposure and white-balance allowlist.

The native engine describes operations and schema identities. The tool MUST validate and persist their meaning under the same contract used for execution, without a competing binary-layout implementation or a bespoke command for every control.

Unsupported settings MUST remain inspectable with specific limitations. An incompatible engine MUST refuse mutation and rendering without resetting or reinterpreting saved intent. [Migration](cli.md#saved-history-and-recovery) is explicit and retains the prior state.

Web controls MUST preserve settings they cannot display, including CLI-created instances, curves, masks, resources, and output choices.

## Automatic Adjustments

An automatic adjustment MUST be an explicit action on a captured input and state revision. The native engine MUST compute concrete values and dependencies; the tool MUST save them atomically under its state guard. Slipstream MUST NOT implement a competing adjustment or recipe.

Each action MUST declare its inputs, affected instances, source or region constraints, and results. Preview and Export MUST reuse accepted values rather than recompute detection.

Failure, unavailable resources, changed input evidence, or a stale revision MUST leave saved state unchanged. Exact-request recovery MUST NOT repeat an uncertain action against newer state; changed intent under the same identity MUST conflict.

## Editing and Outputs

Preview and Export from the same captured input and settings MUST apply the same intended corrections and baseline. Qualified resolution and display conversion MAY differ; preview conversion MUST NOT become the final image input.

Opening state and rendering MUST NOT reset or mutate settings. Export MUST NOT require a Preview. The selected output contract MUST distinguish scene-referred from display-encoded output and refuse incompatible combinations without silently disabling effects.

Saving and rendering are separate outcomes. Rendering MUST capture immutable input and state evidence; later edits MUST NOT retarget accepted work. An older preview MUST remain identifiable as stale and MUST NOT replace the current preview; an older output MUST NOT be attributed to newer state.

File completion, discovery, provenance, download, and explicit handoff follow [Photo Workspace](../workspace/spec.md) and [Photo Development](../spec.md#export). Standalone execution is synchronous; optional supervision MUST preserve the same file and state contract.

## Failure Behavior

Invalid settings MUST identify the affected control and reason. The tool MUST NOT round enums, truncate integers, substitute zero, drop unsupported objects, or ignore corrections.

A stale guard MUST conflict without overwriting newer state. Exact-request replay MUST reconcile the original mutation without duplication and separately identify any newer current revision. [CLI mutation rules](cli.md#atomic-editing) own guard and receipt details.

An engine failure after a confirmed save MUST leave that state intact. Cancellation, interruption, memory exhaustion, and disk exhaustion MUST preserve prior completed files and confirmed state; partial outputs MUST NOT be offered as results.

The tool MUST NOT substitute a camera Preview, uncorrected result, alternate engine, reduced dimensions, or disabled effect for a successful requested result. Input binding and recovery follow [Headless darktable Editing](editing.md#inspecting-and-starting-an-edit).
