# Command-Line Use

Slipstream provides the `slipstream` client for a Photographer or their external Agent. It shares service-owned facts and domain rules with the Web and does not require browser automation or direct storage access.

[Command-Line Architecture](design.md) owns the client boundary and concurrency. [CLI Reference](reference.md) owns common command syntax and machine results. [darktable CLI](../photo-development/darktable/cli.md) owns the independent editor and its optional Slipstream integration.

## Product Boundary

Slipstream MUST expose discoverable, composable operations for delegated use. It MUST NOT include an Agent, model provider, conversation, planner, prompt runner, or autonomous workflow engine, or send Photos to an AI provider itself.

The external Agent owns interpretation, planning, visual judgment, and communication with the Photographer. Slipstream owns valid operations, Original safety, persistence, and truthful results. Model judgments MUST NOT be presented as measured Photo metadata.

A shortlist MAY be an ordinary Album. Not choosing a Photo for it MUST NOT imply `rejected`, and Agent confidence MUST NOT become Rating. Slipstream does not add a suggestion, review-task, or Agent-memory model.

## Supported Work

The CLI MUST let its caller:

- Inspect service compatibility, Library availability, and scan status.
- Discover Albums and read-only Original Folders.
- Query Photos by source, Selection State, Rating, Original kind, availability, and camera-local Capture Time.
- Obtain current Photo facts and Album membership.
- Download one Photo's thumbnail or review Preview.
- Change Selection State or Rating for identified Photos.
- [Read and save metadata](../photo-library/metadata/spec.md) with provenance and checked Sidecar changes.
- Create, rename, and delete Albums and add, remove, or order their members.
- Remove explicitly identified eligible Photos and Restore observed removals under the [Removal contract](../photo-library/removal/spec.md).
- Inspect Trash Locations for external action, explicitly request record cleanup, and recover its outcomes under the [Trash contract](../photo-library/trash/spec.md).
- Inspect unavailable Photos and review and apply confirmed [Location Recovery](../photo-library/spec.md#manual-recovery) mappings.
- Request a Library check and return a browser Destination for a Photo or Album.
- Open a [Photo Workspace](../photo-development/workspace/spec.md), inspect its files, download or safely remove artifacts, and use independent tools or the optional [darktable integration](../photo-development/darktable/editing.md).

Help MUST distinguish Location Recovery from Trash Restore and explain that neither writes Original Files. It MUST advertise inspection and application limits.

Trash inspection MUST expose the Photo identity, removal marker, Original identity, and remembered Location needed for review. Cleanup help MUST describe selection and batch limits, fresh checks, blockers, per-item outcomes, and attempt inspection or replay.

The CLI MUST NOT expose arbitrary Original downloads or mutation, XMP synchronization, Library Expansion, SQL, an arbitrary server path, or a shell argument. Workspace downloads follow the workspace contract and do not grant arbitrary Library access.

## Starting and Discovering Capabilities

[Local Authentication](reference.md#local-authentication) owns instance credential commands and storage. One invocation addresses one explicit or default service URL and MUST NOT silently start or select another service.

The client MUST run without Original Files, server SQLite, or native Preview libraries on its host. Top-level help MUST describe command groups. Command help MUST describe required inputs, effects, limits, and result inspection.

Help and version MUST work offline. A versioned reference and short Agent guide MUST ship with the client without requiring repository access.

Operational commands MUST default to structured JSON. Human-readable output MUST be explicit; terminal attachment or piping MUST NOT change the schema.

Commands MUST NOT open a browser, editor, or pager. Commands other than `auth login` MUST NOT open a confirmation prompt. [Local Authentication](reference.md#local-authentication) defines login's optional terminal interaction and noninteractive forms.

A complete mutation command expresses caller intent, not proof of human authorization.

Album names and filenames are data. Output and help MUST NOT encourage executing Library text as shell commands or Agent instructions.

## Working in a Photo Workspace

Workspace open accepts the indexed Photo ID and MUST identify the actual directory, read-only input, execution host, access mode, and available supervised tools. A server path MUST NOT imply local client access.

The [workspace contract](../photo-development/workspace/spec.md) owns shared tool state, file discovery, protected inputs, retention, and safe cleanup. Independent tools use authorized filesystem access; service supervision MUST NOT provide an arbitrary remote shell.

Web and Agent edits share tool state and guarded concurrency under the [workspace design](../photo-development/workspace/design.md#editing-and-execution). Editing MUST NOT reinterpret camera Preview commands.

## Finding Photos

The service MUST apply filters and order before pagination. A query selects All Photos, one Album by ID, or one recursive Original Folder by Library-relative Location; filters combine with logical AND.

Photo and Album IDs are opaque references. Display names and filenames MUST NOT substitute for identity, and Album lookup MUST use the existing naming comparison.

`photos resolve` MUST resolve an exact Library-relative Original Location to the same Photo ID as a query. It MUST NOT import a file, accept a host path, or select the first duplicate filename. Scan readiness and an absent, unindexed, or Removed Photo MUST remain distinct.

Capture Time ranges use camera-local time, an inclusive lower bound, and an exclusive upper bound. A bounded query MUST exclude missing Capture Time and MUST NOT convert offsets or guess filesystem time. Without a bound, missing values remain eligible under the existing ordering rule.

The first page MUST fix the ordered matching Photo IDs and count. Later pages MUST traverse that set while reporting current facts. Results MUST identify continuation and evaluation time.

An expired continuation MUST fail explicitly without silently starting a new query. An empty match succeeds, a missing source fails distinctly, and no omitted limit means the complete Library.

The CLI MUST NOT offer unbounded query-wide writes. Callers submit explicit bounded IDs; multiple commands are separate commits, not one hidden transaction.

## Viewing Photos

Preview retrieval MUST provide a local JPEG, not only a private server URL. It MUST identify the Photo, Preview Source, actual dimensions, detail limit, source revision, and local path.

[Photo Previews](../photo-library/previews/spec.md) owns source selection and quality. A thumbnail MUST NOT be described as precise focus evidence, and the CLI MUST NOT substitute a sibling JPEG, develop RAW data, upscale evidence, or silently return stale data.

The caller chooses the output path. The CLI MUST refuse an existing file or symbolic link and MUST NOT truncate or replace a local Original. [Preview Transfer](design.md#preview-transfer-and-local-files) owns publication and failure handling.

One command downloads one image; the Agent chooses what to inspect. Preview reads MUST NOT change decisions, membership, or saved browsing position, precompute the Library, or upload images elsewhere.

## Changing Photo Decisions

Each Photo decision write MUST carry the observed decision version. A concurrent Selection State or Rating change invalidates it even if the value changes away and back. Preview completion and scan progress alone MUST NOT cause conflicts.

A command changes one field. Clearing Selection State means `unflagged`; clearing Rating means zero. Neither changes the other field or a browsing position.

A bounded batch MUST report changed, unchanged, conflict, or missing for every requested Photo. Conflicts and missing Photos MUST NOT block valid siblings. Invalid syntax, duplicate IDs, and over-limit batches MUST be refused before writing; storage failure MUST roll back the transaction.

Confirmed changes MUST report prior and resulting decisions. Conflicts MUST provide enough current facts to investigate. A later correction is another checked write; browser Undo remains local and the CLI does not promise durable undo history.

## Managing Albums

[Photo Library and Albums](../photo-library/spec.md#physical-and-virtual-organization) owns naming, membership, order, and deletion safety.

A change to an existing Album MUST carry its observed version. The version covers name, membership, and order, but not saved browsing position.

Additions append new Photos in supplied order and preserve existing positions. Results MUST distinguish new from existing members and removed from already-absent members. A stale version or missing requested Photo MUST refuse the entire membership operation.

Reorder MUST name every current member exactly once. Albums exceeding the documented request limit MUST be refused rather than reordered partially; they remain queryable and support bounded additions and removals.

Creation MUST NOT silently reuse an existing name; a conflict identifies the existing Album. Deletion MUST report that only the Album was deleted.

CLI reads, additions, and decision writes MUST NOT move saved position. Removal of the saved member and Album deletion follow the existing saved-position rules and report their result.

## Checking the Library

A check uses the shared scan lifecycle. Timeout or interruption MUST NOT be described as cancelling an admitted scan; status remains available afterward.

A completed check changes future query membership, not an open query. The CLI MUST distinguish initialization, failed checking with prior data retained, and an empty published Library.

## Results and Recovery

Every completed command MUST return confirmed data or an actionable failure. Writes MUST identify effects; possible admission without a trustworthy response MUST report an unknown outcome. [CLI Reference](reference.md#output-envelope) owns the envelope and [Error Details](reference.md#error-details) owns recovery classifications.

The CLI MUST NOT transparently retry a write, overwrite a Preview, split a batch, or change its target after failure. A committed local Preview MUST remain identifiable after reporting failure.

After an unknown outcome, the caller inspects affected objects. Present state does not prove which caller produced it. A new write requires fresh versions and a new decision; ordinary decision and Album writes do not require an operation ledger or exactly-once claim.

A multi-command task can stop after some commands succeed. Slipstream MUST NOT claim to roll back the task, and the Agent MUST report confirmed effects and unresolved work.

## Moving Between CLI and Web

Photo and Album results MUST include an available browser Destination. [Destinations](../library-browser/experience/spec.md#destinations-and-browser-history) are current views, not historical results or access grants.

A query with filters absent from the Web MUST NOT return a link that appears to preserve them. Individual Photo links and an explicitly created Album provide handoff without implying new Web filters.

Reloading affected Web facts MUST reveal CLI-written decisions and membership. This contract does not promise live push or alter an open browser source's fixed order; existing refresh, conflict, and saved-position rules apply.

## Support Boundary

The first qualified CLI target is Linux amd64. It MUST ship as a versioned binary with checksums, reference documentation, and an Agent guide, without client-side LibRaw, libvips, Bun, or a mounted Library. Incompatibility MUST fail before mutation.

Other platforms require their own build and end-to-end qualification evidence.

[Instance Access](../access/spec.md) applies to both clients without per-person scopes, public sharing, or remote Agent hosting. The 0.1 support contract is a separate release boundary; this target does not publish a release.
