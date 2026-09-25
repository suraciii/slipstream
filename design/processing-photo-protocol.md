# Production Photo Processing Protocol

The qualification protocol is deliberately closed to registered fixtures. A
production Photo request needs a different authority: the Rust service knows
which Photo, source, recipe and Export are being processed, while the host
launcher knows which image, bundle and finite resource policy may run. Giving
either component the other component's authority would allow a fixture path to
be used as a Photo path or would require the launcher to become a second
Library service.

[Photo Development Architecture](photo-development.md) owns recipes, source
guards, Export state and artifact publication. [Processing Executor](processing-executor.md)
owns the launcher, attempt containment, resource evidence and settlement. This
document owns the small admission contract between them. It does not change
the fixture-only [qualification protocol](processing-executor-protocol.md),
qualify a RAW camera, or enable Film work.

## Design Drivers

- A request must identify one current Photo and one immutable Export snapshot.
- Original Files and external XMP remain outside the worker's writable
  boundary.
- The Web service must not send a path, shell command, image, cgroup value or
  arbitrary stage list to a privileged launcher.
- The launcher must verify exact source bytes before releasing a worker and
  must retain ownership until output and resource evidence settle.
- The first production workload is one bounded `development-tiff` stage. Film
  remains a separate capability until its own supported-source and resource
  decisions pass.

## Transport

Production Photo admission uses a dedicated versioned `SOCK_SEQPACKET` Unix
socket. It does not reinterpret the fixture parser: the qualification
protocol remains version 1, rejects ancillary data, and remains fixture-only.
Each Photo packet contains one bounded UTF-8 JSON message (the repository's
control-frame limit) and its ancillary data in the same `sendmsg` operation.
`SOCK_STREAM` framing is not used for Photo packets, so a descriptor cannot be
consumed with a partial length header. `SCM_RIGHTS` is used only for the two
operations that need a file descriptor:

- `LauncherStart` carries exactly one read-only source descriptor;
- `OutputRequest` carries exactly one writable service-output descriptor.

The ancillary descriptor must arrive with its operation packet. Missing,
multiple, or unexpected descriptors are refused before admission. `LauncherStart`
and `OutputRequest` each require exactly one `SCM_RIGHTS` descriptor; every
other request and every response requires zero. The receiver uses
`MSG_CMSG_CLOEXEC`, rejects `MSG_TRUNC` and `MSG_CTRUNC`, and closes every
unexpected descriptor before returning an error. The launcher sets
`FD_CLOEXEC`, checks the descriptor with `fstat`, requires a regular file,
one hard link, the configured peer UID, no write bits, `F_GETFL & O_ACCMODE`
equal to `O_RDONLY` for source input or `O_WRONLY`/`O_RDWR` for output, and an
initial offset of zero. A production transport parser is separate from the
fixture parser because the current fixture transport intentionally rejects
ancillary data.

The production JSON envelope is closed and uses these operation fields:

```text
common:          mode, version, instance, op
start:           export_id, incarnation, sequence, policy, bundle,
                 workload, source={kind, profile_id, size, sha256},
                 recipe={exposure_milli_ev, white_balance_mode},
                 recipe_digest, manifest_sha256       + one source FD
output:          export_id, incarnation, sequence, target=development-tiff
                                                        + one output FD
validate-output: export_id, incarnation, sequence, target, size, sha256,
                 accepted                                  + no FD
reconcile/inspect/cancel: export_id, incarnation, sequence when applicable
                                                        + no FD
```

`op` is one of the listed values, `mode` is exactly `photo-processing`, and
`version` is `1`; unknown fields are refused. A declared `source.size` and a
reported `validate-output` `size` are positive and no greater than the bounded
application limits; a packet outside that range is malformed and is refused
with the rest of envelope validation, before the descriptor count and the
capability are checked. The configured source and output maxima may be smaller,
and admission enforces those separately. Responses use the same mode and
version plus exactly one bounded `result` or `error` object. An output result
is the bounded `OutputReceipt` (attempt identity, target, size and SHA-256),
never file bytes or a path. The validation acknowledgement is durable before
the launcher releases its receipt and ownership.

## Model

### Photo Admission

