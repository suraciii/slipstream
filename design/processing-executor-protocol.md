# Processing Executor Qualification Protocol

A small executable qualification capability establishes the real resource and
recovery boundary before image adapters use it. This is a private Rust IPC
contract, not a public job API or editing capability.
[Processing Executor](processing-executor.md) owns authority and lifecycle.
This reference owns exact configuration, framing, fields, and fixed fixture
behavior. Implementations must validate the examples and reject unsupported
syntax. Photo workloads remain unavailable until their source, bundle, resource,
and artifact contracts are independently qualified.
[Film Resource Measurement](processing-film-measurement.md) owns the separate
version-2 operator measurement syntax; it does not change this native protocol.

## Operator Configuration

This configuration and protocol are qualification-only. The explicit
`qualification` mode admits only the closed synthetic workloads below; it cannot
be repurposed for Photo, TIFF or Film inputs. A successful kernel qualification
does not make the profile a production processing service. This protocol version
has no production mode or production policy fields. Production launcher
configuration and admission must bind an independently approved policy and
immutable workload identities; environment variables, IPC requests and Compose
overrides cannot promote this fixture profile or bypass that admission.

A host-only UTF-8 JSON file, maximum 16 KiB, rejects unknown/duplicate fields. Required fields:

```json
{
  "version": 1,
  "mode": "qualification",
  "instance": "0123456789abcdef0123456789abcdef",
  "root": "/var/lib/slipstream-processing/0123456789abcdef0123456789abcdef",
  "socket": "/run/slipstream-processing/0123456789abcdef0123456789abcdef.sock",
  "peer_uid": 1000,
  "image": "registry.example.com/slipstream/processing@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "memory_bytes": 134217728,
  "receipt_retention_seconds": 86400
}
```

`instance`: exactly 32 lowercase hex digits. `image`: either an immutable repository@sha256 reference or a local `sha256:` image ID, followed by exactly 64 lowercase hexadecimal digits; never a tag or CLI-provided override. Resolve and persist the exact local image ID before admission; no implicit image pull occurs during an attempt. `root` and socket parent: canonical absolute root-owned directories; no writable/replaceable mount-source ancestor, link, or foreign mount. The journal is durable under `root`, separate from attempt tmpfs. Socket is root-owned with configured peer UID access, authenticates SO_PEERCRED, and is never mounted into a worker. The operator must provide a root disjoint from the Library, state, and cache; no user photo directories are used by qualification.

`memory_bytes`: positive integer bytes and a page multiple. Qualification envelope 64 through 256 MiB; the production image envelope remains unavailable. One operator value caps aggregate and full-attempt accounting. The profile fixes 1 CPU, 32 tasks, no swap, 16 MiB tmpfs, 64 tmpfs inodes, a 10-second blocked-bootstrap deadline, and a 30-second total attempt deadline. These are qualification constants, not per-request settings. All writable locations (including /tmp and /dev/shm) must share the one bounded storage mount or be read-only. Retention: integer 1 through 604800 seconds. The log driver is fixed to `none`; diagnostic and result metadata must fit bounded receipts. At most 256 terminal receipts plus one active intent; if unexpired receipt capacity is exhausted, refuse admission. The persistent watermark survives receipt removal.

Reject a finite shared ancestor unless a qualified control/headroom allowance exists for it. The initial fixture profile supports only an unlimited shared ancestor above the capped processing subtree; it does not invent a production service reserve. Validate the host, launcher's and control service's actual cgroup placement; missing proof means unavailable processing. Do not represent this as protection from unrelated host-wide OOM.

Host binary surface: `slipstream-processing-launcher --config /absolute/config.json`. The qualification verifier must opt into this mode explicitly. A separate repository kernel-verification command creates a private config and starts this actual binary; it does not grant arbitrary Docker options through IPC. Native worker binary is part of the digest-pinned qualification image. Do not install this fixture configuration as the production system service configuration or mount its socket into a Photo-processing Web deployment.

## IPC

Unix stream, one request per connection, 4-byte big-endian length followed by UTF-8 JSON, maximum 16 KiB. Response maximum 64 KiB. Read/write deadline 2 seconds. Unknown versions, fields, duplicates, malformed integers, trailing JSON data, oversized frames, unexpected ancillary data, and foreign UID are rejected before admission. The launcher admits at most four simultaneous control connections, with no unbounded pending queue. Large manager operations run independently of the short control-message deadline; the durable receipt supplies their outcome. No shell/path/argv/env/image/controller override exists.

Requests use a closed tagged enum. Every request includes `version:1` and the configured `instance`:

```json
{
  "version": 1,
  "instance": "0123456789abcdef0123456789abcdef",
  "op": "reconcile"
}
```

