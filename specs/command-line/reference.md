# CLI Reference

This document is the authoritative common command and output language for
[Command-Line Use](spec.md). The [darktable CLI Reference](../photo-development/darktable/cli.md)
owns standalone `darktable-edit` grammar and the full optional integration under `slipstream darktable`. Both define the
target contract; release availability is governed separately. Symbols in
uppercase stand for caller-supplied values, not literal IDs or generated defaults.

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
the active saved origin, then an error for network commands when none is
supplied. A supplied empty or invalid value is an error, not a fallback. A URL
must be an HTTP or HTTPS origin
with no credentials, path other than `/`, query, or fragment. HTTPS uses normal
certificate validation; there is no certificate-verification bypass. An HTTP
origin prints one unencrypted-connection warning to stderr before any request.

`--timeout` is an integer from 1 through 300 seconds and defaults to 30. For
ordinary commands it bounds the whole command after argument parsing, including
Preview transfer. For `workspace download`, `processing artifact-download` and `darktable export
download`, it bounds control requests and the maximum idle time between response
chunks; a transfer that continues making progress may run longer. Expiry uses
the result rules below.
It does not cancel an admitted mutation.

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
retains the existing unknown-outcome rules. Credentials resolve from
`--token-file`, then `SLIPSTREAM_ACCESS_TOKEN_FILE`, then the selected origin's
saved credential. An empty path, duplicate options, or omission when no saved
credential exists is `invalid_input` before any network request. File input is independent of
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

## Local Authentication

Slipstream uses one instance Access Token. `auth login` saves that existing
token for one canonical HTTPS origin; it does not create an account, browser
session, refresh token, or second permission model.

```text literal
slipstream --server https://photos.example.com auth login
slipstream --server https://photos.example.com auth login --token-stdin < token.txt
slipstream --server https://photos.example.com auth login --token-file token.txt
slipstream auth status
slipstream --server https://photos.example.com auth use
slipstream --server https://photos.example.com auth logout
```

The login token source is exactly one of a hidden terminal prompt,
`--token-stdin`, or the command-local `--token-file`. A token must never appear
in command arguments, URLs, JSON output, logs, or diagnostics. A noninteractive
login without an explicit source fails before any network request. Replacing an
existing origin requires terminal confirmation or `--force`. The global
`--token-file` override is rejected by `auth login`.

Login sends one bounded, read-only `/api/capabilities` request with the CLI
contract header and stores the token only after the service accepts it.
Authentication rejection, incompatibility, and transport failure leave the
previous saved credential unchanged. HTTPS port 443 and an omitted port name
the same saved origin.

The CLI uses the operating-system credential helper when available and
otherwise stores a user-owned 0600 file under the configuration directory.
Storage mutations and reconciliation after uncertain helper outcomes are
bounded. A failed replacement or logout must preserve the previous visible
credential and selection whenever restoration succeeds; storage failure must
be reported when restoration cannot be confirmed.

`auth status` never prints token material; it lists origins, active selection,
storage kind, login times, and a selected-origin check (`valid`, `invalid`,
`unreachable`, `incompatible`, or `not-configured`). `auth use` changes only the
local active origin. `auth logout` removes only the local saved entry and does
not revoke the server token. An explicit or environmental token file with an
explicit or environmental server origin must work even when the auth store
cannot be read. Operational HTTP and HTTPS support remains as defined above.


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
  [--selection all|unflagged|picked|rejected]
  [--rating-min N] [--rating-max N] [--kind raw|jpeg]
  [--available true|false] [--captured-from LOCAL_TIME]
  [--captured-before LOCAL_TIME]
  [--order capture-time-asc|capture-time-desc|album-order] [--limit N]
