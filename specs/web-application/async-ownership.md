# Web Async Ownership

The Web client starts asynchronous work whose settlement can outlive the UI state that initiated it.

A stale operation MUST NOT revert newer shared data, write into a newer source or Photo, erase a newer failure notice, or release a recovery claim it does not own.

Async ownership has two independent dimensions:

1. the operation's **effect**: presentation read, shared-state read, lifecycle
   release, admitted persistence, or another admitted server command;
2. its **supersession policy**: abort transport, detach continuation, commit in
   order, or always settle.

A read is not automatically abortable, and a write is not automatically tied to the lifetime of its initiating UI.

Workflows decompose into child operations; they never acquire an implicit fifth policy.

## Design Drivers

- Admitted server work is never treated as rolled back by HTTP cancellation.
  Once a persistence mutation or scan command is sent, the server can commit
  or continue it; aborting the fetch only blinds the client.
- Publication and Browse snapshot coherence require one generation at a time.
  A stale Folder or Browse result is never silently reinterpreted as current.
- Cancellation cannot replace ordering. Two successful reads can settle out
  of order, and a newer request can fail while an older valid response remains
  useful.
- Lifecycle cleanup is distinct from both reads and persisted user state: a
  Browse-token release is best-effort cleanup with bounded server expiry as
  its fallback.
- The client remains dependency-free. The required behavior fits one scoped
  task primitive and one write-settlement rule.

## Operation Model

A **scope** owns operations whose UI continuation becomes invalid together.

Halting a scope prevents every child continuation from committing.

Depending on the child's declared policy, halting also aborts the transport or merely detaches the continuation while transport finishes.

An **ownership key** identifies work that supersedes or coalesces other work.

Source- and Photo-scoped keys include their captured generation so one source or Photo cannot supersede another accidentally.

Intentionally global work (the overview and Album settlement families) and uniquely identified cleanup (a Browse-token release) do not add a generation.

A **settlement handle** belongs to non-abortable work.

It captures the initiating generation and presentation surface, runs the request to settlement, and decides whether the result MAY still write locally, MUST fallback to the Library summary, or stays silent.

A generation is ownership metadata captured by an operation.

It is not a separate operation family.

## Operation Assignment

### Publication status polling

`GET /api/status` is a presentation read owned by the application scope with the global `publication-status` key.

The first committed overview starts one application-lifetime status monitor whether the Library is published or not.

The monitor continues at a bounded idle cadence so it observes scans triggered outside the current tab; while scan state is non-idle it MAY use a foreground progress cadence.

Starting a replacement monitor detaches the preceding timer, loop, and in-flight continuation; transport need not be aborted.

Application teardown does the same.

A detached monitor never writes scan status or starts an overview load.

An answered HTTP non-success or transport failure writes no scan status, creates or releases no Recovery claim, and leaves the current monitor running.

A transport failure also clears the reachability axis described under Connectivity and recovery ownership, and a usable answer sets it.

A successful response MAY write scan progress only as a background update through the Summary Notice Channel.

Each individual status request captures a fresh non-owning background epoch when that request starts; the lifetime monitor does not reuse one epoch.

Transition handling compares the response with the last scan state observed by this monitor.

A first committed overview whose scan is already `idle` establishes the baseline and does not invent a completion event because no open snapshot predates that commit.

A semantic `failed` scan retains the prior Published Library, keeps polling at the resting cadence, and claims one actionable `scan-failure` Summary notice with a **Retry Library Check** action.

That notice is the claim: further failed polls keep the monitor probing for reachability without claiming it again, and a missing notice is claimed once even when the committed overview already reported the failure.

When a current poll first observes `idle` after a non-idle scan, it advances the overview data floor, claims a completion notice that the prior open Browse Snapshot remains unchanged, and offers **Refresh Current Source**.

It also starts `loadOverview` to refresh shared facts and File Locations, but the completion notice remains until explicit source refresh or a higher-priority notice replaces it.

### Library scan retry command

`POST /api/scan` is a non-abortable admitted server command owned by the application.

Duplicate Retry Library Check admission while one command is in flight is a no-op.

Starting the command creates a scan-completion handle and records a known non-idle transition for that handle before awaiting the terminal response.

The existing status monitor remains running.

