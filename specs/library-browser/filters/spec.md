# Source-Scoped Library Filters

A filter selects a temporary view of one Library source. It is not an Album, a deletion shortcut, or a saved query.

This contract owns shared Web and Agent filter behavior.

| Dependency | Responsibility |
| --- | --- |
| [Library Browser Experience](../experience/spec.md) | Presentation and history. |
| [Library Browsing and Selection](../spec.md) | Source order, Photo actions, loading, and decision persistence. |
| [Browse Snapshot](../design.md#browse-snapshot) | Server-side sequence. |

## Filter Model

A committed filter applies defined facts to exactly one source and its selected order. Different fact conditions combine with AND; multiple values are allowed only within the same defined discrete field.

The product MUST NOT accept arbitrary Boolean expressions, SQL fragments, or undocumented fields as filters.

Filtering MUST NOT change Photo identity, Selection State, Rating, Album membership, Original Location, or Original File bytes.

## Source Scope

A view selects exactly one source. [Source Order](../spec.md#source-order) owns its order.

| Source | Scope |
| --- | --- |
| All Photos | Every published Library Photo not in Trash. |
| Original Folder | Photos whose remembered Original Locations are in the recursive Folder subtree. |
| Album | The Album's explicitly ordered members. |

Folder and Album are alternative scopes. A workflow needing both MUST use an explicit bounded set of Photo IDs or a separate Album operation.

Trash, Recovery, and Processing queues are separate destinations, not ordinary Library filter sources.

## Defined Conditions

### Selection State

Web View options MUST provide All, Unflagged, Picked, and Rejected. `all` imposes no Selection State restriction; the other values select the corresponding [Selection State](../../../CONTEXT.md#browsing-and-selection).

A Photographer MAY change a decision while reviewing a filtered view. The change applies to the named Photo and does not imply removal, a Rating change, or Album membership.

### Rating

Rating `0` means No Rating, not a separate missing value. Rating bounds are inclusive integers from 0 through 5; clearing Rating sets it to zero without changing Selection State.

### Original Kind

`raw` and `jpeg` match the Photo's own Original File. Filtering MUST preserve the independent identity and Preview of same-basename RAW and JPEG Photos.

### Original Availability

Availability describes the remembered Original File, not Preview readiness. An unavailable Original retains its Photo and remembered facts; filtering does not perform [Location Recovery](../../photo-library/spec.md).

### Capture Time

A Capture Time range uses camera-local time, an inclusive lower bound, and an exclusive upper bound. A Photo without Capture Time remains eligible without a bound and does not match a supplied bound.

[Capture Time](../../../CONTEXT.md#library) comes from the Photo's own Original File, not filesystem modification time. The [CLI Reference](../../command-line/reference.md) owns the local-time syntax.

Web input MUST preserve camera-local seconds without converting through the browser's time zone. Date and time controls MAY use localized presentation, but MUST retain an exact seconds field and an exact-value text alternative when the browser cannot provide it.

Labels MUST distinguish **From (inclusive)** and **Before (exclusive)**. Omitted bounds remain unbounded; the UI MUST NOT silently supply an end-of-day bound or change an existing second value.

Equal or reversed bounds and time-zone-bearing values are invalid under the [CLI time contract](../../command-line/reference.md#photo-queries). Invalid bounds MUST be rejected without opening a source.

## Applying and Reading a Filter

View options MUST stage filter, order, and thumbnail-size changes until the Photographer chooses Apply. Closing or Cancel discards staged values; Apply creates one destination with the chosen source, order, and filter when those choices change.

A size-only change follows the existing presentation rules. Applying filter and order together opens one view, not successive views for each changed field.

**Reset filters** inside View options changes only the draft filter to the source's default. The default is All Selection States, with no Rating, kind, availability, or Capture Time restriction.

Reset MUST preserve draft order and thumbnail size, make no request, and leave the current result and address unchanged until Apply. Cancel after Reset filters retains the original committed filter.

A successfully resolved zero-match view with an active filter MUST offer **Adjust filters** and **Clear filters**. Adjust filters opens View options with the committed choices.

Clear filters immediately opens the same source with its default filter, preserving committed order and thumbnail size. It uses the existing applied-view history replacement and source-reopen rules; it MUST NOT push another history entry, switch source, or change Photo facts.

If opening the cleared view fails, the requested default-filter destination remains retryable; the old result MUST NOT appear as its successful content. Pending, failed, and expired reads MUST NOT be presented as zero matches.

The header MUST identify the active filter and matching Photo count through the [responsive summary](../experience/spec.md#grid-view). View options MUST keep that committed result count separate from complete-source progress and unapplied draft choices.

Editing a draft MUST NOT relabel the committed count as a preview of the draft's results. An unfiltered empty source uses its own recovery actions.

A valid zero result MUST be presented as no matches, not an empty Library.

The service MUST order the complete source, apply the defined conditions, and freeze the resulting Photo ID sequence before pagination. Grid windows, Photo View, Previous, Next, position numbers, and source return MUST use that sequence; the browser MUST NOT derive membership from loaded Grid windows.

Fact changes and newly matching Photos MUST NOT alter an open filtered sequence. Refresh, reopening, or an explicit source or filter change creates a new sequence and re-anchors by Photo identity where possible.

The Destination URL MUST preserve source, order, filter, and optional Photo identity. Refresh, direct open, Back, Forward, and Web-to-Agent handoff MUST retain those choices or explicitly identify unsupported choices in the receiving client.

A CLI query with conditions absent from Web MUST NOT produce a link that appears to preserve them. An Album made from query results MUST NOT imply that the original query conditions remain active in its URL.

## Working on Results

The Photographer MAY explicitly change Selection State, Rating, or Album membership for named Photos. Filtering itself MUST NOT mark, rate, add, remove, Restore, or clean up records.

Grid multi-selection and batch outcomes follow [Library Browsing and Selection](../spec.md#grid-multi-selection-and-batch-actions). A partial result MUST NOT be presented as complete.

## Rejected Review and Cleanup

A Rejected result MAY be corrected while its open sequence remains stable. Explicit **Remove Rejected Photos** review covers the complete current source and filtered result, including Photos outside loaded Grid windows.

[Remove Rejected Photos](../../photo-library/removal/rejected-cleanup/spec.md) owns review, eligibility, confirmation, stale-state handling, outcomes, and recovery. Filtering MUST NOT authorize a first-page or first-N deletion shortcut.

[Trash](../../photo-library/trash/spec.md) owns inspection and separately confirmed record cleanup. Slipstream MUST NOT delete Original Files.

## Agent and CLI Handoff

The CLI and Web share source meanings, fact meanings, Photo identities, and current-state checks. [CLI Reference](../../command-line/reference.md) owns syntax and result envelopes.

Queries are evaluated before pagination and return evaluation time and continuation state. The caller MUST materialize the Photo IDs it intends to change.

Mutations MUST use explicit IDs and observed decision or version evidence, not a live filter expression as an implicit write target. A lost response is an unknown outcome requiring inspection, not an automatic retry.

An Agent MUST report query conditions, Photo IDs, confirmed effects, conflicts, missing items, and unresolved work.

## Failure Behavior

The Photographer MUST be able to distinguish:

- an empty source from a source with no matches;
- an unavailable Original from a missing Photo;
- an expired Snapshot or cursor from a new empty result;
- a changed Photo from a confirmed mutation; and
- a transport failure from proof that a mutation did not run.

Invalid enum values, impossible Rating bounds, reversed time ranges, malformed Locations, duplicate IDs, and over-limit mutations MUST be rejected before admitting a Snapshot or write.

Refresh and retry actions MUST state whether they reopen the same destination or create a newly evaluated result.


## Product Boundaries

Filters MUST NOT provide:

- Smart Albums or Saved Search;
- arbitrary query DSL;
- full-text or keyword search;
- face or AI quality facets;
- edit-history predicates;
- all-facet count panels; or
- automatic actions.
