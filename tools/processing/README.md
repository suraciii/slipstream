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
The attempt slice is transient and keeps its accounting until terminal evidence
is durable. Settlement confirms removal of the loaded unit, cgroup and all exact
unit configuration without requesting a global systemd reload. The host must
provide `busctl` from systemd. Unsettled legacy nontransient slices remain blocked
for operator reconciliation; the launcher does not migrate their configuration.
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

## Film measurement

The separate [Film measurement profile](../../design/processing-film-measurement.md)
uses the same host launcher with version-2 documents and root-only control IPC.
Its catalogue selects registered synthetic images or private pre-staged TIFFs.
It compares complete output with independent references, retains bounded evidence,
and reclaims all image bytes. It cannot publish an Export or admit ordinary Photo
requests. Unknown resource terms remain unqualified after a successful run.

Build the fixed Film worker and adapter using the exact numerical parent retained
locally for qualification. The helper checks the parent's immutable identity
before and after the build and verifies inherited layers; the image build checks
the original numerical source/package inventory. A missing or changed parent is
a build failure, not permission to select another image:

```sh
python3 tools/processing/film/build.py --target qualification --tag slipstream:film-measurement
docker image inspect --format '{{.Id}}' slipstream:film-measurement
```

Focused adapter checks use a separate test target with generated inputs:

```sh
python3 tools/processing/film/build.py --target adapter-checks --tag slipstream:film-adapter-checks
docker run --rm --network none --memory 4g --memory-swap 4g --cpus 4 \
  --pids-limit 256 --read-only --tmpfs /work:rw,size=256m \
  --tmpfs /tmp:rw,size=64m slipstream:film-adapter-checks
```

Prepare a catalogue and resource model according to the linked schema. TIFF
fixtures use `<fixture_id>.tif` names in an explicit private directory; do not
point this directory at Originals or a Photo Library. The verifier copies and
validates these operator fixtures before measurement. This preparation is
separate from the attempt's capped source copy, decode, render, and validation.
No pre-existing source file is modified. The planner reserves source-cache bytes
regardless of their existing residency.

Run the actual Film image with an explicit experimental budget and fresh evidence
directory. The example budget is a probe setting, not a recommended minimum:

```sh
sudo python3 tools/processing/verify-film.py \
  --launcher /absolute/target/debug/slipstream-processing-launcher \
  --worker-image sha256:FILM_IMAGE_ID \
  --web-image sha256:WEB_IMAGE_ID \
  --catalogue /absolute/private/catalogue.json \
  --resource-model /absolute/private/resource-model.json \
  --fixtures /absolute/private/fixtures \
  --memory-gib 16 \
  --output /absolute/private/film-evidence
```

Omit `--fixtures` for a wholly synthetic catalogue. `--fixture ID` selects a
registered case and may repeat; otherwise every entry runs. A contained-failure
experiment must explicitly name its expected outcome with `--expect-outcome`.
The verifier never retries an OOM or increases the budget automatically.
For an expected failure, `--recovery-fixture ID` explicitly adds a successful
small fixture after each selected failed case in the same instance. It requires
explicit `--fixture` selections and a different recovery fixture of at most two
million pixels.
`--lifecycle-fixture ID` adds crash recovery before container start, at both
permits, and after final result capture, cancellation before engine release, and
a subsequent successful attempt.
Use a registered fixture of at most two million pixels for these checks. A TIFF
lifecycle case also verifies detection of a changed private source copy.

`--failure-fixture ID` requires a registered TIFF of at most two million pixels.
It verifies rejection of a retained snapshot writer, exhaustion of the shared
tmpfs bytes and inodes, and an injected per-file limit failure, each followed by
successful work.
These operator faults affect only the exact owned attempt. The storage probe
fills its host-only native directory after sealing; those host-charged pages
are storage-cap evidence and are excluded from memory qualification. The
file-limit probe freezes the owned container, verifies the fixed Python child,
and lowers that child's existing 512 MiB file limit to 4096 bytes before resuming.
It establishes the resource-failure path, not natural exhaustion at 512 MiB or
a particular choice between the kernel's EFBIG and SIGXFSZ mechanisms.

The verifier checks exact output descriptors, settled cleanup, real retained
resource evidence, and an independent synthetic Web Library. An Album rename
must survive a Web container restart. Its worker executes the same fixed adapter
and fresh-cache procedure used by measurement. Sparse stage timings represent
only directly observed phases; absent inner-stage or reclaim timings are not
invented. Whole execution time and aggregate kernel evidence remain separate.

Host-side document and fixture-preparation checks run without Docker or root:

```sh
bun run test:processing-tools
```

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