Reconcile returns current registry incarnation, next admission sequence, current policy SHA-256, bundle SHA-256, active receipt or null, and capability `qualification-only`. It reconciles ownership before returning available; it cannot forget or terminate an ambiguous foreign runtime. Returning current identities here avoids another discovery operation.

```json
{
  "version": 1,
  "instance": "0123456789abcdef0123456789abcdef",
  "op": "start",
  "incarnation": "11111111111111111111111111111111",
  "sequence": 1,
  "policy": "2222222222222222222222222222222222222222222222222222222222222222",
  "bundle": "3333333333333333333333333333333333333333333333333333333333333333",
  "workload": "probe-success"
}
```

The complete immutable manifest in this increment is the closed workload kind. The launcher stores this validated enum as the canonical manifest and compares it directly on replay; a request cannot substitute a claimed digest for the workload. Policy digest binds the exact validated operator policy/profile; bundle digest binds image/native worker identity. The start request cannot override either. No input descriptors are accepted for fixture workloads. Photo staging requires a separately qualified typed descriptor/size/hash contract with source-safety review; this protocol version does not accept mutable paths.

Closed workloads:

- `probe-success`: verifies controller-write/escape denial, writes a small deterministic result, exits zero.
- `probe-native-oom`: native anonymous allocation until the kernel contains it.
- `probe-descendant-oom`: forks two allocating descendants; proves whole-workload group termination.
- `probe-exit-137`: exits 137 without allocation; must not classify as OOM.
- `probe-hold`: remains bounded until cancellation/deadline, for restart/race verification.
- `probe-storage-full`: bounded block writes until ENOSPC; returns explicit storage exhaustion with no host spill.
- `probe-inodes-full`: creates empty files until the inode cap refuses allocation.

Inspect/cancel have only `incarnation` and `sequence` in addition to the common envelope. Matching known starts return the same receipt; changed intent at the same identity conflicts. New starts require exactly watermark+1 and an empty settled slot. An expired sequence at/below watermark returns `expired`, never a new launch. Foreign incarnation is rejected. Persist launch intent + incremented watermark + reserved slot atomically and fsync before any systemd/Docker operation. Registry incarnation is random and persistent. A genuinely new host instance claim and registry establish a new incarnation; an existing claim with a missing registry is quarantined and cannot initialize a replacement journal.

Cancellation is a durable monotonic intent before termination. A settled receipt is immutable; cancelling it returns that receipt. A duplicate start never reverses cancellation. Slot release requires known runtime termination, persisted terminal evidence, and reclaimed temporary storage.

## Responses and Receipts

Every response is either `{"version":1,"result":RESULT}` or
`{"version":1,"error":{"code":CODE}}`, with exactly those fields. No error
contains a path, raw subprocess output, or private image metadata. `CODE` is one
of `invalid-request`, `unauthorized`, `wrong-instance`, `unavailable`,
`incompatible-policy`, `incompatible-bundle`, `conflict`, `busy`, `expired`,
`unknown-attempt`, `stale-incarnation`, `capacity`, or `uncertain`.

Reconcile returns a result with these exact fields: `kind` (`capability`),
`capability` (`qualification-only`), `instance`, `incarnation`, `next_sequence`,
`policy`, `bundle`, `availability` (`available` or `blocked`), and `active`
(a receipt or null). `available` establishes only the fixed qualification
profile. Start, inspect, and cancel return a result with `kind` (`receipt`) and
`receipt`. Unknown future sequences return `unknown-attempt`; a known expired
sequence returns `expired`.

A receipt has these exact fields:

- `incarnation`, `sequence`, `workload`, `policy`, and `bundle`, with the same
  validated identities as the accepted start;
- `state`: `accepted`, `running`, `settling`, `settled`, or `blocked`;
- `cancellation_requested`: Boolean, monotonic while unsettled;
- `accepted_at_unix_ms` and `deadline_unix_ms`: persisted absolute times;
- `outcome`: null or `completed`, `allocation-failed`, `oom`, `storage-full`,
  `cancelled`, `deadline`, `engine-failed`, `interrupted`, or `unknown`;
- `runtime`: null or an object with `launch_id` (32 lowercase hex digits),
  `container_id` (64 lowercase hex digits or null), and `attempt_unit`
  (validated derived systemd slice name, at most 160 ASCII bytes);
- `limits`: an object with `memory_bytes`, `swap_bytes` (zero), `cpu_quota_us`
  (100000), `cpu_period_us` (100000), `tasks` (32), `storage_bytes` (16777216),
  and `storage_inodes` (64);
