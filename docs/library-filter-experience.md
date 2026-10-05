# Source-Scoped Library Filters

Photographers need to find a useful subset of Photos and continue working on
that subset without losing the source, order, or Photo identity. A Rejected
Photo must be easy to review and remove safely, while a selected high-rated
Photo must remain easy to prepare for editing or export. A filter is therefore
a temporary view of one Library source, not a new Album, a deletion shortcut,
or a saved query object.

This document owns the product behavior shared by Web and Agent workflows.
[Library Browser Experience](library-browser-experience.md) owns responsive
presentation and browser history. [Library Browsing and
Selection](library-browsing-and-selection.md) owns source order, Photo actions,
progressive loading, and decision persistence. The server-side Snapshot and
window contract is owned by [Browse Snapshot](../design/library-browsing.md#browse-snapshot).
[Remove Rejected Photos](rejected-photo-cleanup.md) and [Composable Removal and
Restore](library-management-removal-and-restore.md) own the destructive and
recovery contracts.

## Filter Model

A committed filter view has three parts:

```text diagram
one source scope -> zero or more defined facts -> source order
```

The source is chosen before the facts are evaluated. Different fact
conditions combine with AND. A condition may offer multiple values only when
those values belong to the same defined discrete field. The product must not
accept an arbitrary Boolean expression, SQL fragment, or undocumented field.

Filtering must not change Selection State, Rating, Album membership, Photo
identity, Original Location, or Original File bytes. It only determines which
Photos are in the current view.

## Source Scope

The Photographer opens exactly one of these sources:

- **All Photos** contains every Photo in the published Library that is not in
  Trash.
- **Original Folder** contains Photos whose remembered Original Location is in
  that recursive Folder subtree. Folder membership is physical organization;
  it is not an Album.
- **Album** contains the Album's explicitly ordered members. Album membership
  is a work collection; it is not a Folder.

Folder and Album are alternative source scopes in one view. The product must
not silently pretend that choosing one also applies the other. A workflow
that needs both scopes must use an explicit, bounded set of Photo IDs or a
separate Album operation. Trash, Recovery, and Processing queues are separate
destinations, not ordinary Library filter sources.

## Defined Conditions

### Selection State

Selection State is the keep decision: `unflagged`, `picked`, or `rejected`.
The `all` value means that no Selection State restriction is applied. Web
View options must provide All, Unflagged, Picked, and Rejected. The CLI
uses the same values with `photos list --selection`.

Selection State is independent of Rating, Album membership, and Original
facts. A Photographer may change a decision while reviewing a filtered view.
The change applies to the named Photo and never implies removal, Rating
change, or Album membership.

### Rating

Rating is a zero-to-five-star Photo fact. A Photo with no assigned stars is
represented as Rating `0`; the product does not introduce a second missing
Rating state or a separate missing-value filter. Rating bounds are inclusive
integers from 0 through 5 in the CLI. Clearing Rating means zero under the
existing Photo decision contract; it does not change Selection State.

### Original Kind

`raw` and `jpeg` match the Original File belonging to the Photo. A RAW Photo
and a same-basename JPEG Photo are independent Photos. A kind filter must not
merge them, copy decisions between them, or use one Photo's Preview for the
other.

### Original Availability

`available=true|false` describes whether the remembered Original File is
available. It does not describe Preview readiness. An unavailable Original
may still have a Photo, Selection State, Rating, Album membership, and
recoverable Location facts. Recovery remains an explicit recovery workflow;
filtering does not relink, retire, or delete anything.

### Capture Time

Capture Time is the optional camera-recorded local date and time of the
Photo's own Original File. A range uses the CLI's local-time syntax, with an
inclusive lower bound and exclusive upper bound. Filesystem modification time
must not substitute for Capture Time. Photos without Capture Time remain
eligible when no time bound is supplied and do not match a supplied bound.

## Applying and Reading a Filter

View Options must stage changes until the Photographer chooses Apply. Closing
or Cancel discards staged values. Apply creates one destination with the
chosen source, order, and filter. Clear returns to the source's default
filter without changing the source.

The header must show the active non-default filter and the number of matching
Photos. View Options must keep that filtered result count separate from the
source progress counts for the complete source. A valid result of zero is a
no-match state, not an empty Library.

The service resolves the complete ordered source, applies the defined facts,
and freezes the resulting Photo ID sequence before pagination. Grid windows,
Photo View, Previous, Next, position numbers, and source-return behavior all
operate on that same sequence. The browser must not derive membership from
the currently loaded Grid window.

The destination URL must preserve source, order, filter, and optional Photo
identity. Refresh, direct open, Back, Forward, and a Web-to-Agent handoff
must either retain those choices or explicitly state which choices are not
supported by the receiving client. A CLI query with conditions absent from
Web must not produce a link that appears to preserve those conditions.

An open filtered sequence is stable. A Photo that changes fact values does not
silently enter or leave the open sequence, and a new matching Photo does not
appear in it. Refresh, reopening, or an explicit source/filter change creates
a new sequence and re-anchors by Photo identity where possible.

## Working on Results

Filtering is read-only, but the Photographer may continue with explicit
actions on named Photos:

- mark Selection State;
- set or clear Rating;
- add or remove Album membership; and
- review a Rejected result for reversible removal.

Normal Grid multi-selection remains bounded by the existing 100-Photo
contract. A batch action reports each requested Photo as confirmed, changed
elsewhere, or missing and never treats a partial result as complete. A filter
does not automatically mark, rate, add, remove, restore, or permanently delete
any Photo.

## Rejected Review and Cleanup

Rejected is the first complete end-to-end filter workflow:

1. Choose All Photos, an Original Folder, or an Album.
2. Apply the Rejected Selection State filter and inspect the result count,
   source progress, Preview, and Photo facts.
3. Correct an accidental rejection to Picked or Unflagged, or leave it
   Rejected. The open filtered sequence remains stable until refresh.
4. If cleanup is intended, start the explicit **Remove Rejected Photos** review
   from the current source and filtered sequence. The review represents the
   complete current result, including Photos outside loaded Grid windows.
5. Confirm **Remove from Library**. Only Photos that are still eligible under
   the reviewed evidence enter Trash. Changed, missing, or uncertain Photos
   remain visible in the outcome and are not called removed.
6. Undo or Restore returns confirmed removals to normal Library sources with
   their Photo identity and retained facts. Permanent Deletion remains a
   separate confirmation inside Trash.

The full review, operation, stale-state, retry, and Original File rules are
defined in [Remove Rejected Photos](rejected-photo-cleanup.md). This filter
does not authorize a first-page or first-N deletion shortcut.

## Agent and CLI Handoff

The CLI and Web use the same source names, fact meanings, Photo identities,
and current-state checks. [CLI Reference](cli-reference.md) owns the exact
syntax and result envelope. The query contract is:

```text literal
slipstream photos list [--album ALBUM_ID | --folder LOCATION]
  [--selection all|unflagged|picked|rejected]
  [--rating-min N] [--rating-max N] [--kind raw|jpeg]
  [--available true|false] [--captured-from LOCAL_TIME]
  [--captured-before LOCAL_TIME]
  [--order capture-time-asc|capture-time-desc|album-order] [--limit N]
```

Queries are evaluated before pagination and return an evaluated time and
continuation state. The caller must materialize the Photo IDs it intends to
change. Mutations accept explicit IDs and the observed decision/version
evidence; they must not accept a live filter expression as an implicit write
target. A lost response is an unknown outcome that requires inspection, not an
automatic retry.

An Agent may use a CLI query to inspect rejected, picked high-rated, RAW-only,
unavailable, or date-bounded Photos. It may then use explicit mark, Album,
Remove, or Restore operations. The Agent must report the query conditions,
Photo IDs, confirmed effects, conflicts, missing items, and unresolved work.

## Failure Behavior

The Photographer must be able to distinguish:

- an empty source from a source with no matches;
- a Photo whose Original is unavailable from a missing Photo;
- an expired Snapshot or cursor from a new empty result;
- a changed Photo from a confirmed mutation; and
- a transport failure from proof that a mutation did not run.

Invalid enum values, impossible Rating bounds, reversed time ranges, malformed
locations, duplicate IDs, and over-limit mutations are rejected before a
Snapshot or write is admitted. Refresh and retry actions must state whether
they reopen the same destination or create a new evaluated result.

## Examples

The Photographer filters Unflagged Photos in an Album, reviews them in Photo
View, and marks the usable ones Picked. The source and filtered sequence do
not jump after each decision; Refresh shows the new Unflagged result.

The Photographer filters Rejected Photos in a Folder, reviews all 240 matches,
removes them from the Library, and sees 240 confirmed Trash entries. A stale
Photo that became Picked is reported separately and remains in the Library.

An Agent queries `picked` Photos with Rating 4 or 5, groups explicit IDs into
bounded Album operations, and opens the resulting Album in Web. It does not
claim that the original CLI Rating condition is still active in the Album URL.

The Photographer filters RAW Photos with unavailable Originals during a
recovery task. The result keeps Selection State and Rating visible and links
to recovery; it does not merge a same-basename JPEG or silently rebind the
RAW Photo.

The Photographer queries a local Capture Time range and receives no matches.
The Library remains available, and clearing the range returns to the same
source's normal view.

## Product Boundaries

The current filter capability does not include Smart Albums, Saved Search,
arbitrary query DSL, full-text or keyword search, face or AI quality facets,
edit-history predicates, all-facet count panels, or automatic actions.
Adding one of those requires a new product decision and an observable task;
it must not be smuggled into the source-scoped filter contract.
