# Library Management: Trash and Record Cleanup

Photographers need to inspect discarded Photos, delete unwanted files with their own filesystem tools, and then remove the corresponding Slipstream records. Trash separates reversible Library removal, external file deletion, and irreversible record cleanup.

[Photo Library and Albums](../spec.md) owns Photo identity and Album membership. [Composable Removal and Restore](../removal/spec.md) owns reversible operation evidence and recovery.

## Capability boundary

Slipstream must not delete, move, rename, or rewrite an Original File. This rule has no Trash exception. Neither Web nor CLI may offer physical deletion, empty operating-system trash, run a shell deletion command, or obtain additional Original write permissions for cleanup.

The workflow has three distinct actions:

1. **Remove from Library** puts eligible Photos in Trash while preserving their records and Original Files.
2. The Photographer inspects or prints the Trash Locations and deletes the intended physical files outside Slipstream.
3. **Clean up records** explicitly removes selected Slipstream records after their Originals are confirmed absent at their remembered Locations.

Listing, printing, refreshing, a Library Check, a missing Original, or external deletion alone must not authorize record cleanup. Slipstream must not claim that it deleted files or reclaimed disk space.

## Trash

The Library Browser must provide a persistent, named **Trash** entry on desktop and mobile. It shows all Removed Photos whose records have not been restored or cleaned up, across Albums and sessions. Existing Removed Photos appear without another removal action. Trash must not inherit the active Album, Folder, or Selection State filter.

Trash must show the complete item count and support progressively browsing every item, newest removal first. Each item shows its Photo identity, filename, relative Original Location, kind, removal time, Original availability, and a thumbnail when available. An unavailable Original or thumbnail must not hide the record. An empty Trash must state that there are no Photos in Trash.

Loading, failed, and empty reads must remain distinct. A failed read offers Retry. The count and available removal Undo must come from the same successful read; an absent or empty removal operation must withdraw the previous Undo.

Trash persists across reload and restart. It has no automatic expiration, physical emptying, or record cleanup. Deleting a file outside Slipstream does not automatically put its Photo in Trash or remove its record.

On narrow screens, filenames, Locations, facts, and Restore remain readable without horizontal scrolling. Long Locations may wrap. Counts, selection actions, listing output, and available Undo remain reachable without scrolling through the entire list.

The ordinary list must lead with recognizable filename, removal time, and
Restore, retaining the relative Original Location and availability. Existing
thumbnails may support recognition; unavailable or failed delivery uses a
truthful placeholder and must not hide the row or imply Original absence.
Opaque identities, byte counts, and exact capture timestamps may use named
Details. Changed or expired capture, exclusions, uncertainty, and actionable
failure must stay visible at the affected selection or operation.

An empty successful Trash shows its empty state and a close/return path rather
than a bank of targetless disabled actions. With rows but no selection, batch
actions may stay hidden until selection; individual Restore, selection, complete
count, Location output, and available removal Undo remain reachable. No empty
state may hide pending work or an unresolved prior outcome.

Location inspection, copying, printing, or saving must expose the Library Folder,
capture qualification, and server/container namespace at the point of use under
the listing contract below. Disclosure changes presentation only: exported
evidence remains complete. The cleanup review must never hide its scope,
absence evidence, refusal reasons, or irreversible record-loss consequence.

## Querying and printing Locations

Web and CLI must let the caller inspect the complete Trash and obtain the Locations of an explicitly selected set or all captured Trash results. The caller must be able to print or save that listing without changing Library state. A first page or bounded subset must not be presented as the complete result.

The listing must identify its Library and evaluated scope, capture time, total, and any continuation or bound. Each item must include Photo and Original identities, the observed removal marker, and the remembered Library-relative Original Location. Location output must identify the configured Library Folder and the server or container filesystem namespace in which it is meaningful. A remote client must not treat that Location as a client-local path; any host mapping belongs to the operator.

RAW and same-basename JPEG remain separate entries. Sidecars, exports, directories, and other siblings must not join the listing merely by basename. Filenames containing spaces, quotes, Unicode, or line breaks must round-trip in structured output. Human-readable output must escape ambiguous control characters and must not masquerade as an executable deletion script.

The listing is an observation, not a file-deletion authorization or a guarantee that a Location still identifies the same content later. Before external deletion, the Photographer must check the intended physical files in the correct filesystem namespace. Slipstream must not generate or execute destructive shell commands as this capability.

## Selection and Restore

The Photographer must be able to select individual items, select all captured Trash results across pages, exclude items, and clear the selection. The selected count describes the complete selection, not only visible rows. Later arrivals must not join a captured selection automatically. A bound must be disclosed; an oversized operation is refused rather than silently truncated. Leaving Trash clears its unsubmitted selection.

**Restore** remains the primary recovery action. It supports single and multiple selected items through the existing removal-marker compare-and-set. It preserves Photo identity, Selection State, Rating, and membership positions in surviving Albums. It must not recreate deleted Albums or erase newer decisions.

A missing Original remains unavailable after Restore. Restoring a record does not recover file bytes. Restore reports successful, changed, unavailable, and failed items truthfully. Removal Undo applies only to retained records still owned by that removal; a cleaned-up record is no longer eligible. Historical removal outcomes must not recreate a cleaned-up record.

## Trash Record Cleanup

Only explicitly selected records currently in Trash are eligible. A rejected Selection State is not an additional requirement. Cleanup must not act on active unavailable Photos, a live path query, or every missing Photo in the Library.

