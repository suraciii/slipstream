# Capture-Time Library Ordering

Slipstream needs camera capture ordering without discarding explicit Album sequence, inventing timezone facts, or letting a rescan reorder a source that the Photographer is already browsing.

Capture inspection MUST NOT unpack RAW sensor data or generate a Preview.

A malformed Original MUST NOT invalidate valid sibling Photos.

## Order Ownership

`All Photos` owns Capture Time order.

An Original Folder filters that order by Folder ancestry.

An Album owns explicit membership order.

Opening a source copies its ordered Photo IDs into a hidden Browse Snapshot.

A rescan MAY change a newly opened Library or Folder source but MUST NOT reorder an open Snapshot or Album positions.

## Original Capture Fact

Each Original owns one derived capture fact:

- inspection state: `pending`, `known`, `missing`, `invalid`, or `failed`;
- optional fixed-width camera-local ordering key;
- optional selected EXIF base field;
- optional timezone offset in minutes;
- the source revision inspected.

| State     | Meaning                                                                                                                             |
| --------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `pending` | The current Original revision has not been inspected.                                                                               |
| `known`   | The fact has a valid ordering key.                                                                                                  |
| `missing` | No supported base field was present.                                                                                                |
| `invalid` | Supported base fields were present but malformed.                                                                                   |
| `failed`  | An admitted, confined read could not produce a trustworthy result because confinement, I/O, parser, or revision enforcement failed. |

Native-work admission is not an inspection outcome. When shared capacity cannot admit an attempt, the fact remains retryable `pending` for that scan; capacity exhaustion MUST NOT be recorded as `failed`.

A failed fact remains eligible for retry.

Missing and invalid facts are reused while their source revision remains unchanged.

A relocated Original File resets to `pending`.

Its capture facts are re-derived from its current bytes at the new Location, so a value retained from the old Location never orders the Photo.

## Metadata Semantics

Inspect base fields in this order:

1. EXIF `DateTimeOriginal`;
2. EXIF `DateTimeDigitized`.

A malformed higher-priority field does not block a valid lower-priority field.

For the selected base field, use only its matching subsecond and offset companions:

- `SubSecTimeOriginal` and `OffsetTimeOriginal`; or
- `SubSecTimeDigitized` and `OffsetTimeDigitized`.

Normalize a valid camera-local value to:

```text literal
YYYY-MM-DDTHH:MM:SS.fffffffff
```

Missing or malformed subseconds contribute zero.

Use at most the first nine decimal digits and right-pad shorter values with zero.

Retain a valid offset as signed minutes, but do not apply it to the ordering key.

A missing or malformed offset remains unknown and does not invalidate the base value.

Do not use EXIF `DateTime`, GPS time, filesystem modification time, filenames, XMP, Preview metadata, or another Original as a guessed capture fact.

For a supported RAW container without a TIFF header at byte zero, a narrow LibRaw metadata-only fallback MAY use `raw->other.timestamp`.

It is converted with `localtime_r` in the inspecting process and normalized as a camera-local `DateTimeOriginal` value with zero subseconds and no offset.

The fallback never runs for a TIFF container, including a TIFF container with missing fields, so direct EXIF remains exact.

It calls only LibRaw open on a `/proc/self/fd` alias of the retained descriptor; it never unpacks, develops, or reads Preview pixels.

## Photo Capture Fact

A Photo's authoritative ordering key comes from its own Original File's capture fact.

A Photo has no fallback to another Original File.

A Photo whose own Original has no known key sorts in the missing-time partition until a current inspection produces one.

A relocated Original returns to `pending`, so its Photo returns to the missing-time partition until its current bytes are inspected.

## Deterministic Library Order

Filtered Library order is:

```text literal
capture key present before capture key absent,
authoritative capture ordering key by UTF-8 bytes,
Photo ordering Location by UTF-8 bytes,
Photo ID by UTF-8 bytes
```

The Photo ordering Location is the Photo's own Original Location.

Album queries continue to order only by `album_members.position`.

Original Folder sources preserve the relative order of the filtered `All Photos` sequence.

## Confinement and Resource Bounds

Metadata inspection begins from the Library-owned Original capability.

It opens the Original read-only beneath the retained Library Folder descriptor and passes an already-open descriptor or borrowed read/seek adapter to the metadata parser.

The parser MUST NOT receive an Original filesystem path or reopen the Original by name.

Direct parsing uses bounded reads from the retained descriptor.

The LibRaw fallback receives only a `/proc/self/fd` alias of a duplicated retained descriptor.

The first attempt compares the opened descriptor with the discovery device, inode, size, and modification time.

Each attempt checks that its retained descriptor is unchanged after inspection.

A discovery mismatch or a mid-read revision change discards that attempt's metadata; parser failure MUST NOT hide a revision change.

Traversal and symlink escape remain forbidden.

After a changed-revision failure, Capture inspection makes one fresh observation at the same Location.

It opens one new confined read-only descriptor and obtains the observation's source facts, source revision, and metadata through that descriptor.

A completed `known`, `missing`, or `invalid` result replaces the scan's discovery facts and capture fact together, before recovery planning or persistence.

It MUST NOT attach fresh metadata to stale discovery facts.

A replacement already complete before this observation is the current file, just as in a later ordinary scan; the rejected attempt contributes no metadata.

A change during the fresh descriptor's inspection still fails its revision check.

There are at most two attempts per Original per scan.

The fresh attempt MUST NOT reuse a prior capture fact, change Location, infer identity or a Content Fingerprint, or start another scan.

If it fails, the scan retains its discovery facts and records `failed` without a source revision or ordering key; a later scan MAY try again.

