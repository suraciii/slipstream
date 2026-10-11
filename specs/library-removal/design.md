# Library Management: Explicit Removal and Restore

## Problem and drivers

The browser removal flow owns a Browse Snapshot, but an Agent can already identify an exact Photo set from a query or Photo read. The same Library must accept that set without treating a path, filename, Album, or live query as identity. A caller must also recover an accepted attempt after a lost response or service restart without repeating effects.

The design must preserve the existing browser route and its rejected-result review semantics while adding a machine-facing boundary that provides:

- explicit Photo IDs with the observed Selection State, decision version, and removal state;
- bounded, all-or-nothing admission validation before any Photo changes;
- durable attempt receipts and read-only outcome inspection;
- compare-and-set Restore using the removal marker;
- no authority or workflow coupling to permanent deletion.

The existing process-local Photo decision version is the caller's evidence token. It changes on every successful decision, removal, and Restore mutation; a restarted owner starts a new version epoch, so pre-restart evidence is stale rather than silently reusable.

## Domain model

A removal attempt is identified by its caller-supplied operation ID and one ordered, distinct target set. Each target carries:

- Photo identity;
- expected Selection State (`rejected` is the only eligible value);
- expected decision version;
- expected removal marker (`null` for a current Library Photo).

The owner checks the attempt receipt first. A known attempt replays its historical result only when the complete intent matches; reusing the ID with different targets or evidence is a conflict. A new attempt checks every target against current state in the owner transaction. A target that fails evidence or eligibility receives its own no-effect outcome; valid siblings may still commit.

Each newly removed target receives one monotonic removal marker. The persisted receipt stores the marker and outcome for every target, so an explicit response and later outcome read can name the exact removal that Restore must compare against.

A Restore attempt has its own caller-supplied operation ID and an ordered set of `{photoId, removedAtMs}` markers. Its receipt follows the same replay rule. Restore clears only a matching marker; an active, changed, missing, permanently deleted, or unsettled target is reported without claiming a restore. The owner serializes Restore with permanent deletion, so both cannot claim success for one removal.

## Boundary and ownership

The persistence owner remains the only writer of Photo removal, Restore, operation receipts, and removal markers. `Library` exposes explicit removal, explicit Restore, and read-only attempt lookup. The server maps browser calls to the existing `PhotoRemovalMutation` and `PhotoRestoration` owners, and maps CLI calls to explicit owner requests. The Web publication patch is held under the same publication lock as the owner call.

The CLI boundary adds:

- `photos remove OPERATION_ID --input FILE`;
- `photos restore OPERATION_ID --input FILE`;
- `photos removal-operation OPERATION_ID`;
- `photos restore-operation OPERATION_ID`.

Input files contain complete explicit target documents and are validated locally for exact keys, nonempty distinct IDs, evidence fields, and the advertised maximum before network access. CLI reads carry the normal Access Token and CLI contract header. Missing attempt receipts are reported as `outcome_unknown`; they are never treated as proof that a replacement mutation is safe.

## Options considered

### Option A: Reuse the browser removal route with an overloaded body

The existing `POST /api/photos/remove` would accept either `{token, operationId}` or the explicit target document, and the existing Restore route would gain an optional attempt ID.

This minimizes route count but makes two authorization and validation protocols share one shape-dispatching handler. It also makes it easy for a browser body to accidentally reach the explicit path and makes outcome reads harder to distinguish from current Web reads.

### Option B: Add explicit CLI routes over shared owner operations — selected

Keep the existing browser routes byte-compatible and add CLI-contract-protected routes for explicit removal, explicit Restore, and attempt lookup. Both route families call the same persistence owner and publication patching code. Capability limits advertise the explicit removal bound.

This adds three small routes but keeps authorization, request validation, browser compatibility, and historical-result reads distinct. The domain and persistence logic remains one owner implementation rather than a second mutation path.

## Failure and concurrency rules

- Duplicate, empty, malformed, or over-limit input is rejected before owner admission and before any Photo changes.
- A known operation ID with changed intent is a conflict; a known operation ID with the same intent replays the stored result.
- A response timeout, malformed result, missing result entry, or unknown receipt is uncertain. The caller must inspect the original operation before submitting replacement mutations.
- Stale decision or removal evidence is a per-Photo no-effect outcome. It cannot be repaired by checking only the current Selection State.
- Permanent deletion remains a separate reviewed capability. Pending or unresolved deletion takes precedence over Restore and removal classification.
- Successful removal and Restore never modify Original Files, Album membership, rating, or decision values.

## Observable contract

Every accepted explicit attempt returns one result item per requested Photo, with a stable outcome and, for a removed item, its `removedAtMs`. Summary counts must equal the per-item partition. Outcome inspection returns the exact historical response and does not change state. Web Trash reads and browser views observe the same committed state after the CLI operation.
