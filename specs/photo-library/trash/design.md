# Trash Inspection and Record Cleanup Design

[Trash and Record Cleanup](spec.md) owns observable behavior. [Photo Library Design](../design.md) owns Photo and Album state; [Composable Removal and Restore](../removal/design.md) owns reversible receipts; [Metadata Design](../metadata/design.md#sidecar-association-record) owns retained Sidecar associations.

## Design drivers

- The filesystem owner, not Slipstream, deletes Original Files. The existing read-only deployment must remain sufficient.
- A missing Location is an observation, not proof of who deleted or moved a file. An unavailable flag must not authorize record loss.
- Cleanup changes SQLite-owned records only. It needs no cross-filesystem deletion journal, unlink capability, privileged helper, or permission expansion.
- Photo identity and a removal marker protect the selected records. A path is output for the Photographer, not a mutation identity.
- Cleanup must preserve other Photos, Album order, retained development work, Sidecar ownership evidence, and historical operation outcomes.

## Model and ownership

### Trash inspection

Trash remains the projection of current Removed Photo records. A listing item contains the current Photo and Original identities, removal marker, remembered Location, kind, file availability, and recognizable Photo facts. The response identifies its Library, scope, total, observation time, and finite traversal limits.

A captured selection retains an ordered set of Photo identities, removal markers, Original identities, and remembered Locations, excluding deliberate exclusions before capture. Later arrivals and newer removal markers cannot enter that selection. Paged inspection retains this evidence or explicitly expires; current facts may mark a captured item changed or unavailable but cannot silently replace its evidence. The CLI uses the existing `--cursor` continuation convention for subsequent pages rather than evaluating another live offset. Existing [query continuation ownership](../../command-line/design.md) supplies bounded retention rather than a new durable export job.

Listing output is data. Structured output preserves exact Location characters; text output escapes control characters. The server identifies the configured Library Folder and its execution namespace so a client cannot mistake a server-relative Location for a client-local file. The CLI may print through its existing output modes. Neither interface constructs or executes deletion commands.

### Cleanup attempt

One caller-owned attempt identity names one bounded, ordered, nonempty explicit set. Each target carries the reviewed Photo identity, removal marker, Original identity, and remembered Location. These facts come from public Trash inspection, not guessed filenames or private storage. The request's explicit confirmation authorizes loss of those application records only.

The SQLite owner retains the immutable submitted intent and one outcome for every target in a durable receipt. The receipt is historical evidence, not a replacement Photo row or a recoverable file. Its identity fields must not require a live foreign key to a cleaned-up row. Existing removal and Restore receipts remain historical for the same reason.
Before confirmation, the review reads current absence and retained-work evidence through a read-only preview. It performs the same revalidation as cleanup, writes no receipt, and never changes a record or file. The final application still revalidates every target because the preview is not authorization.

There is no file-deletion progress state. A completed cleanup outcome is a database effect. Lost transport can leave the caller uncertain about that effect, but must not create an unresolved unlink or indefinitely block Restore.

## Semantics

### Inspection and external deletion

Inspection and printing do not write Library state. Reading file presence uses descriptor-confined read-only access under the admitted Library Folder. A missing thumbnail, stale `available` fact, or inability to open a file is not a successful absence observation.

The Photographer separately maps the reported namespace to their filesystem tools, checks the current intended files, and deletes them outside Slipstream. This external action has no application receipt and is not inferred from a printed list. It does not authorize cleanup of new Trash arrivals or a replacement record at the same Location.

### Admission and revalidation

Before admitting a new attempt, validate the complete input and advertised bound. Duplicate identities, malformed evidence, empty selection, unknown keys, and an oversized set are request-wide refusals with no effect. Never split, truncate, or reinterpret an omitted set as all Trash.

Read a known attempt's receipt before resolving current candidates or inspecting files. Identical intent replays the original outcome even after cleanup; changed intent conflicts. A refusal or failure in a settled receipt is not silently retried with fresh filesystem facts under the old identity.

For a new attempt, the Library mutation and publication boundary serializes admission with Restore, removal, Library Check application, Location Recovery, metadata association changes, and development-reference admission. For each target:

1. Revalidate the current Photo, exact removal marker, Original identity, and remembered Location. Active, re-removed, relocated, or otherwise changed records are refused.
2. Inspect authoritative workspace, editing, processing, and active-use dependencies. Retained work or unresolved use refuses cleanup with named blockers under [Workspace Lifetime and Recovery](../../photo-development/workspace/spec.md#lifetime-and-recovery).
3. Revalidate the admitted Library Folder and freshly observe the remembered Location without following symlinks. Only a definite absence permits cleanup. A present entry, including a replacement, refuses it. An unavailable root, changed binding, inaccessible component, unsafe traversal, or I/O error is an inspection failure, not absence.
4. Immediately before applying the record changes, use that observation under the same Library mutation admission. Refused targets remain intact; otherwise remove only the reviewed records and retain the per-target outcome.

Filesystem observation and SQLite commit cannot be atomic against arbitrary external writers. The contract records absence at the checked instant; it does not prove physical deletion or prevent a file appearing afterward. A later external arrival cannot be deleted by cleanup because cleanup has no filesystem mutation capability. A later Library Check applies normal Location Recovery and discovery, never recovery of the cleaned-up identity.

### Record effects

Admitted effects and the immutable attempt receipt commit in one SQLite transaction. Mixed eligibility produces a complete per-target partition in that transaction; a storage failure rolls back all its record effects. A known attempt with identical intent never reapplies any effect.

For each admitted target, remove the Photo's decisions, Album memberships, and other Photo-owned Library projections. Remove its Original record and fingerprint when no retained Library state references that Original. Compact the remaining Album positions without changing relative order, advance the affected Album membership version, and make a saved position for the removed Photo unavailable.

Retain only the history and conflict evidence required by their owners:
immutable operation outcomes and retained-orphan Sidecar association evidence.
A Photo-owned historical XMP export snapshot is retained as photo-independent
provenance evidence; its expired document bytes remain swept. It is not
retained workspace and must not block record cleanup forever. An orphan association retains its former owner as historical identity, not a live foreign-key dependency. It must continue to prevent an adjacent JPEG or later occupant from inheriting the Sidecar silently.

Cleanup must not erase retained workspace or processing state to satisfy its own preconditions. That work must first be handled through its separately confirmed safe removal lifecycle. Cleanup performs no filesystem mutation, including deletion of workspace files or derivative cache bytes. Ordinary independent cache retention remains its own capability.

A cleaned-up identity no longer resolves as a current Photo, cannot be restored by an old marker, and cannot be recovered from a fingerprint or immutable attempt receipt. For a later supported file at the old Location, apply normal Location Recovery for other retained identities before allocating a new Photo and Original identity. Neither case recovers the discarded decisions or Album memberships of the cleaned-up Photo.

### Publication and outcome recovery

Publish the committed removal of records and derived indexes before acknowledging success. Trash, Photo lookup, normal sources, Album counts, saved positions, query projections, and Undo eligibility must agree. An existing retained query keeps its evaluated membership and reports a cleaned-up identity as unavailable in its original position rather than substituting another Photo.

The caller stores the attempt identity before sending the confirmed request. A read-only outcome lookup returns the durable receipt without inspecting or deleting filesystem entries, changing Photo state, or retrying a refusal. A missing or uninspectable receipt is unknown outcome, not proof that no cleanup occurred. Successful outcome recovery also refreshes affected Web projections before presenting converged counts.

If Restore commits first, cleanup refuses the stale target. If cleanup commits first, Restore reports that identity unavailable. A new removal after Restore always has a different marker and requires new cleanup evidence and consent.

## Interface boundary and cutover

The shared service exposes bounded Trash inspection, the existing compare-and-set Restore, explicit record-only cleanup, and read-only cleanup outcome inspection. Web and CLI use the same Library owner. The [CLI Reference](../../command-line/reference.md#trash-inspection-and-record-cleanup) owns the listing and cleanup command language.

The physical-deletion review, confirmation, retry, and confined unlink capability are retired, not repurposed. Their HTTP mutation routes and CLI mutation commands must be removed together with their callers. There is no compatibility alias that can delete a file or reinterpret old deletion consent as record-cleanup consent. Contract negotiation and capability discovery must prevent clients from treating the retired commands as supported.

Previously retained deletion receipts are history only. Startup, operation inspection, migration, and retry must never resume their filesystem effects. Previously confirmed deletions must not become recoverable Photos. Existing records with failed or unresolved old deletion attempts remain inspectable, with unknown historical file outcomes labeled honestly; fresh presence inspection and explicit Restore or cleanup follow the new record-only contract, without an everlasting old deletion lock. Migration must not silently clean up those records or invent proof that Slipstream deleted their files.

Keep the Library bind read-only and expose no privileged delete helper. Before implementation cutover, the deployment host's operator tooling must verify the new listing, external fixture deletion, record cleanup, outcome recovery, and retired-route refusal end to end. No production Original is a qualification fixture.

## Options

### Selected: external file deletion and explicit missing-record cleanup

This keeps the filesystem owner in control, uses the existing read-only Original capability, and gives Slipstream one transactional effect domain. Explicit target evidence and a durable receipt protect application records without a second filesystem operation journal.

### Rejected: make Originals writable or add an application deletion helper

Both preserve a destructive capability the product no longer needs. They require additional permission, replacement-race, partial-unlink, and crash-reconciliation boundaries. A missing-record cleanup does not justify any of them.

### Rejected: automatically purge unavailable Photos during Library Check

Unavailable can mean temporary storage failure, a moved Original, or incomplete inspection. Automatic purging discards recoverable decisions and Album membership without consent and expands a Trash action to active Photos.

### Rejected: forget records while Originals are still present

This makes a normal Library Check rediscover an orphaned file as a new Photo and erases decisions even though the external deletion step did not occur. Fresh absence is the conservative eligibility rule for this workflow.

## Verification

Implementation must prove through disposable fixtures:

- complete bounded listing and lossless printed Location data, including cross-page exclusions, later arrivals, unusual filenames, and the server/container namespace;
- read-only Library mounts are sufficient for listing, Restore, and cleanup;
- externally absent Originals permit cleanup, while present replacements, unreadable storage, invalid roots, unsafe entries, and retained work refuse it without file changes;
- stale removal markers, changed identities or Locations, and competing Restore cannot clean up a different state;
- one transaction commits admitted record effects and receipts, preserves Album order and orphan Sidecar protection, and publishes consistent counts and Undo;
- same-attempt replay and lost-response recovery return historical outcomes without reapplying effects or retrying filesystem work;
- a recreated file cannot recover the cleaned-up identity or its decisions, while a qualifying different retained identity still follows normal Location Recovery;
- upgrade never executes an old unlink, automatically purges uncertain records, revives confirmed deletions, or turns old file-deletion consent into record-cleanup consent; and
- Web, CLI, service routes, and operator tooling perform no Original deletion and agree on record-only outcomes.
