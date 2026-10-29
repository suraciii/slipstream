# CLI Reference

[Command-Line Use](spec.md) owns product behavior. This document owns the common command and output language.

The [darktable CLI Reference](../photo-development/darktable/cli.md) owns standalone `darktable-edit` grammar and the full optional `slipstream darktable` integration. Both references define the target contract; release availability is separate.

Uppercase symbols represent caller-supplied values, not literal IDs or generated defaults.

## Invocation

```text literal
slipstream [--server URL] [--token-file FILE] [--output json|text] [--timeout SECONDS] COMMAND
slipstream --help
slipstream --version
slipstream COMMAND --help
```

Global options precede the command. `--output` defaults to `json` regardless of terminal detection. Help and version are plain text and require no network request.

Unknown or duplicate options, extra positional arguments, and invalid combinations are errors. Flags MUST NOT accept abbreviations.

The service URL resolves in this order:

1. `--server`.
2. `SLIPSTREAM_SERVER_URL`.
3. The active saved origin.

Network commands fail when none is supplied. A supplied empty or invalid value is an error, not a fallback.

A URL MUST be an HTTP or HTTPS origin without credentials, a path other than `/`, a query, or a fragment. HTTPS uses normal certificate validation without a bypass. HTTP prints one unencrypted-connection warning to stderr before any request.

`--timeout` is an integer from 1 through 300 seconds and defaults to 30. Ordinary commands use it as a whole-command bound after argument parsing, including Preview transfer.

For `workspace download`, `processing artifact-download`, and `darktable export download`, it bounds control requests and idle time between chunks. A transfer making progress can run longer.

Expiry follows the result rules below and does not cancel an admitted mutation.