The first of (a monitor transition to `idle`) or (the command's successful terminal `idle` response) consumes the handle, advances the overview floor, and presents the completion notice; the other path is an exact-handle no-op.

This guarantees exactly-once completion even when the scan finishes between monitor polls.

An answered `4xx` keeps the scan-failure notice without adding a transport Recovery claim. `5xx` or transport failure adds a scan-command Recovery claim and keeps the same retry action.

Successful terminal settlement releases that exact claim.

Application teardown detaches presentation from the command but cannot treat accepted scan work as rolled back; the application-owned server Scan Cycle continues independently of the HTTP waiter.

A client-side timeout or transport loss on the scan command is an unknown outcome, not a cancellation or terminal result. The monitor reconciles the cycle through `GET /api/status`.

A re-issued command joins the still-running server cycle rather than starting a second physical scan.

Concurrent startup and explicit scan admissions join one application-owned leader.

It publishes once, returns one captured terminal status to all live waiters, and completes status accounting even if all HTTP waiters disconnect.

### Shared overview refresh

`GET /api/overview` is an application-owned shared-state read keyed by `overview`.

It uses **commit-in-order**: newer requests MUST NOT abort or detach older ones.

Each request captures a monotonically increasing sequence and the current overview data floor.

Successful Album mutations and observed publication replacement advance the floor before refresh.

Published Overview and Loading Status responses carry the same opaque publication generation as File Locations.

Immediately before commit, the Overview response MUST revalidate that generation with Loading Status.

```text
if publication was replaced:
    discard response and advance floor to observed current generation
else if captured floor equals current floor and sequence is newer than last commit at that floor:
    commit response
else:
    discard response
```

An older success can commit after a newer request fails if no data-changing boundary intervened.

A response started before a committed mutation or publication change MUST NOT regress shared data.

Application teardown detaches all overview continuations.

Overview failure has no notice or Recovery policy of its own.

Its live parent supplies Summary and Recovery claim handles and decides whether failure is a foreground reload failure, a persisted-Album-but-refresh-failed notice, or a silent recovery probe.

The owner action controls only that presentation; it does not replace commit ordering or the data floor.

### File Location windows and root binding

`GET /api/file-locations` belongs to an independent File Location scope with key `(fileLocationGeneration, parent)`.

Source, Grid, and Photo changes neither abort nor detach it.

Only same-parent supersession, explicit count refresh, File Location generation reset, or application teardown MAY detach it.

Its transport can finish after detachment, but its response cannot render, clear retry state, or replace the parent's current page.

An answered publication conflict (`409`) advances the overview data floor, resets the File Location generation, refreshes overview as a silent recovery probe, rebinds the root, and claims an actionable summary notice only after the new root is coherent.

Any other answered failure, malformed response, or transport failure for the current key retains loaded siblings, records the exact failed range, claims the actionable summary notice, exposes that range's retry control, and changes connectivity to disconnected.

Only success for that same range clears its retry state, releases its Summary claim, and releases that range's Recovery claim.

Success for another File Location window can prove server reachability but cannot release the failed range or any current Source/Photo recovery claim, and therefore cannot re-enable decisions by itself.

The unbound root uses the distinct key `(fileLocationGeneration, root-binding)`.

Exactly one operation owns that key; other callers await its settlement instead of starting or cancelling a second bind.

Resetting the File Location generation or tearing down the application releases those waiters with an unbound result.

Root binding is not superseded by ordinary child-window work.

Explicit count refresh MUST renew the File Location request scope while preserving publication authority and existing exact-range failure identities. It MUST reject already-produced outcomes from the previous request scope and release superseded waiters without awaiting their transports. The [File Location refresh plan](../library-browser/design.md#file-location-window) owns which current requests are re-admitted.

### Browse open and reopen

`POST /api/browse` is a presentation read that allocates a bounded server snapshot.

It is owned by the source scope, keyed by the source generation, and uses **abort-transport** policy when a newer source supersedes it.

If a completed stale response exposes a token, the client starts a token-release operation.

If aborting loses the response after server allocation, bounded server expiry remains the cleanup fallback.

During a current source open or reopen, retained Grid cells remain display-only.

Their enabled state and Photo-open admission share one current-source readiness predicate, so an enabled action is immediately admissible.

The replacement source's first required window establishes that readiness; a failed or superseded reopen follows its existing recovery path.

When the first required window fails, its placeholder range stays presented. The Grid's range report re-admits it with the same establishing operation kind as a manual Retry.

Recovering facts without establishing readiness would leave every cell disabled with no visible Retry.

A Folder Browse open additionally captures the bound File Location publication.

It never sends without that publication, and publication expiry starts a new root-binding operation before a retry.

A current open or reopen failure retains the recoverable source, reports on the Grid/source surface, and changes connectivity to disconnected.

A Folder publication conflict also runs the File Location recovery above; its actionable summary notice is independent of the source failure message.

Superseded failures are silent.

### Browse windows

`GET /api/browse/{token}` is a presentation read keyed by `(sourceGeneration, alignedStart)` within its owning scope.

Grid scrolling and Grid look-ahead use the source/Grid scope.

A boundary window needed to open or retry the current Photo uses the Photo scope at high priority. It aborts an in-flight low-priority Grid or adjacent request for the same range, then starts a foreground request instead of coalescing with the aborted work.

A source change aborts every window request from the old source.

A response commits only to its captured source generation and token.

`GET /api/browse/{token}/position?photoId={id}` is a source-owned bounded read.

It captures the current source generation and token, returns one position or `null` from that immutable Snapshot, and never transfers Photo facts.

A source change or application teardown detaches the lookup.

An expired Snapshot follows the existing source-reopen recovery path; another answered or transport failure leaves the current view intact and keeps a stable-identity Undo available for retry.

Photo navigation keeps its target index pending while a required window is unavailable.

The Photo owner commits that index and the source's Grid position only after the target fact is available under the same current owner.

A failed or detached window leaves the prior committed Photo binding unchanged, so Photo Retry derives its identity and aligned range from the Photo still shown to the Photographer.

A source reopen MAY replace that binding only through its own current source authority.

An answered `404` starts current-source Browse recovery.

If recovery fails, the owning Grid or Photo surface identifies that the source expired and remains retryable.

A current-generation transport failure changes connectivity to disconnected and presents range retry on the owning Grid or Photo surface.

Another answered failure or malformed response retains connectivity and loaded content, identifies the failed range on the owning surface, and permits that range to be requested again.

An aborted or superseded settlement is silent.

A speculative adjacent-window failure follows the same read contract; an adjacent Preview failure remains separately silent.

### Browse-token release

`DELETE /api/browse/{token}` is a lifecycle release keyed by token.

It is best-effort detached cleanup: UI teardown does not abort it, it presents no success or failure notice, and it never changes connectivity.

Bounded server expiry is the fallback if the request cannot complete.

### Current Preview and adjacent Preview

The current Preview read is owned by the Photo scope with key `(requestGeneration, photoId, current)`.

Leaving the Photo, changing Photo, or changing source aborts its transport and prevents every continuation after an `await` from rendering, persisting position, or updating controls.

An answered non-ready Preview keeps the Photo navigable, reports the server's Preview message on the Photo surface, and does not change connectivity.

A transport or malformed-response failure reports Photo retry and changes connectivity to disconnected.

A superseded failure is silent.

Adjacent Preview reads use key `(requestGeneration, photoId, adjacent)`, run at low priority, and are best effort.

The Photo scope aborts them on Photo or source change.

Their answered and transport failures are silent and never change connectivity.

When that neighbor becomes current, the browser starts a foreground current-Preview request for the same Photo and derivative identity.

The Preview service deduplicates that identity and raises the existing native job to current priority rather than running duplicate work; browser-request cancellation does not cancel an admitted reusable server job.

### Thumbnail reads

Thumbnail reads are owned by the source/Grid scope with key `(sourceGeneration, photoId)`.

Requests with the same key coalesce.

Source change or application teardown aborts old work.

Only the captured generation MAY attach a thumbnail or mark its delivery failed.

Answered and transport failures leave a stable failed-delivery placeholder; they present no summary or connectivity failure.

Server-supplied Photo availability, Original availability, and Preview state remain bounded Photo facts; a browser-owned thumbnail delivery failure is retained separately and does not overwrite them.

### Browser image transfers

Assigning a URL to a Grid or Review `<img src>` starts a browser-managed byte transfer distinct from `fetch`.

Each Grid image transfer belongs to the source/Grid scope and each Review image transfer belongs to the Photo scope.

Before its scope is halted or its element is replaced, the client removes the pending `src` and its completion/error handlers.

A completion or error MAY write only while the element, URL, and captured generation still match; a detached image error cannot mark a replacement cell or Photo unavailable.

### Album persistence

Album create, rename, delete, membership add, membership remove, and scoped batch compensation are admitted writes.

They share one global settlement family key, `album-mutation`, because a newer Album action owns Album notices and connectivity presentation regardless of which Album it targets.

A batch Add to Album stores its returned `addedPhotoIds` in a page-level compensation record.

The record is owned by the source generation and Album operation that created it; a new membership operation, source change, target Album deletion, page teardown, page reload, or application teardown expires it.

A compensation request can settle, but its local result can write only while that record and Album remain current.

The write always settles.

Immediately after success it advances the overview data floor.

Its shared-data refresh is a separate owner-tagged overview operation captured at that new floor and therefore follows overview commit order without permitting any pre-mutation response to regress Album state.

A current success MAY update its initiating form or Photo controls; a superseded success is locally silent and the committed overview is its confirmation.

A current failure writes to its initiating surface.

A superseded failure falls back to the Library summary unless a newer Album failure already owns it.

Membership duplicate suppression is a separate admission key, `(verb, albumId, photoId)`.

Repeating the same key while in flight is a no-op; a different verb or Album is independently admitted.

Form continuation ownership is keyed by stable form ID, so an older settlement never clears a newer form or draft.

Contextual creation and batch Album choice retain only the bounded return and target context defined by [Responsive Surfaces](../library-browser/experience/design.md#contextual-album-creation-and-choice). Replacing a chooser with the creation form MUST NOT change source ownership.

Creation MUST NOT schedule membership automatically. Closing or superseding the context MUST suppress its return continuation without aborting creation. A later explicit membership submit captures its own IDs and existing settlement handle.

Answered `404`, `409`, and other `4xx` failures do not change connectivity.

Transport failures and service failures (`5xx`) change connectivity only while the operation still owns the global Album settlement family.

### Recovery review and relocation apply

`GET /api/recovery/unavailable` is a presentation read owned by the application scope with the global `recovery-survey` key.

Opening or refreshing the review supersedes an older survey; the response MAY write only while the review remains open and its key is current.

Application teardown detaches it and presents nothing.

`POST /api/recovery/propose` is a presentation read keyed by the current review generation.

A newer proposal request supersedes an older one.

An answered or transport failure keeps the review open, reports on the review surface, and does not change connectivity; a superseded failure is silent.

`POST /api/recovery/apply` is an admitted server command.

It always settles after send: once sent, the batch can commit on the server even when the review or the page closes.

Success refreshes shared recovery facts and the affected open source; a late success MUST NOT overwrite a destination that a newer operation changed.

An answered conflict keeps the prior mappings visible, reports the refusal on the review surface, and offers a fresh proposal.

Other answered failures report that nothing was applied, and a transport failure leaves the batch retryable without claiming whether it committed.

An apply failure that loses its review surface falls back to the Library summary.

Proposals MUST NOT write.

A refused batch keeps its per-mapping reasons visible.

The review is one bounded entry.

It MUST distinguish not found, unreadable, Preview failure, request failure, and disconnection, and it MUST show one primary actionable failure for the affected Photo.

Thumbnail and Preview errors for a Photo whose Original unavailability is already established are suppressed as redundant.

### Photo-state persistence and Undo

Selection, Rating, and Undo are admitted writes serialized by the current Photo interaction (`busy` permits one at a time).

Their settlement ownership key is `(sourceGeneration, photoId, field)`.

They always settle after send.

If source generation or Photo identity changes, their success and failure continuations are silent; they do not fallback to the Library summary.

A Grid decision is the same admitted write addressed by Grid position instead of by the current Photo.

It shares the one admission, the one-level Undo description, and every failure classification, and it never advances the current Photo.

A write keeps its identity while its own address still holds the Photo it captured: the current Photo for a Photo View write, and the Grid position for a Grid write.

The browser stores an Undo target by stable `photoId`; a Snapshot index is only an optimization hint and never establishes identity.

After a same-source Snapshot replacement, the Photo owner resolves that ID through the current Snapshot position lookup before requesting a bounded window or sending the compare-and-set Undo write.

A `null` position means that the target no longer belongs to the source, so the browser retires the one-level Undo description without a write.

A failed lookup retains the description and current Photo so the Photographer can retry.

Current Selection, Rating, and Undo failures have these effects:

| Settlement                                                          | Selection or Rating                                                              | Undo                            | Connectivity |
| ------------------------------------------------------------------- | -------------------------------------------------------------------------------- | ------------------------------- | ------------ |
| Answered `409`                                                      | Report concurrent change on the Photo surface; Retry MUST refresh current facts. | Report stale Undo.              | Disconnected |
| Other answered non-success, including `404`, other `4xx`, and `5xx` | Report that the change was not saved.                                            | Report that Undo was not saved. | Unchanged    |
| Transport failure                                                   | Report uncertainty.                                                              | —                               | Disconnected |

A Grid batch Selection State write shares that one admission and the one-level Undo.

The batch address captures multi-selected stable Photo identifiers and their last browser-confirmed Selection States at admission. It keeps that identity while the source generation remains current.

A source change detaches its continuation; confirmed server work stays committed.

Its settlement reports `applied`, `changedElsewhere`, and `missing` outcomes from one bounded batch response.

The server never silently overwrites a changed expected value.

| Batch Selection outcome | Effect                                                                 |
| ----------------------- | ---------------------------------------------------------------------- |
| `applied`               | Add the Photo to the one Undo description.                             |
| `changedElsewhere`      | Keep the Photo selected for `Review N` and retry.                      |
| `missing`               | Keep the Photo selected as non-retryable until clear or source change. |

A malformed response is untrusted if an outcome field is omitted, duplicated, invented, or obsolete. It changes no facts, counts, or Undo entries and leaves the whole selection retryable.

A batch Undo is one settlement family of sequential single-Photo compare-and-set writes:

| Settlement                 | Effect                                                                                       |
| -------------------------- | -------------------------------------------------------------------------------------------- |
| `409`                      | Retire only the Photos whose response proves that the value to restore is no longer current. |
| Other answered non-success | Keep the remaining Photos retryable.                                                         |
| Transport failure          | Keep the whole description retryable.                                                        |

A batch Undo never navigates: the change it restores never advanced away from a Photo.

### Edit Recipe reads and guarded saves

An Edit Recipe read is owned by the Photo scope with key `(requestGeneration, photoId, recipe)`.

It starts when a Photo's Develop surface opens; the Photo scope aborts it on Photo or source change, and a stale read writes neither controls nor committed state.

Readiness is represented by independent source-support, processing-capability, and preview-result axes: `read-pending` and `resource-unavailable` are retryable waits, not evidence that the Original is unreadable.

Reads for the same Photo and source revision join one in-flight request; a successful publication advance invalidates that request's captured facts and schedules one replacement read rather than creating parallel reads.

A save is an admitted write with settlement-family key `(photoId, recipe-write)`. The newest save for that Photo owns the Develop notice and its connectivity presentation.

A superseded save stays locally silent; its committed revision remains observable through the recipe read.

The client coalesces edits during one gesture and sends one save per settled gesture through a single per-Photo queue; a settled gesture waits behind an in-flight save instead of running concurrently.

Each queued write carries the stable request identity captured for it and the revisions the Photographer observed; identity is never regenerated for a retry.

`recipe_conflict`, `source_changed`, and `requires_rebind` present current server facts and require an explicit Photographer action before a further write; they never silently adopt the newer revision or the newer source.

An `unknown` outcome keeps the pending draft and offers retry under the same identity. `unsupported`, `invalid_settings`, `unavailable`, and transport failures keep the last committed revisions in the controls and never claim the draft was saved.

Session undo, redo, and reset are browser-local over drafted settings; none is a save until the next settled write.

Each session undo, redo, or reset becomes one guarded write through the same per-edit queue as an ordinary edit. It uses a new request identity and settles as its own operation.

The tool's confirmed state records the reversal exactly as it records a slider change.

A revision restore from the tool's retained history is a separate tool-owned operation with its own request identity and is never presented or submitted as a session undo, redo, or reset.

An `unavailable` read or refusal carries the closed `supportReason`.

A retryable reason (`read-pending`, `resource-unavailable`) presents a wait state: the Develop surface keeps its last committed revisions read-only with a retry or refresh action and re-reads later; it never presents the source as unreadable.

A confirmed reason (`original-missing`, `original-unreadable`) presents the permanent read failure.

The Develop surface keeps the engine, Library, and Photo axes separate — capability note, scan status, and source reason each name their own condition — and native-work saturation never downgrades a supported Photo's presentation.

An Edit Preview request is owned by the Photo and selected Processing Step scope with key `(requestGeneration, photoId, step, identity)`.

The identity includes the exact staged/input byte digest and size, the input artifact or guarded source binding, selected module and adapter/schema version, complete parameter snapshot or digest, bounded Preview geometry, processing bundle, and display conversion.

An Original's guarded source revision remains a separate source guard; it does not replace the exact byte evidence in the identity.

Requests with the same full identity coalesce.

A changed identity supersedes the older request locally without claiming to cancel admitted server work, and displays the last image as stale while the replacement is pending.

A superseded result never publishes, including one whose source revision is unchanged but whose selected module, parameter digest, artifact input, or proxy digest changed.

The rendered image retains explicit provenance: Original-backed, Processing-Artifact-backed, and development-proxy-backed results are distinct facts.

A proxy-backed result is labelled as such rather than presented as full-resolution Original evidence.

A refusal names the selected module, input compatibility, and reason.

Preview failure never changes Originals, committed recipe intent, or a published artifact.

### Export submission and lifecycle

Export submit, inspect, cancel, and retry are owned by the Photo scope with settlement-family key `(photoId, export-command)`.

Submission captures the revisions the Photographer observed and renders its returned snapshot, and is admitted only against a confirmed save; an `unknown` save outcome is reconciled first.

A submit that loses its response presents an `unknown` outcome and resolves by inspecting that Photo's current Exports, never by resubmitting with a fresh identity.

Cancellation settles once against the returned state, and a result that raced it is presented as the actual terminal outcome.

Inspection polling stops with the Photo scope, and a later poll never refreshes controls for a superseded identity.

### Artifact downloads

An artifact download is a browser-managed byte transfer owned by the Photo scope and follows the Browser image transfer rules. Before its scope halts or the element is replaced, the client removes the pending transfer and its handlers.

A completion writes only while the download record and Photo identity remain current.

The Develop surface discloses the artifact's expiry from Export metadata.

A download requested after expiry presents `artifact_expired` and never implies that settings or the artifact changed.

### Saved Album position

Saved-position writes are admitted writes serialized by the progress queue and keyed by `(sourceGeneration, requestGeneration, albumId, photoId)`.

They always settle after send.

Their settlement handle captures both the Album source owner and current Photo owner.

Answered `404` and `409` are stale-position results and do not change recovery state.

Any other answered non-success—including other `4xx` and `5xx`—and transport failure adds a saved-position Recovery claim only while the same source generation, Photo generation, Album, and Photo remain current.

A settlement from an older Photo is silent; saved-position writes never fallback to the Library summary.

### Trash inspection, review, and record cleanup

`GET /api/trash` is a presentation read owned by the Trash scope with key `(trashGeneration, inspectionScope, range)`.

Leaving Trash, reopening it, or a newer request supersedes the older window.

A superseded response MUST NOT render, clear a retry state, or change selection.

Client generation fencing does not freeze server membership: complete captured inspection follows the separate [Trash scope contract](../photo-library/trash/spec.md#querying-and-printing-locations).

Captured inspection MUST NOT silently substitute offset pages or include later arrivals.

Review captures an explicit selected record set and the observed removal and Original evidence.

Later Trash arrivals never join it.

Explicit Cancel, Close, Escape, and scrim dismissal all clear the same unsubmitted review state; closing a dialog MUST NOT leave an invisible review blocking the next action.

Record cleanup is an admitted server command, not a file-deletion command.

The client retains its attempt identity before sending confirmation.

Leaving Trash detaches presentation only; it does not cancel an accepted database effect.

A lost or unusable response is an unknown outcome, not proof of rollback.

A confirmed refusal before admission remains a no-effect refusal.

Read-only outcome inspection recovers the same attempt's durable record results.

It MUST NOT retry filesystem work, widen the set, or submit a fresh confirmation to discover what happened.

Reopening Trash preserves the pending outcome and its recovery actions across listing loads and failed outcome reads.

Successful settlement or recovery refreshes affected counts, rows, and Undo before presenting converged state.

A response renders only while its captured Trash generation, attempt identity, and surface remain current.

Leaving Trash clears unsubmitted selection and continuations.

No stale continuation writes local selections, outcomes, or counts after teardown.

The attempt identity stored before submission is recovery evidence, not a locally invented success.

Physical deletion and its former request/retry lifecycle are not supported under the record-only Trash contract.

## Composite Workflows

A workflow is only composition; each child retains its assigned policy.

A presentation workflow captures its parent owner before starting and rechecks that owner after every `await` and before sending each later child.

Superseded presentation workflows start no further children.

A shared overview that successfully commits MAY separately elect a bootstrap workflow as described below; that is a new child of the committed shared state, not continuation of a superseded presentation workflow.

### Overview loading and bootstrap

`loadOverview` and Retry own a foreground presentation ticket in the application scope. They start a shared overview refresh without cancelling older overview work. Only the newest pending load presents load failure or creates a reload Recovery claim.

Startup follow-up belongs to the newest successfully **committed overview**, not the newest request. When a response commits and the application still lacks its initial source, that commit elects a bootstrap owner.

The bootstrap owner starts or awaits independent root binding and can then start source-scoped Browse open. A newer failed request cannot strand an older valid committed response.

Every first committed overview ensures the single application-lifetime publication-status monitor is running. An unpublished commit withholds Browse opening until publication. A published non-idle commit remains browsable while the same monitor reports its background scan.

Every follow-up rechecks the elected commit sequence and application lifetime before starting its next child.

### Opening a Photo

Opening a Photo creates a new Photo scope and aborts Grid-speculative work. If the Photo lies outside retained facts, it starts a high-priority Photo-owned Browse-window read; it never coalesces with an aborted adjacent request.

If still current, it renders facts and starts current Preview. It sends saved position only while still current immediately before send; once sent, that write always settles.

### Opening an Edit surface

Opening a Photo's Edit surface opens or resumes its Photo Workspace and reads
the selected tool's canonical file state. It starts no rendering. Each settled
gesture sends one guarded tool mutation through the per-edit queue; a preview
refresh starts only for the committed revision and only while the Photo scope,
selected edit, and that revision remain current.

### Submitting an Export

Submitting an Export is the admitted submission followed, while the Photo
scope and the captured identity remain current, by inspection polling until a
terminal state. Cancelling replaces inspection for that Export only.

### Reopening an expired source

Reopening an expired source is source-owned. It MAY await independent root
binding, then starts Browse open, a high-priority source window, optional
Photo-owned Preview, and conditional saved position. Each child starts only
while the source and optional Photo owner remain current.

### Retrying a Photo

Retrying a Photo creates a new Photo owner and invalidates the current aligned bounded Browse-window facts. It reloads that range at high priority in the Photo scope and verifies source generation and Photo identity.

Only then does it revalidate current Preview and conditionally persist Album position. It reports Connected only if the window, Preview, and required position write all succeed under the same owners.

### Album mutation refresh

An Album mutation consists of the admitted write followed after success—and
only while the application still exists—by an owner-tagged shared overview
refresh. UI-surface supersession does not prevent shared overview commit.

### Album form submission

A form submit consists of form-continuation ownership plus the admitted Album write.

Destroying or replacing the form detaches the form continuation but never aborts the write.

## Notice and Teardown Rules

The Library summary has one **Summary Notice Channel** shared by Album, File Location, overview, status, scan failure/completion, failed-range recovery, and explicit reload.

An operation that can own a notice receives an opaque claim handle containing a monotonically increasing ticket, owner kind, and exact owner key (for example a failed Folder range).

A non-owning background HTTP request captures an opaque **background epoch** from the same monotonic sequence when that request starts.

A lifetime monitor therefore uses a fresh epoch for every status request, and each overview request does the same.

Persistent claims are identified by their full handle; within the same priority, only a newer ticket replaces the visible owner.

Notice precedence is:

| Priority   | Notice                                                                                |
| ---------- | ------------------------------------------------------------------------------------- |
| Highest    | Actionable scan failure or completion, File Location and range retry, explicit reload |
| Fallback   | Album failure                                                                         |
| Background | Ordinary overview and in-progress status                                              |

A background write is accepted only while no persistent owner exists, its epoch is not older than `reloadBarrierTicket`, and it is not older than the last accepted background epoch.

Rejection returns a stale/no-op result without changing DOM, owners, pending notices, barrier, or Recovery.

An accepted background write updates the last background epoch but never acquires ownership.

A lower-priority settled failure blocked by a higher-priority owner remains pending until that owner releases, unless explicit reload invalidated it.

A persistent owner is released only by presenting its exact claim handle or by a newer operation authorized for that owner kind.

A File Location retry success therefore releases only its exact failed-range owner.

A newer Album success MAY release an already displayed older Album notice, but it does not predeclare an in-flight older write successful; if that older write later fails, it MAY claim an otherwise free channel.

An explicit reload atomically advances a persistent `reloadBarrierTicket`, invalidates and removes every active or pending handle older than that barrier, and then claims the channel before its first request.

Every later claim, pending fallback, background write, and release is rejected when its ticket is older than the barrier, even after the reload's visible notice is released.

No invalidated owner remains present to block background writes or await an impossible release.

Application teardown invalidates all claim handles.

If the initiating source, Photo, or form disappears after an Album write is sent, success stays locally silent and shared data still refreshes; failure falls back through the Summary Notice Channel.

If those surfaces disappear after a Photo-state, Undo, or saved-position write is sent, both success and failure are locally silent, while the write still settles.

These different failure destinations are intentional product contracts.

### Connectivity and recovery ownership

Transport reachability and permission to make Photo decisions are separate.

The application has one **Recovery Gate** containing opaque claims keyed by the operation and UI owner that requires revalidation (Folder range, source snapshot, Photo generation, write, or scan command).

Every failure described as disconnected adds a blocking exact claim.

The retry action carries that handle: exact-range retry for File Locations, source Retry for source/Album failures, Photo Retry for current-window/Preview/Photo-write failures, and Retry Library Check for scan admission.

An unrelated successful request can prove that the server is reachable, but only the designated recovery under the same current owner releases the claim.

Decision controls are enabled only when no blocking current claim remains and the current bounded source and Photo facts have been confirmed.

The idle status poll is also the designated reachability signal.

A poll that cannot reach the server clears the reachability axis, and a usable status answer sets it.

That signal owns no claim: it neither adds a blocking claim nor retires the one a failed operation created, so reachability can be restored while decision readiness still waits for its own recovery.

Changing source or Photo creates a transition lineage.

Claims from owner A become predecessor claims for the in-progress owner B: they cannot present into B, but they keep decisions blocked during establishment.

If B establishes its bounded current facts successfully, that exact transition retires A's predecessor claims and any B establishment claim.

If B establishment fails, a current B claim replaces the predecessor blocker and A's claims retire; a later B retry or later successor releases only the then-current lineage.

An old A claim never reactivates if the UI later returns to the same logical Photo under a new generation.

Independent File Location claims remain outside this lineage until their own range succeeds.

Claim creation, replacement, and release are generation-gated, so stale failure and success cannot change the current gate.

Application teardown:

- halts the application, File Location, source/Grid, and Photo scopes;
- cancels timers and aborts abortable transports;
- removes pending browser image sources;
- detaches every remaining read continuation and root-binding waiter;
- invalidates Summary and Recovery handles; and
- starts best-effort release for a known Browse token.

Admitted writes continue to settlement but perform no DOM, connectivity, notice, or follow-up overview work after the application itself is gone.

## Ownership Module Contracts

The implementation provides one dependency-free ownership module containing four orthogonal mechanisms:

- scoped tasks create and halt scopes; start keyed children with
  `abort-transport`, `detach-continuation`, or `commit-in-order` policy; pass an
  `AbortSignal` only to abortable operations; coalesce keys where specified;
  and run cleanup exactly once;
- settlement handles run admitted writes without cancellation and decide
  whether their captured surface MAY still present the result;
- the Summary Notice Channel issues opaque owning `(ticket, kind, key)` handles and a fresh non-owning background epoch per HTTP request from one monotonic sequence. Its responsibilities are:
  - atomically invalidating pre-barrier active and pending handles;
  - retaining a reload-barrier floor and last accepted background epoch;
  - arbitrating persistent owners by priority and ticket;
  - reporting stale/no-op background writes;
  - performing exact-handle release; and
  - retaining at most the newest eligible pending fallback per owner kind;
- the Recovery Gate creates and releases exact-owner revalidation claims,
  separately records transport reachability, and reports whether current
  decision facts are ready.

The module attaches generation metadata without defining feature-specific task types and owns no DOM or protocol knowledge.

Source, Photo, Folder parent, Album, and form identifiers are supplied by callers as ownership keys.
