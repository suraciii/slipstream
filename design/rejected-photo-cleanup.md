# Removing Rejected Photos from the Library

[The product spec](../docs/rejected-photo-cleanup.md) defines the observable behavior of this capability. This spec records the design that provides it.

A Photographer reviews a filtered `Rejected` result and decides to take those Photos out of the Library while keeping every Original File untouched and every removal recoverable. The reviewed result can be larger than the ordinary 100-Photo Grid multi-selection, and Photo facts can change between the review and the confirmation. The design must therefore remove exactly what was reviewed, make concurrent changes inspectable instead of silently deleting, and survive a reload or restart.

## Design Drivers

- A rejected result can exceed the existing 100-Photo ordinary multi-selection limit, so the removal path cannot reuse that bound.
- A Browse Snapshot is a frozen ordered sequence, but Photo facts change while it is open.
- Undo must be exact and cheap even for a large removal, and a retry after a transport failure must not invent a second outcome set.
- Recovery must survive reload, snapshot expiry, and restart; an in-memory undo record is not sufficient.
- Removal changes Library visibility only. Original Files, Photo identity, Original Location, Rating, Album membership, and Selection State remain untouched.
- XMP currently represents Rating only. Removal must not encode Selection State as a rating or keyword.

## Model

### Removed Photo

Removal is application-owned Photo state: a Photo is either part of the Library or removed from it. It is stored on the Photo row as two nullable columns:

- `removed_at_ms`: when the removal was confirmed; and
- `removed_operation`: the removal operation that removed it.

Both are null together or set together. A removed Photo keeps its row, so every retained fact — identity, Original Location, Rating, Album membership, Album position, saved-position eligibility, and Selection State — is still the one the Photographer had before the removal. There is no separate recovery record to keep in sync, and restoring a Photo never rewrites facts it never lost.

### Removal Operation

A Removal Operation is one confirmed removal of one reviewed result. Its identity is an opaque id supplied by the browser, so a retry after a lost response repeats the same operation instead of creating a second one. The operation owns the group of Photos that Undo restores.

### Removed Source Materialization

Normal Library sources exclude removed Photos. That covers every Browse Snapshot source (`All Photos`, Original Folder, Album), the Library Overview Photo count, Album summaries and Album member listings, Original Folder Photo counts, the CLI Photo query, and the unavailable-Original recovery survey. Album membership rows are not deleted, so restoring a Photo re-admits it to the same Albums at the same positions.

A removed Photo is not deleted state: reading it by identity still resolves, and the Removed Photos listing is its only ordinary surface. The listing is bounded and ordered newest removal first.

### Removed Photos Listing

The Removed Photos listing is a bounded page over removed Photos in removal order. Each item carries the Photo facts Grid and Photo View already use, so the Photographer can recognize what is recoverable before restoring it. It is not a source and does not create a second browsing model.

## Semantics

### Removal Admission

Removal is admitted only against a live Browse Snapshot created with the `Rejected` Selection State filter. That Snapshot's frozen ID sequence is the reviewed result; a Snapshot with any other filter, an expired Snapshot, or an unknown token is refused before any state changes.

The browser supplies the operation id. The server resolves the Snapshot's complete frozen sequence — never a window — and asks the Library owner to remove those Photos in one transaction.

### Removal Outcome

Each requested Photo yields exactly one outcome:

- `removed`: the Photo was `rejected` and is now removed by this operation, including a Photo this same operation already removed on a retried request;
- `changedElsewhere`: the Photo is no longer `rejected`, so it stays in the Library;
- `missing`: the Photo row no longer exists; or
- `alreadyRemoved`: another operation had already removed the Photo, so this operation does not adopt it.

The response reports the outcome counts, the operation id, and the identities of every Photo that was not newly removed. Photos that were removed are reported by count, because the operation id — not a Photo list — is what Undo needs. Nothing else changes: a refused or partial outcome leaves Selection State, Rating, Album membership, identity, and Original Files as they were.

The operation is one transaction: a persistence failure leaves the whole request unapplied and retryable.

### Undo and Restore

Undo restores every Photo of one operation that is still removed, in one transaction. It is a compare-and-set against the removal marker: a Photo that another action already restored is simply no longer part of the operation and is not overwritten.

The durable Restore action restores named Photos through the same compare-and-set, and reports one outcome per requested Photo: `restored`, `changedElsewhere`, or `missing`. Each named Photo carries the removal marker the listing presented, and the compare-and-set is against that marker: a Photo removed again since the listing was read is reported as `changedElsewhere` instead of being restored past its newer removal, and a marker that does not match the current removal is refused before any state changes. A request naming neither one operation nor a bounded non-empty list of markers is refused.

The response also reports what each operation it touched still owns. An operation with nothing left is reported with a zero count, and an operation the restore did not touch is absent, so the browser withdraws Undo exactly when the operation emptied and keeps the remaining count when a restore took part of it.

Restoring clears the removal marker. It never rescans, renames, moves, or rewrites an Original File, and it never changes Selection State, Rating, or Album membership.

### In-Memory Publication

The running server serves browsing from one in-memory published Library. A confirmed removal or restore patches that publication in place — the removal marker and the derived Original Folder index — so a source opened immediately afterwards already excludes or re-admits the Photo. A rescan publishes a replacement that carries the persisted markers unchanged.

