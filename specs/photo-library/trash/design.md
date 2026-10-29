# Trash Inspection and Record Cleanup Design

[Trash and Record Cleanup](spec.md) owns observable behavior. [Photo Library Design](../design.md) owns Photo and Album state; [Removal Design](../removal/design.md) owns reversible receipts; [Metadata Design](../metadata/design.md#sidecar-association-record) owns retained Sidecar associations.

## Trash inspection

Trash is a bounded page projection of current Removed Photo records, not a source, Browse Snapshot, second browsing model, or Grid position. Each row MUST carry recognizable Grid and Photo View facts. The response MUST report the newest removal operation that still owns a Photo and its remaining count, enabling durable Undo after reload or restart.

The Web list MUST reuse existing bounded Trash facts and available derivative URLs. Its presentation MUST NOT require a new thumbnail pipeline or a per-row full Photo read.

A successful listing read MUST establish its current count and Undo operation together. Loading and failed reads MUST NOT present an empty Trash or offer an operation retained from another read. An absent operation MUST withdraw Undo.

A captured selection MUST retain an ordered set of Photo identities, removal markers, Original identities, and remembered Locations, excluding deliberate exclusions before capture. Later arrivals and newer removal markers MUST NOT enter that selection. Paged inspection MUST retain this evidence or explicitly expire; current facts MAY mark an item changed or unavailable but MUST NOT replace its evidence.

CLI continuation MUST use the existing `--cursor` convention rather than another live offset. [Query continuation ownership](../../command-line/design.md) provides bounded retention; no durable export job is required.

The server MUST identify the configured Library Folder and its execution namespace. Structured output MUST preserve Location characters; text output MUST escape control characters. Web and CLI MUST NOT construct or execute deletion commands.

## Cleanup attempt

A caller-owned attempt identity names one bounded, ordered, nonempty explicit set. Each target MUST carry reviewed Photo identity, removal marker, Original identity, and remembered Location from public Trash inspection. Explicit confirmation authorizes loss of those application records only.

The SQLite owner MUST retain immutable intent and one outcome per target in a durable receipt. Receipt identity fields MUST NOT require a live foreign key to a cleaned-up row. Removal and Restore receipts remain historical under the same constraint.

Before confirmation, a read-only preview MUST inspect current absence and retained-work evidence using the same revalidation as cleanup. It MUST NOT write a receipt or change a record or file. Final application MUST revalidate every target because the preview is not authorization.

A completed cleanup outcome is a database effect, not file-deletion progress. Lost transport MUST NOT create unresolved unlink work or indefinitely block Restore.

## Admission and revalidation

Before admitting a new attempt, Slipstream MUST validate the complete input and advertised bound. Duplicate identities, malformed evidence, empty selections, unknown keys, and oversized sets MUST be refused request-wide without effect. Slipstream MUST NOT split or truncate the set, or interpret an omitted set as all Trash.

The owner MUST read a known attempt's receipt before resolving candidates or inspecting files. Identical intent MUST replay the original outcome after cleanup; changed intent MUST conflict. A settled refusal or failure MUST NOT be silently retried with fresh filesystem facts under the old identity.

The Library mutation and publication boundary MUST serialize new cleanup admission with Restore, removal, Library Check application, Location Recovery, metadata association changes, and development-reference admission. For each target:

1. Revalidate the Photo, exact removal marker, Original identity, and remembered Location. Refuse active, re-removed, relocated, or otherwise changed records.
2. Inspect authoritative workspace, editing, processing, and active-use dependencies. Refuse retained work or unresolved use with named blockers under [Workspace Lifetime and Recovery](../../photo-development/workspace/spec.md#lifetime-and-recovery).
3. Revalidate the admitted Library Folder and inspect the Location through descriptor-confined read-only access without following symlinks. Only definite absence permits cleanup; a present entry, including a replacement, refuses it.
4. Report an unavailable root, changed binding, inaccessible component, unsafe traversal, or I/O error as an inspection failure, not absence.
5. Immediately before applying changes, use that observation under the same Library mutation admission. Retain refused targets and record each outcome.

A missing thumbnail, stale `available` fact, or inability to open a file is not a successful absence observation. Filesystem observation and SQLite commit cannot be atomic against arbitrary external writers. Absence is evidence at the checked instant, not proof of physical deletion or a guarantee against later arrival.

## Record effects

Admitted effects and the immutable attempt receipt MUST commit in one SQLite transaction. Mixed eligibility MUST produce a complete per-target partition in that transaction; storage failure MUST roll back all record effects. An identical known attempt MUST NOT reapply effects.

Cleanup MUST remove each admitted Photo's decisions, Album memberships, and other Photo-owned Library projections. It MUST remove the Original record and fingerprint when no retained Library state references that Original. It MUST compact remaining Album positions without changing relative order, advance the affected membership version, and make the cleaned-up Photo's saved position unavailable.

Cleanup MUST retain immutable operation outcomes and retained-orphan Sidecar association evidence. A Photo-owned historical XMP export snapshot MUST remain as photo-independent provenance evidence; its expired document bytes remain swept. That snapshot is not retained workspace and MUST NOT permanently block cleanup.

An orphan association MUST retain its former owner as historical identity, not a live foreign-key dependency. It MUST prevent an adjacent JPEG or later occupant from silently inheriting the Sidecar.

Cleanup MUST NOT erase retained workspace or processing state to satisfy its preconditions. That work requires its separately confirmed safe removal lifecycle. Cleanup MUST NOT mutate filesystem entries, including workspace files and derivative cache bytes; independent cache retention remains separate.

A cleaned-up identity MUST NOT resolve as a current Photo or be recovered from an old marker, fingerprint, or receipt. A later supported file at the old Location follows normal Location Recovery for other retained identities before new Photo and Original identities are allocated. Neither case restores discarded decisions or Album memberships.

## Publication and outcome recovery

Committed record removal and derived indexes MUST be published before success is acknowledged. Trash, Photo lookup, normal sources, Album counts, saved positions, query projections, and Undo eligibility MUST agree. A retained query MUST keep evaluated membership and report a cleaned-up identity as unavailable in its original position, without substituting another Photo.

The caller MUST store the attempt identity before confirmation. Read-only outcome lookup MUST return the durable receipt without inspecting or deleting filesystem entries, changing Photo state, or retrying a refusal. A missing or uninspectable receipt is an unknown outcome, not proof that no cleanup occurred.

Successful outcome recovery MUST refresh affected Web projections before presenting converged counts.

Restore and cleanup of the same removal MUST serialize:

| First commit | Competing action                              |
| ------------ | --------------------------------------------- |
| Restore      | Cleanup MUST refuse the stale target.         |
| Cleanup      | Restore MUST report the identity unavailable. |

A new removal after Restore requires new cleanup evidence and consent.

## Interface and compatibility

The shared service MUST expose bounded Trash inspection, compare-and-set Restore, explicit record-only cleanup, and read-only cleanup outcome inspection. Web and CLI MUST use the same Library owner. The [CLI Reference](../../command-line/reference.md#trash-inspection-and-record-cleanup) owns listing and cleanup command language.

Physical-deletion review, confirmation, retry, confined unlink capability, HTTP mutation routes, CLI mutation commands, and their callers MUST be retired, not repurposed. There MUST be no compatibility alias that deletes a file or reinterprets old deletion consent as record-cleanup consent. Contract negotiation and capability discovery MUST prevent clients treating retired commands as supported.

Previously retained deletion receipts are history only. Startup, inspection, migration, and retry MUST NOT resume their filesystem effects. Previously confirmed deletions MUST NOT become recoverable Photos.

Records with failed or unresolved old deletion attempts MUST remain inspectable, with unknown historical file outcomes labeled honestly. Fresh presence inspection and explicit Restore or cleanup MUST remain available without an everlasting old deletion lock. Migration MUST NOT silently clean up those records or invent proof of file deletion.

The Library bind MUST remain read-only, with no privileged delete helper.