- `evidence`: null or an object with `peak_bytes`, `exit_code` (null or 0–255),
  `docker_oom_killed` (Boolean or null), `attempt_before`, `attempt_after`,
  `parent_before`, `parent_after`, and `populated` (Boolean or null); and
- `cleanup`: `pending`, `complete`, or `uncertain`.

Each before/after event object is null or has exactly `oom`, `oom_kill`,
`oom_group_kill`, `local_oom`, `local_oom_kill`, and `local_oom_group_kill`.
A missing observation is null, never synthesized as a zero counter. Kernel
counters and byte quantities are unsigned 64-bit integers. `peer_uid` is an
unsigned 32-bit integer other than the Linux invalid UID value 4294967295.
Sequences are integers from 1 through 18446744073709551615. Overflow of a
counter calculation or next sequence refuses admission. Times are nonnegative
64-bit milliseconds since the Unix epoch; deadline addition is checked.
JSON Boolean, fractional, signed-negative, duplicate, and overflowing integer
values cannot substitute for these integers.

`accepted` means durable intent owns the slot; `running` means the worker was
released. `settling` means termination or cleanup is in progress. `blocked`
means ownership or cleanup is uncertain and admission is disabled. Only
`settled` is terminal and immutable; it requires an observed outcome, persisted
evidence, confirmed process termination, and `cleanup:complete`. Execution
outcome, once established, stays unchanged while cleanup advances. An unknown
outcome can gain evidence during reconciliation before settlement. Cancellation
of a settled receipt returns it unchanged. A cancellation received after a
recorded completed execution cannot undo that outcome.

## Native Bootstrap

The launcher opens and retains the gate FIFO read-write before container start;
the worker opens it read-only and polls within its bootstrap deadline. A launcher
crash closes the gate writer and cannot grant permission to run. The worker uses
a fixed 4096-byte result file inside the capped tmpfs, allocated before storage
exhaustion probes, so those probes can report an outcome without extra storage.
Result reads are bounded and strictly validated.

The pinned worker begins as PID 1 with fixed profile-only bootstrap arguments. It blocks on a launcher-owned read-only FIFO/control descriptor and has its own bounded startup timeout. No native allocation fixture or engine starts before the host has verified full-container accounting, workload leaf membership, controller values, and exact runtime identity and has durably recorded release intent. Release carries only a fixed token bound to that launch; invalid/EOF/timeout exits without running a workload. The worker then executes its already fixed closed workload kind; it cannot select a program or load a supplied module. No worker has a Docker/control socket or cgroup write mount.

The qualification verifier separately exercises intentionally restrictive ancestor/leaf policies and asserts rejection in ordinary admission. Test-only parent-pressure arrangements are created by the operator verifier, not client overrides.

## Operator Fault Barriers

Qualification mode supports deterministic operator fault barriers beneath the
sealed instance root's `faults` directory. These files are not mounted into a
worker or exposed through IPC. The launcher rejects links, non-regular files,
multiple hard links, non-root ownership, group/other access, and files above
1024 bytes. No request or configuration field can select a barrier path.

An operator may create `arm.json` with exactly `phase`, `incarnation`, and
`sequence`. The incarnation and sequence must identify the intended admission.
The closed phases are `after-intent`, `after-slice`,
`after-create-response`, `after-container-bound`, `after-release-intent`,
`after-exit`, `after-evidence`, `after-container-removal`,
`after-storage-unmount`, `after-slice-stop-intent`, and `after-slice-stop`.
`after-slice-stop-intent` occurs after the pending stop fence is synchronized and
before the manager stop call; crashing there must retain blocked ownership.
`after-slice-stop` occurs after stop completion and its confirmed fact are
synchronized. Other phases refer to completed operations; an unresolved manager
request remains a separate uncertain state.

At a matching phase, the launcher atomically writes `marker.json` with those
three fields plus the exact `launch_id`, then waits for root-owned
`release.json` containing the identical four fields. The wait ends no later
than the attempt's captured absolute deadline. Invalid files or an expired
wait block admission; they never release an engine. A verifier can kill the
launcher after observing the marker and inspect the real persisted/kernel
state. Restart reconciles owned work and never resumes or releases a prior
bootstrap merely because a barrier exists. Barrier cleanup removes only
validated files for the matching launch. A later production mode must reject
these barriers rather than inherit this qualification capability.

Before an asynchronous manager operation can begin, its pending phase is
durable. A lost create/start response cannot be settled from one empty runtime
lookup: a daemon operation may still complete later. The launcher retains the
slot and reports uncertain ownership until completion or safe settlement is
proven. A confirmed create response may be durably recorded before the separate
exact runtime-identity binding; this is the `after-create-response` barrier.