`PhotoAdmission` is a server-owned immutable snapshot for one accepted Export.
The server retains the complete record; only the smaller `LauncherStart`
projection crosses the private socket. The service record has these semantic
fields:

| Field              | Meaning                                                                                                                                                                                                   |
| ------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `protocol_version` | The production Photo IPC version. Unknown versions are refused.                                                                                                                                           |
| `export_id`        | The service-owned attempt identity and idempotency key. Preview-class work carries a preview attempt identity instead of an Export identity, and the launcher treats either as an opaque correlation key. |
| `photo_id`         | The opaque Library Photo identity. It is never resolved by the launcher.                                                                                                                                  |
| `source`           | A `PhotoSourceBinding` containing `kind=raw`, an approved `profile_id`, the current opaque Library `source_revision`, the staged byte length, and the staged SHA-256.                                     |
| `recipe`           | A `RecipeSnapshot` containing the guarded recipe revision and the semantic exposure and white-balance settings captured by the Export. It contains no darktable options or executable data.               |
| `policy_id`        | The exact finite resource policy selected by the server capability read.                                                                                                                                  |
| `bundle_id`        | The exact approved processing bundle digest selected by the server capability read.                                                                                                                       |
| `workload`         | The closed value `development-tiff`. It is not a caller-provided stage plan.                                                                                                                              |

`RecipeSnapshot` is closed for this first workload. The execution payload is
`exposure_milli_ev`, a signed 64-bit integer in thousandths of an EV, and
`white_balance_mode=as-shot`. The approved bundle supplies the finite EV
range; values outside that range, non-finite stored values, custom temperature
or tint, and unknown recipe fields are refused. The service may retain a
recipe that is readable but not representable by this execution payload; that
Photo remains browseable and processing-unavailable until an explicitly
qualified mapping exists.

The service computes `recipe_digest` from the canonical ordered tuple
`{exposure_milli_ev, white_balance_mode}` and includes it in the durable Export
snapshot and launcher manifest. The launcher receives the two semantic values
and the digest, validates them against the configured bundle, and never
receives a darktable history or engine-private settings blob.

`LauncherStart` carries `protocol_version`, `export_id`, the existing executor
attempt identity `(incarnation, sequence)`, the staged source `kind`, byte
length and SHA-256, the semantic recipe settings, `policy_id`, `bundle_id`,
`recipe_digest`, and `workload=development-tiff`. The server obtains
`incarnation` and the
next `sequence` through the launcher's existing reconcile operation and
persists that pair with the Export. A retry after a lost response reuses the
same pair; an explicit retry allocates a new sequence. The launcher receipt
therefore retains the existing executor identity rather than introducing a
second attempt identity. Photo ID, Library `source_revision`, and recipe
revision remain in the service-owned snapshot and are not sent over IPC.

The source descriptor is a transport attachment, not a path field. The server
opens one regular file from its private staging workspace and sends that
read-only descriptor with the request over the authenticated Unix socket. The
descriptor's regular-file facts must match the declared size. It must be owned
by the configured Web peer UID, have no write bits, one hard link, and an
initial offset of zero. The launcher then hashes the bounded byte stream and
compares the result with `source.sha256`; filesystem metadata alone does not
prove content identity. It reads the descriptor into its own private immutable
attempt snapshot;
it never re-resolves a pathname supplied by the server or Web client. The
staging file and its parent are owned by the configured Web peer UID, are
created outside the Library Folder. The server syncs the file and parent,
removes writable aliases, seals the file read-only, and keeps the descriptor
open until the launcher acknowledges copy or refusal. The staged file is then
removed by the server. The launcher is the only component
that gives a worker a read-only input mount, and it derives that mount from its
own snapshot under the fixed worker identity.

The launcher derives an `AdmittedStagePlan` after validating the descriptor and
the source facts. For this version it is exactly:

```text
workload: development-tiff
steps:    [develop]
output:   development-tiff
```

The plan, engine command, input/output paths, worker identity, limits and
temporary storage are derived from the configured `bundle_id` and finite
resource policy. They are not request fields. A source kind, bundle, geometry,
or policy outside that fixed authority has no admitted plan.