The cleanup review must identify the exact records, remembered Locations, current absence evidence, and affected Albums. It must explain that cleanup discards the selected Photo records, Library decisions, and Album memberships, cannot be undone by Slipstream, and does not delete any physical file. The final action says **Clean up N Photo records**. Cancel changes nothing.

At application, Slipstream must revalidate the Photo identity, removal marker, Original identity, and remembered Location. A restored, re-removed, relocated, or otherwise changed target is refused and requires fresh inspection. Another record occupying an old Location must never inherit permission to be cleaned up.

Slipstream must freshly inspect each admitted Location through read-only access beneath the admitted Library Folder. An outdated unavailable flag or missing thumbnail is not absence evidence. The following must refuse cleanup and retain the record:

- a file or other entry still exists at the Location, including a replacement file;
- the Library Folder is unavailable, changed, or cannot be safely inspected;
- permission denial, I/O failure, unsafe traversal, or uncertain inspection prevents establishing absence; or
- retained workspace state, an Edit Recipe, a referenced Processing Artifact, or active or unresolved work still depends on the Photo. Such work uses the separate [Photo Workspace lifetime](../../photo-development/workspace/spec.md#lifetime-and-recovery) and explicit removal rules.

Absence at one remembered Location does not prove that the Photographer deleted all copies, rather than moved a file, or that any storage was reclaimed. The Photographer confirms that record cleanup is intended despite that limit. Cleanup does not search backups, delete a replacement, recover a moved Original, or implicitly discard retained development work.

## Effects and recovery

A confirmed cleanup removes the selected Photo records, their decisions and Album memberships, and Original records no longer referenced by retained Library state. It preserves remaining Album order and makes a saved position for a cleaned-up Photo unavailable. It must not delete an Original, Sidecar, workspace file, export, backup, directory, or cache file as a cleanup effect.

Known Sidecar ownership must remain protected by the [Metadata association contract](../metadata/spec.md#sources-and-association). Cleaning up a missing RAW must not silently transfer its retained Sidecar to a same-basename JPEG.

A batch may refuse some items and clean up others. Results account for every selected identity exactly once and distinguish cleaned-up records, changed targets, present Originals, blocked dependencies, unavailable records, and inspection failures. Counts describe records, never deleted files or reclaimed bytes. Refused records remain available for inspection and Restore.

The caller must retain the cleanup attempt identity before submission. A lost response has an unknown outcome until the same attempt is inspected. Accepted outcomes survive reload and restart. Repeating a known attempt with identical intent returns its historical result without applying it again; different intent under that identity is refused. A failed or refused target needs fresh inspection and a new confirmed attempt, not an automatic widening retry.

Restore, cleanup, Library Check, Location Recovery, and development admission must not publish conflicting effects on one record. A stale review cannot clean up a restored Photo or a newer removal. After a confirmed result, Trash, Library and Album counts, open results, saved positions, and Undo availability must agree with the committed records.

Cleaned-up identities must not reappear through receipt replay, restart, Restore, or Location Recovery. If a physical file later appears under a cleaned-up Location, the normal Library Check and Location Recovery rules apply to other retained identities first. If no retained identity qualifies, discovery creates a new independent Photo. Neither case recovers the cleaned-up identity, decisions, or Album memberships.

## Human and Agent use

Web and CLI must expose the same inspection, selection, absence checks, explicit record-cleanup consent, and outcome recovery. Neither requires browser automation, database access, or a privileged filesystem-deletion helper. Exact command and transport mechanics belong to their interface contracts; the capability must not reuse an Original-deletion action with a different meaning.

## Acceptance examples

- Photos removed across Albums and sessions remain in Trash after restart. Listing or printing all pages changes no record or file.
- Select more records than one visible page and exclude one. The output and cleanup name only the captured selection; later Trash arrivals do not join it.
- The Photographer externally deletes a listed RAW. Cleanup removes its eligible records while the JPEG sibling, retained Sidecar, exports, and directories remain byte-for-byte unchanged.
- A selected set contains an absent Original, a present Original, and an uninspectable Location. Only the absent, otherwise eligible record is cleaned up; the other records and all filesystem entries remain unchanged.
- Restore a reviewed Photo and remove it again. The old cleanup confirmation cannot remove either the active Photo or the newer removal.
- A file is moved or replaced after listing. Listing alone authorizes no mutation; cleanup refuses a present replacement, and absence elsewhere is not presented as proof of physical deletion.
- A missing Original has retained editing work. Cleanup names the blocker and leaves both the Photo record and work intact until the separate explicit workspace action permits cleanup.
- Lose the cleanup response and restart. Inspecting the original attempt recovers its committed record outcomes without a second effect.
- A cleaned-up Location later contains a supported file with no qualifying retained recovery candidate. A Library Check discovers a new Photo with default decisions and no old Album membership. If the file instead uniquely matches another retained unavailable Original, normal Location Recovery preserves that other identity, never the cleaned-up one.
- A thumbnail is unavailable or fails to transfer. Its Trash record remains
  recognizable and restorable; cleanup still requires fresh absence evidence.
- The last visible row is restored while an earlier outcome remains unknown.
  The resulting empty list must retain that unknown outcome's inspection path.
- A remote Photographer opens File locations from a compact row. The output
  identifies the Library Folder, capture, and execution namespace before it can
  be mistaken for a client-local path; closing details changes no selection.
