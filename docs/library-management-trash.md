# Library Management: Trash and Permanent Deletion

Photographers need to inspect and recover discarded Photos before deleting their Original Files. A visible Trash destination separates reversible removal from permanent deletion and makes the consequences of each action clear.

## Capabilities

Library Management includes Mark Photos, Remove from Library, Restore Photos, and Delete Original Files. Marking records Selection State and must not delete anything. [Remove Rejected Photos](rejected-photo-cleanup.md) defines entry into Trash. This document owns Trash, Restore from Trash, and permanent deletion.

Delete in normal Library views must mean removal into Trash. Only **Permanently Delete** inside Trash may delete an Original File. Original access remains read-only for all other capabilities. This explicit, confirmed deletion is the sole exception; it grants no permission to rewrite Original contents or move files.

## Trash

The Library Browser must provide a persistent, named **Trash** entry accessible on desktop and mobile. It must show all Photos removed through Slipstream that have not been restored or permanently deleted, across Albums and sessions. Existing Removed Photos must appear here without another removal action. Trash replaces that recovery surface rather than creating a second collection.

Trash must show the complete item count and allow progressively browsing every item, newest removal first. It must not inherit the active Album, Folder, or Selection State filter. Each item must show its filename, relative Original Location, file kind, removal time, and thumbnail when available. An unavailable Original or thumbnail must have a truthful fallback and must not hide the item. An empty Trash must state that there are no Photos in Trash.

Items must remain in Trash across reload and restart. There is no automatic expiration or emptying. Trash is an application view; entering it does not move an Original into an operating-system trash directory or release its storage. Files deleted outside Slipstream are not automatically Trash items.

## Selection and Restore

The Photographer must be able to select individual items, select all current Trash results across pages, exclude individual items, and clear the selection. The selected count must describe the complete selection, not only visible cells. Later arrivals must not join an existing selection automatically. Leaving Trash must clear its unsubmitted selection.

**Restore** must support one or multiple selected items. It returns Photos with the same identities, Selection States, Ratings, and membership positions in Albums that still exist. It must not recreate deleted Albums or erase newer decisions. A missing Original remains unavailable after Restore; restoring a Library record must not claim to recover missing file bytes.

Restore must report successful, changed, and failed items separately. Restored items leave Trash and return to applicable normal views. The existing immediate removal Undo remains available under the removal contract until its items are restored or permanently deleted. Neither Restore nor Undo can recover a permanently deleted Original.

## Permanent deletion review

Only items currently in Trash are eligible. A rejected Selection State is not an additional requirement. Permanent deletion must not be available from a normal Grid, Photo View, an Album removal action, or a marking shortcut.

The review must identify the selected files, count, relative Locations, and affected Albums. It must show the total logical file size when known and identify unknown sizes. It must explain that deletion affects every Album containing that Photo and that Slipstream cannot Undo it. The final action must say **Permanently delete N Original Files**. Cancel must delete nothing. No automatic retry may widen the reviewed set.

A Photo restored, removed again, or changed since review must be skipped and require fresh review. A moved or replaced Original must not be deleted using the old review; Slipstream must not delete another file merely because it occupies the reviewed Location. If it cannot establish that the target is still the reviewed Original, it must refuse that item.

## File scope

Permanent deletion must affect only each selected Photo's own Original. RAW and same-basename JPEG are independent. Unselected sibling files, XMP Sidecars, exports, backups, and directories must remain unchanged. Retained Sidecars must not silently transfer metadata to a sibling Photo after deletion changes their association.

An Original already absent must be reported as missing, not as successfully deleted or reclaimed storage. Its Trash item must remain inspectable; a Library-record Restore may still return it as unavailable. This capability does not add a separate command to purge missing records.

Read-only or inaccessible storage must produce an actionable refusal. Slipstream must not silently weaken permissions or switch to a different target. Original bytes must not be modified as a preliminary deletion step.

## Results and recovery

A batch may complete partially. The result must distinguish confirmed deletion, changed targets, already-missing Originals, failed items, and outcomes still being verified. Confirmed deletions leave Trash and normal sources and lose Album membership. Remaining Album order must be preserved; a saved position pointing to a deleted Photo must become unavailable.

Failed items must retain their recoverable Library state. An uncertain item must remain visible as pending verification, with conflicting Restore and deletion actions unavailable until its outcome is known. A lost response must not be presented as proof of failure or success. Reopening the operation after reload or restart must recover its result; retry must reconcile prior effects before attempting unresolved items.

After settlement, Library counts, Album counts, Trash counts, and Undo availability must agree with confirmed effects. Completed deletion must not later reappear as a recoverable Photo merely because a rescan or restart occurs. Retained results are evidence, not recoverable files.

Report the logical bytes of confirmed deleted Originals separately from item counts. Do not describe that value as measured free space; hard links and filesystem snapshots may retain storage. Do not promise rollback of files already deleted when another item fails.

## Human and Agent use

Web and CLI must expose the same Trash eligibility, review scope, explicit confirmation, and outcome rules. A command that marks or removes Photos must not implicitly authorize permanent deletion. Programmatic deletion must express the separate confirmed intent; exact syntax belongs to Build's design.

## Acceptance examples

- Photos removed from several Albums before and after upgrade all appear in Trash after restart. Restoring one retains its identity and surviving Album memberships.
- Select more items than a visible page and exclude one. Only the reviewed files may be deleted; the excluded item remains recoverable.
- Delete a RAW while its same-basename JPEG remains selected in the Library. The JPEG, Sidecar, and exports remain unchanged.
- Restore an item or replace its Original after review. Permanent deletion refuses that stale target.
- A batch contains a writable file, an unwritable file, and an absent file. Only the first counts as deleted; the other outcomes remain explicit.
- Lose the connection during deletion and reopen the result. Confirmed deletions do not run twice, and unresolved items do not falsely offer file recovery.