`profile_id` is a closed bundle-owned identifier (1 to 64 lowercase ASCII
letters, digits, `.`, `_`, or `-`). The bundle lists the supported camera or
container classes; `kind=raw` without a matching profile is unsupported.

The first qualified list has two entries: `sony-ilce-7rm5-arw` for the Sony
ILCE-7RM5 with an ARW container and `sony-ilce-7cm2-arw` for the Sony
ILCE-7CM2 with an ARW container. Qualification evidence and its coverage
limits are recorded in Issue #329. The service classifies a RAW Original by
its camera make, camera model and RAW container against that list and reports
an unlisted class as unsupported; the launcher revalidates the received
`profile_id` against its own configured bundle before it admits any plan.
Other bounds of this workload stay closed at a finite exposure range of 0 to
+1 EV in 1000th-EV steps with `white_balance_mode=as-shot`.

### Authority split

The service is the sole authority for Photo and product state. It resolves the
Photo ID through the confined Library boundary, checks the current source
revision, persists the guarded recipe and Export snapshot, stages and hashes
the source, and records the Export ID and executor attempt identity. It
validates the returned output as a Development TIFF and publishes it through the existing
private Export workspace. The launcher never reads SQLite, resolves a Photo,
opens an Original Location, or marks an Export successful.

The launcher is the sole authority for execution state. It authenticates the
configured peer UID and instance, compares the exact bundle and workload with
its root-owned configuration, validates the received descriptor, derives the
stage plan, creates the finite attempt boundary, starts the fixed worker, and
retains resource and terminal evidence. It owns runtime paths, cgroup and
container identities, actual limits, cancellation, and cleanup receipts. A
launcher receipt refers to the service `export_id` and executor attempt
identity but does not become a second Photo or Export record.

The service and launcher both retain the Export ID, executor attempt identity,
source digest, policy and bundle identity and workload in their receipts. A
mismatch is a refusal or an explicit uncertain settlement, never a best-effort
execution.

### Bundle and policy authority

The production launcher configuration binds one immutable worker image, one
processing bundle, one finite resource policy and one instance identity. The
bundle digest covers the engine adapter, profiles, native IO dependencies,
the qualified source profile (camera/container class), recipe ranges and the
fixed `development-tiff` output contract. A `kind=raw` label alone never
qualifies every RAW extension or camera. The policy identity covers the
approved attempt limits and shared-ancestor/headroom rules. The request may
echo these identities for correlation, but it cannot change them. The
launcher must compare them before copying a descriptor into a workload
boundary and again before worker release.

This capability is distinct from `qualification`, `film-measurement`, and
`film-qualified-fixtures`. A well-formed fixture configuration, a reachable
socket, or an installed launcher must not make `photo-processing` available.
The production capability is available only when the exact bundle, policy,
source kind, and deployment prerequisites all pass their respective checks.

### Production configuration and receipt

The root-owned production configuration is a separate document with these
required fields: `version=1`, `mode=photo-processing`, instance identity,
canonical private root and socket, authenticated peer UID, immutable worker
image digest, bundle digest, policy digest, source-byte maximum, staged
storage byte and inode maxima, output-byte maximum, finite memory and CPU/task
limits, zero swap, control reserve and shared-ancestor headroom, and receipt
retention. `export_id` is 1 to 128 ASCII letters, digits, `.`, `_`, or `-`;
`policy_id`, `bundle_id`, and every SHA-256 are exactly 64 lowercase hex
characters. The source, staging, output, memory, task, reserve and retention
limits are positive finite integers within the application limit; zero or
unlimited values are invalid.
The policy digest also binds the finite memory, zero-swap, CPU/task, control
reserve, shared-ancestor headroom and cleanup rules. The source and output
maxima are finite and must be no greater than the existing bounded application
limits. Missing, unlimited, or mismatched values leave the capability
unavailable. No request or environment variable can replace these fields.

Every accepted production receipt retains a bounded record containing
`export_id`, `(incarnation, sequence)`, `workload`, `policy_id`, `bundle_id`,
`recipe_digest`, source kind/size/SHA-256, state, outcome, launch and terminal
identities, output transfer state, and cleanup state. The receipt is the
launcher's execution record; the service's Export snapshot remains the
authority for Photo ID, source revision, recipe revision and publication.
The canonical manifest digest covers all fields that affect execution:
workload, source kind/size/SHA-256, recipe payload, bundle, policy and output
target. Replaying `(export_id, incarnation, sequence)` is accepted only when
that digest is identical. A changed manifest is a conflict; an expired receipt
cannot be reused.

