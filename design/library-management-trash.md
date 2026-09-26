# Library Management: Trash and Permanent Deletion

The product contract is [Library Management: Trash and Permanent Deletion](../docs/library-management-trash.md). This design adds the irreversible filesystem boundary without weakening the existing read-only Original capability.

## Design Drivers

- Remove from Library already persists a reversible marker on the Photo row. Trash must include that state across restart without creating a second recovery record.
- Permanent deletion has two different durability domains: SQLite records intent and outcome, while the Library Folder contains the Original File. A successful HTTP response cannot be the only evidence of either effect.
- A reviewed Location is not an identity. The final deletion must re-open the confined Location and compare the reviewed file facts before unlinking it.
- A batch may partially settle. One failed or uncertain item must not roll back a confirmed deletion or make a failed item disappear.
- Existing Albums retain ordered membership until a deletion is confirmed. Confirmed deletion removes that membership and compacts the remaining positions through the existing Album invariant.

## Model

A **Permanent Deletion Operation** is a durable, browser- or CLI-supplied operation id and a fixed ordered set of reviewed Trash items. Each item stores:

- Photo identity and the removal marker reviewed;
- Original identity, relative Location, kind, size, modification time, device, and inode observed for the review;
- the item state: `pending`, `deleting`, `deleted`, `missing`, `changed`, `failed`, or `uncertain`;
- the logical size credited only after confirmed deletion and an actionable failure message when applicable.

The operation record is persisted in the existing SQLite `library_metadata` owner boundary as one JSON receipt per operation. It is an application receipt, not a replacement Photo row. Permanent receipts are retained so reload and retry can reconcile the same operation; they also prevent Restore from clearing a permanently deleted item.

The Photo row and Original row remain durable evidence after deletion. A confirmed item is no longer eligible for Trash or Restore, its Album memberships are removed, and its published Original is marked unavailable. The Original bytes are never rewritten.

## Semantics

### Review

The Library owner resolves either an explicit Photo-ID list or the complete current Trash set minus explicit exclusions. It snapshots the set before confirmation. The owner reads current confined file facts for every candidate, then accepts only a currently removed Photo whose Original is present and whose stored size and modification time still match. Stale, missing, or permanently deleted candidates are returned as explicit review rejections and are never included in the operation. A Photo whose own deletion has not settled is refused the same way, with its own reason, because its outcome must be known before another confirmation may capture it.

The review response contains the fixed eligible set, each relative Location, kind, known logical size, and Album names. The operation id makes repeating review or confirmation idempotent; a repeat returns the original receipt rather than widening the set with later Trash arrivals.

### Confirmation and retry

Confirmation names only the reviewed operation id. For each unresolved item, the owner first durably changes `pending` or `failed` to `deleting`, then uses the confined deletion capability:

1. open the reviewed Location beneath the Library Folder without following symlinks;
2. require one regular file and compare device, inode, size, and modification time;
3. unlink only that confined relative name;
4. durably settle the item and, for `deleted`, remove its Album memberships and compact positions.

A mismatch is `changed`; an absent file before a deletion attempt is `missing`; an access or filesystem refusal is `failed`. `deleting` survives a crash as pending verification. It is never presented as success or failure, and Restore and another destructive confirmation remain unavailable until a later reconciliation settles it. Reopening that operation reconciles it: the item is attempted again, and an absent Original it owned settles as `missing` instead of staying unresolved forever. A retry repeats only unresolved items and never changes the reviewed set.

The operation result always partitions the fixed items into `deleted`, `changed`, `missing`, `failed`, and `pendingVerification`, and reports logical bytes for `deleted` only. A confirmed `deleted` item is idempotent on later retries and cannot become recoverable through a rescan or Restore.

### Interfaces

The server exposes the same model to Web and CLI:

- `GET /api/trash` — bounded Trash page, newest removal first, publishing the review bound one confirmation may capture and, per row, the retained operation still owing that Photo an outcome;
- `POST /api/trash/review` — explicit IDs or all-current-minus-exclusions, returning the fixed review and rejections;
- `POST /api/trash/delete` — explicit confirmation of one operation id;
- `GET /api/trash/operations/{operationId}` — durable result for reload and reconciliation;
- existing Restore routes remain the only reversible action and refuse permanently deleted items.

The existing `/api/photos/removed` listing and `/api/photos/restore` behavior remain compatible aliases while the Web surface changes its product label to Trash.

### Publication

Permanent deletion uses the same application publication lock as removal and Restore. The SQLite settlement commits before the published Photo and Original facts are patched. A reader therefore sees either the pre-deletion Trash item or the post-deletion unavailable evidence, never a confirmed result with a stale normal source.

## Options

### Selected: durable per-operation receipt plus confined per-item settlement

This keeps the existing Photo-row removal marker as the one recovery source, adds no second Photo model, and makes filesystem uncertainty explicit. It supports partial failure, restart, exact retry, and a review that cannot grow. The operation receipt is the smallest durable boundary already owned by the SQLite persistence thread.

### Rejected: delete every selected file first, then write one batch result

A process loss between unlink and the database commit would leave physical deletions with no durable evidence. Retry could report missing as if the user deleted the file elsewhere, and a partial failure could not distinguish confirmed deletion from unresolved work.

### Rejected: move Originals into an operating-system Trash directory

Moving changes the Original Location, creates a second file-identity protocol, and can silently capture sibling or sidecar files. The product contract requires Originals to stay in place until the explicit second confirmation and limits the deletion scope to the selected Original itself.

## Verification

Focused coverage must prove:

- review is limited to current Trash, captures a fixed set, reports Location, kind, size, and Albums, and excludes later arrivals;
- RAW and same-basename JPEG are independent and sidecars, exports, siblings, and directories remain unchanged;
- restored, moved, replaced, missing, inaccessible, and changed items are not deleted by an old review;
- mixed batches settle independently, report logical bytes only for confirmed deletions, and preserve recoverable state for failures;
- retry and reload return the durable operation result without deleting a confirmed item twice;
- confirmed deletion removes Album memberships, updates counts and publication, and never reappears in Trash after restart or rescan;
- Web and CLI produce the same review, confirmation, and outcome partitions.
