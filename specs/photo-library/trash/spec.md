# Trash and Record Cleanup

[Photo Library and Albums](../spec.md) owns Photo identity and Album membership. [Removal and Restore](../removal/spec.md) owns reversible effects, attempt evidence, and recovery. Trash owns inspection and explicit record cleanup after external file deletion.

## Capability boundary

Slipstream MUST NOT delete, move, rename, or rewrite an Original File.

Web and CLI MUST NOT offer physical deletion, empty operating-system trash, run a shell deletion command, or obtain additional Original write permissions for cleanup.

Listing, printing, refreshing, a Library Check, a missing Original, or external deletion alone MUST NOT authorize record cleanup. Slipstream MUST NOT claim that it deleted files or reclaimed disk space.

## Trash

The Library Browser MUST provide a persistent named **Trash** entry on desktop and mobile. It MUST show all Removed Photos whose records have not been restored or cleaned up, across Albums and sessions. Existing Removed Photos MUST appear without another removal; Trash MUST NOT inherit an active Album, Folder, or Selection State filter.

Trash MUST show the complete item count and support progressive browsing of every item, newest removal first.

Each item MUST show:

- Photo identity;
- filename and relative Original Location;
- kind and removal time;
- Original availability; and
- a thumbnail when available.

An unavailable Original or thumbnail MUST NOT hide the record.

An empty Trash MUST state that there are no Photos in Trash. Loading, failed, and empty reads MUST remain distinct; a failed read MUST offer Retry. The count and available removal Undo MUST come from the same successful read; an absent or empty operation MUST withdraw the previous Undo.

Trash MUST persist across reload and restart without automatic expiration, physical emptying, or record cleanup. External deletion MUST NOT automatically put a Photo in Trash or remove its record.

On narrow screens, filenames, Locations, facts, and Restore MUST remain readable without horizontal scrolling. Long Locations MAY wrap. Counts, selection actions, listing output, and available Undo MUST remain reachable without scrolling through the entire list.

The ordinary list MUST lead with recognizable filename, removal time, and Restore, while retaining the relative Original Location and availability. Existing thumbnails MAY support recognition; unavailable or failed delivery MUST use a truthful placeholder and MUST NOT hide the row or imply Original absence.

Opaque identities, byte counts, and exact capture timestamps MAY use named Details. Opening or closing Details MUST NOT change selection.

Changed or expired capture, exclusions, uncertainty, and actionable failure MUST stay visible at the affected selection or operation. The cleanup review MUST NOT hide its scope, absence evidence, refusal reasons, or irreversible record-loss consequence.

An empty successful Trash MUST show its empty state and a close or return path instead of targetless disabled actions. With rows but no selection, batch actions MAY remain hidden, but individual Restore, selection, complete count, Location output, and available removal Undo MUST remain reachable.

An empty state MUST NOT hide pending work or an unresolved prior outcome. An unknown outcome MUST retain its inspection path even when no Trash rows remain.

## Querying and printing Locations

Web and CLI MUST support complete Trash inspection and Location output for an explicitly selected set or all captured Trash results. The caller MUST be able to print or save the listing without changing Library state. A first page or bounded subset MUST NOT be presented as complete.

The Web copy action MUST copy the explicit selection when one exists, or all captured Trash results otherwise. It MUST honor exclusions, leave the selection unchanged, and identify which scope was copied.

The listing MUST identify its Library, evaluated scope, capture time, total, and any continuation or bound. It MUST identify the capture expiry when applicable.

Each item MUST include:

- Photo and Original identities;
- the observed removal marker; and
- the remembered Library-relative Original Location.

Location output MUST identify the configured Library Folder and the server or container filesystem namespace in which it is meaningful.

Location inspection, copying, printing, and saving MUST expose the Library Folder, capture qualification, and server or container namespace at the point of use. Disclosure changes presentation only; exported evidence MUST remain complete.

A remote client MUST NOT treat that Location as a client-local path. Any host mapping belongs to the operator.

RAW and same-basename JPEG MUST remain separate entries. Sidecars, exports, directories, and other siblings MUST NOT join by basename. Filenames containing spaces, quotes, Unicode, or line breaks MUST round-trip in structured output; human-readable output MUST escape ambiguous control characters and MUST NOT masquerade as an executable deletion script.

The listing is an observation, not deletion authorization or a guarantee of later content identity. Before external deletion, the Photographer MUST check the intended physical files in the correct filesystem namespace. Slipstream MUST NOT generate or execute destructive shell commands as this capability.

## Selection and Restore

