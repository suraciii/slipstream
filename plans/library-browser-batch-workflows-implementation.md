# Library Browser Batch Workflows Implementation Plan

Status: Ready for implementation after product approval.

Tracking Epic: #281

Child Issues: #282, #283, #284, #285, #286

This plan turns the approved Library Browser batch-workflow design into bounded implementation slices. It does not change the Product or Design Specs by itself. The governing Issue and PRs must update the authoritative specs before code changes land.

## Scope

Implement the following product behavior:

- A visible multi-selection tray with an explicit `0 / 100` bound.
- Selection State decisions with the existing one-level global Undo.
- Album membership additions with a scoped `Remove added Photos` compensation action.
- Explicit per-Photo handling for missing Photos and Photos changed elsewhere.
- Separate visible-result counts from complete-source Selection State progress.
- Explicit feedback that a Grid batch does not move an Album's Photo View resume position.
- Keyboard-safe pending, success, partial-result, failure, retry, and clear states.

## Product Decisions

These decisions are the implementation contract for this plan:

1. Selection remains after a batch decision or one batch Album addition. Clear and Escape empty it and leave Select mode, including from an empty `0 / 100 Photos` tray; source open and source reopen also empty it and leave Select mode. A missing Photo remains counted and visible in the result until clear or source change, but it is excluded from later requests.
2. Select and Reject share the existing one-level Selection State Undo. Add to Album remains outside global Undo.
3. Add to Album exposes a scoped compensating action named `Remove added Photos`. It removes only Photos added by that operation and does not claim to restore historical Album positions.
4. The recommended concurrency policy is optimistic comparison for the initial batch write. A Photo whose Selection State changed after the browser last confirmed it is not overwritten.
5. A batch Selection State decision or batch Album addition in an Album does not move the durable Photo View resume position. The result surface says that the resume point is unchanged. `Remove added Photos` is an ordinary Album removal and reports the resulting saved position when it removes the saved Photo.
6. Progress labels distinguish the active filtered sequence from counts over the complete source.
7. No new global history abstraction, modal conflict workflow, or batch Undo endpoint is added.

The current repository implements the older batch contract: existing Photos are last-writer-wins, missing Photos are the only batch conflicts, and Album membership writes return counts through the general Album summary response. Those contracts must change only in the slices below. The old executable route fixtures remain a transitional baseline during the contract PR because the route implementation changes in Slices 2 and 3; Slice 3 must retire the old `photoIds`/`conflicts` vectors and replace the old response golden before this Epic can be complete.

## Ownership and Delivery Rules

- Product owner: the user approves the observable behavior and final wording.
- Technical owner: one implementation owner per slice, working in an isolated worktree.
- Reviewer: an independent read-only reviewer for each PR.
- Gate owner: the PR owner runs the focused checks and `bun run verify`; the parent verifies the authoritative CI result before merge.
- Workers do not merge PRs, wait for CI, or edit Issues.
- A PR owns one slice and one writer. Do not run concurrent writers against `page.ts`, `library-browser-view.ts`, or the shared browser test file.
- Preserve the pre-existing uncommitted `CONTEXT.md` change in the main checkout. Do not include it in any PR.

Proposed branch names:

- `feat/library-browser-selection-tray`
- `feat/library-browser-album-compensation`
- `feat/library-browser-batch-concurrency`
- `fix/library-browser-browser-coverage`

## Dependency Order

```text literal
Spec and contract update
  -> Selection tray and result surfaces
       -> Album compensation
            -> Optimistic batch concurrency
                 -> Browser hardening and real-library qualification
```

The first three implementation PRs touch shared Web surfaces and must merge serially. The final hardening PR may start only after the behavior PRs stabilize.

## Slice 0: Contract and Specification Update

### Outcome

The repository has one authoritative definition for the new behavior before implementation begins.

### Files allowed

- `docs/library-browsing-and-selection.md`
- `design/library-browsing.md`
- `design/photo-organization.md`
- `design/web-async-ownership.md` only if the scoped Album compensation or Grid batch Selection State settlement needs a durable ownership rule
- `compatibility/protocol/batch-workflows.json`
- `crates/slipstream-compat/src/lib.rs`
- `compatibility/protocol/browse-vectors.json`
- `compatibility/protocol/responses.json`
- `plans/library-browser-batch-workflows-implementation.md`

### Required changes

