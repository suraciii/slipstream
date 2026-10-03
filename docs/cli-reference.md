# CLI Reference

This document is the authoritative command and output language for
[Command-Line Use](command-line.md). It defines the target CLI contract;
release availability is governed separately. Symbols in uppercase below stand
for caller-supplied values, not literal IDs or generated defaults.

## Invocation

```text literal
slipstream [--server URL] [--token-file FILE] [--output json|text] [--timeout SECONDS] COMMAND
slipstream --help
slipstream --version
slipstream COMMAND --help
```

Global options precede the command. `--output` defaults to `json`, independently
of terminal detection. Help and version output are plain text and make no
network request. Unknown options, duplicate options, extra positional arguments,
and invalid combinations are errors. Flags must not accept abbreviations.

The service URL resolves from `--server`, then `SLIPSTREAM_SERVER_URL`, then
an error for network commands when neither is supplied. A supplied empty or
invalid value is an error, not a fallback. A URL must be an HTTP or HTTPS origin
with no credentials, path other than `/`, query, or fragment. HTTPS uses normal
certificate validation; there is no certificate-verification bypass. An HTTP
origin prints one unencrypted-connection warning to stderr before any request.
The CLI must not follow HTTP redirects. There is no profile file, automatic
discovery, or login command.

`--timeout` is an integer from 1 through 300 seconds and defaults to 30. It
bounds the whole command after argument parsing, including preview transfer.
Expiry uses the result rules below. It does not cancel an admitted mutation.

`--input FILE` reads one UTF-8 JSON document; `--input -` reads stdin. The input
must fit 64 KiB. An oversized document is refused as soon as a read crosses the
bound; `limit_exceeded` reports `actual` as `limit + 1` and does not read the
rest of the input. Unknown keys, duplicate JSON object keys, duplicate Photo IDs,
trailing content, and an empty mutation list are invalid. The entire document
must validate before a write. A mutation contains at most 100 Photo IDs; the
CLI must not split it automatically. Local input-file failures perform no write.

## Instance Credentials

A generated Access Token is loaded from a private file and sent only in the
Authorization header to the selected HTTP or HTTPS origin. The file must
contain exactly one base64url token, optionally followed by one line ending.
The CLI must reject missing, unreadable, empty, or malformed credentials before
network mutation and must never echo file contents. Help and version require no
credential. Credentials must not be accepted as plaintext command arguments,
URL components, or mutation input.
No request redirect may forward the credential.

A confirmed authentication rejection means the request was not admitted; the
CLI must not retry it automatically. Transport loss after a write was sent
retains the existing unknown-outcome rules. The token file resolves from `--token-file`, then
`SLIPSTREAM_ACCESS_TOKEN_FILE`. Omission, an empty path, or duplicate options
is `invalid_input` before any network request. File input is independent of
mutation stdin. The file must be a regular nonsymlink file owned by the effective
user with no group or other permission bits; validate and read the same opened
file. Refuse files larger than 45 bytes. Missing or unreadable files use
`local_io_failed` with operation `read-credential` and `fileCommitted: false`; unsafe permissions or invalid content use `invalid_input`.
No credential contents may appear in either error.

401 maps to `authentication_required`; 403 to `access_denied`; 429 to
`server_busy` with parsed Retry-After when available. A confirmed boundary
503 `access_unavailable` or `access_unconfigured` maps to `server_busy`, effect
`none`, and `retryAfterSeconds: null`; it is not a storage rollback or evidence
that the requested operation ran. Do not infer this from an arbitrary 503 body.
Both new error codes have
`details: { "operation": OPERATION }`, exit 6, and effect `none` when rejection
is confirmed before admission. An unexpected or invalid response to a possibly
admitted write must retain `outcome_unknown`. A token does not bypass contract
version negotiation. HTTP and HTTPS origins use the same authentication and
result mappings.

## Service and Discovery

```text literal
slipstream status
slipstream library check
slipstream folders list [--parent LOCATION] [--limit N]
slipstream folders list --cursor CURSOR
slipstream albums list [--name NAME | --photo PHOTO_ID] [--limit N]
slipstream albums list --cursor CURSOR
slipstream albums get ALBUM_ID
```

`status` returns client/server versions, supported CLI contract version,
published availability, Photo count, and the existing scan facts. A reachable
service with an initializing or failed scan is a successful status query; the
scan state remains explicit. An incompatible service is an error. The scan
facts are the Library axis only; Processing Module availability and a Photo's
input support are separate axes that a recovering or saturated scan does not change.

Compatibility is checked before any operational request. Use a client and service
built from the same candidate source revision. Contract version 1 alone does not
prove that a service implements all required capability fields. An invalid or
incomplete capability response is `incompatible_server`, exit 6, effect `none`.
The diagnostic preserves any valid advertised versions and identifies the
missing or invalid field. Install a matching candidate; do not bypass negotiation.

`library check` waits within the command timeout for the existing scan cycle's
terminal result. A failed scan is a command error with its last confirmed status.
A timeout tells the caller to query `status`; it does not claim cancellation.
The timeout outcome is unknown and non-terminal: the admitted, server-owned
scan keeps running, later `status` answers preserve the latest scan phase and
counts, and reissuing `library check` joins the running cycle instead of
starting a second physical scan.

Folder Locations are Library-relative and recursive for Photo queries. The
empty parent denotes the Library Folder; omission uses that parent. Folder
listing returns direct children, recursive Photo counts, whether each child
has descendant Folders, and the Published Library reference. An expired Folder
publication requires a fresh listing, as in the existing Web contract.

Album name lookup uses the existing exact ASCII-case-folded comparison, not
substring search. `--photo` lists only Albums containing that Photo. Every Album
summary contains ID, name, member count, saved-position availability, Album
version, and Web URL. `get` never downloads all members.

List commands default to 50 items and accept limits from 1 through 60. Every
list returns `items`, `total`, and `nextCursor` (a string or `null`). Album lists
freeze their ordered IDs on the first page; summaries are current at page read.
A concurrently deleted Album is an item with its ID and `state: "missing"`.
Name/Photo-filter membership is evaluated at query creation, not on each page.
Folder windows use their existing publication-bound continuation instead.

A continuation accepts only `--cursor` and global options. Its cursor carries
the original query and page size. Callers must not combine it with new filters.
Every cursor is opaque and bound to its endpoint kind.

Album and Photo cursors refer to retained ordered-ID collections in one server
process. They expire on restart, after 15 minutes idle, or under bounded
least-recently-used eviction. Their `expiresAt` is the current idle deadline;
later activity may extend it, while eviction may invalidate the cursor earlier.
If one valid Album or Photo query would exceed the 1,000,000 retained-ID maximum,
the service returns `server_busy` with `effect: "none"`, the corresponding
`albums-list` or `photos-list` operation, and `retryAfterSeconds: null`; it does
not publish a partial query or cursor. The caller must narrow the source or
filters rather than assume that waiting or retrying will make the result fit.

