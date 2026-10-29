# Rejected Cleanup Design

[Rejected Cleanup](spec.md) owns the browser review. [Removal Design](../design.md) owns removal state, receipts, Restore, and publication. The reviewed result can exceed ordinary Grid multi-selection, so it uses the Browse Snapshot rather than that selection's 100-Photo bound.

## Snapshot admission

Browser removal MUST be admitted only against a live Browse Snapshot created with the `Rejected` Selection State filter. A Snapshot with another filter, an expired Snapshot, or an unknown token MUST be refused before state changes.

The browser MUST supply the operation ID. The server MUST resolve the Snapshot's complete frozen ordered identity sequence, never only a window, and request removal in one transaction. Each Photo MUST be revalidated under the [generic removal rules](../spec.md#remove-from-library).

Each requested Photo MUST have exactly one browser outcome: `removed`, `changedElsewhere`, `missing`, or `alreadyRemoved`. The response MUST report outcome counts, operation ID, and identities of Photos not newly removed. The browser response reports newly removed Photos by count because Undo uses the operation ID.

## Review and recovery surface

The confirmation outcome MUST remain in the review.

A transport failure, malformed response, or incomplete response MUST retain the same operation ID and reviewed Snapshot for retry.

An expired, evicted, or unknown Snapshot MUST be reported as not found. The browser MUST discard the review, read the source again, and require a fresh review.

Undo MUST be offered beside a successful removal outcome and in Trash while that operation still owns a Photo. Trash listing and operation recovery follow the [Trash contract](../../trash/spec.md). A reload or restart MUST NOT lose the durable operation-level Undo.

After confirmed removal or Restore, the browser MUST reread the Overview count, Album summaries, Folder counts, and open source Snapshot before reporting the outcome. An emptied source MUST present the normal explained empty state.