- Replace the current last-writer-wins batch rule with the approved `applied`, `changedElsewhere`, and `missing` outcome semantics.
- Define the request identity for every selected Photo: `photoId` plus the last confirmed Selection State.
- Define the exact message and action semantics for selection retention, cap refusal, Album compensation, and unchanged Album resume position.
- Define the difference between `Visible results` and `Source progress`.
- Define the response shape for Album membership additions so the browser knows which requested Photos were newly added and which were already members.
- Add examples that cover a full success, a partial result, a changed-elsewhere result, a missing Photo, and a scoped Album compensation.
- Add or update protocol vectors for every new wire shape. The target examples live in `batch-workflows.json` and have an executing structural consumer in `slipstream-compat`; every executed browse vector or response golden that pins a superseded batch shape must be replaced by the slice that changes that route. The old Photo State route fixtures are explicitly transitional and Slice 3 owns their replacement. Slice 2 replaces the executed Album membership vector and response golden when the Add Members response gains identity fields. Every vector must have an executing consumer.

### Exit criteria

- An independent reviewer can derive the expected UI and wire behavior without reading the implementation.
- No current spec still says that existing Photos are silently overwritten if the optimistic policy is selected.
- The product wording does not call Album compensation `Undo`.
- If old route fixtures remain during this slice, the exact retirement owner and completion gate are recorded above; no final Epic acceptance may pass while both old and new batch shapes are active route contracts.

## Slice 1: Selection Tray, Results, and Progress Scope

### Outcome

The current batch UI makes its lifecycle visible without changing the server contract.

### Files allowed

- `apps/web/src/pages/library-browser/page.ts`
- `apps/web/src/pages/library-browser/ui/library-browser-view.ts`
- `apps/web/src/pages/library-browser/ui/library-browser.css`
- `apps/web/src/browser-review.browser-test.ts`
- `apps/web/src/pages/library-browser/model/photo-owner.test.ts` when a presentation-facing Undo assertion needs a model pin

### Implementation shape

- Extend `GridViewModel.multi` with the presentation state needed for an empty active Select mode, a retained selection, a pending result, and a result action.
- Keep selection identity and range-anchor ownership in `page.ts`; do not move selection state into the view.
- Add an expected-state map alongside `multiSelection`. This map records the last confirmed Selection State for each selected Photo and is updated only after a confirmed result.
- Render the tray while Select mode is active, including `0 / 100` before the first selection.
- Keep the tray after successful Select/Reject and show the global Undo action separately from the selection controls.
- Render the Album resume message only when the active source is an Album and a batch Selection State decision or batch Album addition settles. Render the ordinary saved-position result for `Remove added Photos` when it removes the saved Photo.
- Render `Visible results` and `Source progress` as separate labels using the existing source total and `selectionCounts` values.
- Keep the existing disabled-control focus hand-off. Pending results must park focus on a stable control and restore it only when the held control is still available.
- Keep `Clear`, Escape, source open, and failed reopen paths using `clearMultiSelection` and `resetGridMultiSelection` so DOM markers and model state cannot diverge. Select mode exposes its exit even at `0 / 100 Photos`; `Review N` focuses changed Photos without opening a modal, while missing Photos remain visible but are not resubmitted.

### Tests

Add focused browser coverage for:

- Select mode showing `0 / 100` before a Photo is selected.
- Retained selection after Select, Reject, and Add to Album result presentation.
- Clear and Escape removing the tray, markers, and `aria-pressed` state.
- Visible-result count and complete-source progress being labelled separately.
- Album result stating that the resume point is unchanged.
- Keyboard focus during pending and after settlement.
- Narrow viewport layout with every batch action reachable.

### Verification

```text
bun run --cwd apps/web test:unit
bun run lint
bun run typecheck
bun x playwright test apps/web/src/browser-review.browser-test.ts -g "multi-selection|batch|Select mode|progress"
bun run verify
```

## Slice 2: Album Membership Compensation

### Outcome

A successful batch Album addition identifies the newly added Photo IDs and offers a bounded, scoped removal action without changing global Selection State Undo.

### Files allowed