A Folder cursor instead carries the existing publication, parent, range, and
page size without a retained query collection. It has `expiresAt: null`, which
means that it has no time deadline. It expires whenever its bound Published
Library is no longer current, including publication replacement after a server
restart. It does not participate in idle expiry or query-collection eviction.
Every list response includes `evaluatedAt` and `expiresAt` as defined by its
cursor kind.

## Photo Queries

```text literal
slipstream photos list [--album ALBUM_ID | --folder LOCATION]
  [--selection all|undecided|selected|rejected]
  [--rating-min N] [--rating-max N] [--kind raw|jpeg]
  [--available true|false] [--captured-from LOCAL_TIME]
  [--captured-before LOCAL_TIME]
  [--order capture-time-asc|capture-time-desc|album-order] [--limit N]
slipstream photos list --cursor CURSOR
slipstream photos get PHOTO_ID
```

Multiline command shapes above describe one invocation. All Photos is the
default source. Omitted filters impose no restriction. Rating bounds are
inclusive integers from 0 through 5; a lower bound above an upper bound fails.
Availability means Original File availability, not Preview readiness.

`LOCAL_TIME` has exact form `YYYY-MM-DDTHH:MM:SS` and must be a valid
camera-local date and time with no offset or timezone suffix. A supplied lower
bound must precede a supplied upper bound. Existing subsecond facts participate
in comparison against these second-aligned boundaries.