## Semantics

### Admission ordering

The service accepts a request only after it has atomically captured the Export
snapshot: `export_id`, Photo ID, recipe revision/settings, source revision,
target, policy and bundle identity. It then stages the source through the
confined descriptor boundary, verifies the complete copy and post-copy source
revision, confirms the selected source profile is available, and obtains the
launcher's current incarnation and next sequence. Reconcile must report
`available` with no active receipt; the service single-flights this slot and
reconciles again on a busy or sequence conflict. It then sends one
`LauncherStart` with one descriptor.

The launcher performs these checks in order:

1. Authenticate the peer and validate bounded framing, version, identities,
   closed workload value and exactly one descriptor attachment.
2. Compare instance, bundle, policy and capability incarnation with its
   root-owned configuration. Refuse a qualification or Film-only endpoint.
3. Verify the descriptor is a regular file and that its declared size is
   positive, within the approved source-byte and private-storage byte/inode
   bounds, and reservable with one active slot. Confirm the configured control
   reserve and effective shared-ancestor/headroom admission before any copy.
   These checks use bounded metadata only.
4. Persist the existing executor intent, reserve the active slot and the
   bounded private-storage allowance, then copy and hash the descriptor into
   the launcher-owned private snapshot with bounded control-path buffers. The
   launcher is outside the worker cgroup, so this copy is covered by the
   qualified control and storage reserves; placing the file under an attempt
   directory does not make the launcher buffer part of the attempt peak. A
   size or digest mismatch settles the owned intent without releasing a
   worker. Sync the private snapshot, remove writable aliases, and seal it
   read-only before continuing.
5. Derive and persist the one `AdmittedStagePlan` from the validated source
   facts, bundle and policy. Create the fresh retained attempt boundary before
   worker bootstrap and release. Worker bootstrap, engine work and any helper
   explicitly placed in that boundary are charged to the finite policy. The
   server's bounded pre-staging copy is never an unbounded preflight.
6. Start the fixed worker with no network, display, GPU, Library mount,
   launcher socket or writable cgroup access. The worker sees only the
   launcher-owned read-only source and private writable work/output mounts.

The launcher returns an accepted receipt only after the executor attempt
identity and ownership journal are durable. Repeating the same `export_id` and
`(incarnation, sequence)` with the same digest-bound payload returns that
receipt. Reusing either identity with different source, recipe settings,
bundle or workload data is a conflict. An expired or unknown identity cannot
start new work.

### Output and settlement

The worker writes only to its private output area. On successful completion,
the launcher verifies terminal settlement. The service creates a private
temporary output file through the existing Export workspace, reserves its
bounded size, and sends one `OutputRequest` carrying that writable descriptor.
The launcher checks it is a regular, single-link file owned by the configured
Web peer, starts at offset zero and has no pre-existing bytes, then copies the
launcher-owned result into it with bounded chunks. The launcher returns a
bounded `OutputReceipt` containing the attempt identity, target, byte length and
SHA-256; no filesystem path or multi-gigabyte control response is used.

Before it offers any output, the launcher validates its own engine artifact
against the closed Development TIFF contract: IEEE float32 RGB samples, Deflate
strip ranges that account for the declared full geometry with no unaccounted
trailing payload, no hidden orientation, and the exact pinned embedded profile
bytes. The strip layout is the qualified writer's shape, one entry per declared
row block, so the launcher bounds the parsed arrays by the qualified geometry
instead of a fixed small count. An artifact outside that shape settles as
`refused-output-validation` and is never offered.

The service hashes and validates the received file as the captured
`development-tiff` target, including dimensions, sample type, color profile,
size and content identity, syncs it, and sends an explicit validation
acknowledgement. The launcher retains its result, receipt and ownership until
that acknowledgement is durable. The service removes its uncommitted temporary
file after a negative acknowledgement; the launcher retains its result and
receipt for reconciliation. A disconnect, failed validation or lost
acknowledgement leaves the receipt unsettled and never causes a second worker
start. Only after publication and durable Export state commit may the service
report success.