`--input FILE` reads one UTF-8 JSON document; `--input -` reads stdin. The document MUST fit 64 KiB, except [Trash cleanup input](#trash-inspection-and-record-cleanup), which permits up to 1 MiB.

An oversized document is refused as soon as a read crosses its applicable bound. `limit_exceeded` reports `actual` as `limit + 1` and does not read the rest of the input.

Unknown keys, duplicate JSON object keys, duplicate Photo IDs, trailing content, and an empty mutation list are invalid. The entire document MUST validate before a write.

Except Trash Record Cleanup, a mutation contains at most 100 Photo IDs. The CLI MUST NOT split it automatically. Local input-file failures perform no write.

## Instance Credentials

The CLI sends a generated Access Token from a private file only in the Authorization header to the selected origin. The file MUST contain exactly one base64url token, optionally followed by one line ending.

The CLI MUST reject missing, unreadable, empty, or malformed credentials before network mutation. It MUST NOT echo file contents. Help and version require no credential.

Credentials MUST NOT appear in plaintext command arguments, URL components, or mutation input. A request redirect MUST NOT forward the credential.

A confirmed authentication rejection means the request was not admitted; the CLI MUST NOT retry it automatically. Transport loss after a sent write follows the unknown-outcome rules.

Credentials resolve in this order:

1. `--token-file`.
2. `SLIPSTREAM_ACCESS_TOKEN_FILE`.
3. The selected origin's saved credential.

An empty path, duplicate options, or omission without a saved credential is `invalid_input` before any network request. Credential input is independent of mutation stdin.

The file MUST be a regular nonsymlink file owned by the effective user without group or other permission bits. Validation and reading MUST use the same opened file. Files larger than 45 bytes MUST be refused.

Missing or unreadable files use `local_io_failed` with operation `read-credential` and `fileCommitted: false`. Unsafe permissions or invalid content use `invalid_input`. Neither error contains credential contents.

| Confirmed response | CLI result |
| --- | --- |
| 401 | `authentication_required` |
| 403 | `access_denied` |
| 429 | `server_busy` with parsed Retry-After when available |
| Boundary 503 `access_unavailable` or `access_unconfigured` | `server_busy`, effect `none`, and `retryAfterSeconds: null` |

The boundary 503 is not a storage rollback or evidence that the operation ran. The CLI MUST NOT infer it from an arbitrary 503 body.

`authentication_required` and `access_denied` have `details: { "operation": OPERATION }`, exit 6, and effect `none` when rejection is confirmed before admission. An unexpected or invalid response to a possibly admitted write MUST retain `outcome_unknown`.

A token does not bypass contract negotiation. HTTP and HTTPS use the same authentication and result mappings.

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

The login token source is exactly one of a hidden terminal prompt, `--token-stdin`, or the command-local `--token-file`. A token MUST NOT appear in command arguments, URLs, JSON output, logs, or diagnostics.

Noninteractive login without an explicit source fails before any network request. Replacing an existing origin requires terminal confirmation or `--force`. `auth login` rejects the global `--token-file` override.

Login sends one bounded, read-only `/api/capabilities` request with the CLI
contract header and stores the token only after the service accepts it.
Authentication rejection, incompatibility, and transport failure leave the
previous saved credential unchanged. HTTPS port 443 and an omitted port name
the same saved origin.

The CLI uses the operating-system credential helper when available and
otherwise stores a user-owned 0600 file under the configuration directory.
Storage mutations and reconciliation after uncertain helper outcomes are
bounded. A failed replacement or logout MUST preserve the previous visible
credential and selection whenever restoration succeeds; storage failure MUST
be reported when restoration cannot be confirmed.

`auth status` never prints token material. It lists origins, active selection, storage kind, login times, and a selected-origin check: `valid`, `invalid`, `unreachable`, `incompatible`, or `not-configured`.

`auth use` changes only the local active origin. `auth logout` removes only the local saved entry and does not revoke the server token.

An explicit or environmental token file with an explicit or environmental server origin MUST work even when the auth store cannot be read. Operational HTTP and HTTPS support remains as defined above.


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

`status` returns client/server versions, supported CLI contract version, published availability, Photo count, and scan facts. A reachable service with an initializing or failed scan is a successful status query with explicit scan state. An incompatible service is an error.

Scan facts describe only the Library axis. Processing Module availability and Photo input support are separate axes unaffected by a recovering or saturated scan.

Compatibility is checked before any operational request. Use a client and service built from the same candidate source revision. Contract version 2 alone does not prove all required capability fields exist.

An invalid or incomplete capability response is `incompatible_server`, exit 6, effect `none`. Its diagnostic preserves valid advertised versions and identifies the missing or invalid field. Install a matching candidate; do not bypass negotiation.

`library check` waits within the command timeout for the existing scan cycle's terminal result. A failed scan is a command error with its last confirmed status.

Timeout reports an unknown, non-terminal outcome and directs the caller to `status`; it does not claim cancellation. The admitted server-owned scan continues, and later status reports its latest phase and counts. Reissuing `library check` joins that cycle rather than starting a second physical scan.

Folder Locations are Library-relative and recursive for Photo queries. The empty parent denotes the Library Folder; omission uses that parent.

Folder listing returns direct children, recursive Photo counts, descendant-Folder presence, and the Published Library reference. An expired Folder publication requires a fresh listing under the existing Web contract.

Album name lookup uses the existing exact ASCII-case-folded comparison, not substring search. `--photo` lists only Albums containing that Photo.

Album summaries include ID, name, member count, saved-position availability, version, and Web URL. `get` never downloads all members.

List commands default to 50 items and accept limits from 1 through 60. Each list returns `items`, `total`, and `nextCursor`, which is a string or `null`.

Album lists freeze ordered IDs on the first page, but summaries are current when read. A concurrently deleted Album is an item with its ID and `state: "missing"`. Name and Photo membership filters are evaluated at query creation.

Folder windows use their publication-bound continuation. A continuation accepts only `--cursor` and global options; callers MUST NOT combine it with new filters.

Every cursor is opaque and bound to its endpoint kind. It carries the original query and page size.

Album, Photo, and Trash cursors refer to retained ordered-ID collections in one server process. They expire on restart, after 15 minutes idle, or through bounded least-recently-used eviction.

Their `expiresAt` is the current idle deadline. Later activity can extend it, while eviction can invalidate the cursor earlier.

A valid query exceeding the 1,000,000 retained-ID maximum returns `server_busy`, effect `none`, and `retryAfterSeconds: null`. The operation is `albums-list`, `photos-list`, or `trash-list`; no partial query or cursor is published. The caller MUST narrow the source or filters rather than assume retrying will make it fit.

A Folder cursor instead carries the publication, parent, range, and page size without a retained query collection. Its `expiresAt: null` means no time deadline.

It expires when its Published Library is no longer current, including replacement after restart. It does not participate in idle expiry or query eviction. Every list response includes `evaluatedAt` and `expiresAt` for its cursor kind.

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

Each multiline command shape above is one invocation. All Photos is the default source; omitted filters impose no restriction.

Rating bounds are inclusive integers from 0 through 5; a lower bound above an upper bound fails. Availability means Original File availability, not Preview readiness.

`LOCAL_TIME` has exact form `YYYY-MM-DDTHH:MM:SS` and MUST be a valid
camera-local date and time with no offset or timezone suffix. A supplied lower
bound MUST precede a supplied upper bound. Existing subsecond facts participate
in comparison against these second-aligned boundaries.

Order defaults to `album-order` for an Album and `capture-time-asc` otherwise. `album-order` is invalid for other sources. Capture ordering and ties follow [Source Order](../library-browser/spec.md#source-order) and [Source Ordering Selection](../library-browser/spec.md#source-ordering-selection).

Filters never rewrite Album order. The list pagination rules above apply.

The Web exposes the same Photo facts with the same meaning. It MUST NOT add a second vocabulary, renamed field, or Web-only value for a fact already named by the CLI.

Preview facts, including `detailLimited`, retain viewport- and zoom-agnostic source meaning everywhere. The Web MAY add a local display condition, but that condition is not a CLI fact and does not rename the capability.

Each ordinary Photo item uses the `PhotoItem` shape in [Result Schemas](#result-schemas), including the complete `preview` object and nullable `removedAtMs`. An active Library Photo has `removedAtMs: null`.

`get` adds bounded `metadata`; use `albums list --photo` for membership. Absolute server paths never appear. Missing metadata fields are `null` with their inspection state, not guessed.

A Photo removed after query creation retains its result position as `{"id":"…","state":"missing"}`. An unavailable Original with a retained Photo still returns ordinary Photo facts.

`total` is the original match count, including missing placeholders, not the current matching count. `get` for a removed Photo is `not_found`.

`photos resolve` performs an exact current Library-relative Original Location lookup. It returns one `PhotoItem`, including `id` and `webUrl`, without metadata inspection or mutation. `PhotoItem.id`, not `originalId`, is the Photo ID accepted by editing commands.

The Location MUST be a nonempty relative UTF-8 path with `/` separators. It MUST NOT contain an empty, `.` or `..` component, a leading `/`, a backslash, or NUL. Matching is case-sensitive; this is not a host path or filename search.

| Condition | Result |
| --- | --- |
| Invalid or outside-Library syntax | `invalid_input` with `argument: "location"` |
| Valid Location without an active indexed Photo, including unindexed or removed Originals | `not_found` with `resource: "original"` and the supplied Location as `reference` |
| Retained Photo with unavailable Original | Resolved Photo with `originalAvailable: false` |
| Unavailable Library publication | `library_unavailable` |

Resolution does not scan, import, upload, or create a Photo. If discovery is stale, run `library check` and create a fresh query. With only a filename, query Original Folders and disambiguate Locations; never choose the first basename match.

Location Recovery preserves `PhotoItem.id`.

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

`OPERATION_ID` is a nonempty caller-generated identifier.

Removal input contains exactly one `photos` array.

Each item contains exactly `photoId`, `selectionState: "rejected"`, nonempty `decisionVersion`, and `removedAtMs`, either `null` or a nonnegative integer.

The target list MUST be nonempty, distinct, and no larger than the advertised `removalPhotoIdsMaximum`. The CLI validates the complete document before network access. The service validates request-wide constraints before admitting the attempt.

A stale decision or removal marker affects only that target; valid siblings can still be removed.

`photos restore` accepts one of these forms:

| Form | Scope and identity |
| --- | --- |
| `--input FILE` | The input contains exactly one `photos` array. Items contain exactly `photoId` and the `removedAtMs` observed in a removal result or Trash listing. It restores exactly those markers under the caller-generated `OPERATION_ID`. |
| `--all` | Capture current Trash identities and markers before mutation. Apply any supplied `--exclude-input FILE` and bind the remainder to `OPERATION_ID`. Later arrivals and newer markers never join it; retry replays the original outcome. |
| Neither flag | `OPERATION_ID` names the removal operation whose complete removed set is restored. |

Use `--input` with observed markers to restore an earlier inspected selection. `--exclude-input` requires `--all` and contains `{"photoIds": [...]}` or a bare Photo ID array. `--all` and `--input` are mutually exclusive.

The explicit-set and select-all forms report the same result partition and
retain their own operation identity. `photos restore-operation` reopens that
result without changing Library state.

Removal returns one result per requested Photo in request order. Outcomes are `removed`, `changed-elsewhere`, `unavailable`, or `already-removed`.

`removed` includes a nonnegative `removedAtMs`; all other outcomes include `removedAtMs: null`. Counts contain `removed`, `changedElsewhere`, `missing`, and `alreadyRemoved`, and equal the result partition.

Successful removal does not change the Original File, Album membership, rating, or decision.

`photos removal-operation` is read-only and returns the durable result. Repeating an operation ID with identical intent replays it; different targets or evidence produce `conflict`.

A missing receipt or an incomplete or invalid response is `outcome_unknown`. Inspect the original operation before submitting a replacement.

Restore uses a separate operation ID and the removal marker as its compare-and-set identity. Its input contains exactly one `photos` array of distinct IDs with nonnegative markers.

Restore outcomes are `restored`, `already-active`, `changed-elsewhere`, or `unavailable`. Counts contain `restored`, `alreadyActive`, `changedElsewhere`, and `missing`.

Restore clears only the matching removal marker. It does not authorize Trash Record Cleanup or file mutation.

`photos restore-operation` is read-only and follows the same receipt replay and `outcome_unknown` rules. Cleaned-up identities cannot be restored; historical results MUST NOT recreate their records.

When at least one target changed and another target was conflicting or
unavailable, the CLI returns `partial_result` with the complete result in
`data`. An all-conflict result uses `conflict`; an all-unavailable result uses
`not_found`. Inspect the original operation before retrying either command.

## Trash Inspection and Record Cleanup

The CLI queries and prints Trash Locations for user-operated external deletion.
Original access remains read-only.

```text literal
slipstream trash list [--start N] [--limit N]
slipstream trash list --cursor CURSOR
slipstream --output text trash list [--start N] [--limit N]
slipstream trash empty [--operation-id OPERATION_ID]
slipstream --output text trash empty [--operation-id OPERATION_ID]
slipstream trash cleanup OPERATION_ID --input FILE
slipstream trash cleanup-operation OPERATION_ID
```

`trash list` captures ordered identities and observed removal and Original evidence. `--limit` accepts 1 through 60; `--start` chooses the first range.

Use `nextCursor` to continue the same capture or receive explicit expiry, never another live offset page. New invocations capture independently. The response publishes `cleanupMaximum`, `restoreMaximum`, `libraryRoot`, and the service namespace.

The [Trash contract](../photo-library/trash/spec.md#querying-and-printing-locations)
owns scope, totals, Locations, and namespace labeling. JSON preserves exact
characters; text escapes control characters, not shell commands. Text output
names the Library Folder and one item per line.

`trash empty` supports the human workflow of inspecting Locations, deleting intended files externally, and requesting cleanup. It captures all current Trash records through cursor listing and submits them as one cleanup with a generated operation identity. No input file or `OPERATION_ID` is required.

The identity is printed to stderr before submission. Supply it through `--operation-id` to recover or replay the same attempt after a lost response. An empty Trash succeeds with `reviewed: 0` and no mutation.

A capture exceeding `cleanupMaximum` or the service body bound is refused without truncation. Per-record outcomes and no-file-deletion rules apply; present or uninspectable Locations remain in Trash. After a lost response, inspect the attempt with `trash cleanup-operation OPERATION_ID`.

`trash cleanup` is the explicit-set alternative for Agents or a reviewed subset. Its input has exactly one `items` array; each item contains `photoId`, `removedAtMs`, `originalId`, and `originalLocation` copied from a Trash listing.

The set MUST be nonempty, distinct, and no larger than `cleanupMaximum`; input MAY be up to 1 MiB. The CLI validates the complete document before network access. The caller retains `OPERATION_ID` before submission.

Outcomes are `cleaned`, `changed`, `present`, `blocked`, `unavailable`, or `inspection-failed`; a `blocked` item names its blockers. Counts equal the result partition.

Confirmed cleanup removes application records, decisions, and Album memberships. It never deletes, moves, renames, or rewrites a file and reports no reclaimed bytes.

Cleanup revalidates the observed removal marker, Original identity, remembered Location, fresh absence, and retained dependencies. Present or changed entries, unreadable storage, unsafe paths, and retained dependencies refuse that item.

Retained workspace, Edit Recipe, Export, or Processing Artifact dependencies are blockers. Repeating an operation with identical intent returns its immutable result; different intent under that identity is `conflict`.

A lost response is `outcome_unknown` until `trash cleanup-operation` inspects the attempt. Cleanup has no Undo.

Physical-deletion commands and aliases are outside this contract. Capability
negotiation MUST distinguish this target from older services. CLI contract
version 2 retires the physical-deletion commands; a version 1 client and a
version 2 service reject each other as incompatible.

## Preview Download

```text literal
slipstream photos preview PHOTO_ID --file PATH [--size thumbnail|review]
```

Size defaults to `review`. The command obtains the current supported derivative and MAY wait for generation within the command timeout.

The result identifies `photoId`, `path`, `source`, `sourceRevision`, `width`, `height`, `detailLimited`, and `webUrl`. `source` is `jpeg-original` or `raw-embedded-jpeg`. These facts MUST describe the downloaded image; arbitrary remote URLs are not accepted.

`detailLimited` reports source capability, not viewport or zoom state. It is `true` when delivered dimensions are below the review target for the requested size, and `false` when they reach it.

The CLI MUST NOT invent a viewport or zoom condition. No CLI command accepts viewport, window, or zoom parameters. The Web MAY report a separate local display condition, which is not a CLI result.

PATH is local to the CLI host, MUST be valid UTF-8, MUST have an existing parent directory, and MUST NOT exist. Non-UTF-8 input is `invalid_input` before network access or temporary-file creation, without lossy substitution.

The transfer MUST NOT exceed the [64 MiB Preview JPEG bound](../photo-library/previews/design.md#native-library-boundary). `photos preview --help` MUST report this limit.

The command publishes a complete JPEG without replacement. Final no-replace publication is the local commit point. Before it, cancellation or failure removes only this invocation's temporary file.

After publication, failure preserves the final JPEG. Existing files and symbolic links remain unchanged. There is no overwrite flag, automatic filename, binary stdout, or stale-image fallback.

A normal success has `fileCommitted: true`. Reporting failure after publication MUST NOT claim `effect: "none"`.

When a valid JSON error can still be delivered, it retains the complete Preview result in `data`, uses `local_io_failed` with `effect: "partial"`, and exits `6`.

Otherwise the surviving process emits a best-effort stderr diagnostic that the file was published. It renders PATH as a JSON string literal, escaping control characters including newlines. This diagnostic is for a person, not a second machine protocol.

A handled interruption after publication preserves the file and exits `130` with the same best-effort distinction. Process death cannot promise either diagnostic.

After any post-publication reporting failure, the caller MUST inspect the named
path. An automatic retry cannot overwrite it and MUST NOT choose a different
path silently.

## Standard Photo Metadata

```text literal
slipstream photos metadata PHOTO_ID
slipstream photos metadata-save PHOTO_ID --input FILE
```

`metadata` reads one Photo's standard metadata without changing Library decisions or files. The result carries association status, supported field states and provenance, typed values, read-only capture facts, Library Rating, Save evidence, and `saveAvailable` with an actionable refusal reason.

Effective values use the Sidecar, then embedded XMP, then the IPTC IIM counterpart. Underlying values remain inspectable; an empty Sidecar value suppresses fallback.

`FILE` for `metadata-save` is a UTF-8 JSON document; `-` reads stdin. Read includes Removed Photos with available Originals, but Save refuses Removed Photos.

Sidecar evidence is `absent`, `present`, or `unavailable`; unavailable evidence includes a reason and cannot authorize Save. A readable XMP document with unsupported preservation constructs exposes supported fields but cannot be saved.

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
change. Metadata errors use these CLI exits:

- `invalid_input` and `unsupported_field`: `2`.
- `photo_missing`, `original_unavailable`, `association_unresolved`, and `photo_removed`: `3`.
- `evidence_stale` and `metadata_malformed`: `4`.
- `save_unavailable` and `permission`: `5`.
- `resource_limit`: `6`.
- `storage_failure` and `outcome_unknown`: `7`.

Metadata `storage_failure` has effect `none`; metadata `outcome_unknown` has effect `unknown`. These are distinct codes and outcomes. A possibly admitted Save whose effect cannot be confirmed MUST use `outcome_unknown`.

`metadata-save` writes only the associated XMP Sidecar. A response lost after
the request was admitted is `outcome_unknown`: the caller MUST re-read and
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


`field` is `selectionState` or `rating`; `value` MUST have the corresponding type and range. Each result is `changed`, `unchanged`, `conflict`, or `missing`.

Changed and unchanged results include current decisions and version; changed results also include prior decisions. Conflicts include current decisions and version; missing results contain the requested ID.

`results` retains request order and `counts` reports each outcome. A nonmatching version conflicts before no-op detection.

| Batch outcomes | Status, effect, and exit |
| --- | --- |
| Only changed or unchanged | `status: "ok"`, `error: null`, exit `0` |
| At least one changed or unchanged and at least one conflict or missing | `status: "partial"`, `partial_result`, `effect: "partial"`, exit `5` |
| Only conflict or missing, with any conflict | `status: "error"`, `effect: "none"`, `conflict`, exit `4` |
| Only missing | `status: "error"`, `effect: "none"`, `not_found`, exit `3` |

A mixed batch is partial even when its only successful outcomes are unchanged. Mixed and all-unsuccessful batches retain the complete result array in `data`.

A storage failure commits no items and returns an error, not a fabricated per-Photo partition.

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

`recovery unavailable` opens a bounded review of active unavailable Photos. `recovery propose` is read-only and returns remembered and proposed Locations, file kind, and a plain-language evidence state.

The Folder form retains relative suffixes; the single form evaluates one Original. A scope beyond the advertised bound is refused rather than silently truncated.

The review and apply commands retain opaque mapping identities and bounded
continuations for automation. These fields are opaque to Photographers; the
CLI MAY display them in structured output for an Agent or scripted workflow.
Unverified content requires explicit confirmation; an occupied destination is
never merged silently.

`PREFIX` is a Library-relative Folder Location with no leading or trailing
separator; the empty value addresses the Library Folder. `--new-location` names
one Library-relative Original Location including the filename.

A continuation accepts only `--cursor` and global options. Recovery
continuations expire after the advertised idle period and then fail as
`cursor_expired`; the caller opens a fresh review instead of assuming the
retained set.

`recovery apply` accepts exactly one JSON object containing a `mappings` array. Each entry confirms a correspondence returned by `recovery propose`.

Each mapping requires `originalId`, `newLocation`, and the reviewed proposal's `mappingId`. `confirmUnverifiedContent` is required exactly when `verified: false`, acknowledging that remembered content cannot be verified.

`retirePhotoId` is required exactly when the review identifies a `retire` candidate, and MUST name that Photo. Unknown keys, an empty list, excess mappings, duplicate Original IDs, or duplicate destinations are invalid before any write.

The service recomputes every mapping from current state. It refuses the complete batch as `recovery_conflict`, with one reason per mapping and no association changes, if any of these conditions applies:

- A mapping no longer matches its reviewed identity or a required confirmation is absent.
- The destination revision or occupant changed.
- An occupant gained state or a Trash removal.
- The Original is no longer an active unavailable Photo.

Separate invocations are separate commits; a later refusal never rolls back an earlier committed batch.

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

Album names are nonempty strings of at most 120 characters under the existing naming contract. Add, remove, and reorder input contains one `photoIds` key with an ordered array of 1 through 100 distinct IDs.

An empty Album needs no reorder command. Reorder MUST name complete current membership and refuses Albums over the bound without changes. A caller MAY append or remove successive explicit batches in a larger Album using each confirmed new version.

Album mutation results use [Result Schemas](#result-schemas). Create returns the new summary; rename reports whether the name changed. Add and remove partition every requested ID in request order.

Reorder returns the complete accepted order and whether it changed. Removal reports the resulting saved Photo ID or `null`; delete provides no dead Album URL.

All Album mutations are atomic. A missing Photo, stale version, invalid complete order, or input-limit failure commits nothing.

After local syntax and request-bound validation, an existing-Album mutation selects the first applicable failure:

1. Missing target Album.
2. Stale supplied Album version.
3. First missing requested Photo in request order.
4. Complete-membership mismatch for reorder.
5. Duplicate-name conflict.

The version guard is checked before no-op classification. Create has no target or version guard and reports a name conflict when applicable. That conflict includes the existing Album ID.

No `--force`, implicit name reuse, automatic retry, or generic `undo` command exists.

## Photo Workspace

The [Photo Workspace](../photo-development/workspace/spec.md) is a persistent real directory associated with one Photo.

Library queries supply the Photo ID once, to open that directory.

Tools then consume paths and their own state directories.

Ordinary same-host editing requires no download/upload/register/commit loop.

A completed file is visible through reconciliation even when produced by an independent tool with no Slipstream integration.

Workspace command names, arguments, and options are discoverable through `slipstream workspace --help` and command-specific help. The [workspace parser](../../crates/slipstream-cli/src/workspace.rs) owns the exhaustive grammar.

Open creates or reopens the Photo's default workspace idempotently, using an independent read-only input snapshot rather than a writable link to the Original.

[Target workspace schemas](../../schemas/cli/workspace.schema.json) describe the specified carriers. Open reports the actual absolute directory and execution host/access facts.

Both input fields are null when no default input remains; reopen does not recreate it, select another file, or read the Original.

Explicit refresh prepares an input again.

Local/shared accessibility MUST be established, never inferred from a returned path.

Unsupported remote execution is explicitly unavailable; the CLI does not create a remote shell, mount, checkout or synchronization protocol.

Callers run independent tools on the disclosed host/environment with filesystem access, or use an available supervised integration.

Open also returns suggested private edit-state paths.

These are convenience paths, not required tool metadata or an initialized edit.

Only the initial input image is visible until a tool writes a result; directories are created lazily.

Show reports current directory/access facts and reconciled bounded file groups, including missing/checking/changed evidence; it returns `observedSourceRevision: string|null` and an opaque `workspaceStateToken` binding the complete current retained content/dependency inventory.

Its `deletionSummary` contains input/preview/output/resource/other-file/edit-state counts, total known bytes, any external dependencies, and active operations, plus a bounded named sample/continuation for inspection.

Reopen never retargets saved edits to changed Original bytes.

Refresh input accepts the exact bounded JSON document in the target schema rather than embedding source evidence in a command-line token.

The value comes from show; missing/unavailable Original evidence refuses refresh without damaging existing files.

Admission checks that observed Library revision and copies/reflinks through the guarded source descriptor, verifying stability before creating a unique protected input version.

Success returns workspace identity, request identity, outcome (`created`, `unchanged`, `replayed`), input artifact/path/revision and fresh workspace token.

An intact already prepared identical source revision MAY be reused; an explicitly removed identity MUST NOT be resurrected, including for identical source bytes.

Explicit refresh establishes its returned input as default if none exists, without changing any saved tool/downstream binding.

Request IDs default to generated returned values; explicit exact replay returns the original preparation result with separate current availability rather than recreating a subsequently removed file.

A new refresh intent after removal needs a new request identity.

Possibly admitted outcomes remain unknown until exact replay reconciliation; a source change or storage failure publishes no new ready input.

Refresh result identifies the prepared input and its current availability.

Replay preserves the settled input identity/path/revision/token; currentAvailability separately reports its current usability.

Replay never prepares removed bytes or reuses a deletion receipt as new creation intent.

Whole-workspace delete requires the token from the reviewed show/deletion summary, an explicit request identity and `--confirm`; no interactive prompt or `--force` bypass exists.

The token binds the full loss inventory, so new files/state/dependencies invalidate approval.

Active operations or references outside the workspace refuse deletion with their identities; caller MUST settle/rebind/remove them explicitly first.

Under workspace admission coordination, delete atomically removes its usable association/visibility and records a durable deletion receipt before reclaiming only contained owned storage.

Failed reclamation is disclosed and retriable from that receipt rather than claiming bytes are gone.

Exact replay returns the same deletion result; a stale token has effect none.

It never deletes a Library Original/sidecar or changes Library decisions. `--confirm` expresses explicit caller intent, not proof of human authorization.

Files uses the common bounded list/cursor envelope, default 50 and maximum 60, and reconciles completed regular files after missed events/restart.

Private tool state/temp files are excluded.

File items carry the identity, revision, relative path, role, state, byte evidence, image contract, and Web URL described by the target schema. The verified `sha256` and `imageContract` fields are nullable.

An unknown format remains a generic file.

Folder/extension MAY suggest a role, but cannot fabricate successful processing provenance or next-tool compatibility.

Inspect returns full verified file evidence, nullable tool/input/state-revision provenance, dependency references and availability.

A preview's bounded display contract MUST NOT be represented as high-precision downstream input.

Download pins the named or freshly observed current completed revision, verifies length/digest and any asserted image contract, and publishes to a caller-local new path without replacement.

In-place external changes invalidate old revision evidence and fail a pinned transfer; no old revision MAY continue with new bytes.

Remote final delivery is optional; same-host callers read a completed file directly.

Remove accepts an explicitly selected protected input snapshot: protection means read-only to tools, not permanently undeletable.

An Agent MUST supply its observed `--revision`; a direct human action MAY atomically observe current evidence.

Under the existing workspace admission coordination and tool-use locks, removal checks the revision and refuses current/retained state, captured jobs, kept-output/dependency references, active use, or unverified external use, naming each blocker.

New binding/use admission shares that coordination, so it either pins the file before removal (which refuses), or sees the removed identity and fails.

Success returns the settled removal and its default-input facts. Exact replay MUST return those original settled facts, not later inventory; use workspace show for current facts.

Removing the default clears both input fields even if another input exists; removing the last leaves truthful no input.

Reopen does not recreate/retarget/read Original; explicit refresh MAY prepare a new default.

Removal never changes Library Original bytes or discards unrelated edits/results.

## Photo Development

Camera Preview (`photos preview`) remains independent. [darktable CLI Reference](../photo-development/darktable/cli.md) owns standalone semantic grammar and the optional `slipstream darktable` same-directory adapter.

Native parsing remains private. [Photo Workspace Architecture](../photo-development/workspace/design.md) owns indexing, file revision evidence, dependency-safe removal and non-destructive migration.

Independent peer tools use their own documented file/state CLI, with a completed workspace file as explicit input and the same directory as output.

Their names are independent of Slipstream.

No generic server recipe DSL is required to compose them: choosing a compatible output path for the next tool is the handoff.

There is no implicit conversion, automatic chaining, upstream rerun or Library import.

Slipstream MAY discover/supervise qualified tools, but cannot require peer tools to depend on its IDs, HTTP protocol, token or database.

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

These adapter commands cannot create a second parameter source of truth or require a recipe before ordinary file processing.

Recipe/step IDs project canonical on-disk tool states and retain old identities; `get` preserves unsupported historical intent read-only.

Non-darktable writes preserve every existing darktable state and selection and refuse to mutate/execute one; darktable work stays under the accepted full namespace.

Tools without a qualified semantic adapter remain usable through directory handoff, without fake generic control support.

### Retained Processing Adapter Exchanges

Target processing carriers are defined by these schemas:

- [Recipe and input](../../schemas/cli/processing-recipe.schema.json).
- [Export and Artifact](../../schemas/cli/processing-export.schema.json).
- [Preview and discovery](../../schemas/cli/processing-discovery.schema.json).

Results appear as `data` in the [Output Envelope](#output-envelope). These schemas describe the target, not a runtime conformance claim.

Every named key is required unless marked optional; nullable keys MUST be present. URLs MUST be ordinary credential-free Web URLs.

These contracts project supported non-darktable canonical tool state.

They do not define a second darktable language.

Generic writes/execution targeting darktable refuse and direct the caller to its full namespace; unrelated canonical states and their selections MUST survive every supported adapter write.

#### Bindings and Shared Types

An `ImageContract` contains verified actual image facts admitted by the selected module, not filename guesses. `Parameters` carries the complete module-owned tree; discovery owns its schema and output choices.

Names are nonempty and at most 128 UTF-8 bytes. Image edges are 1 through 65536 pixels.

Step/module IDs are at most 64 bytes, other IDs/revisions 128 bytes, source evidence 16384 bytes, steps 64, parameter nesting depth 32, and complete mutation JSON 64 KiB.

Request IDs are 1 through 128 ASCII alphanumeric, dot, underscore or hyphen bytes; Photo IDs retain the existing Library identity format.

Other bounded names have no surrounding whitespace or control characters.

Unknown/duplicate outer keys refuse; module trees follow their advertised schema.

Step IDs are distinct. currentStepId MUST name one step when steps exist and MUST be null for zero steps.

Bindings select exactly one of these sources:

- `original`: exact evidence for the same retained Original, never the default workspace input. A qualified migration MAY use an inspectable protected capture of that exact revision; otherwise execution requires the unchanged available Original.
- `artifact`: the existing immutable Processing Artifact. Admission verifies retained bytes and exact image contract; a missing or expired artifact cannot be replaced by a same-name file.
- `workspace-artifact`: one explicit completed workspace image revision. Admission verifies containment, stable bytes, and actual compatible image facts under [Stable Bytes and Containment](../photo-development/workspace/design.md#stable-bytes-and-containment).

An unknown workspace-artifact producer or provenance remains unknown. Path spelling and the current default cannot retarget its binding.

Development Proxy is a disposable display cache, not a fourth binding kind, protected workspace input, or full-resolution Film input.

A legacy Export ID is not a Processing Artifact ID: historical export metadata/bytes remain readable through historical commands, and a verified completed image discovered in the workspace can be explicitly bound with `workspace-artifact`.

Neither path invents missing historical provenance.

Preview contracts cannot masquerade as full-resolution compatible inputs.

`sourceRevision` at the read-result level is the observed live Library source evidence, with the existing string carrier: `""` means unavailable/no published evidence.

A stored recipe's sourceRevision is its captured source guard and MAY also be empty for workspace-only intent.

Save/export MUST echo the read observation, including empty string; the service MUST NOT refresh the supplied value.

For exclusively artifact/workspace-bound supported changes, source evidence is contextual: a subsequent unrelated live Original change or loss does not conflict or require a source read, and intact bound workspace bytes remain usable.

Original-dependent admission still verifies the bound nonempty evidence or exact migrated protected capture; it never silently refreshes/rebinds it.

Exact replay uses retained request identity before any fresh source/state admission.

#### Recipe Exchanges

The target schema defines `Recipe` and its get result.

A null recipe means absent; a nonnull recipe with `steps: []` and `currentStepId: null` is a saved empty recipe with its own revision.

Unsupported retained module/version trees remain readable verbatim and refuse execution/mutation.

Optional `executionRefusals` is read-only refusal metadata, not a save key or a substitute for retained intent.

Construct the save input from get by copying `sourceRevision` to `expectedSourceRevision` and `recipe.revision` (or null if absent) to `expectedRecipeRevision`. Copy the retained currentStepId and steps (or null and an empty array), add a new requestId, and modify only intended supported state.

Never submit Recipe's photoId/revision or diagnostic keys as save keys.

Null expectedRecipeRevision asserts absence; it cannot overwrite a saved empty recipe.

Under canonical state locks, the projected revision binds every supported state being replaced.

Preserve unrelated/unknown states; refuse an unqualified multi-state atomic change rather than partially saving.

A mismatched guard conflicts before no-op detection and is never refreshed by the adapter.

Unchanged retained steps outside the supported write scope, including darktable, MAY be echoed for a read-derived complete save. They are compared and preserved verbatim; omission MUST NOT delete canonical state.

Changing or removing their parameters or bindings, or changing their tool selection, refuses the save. Selecting an existing Film step changes only the generic adapter selection, not the separate darktable edit selection.

Removing a supported step removes only that adapter's canonical state after dependency checks, never another tool's state.

Auto adds an `automaticAdjustment` to the complete save input.

These existing wire names remain unchanged; only a selected qualified non-darktable adapter's advertised action MAY execute.

Instruction follows its schema, multiPriority is 0 through 128, and stepId MUST be current.

An Original-only advertised action requires an Original binding.

Unsupported actions, including old darktable exposure/channelmixerrgb actions, refuse here and use the darktable namespace.

Rebind uses the exact input in the target schema.

It explicitly selects an already prepared protected input matching that observed nonempty source revision for eligible Original-bound supported states; it does not copy/change the Original, prepare an input implicitly, change artifact/workspace bindings, or mutate darktable/unrelated state.

If no supported Original-bound state is eligible, the command refuses without changing state.

Coordinates/resources/settings MUST remain valid or the complete request refuses.

A selected workspace-artifact is changed only by guarded save of its explicit new Binding.

The returned `recipeVersion` equals the committed `recipe.revision`. `sourceRevision` echoes the supplied expected or new source revision.

Replayed returns the originally settled recipe/version even if a later write changed current state; obtain a fresh get before new intent.

Request reuse with changed payload conflicts.

A no-op keeps the revision; an effective save changes it.

Recipe revisions include canonical on-disk state changes, so an external writer invalidates old evidence.

A state conflict returns `data: null`, `error.code: "conflict"`, `effect: "none"`, and `details: {photoId, currentRecipeVersion: string|null}`; the existing currentRecipeVersion name refers to the projected revision.

The caller keeps its intended draft and explicitly reconciles with a fresh read.

#### Export, Artifact, and Historical Results

Export requires its selected step to be current.

It captures immutable canonical state, bound bytes, resources, engine/bundle and the module-tree output choice before queue admission.

Retry uses a new request ID and targets a retained failed or cancelled request.

It reuses that captured snapshot, not current settings/default input/Original.

Missing resources or bytes refuse truthfully.

Only accepted supervised work continues after disconnect; independent synchronous processes do not gain that guarantee.

Work timestamps use Unix seconds. A null `retainUntil` means no scheduled receipt expiry.

Empty sourceRevision is permitted as above.

Receipts retain immutable input evidence/resources internally; exposing input does not authorize retargeting them.

Artifact timestamps are UTC. Its `webUrl` is optional except in artifact inspection.

Filename is a basename with no separators/control characters.

Workspace-backed artifact expiresAt is null until explicit safe removal; historical immutable retained artifact expiry remains its real time.

Processing provenance MUST be verified to construct Artifact; a newly discovered unknown-provenance image uses workspace inspect instead.

Its eligibility as an input does not fabricate an Artifact of its producer.

Acceptance returns the captured Work receipt; already completed success returns the Artifact.

Replay preserves captured identity/terminal result, even when it differs from current state.

A retained failed/cancelled export replay returns status error with `data: Work`, effect none and its retained failure/terminal conflict, not new execution.

Status returns Work with a Web URL. Cancel settles the first terminal outcome and returns Work unless already succeeded, when it returns the published Artifact.

Cancellation cannot undo a published result.

Each Export-list array is bounded to 64 and ordered newest first by its acceptance, publication, or creation time. Identity ascending breaks ties.

The bound limits the recent window, not retention; exact-ID status and inspection remain available. Artifact download dimensions come from `outputContract.geometry` and the transfer pins verified bytes.

Historical Export and Artifact times are UTC. A historical artifact is nullable even for a succeeded Export when its retained metadata is missing.

Historical Artifact target MUST match its Export target. Target fixes the corresponding stage and content type. A terminal outcome MUST equal the terminal state.

Historical download identifies its result as historical.

Missing/expired bytes refuse, without inventing composable provenance.

#### Preview and Discovery Results

Preview captures the currently selected saved step under its state lock; it does not save/rebind.

A pending result's `PreviewHandle` identifies captured rendition provenance in workspace inspection. It is neither a new command nor a reusable current-state guard.

A subsequent preview invocation observes the then-current saved step and reuses matching work when available.

A newer edit changes current identity; an old captured result remains inspectable in workspace files but cannot be returned as current or replace the selected preview.

The caller repeats the same command for readiness without an implicit CLI polling loop.

Ready results identify the captured PNG and its Preview provenance.

Scale is positive and at most 1; dimensions describe the actual bounded display PNG, not the full output.

The identity binds captured state/input/bundle/output/display conversion. sourceRevision MAY be empty for workspace-only work.

Transfer validates exact captured evidence, PNG structure, digest, and the 64 MiB bound before no-replace publication.

Pending never creates the requested local file; no camera/baseline image substitutes for requested processing.

Module discovery results are ordered by module name. A discovery image contract with zero width or height is unconstrained subject to finite module limits, not a zero-sized bound image.

Discovery and Binding image carriers keep their distinct existing names; derive transfer/precision/encoding from the selected module's advertised semantic constraints and verify actual bytes.

Availability alone does not establish compatibility.

Darktable MAY be discovered, but generic semantic execution remains unavailable and directs to its full namespace.

Artifact download streams incrementally, validates strong revision/length/SHA-256 and asserted image structure/contract, and publishes without replacement.

Private resumable state uses `PATH.slipstream-part` and `PATH.slipstream-part.json`; matching state is rehashed before one contiguous range resumes.

Mismatched state or ignored ranges restart safely; expired/missing artifacts remove only owned partial state.

In-place workspace changes fail the pinned revision rather than mix bytes.

Headers bind actual digest, length, filename and revision; historical download validates retained metadata without inventing composable evidence.

File metadata does not carry credential-bearing URLs or raw image bytes.

Development errors retain invalid-settings/input (exit 2), missing identity/state/file (exit 3), stale state/source/request or terminal conflict (exit 4), and unavailable execution/resources, exhausted capacity, disk-full or expired historical data (exit 6).

Possibly admitted writes retain effect `unknown`, exit 7, and exact replay guidance.

Confirmed refusal has effect `none`.

None changes Library decisions/Albums, refreshes a supplied guard, silently substitutes resources, or submits dependent work after uncertainty.

## Output Envelope

In JSON mode, every operational command emits one JSON document followed by a newline on stdout. Diagnostics and progress use stderr. Help and version remain plain text.

Text mode emits one human-readable result on stdout. It has no parsing contract, but uses the same effects and exit codes and MUST identify partial or unknown outcomes.

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

Envelope keys are always present. `status` is `ok`, `partial`, or `error`; `data` contains the command result shape or `null`.

`error` is `null` when status is `ok`. Otherwise it contains `code`, `message`, `effect`, and `details`. `message` gives a concrete next action but is not a parsing key.

| `effect` | Meaning |
| --- | --- |
| `none` | No requested effect committed and no requested item completed successfully. |
| `partial` | At least one requested effect committed or item completed successfully, but the command did not complete normally. Confirmed data remains in `data`. |
| `unknown` | Admission could have occurred but the response cannot prove the outcome. |

A changed or unchanged Photo result is a successful item outcome; a conflict or missing result is not.

Unless a shape says otherwise, every named key is required. An opaque ID or version is a nonempty string. A count is a nonnegative JSON integer.

A time is an RFC 3339 UTC string. Nullable keys are present with either the stated type or `null`. Arrays preserve the stated order.

## Result Schemas

[Query result schemas](../../schemas/cli/results-query.schema.json) and [mutation result schemas](../../schemas/cli/results-mutations.schema.json) define the common carriers. They describe the target, not a claim that current runtime responses implement it.

`ScanStatus.updatedAt` is the last scan progress time in milliseconds since the Unix epoch, or `null` before activity in this process. `updatedMs` is the most recent scanner progress time, or zero before a scan starts. Phase counts are `null` when they cannot be reported.

A failed Library check retains `LibraryCheckData` with `status: "error"` and `library_unavailable`. Folder continuations have no time deadline; Album, Photo, and Recovery lists expose an expiry exactly when a continuation exists.

`PreviewFacts.detailLimited` describes source capability, not a display measurement. It compares delivered dimensions with the review target, independent of viewport, Preview area, or zoom. All nullable Preview facts are non-null when ready and null when no current derivative facts exist.

`PhotoItem.originalId` and `location` identify the Original and its current Library-relative Location. `photos get` adds metadata; its values are facts only when inspection state is `known`, and absent facts remain null. `photos resolve` returns one `PhotoItem` and its `id` is the editing Photo ID.

Preview download facts describe the published bytes. Its `detailLimited` compares actual dimensions against the requested size's review target and has no viewport or zoom meaning.

Photo decision and removal results preserve request order. Result counts MUST equal their partition; operation inspection returns the same result without changing Library state.

Album add and remove results partition submitted IDs without duplication and preserve request order within each array. Reorder returns the complete accepted order. A returned Album summary carries the post-command version; a no-op keeps the observed version.

A missing `RecoveryItem` retains its last reviewed facts. `fingerprintEnrolled` reports persisted content evidence, and a missing fingerprint MUST NOT be presented as a content match.

`RecoveryMapping.verified` is true only when persisted evidence matched the destination. `blockedReason` is null exactly when the mapping can be applied. `retire` identifies a permitted Retire and Bind candidate.

`mappingId` is opaque and binds confirmation to the Original, proposed Location, content evidence, destination revision, and any Photo to retire. It is not a Photo, Original, or operation identity; clients MUST NOT construct or reuse it from another review.

Recovery `total` is the complete retained review size, not the page size. A single-mapping review returns one item, `total: 1`, and no continuation.

Recovery review pages, evaluated mappings, and apply batches use their advertised capability maxima. A Folder-prefix scope exceeding its maximum is refused as `recovery_scope_exceeded` before a continuation is published.

Continuations retain the original page bound and expire on idle, restart, or bounded least-recently-used eviction. The caller opens a fresh review after expiry.

## Error Details

[Target error schemas](../../schemas/cli/errors.schema.json) define common error-detail carriers by code. [Photo Development](#photo-development) and [darktable Errors](../photo-development/darktable/cli.md#darktable-errors) own their additional codes.

Development details use shared route recovery facts. Conflict-family errors MAY include nonempty `currentSourceRevision` and nullable `currentRecipeVersion`; request conflicts, missing recipes, and unsupported Photos MAY include `photoId`.

Invalid settings MAY include `argument` and `reason`. Unavailable processing MAY include `photoId`, `operation`, or `reason`; Export routes MAY return empty details.

An incompatible capability shape includes both `reason` and nullable `field`. Recovery conflict rejections contain one item per refused mapping.

`trash_cleanup_partial` includes the complete per-item result. No cleaned item with at least one refusal is `conflict`; all unavailable targets are `not_found`.

Workspace and darktable operation names use their actual command paths joined by hyphens. Common envelope and error rules also apply to darktable operations.

`artifact_changed` and `artifact_referenced` are confirmed refusals with exit 4 and effect `none`.

Blocker names and recovery actions remain useful when external use has no service identity. `workspace_unavailable` and `execution_unavailable` disclose host/access facts and recovery, exit 6.

Uncertain workspace open/remove additionally retain nullable `workspaceId`, `artifactId`, and `requestId`; a lost open is reconciled by reopening the same Photo's default workspace, while removal uses exact retained request identity.

Workspace reads/downloads do not claim unknown semantic writes.

Whole-workspace deletion returns `workspace_changed` for a changed loss inventory and `workspace_referenced` for blocking dependencies or independent use. Both are confirmed refusals with exit 4 and effect `none`.

The caller reviews the inventory again or settles the named use. A malformed required field is not evidence of confirmed refusal. After mutation admission, the CLI reports `outcome_unknown`, exit 7, retaining the submitted request identity without retrying.

Supervised darktable `invalid_mapping` is a confirmed invalid-command refusal, exit 2 with `effect: none`; its details MAY be empty.

`photoIds` in `outcome_unknown` contains all submitted Photo IDs in request order. Album fields identify the submitted target when one exists and are null otherwise.

Recipe writes, Export submission, and Recovery application can have unknown mutation outcomes. Module discovery, recipe reads, Recovery reads, and downloads use read or transfer errors and do not claim a saved mutation.

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

A complete Photo batch uses `partial_result` for mixed outcomes. An all-unsuccessful batch uses `conflict` when any item conflicts and otherwise `not_found`; details identify the first such item in request order. Complete item data remains available.

An HTTP response with an invalid body or required fields is not evidence of write failure. An invalid read response uses `transport_failed` with effect `none`. A write or scan whose request could have been admitted uses `outcome_unknown`.

Exit codes are:

- `0`: confirmed success, including an empty list or unchanged mutation;
- `2`: invalid command, input, or request limit;
- `3`: confirmed missing object;
- `4`: confirmed conflict or name conflict, with no changes;
- `5`: a confirmed partial Photo batch;
- `6`: authentication required, access denied, unavailable service, expired cursor, incompatible contract, busy service, failed scan/Preview, local I/O, output failure, or confirmed `storage_failed` transaction rollback.
- `7`: an unknown write or scan outcome, or metadata `storage_failure` as specified under [Standard Photo Metadata](#standard-photo-metadata).
- `130`: interruption. If a write could have been admitted, a best-effort JSON
  error uses `outcome_unknown`; abrupt process death cannot promise output.

For an all-unsuccessful Photo batch, use exit `4` if any conflict exists, otherwise `3`. Syntax and limits are checked before network mutation.

Transport failure known to precede sending is `transport_failed` with `effect: "none"`. Uncertainty after sending is `outcome_unknown` with `effect: "unknown"`.

Failed JSON serialization or stdout after a confirmed server commit cannot roll back that commit. Failed reporting after local Preview publication follows [Preview Download](#preview-download).

Consumers MUST validate `schemaVersion`, status, and required fields. Version 1
MAY add optional fields; it MUST NOT remove required fields or change their
types or meanings. Unknown future error codes MUST be treated as failures.

## Composition

Multiple commands are separate operations, not an atomic task.

After a lost Album creation response, an exact-name lookup reveals present state but MUST NOT be treated as proof of who created it.

A concurrent membership change makes the next versioned addition conflict.
