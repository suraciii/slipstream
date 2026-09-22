# Processing isolation qualification

This directory qualifies a private Rust processing boundary on Linux. It does
not enable photo editing or run darktable or Spektrafilm. The
[executor design](../../design/processing-executor.md) owns the authority and
lifecycle; the [protocol](../../design/processing-executor-protocol.md) owns
configuration and wire formats.

The host launcher owns only operational receipts, fixed native qualification
containers, retained accounting slices, and bounded temporary storage. Rust's
Photo Library service retains its domain state and publication authority. The
Web container receives no Docker socket, controller-write mount, or privileged
container settings.

## Build

Build on the repository's Rust toolchain. A separate worker image has a fixed
native entrypoint and no Python runtime:

```sh
cargo build --locked -p slipstream-processing --bins
docker build -f tools/processing/Dockerfile -t slipstream:processing-qualification .
docker image inspect --format '{{.Id}}' slipstream:processing-qualification
```

Use the inspected immutable local image ID in operator configuration. The
launcher does not pull images. Build a supported Web image using the repository's
[deployment procedure](../../docs/deployment.md) for independent survivor checks.

Run ordinary focused coverage without root or Docker:

```sh
cargo test --locked -p slipstream-processing
cargo clippy --locked -p slipstream-processing --all-targets -- -D warnings
```

These checks are also part of the workspace Rust gate. They do not replace the
kernel qualification below.

## Operator setup

Qualification needs a host systemd manager, Docker's systemd cgroup driver,
cgroup v2 with memory/CPU/PID controllers, and tmpfs with `noswap` support. The
launcher needs host root authority to create its exact owned slices, fixed
containers, and temporary mounts. Install the built launcher as a root-owned
executable. Provision separate root-owned canonical state and socket directories
with stable non-writable ancestors. Keep these disjoint from the Photo Library,
state, and cache. Never mount real Originals into qualification containers.

Use the complete configuration example in the protocol. Set `mode` explicitly
to `qualification`, choose a fresh random 32-character hexadecimal instance, and
set `peer_uid` to the intended local control-service UID. Keep the config
root-owned and not writable by other users. The socket parent needs traversal
permission for that UID; the launcher grants socket access with a named POSIX
ACL and also verifies peer credentials. Socket ownership includes a separate
locked inode/device claim so another instance cannot replace a live endpoint.
A fixed root-owned claim under `/var/lib/slipstream-processing/instances` binds
the instance to its journal root and holds a host-wide lifetime lock. The launcher
never removes this claim on exit. A missing journal for a claimed instance remains
quarantined even when runtime inventory is empty.

The journal root cannot contain whitespace, comma, backslash, or double quote;
this makes Docker mount arguments and mount identity checks unambiguous.

Start the host executable outside all processing cgroups:

```sh
sudo /usr/local/sbin/slipstream-processing-launcher --config /absolute/config.json
```

The supported qualifier requires unlimited shared ancestors above the dedicated
finite processing parent. A finite shared ancestor without a qualified control
reserve makes processing unavailable. The qualifier does not claim protection
from unrelated host-wide OOM or interference by another privileged host actor.
The Web/control process and launcher must both remain outside processing caps.

Each attempt gets a retained systemd slice and a fresh unprivileged container.
The blocked bootstrap is already charged to its capped container and ancestors.
The host freezes the exact container, verifies its identity and process liveness,
places its bootstrap in the group-OOM leaf, and reads back limits before release.
Frozen placement prevents the bootstrap timer and ordinary exit from recycling
the PID. A pidfd and opened proc directory detect disappearance. Linux does not
provide atomic PID-based migration against an external privileged actor that
kills and replaces the frozen process; such interference is outside this
qualification boundary.

Writable storage is one 16 MiB, 64-inode, `noswap` host tmpfs bound at `/work`,
`/tmp`, and `/dev/shm`. It remains charged to the retained attempt after exit.
The slot remains held until the launcher persists evidence and unmounts it.
The Docker log driver is `none`; only bounded result metadata is retained.

## Actual kernel verification

The verifier runs the actual launcher and pinned worker image, plus a supported
packaged Web image against an empty synthetic Library. Supply immutable local
image IDs; the output directory must not exist:

```sh
sudo python3 tools/processing/verify.py \
  --launcher /absolute/target/debug/slipstream-processing-launcher \
  --worker-image sha256:WORKER_IMAGE_ID \
  --web-image sha256:WEB_IMAGE_ID \
  --output /absolute/private/qualification-evidence
```

The verifier creates a fresh private root under
`/var/lib/slipstream-processing-qualification`, exact-owned containers and slices,
and small capped tmpfs mounts. It removes its runtime resources and reverse
checks cleanup. It does not inspect or stop existing Slipstream deployments.
Evidence includes binary/image identities, receipts, pressure counters, retained
storage accounting, crash boundaries, permitted UID/socket proof, and repeated
Web Library reads. A synthetic Album must survive pressure, a rename, and a Web
container restart.

The suite exercises native and descendant OOM, deliberate leaf and parent
pressure, non-OOM exit 137, storage and inode exhaustion, replay/cancellation,
real launcher SIGKILL at the closed operator fault barriers, and retained
accounting after Docker removes its scope. It also checks fixed protocol bounds,
concurrent ownership, foreign identity rejection, policy reduction, receipt
expiry, and successful subsequent attempts. Temporary fixture evidence stays
outside the repository.

## Quarantine and recovery

A blocked capability never means that work may run without limits. Inspect the
root-only journal and exact manager identities. An unresolved create, mount,
start, or parent provisioning request retains ownership even if one runtime
lookup is empty: the earlier operation may complete later. Do not erase its
journal, remove a lock file, or start a replacement under a reused identity.

A normal launcher restart verifies the aggregate binding first, settles the old
attempt at its captured policy, preserves evidence and cancellation, then applies
a new operator policy. It never releases an old bootstrap. An unknown, missing,
or recreated aggregate or child is quarantined rather than adopted or deleted.
A host reboot invalidating the stored runtime binding also requires operator
reconciliation. After proving all prior work and manager operations stopped,
preserve their journal/evidence and provision a fresh instance identity. Receipt
expiry does not expire an active ownership fence.