On a source revision change or recipe conflict, the service refuses before IPC.
On descriptor mismatch, unsupported RAW/source facts, bundle/policy mismatch,
unavailable resources, malformed request, or failed preflight, the launcher
refuses before worker release. The service records the actionable refusal and
removes its staged copy after the refusal is durable.

On worker failure, OOM, cancellation, deadline, lost client connection,
incomplete output or output validation failure, the launcher keeps ownership,
records cancellation and terminal resource evidence, removes the worker's
private workspace only after settlement, and returns a failed or uncertain
receipt. The service never publishes a partial artifact and does not infer
failure from a dropped response. It removes its staged copy only after the
launcher receipt or an explicit reconciliation proves that no descriptor or
attempt still depends on it. A launcher restart reconciles the same journal
and remains unavailable for new Photo work while ownership is uncertain, as
required by [Processing Executor](processing-executor.md#launcher-lifecycle).

If the service crashes after launcher acceptance, restart reconciliation uses
the durable `export_id` and `(incarnation, sequence)` receipt. It either
validates and publishes the complete output or records the terminal failure.
It never starts
a replacement against a possibly live attempt. If publication succeeds but
the database commit is uncertain, the service validates the artifact and
resolves the recorded Export before allowing a retry.

### Errors and capability

The service exposes processing availability separately from Library health. It
reports at least these distinct conditions: disabled/unconfigured,
launcher-unavailable, bundle-unavailable, source-unsupported,
resource-unavailable, and ready for `development-tiff`. A healthy `/healthz`,
an open socket, or a fixture receipt cannot produce the ready state.

All refusal and settlement outcomes retain the Export ID and executor attempt
identity, workload, source digest, policy, bundle identity and a bounded
reason. The protocol does not expose host paths, command lines, engine-private
settings, cgroup paths, or Original locations to clients.

## Options

### Selected: Descriptor handoff plus launcher-owned snapshot

Passing one descriptor over the authenticated local socket binds the bytes the
service staged without a path lookup or a privileged re-open. Copying those
bytes into a launcher-owned snapshot gives the launcher stable mount authority,
lets the worker run under the configured UID, and permits source cleanup after
the handoff. It keeps Photo and Export authority in the service while the
launcher retains execution ownership.

### Rejected: Send a server path

A path lets a privileged launcher follow a changed, linked, or replaced object
after the service's check. It also leaks the Library/workspace topology and
turns a Web-controlled pathname into a host mount request. A path is therefore
not a production admission field.

### Rejected: Let the launcher resolve `photo_id`

That would require a second Library/database authority, a Library mount or
duplicate source and recipe guards. It could diverge from the Export snapshot
while a Photo is recovered or changed. The launcher receives the immutable
descriptor and snapshot facts instead.

### Rejected: Accept a caller-provided stage list or limits

An arbitrary plan would make the launcher a general task runner and could
escape the qualified memory, storage and output policy. The first workload has
one closed stage; the launcher derives its exact plan from its approved bundle
and policy.

## Verification

Before implementation, an independent reviewer must derive these cases from
the document:

- a valid RAW Photo with a stable staged descriptor is accepted as one
  `development-tiff` attempt;
- a changed source revision, digest/size mismatch, stale recipe or wrong
  bundle is refused before worker release;
- a path-only, fixture-mode, Film-mode, arbitrary-workload or caller-limit
  request is rejected;
- duplicate start with the same Export ID and `(incarnation, sequence)`
  resolves to one receipt, while conflicting identity data is
  rejected;
- worker failure and OOM preserve the recipe, do not publish output, settle
  ownership, and clean both workspaces;
- a complete output is validated and atomically published before Export
  success; and
- launcher restart reconciles accepted work without a duplicate attempt while
  ordinary Library operations remain usable.

The implementation must add focused protocol, descriptor, idempotency,
failure/cleanup and publication tests before enabling `photo-processing`.
Actual deployment acceptance additionally requires the supported-host checks in
`docs/deployment.md`, plus independent #329 source/WB qualification and #330
Export lifecycle evidence. This document itself is not evidence that a
production launcher or Photo capability is installed.