The Photographer MUST be able to:

- select individual items;
- select all captured Trash results across pages;
- exclude items; and
- clear the selection.

The selected count MUST describe the complete selection, not only visible rows. Later arrivals MUST NOT join a captured selection automatically.

A bound MUST be disclosed; an oversized operation MUST be refused rather than silently truncated. Leaving Trash MUST clear its unsubmitted selection.

**Restore** MUST remain the primary recovery action and support single and multiple selected items under the [Removal and Restore contract](../removal/spec.md#restore). Removal Undo applies only to retained records still owned by that removal; cleaned-up records are ineligible.

## Trash Record Cleanup

Only explicitly selected records currently in Trash are eligible. Rejected Selection State is not an additional requirement. Cleanup MUST NOT act on active unavailable Photos, a live path query, or every missing Photo in the Library.

The cleanup review MUST identify the exact records, remembered Locations, current absence evidence, and affected Albums.

It MUST explain that cleanup discards selected Photo records, Library decisions, and Album memberships. It MUST explain that cleanup cannot be undone by Slipstream and does not delete files.

The final action says **Clean up N Photo records**. Cancel MUST change nothing.

At application, Slipstream MUST revalidate Photo identity, removal marker, Original identity, and remembered Location. A restored, re-removed, relocated, or otherwise changed target MUST be refused and require fresh inspection. Another record occupying an old Location MUST NOT inherit cleanup permission.

Slipstream MUST freshly inspect each admitted Location through read-only access beneath the admitted Library Folder. An outdated unavailable flag or missing thumbnail is not absence evidence. Cleanup MUST refuse and retain the record when:

- a file or other entry still exists at the Location, including a replacement;
- the Library Folder is unavailable, changed, or cannot be safely inspected;
- permission denial, I/O failure, unsafe traversal, or uncertain inspection prevents establishing absence; or
- retained workspace state, an Edit Recipe, a referenced Processing Artifact, or active or unresolved work depends on the Photo. Such work follows [Photo Workspace lifetime](../../photo-development/workspace/spec.md#lifetime-and-recovery) and its explicit removal rules.

Absence at one Location does not prove that the Photographer deleted all copies or reclaimed storage. The Photographer MUST confirm record cleanup despite that limit. Cleanup MUST NOT search backups, delete a replacement, recover a moved Original, or implicitly discard retained development work.

## Effects and recovery

Confirmed cleanup removes selected Photo records, their decisions and Album memberships, and Original records no longer referenced by retained Library state. It MUST preserve remaining Album order and make a saved position for a cleaned-up Photo unavailable. It MUST NOT delete Original, Sidecar, workspace, export, backup, directory, or cache files.

Known Sidecar ownership MUST remain protected by the [Metadata association contract](../metadata/spec.md#sources-and-association). Cleaning up a missing RAW MUST NOT silently transfer its retained Sidecar to a same-basename JPEG.

A batch MAY refuse some items and clean up others. Results MUST account for each selected identity exactly once and distinguish:

- cleaned-up records;
- changed targets;
- present Originals;
- blocked dependencies;
- unavailable records; and
- inspection failures.

Counts MUST describe records, never deleted files or reclaimed bytes. Refused records remain available for inspection and Restore.

The caller MUST retain the cleanup attempt identity before submission. A lost response has an unknown outcome until the same attempt is inspected. Accepted outcomes MUST survive reload and restart; identical retries MUST return historical results without reapplying effects, while different intent under that identity MUST be refused.

A failed or refused target requires fresh inspection and a new confirmed attempt, not an automatic widening retry. Restore, cleanup, Library Check, Location Recovery, and development admission MUST NOT publish conflicting effects on one record. A stale review MUST NOT clean up a restored Photo or newer removal.

After confirmation, Trash, Library and Album counts, open results, saved positions, and Undo availability MUST agree with committed records. Cleaned-up identities MUST NOT reappear through receipt replay, restart, Restore, or Location Recovery.

If a physical file later appears under a cleaned-up Location, normal Library Check and Location Recovery MUST apply to other retained identities first. If none qualifies, discovery creates a new independent Photo with default decisions and no old Album membership. Neither case recovers the cleaned-up identity, decisions, or Album memberships.

## Human and Agent use

Web and CLI MUST expose the same inspection, selection, absence checks, explicit record-cleanup consent, and outcome recovery. Neither requires browser automation, database access, or a privileged filesystem-deletion helper. Interface contracts own command and transport mechanics; cleanup MUST NOT reuse an Original-deletion action with a different meaning.
