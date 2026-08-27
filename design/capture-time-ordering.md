# Capture-Time Review Ordering

Slipstream needs camera capture ordering without discarding explicit Photo Set sequence, inventing timezone facts, or letting a rescan reorder an active Review Session.

## Design Drivers

- Existing SQLite v2 state persists explicit Photo Set membership positions and Photo-ID review progress.
- Camera files often omit timezone and subsecond metadata.
- A RAW/JPEG pair may contain missing, invalid, or conflicting metadata.
- Every Original read remains descriptor-confined and read-only.
- One malformed Original must not invalidate valid sibling Photos.
- A completed Library snapshot must have one stable order. Metadata inspection must not reorder it progressively after publication.
- Capture inspection must not unpack RAW sensor data or generate a Preview.

## Order Ownership

A filtered Photo Library query owns Capture Time order. A Photo Set owns explicit membership order. A Review Session snapshots the ordered Photo IDs supplied by its source when the Session starts.

This keeps one owner for each order:

- `GET /api/photos` returns filtered Library order;
- `GET /api/photo-sets` returns members by persisted membership position;
- a rescan may change a future filtered Library Session but never an active Session or Photo Set positions.

## Original Capture Fact

Each Original owns one derived capture fact:

- inspection state: `pending`, `known`, `missing`, `invalid`, or `failed`;
- optional fixed-width camera-local ordering key;
- optional selected EXIF base field;
- optional timezone offset in minutes;
- the source revision inspected.

`pending` means the current Original revision has not been inspected. `known` has a valid ordering key. `missing` means no supported base field was present. `invalid` means supported base fields were present but malformed. `failed` means confinement, I/O, parser, revision, or resource enforcement prevented a trustworthy result.

A failed fact remains eligible for retry. Missing and invalid facts are reused while their source revision remains unchanged.

## Metadata Semantics

Inspect base fields in this order:

1. EXIF `DateTimeOriginal`;
2. EXIF `DateTimeDigitized`.

A malformed higher-priority field does not block a valid lower-priority field. For the selected base field, use only its matching subsecond and offset companions:

- `SubSecTimeOriginal` and `OffsetTimeOriginal`; or
- `SubSecTimeDigitized` and `OffsetTimeDigitized`.

Normalize a valid camera-local value to:

```text literal
YYYY-MM-DDTHH:MM:SS.fffffffff
```

Missing or malformed subseconds contribute zero. Use at most the first nine decimal digits and right-pad shorter values with zero. Retain a valid offset as signed minutes, but do not apply it to the ordering key. A missing or malformed offset remains unknown and does not invalidate the base value.

Do not use EXIF `DateTime`, GPS time, filesystem modification time, filenames, XMP, Preview metadata, or another Original as a guessed capture fact.

## Photo Capture Fact

A Photo derives its authoritative ordering key from its Originals:

1. a known RAW fact;
2. otherwise a known matching JPEG fact;
3. otherwise no authoritative key.

RAW authority is independent of Preview Source authority. Matching JPEG remains the preferred Preview source, while RAW remains the primary capture record for chronology.

A paired Photo has a capture disagreement when both Originals have known ordering keys that differ, or when both have known offsets that differ. One known and one unknown offset is incomplete metadata, not a disagreement. The disagreement is derived from Original facts and is not persisted separately. It does not make the Photo unavailable and is not exposed by the first browser protocol.

## Deterministic Library Order

Filtered Library order is:

```text literal
capture key present before capture key absent,
authoritative capture ordering key by UTF-8 bytes,
Photo ordering path by UTF-8 bytes,
Photo ID by UTF-8 bytes
```

The Photo ordering path is the RAW relative path when the Photo contains RAW; otherwise it is the JPEG relative path.

Photo Set queries continue to order only by `photo_set_members.position`.

## Confinement and Resource Bounds

Metadata inspection begins from the Library-owned Original capability. It opens the Original read-only beneath the retained canonical Library descriptor and passes an already-open descriptor or borrowed read/seek adapter to the metadata parser.

The parser must not receive or reopen an absolute or relative filesystem path. The same descriptor is revision-checked before and after inspection. Traversal, symlink escape, inode substitution, and mid-read revision changes fail only the affected fact.

Metadata input, parser allocation, blocking workers, and queued work are bounded. Capture inspection shares the existing per-cache native-work capacity of two with Preview extraction and derivative work. It must not unlock LibRaw sensor unpack, demosaic, or any RAW development path.