slipstream photos list --cursor CURSOR
slipstream photos get PHOTO_ID
slipstream photos resolve --location LOCATION
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
[Source Order](../library-browser/spec.md#source-order) and
[Source Ordering Selection](../library-browser/spec.md#source-ordering-selection).
Filters never rewrite Album order. The list pagination rules above apply.

The Web exposes exactly these Photo facts with the same meaning. It must not
add a second vocabulary, a renamed field, or a Web-only value for a fact the CLI
already names. A Preview fact, including `detailLimited`, keeps its
viewport- and zoom-agnostic source meaning on every surface; the Web may add a
local display condition for its own presentation, but that condition is never a
CLI fact and never renames the capability.

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

`photos resolve` performs an exact lookup of a current Library-relative Original
Location and returns one `PhotoItem`, including `id` and `webUrl`, without
metadata inspection or mutation. `PhotoItem.id` is the Photo ID accepted by
editing commands; `originalId` is not. The Location must be a nonempty relative
UTF-8 path with `/` separators and no empty, `.` or `..` component, leading `/`,
backslash, or NUL. It is case-sensitive and is not a host path or filename
search. Invalid or outside-Library syntax is `invalid_input` with
`argument: "location"`; a valid Location with no active indexed Photo,
including an unindexed or removed Original, is `not_found` with
`resource: "original"` and `reference` equal to the supplied Location. A retained
Photo whose Original is unavailable still resolves with `originalAvailable:
false`. An unavailable Library publication uses `library_unavailable`.
Resolution does not scan, import, upload, or create a Photo. If discovery is
stale, explicitly run `library check` and create a fresh query afterward.
With only a filename, query Original Folders and disambiguate returned Locations;
never choose a first basename match. Location Recovery preserves `PhotoItem.id`.

## Reversible Photo Removal and Restore

The explicit removal and Restore commands operate on a caller-selected set of
Photo IDs. They do not infer identity from a path, filename, Album, live query,
or current Selection State. Query the Photos first and preserve the returned
`selectionState`, `decisionVersion`, and `removedAtMs` as removal evidence.

```text literal
slipstream photos remove OPERATION_ID --input FILE
slipstream photos removal-operation OPERATION_ID
slipstream photos restore OPERATION_ID [--input FILE | --all [--exclude-input FILE]]
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

`photos restore` accepts one of three forms:

- With `--input FILE`, the input holds exactly one `photos` array whose items
  have exactly `photoId` and `removedAtMs`, using the evidence observed in a
  removal result or Trash listing. The command restores exactly those markers
  and `OPERATION_ID` is the caller-generated identity of this Restore attempt.
- With `--all`, the command restores every current Trash item except the
  identities in `--exclude-input FILE`. The remaining identities are resolved
  at admission under `OPERATION_ID`, and Trash items that arrive later never
  join the captured set. `--exclude-input` requires `--all` and holds
  `{"photoIds": [...]}` or a bare array of Photo IDs. `--all` and `--input` are
  mutually exclusive.
- With neither flag, `OPERATION_ID` names the removal operation whose complete
  removed set is restored.

The explicit-set and select-all forms report the same result partition and
retain their own operation identity. `photos restore-operation` reopens that
result without changing Library state.

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

## Trash and Permanent Deletion

The Trash commands inspect the persistent Trash and perform the reviewed second
deletion. Selection is shared with Restore: an explicit set names exact Photo
IDs, and a select-all set is every current Trash item minus explicit
exclusions. Permanent deletion is available only after a Trash review.

```text literal
slipstream trash list [--start N] [--limit N]
slipstream trash review OPERATION_ID (--input FILE | --all [--exclude-input FILE])
slipstream trash delete OPERATION_ID
slipstream trash operation OPERATION_ID
```

`trash list` pages Trash items newest removal first. `--limit` accepts 1
through 60 items and `--start` skips items.

`trash review` captures one fixed reviewed set. `--input FILE` names the exact
Photos to review and holds `{"photoIds": [...]}` or a bare array of Photo IDs.
`--all` reviews every current Trash item except the identities in
`--exclude-input FILE`; `--exclude-input` requires `--all` and uses the same
file format. `--all` and `--input` are mutually exclusive. `OPERATION_ID` is a
nonempty caller-generated identifier, and repeating it with the same intent
reopens the original review instead of capturing a second set. A selection
beyond the published review bound is refused, never truncated.

`trash delete` permanently deletes the Original Files of exactly that reviewed
operation and reports each reviewed Photo as `deleted`, `changed`, `missing`,
`failed`, or `pendingVerification`. It never widens the reviewed set, never
touches a Photo the review excluded, and never deletes a sibling, Sidecar,
export, or directory. `trash operation` reopens the durable result without
changing Library state. Permanent deletion has no Undo.

## Preview Download

```text literal
slipstream photos preview PHOTO_ID --file PATH [--size thumbnail|review]
```

Size defaults to `review`. The command obtains the current supported derivative;
it may wait for generation within the command timeout. It returns `photoId`,
`path`, `source`, `sourceRevision`, `width`, `height`, `detailLimited`, and
`webUrl`. `source` uses `jpeg-original` or `raw-embedded-jpeg`. These facts must
refer to the actual downloaded image. No arbitrary remote URL is accepted.

`detailLimited` is viewport- and zoom-agnostic. It reports the source capability:
`true` when the delivered dimensions are below the review target the requested
size asks for, and `false` when they reach it. It is derived from the delivered
Preview dimensions and the review target, never from a browser viewport, Preview
area, or zoom state, because the CLI has none. The CLI must not invent a browser
viewport or a zoom condition, and no CLI command accepts a viewport, window, or
zoom parameter. The Web may additionally report a local display condition, but
that presentation fact is not part of any CLI result.

PATH is local to the CLI host, must be valid UTF-8, must have an existing parent
directory, and must not exist. A non-UTF-8 value is `invalid_input` and must fail
before network access or temporary-file creation, without lossy substitution.
The transfer must not exceed 64 MiB, which is the existing
[Preview output JPEG bound](../photo-library/previews/design.md#native-library-boundary);
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
slipstream photos set PHOTO_ID --selection unflagged|picked|rejected --if-version VERSION
slipstream photos set PHOTO_ID --rating N --if-version VERSION
slipstream photos set --input FILE
```

The single-Photo forms are shorthand for a one-item batch. Selection and Rating
cannot be changed in the same command. Batch input has exactly `field`, `value`,
and `photos`; each item has exactly `photoId` and `ifVersion`.

```json
{
  "field": "selectionState",
  "value": "picked",
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

`recovery unavailable` opens one bounded review of active unavailable Photos.
`recovery propose` is read-only and returns the remembered and proposed
Locations, the file kind, and a plain-language evidence state. The Folder form
keeps relative suffixes; the single form evaluates one Original. A scope beyond
the advertised bound is refused rather than silently truncated.

The review and apply commands retain opaque mapping identities and bounded
continuations for automation. These fields are opaque to Photographers; the
CLI may display them in structured output for an Agent or scripted workflow.
Unverified content requires explicit confirmation; an occupied destination is
never merged silently.

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
`unavailablePhotos` for the remaining active unavailable Photos (`available=0`
and not Removed), and `mappings` in request order. Each applied mapping contains
`originalId`, `photoId`, `fromLocation`, `toLocation`, `webUrl`, and nullable
`retired`. A successful mapping is not proof that missing file bytes were
recreated.

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

## Photo Workspace

The [Photo Workspace](../photo-development/workspace/spec.md) is a persistent real directory associated with one Photo. Library queries supply the Photo ID once, to open that directory. Tools then consume paths and their own state directories. Ordinary same-host editing requires no download/upload/register/commit loop. A completed file is visible through reconciliation even when produced by an independent tool with no Slipstream integration.

```text literal
slipstream workspace open PHOTO_ID
slipstream workspace show WORKSPACE_ID
slipstream workspace files WORKSPACE_ID [--kind input|preview|output|resource|file] [--limit N]
slipstream workspace files --cursor CURSOR
slipstream workspace inspect WORKSPACE_ID ARTIFACT_ID
slipstream workspace download WORKSPACE_ID ARTIFACT_ID --file PATH [--revision REV]
slipstream workspace remove WORKSPACE_ID ARTIFACT_ID [--revision REV] [--request REQUEST_ID]
slipstream workspace refresh-input WORKSPACE_ID --input FILE [--request REQUEST_ID]
slipstream workspace delete WORKSPACE_ID --if-state TOKEN --request REQUEST_ID --confirm
```

Open creates or reopens the Photo's default workspace idempotently, using an independent read-only input snapshot rather than a writable link to the Original. It returns `workspaceId`, `photoId`, `path` (absolute actual directory), nullable `inputPath` and `inputArtifactId`, `webUrl`, and `execution: {hostId: string, mode: "local"|"shared-filesystem"|"service-host", accessibleToCaller: boolean, supervisedTools: string[]}`. Both input fields are null when no default input remains; reopen does not recreate it, select another file, or read the Original. Explicit refresh prepares an input again. Local/shared accessibility must be established, never inferred from a returned path. Unsupported remote execution is explicitly unavailable; the CLI does not create a remote shell, mount, checkout or synchronization protocol. Callers run independent tools on the disclosed host/environment with filesystem access, or use an available supervised integration.

Open also returns `previewDirectory`, `outputDirectory`, and suggested private edit-state paths. These are convenience paths, not required tool metadata or an initialized edit. Only the initial input image is visible until a tool writes a result; directories are created lazily. Show reports current directory/access facts and reconciled bounded file groups, including missing/checking/changed evidence; it returns `observedSourceRevision: string|null` and an opaque `workspaceStateToken` binding the complete current retained content/dependency inventory. Its `deletionSummary` contains input/preview/output/resource/other-file/edit-state counts, total known bytes, any external dependencies, and active operations, plus a bounded named sample/continuation for inspection. Reopen never retargets saved edits to changed Original bytes.

Refresh input accepts exactly `{expectedSourceRevision: string}` in bounded JSON rather than embedding raw source evidence in a command-line token. The value comes from show; missing/unavailable Original evidence refuses refresh without damaging existing files. Admission checks that observed Library revision and copies/reflinks through the guarded source descriptor, verifying stability before creating a unique protected input version. Success returns workspace identity, request identity, outcome (`created`, `unchanged`, `replayed`), input artifact/path/revision and fresh workspace token. An intact already prepared identical source revision may be reused; an explicitly removed identity must never be resurrected, including for identical source bytes. Explicit refresh establishes its returned input as default if none exists, without changing any saved tool/downstream binding. Request IDs default to generated returned values; explicit exact replay returns the original preparation result with separate current availability rather than recreating a subsequently removed file. A new refresh intent after removal needs a new request identity. Possibly admitted outcomes remain unknown until exact replay reconciliation; a source change or storage failure publishes no new ready input.

Refresh result data is `{workspaceId: string, requestId: string, outcome: "created"|"unchanged"|"replayed", inputArtifactId: string, inputPath: string, revision: string, workspaceStateToken: string, currentAvailability: "ready"|"missing"|"changed"|"removed"}`. Replay preserves the settled input identity/path/revision/token; currentAvailability separately reports its current usability. Replay never prepares removed bytes or reuses a deletion receipt as new creation intent.

Whole-workspace delete requires the token from the reviewed show/deletion summary, an explicit request identity and `--confirm`; no interactive prompt or `--force` bypass exists. The token binds the full loss inventory, so new files/state/dependencies invalidate approval. Active operations or references outside the workspace refuse deletion with their identities; caller must settle/rebind/remove them explicitly first. Under workspace admission coordination, delete atomically removes its usable association/visibility and records a durable deletion receipt before reclaiming only contained owned storage. Failed reclamation is disclosed and retriable from that receipt rather than claiming bytes are gone. Exact replay returns the same deletion result; a stale token has effect none. It never deletes a Library Original/sidecar or changes Library decisions. `--confirm` expresses explicit caller intent, not proof of human authorization.

Files uses the common bounded list/cursor envelope, default 50 and maximum 60, and reconciles completed regular files after missed events/restart. Private tool state/temp files are excluded. Items contain `artifactId`, `revision`, `relativePath`, `kind`, `state`, `byteLength`, nullable verified `sha256`, nullable verified `imageContract`, and `webUrl`. An unknown format remains a generic file. Folder/extension may suggest a role, but cannot fabricate successful processing provenance or next-tool compatibility. Inspect returns full verified file evidence, nullable tool/input/state-revision provenance, dependency references and availability. A preview's bounded display contract must not be represented as high-precision downstream input.

Download pins the named or freshly observed current completed revision, verifies length/digest and any asserted image contract, and publishes to a caller-local new path without replacement. In-place external changes invalidate old revision evidence and fail a pinned transfer; no old revision may continue with new bytes. Remote final delivery is optional; same-host callers read a completed file directly. Remove accepts an explicitly selected protected input snapshot: protection means read-only to tools, not permanently undeletable. An Agent must supply its observed `--revision`; a direct human action may atomically observe current evidence. Under the existing workspace admission coordination and tool-use locks, removal checks the revision and refuses current/retained state, captured jobs, kept-output/dependency references, active use, or unverified external use, naming each blocker. New binding/use admission shares that coordination, so it either pins the file before removal (which refuses), or sees the removed identity and fails. Success returns `{workspaceId, artifactId, revision, requestId, outcome: "removed"|"replayed", inputArtifactId: string|null, inputPath: string|null, workspaceStateToken}`. Removing the default clears both input fields even if another input exists; removing the last leaves truthful no input. Reopen does not recreate/retarget/read Original; explicit refresh may prepare a new default. Removal never changes Library Original bytes or discards unrelated edits/results.

## Photo Development

Camera Preview (`photos preview`) remains independent. [darktable CLI Reference](../photo-development/darktable/cli.md) owns standalone semantic grammar and the optional `slipstream darktable` same-directory adapter. Native parsing remains private. [Photo Workspace Architecture](../photo-development/workspace/design.md) owns indexing, file revision evidence, dependency-safe removal and non-destructive migration.

Independent peer tools use their own documented file/state CLI, with a completed workspace file as explicit input and the same directory as output. Their names are independent of Slipstream. No generic server recipe DSL is required to compose them: choosing a compatible output path for the next tool is the handoff. There is no implicit conversion, automatic chaining, upstream rerun or Library import. Slipstream may discover/supervise qualified tools, but cannot require peer tools to depend on its IDs, HTTP protocol, token or database.

The existing generic commands remain adapter projections where qualified, and retain historical access:

```text literal
slipstream processing modules
slipstream processing artifact ARTIFACT_ID
slipstream processing artifact-download ARTIFACT_ID --file PATH
slipstream photos processing-recipe get PHOTO_ID
slipstream photos processing-recipe save PHOTO_ID --input FILE
slipstream photos processing-recipe auto PHOTO_ID --input FILE
slipstream photos processing-recipe rebind PHOTO_ID --input FILE
slipstream photos processing-preview PHOTO_ID --step STEP_ID --file PATH
slipstream photos processing-export PHOTO_ID --input FILE
slipstream photos processing-export-list PHOTO_ID
slipstream photos processing-export-status PHOTO_ID REQUEST_ID
slipstream photos processing-export-cancel PHOTO_ID REQUEST_ID
slipstream photos processing-export-retry PHOTO_ID REQUEST_ID --input FILE
slipstream photos historical-export-download EXPORT_ID --file PATH
```

These adapter commands cannot create a second parameter source of truth or require a recipe before ordinary file processing. Recipe/step IDs project canonical on-disk tool states and retain old identities; `get` preserves unsupported historical intent read-only. Non-darktable writes preserve every existing darktable state and selection and refuse to mutate/execute one; darktable work stays under the accepted full namespace. Tools without a qualified semantic adapter remain usable through directory handoff, without fake generic control support.

### Retained Processing Adapter Exchanges

This section owns the exact generic adapter exchanges; all results are `data` in the [Output Envelope](#output-envelope). Every named key is required unless marked optional; nullable keys must be present. IDs are nonempty strings, digests are lowercase 64-character SHA-256, byte lengths are positive integers, and URLs are ordinary credential-free Web URLs. These contracts project supported non-darktable canonical tool state. They do not define a second darktable language. Generic writes/execution targeting darktable refuse and direct the caller to its full namespace; unrelated canonical states and their selections must survive every supported adapter write.

#### Bindings and Shared Types

`ImageContract` is `{format: string, precision: string, colorSpace: string, transfer: string, geometry: {width: integer, height: integer}, encoding: string}`. Names are nonempty strings of at most 128 UTF-8 bytes; edges are 1 through 65536. Values are verified actual image facts admitted by the selected module, not filename guesses. `Parameters` is `{schemaVersion: string, tree: object}` with the complete module-owned tree; discovery owns its schema/output choices. `Step` is `{stepId: string, module: string, input: Binding, parameters: Parameters}`. Step/module IDs are at most 64 bytes, other IDs/revisions 128 bytes, source evidence 16384 bytes, steps 64, parameter nesting depth 32, and complete mutation JSON 64 KiB. Request IDs are 1 through 128 ASCII alphanumeric, dot, underscore or hyphen bytes; Photo IDs retain the existing Library identity format. Other bounded names have no surrounding whitespace or control characters. Unknown/duplicate outer keys refuse; module trees follow their advertised schema. Step IDs are distinct. currentStepId must name one step when steps exist and must be null for zero steps.

`Binding` is exactly one of:

- `{kind: "original", photoId: string, sourceRevision: string}`: retained Original identity and nonempty exact source evidence for that same Photo. It is not a workspace default-input alias. A qualified migration may resolve it to a retained protected capture of that exact Original revision; this association must remain inspectable and cannot retarget to refreshed bytes. Without such a capture, execution needs the guarded Original and fails when it is unavailable/changed.
- `{kind: "artifact", artifactId: string, contract: ImageContract}`: the existing immutable Processing Artifact identity. Admission verifies retained bytes and exact asserted contract. An expired/missing artifact refuses; a same-name file cannot replace it.
- `{kind: "workspace-artifact", workspaceId: string, artifactId: string, revision: string, contract: ImageContract}`: an explicitly selected completed workspace image revision, including protected RAW or independently produced TIFF. Admission verifies containment, stable bytes and compatible actual image facts, then pins a protected capture when needed under [Stable Bytes and Containment](../photo-development/workspace/design.md#stable-bytes-and-containment). Unknown producer/parameters/provenance is allowed and stays unknown. Neither current default input nor path spelling can retarget this binding.

Development Proxy is a disposable display cache, not a fourth binding kind, protected workspace input, or full-resolution Film input. A legacy Export ID is not a Processing Artifact ID: historical export metadata/bytes remain readable through historical commands, and a verified completed image discovered in the workspace can be explicitly bound with `workspace-artifact`. Neither path invents missing historical provenance. Preview contracts cannot masquerade as full-resolution compatible inputs.

`sourceRevision` at the read-result level is the observed live Library source evidence, with the existing string carrier: `""` means unavailable/no published evidence. A stored recipe's sourceRevision is its captured source guard and may also be empty for workspace-only intent. Save/export must echo the read observation, including empty string; the service must not refresh the supplied value. For exclusively artifact/workspace-bound supported changes, source evidence is contextual: a subsequent unrelated live Original change or loss does not conflict or require a source read, and intact bound workspace bytes remain usable. Original-dependent admission still verifies the bound nonempty evidence or exact migrated protected capture; it never silently refreshes/rebinds it. Exact replay uses retained request identity before any fresh source/state admission.

#### Recipe Exchanges

`Recipe` is `{photoId: string, revision: string, sourceRevision: string, currentStepId: string|null, steps: Step[]}`. `get` data is `{photoId: string, sourceRevision: string, recipe: Recipe|null, webUrl: string}`. A null recipe means absent; a nonnull recipe with `steps: []` and `currentStepId: null` is a saved empty recipe with its own revision. Unsupported retained module/version trees remain readable verbatim and refuse execution/mutation. Read-only execution refusal metadata, if supplied, is optional `executionRefusals: [{stepId, code, message}]` on Recipe; it is not a save key or a substitute for retained intent.

Save input is exactly `{requestId: string, expectedRecipeRevision: string|null, expectedSourceRevision: string, currentStepId: string|null, steps: Step[]}`. Construct it from get by copying `sourceRevision` to `expectedSourceRevision`, `recipe.revision` (or null if absent) to `expectedRecipeRevision`, and the retained currentStepId/steps (or null/[]); add a new requestId and modify only intended supported state. Never submit Recipe's photoId/revision or diagnostic keys as save keys. Null expectedRecipeRevision asserts absence; it cannot overwrite a saved empty recipe. Under canonical state locks, the projected revision binds every supported state being replaced. Preserve unrelated/unknown states; refuse an unqualified multi-state atomic change rather than partially saving. A mismatched guard conflicts before no-op detection and is never refreshed by the adapter.

Unchanged retained steps outside the supported write scope, including darktable,
may be echoed for a read-derived complete save, but are compared and preserved
verbatim; their omission must not delete canonical state. Changing/removing their
parameters/bindings or changing their tool selection refuses. Setting an existing
Film step current changes only the generic adapter selection, not the separate
darktable edit selection. Removing a supported step removes only that adapter's
canonical state after dependency checks, never another tool's state.

Auto has that same complete input plus `automaticAdjustment: {stepId: string, operation: string, multiPriority: integer, instruction: object}`. These existing wire names remain unchanged; only a selected qualified non-darktable adapter's advertised action may execute. Instruction follows its schema, multiPriority is 0 through 128, and stepId must be current. An Original-only advertised action requires an Original binding. Unsupported actions, including old darktable exposure/channelmixerrgb actions, refuse here and use the darktable namespace.

Rebind input is exactly `{requestId: string, expectedRecipeRevision: string, newSourceRevision: string}`. It explicitly selects an already prepared protected input matching that observed nonempty source revision for eligible Original-bound supported states; it does not copy/change the Original, prepare an input implicitly, change artifact/workspace bindings, or mutate darktable/unrelated state. If no supported Original-bound state is eligible, the command refuses without changing state. Coordinates/resources/settings must remain valid or the complete request refuses. A selected workspace-artifact is changed only by guarded save of its explicit new Binding.

Save/auto/rebind data is `{photoId: string, requestId: string, outcome: "saved"|"unchanged"|"replayed", recipe: Recipe, recipeVersion: string, sourceRevision: string, webUrl: string}`. recipeVersion equals the returned committed recipe.revision; sourceRevision echoes the supplied expectedSourceRevision or newSourceRevision. Replayed returns the originally settled recipe/version even if a later write changed current state; obtain a fresh get before new intent. Request reuse with changed payload conflicts. A no-op keeps the revision; an effective save changes it. Recipe revisions include canonical on-disk state changes, so an external writer invalidates old evidence. A state conflict returns `data: null`, `error.code: "conflict"`, `effect: "none"`, and `details: {photoId, currentRecipeVersion: string|null}`; the existing currentRecipeVersion name refers to the projected revision. The caller keeps its intended draft and explicitly reconciles with a fresh read.

#### Export, Artifact, and Historical Results

Export input is exactly `{requestId: string, stepId: string, expectedRecipeRevision: string, expectedSourceRevision: string}`; the step must be current. It captures immutable canonical state, bound bytes, resources, engine/bundle and the module-tree output choice before queue admission. Retry input is exactly `{requestId: string}` with a new ID and a retained failed/cancelled request as the command target. It reuses that captured snapshot, not current settings/default input/Original. Missing resources or bytes refuse truthfully. Only accepted supervised work continues after disconnect; independent synchronous processes do not gain that guarantee.

`Work` contains string `photoId`, `requestId`, `stepId`, `module`, `recipeRevision`, `sourceRevision`, `adapterSchemaVersion`, `bundleId`; `parameters: Parameters`; `input: Binding`; `state: "accepted"|"executing"|"succeeded"|"failed"|"cancelled"`; integer `acceptedAt` (Unix seconds); nullable `attempt: {sequence: positive integer, beganAt: Unix seconds}`; nullable string `artifactId` (nonnull exactly when succeeded), nullable string `failureReason` (nonnull exactly when failed); nullable integer `terminalAt` (Unix seconds, nonnull exactly when terminal); and nullable integer `retainUntil` (Unix seconds; null means no scheduled receipt expiry). Empty sourceRevision is permitted as above. Receipts retain immutable input evidence/resources internally; exposing input does not authorize retargeting them.

`Artifact` contains string `artifactId`, `photoId`, `stepId`, `module`, `adapterSchemaVersion`, `bundleId`, `filename`; `parameters: Parameters`; `input: {binding: Binding, sha256: digest, byteLength: integer}`; `outputContract: ImageContract`; `sha256: digest`; `byteLength: integer`; UTC time `publishedAt`; nullable UTC time `expiresAt`; and optional `webUrl`. Filename is a basename with no separators/control characters. Workspace-backed artifact expiresAt is null until explicit safe removal; historical immutable retained artifact expiry remains its real time. Processing provenance must be verified to construct Artifact; a newly discovered unknown-provenance image uses workspace inspect instead. Its eligibility as an input does not fabricate an Artifact of its producer.

Export/retry acceptance data is `{photoId, requestId, stepId, outcome: "accepted"|"replayed", state: Work.state, receipt: Work, webUrl}`. Already completed success data is `{photoId, requestId, stepId, outcome: "published"|"replayed", artifact: Artifact, webUrl}`. Replay preserves captured identity/terminal result, even when it differs from current state. A retained failed/cancelled export replay returns status error with `data: Work`, effect none and its retained failure/terminal conflict, not new execution. Status data is Work plus webUrl. Cancel settles the first terminal outcome: non-success data is Work plus webUrl (including already failed/cancelled); already succeeded data is `{photoId, requestId, stepId, outcome: "succeeded", replayed: boolean, artifact: Artifact, webUrl}`. Cancellation cannot undo a published result.

Export-list data is `{photoId, exports: Work[], artifacts: Artifact[], historicalExports: HistoricalExport[], webUrl}`. Each array is bounded to 64, ordered newest first by acceptedAt, publishedAt, or createdAt respectively (identity ascending breaks ties). The bound limits the returned recent window, not retention; exact-ID status/inspect remains available. `processing artifact` data is Artifact with required webUrl. `processing artifact-download` data is `{artifactId, photoId, stepId, module, filename, expiresAt: time|null, bundleId, adapterSchemaVersion, width: integer, height: integer, byteLength: integer, sha256: digest, path: string, webUrl, fileCommitted: true}`; dimensions come from outputContract.geometry and transfer pins verified bytes.

`HistoricalExport` contains string `exportId`, `photoId`, `recipeVersion`, `sourceRevision`, `bundleId`; `target: "development-tiff"|"film-jpeg"`; `state: "queued"|"running"|"succeeded"|"failed"|"cancelled"`; UTC time `createdAt`; nullable UTC time `settledAt` (nonnull exactly when terminal); nullable `terminalOutcome` (terminal state exactly, otherwise null); nullable string `failureReason` (nonnull exactly when failed); nullable UTC time `receiptExpiresAt`; and nullable `artifact: HistoricalArtifact` (only a succeeded record may have one; missing retained metadata remains null). `HistoricalArtifact` contains string `exportId`, `filename`, `orientation`, `sampleFormat`, `colorSpace`, `profileIdentity`; matching target; `stage: "develop"|"film"`; `contentType: "image/tiff"|"image/jpeg"`; boolean `iccEmbedded`; positive integer `width`, `height`, `byteLength`; digest `sha256`; UTC time `expiresAt`. Target fixes the corresponding stage/content type. Historical download data is `{exportId, photoId, historical: true, artifact: HistoricalArtifact, path: string, fileCommitted: true, webUrl}`. Missing/expired bytes refuse, without inventing composable provenance.

#### Preview and Discovery Results

Preview captures the currently selected saved step under its state lock; it does not save/rebind. Pending data is `{photoId, stepId, state: "queued"|"running", webUrl, fileCommitted: false, handle: PreviewHandle}`. `PreviewHandle` is `{photoId: string, stepId: string, recipeRevision: string, identity: digest}` and identifies the captured rendition in workspace inspect provenance; it is not a new command or reusable current-state guard. A subsequent preview invocation observes the then-current saved step and reuses matching work when available. A newer edit changes current identity; an old captured result remains inspectable in workspace files but cannot be returned as current or replace the selected preview. The caller repeats the same command for readiness without an implicit CLI polling loop.

Ready data is `{photoId, stepId, state: "ready", sourceRevision: string, recipeRevision: string, comparison: "current", inputSha256: digest, inputByteLength: integer, contentType: "image/png", width: integer, height: integer, byteLength: integer, sha256: digest, path: string, webUrl, fileCommitted: true, provenance: PreviewProvenance}`. `PreviewProvenance` is `{input: Binding, module: string, adapterSchemaVersion: string, bundleId: string, parameterDigest: digest, identity: digest, outputContract: ImageContract, displayConversion: string, detail: {fullResolution: boolean, scale: number}}`. Scale is positive and at most 1; dimensions describe the actual bounded display PNG, not the full output. The identity binds captured state/input/bundle/output/display conversion. sourceRevision may be empty for workspace-only work. Transfer validates exact captured evidence, PNG structure, digest, and the 64 MiB bound before no-replace publication. Pending never creates the requested local file; no camera/baseline image substitutes for requested processing.

Modules data is `{contractVersion: string, modules: ModuleDescription[]}` ordered by module name. `ModuleDescription` is `{id: {name: string, adapterVersion: string}, parameterVersions: string[], parameterSchema: object, admittedInputs: DiscoveryImageContract[], admittedOutputs: DiscoveryImageContract[], limits: {maxInputBytes: positive integer, maxParameterBytes: positive integer, maxOutputPixels: positive integer, deadlineMillis: positive integer}, availability: {state: "ready"|"unavailable", refusalReasons: string[]}}`. `DiscoveryImageContract` preserves the existing discovery carrier `{format: string, colorSpace: string, transferFunction: string, precisionBits: positive integer, width: integer, height: integer}`: zero discovery width/height means unconstrained subject to finite module limits, never a zero-sized bound image. Discovery and Binding image carriers keep their distinct existing names; derive transfer/precision/encoding from the selected module's advertised semantic constraints and verify actual bytes. Availability alone does not establish compatibility. Darktable may be discovered, but generic semantic execution remains unavailable and directs to its full namespace.

#### JSON and Acceptance Examples

An absent recipe read for a Photo whose Original is unavailable has this `data` (the envelope is omitted here):

```json
{
  "photoId": "00000000-0000-4000-8000-000000000001",
  "sourceRevision": "",
  "recipe": null,
  "webUrl": "https://photos.example/?photoId=00000000-0000-4000-8000-000000000001"
}
```

The complete empty save constructed from that read is valid JSON and distinguishes absence from saved empty state:

```json
{
  "requestId": "empty-save-1",
  "expectedRecipeRevision": null,
  "expectedSourceRevision": "",
  "currentStepId": null,
  "steps": []
}
```

A selected completed TIFF binding, copied from verified workspace inspect, is:

```json
{
  "kind": "workspace-artifact",
  "workspaceId": "workspace-1",
  "artifactId": "tiff-1",
  "revision": "bytes-r1",
  "contract": {
    "format": "image/tiff",
    "precision": "float32",
    "colorSpace": "prophoto-rgb",
    "transfer": "linear",
    "geometry": { "width": 9504, "height": 6336 },
    "encoding": "deflate"
  }
}
```

For Film, obtain the ready `spektrafilm` ModuleDescription and its complete `parameterSchema.default` tree; create one Step with `stepId: "film-1"`, `module: "spektrafilm"`, the above verified Binding, and Parameters with the discovered parameterVersion and that full tree. This example assumes inspection verified those exact TIFF facts, including the qualified linear ProPhoto ICC profile; copy the actual verified geometry and encoding, never infer them from discovery-example dimensions. Save using the get guards; no placeholder tree or darktable parameter DSL is valid. After get reports revision `recipe-r1`, export uses:

```json
{
  "requestId": "film-export-1",
  "stepId": "film-1",
  "expectedRecipeRevision": "recipe-r1",
  "expectedSourceRevision": ""
}
```

A stale observed save refuses with the complete error envelope:

```json
{
  "schemaVersion": 1,
  "status": "error",
  "data": null,
  "error": {
    "code": "conflict",
    "message": "Read the current recipe and reconcile your retained draft.",
    "effect": "none",
    "details": {
      "photoId": "00000000-0000-4000-8000-000000000001",
      "currentRecipeVersion": "recipe-r2"
    }
  }
}
```

Selective removal success has this data; the null fields disclose that the
removed snapshot was the default, without selecting another input:

```json
{
  "workspaceId": "workspace-1",
  "artifactId": "raw-old",
  "revision": "raw-r1",
  "requestId": "remove-1",
  "outcome": "removed",
  "inputArtifactId": null,
  "inputPath": null,
  "workspaceStateToken": "inventory-r2"
}
```

The Agent submits this selection with `slipstream workspace remove workspace-1 raw-old --revision raw-r1 --request remove-1`. A blocked removal returns `artifact_referenced`, exit 4 and effect `none`, with the named blockers defined under [Error Details](#error-details). A changed revision uses `artifact_changed`. Exact removal replay returns its settled selection/default facts, not the later current inventory; use show for current facts.

- Film over a completed TIFF: validate an independently produced image with unknown provenance, save its explicit revision binding and discovered complete Film parameters, export, and decode Work/Artifact. The TIFF and unrelated darktable edits remain intact; no upstream engine, upload, registration or provenance invention occurs.
- Source missing: prepare a workspace capture or select a verified TIFF, then lose the Library Original. Get reports empty live source evidence; guarded supported workspace-bound save/preview/export continues from intact bytes. A legacy Original binding without its exact protected capture refuses; rebind/refresh needs actual source evidence.
- State conflict: read recipe-r1, change canonical state externally, and submit the old complete save. It returns conflict with no effect and retains the caller's draft. Exact replay of an earlier committed request returns its old recipe-r1 result; a fresh read observes the newer current revision.
- Selective removal: refresh to a second protected RAW, explicitly remove the unreferenced old input at its observed revision, and retain all edits/results using the new RAW or TIFF. An old input still named by a retained state/job/output refuses with named blockers; a concurrent new bind cannot race past removal. Removing the default/last unused input leaves null input fields on reopen, never an Original read; explicit refresh creates a new artifact identity and the old removal receipt remains exactly replayable.

Artifact download streams incrementally, validates strong revision/length/SHA-256 and asserted image structure/contract, and publishes without replacement. Private resumable state uses `PATH.slipstream-part` and `PATH.slipstream-part.json`; matching state is rehashed before one contiguous range resumes. Mismatched state or ignored ranges restart safely; expired/missing artifacts remove only owned partial state. In-place workspace changes fail the pinned revision rather than mix bytes. Headers bind actual digest, length, filename and revision; historical download validates retained metadata without inventing composable evidence. File metadata does not carry credential-bearing URLs or raw image bytes.

Development errors retain invalid-settings/input (exit 2), missing identity/state/file (exit 3), stale state/source/request or terminal conflict (exit 4), and unavailable execution/resources, exhausted capacity, disk-full or expired historical data (exit 6). Possibly admitted writes retain effect `unknown`, exit 7, and exact replay guidance. Confirmed refusal has effect `none`. None changes Library decisions/Albums, refreshes a supplied guard, silently substitutes resources, or submits dependent work after uncertainty.

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

`detailLimited` is the source capability, not a display measurement. It is
`true` when the delivered dimensions are below the review target and `false`
when they reach it, so it never depends on a viewport, Preview area, or zoom.
All five nullable Preview fact fields are non-null when `state` is `ready`.
They are null when no current derivative facts exist. `PhotoItem` contains
string `id` and `filename`, string `originalId` with the associated Original
File identity, string `location` with that Original's current Library-relative
Location, `originalKind` of `raw` or `jpeg`, boolean
`originalAvailable`, `selectionState` of `unflagged`, `picked`, or `rejected`,
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
`photos resolve` data is exactly one `PhotoItem`; use its `id` as `PHOTO_ID`.

`photos preview` success data contains string `photoId`, local string `path`,
`source` of `jpeg-original` or `raw-embedded-jpeg`, opaque string
`sourceRevision`, positive integer `width` and `height`, boolean
`detailLimited`, absolute HTTP or HTTPS string `webUrl`, and
`fileCommitted: true`. These facts describe the bytes published at `path`.
`detailLimited` compares those delivered `width` and `height` against the review
target of the requested size; it carries no viewport or zoom meaning.

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
`selectionState` of `unflagged`, `picked`, or `rejected`, boolean
`fingerprintEnrolled`, count `albumCount`, and absolute
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
it can. `retire` is present only for a permitted Retire and Bind and contains
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
listed in [Photo Development](#photo-development) and the command-specific
codes in [darktable Errors](../photo-development/darktable/cli.md#darktable-errors). Development `details` uses
the shared route's recovery facts: conflict-family errors can carry nonempty
`currentSourceRevision` and nullable `currentRecipeVersion`; request conflicts,
missing recipes and unsupported Photos can carry `photoId`; invalid settings
can carry `argument` and `reason`; unavailable processing can carry `photoId`,
`operation`, or `reason`. Export routes can return an empty details object.
The other codes have these required shapes:

- `authentication_required` and `access_denied`: string `operation`;
- `invalid_input`: string `argument` and string `reason`;
- `not_found`: `resource` of `photo`, `album`, `folder`, `original`, `workspace`, or `workspace-artifact`, and
  string `reference`;
- `conflict`: `resource` of `photo` or `album`, string `reference`, and opaque
  string `currentVersion`;
- `name_conflict`: string `name` and string `albumId`;
- `limit_exceeded`: string `limitName`, count `limit`, and count `actual`;
- `cursor_expired`: `cursorKind` of `folder`, `album`, `photo`, `unavailable`,
  `mappings`, `workspace`, or `darktable`, and `reason` of `publication_replaced`,
  `process_restarted`, or `idle_or_evicted`;
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
`operation` is one of `auth-login`, `auth-status`, `auth-use`, `auth-logout`,
`status`, `library-check`, `folders-list`, `albums-list`,
`albums-get`, `photos-list`, `photos-get`, `photos-resolve`, `photos-preview`, `photos-set`,
`albums-create`, `albums-rename`, `albums-delete`, `albums-add`, `albums-remove`,
`albums-reorder`, `processing-modules`, `processing-artifact`,
`processing-artifact-download`, `photos-processing-recipe-get`,
`photos-processing-recipe-save`, `photos.processing-recipe.auto`,
`photos-processing-recipe-rebind`,
`photos-processing-preview`, `photos-processing-export`,
`photos-processing-export-list`, `photos-processing-export-status`,
`photos-processing-export-cancel`, `photos-processing-export-retry`,
`photos-historical-export-download`,
`recovery-unavailable`, `recovery-propose`, or `recovery-apply`.
darktable operations use their full command path joined with hyphens as defined
in the subreference. Common error/envelope rules also apply to those operations.
Workspace operation names use their command path joined by hyphens. Workspace-specific errors are `artifact_changed` with `{workspaceId, artifactId, expectedRevision, currentRevision}` and `artifact_referenced` with `{workspaceId, artifactId, revision, blockers: [{kind: "state"|"job"|"output"|"dependency"|"active-use"|"unverified-use", id: string|null, name: string, action: string}]}`, both exit 4 with effect `none`. Blocker names and recovery actions remain useful when external use has no service identity. `workspace_unavailable` and `execution_unavailable` disclose host/access facts and recovery, exit 6. Uncertain workspace open/remove additionally retain nullable `workspaceId`, `artifactId`, and `requestId`; a lost open is reconciled by reopening the same Photo's default workspace, while removal uses exact retained request identity. Workspace reads/downloads do not claim unknown semantic writes.

Whole-workspace deletion also reports `workspace_changed` when the reviewed
loss inventory changed and `workspace_referenced` with `{workspaceId, blockers}`
when dependencies or independent use prevent deletion. Both are confirmed
refusals, exit 4 with `effect: none`; `workspaceId` must be a nonempty string
and `blockers` must be a nonempty array of objects using the named recovery
fields above. Each blocker requires a declared `kind`, nullable/string `id`,
and nonempty string `name` and `action`; an optional `reason` may supply
additional context. The caller reviews the inventory again or settles the
named use. A malformed required field is not evidence of a confirmed refusal:
after mutation admission the CLI reports `outcome_unknown`, exit 7, retaining
the submitted request identity without retrying the mutation.
Supervised darktable `invalid_mapping` is a confirmed invalid-command refusal,
exit 2 with `effect: none`; its details may be an empty object.
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
slipstream photos list --album 00000000-0000-4000-8000-000000000010 --selection picked --rating-min 4 --order capture-time-asc --limit 60
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