The derived projection a machine client queries moves with the same commit. A retained query keeps its ordered identities and reports a Photo removed after the query was created as `{"id":"…","state":"missing"}` in its original position, and re-admits it after a restore, without waiting for a rescan. Only a committed removal or restore may move that projection: a refused or rolled-back request leaves it exactly as it was.

### Failure Behavior

A transport failure, malformed response, or incomplete response must leave the removal retryable without inventing per-Photo outcomes; the browser keeps the same operation id and the same reviewed Snapshot until a result is confirmed.

An expired, evicted, or unknown Snapshot is refused as not found. The browser discards the pending review and requires the Photographer to review the current rejected result again.

A Photo whose Original File is unavailable is still removed and restored by the same rules: removal is Library state, not filesystem state.

## Web Surfaces

Removal is a Library Management action, so it is offered where the Photographer
manages the Library and never as a Grid decision:

- The **removal review** is the only surface that removes Photos. It opens from
  the Grid header of a `Rejected` result and names the count that would leave
  the Library, the Original Files that stay, and the one confirmation it
  requires. Opening it removes nothing, and it is offered only while a removal
  could be admitted against the open Snapshot.
- The **confirmation outcome** reports the counts the server committed and
  keeps the review open. A removal that removed nothing offers no Undo; a
  removal that removed Photos offers Undo beside the outcome. A transport
  failure, a malformed response, or an incomplete response keeps the review and
  its operation id, so Retry repeats one operation. A review the server refuses
  as gone is discarded: the source reads again and the Photographer reviews the
  current rejected result.
- **Undo** restores every Photo of the confirmed operation that is still
  removed. It is offered beside the confirmation and in the Removed Photos
  listing, because the listing is where a Photographer returns to recover a
  removal.
- The **Removed Photos listing** is a bounded page over removed Photos, newest
  removal first, with each row presenting the facts the Grid presents and its
  own Restore. A confirmed removal or restore reads the Library again — the
  Overview count, the Album summaries, the Folder Photo counts, and the open
  source Snapshot — before the surface reports the outcome, so no row, count,
  or Grid cell outlives the state it presents. An emptied source presents the
  same explained empty state any other open presents.

The listing is not a source: it creates no Browse Snapshot, no second browsing
model, and no Grid position. It reads one bounded page at a time and restores
named Photos through the same compare-and-set the operation-level Undo uses,
naming the removal marker each row presented. Undo stays offered beside the
listing exactly while the confirmed operation still owns a Photo.

## Options

### Selected: Snapshot-bound removal with a Photo-row removal marker

The browser removes the frozen rejected result of the Snapshot it reviewed and names the operation. The Library owner revalidates each Photo inside one transaction and writes one removal marker per admitted Photo.

This supports results larger than one Grid window, preserves the existing Snapshot contract, keeps the dangerous action behind a dedicated review, and makes concurrent changes visible as outcomes instead of silent effects. Because the marker lives on the Photo row, restore has nothing to reconstruct and no second table can drift out of sync.

### Rejected: Reuse ordinary Grid multi-selection

The existing multi-selection is capped at 100 Photos and only includes loaded positions. Reusing it would either make large removals impossible or weaken a bound that protects every existing batch route. It would also expose a removal action in a surface designed for decisions and Album membership.

### Rejected: A separate Recovery Area record per removed Photo

A dedicated table could copy the Photo's pre-removal facts. It would duplicate state that already exists, require the copy to stay correct through concurrent decisions and Album edits, and give restore two sources of truth to reconcile. The Photo row already retains everything recovery needs.

### Rejected: Delete Original Files immediately

Immediate filesystem deletion is irreversible, violates the Original File safety boundary, and gives a concurrent state change no recovery path. Removal keeps the file in place and leaves physical deletion to its own future capability.

## Verification

The implementation must prove that:

- a rejected Snapshot can remove a result larger than one Grid window in one confirmation;
- removal is refused for a Snapshot whose filter is not `Rejected`, and for an expired or unknown token;
- every requested Photo yields exactly one `removed`, `changedElsewhere`, `missing`, or `alreadyRemoved` outcome, and a retried request with the same operation id adopts what it already removed;
- a restore names the removal marker each Photo was listed under, so a stale marker is refused and a Photo removed again since is reported as `changedElsewhere` rather than cleared;
- a restore reports what each operation it touched still owns, including a zero remainder for one it emptied, so Undo is withdrawn when the operation is empty and keeps its remaining count otherwise;
- a changed, missing, or already-removed Photo is never removed by that request;
- removed Photos leave every normal source, Album listing, Album count, Overview count, Folder count, and CLI query, while keeping their Album membership rows;
- a removed Photo's bytes and Original Location are unchanged, and its Selection State, Rating, identity, and Album membership survive removal and restore;
- a restart preserves removal markers, the Removed Photos listing, and operation-based Undo; and
- malformed, incomplete, stale, and transport-failed responses do not update visible state or claim success.

The Web surfaces carry their own proof: the page-model unit tests characterize the removal owner's review, admission, retry, Undo, and restoration policy, and the browser suite proves against the real stack that a reviewed `Rejected` result leaves the Library in one confirmation, that the Overview count and the open source read again, that Undo restores the operation from the listing and is withdrawn once the operation is empty, and that a named Photo is restored and stays restored across a reload.