- `crates/slipstream-core/src/domain.rs`
- `crates/slipstream-core/src/lib.rs`
- `crates/slipstream-core/src/library.rs`
- `crates/slipstream-core/src/persistence/owner.rs`
- `crates/slipstream-core/src/persistence/mod.rs`
- `crates/slipstream-server/src/app.rs`
- `crates/slipstream-server/src/http.rs`
- `crates/slipstream-server/src/lib.rs`
- `crates/slipstream-server/src/wire.rs`
- `crates/slipstream-server/src/tests.rs`
- `crates/slipstream-compat/src/lib.rs`
- `apps/web/src/pages/library-browser/api/album-actions.ts`
- `apps/web/src/pages/library-browser/model/album-action-owner.ts`
- `apps/web/src/pages/library-browser/model/album-action-owner.test.ts`
- `apps/web/src/pages/library-browser/page.ts`
- `apps/web/src/pages/library-browser/ui/library-browser-view.ts`
- `apps/web/src/browser-review.browser-test.ts`
- `compatibility/protocol/batch-workflows.json`
- `compatibility/protocol/browse-vectors.json`
- `compatibility/protocol/responses.json`
- `design/photo-organization.md`
- `design/web-async-ownership.md`
- `docs/library-browsing-and-selection.md`

### Contract

Keep the existing Add Members route and its 100-Photo bound. Slice 2 must retire or replace the executed `album-add-existing-member` browse vector and its matching response golden when the response gains identity fields. Its successful response must identify:

- `addedPhotoIds`: requested Photos newly inserted into the Album;
- `alreadyMemberPhotoIds`: requested Photos skipped because membership already existed;
- the refreshed bounded Album summaries used by the existing source list.

Add one bounded compensation operation for the returned `addedPhotoIds`. Prefer a batch remove command at the existing Album ownership boundary over up to 100 independent browser requests. The operation must:

- accept at most the same bounded number of unique IDs;
- remove only memberships in the named Album;
- be idempotent for a Photo already removed;
- report removed and already-absent IDs separately;
- preserve the existing saved-position rules when the saved Photo is removed.

Do not call this operation `Undo` in the user-facing surface or in the domain model.

### Implementation notes

- Avoid changing the general `AlbumMutationResult` shape for create, rename, delete, and reorder. Add a dedicated membership batch result or a dedicated application method so unrelated Album routes do not gain meaningless fields.
- Keep the existing single-member route for Photo View membership toggles. Add the dedicated bounded `POST /api/albums/{albumId}/members/batch-remove` route rather than issuing up to 100 independent requests.
- The client compensation record belongs to the page-level batch workflow and expires on source change, a new membership action, target Album deletion, or application teardown.
- A compensation settlement must not replace a newer Album action's status.

### Tests

- Core transaction tests identify added versus existing IDs and preserve positions for existing members.
- Core and HTTP tests cover bounded removal, missing members, duplicate IDs, unknown Photos, and saved-position invalidation.
- API validation rejects malformed or incomplete result shapes.
- Album action owner tests cover admission, supersession, transport failure, and compensation settlement.
- Browser test adds Photos to an Album, covers an already-absent newly added Photo and saved-position messaging, removes only newly added Photos, verifies existing members remain, and verifies selection and global decision Undo are unaffected.

### Verification

```text
bun run test:rust
bun run --cwd apps/web test:unit
bun run lint && bun run typecheck
bun x playwright test apps/web/src/browser-review.browser-test.ts -g "batch Add to Album|Remove added Photos"
bun run verify
```

## Slice 3: Optimistic Batch Concurrency

### Outcome

A batch does not silently overwrite a Selection State changed after the browser last confirmed it.

### Files allowed

- `crates/slipstream-core/src/domain.rs`
- `crates/slipstream-core/src/library.rs`
- `crates/slipstream-core/src/persistence/owner.rs`
- `crates/slipstream-core/src/persistence/mod.rs`
- `crates/slipstream-server/src/app.rs`
- `crates/slipstream-server/src/http.rs`
- `crates/slipstream-server/src/tests.rs`
- `apps/web/src/pages/library-browser/api/photo.ts`
- `apps/web/src/pages/library-browser/model/photo-owner.ts`
- `apps/web/src/pages/library-browser/model/photo-owner.test.ts`
- `apps/web/src/pages/library-browser/page.ts`
- `apps/web/src/browser-review.browser-test.ts`
- `compatibility/protocol/browse-vectors.json`
- `compatibility/protocol/responses.json`

Slice 3 must retire or replace the executed Photo State `photoIds`/`conflicts` browse vectors and response golden when the optimistic request and response become live.

### Request model

Replace the batch request's bare `photoIds` list with one bounded item per Photo:

```json
{
  "selectionState": "selected",
  "photos": [{ "photoId": "photo-1", "expectedCurrent": "undecided" }]
}
```