## Scan and Session Lifecycle

Initial startup and explicit rescan inspect newly discovered and changed available Originals before atomically publishing the new Library snapshot. An unchanged Original reuses its persisted fact when the inspected source revision matches.

After a v2-to-v3 migration, available Originals begin as `pending` and are inspected during the initial scan before readiness. This one-time backfill may lengthen the first upgraded startup, but it prevents the server from publishing a temporary path order that changes after readiness. Preview generation remains demand-driven.

An unavailable Original retains its last completed capture fact. When it becomes readable with a changed revision, the result from the current bytes replaces the retained fact, including `missing`, `invalid`, or `failed`; an old key must not remain authoritative for changed bytes.

A per-file capture failure does not abort valid sibling Photos. A root-level scan or persistence failure leaves the previously committed snapshot authoritative.

An active Review Session keeps its snapshotted Photo-ID sequence. Availability and Preview facts may refresh, but a rescan does not insert, remove, or reorder the active sequence. A later filtered Library Session may observe the newly published order.

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

Do not add a winning timestamp or disagreement column to `photos`. Query and domain mapping derive RAW-first authority and disagreement from the joined Original rows.

The v2-to-v3 migration adds only derived metadata columns and changes `PRAGMA user_version` to `3` in one admitted `BEGIN IMMEDIATE` transaction. It preserves every existing row, identity, membership position, Selection State, Rating, Preview fact, and progress record. Exact canonical-shape validation remains mandatory.

## Rollback

Before the first v3 startup, the operator creates a consistent, sidecar-free v2 backup while the production service is stopped or through an equivalently proven snapshot. A v2 binary rejects v3 as a newer schema.

Rollback stops the v3 process and restores the pre-upgrade v2 backup. There is no in-place down migration. Original Files and rebuildable cache bytes require no conversion and remain untouched.

## Protocol

`GET /api/photos` exposes the authoritative filtered-Library order. `GET /api/photo-sets` continues to expose members by explicit position.

The first browser protocol does not expose Capture Time, offset, inspection state, or pair disagreement. The user-visible contract is deterministic Library order. Capture failures must not disable selection, Rating, navigation, or Preview behavior.

## Options

### Selected: Separate Library and Photo Set Order Owners

Filtered Library Review uses Capture Time order. Photo Set Review uses membership position. This preserves explicit durable state and gives each source one order owner.

### Rejected: Capture Order for Every Review

This would make membership positions and explicit reorder ineffective during Photo Set Review and unexpectedly change existing production sequences.

### Selected: RAW-First Capture Authority

RAW is the primary capture record. JPEG is the fallback when RAW has no valid key. Preview Source remains independent.

### Rejected: Preview-Source Capture Authority

JPEG-first metadata would couple chronology to appearance delivery and allow a regenerated matching JPEG to override valid RAW chronology.

### Selected: Camera-Local Ordering

The ordering key compares camera-local values and retains, but does not apply, optional offsets. This avoids inventing a timezone for files that omit one.

### Rejected: Treat Missing Timezone as UTC

This turns an unknown fact into an assumed instant and can move mixed camera files by hours.

### Selected: Inspection Before Snapshot Publication

Capture inspection is part of initial scan and rescan completion. This preserves one stable published order.

### Rejected: Lazy Reordering After Readiness

A lazy backfill would expose temporary path order and reorder later Sessions as background work completes.

## Compatibility Fixtures

- `compatibility/metadata/capture-time.json` owns field precedence, parsing, normalization, offset, subsecond, disagreement, and ordering vectors.
- `compatibility/sqlite/schema-v3.sql` and `schema-v3.json` own canonical v3 shape; canonical v2 remains a migration input.
- Migration fixtures prove preservation of Photo identity, Photo Sets, positions, Selection State, Rating, Preview facts, and progress.
- Protocol fixtures own ordered responses and prove that capture fields remain omitted.
- Browser tests own the filename-versus-capture-order example and the explicit Photo Set order example.
- Generated metadata fixtures are minimal and redistributable. Real camera Originals remain opt-in and retain their SHA.

## Verification

Verification covers every Product Spec example, descriptor-confined inspection, mid-read revision change, parser and resource failures, v2-to-v3 migration, v2 backup restore, stable active Session order, future-session rescan reorder, unchanged Photo Set positions, exact protocol omission, and unchanged Original bytes and metadata.