Order defaults to `album-order` for an Album and `capture-time-asc` otherwise.
`album-order` is invalid for other sources. Capture ordering and ties follow
[Source Order](library-browsing-and-selection.md#source-order) and
[Source Ordering Selection](library-browsing-and-selection.md#source-ordering-selection).
Filters never rewrite Album order. The list pagination rules above apply.

Each ordinary Photo item uses the `PhotoItem` shape defined under
[Result Schemas](#result-schemas), including the complete `preview` object and
nullable `removedAtMs`. An active Library Photo has `removedAtMs: null`.
`get` adds the bounded `metadata` object; use `albums list --photo` for
membership. Absolute server paths never appear. Missing metadata fields are
`null`, with their inspection state, rather than guessed.

A Photo removed after query creation occupies its original result position as
`{"id":"…","state":"missing"}`. An unavailable Original with a retained
Photo still returns the Photo's ordinary facts. `total` is the original match
count and includes missing placeholders. It is not the count of currently
matching Photo facts. `get` for a removed Photo is `not_found`.

## Reversible Photo Removal and Restore

The explicit removal and Restore commands operate on a caller-selected set of
Photo IDs. They do not infer identity from a path, filename, Album, live query,
or current Selection State. Query the Photos first and preserve the returned
`selectionState`, `decisionVersion`, and `removedAtMs` as removal evidence.

```text literal
slipstream photos remove OPERATION_ID --input FILE
slipstream photos removal-operation OPERATION_ID
slipstream photos restore OPERATION_ID --input FILE
slipstream photos restore-operation OPERATION_ID
```

`OPERATION_ID` is a nonempty caller-generated identifier. A removal input has
exactly one `photos` array. Each item has exactly `photoId`,
`selectionState: "rejected"`, nonempty `decisionVersion`, and
`removedAtMs`, which is either `null` or a nonnegative integer:

```json
{
  "photos": [
    {
      "photoId": "00000000-0000-4000-8000-000000000001",
      "selectionState": "rejected",
      "decisionVersion": "opaque-version-from-photo-query",
      "removedAtMs": null
    }
  ]
}
```

The target list must be nonempty, distinct, and no larger than the advertised
`removalPhotoIdsMaximum` capability. The CLI validates the complete document
before network access. The service validates all request-wide constraints
before admitting the attempt. A stale decision or removal marker affects only
that target; valid siblings may still be removed.

Removal returns one result per requested Photo in request order. Outcomes are
`removed`, `changed-elsewhere`, `unavailable`, or `already-removed`.
`removed` includes a nonnegative `removedAtMs`; all other outcomes include
`removedAtMs: null`. Counts contain `removed`, `changedElsewhere`, `missing`,
and `alreadyRemoved`, and equal the result partition. A successful removal
does not change the Original File, Album membership, rating, or decision.

`photos removal-operation` is read-only and returns the durable result for the
specified operation. Repeating the same operation ID with identical intent
replays that result. Reusing it with different targets or evidence is a
`conflict`. A missing receipt, incomplete response, or invalid response is
`outcome_unknown`; inspect the original operation before submitting a
replacement.

Restore has a separate operation ID and uses the removal marker as its
compare-and-set identity. Its input has exactly one `photos` array containing
distinct IDs and nonnegative markers:

```json
{
  "photos": [
    {
      "photoId": "00000000-0000-4000-8000-000000000001",
      "removedAtMs": 1735689600000
    }
  ]
}
```

Restore results use `restored`, `already-active`, `changed-elsewhere`, or
`unavailable`. Counts contain `restored`, `alreadyActive`,
`changedElsewhere`, and `missing`. A Restore clears only the matching
removal marker. It does not authorize or perform permanent deletion.
`photos restore-operation` is read-only and follows the same receipt replay and
`outcome_unknown` rules. Permanent deletion remains a separate reviewed Trash
operation.

When at least one target changed and another target was conflicting or
unavailable, the CLI returns `partial_result` with the complete result in
`data`. An all-conflict result uses `conflict`; an all-unavailable result uses
`not_found`. Inspect the original operation before retrying either command.

## Preview Download

```text literal
slipstream photos preview PHOTO_ID --file PATH [--size thumbnail|review]
```

Size defaults to `review`. The command obtains the current supported derivative;
it may wait for generation within the command timeout. It returns `photoId`,
`path`, `source`, `sourceRevision`, `width`, `height`, `detailLimited`, and
`webUrl`. `source` uses `jpeg-original` or `raw-embedded-jpeg`. These facts must
refer to the actual downloaded image. No arbitrary remote URL is accepted.

PATH is local to the CLI host, must be valid UTF-8, must have an existing parent
directory, and must not exist. A non-UTF-8 value is `invalid_input` and must fail
before network access or temporary-file creation, without lossy substitution.
The transfer must not exceed 64 MiB, which is the existing
[Preview output JPEG bound](../design/preview-pipeline.md#native-library-boundary);
`photos preview --help` must report this limit. The command publishes a complete
JPEG without replacement. Final no-replace publication is the local commit
point. Cancellation or failure before that point removes only this invocation's
temporary file. Failure after that point preserves the final JPEG. Existing
files and symbolic links remain unchanged. No overwrite flag, automatic
filename, binary stdout, or stale-image fallback is supported.

A normal success result has `fileCommitted: true`. If publication commits but
result serialization or stdout completion fails, the command must not report
`effect: "none"`. When a valid JSON error can still be delivered, it retains the
complete Preview result in `data`, uses `local_io_failed` with
`effect: "partial"`, and exits `6`. Otherwise the surviving process writes a
best-effort stderr diagnostic that states that the Preview file was already
published and renders PATH as a JSON string literal so control characters,
including newlines, are escaped. That diagnostic is for recovery by a person,
not a second machine protocol. A handled interruption after publication
preserves the file and exits `130` with the same best-effort distinction.
Process death cannot promise either diagnostic.

After any post-publication reporting failure, the caller must inspect the named
path. An automatic retry cannot overwrite it and must not choose a different
path silently.

## Standard Photo Metadata

```text literal
slipstream photos metadata PHOTO_ID
slipstream photos metadata-save PHOTO_ID --input FILE
```

`metadata` reads one Photo's standard metadata without changing any Library
decision or file. The result carries the association status, every supported
field with its state, provenance, and typed value, the read-only capture
facts, the Library Rating, the evidence a checked Save requires, and
`saveAvailable` with an actionable reason when saving is unavailable. The
effective field value uses the Sidecar when present, then embedded XMP, then
the IPTC IIM counterpart; the underlying source values stay inspectable. An
empty Sidecar value suppresses fallback. `FILE` for `metadata-save` is a
UTF-8 JSON document; `-` reads it from stdin.

Read includes Removed Photos whose Original is still available. Save refuses
a Removed Photo. Sidecar evidence is `absent`, `present`, or `unavailable`;
`unavailable` includes a reason and cannot authorize Save. A readable XMP
document with unsupported preservation constructs still exposes its supported
fields, but Save refuses to rewrite it.

The authoritative shapes are the shared vectors under
`compatibility/metadata/`. A save document contains exactly `evidence` from a
prior Read and a nonempty `changes` object with one patch per field:

- `set` with the typed value. For language-alternative fields the value is a
  language map.
- `setLanguages` with a `languages` map whose null entries remove exactly
  that alternative; unmentioned languages survive.
- `clear` for a valid empty value and `remove` to delete the Sidecar
  property.

The CLI validates the document shape and that the evidence names the same
Photo before any network access. The service owns field semantics: an
unsupported field, an invalid patch, or stale evidence is refused before any
change. Metadata errors use the shared CLI exit-code table:
`invalid_input` and `unsupported_field` exit `2`;
`photo_missing`, `original_unavailable`, `association_unresolved`, and
`photo_removed` exit `3`; `evidence_stale` and `metadata_malformed` exit `4`;
`save_unavailable` and `permission` exit `5`; `resource_limit` exits `6`;
`storage_failure` and `outcome_unknown` exit `7`.

`metadata-save` writes only the associated XMP Sidecar. A response lost after
the request was admitted is `outcome_unknown`: the caller must re-read and
decide, never retry the same save blindly. A success result carries the
affected fields, the verified values, and fresh evidence for the next
operation.

## Photo Decisions

```text literal
slipstream photos set PHOTO_ID --selection undecided|selected|rejected --if-version VERSION
slipstream photos set PHOTO_ID --rating N --if-version VERSION
slipstream photos set --input FILE
```

The single-Photo forms are shorthand for a one-item batch. Selection and Rating
cannot be changed in the same command. Batch input has exactly `field`, `value`,
and `photos`; each item has exactly `photoId` and `ifVersion`.

```json
{
  "field": "selectionState",
  "value": "selected",
  "photos": [
    {
      "photoId": "00000000-0000-4000-8000-000000000001",
      "ifVersion": "opaque-version-from-photo-query"
    }
  ]
}
```

`field` is `selectionState` or `rating`, and `value` must have the corresponding
type and range. Every Photo result uses `outcome` of `changed`, `unchanged`,
`conflict`, or `missing`. Changed and unchanged results include current decisions
and their version; changed results also include prior decisions. Conflicts
include current decisions/version; missing results contain the requested ID.
The `results` array retains request order. A `counts` object reports each
outcome count. A nonmatching version conflicts before no-op detection.

A batch containing only changed or unchanged results produces `status: "ok"`,
`error: null`, and exit `0`. A mixed batch with at least one changed or unchanged
result and at least one conflict or missing result produces `status: "partial"`,
`partial_result` with `effect: "partial"`, and exit `5`. This includes a mixed
batch whose only successful outcomes are unchanged. A batch containing only
conflict or missing results produces `status: "error"` with `effect: "none"`;
it uses `conflict` and exit `4` when any item conflicts, otherwise `not_found`
and exit `3`. The complete result array remains in `data` for mixed and
all-unsuccessful batches. A storage failure commits no items and returns an
error, not a fabricated partition of per-Photo outcomes.

## Reviewed Location Recovery

```text literal
slipstream recovery unavailable [--limit N]
slipstream recovery unavailable --cursor CURSOR
slipstream recovery propose --old-prefix PREFIX --new-prefix PREFIX [--limit N]
slipstream recovery propose --cursor CURSOR
slipstream recovery propose --original-id ORIGINAL_ID --new-location LOCATION
slipstream recovery apply --input FILE
```

Recovery restores the association between an unavailable Photo and a moved or
renamed Original File. It never moves, renames, uploads, or deletes file bytes,
never restores a Photo from Trash, and never writes a Sidecar. A Photo in Trash
uses `photos restore`.

`recovery unavailable` opens one bounded review of the active unavailable
Photos. Its data is a list result whose items are `RecoveryItem` records. An
empty result means that no active unavailable Photo needs recovery in the
evaluated scope; it is not evidence that any missing file was restored.

`recovery propose` is read-only. The Folder-prefix form maps every unavailable
Original whose remembered Location is under `--old-prefix` onto `--new-prefix`
with the same relative suffix. The single form evaluates exactly one mapping for
`--original-id`. Both return a list result whose items are `RecoveryMapping`
records; the single form has `total: 1` and no continuation. A prefix scope
larger than the advertised mapping bound is refused as `recovery_scope_exceeded`
before any continuation is issued; no truncated proposal is returned.

`PREFIX` is a Library-relative Folder Location with no leading or trailing
separator; the empty value addresses the Library Folder. `--new-location` names
one Library-relative Original Location including the filename.

A continuation accepts only `--cursor` and global options. Recovery
continuations expire after the advertised idle period and then fail as
`cursor_expired`; the caller opens a fresh review instead of assuming the
retained set.

`recovery apply` takes exactly this JSON object. Every mapping is an explicit
confirmation of one reviewed correspondence; replace the example values with
records returned by `recovery propose`.

```json
{
  "mappings": [
    {
      "originalId": "00000000-0000-4000-8000-000000000001",
      "newLocation": "2024/travel/renamed.ARW",
      "mappingId": "reviewed-mapping-identity",
      "confirmUnverifiedContent": true,
      "retirePhotoId": "00000000-0000-4000-8000-000000000002"
    }
  ]
}
```

Each mapping requires `originalId`, `newLocation`, and the `mappingId` of the
reviewed proposal. `confirmUnverifiedContent` is required exactly when the
reviewed mapping has `verified: false`, and the caller thereby acknowledges that
the remembered content cannot be verified. `retirePhotoId` is required exactly
when the reviewed mapping identifies a `retire` candidate, and it must name that
Photo. Unknown keys, an empty list, more than the advertised apply bound,
duplicate Original IDs, or duplicate destinations are invalid before any write.

The service recomputes every mapping from current state. If a recomputed mapping
no longer matches its reviewed identity, a required confirmation is absent, the
destination revision or occupant changed, an occupant gained state or a Trash
removal, or the Original is no longer an active unavailable Photo, the complete
batch is refused as `recovery_conflict` with one reason per mapping and no
association change. Separate invocations are separate commits; a later refusal
never rolls back an earlier committed batch.

Apply data contains count `appliedMappings`, count `refusedMappings`, count
`unavailablePhotos` for the remaining Library-wide unavailable Photos, and
`mappings` in request order. Each applied mapping contains `originalId`,
`photoId`, `fromLocation`, `toLocation`, `webUrl`, and nullable `retired`. A
successful mapping is not proof that missing file bytes were recreated.

Recovery reasons are `colliding`, `content-mismatch`, `content-unconfirmed`,
`destination-in-use`, `destination-removed`, `kind-mismatch`, `missing`,
`occupied`, `retire-mismatch`, `retire-unconfirmed`, `reviewed-stale`, `stale`,
and `unreadable`. A destination revision change is reported as `reviewed-stale`.
`occupied` reports a destination that another Original claimed after the
reviewed evaluation and before the commit.

A lost or unusable apply response is `outcome_unknown`, exit 7, effect
`unknown`, with the submitted mapping identities in `details`. Inspect the same
Photo identities with `photos get` before deciding what remains; current
Location and availability do not prove which attempt changed them, and no
command retries automatically.

## Album Mutations

```text literal
slipstream albums create --name NAME
slipstream albums rename ALBUM_ID --name NAME --if-version VERSION
slipstream albums delete ALBUM_ID --if-version VERSION
slipstream albums add ALBUM_ID --input FILE --if-version VERSION
slipstream albums remove ALBUM_ID --input FILE --if-version VERSION
slipstream albums reorder ALBUM_ID --input FILE --if-version VERSION
```

Album names are nonempty strings of at most 120 characters under the existing
Album naming contract. The input for add, remove, and reorder has one key,
`photoIds`, containing an ordered array of 1 through 100 distinct Photo IDs. An
empty Album needs no reorder command. Reorder must name the complete current
membership and refuses
Albums over that bound without any change. A caller may append or remove
successive explicit batches in a larger Album using each confirmed new version.

```json
{
  "photoIds": [
    "00000000-0000-4000-8000-000000000001",
    "00000000-0000-4000-8000-000000000002"
  ]
}
```

Every Album mutation uses its exact data shape under
[Result Schemas](#result-schemas). Create returns the new summary. Rename
reports whether the name changed. Add and remove partition every requested ID
in request order. Reorder returns the complete accepted order and whether that
order changed. Removal reports the resulting saved Photo ID or `null`. Delete
provides no dead Album URL.

All Album mutations are atomic. A missing Photo in the Library, stale version,
invalid complete order, or input-limit failure commits nothing. After local
syntax and request-bound validation, an existing-Album mutation selects the
first applicable failure in this order: missing target Album, stale supplied
Album version, first missing requested Photo in request order, complete-membership
mismatch for reorder, then duplicate-name conflict. The version guard is checked
before no-op classification. Create has no target or version guard and reports a
name conflict when applicable. A name conflict includes the existing Album ID.
No `--force`, implicit name reuse, automatic retry, or generic `undo` command
exists.

## Photo Development

Camera Preview (`photos preview`) remains independent of saved editing intent.
The shared Processing Module surface uses this grammar:

```text literal
slipstream processing modules
slipstream processing artifact ARTIFACT_ID
slipstream processing artifact-download ARTIFACT_ID --file PATH
slipstream photos processing-recipe get PHOTO_ID
slipstream photos processing-recipe save PHOTO_ID --input FILE
slipstream photos processing-recipe rebind PHOTO_ID --input FILE
slipstream photos processing-preview PHOTO_ID --step STEP_ID --file PATH
slipstream photos processing-export PHOTO_ID --input FILE
slipstream photos processing-export-list PHOTO_ID
slipstream photos processing-export-status PHOTO_ID REQUEST_ID
slipstream photos processing-export-cancel PHOTO_ID REQUEST_ID
slipstream photos processing-export-retry PHOTO_ID REQUEST_ID --input FILE
slipstream photos historical-export-download EXPORT_ID --file PATH
slipstream photos proxy get PHOTO_ID
slipstream photos proxy create PHOTO_ID --input FILE
slipstream photos proxy remove PHOTO_ID
```

`processing modules` reads `GET /api/processing/modules`. It preserves each
peer module's parameter schema tree, finite limits, versions, availability,
and refusal reasons. Discovery does not authorize arbitrary engine controls.
The service separately checks input, parameters, bundle, and resources before
execution. [Processing Modules](../design/processing-modules.md) owns the
module boundary. Output format, precision, color, geometry, and encoding are
ordinary module parameters. Each execution selects one step; composition
requires explicitly selecting a published immutable artifact as another
step's input. Commands perform no implicit conversion or chaining.

`processing-recipe get` returns `photoId`, the observed `sourceRevision`
(empty when none is published), nullable `recipe`, and `webUrl`. A saved
recipe contains `photoId`, `revision`, its bound `sourceRevision`, nullable
`currentStepId`, and `steps`. Each step contains `stepId`, `module`, `input`,
and `parameters`. An Original input contains `kind: "original"`, `photoId`,
and `sourceRevision`. An Artifact input contains `kind: "artifact"`,
`artifactId`, and `contract` with `format`, `precision`, `colorSpace`,
`transfer`, `encoding`, and `geometry` (`width`, `height`). Parameters contain
`schemaVersion` and an opaque object `tree`. Reads preserve retained intent,
including unsupported controls and bindings to an older source revision.
The observed source revision can differ from the saved binding; processing
requires explicit rebind before using a changed Original.

`processing-recipe save` requires every key in this document, including nulls:

```json
{
  "requestId": "composable-001",
  "expectedRecipeRevision": null,
  "expectedSourceRevision": "observed-source-revision",
  "currentStepId": null,
  "steps": []
}
```

The nullable recipe guard explicitly requires absence when null. Steps may be
empty; otherwise `currentStepId` selects exactly one of the unique step IDs.
Step and module IDs are at most 64 bytes. Artifact IDs, Photo IDs, recipe
revisions, and contract names are at most 128 bytes. A recipe has at most
64 steps, parameter trees have depth at most 32, and geometry edges are
1 through 65536. Source revisions have a separate bound of 16384 bytes;
they are opaque UTF-8 strings that may contain NUL. Copy them verbatim in
JSON with `\u0000` escapes. Request IDs use 1 through 128 ASCII letters,
digits, `.`, `_`, or `-`. Every document remains subject to the 64 KiB input
bound. The CLI preserves the complete module trees and sends the prepared
body once, without reading replacement guards. A wire-valid unsupported
setting remains saved intent; execution availability does not rewrite it.

`processing-recipe rebind` requires exactly this document:

```json
{
  "requestId": "rebind-001",
  "expectedRecipeRevision": "observed-recipe-revision",
  "newSourceRevision": "newly-observed-source-revision"
}
```

Rebind creates a new recipe revision, preserves step identity, selection,
module, and exact parameter trees, and changes only Original source bindings.
Artifact inputs retain their immutable identities. Save and rebind report
`photoId`, `requestId`, `outcome` (`saved`, `replayed`, or `unchanged`),
`recipe`, `recipeVersion`, `sourceRevision`, and `webUrl`. `saved` uses 201;
`replayed` and `unchanged` use 200. Lost or unusable write responses are
`outcome_unknown`, exit 7. Reconcile the identical request before dependent
work; a replay confirms retained admission, not the absence of later edits.

`processing-preview` requests only the selected current step's bounded
Preview. It performs one request without polling. Accepted work reports
`state: "queued"` or `"running"`, `fileCommitted: false`, and the selected identity; invoke
the read again to reconcile. A ready result includes `photoId`, `stepId`,
`state: "ready"`, `sourceRevision`, `recipeRevision`, `contentType`, `width`,
`height`, `byteLength`, `sha256`, `path`, `webUrl`, and
`fileCommitted: true`. The CLI validates PNG identity headers, geometry,
length, digest, and image structure before publishing without replacement.
A refusal publishes no file. A Preview never becomes a full-resolution
handoff, selects another module, or substitutes Camera Preview.
The ready result also exposes `comparison: "current"`, `inputSha256`, and
`inputByteLength` from validated input-evidence headers. A baseline rendition
cannot satisfy an ordinary current Preview request.
An explicit `outcome_unknown` response retains exit 7 and effect `unknown`,
with the Photo identity and `photos-processing-preview` operation. The CLI
publishes no file and performs no automatic retry.

`processing-export` requires exactly the following guarded body:

```json
{
  "requestId": "export-001",
  "stepId": "develop-1",
  "expectedRecipeRevision": "observed-recipe-revision",
  "expectedSourceRevision": "observed-source-revision"
}
```

Admission captures the selected saved step's input, complete parameters,
module/schema, bundle, and intended output. Accepted initial submissions and
captured retries return 202 promptly with `outcome: "accepted"` or
`"replayed"` and the durable `receipt`. The CLI adds `photoId`, `requestId`,
`stepId`, current `state`, and `webUrl`. Work continues under service
ownership after the command exits. A receipt may already be terminal when
read. A successful terminal replay uses the retained `{artifact,replayed}`
wire response and reports the artifact with `outcome: "replayed"`.
No command automatically retries a lost admission or replaces its guards.

`processing-export-list` returns `photoId`, `exports`, `artifacts`,
`historicalExports`, and `webUrl`. Each array is durable, newest first, and
independently bounded to 64 records. This view bound does not shorten retained
data's disclosed lifetime. All retained records remain readable after restart
or when processing is unavailable.
`processing-export-status` reads the work record named by Photo and request
identity. Each record retains `requestId`, `photoId`, `stepId`, `module`,
`recipeRevision`, `sourceRevision`, `adapterSchemaVersion`, `parameters`,
captured `input` binding, `bundleId`, `acceptedAt`, nullable `attempt`, and `state`
(`accepted`, `executing`, `succeeded`, `failed`, or `cancelled`). Terminal
records include `terminalAt`, applicable `artifactId` or `failureReason`,
and disclosed `retainUntil`; inapplicable terminal fields are null.
`processing-export-cancel` asks the service to settle cancellation; the first
terminal decision wins. Repeating cancellation never reruns an engine.

`processing-export-retry PHOTO_ID REQUEST_ID` requires exactly:

```json
{ "requestId": "new-export-attempt-002" }
```

It creates a new attempt from the retained failed or cancelled request's
captured snapshot. It never reads today's recipe to reconstruct that intent.
Admission revalidates captured source/input, bundle, and resource guards.
Inspect the new request ID to reconcile its outcome. Identical retained
requests replay before inspecting current recipe or module availability;
the same identity with a different body conflicts.

`processing artifact` returns a published artifact's complete immutable
provenance and `webUrl`: `artifactId`, `photoId`, `stepId`, `module`,
`adapterSchemaVersion`, `parameters`, verified `input`, `outputContract`,
`bundleId`, `sha256`, `byteLength`, `filename`, `publishedAt`, and `expiresAt`.
Publication and expiry use RFC3339 UTC strings; work lifecycle times are integers.
The filename derives from module, step, and immutable artifact identity with
`.tif` or `.jpg`. `artifact-download` validates the record against transfer
headers, including filename, length, and recomputed SHA-256, before publishing
to the caller's `PATH`. It reports `filename`, expiry, byte evidence,
provenance, `path`, and `fileCommitted: true`. Existing files and symbolic
links are never replaced; mismatched or partial transfers publish nothing.

`historicalExports` preserves existing read-only image Export records separately
from Processing Artifacts. Each record contains `exportId`, `photoId`, `state`,
historical `target`, captured `recipeVersion` and `sourceRevision`, `createdAt`,
nullable `settledAt`, `bundleId`, nullable `terminalOutcome`, `failureReason`,
`receiptExpiresAt`, and `artifact`. These records do not imply complete
composable provenance. A retained artifact preserves its `exportId`, `target`,
`stage`, `contentType`, `filename`, `orientation`, `sampleFormat`, `colorSpace`,
`iccEmbedded`, `width`, `height`, `profileIdentity`, `byteLength`, `sha256`,
and `expiresAt` exactly as retained.

`historical-export-download` reads that existing record and downloads its retained
bytes through `GET /api/exports/{id}/artifact`. It validates every historical
artifact header, length, and recomputed SHA-256 before publishing without
replacement. It reports `exportId`, `photoId`, `historical: true`, complete
`artifact` metadata, `path`, `webUrl`, and `fileCommitted: true`. Failed or
expired transfers publish no file. The command does not submit, retry, or
reinterpret a historical Export as a Processing Artifact.

`photos proxy get` reports `state` (`absent`, `building`, `current`, or
`stale`), nullable `proxy`, and bounded `failure`. Create requires exactly:

```json
{ "expectedSourceRevision": "observed-source-revision" }
```

Create sends the observed revision once. `building` confirms accepted work;
read again to reconcile it. Remove is idempotent. A Development Proxy remains
a Preview stand-in and cannot authorize an Original-backed Export while its
Original is unavailable. Explicit compatible retained artifact inputs remain
subject to their own lease, expiry, and admission checks.

Development errors preserve structured codes and details. `invalid_settings`,
`invalid_recipe`, and `incompatible_input` exit 2; `unknown_photo`, `unknown_step`,
`unknown_module`, `unknown_request`, `unknown_artifact`, and `missing_recipe`
exit 3; `recipe_conflict`, `source_changed`, `requires_rebind`,
`request_conflict`, `step_not_current`, `original_required`, and `export_terminal` exit 4;
expired receipts/artifacts, unavailable processing/source/resources, and
`retained_output_full` exit 6. Confirmed refusals have effect `none`;
unusable responses to possibly admitted writes retain effect `unknown`.
No command changes decisions or Albums, silently rebinds, enables an
unavailable module, or submits dependent work after an uncertain save.

## Output Envelope

In JSON mode, every operational command emits one JSON document followed by a
newline on stdout. Diagnostic text and progress use stderr. In text mode, every
operational command emits one human-readable result to stdout. Text mode has no
parsing contract, but it uses the same command effects and exit codes and must
identify partial or unknown outcomes. Help and version output remain plain text.

```json
{
  "schemaVersion": 1,
  "status": "ok",
  "data": {
    "items": [],
    "total": 0,
    "nextCursor": null,
    "evaluatedAt": "2026-01-01T12:00:00Z",
    "expiresAt": null
  },
  "error": null
}
```

Envelope keys are always present. `status` is `ok`, `partial`, or `error`.
`data` has the command result shape below or is `null`. `error` is `null` when
status is `ok`. Otherwise it contains required `code`, `message`, `effect`, and
`details` keys. `effect` is `none` when no requested effect committed and no
requested item completed successfully. It is `partial` when at least one
requested effect committed or requested item completed successfully but the
command did not complete normally. A changed or unchanged Photo result is a
successful item outcome; a conflict or missing result is not. `effect` is
`unknown` when admission may have occurred but the response cannot prove the
outcome. Confirmed data remains in `data` for a partial result. `message` gives
a concrete next action but is not a parsing key.

Unless a shape below says otherwise, every named key is required. An opaque ID
or version is a nonempty string. A count is a nonnegative JSON integer. A time
is an RFC 3339 UTC string. Nullable keys are present with either the stated type
or `null`. Arrays preserve the ordering stated below.

## Result Schemas

`ScanStatus` contains:

- `state`: `initializing`, `discovering`, `inspecting`, `recovering`, `applying`,
  `idle`, or `failed`;
- `updatedAt`: milliseconds since the Unix epoch when scan progress last advanced,
  or `null` before any scan activity in this process;
- `publication`: an opaque string, or `null` before a Library is published;
- `completed` and `total`: counts, or `null` while the phase cannot report them;
- `updatedMs`: Unix milliseconds of the most recent scanner progress update, or
  zero before a scan starts in this process;
- `lastRecovery`: `null` or an object with count fields `relocatedPhotos`,
  `fingerprintedOriginals`, and `unavailablePhotos`; and
- `fingerprints`: `null` or an object with count fields `enrolled` and `pending`.

`status` data contains string fields `clientVersion` and `serverVersion`, integer
`cliContractVersion` with value `1`, boolean `published`, nullable string
`publication`, count `photoCount`, and `scan: ScanStatus`. `library check` data
is an object containing only `scan: ScanStatus`. A failed check retains that
same data shape with status `error` and `library_unavailable`.

`AlbumSummary` contains string `id` and `name`, count `photoCount`, boolean
`hasSavedPosition`, opaque string `albumVersion`, and absolute HTTP or HTTPS
string `webUrl`. An Album list item is either an `AlbumSummary` or exactly an
object with string `id` and `state: "missing"`. `albums get` data is one
`AlbumSummary`.

A Folder item contains string `location` and `name`, count `photoCount`, and
boolean `hasDescendantFolders`. Folder-list data contains `items` of that shape,
count `total`, nullable string `nextCursor`, time `evaluatedAt`,
`expiresAt: null`, string `publication`, and string `parent`.

Album-list data contains Album list `items`, count `total`, nullable string
`nextCursor`, time `evaluatedAt`, and nullable time `expiresAt`. Photo-list data
has the same five keys with Photo list `items`. For Album and Photo lists,
`expiresAt` is a time exactly when `nextCursor` is non-null and is otherwise
`null`.

`PreviewFacts` contains:

- `state`: `inspection-pending`, `ready`, `failed`, or `unavailable`;
- `source`: `jpeg-original`, `raw-embedded-jpeg`, or `null`;
- `sourceRevision`: an opaque string or `null`;
- `width` and `height`: positive integers or `null`; and
- `detailLimited`: a boolean or `null`.

All five nullable Preview fact fields are non-null when `state` is `ready`.
They are null when no current derivative facts exist. `PhotoItem` contains
string `id` and `filename`, string `originalId` with the associated Original
File identity, string `location` with that Original's current Library-relative
Location, `originalKind` of `raw` or `jpeg`, boolean
`originalAvailable`, `selectionState` of `undecided`, `selected`, or `rejected`,
integer `rating` from 0 through 5, opaque string `decisionVersion`, nullable
nonnegative integer `removedAtMs`, nullable camera-local string `captureTime`,
`preview: PreviewFacts`, and absolute HTTP or HTTPS string `webUrl`. A Photo
list item is either a `PhotoItem` or exactly an object with string `id` and
`state: "missing"`. `originalId` and `location` are how a caller inspects a
recovered Photo after it leaves a Recovery review.

`photos get` data contains every `PhotoItem` key plus `metadata`. `metadata`
contains `state` of `pending`, `known`, `missing`, `invalid`, or `failed`,
nullable string fields `captureTime`, `aperture`, `shutterSpeed`, and
`focalLength`, and nullable integer `iso`. The five value fields are facts only
when state is `known`; absent facts remain null.

`photos preview` success data contains string `photoId`, local string `path`,
`source` of `jpeg-original` or `raw-embedded-jpeg`, opaque string
`sourceRevision`, positive integer `width` and `height`, boolean
`detailLimited`, absolute HTTP or HTTPS string `webUrl`, and
`fileCommitted: true`. These facts describe the bytes published at `path`.

A Photo decision object contains `selectionState`, integer `rating` from 0
through 5, and opaque string `decisionVersion`. Photo decision data contains
`results` in request order and `counts`, whose required count fields are
`changed`, `unchanged`, `conflict`, and `missing`. Each result contains string
`photoId` and one of these exact outcome-specific additions:

- `changed`: `outcome: "changed"`, `prior` with the prior `selectionState` and
  `rating`, and `current` with a complete Photo decision object;
- `unchanged`: `outcome: "unchanged"` and `current` with a complete Photo
  decision object;
- `conflict`: `outcome: "conflict"` and `current` with a complete Photo decision
  object; or
- `missing`: only `outcome: "missing"`.

Explicit removal data contains string `operationId`, `counts`, and `results`.
`counts` has `removed`, `changedElsewhere`, `missing`, and `alreadyRemoved`.
`results` has one item per requested Photo in request order. Each item contains
`photoId`, `outcome`, and nullable `removedAtMs`. The exact outcome rules are:

- `removed`: `removedAtMs` is a nonnegative integer;
- `changed-elsewhere`, `unavailable`, and `already-removed`:
  `removedAtMs` is `null`.

Explicit Restore data contains string `operationId`, `counts`, and `results`.
`counts` has `restored`, `alreadyActive`, `changedElsewhere`, and `missing`.
`results` has one item per requested Photo in request order. Each result
contains `photoId` and one of `restored`, `already-active`, `changed-elsewhere`,
or `unavailable`. Counts must equal the result partition. The
operation-inspection commands return the same data shape without changing
Library state.

Album mutation data shapes are:

- create: `{ "album": AlbumSummary }`;
- rename: `{ "album": AlbumSummary, "renamed": boolean }`;
- delete: string `albumId`, `deleted: true`, and
  `originalFilesChanged: false`;
- add: `album: AlbumSummary`, `addedPhotoIds`, and `alreadyMemberPhotoIds`;
- remove: `album: AlbumSummary`, `removedPhotoIds`,
  `alreadyAbsentPhotoIds`, and nullable string `savedPhotoId`; and
- reorder: `album: AlbumSummary`, `orderedPhotoIds`, and boolean `reordered`.

`addedPhotoIds`, `alreadyMemberPhotoIds`, `removedPhotoIds`,
`alreadyAbsentPhotoIds`, and `orderedPhotoIds` are arrays of strings. Add and
remove partition the submitted IDs without omission or duplication and preserve
request order within each array. `orderedPhotoIds` is exactly the accepted
complete membership order. `savedPhotoId` remains a string or `null`. Every
returned Album summary contains the post-command version; a no-op keeps the
observed version.

`RecoveryItem` contains `state` of `unavailable`, `available`, `removed`, or
`missing`, string `originalId` and `photoId`, string `location` with the
remembered Library-relative Original Location, `kind` of `raw` or `jpeg`,
integer `rating` from 0 through 5, `selectionState` of `undecided`, `selected`,
or `rejected`, boolean `fingerprintEnrolled`, count `albumCount`, and absolute
HTTP or HTTPS string `webUrl`. A `missing` item retains the last reviewed
facts. `fingerprintEnrolled` reports persisted content evidence, and a missing
fingerprint is never presented as a content match.

`RecoveryMapping` contains string `mappingId`, `originalId`, and `photoId`,
string `fromLocation` and `toLocation`, `kind` of `raw` or `jpeg`, `outcome` of
`matched`, `content-mismatch`, `missing`, `kind-mismatch`, `unreadable`,
`occupied`, or `colliding`, boolean `verified`, nullable `blockedReason`, and
nullable `retire`. `verified` is true only when persisted content evidence
matched the destination. `blockedReason` is one of `colliding`,
`content-mismatch`, `destination-in-use`, `destination-removed`, `kind-mismatch`,
`missing`, or `unreadable` when the mapping cannot be applied, and `null` when
it can. `retire` is present only for a permitted retire-and-bind and contains
string `photoId`, `originalId`, and `location`.

`mappingId` is opaque. It identifies the reviewed correspondence and binds a
later confirmation to the remembered Original, the proposed Location, the
content evidence, the observed destination revision, and any destination Photo
to retire. It is not a Photo, Original, or operation identity, and a client must
not construct or reuse one from another review.

Recovery list data contains `RecoveryItem` or `RecoveryMapping` `items`, count
`total`, nullable string `nextCursor`, time `evaluatedAt`, and nullable time
`expiresAt`. `expiresAt` is non-null exactly when `nextCursor` is non-null.
`total` is the size of the complete retained review, not of the returned page.
The single mapping form returns the same list data with `total: 1`, one item,
and `nextCursor: null`.

Recovery reviews are bounded process state. A review page is at most the
advertised `recoveryPageMaximum`; a continuation keeps the page bound of the
command that opened the review. A Folder-prefix review evaluates at most
`recoveryMappingsMaximum` mappings and a larger scope is refused as
`recovery_scope_exceeded` before any continuation. One apply commits at most
`recoveryApplyMaximum` mappings. A continuation expires after
`recoveryReviewIdleSeconds` idle, on server restart, or earlier under bounded
least-recently-used eviction; the caller opens a fresh review instead of
assuming the retained set.

## Error Details

Known error codes are `invalid_input`, `not_found`, `conflict`, `name_conflict`,
`limit_exceeded`, `cursor_expired`, `incompatible_server`, `library_unavailable`,
`preview_unavailable`, `recovery_conflict`, `recovery_scope_exceeded`,
`server_busy`, `storage_failed`, `partial_result`,
`transport_failed`, `outcome_unknown`, `local_io_failed`,
`authentication_required`, and `access_denied`, plus the development codes
listed in [Photo Development](#photo-development). Development `details` uses
the shared route's recovery facts: conflict-family errors can carry nonempty
`currentSourceRevision` and nullable `currentRecipeVersion`; request conflicts,
missing recipes and unsupported Photos can carry `photoId`; invalid settings
can carry `argument` and `reason`; unavailable processing can carry `photoId`,
`operation`, or `reason`. Export routes can return an empty details object.
The other codes have these required shapes:

- `authentication_required` and `access_denied`: string `operation`;
- `invalid_input`: string `argument` and string `reason`;
- `not_found`: `resource` of `photo`, `album`, `folder`, or `original`, and
  string `reference`;
- `conflict`: `resource` of `photo` or `album`, string `reference`, and opaque
  string `currentVersion`;
- `name_conflict`: string `name` and string `albumId`;
- `limit_exceeded`: string `limitName`, count `limit`, and count `actual`;
- `cursor_expired`: `cursorKind` of `folder`, `album`, `photo`, `unavailable`,
  or `mappings`, and `reason` of `publication_replaced`, `process_restarted`, or
  `idle_or_evicted`;
- `incompatible_server`: integer `requestedContractVersion` and an integer array
  `supportedContractVersions`; capability-shape failures also include string
  `reason` and nullable string `field`;
- `library_unavailable`: `scan: ScanStatus`;
- `preview_unavailable`: string `photoId` and `state` of `inspection-pending`,
  `failed`, or `unavailable`;
- `recovery_conflict`: count `appliedMappings` with value `0`, count
  `refusedMappings`, and `rejections` with one item per refused mapping, each
  carrying string `originalId` and string `reason`;
- `recovery_scope_exceeded`: count `evaluated` and count `limit`;
- `server_busy`: string `operation` and nullable count `retryAfterSeconds`;
- `storage_failed`: string `operation`;
- `partial_result`: `counts` with the four required Photo outcome counts;
- `transport_failed`: string `operation`;
- `outcome_unknown`: string `operation`, string array `photoIds`, nullable string
  `albumId`, and nullable string `albumName`; and
- `local_io_failed`: `operation` of `read-credential`, `read-input`, `write-preview`, or
  `write-output`, nullable local string `path`, and boolean `fileCommitted`.

For `authentication_required`, `access_denied`, `server_busy`, `storage_failed`, `transport_failed`, and `outcome_unknown`,
`operation` is one of `status`, `library-check`, `folders-list`, `albums-list`,
`albums-get`, `photos-list`, `photos-get`, `photos-preview`, `photos-set`,
`albums-create`, `albums-rename`, `albums-delete`, `albums-add`, `albums-remove`,
`albums-reorder`, `processing-modules`, `processing-artifact`,
`processing-artifact-download`, `photos-processing-recipe-get`,
`photos-processing-recipe-save`, `photos-processing-recipe-rebind`,
`photos-processing-preview`, `photos-processing-export`,
`photos-processing-export-list`, `photos-processing-export-status`,
`photos-processing-export-cancel`, `photos-processing-export-retry`,
`photos-historical-export-download`,
`photos-proxy-get`, `photos-proxy-create`, `photos-proxy-remove`,
`recovery-unavailable`, `recovery-propose`, or `recovery-apply`.
`photoIds` in `outcome_unknown` contains all submitted
Photo IDs in request order. The Album fields identify the submitted target when
one exists and are null otherwise. Recipe writes, Export submission, and Recovery application can
have unknown mutation outcomes; module discovery, recipe reads, Recovery reads, and
download transfers use read/transfer errors and do not claim a saved mutation.

Command failures select codes as follows:

- syntax, closed-object, value, duplicate-ID, and malformed-cursor failures use
  `invalid_input`; a request bound exceeded before admission uses
  `limit_exceeded`;
- a missing direct target or requested Album member uses `not_found`; when more
  than one submitted ID is missing, `reference` is the first in request order;
- a stale Photo or Album version uses `conflict`; an Album order that no longer
  names the complete current membership also uses Album `conflict` with its
  current version;
- duplicate Album naming uses `name_conflict`;
- a well-formed continuation whose backing query or publication is gone uses
  `cursor_expired`; Folder restart uses `publication_replaced`, while retained
  queries distinguish `process_restarted` from `idle_or_evicted`;
- an unavailable Library or Preview uses its corresponding `library_unavailable`
  or `preview_unavailable` code; an unavailable admitted resource and an
  intrinsically over-budget retained query use `server_busy`; and
- a confirmed transaction rollback uses `storage_failed`.

A complete Photo batch result uses `partial_result` for mixed outcomes. An
all-unsuccessful batch uses `conflict` when any item conflicts and otherwise
`not_found`; its error details identify the first such item in request order and
its complete item data remains available. An HTTP response whose body or
required fields cannot be validated is not evidence of write failure. A read
whose response cannot be validated uses `transport_failed` with effect `none`.
A write or scan whose request may have been admitted uses `outcome_unknown`.

Exit codes are:

- `0`: confirmed success, including an empty list or unchanged mutation;
- `2`: invalid command, input, or request limit;
- `3`: confirmed missing object;
- `4`: confirmed conflict or name conflict, with no changes;
- `5`: a confirmed partial Photo batch;
- `6`: authentication required, access denied, unavailable service, expired cursor, incompatible contract, busy service,
  failed scan/Preview, local I/O, output failure, or a confirmed rolled-back
  storage failure;
- `7`: a write or scan may have been admitted but its outcome is unknown; and
- `130`: interruption. If a write may have been admitted, a best-effort JSON
  error uses `outcome_unknown`; abrupt process death cannot promise output.

For an all-unsuccessful Photo batch, use exit `4` if any conflict exists, else
`3`. Syntax and limits are checked before network mutation. Transport failure
known to precede sending is `transport_failed` with `effect: "none"`; uncertainty
after sending is `outcome_unknown` with `effect: "unknown"`. Failed JSON
serialization or stdout after a confirmed server commit cannot roll back that
commit. Failed reporting after local Preview publication follows
[Preview Download](#preview-download).

Consumers must validate `schemaVersion`, status, and required fields. Version 1
may add optional fields; it must not remove required fields or change their
types or meanings. Unknown future error codes must be treated as failures.

## Composition Example

The IDs in these commands are fixture examples. A real caller uses returned
IDs and versions and inspects every command result before its dependent step.

```sh
slipstream status
slipstream albums list --name '26春节'
slipstream photos list --album 00000000-0000-4000-8000-000000000010 --selection selected --rating-min 4 --order capture-time-asc --limit 60
slipstream albums create --name '春节精选'
```

The caller follows every `nextCursor`, then constructs `members.json` using
the membership input shape above. It adds at most 100 IDs per explicit command,
using the current version of the newly created Album:

```sh
slipstream albums add 00000000-0000-4000-8000-000000000020 --input members.json --if-version returned-album-version
slipstream photos list --album 00000000-0000-4000-8000-000000000020
slipstream photos preview 00000000-0000-4000-8000-000000000001 --file ./preview.jpg
```

These are composable operations, not an atomic task script. If the create
response is lost, the caller queries the exact Album name and inspects the
existing object; it must not claim ownership merely because the name matches.
If membership changes during assembly, the next versioned add fails and the
caller decides how to continue. The returned Album Web URL opens its Grid.