The server must reject an empty list, duplicate IDs, missing expected values, invalid Selection States, unknown fields that alter the contract, and more than 100 items before any write.

### Response model

Return exactly one outcome per request item:

```json
{
  "applied": [{ "photoId": "photo-1", "priorValue": "undecided" }],
  "changedElsewhere": [{ "photoId": "photo-2", "currentValue": "rejected" }],
  "missing": [{ "photoId": "photo-3" }]
}
```

The transaction must compare each existing Photo's current state with its expected state. A mismatch produces `changedElsewhere` and no write for that Photo. A missing row produces `missing`. Matching rows are applied and become the only entries eligible for the one-level batch Undo.

### Client behavior

- Capture expected state when a Photo enters the multi-selection.
- Update expected state to the new value only for `applied` outcomes.
- Keep changed and missing Photos in the multi-selection, but do not fabricate or move their facts. Missing Photos remain counted for presentation but are not retry candidates.
- `Review N` focuses and refreshes the N changed bounded facts, then replaces their expected states before a retry.
- A retry sends only the still-selected changed Photos whose expected state is known.
- A malformed or incomplete response moves no local facts, progress counts, or Undo entries; the whole selection remains retryable.
- Batch Undo continues to use the existing single-Photo compare-and-set route and remains independent of the new initial-write comparison.

### Tests

- Core transaction tests pin match, mismatch, missing, mixed, and atomicity behavior.
- HTTP and compatibility vectors pin exact response fields and omission rules.
- Client validators reject omitted, duplicated, or invented outcomes.
- Photo owner tests pin expected-state capture, partial application, retry preparation, and one-level Undo contents.
- Browser tests create a real changed-elsewhere condition through a second write, assert that the changed Photo is not overwritten, review it, retry it, and verify the other Photos complete.

### Verification

```text
bun run test:rust
bun run --cwd apps/web test:unit
bun run lint && bun run typecheck
bun x playwright test apps/web/src/browser-review.browser-test.ts -g "changed elsewhere|batch conflict|batch Undo"
bun run verify
```

## Slice 4: Browser Reliability and Qualification

### Outcome

The product flow is reliable under the conditions that motivated the design review.

### Files allowed

- `apps/web/src/browser-review.browser-test.ts`
- `apps/web/src/pages/library-browser/ui/library-browser-view.ts`
- `apps/web/src/pages/library-browser/ui/library-browser.css`
- `apps/web/src/pages/library-browser/page.ts` only if a test exposes a real lifecycle defect
- `docs/0.1-support-and-release.md` only for an approved support limitation

### Required checks

- Fix the existing source-switch test that reads a null thumbnail `src` instead of relying on a timing-sensitive element handle.
- Fix the existing filmstrip image wait so it distinguishes an unavailable placeholder from an image that is still loading.
- Repeat both tests under the same CI-like conditions before classifying another failure as a flake.
- Run the approved HTML prototype flow against the production build at desktop and 390px mobile widths.
- Dogfood with at least 130 Photos, a filtered source, an Album source, a slow or interrupted request, and two browser tabs before calling the new contract complete.

### Verification

```text
bun run test:fast
bun run test:rust
bun run test:browser
bun run verify
```

## Merge Gates

Each PR must include:

- links to the governing Product and Design Spec sections;
- exact focused commands and results;
- browser coverage for pointer, keyboard, narrow viewport, success, partial result, transport failure, source change, and retry;
- no changes outside its allowlist;
- an independent review with findings resolved or explicitly deferred;
- `MERGEABLE CLEAN` and a green canonical CI run;
- squash merge only after the base branch is current.

The parent tracking Issue must be updated only after each PR merges. The final completion condition is not the existence of the UI: it is a green full gate plus verified behavior for compensation, concurrency, progress scope, and source lifecycle.

## Risks and Rollback

- If the optimistic batch contract creates unacceptable wire churn before 0.1, keep the current endpoint temporarily and record last-writer-wins as an explicit support limitation. Do not implement a half-client, half-server comparison model.
- If membership compensation cannot preserve an acceptable user promise, ship the scoped action only after the UI calls it `Remove added Photos` and states that it is not historical Undo. Otherwise defer the feature and keep the existing Add to Album behavior.
- A failure in a new batch operation must not mutate Original Files. All rollback is at the SQLite transaction and UI ownership boundaries; Original Files remain read-only.