Other first-attempt errors keep their existing failure behavior.

Both attempts use the same Library native-work permit, and each keeps the existing parsing limits.

When the shared budget cannot admit an inspection, the scan records a retryable wait state instead of failure. The next admitted scan MAY retry it without a restart, and the Published Library keeps the prior published facts.

A saturated queue MUST NOT publish null, stale, or `failed` facts over known published facts, and
recovering one deferred Original MUST NOT require a full restart.

Metadata input, parser allocation, blocking workers, and queued work are bounded.

Each Library owns one capacity-two native-work budget shared by Capture inspection, Preview extraction, and derivative processing; standalone cache schedulers MAY own an independent budget.

It MUST NOT unlock LibRaw sensor unpack, demosaic, or any RAW development path.

## Scan and Browse Lifecycle

A completed scan is the only event that advances the Published Library, and
its publication is atomic: discovery facts, source facts, source revision,
capture fact, and derivative eligibility change together.

A deferred, failed,
or interrupted inspection or scan publishes nothing and leaves every prior
published metadata fact, source revision, recipe binding, Preview fact, and
processing eligibility intact.

An empty or failed read MUST NOT overwrite a
known published fact.

A completed scan atomically publishes one Library snapshot.

An unchanged Original reuses its persisted fact when the inspected source revision matches.

After storage and state admission, an existing completed Published Library MAY remain browsable while an ordinary replacement scan runs in the background.

An ordinary startup MAY serve a compatible persisted completed snapshot while a background rescan builds its replacement.

A new state store MUST report initialization progress and finish its first scan before Browse Snapshots are available.

After a v2-to-v3 migration, available Originals begin as `pending` and are inspected before the first v3 ordered snapshot is published.

This one-time backfill MAY lengthen the first upgraded initialization, but it prevents the server from publishing temporary path order that changes as metadata work completes.

Preview generation remains demand-driven.

An unavailable Original retains its last completed capture fact.

When it becomes readable with a changed revision, the result from the current bytes replaces the retained fact, including `missing`, `invalid`, or `failed`; an old key MUST NOT remain authoritative for changed bytes.

A per-file capture failure does not abort valid sibling Photos.

A root-level scan or persistence failure leaves the previously committed snapshot authoritative.

An open Browse Snapshot keeps its ordered Photo-ID sequence. Availability, Selection State, Rating, and Preview facts MAY refresh, but a rescan does not insert, remove, or reorder that Snapshot.

Reopening `All Photos` MAY observe the newly published order.

When explicit Album Resume resolves an unavailable saved Photo, Slipstream searches later members and then wraps once for an available member.

If every member is unavailable, it remains at the saved member.

Browse creation without a saved position still resolves its initial position to the first available member or, if none are available, the first member.

That value is advisory for Snapshot creation: it is not written as the Album's durable saved position, and Resume availability follows only the durable position.

It does not force a Grid destination into Photo View.

Grid entry and Resume presentation follow [Library Browser Experience](../library-browser/experience/spec.md#destinations-and-browser-history).

A disconnected Album keeps mutation controls disabled until its required current facts and saved position are confirmed. `All Photos` and Original Folder sources have no durable position and reconnect through their current bounded windows.

## Persistence

SQLite schema version 3 adds these columns to `original_files`:

```text literal
capture_metadata_state TEXT NOT NULL DEFAULT 'pending'
capture_order_key TEXT NULL
capture_time_field TEXT NULL
capture_offset_minutes INTEGER NULL
capture_source_revision TEXT NULL
```

The canonical schema constrains:

- state to `pending`, `known`, `missing`, `invalid`, or `failed`;
- known ordering keys to the fixed-width normalized format;
- field authority to `date-time-original` or `date-time-digitized`;
- offsets to the application-supported range from `-840` through `840` minutes; syntactically valid values outside that range remain unknown.

Application validation enforces:

- `known` has a key, field, and source revision;
- `missing` and `invalid` have a source revision but no key or field;
- `pending` has no derived fact;
- `failed` has no authoritative key and remains retryable.

Do not add a winning timestamp column to `photos`.

Query and domain mapping derive the Photo's ordering key from its single Original File row.

The v2-to-v3 migration adds only derived metadata columns and changes `PRAGMA user_version` to `3` in one admitted `BEGIN IMMEDIATE` transaction.

It preserves every existing row, identity, membership position, Selection State, Rating, Preview fact, and saved virtual-group position.

The later v4-to-v5 Album migration preserves those same values while adopting canonical Album names, and the v5-to-v6 independent-Photo migration preserves every Original File's capture facts, including paired inputs whose RAW and JPEG become independent Photos.

Exact canonical-shape validation remains mandatory.

## Rollback

Before the first v3 startup, the operator creates a consistent, sidecar-free v2 backup while the production service is stopped or through an equivalently proven snapshot.

A v2 binary rejects v3 as a newer schema.

Rollback stops the v3 process and restores the pre-upgrade v2 backup.

There is no in-place down migration.

Original Files and rebuildable cache bytes require no conversion and remain untouched.

## Protocol

The browser obtains deterministic order through the hidden bounded Browse Snapshot protocol in [Scalable Library Browsing](../library-browser/design.md).

The protocol MUST NOT expose one unbounded complete-Library response.

Library and Original Folder Snapshots use Capture Time order; an Album Snapshot uses explicit membership position.

Browse Windows do not expose Capture Time, offset, or inspection state in every Grid item.

Photo View MAY request the bounded review
metadata view defined by [Scalable Library Browsing](../library-browser/design.md),
which exposes Capture Time for the current Photo when available.

Capture
failures MUST NOT disable selection, Rating, navigation, or Preview behavior.
