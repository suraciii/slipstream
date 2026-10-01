# Deployment

The Library service ships as one Docker image containing the Rust server, the built Web
application, native runtime libraries, and `curl` for the `/healthz` check.
There is no Node, Bun, Sharp, or Node-API runtime in the image. This guide
defines the supported Linux-local Docker deployment contract. Backup,
acceptance, and rollback step-by-step procedures are operator material and live
with the deployment, not in this repository.

## Optional Standard Metadata Save

The default read-only Compose Library supports Read Metadata, not Save. Save
requires the exclusive deployment described in
[Standard Metadata design](../design/library-management-metadata.md#supported-deployment-shape).
The host supervisor is a trusted root component; the Web service must receive
only its private Unix socket and a read-only bind of the Library Folder. It
must not receive the backing tree directly, host root privilege, the systemd
bus, or the Docker socket. Keep the writer identity distinct from Web and
reserve it for this instance's Samba workers and metadata helper.

The repository supplies the Python standard-library
[`supervisor.py`](../tools/metadata/supervisor.py),
[`provision.py`](../tools/metadata/provision.py), and the
[`supervisor`](../systemd/slipstream-metadata-supervisor%40.service) and
[`Samba`](../systemd/slipstream-metadata-smb%40.service) systemd templates.
Install on a systemd/cgroup-v2 host with Python 3, Samba (`smbd`, `testparm`,
`smbpasswd`), util-linux (`findmnt`), and an ext2/ext3/ext4, XFS, or Btrfs
backing filesystem. Set `fs.protected_hardlinks=1`. Samba must recognize
`smb3 directory leases`; unknown parameters make Save unavailable rather than
silently weakening the boundary. Do not install template overrides or replace
the supplied `KillMode=control-group` and `ExecStopPost` recovery.

For an isolated disposable instance, place the built save helper at a
root-owned executable path with root-owned, non-writable ancestors, and run:

```sh
sudo python3 tools/metadata/provision.py --instance metadata-qa \
  --source /srv/disposable-photo-fixtures \
  --helper /usr/local/libexec/slipstream-metadata-save-helper \
  --web-user slipstream-web --listen-address 127.0.0.1 --smb-port 1445
sudo smbpasswd -c /etc/slipstream-metadata/metadata-qa/smb.conf \
  -a ssmeta-metadata-qa
sudo systemctl start slipstream-metadata-smb@metadata-qa.service \
  slipstream-metadata-supervisor@metadata-qa.service
```

The Web account must already exist and be non-root. Provisioning copies the
source into a new private instance; it never changes the source or an existing
instance and never stops the host's ordinary Samba service. It creates a
dedicated non-login writer account, root-owned sticky/setgid `3770`
directories, root-owned `0440` Originals, and writer-owned Sidecars. It refuses
source symlinks in every path component before resolving them and rejects
hardlinks and special files. The private backing parent is exactly root:writer
`0710`, and every host-side ancestor must be traversable without extended
attributes. Inside the tree, only Samba's own `user.DOSATTRIB` attribute is
admitted so ordinary external attribute edits do not invalidate admission;
any other extended attribute or ACL must be removed before Save is admitted.
Shared installed templates and supervisor code must be identical when
provisioning additional instances.
The units are installed below `/usr/local/lib/systemd/system`, so a runtime
mask can actually override them. Do not install a higher-priority instance or
template unit under `/etc/systemd/system`.

Configure the server with `SLIPSTREAM_METADATA_SUPERVISOR` set to the socket
`/run/slipstream-metadata/metadata-qa/supervisor.sock` and mount
`/var/lib/slipstream-metadata/metadata-qa/library` read-only into its container.
Make the bind as root; Web cannot traverse the private host ancestor. Preserve
the configured numeric Web UID/GID in the container. Restrict the socket bind
to this Web deployment. External applications use only the isolated SMB share
`library`; for a non-disposable deployment select a private/Tailscale listen
address and restrict its port accordingly. The supplied profile disables
oplocks, kernel oplocks, SMB and directory leases, durable handles, and
clustering (therefore persistent handles); status checks `testparm`'s effective
values and rejects unrecognized configuration. Do not grant direct backing
access to another account, reuse the writer account in another service or
container, enable extra shares, or give Samba restart authority to Web.

The provisioned copy is the admitted Library. New Originals or directories
created through SMB are not automatically admitted: stop this instance's SMB
service and supervisor, then have the deployment administrator inspect and
re-own the new Originals as root:writer `0440` and directories as root:writer
`3770` before scanning. Sidecars remain writer-owned with no access for others.
Reject links, nested filesystems, ACLs, and extended attributes. Status and Save
reinspect the entire tree and refuse until these invariants hold. Originals
remain readable but cannot be rewritten, replaced, unlinked, or hardlinked by
the writer. Back up SQLite state and the backing tree together according to
the ordinary deployment's backup policy.

### Supervisor Protocol and Recovery

The root-owned immutable JSON at
`/etc/slipstream-metadata/INSTANCE/config.json` contains exactly `backingRoot`,
`writerUid`, `writerGid`, `webUid`, `webGid`, `serviceUnit`, `helper`, and
`smbConfig`. Paths are absolute; the service unit is fixed to
`slipstream-metadata-smb@INSTANCE.service`. Requests cannot select executable,
unit, or path. Only the configured Web UID passes Unix `SO_PEERCRED` admission;
other peers are disconnected without a response. One UTF-8 JSON object plus a
newline is accepted per connection, at most 2 MiB including the newline; one
JSON response plus newline is returned, at most 32 MiB. Connection input/output
has a five-second timeout.

`{"operation":"status"}` returns `{"available":true}` or
`{"available":false,"reason":"actionable prerequisite failure"}`. This is a
prerequisite check, not a qualification certificate. A save request is
`{"operation":"save","request":HELPER_REQUEST}`. The nested request is opaque
to the supervisor; the helper owns its validation. Save returns the helper's
single `{"ok":VALUE}` or `{"error":{"code":CODE,"message":MESSAGE,"details":VALUE}}`
object. Supervisor-generated errors use `details:null`: malformed requests use
`invalid_input`, failure before helper invocation uses `save_unavailable`, and
timeout, crash, malformed/oversized helper output, or recovery failure after
invocation uses `outcome_unknown`. A lost connection does not prove no change;
refresh metadata evidence before another save.

Save holds an instance-level `flock`, creates a recovery marker, runtime-masks
the managed SMB service, verifies the mask, stops it with a bounded timeout,
and reads its retained cgroup-v2 `cgroup.events` to prove `populated=0`. After
rechecking identities and the tree, it launches the fixed helper as writer
with no supplementary groups and an inherited Unix socketpair lease whose
peer credentials identify root. The supervisor holds the peer until helper
exit. Helper input/output is bounded and the helper has a 30-second deadline.
The helper's entire descendant cgroup is killed and observed empty before
unmasking and restarting SMB. Systemctl operations each have a ten-second
deadline. A supervisor SIGKILL invokes systemd `ExecStopPost`, which proves
helper descendants exited before releasing the mask; ordinary restarts use the
same recovery. Before the helper starts, the supervisor writes an atomic,
root-owned publication record in its private runtime directory. The record
pins the lease-derived temporary name and the parent device and inode. Recovery
may remove only that exact regular, single-link artifact after reproving the
fence and the parent identity; it must not glob temporary-looking names. A
missing or invalid record is retained and reported as an actionable refusal.
The record is removed before the service is released, while an interrupted
record is discarded only from the private runtime directory. If descendants
cannot be stopped, recovery retains the mask and refuses, rather than
admitting concurrent writers. The persistent runtime marker also drives
recovery after supervisor restart.

Before treating a deployment as writable, independently exercise real helper
publication and refusal, concurrent SMB writes and restart attempts during the
fence, helper timeout/crash, supervisor SIGKILL and ordinary restart, and
automatic SMB recovery. Inspect actual cgroup emptiness, Original hashes, and
external-tool Sidecar read-back. A reachable socket, `available:true`, or active
units alone does not qualify the environment. Lightroom interoperability must
not be claimed without exercising Lightroom. The default Compose deployment
remains read-only; this profile does not silently make it writable.

## Optional Photo Development

Photo Development is an optional capability inside the same application image.
The image may carry the bundled engine extension: the pinned native
darktable-mcp runtime, its native assets and module/tool metadata, the
processing manifest, and the ICC profile. A normal start runs one application
container. There is no host processing launcher, processing systemd unit,
worker container, processing control socket, or processing Compose overlay to
install or enable.

The server detects the extension at startup. It verifies the manifest identity
and every named asset digest, including the manifest's own SHA-256 recorded in
the bundle. A deployment without the extension, or one whose assets are
missing or invalid, keeps every Library operation available and reports
development unavailable. Startup configuration cannot be changed by an HTTP
request.

The capability is controlled by one startup environment variable:

- `SLIPSTREAM_PHOTO_DEVELOPMENT=enabled` forces the capability on; startup
  still requires a valid bundle and reports the bundle unavailable when the
  assets do not verify.
- `SLIPSTREAM_PHOTO_DEVELOPMENT=disabled` forces the capability off.
- `SLIPSTREAM_PHOTO_DEVELOPMENT=auto`, also the default when the variable is
  absent, enables the capability exactly when the installed bundle verifies.

`SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY` optionally selects an absolute bundle root
for development and smoke runs, such as a locally built bundle outside an
image. Production images carry the bundle at its fixed installed location
(`/opt/slipstream-photo`); the variable is not a production processing
endpoint. `SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES` keeps its existing
meaning: the finite retained-output allowance in bytes that bounds published
Development TIFF artifacts, so exhausting it refuses export admission with
`resource_unavailable` instead of filling the state volume; the repository
Compose file defaults it to 8589934592 and the environment file may override
it. The Compose file fixes the documented shared container bounds — 8 GiB
memory, 4 CPUs, 512 PIDs, and 4 engine OpenMP threads — and mounts no Docker
socket.

The one container's finite memory and CPU allocation is shared by the Web
service and the engine. Development requests are serialized: at most one runs
at a time, and a second request queues behind it. There is no per-attempt
isolation domain; an engine failure can therefore stop the whole application,
and the ordinary restart path recovers. This trade-off fits the personal
single-machine deployment and is not
a multi-user service boundary.
[Processing Memory](../design/processing-memory.md) owns the details. Legacy
`SLIPSTREAM_PROCESSING_INSTANCE`, `SLIPSTREAM_PROCESSING_SOCKET`,
`SLIPSTREAM_PROCESSING_POLICY_SHA256`, and `SLIPSTREAM_PROCESSING_BUNDLE_SHA256`
declarations are rejected by the Compose entry point; the retired launcher
endpoint they named no longer exists.

### Building the engine bundle

Build the optional engine bundle with the repository helper. It verifies a
clean native source checkout at the exact commit, performs the engine build,
discovers the MCP tools, modules, and schemas, and derives the bundle manifest
and identity:

```sh
python3 tools/processing/photo/build.py \
  --darktable-source /absolute/path/to/darktable-native \
  --darktable-commit <40-lowercase-hex-commit> \
  --tag slipstream:photo-local
```

The helper builds (or reuses) the normal application runtime and extends it
with the pinned native engine, the discovered MCP metadata, the vendored ICC
output profile, and the deterministic bundle manifest. Pass `--app-image` to
extend an already-built immutable application image instead of rebuilding the
runtime, or `--app-tag` to build it under a different local tag. It prints the
application image ID, the extended image ID, the 64-character bundle digest,
and the native commit as one JSON object. Use the extended image's immutable
ID as the deployment's `SLIPSTREAM_IMAGE` with the ordinary Compose start;
there is no processing-specific command or overlay. The
manifest records the pinned native module map and asset digests; it names no
worker image and no launcher configuration. The RAW reference qualification
and the acceptance runner are documented in
[tools/processing](../tools/processing/README.md).

### Verification

`GET /api/processing/capability` reports the closed capability condition
following the merged Photo Development service surface
(`design/photo-development.md`): `disabled` when the operator has disabled the
capability, `bundle-unavailable` when an enabled path has missing or invalid
engine assets, `source-unsupported`, `resource-unavailable`, and `ready` only
when the bundled engine verifies at startup. It never reports a qualification
harness profile as product capability. `/healthz` remains a Library readiness
check and reports no processing state.

Accept the deployment for processing only after the local smoke path exercises
a real qualified RAW development end to end through this one-container path:
Export before Preview, download validation, cancellation, deadline, engine
failure, restart, scratch cleanup, and unchanged Original and external-XMP
digests. The workflow acceptance runner and the real-camera smoke commands
are documented in [tools/processing](../tools/processing/README.md) and
[CONTRIBUTING.md](../CONTRIBUTING.md). Do not claim the
RAW-to-TIFF-to-Film-to-JPEG workflow until its separate product acceptance
Issues pass; Film remains unavailable.

## Configuration

Serve the Web application and API on the same HTTP or HTTPS origin. Local use
defaults to `http://localhost:3000` and needs no certificate, domain, or reverse
proxy. Direct HTTP on an explicitly configured network interface is also
supported; the browser and CLI warn that network observers can read photos and
credentials. HTTPS is recommended for public or untrusted networks. Installation
has the secure-context requirements in [Installed Web Application](installed-web-app.md).

Create an environment file outside the repository:

```dotenv
SLIPSTREAM_IMAGE=registry.example.com:5000/slipstream/release@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
SLIPSTREAM_LIBRARY_ROOT=/srv/slipstream/originals
SLIPSTREAM_STATE_DIRECTORY=/srv/slipstream/state
SLIPSTREAM_CACHE_DIRECTORY=/srv/slipstream/cache
SLIPSTREAM_BIND_ADDRESS=127.0.0.1
SLIPSTREAM_PORT=3000
# Optional: SLIPSTREAM_PUBLIC_ORIGIN=http://photos.local:3000
```

`SLIPSTREAM_BIND_ADDRESS` controls the host-side published interface and defaults
to loopback. Setting an origin never changes it. To use direct HTTP from a trusted
network, explicitly set the bind address and an HTTP origin reachable from those
devices. The Rust process listens on the container network; Docker controls host
publication. An omitted origin uses `http://localhost:<published port>`.

[Instance Access](access.md) defines token provisioning and access behavior.
[Instance Access Architecture](../design/access.md) owns credential enforcement,
canonical-origin validation, and private response caching. Provision a token before
opening the Library. An absent token never enables anonymous access, including on
loopback. Forwarded headers do not establish identity.

For an existing HTTPS deployment, set `SLIPSTREAM_PUBLIC_ORIGIN=https://photos.example.com`
and retain the TLS proxy. Keep its backend private and disable proxy caching for
private responses. Slipstream does not manage certificates or listen with TLS itself.

For upgrade, the operator must keep public ingress closed, back up application
state, provision authentication through local administration with the server
stopped, and verify private route rejection before opening ingress. Old publicly
cacheable image paths and proxy caches must be retired under the access design.
A restore requires token rotation before reopening access. A rollback to a
binary without authentication must remain isolated from public traffic.
Operator acceptance tooling must use the credential for private API probes;
`/healthz` remains a minimal readiness check and is not authentication evidence.

## Access Administration

When supplied, `SLIPSTREAM_PUBLIC_ORIGIN` must contain a canonical HTTP or HTTPS
origin with no userinfo, query, fragment, or path other than `/`. It contains no
secret. When omitted, it uses `http://localhost:<port>`; Compose uses the published
host port. The environment file selects the origin. Trailing `/` is canonicalized;
matching uses parsed scheme, host, and effective port.

The supported Compose entry point adds `access-create`, `access-rotate`, and
`access-revoke`. Each accepts the same single environment file and immutable
image selection as existing Compose operations, runs host storage preflight,
and starts a one-shot `slipstream-server` container with the matching subcommand.
It must not start the ordinary HTTP service or scan the Library. These commands
use admitted existing state or initialize only a valid new state directory.
They must acquire the same exclusive process lock used by server startup and
Library expansion; an active owner causes failure without touching state.

```sh
./scripts/compose --env-file /srv/slipstream/instance.env down
./scripts/compose --env-file /srv/slipstream/instance.env access-create
./scripts/compose --env-file /srv/slipstream/instance.env up -d
```

`access-create` refuses an already configured credential. `access-rotate`
requires existing authentication state, including explicitly revoked state;
`access-revoke` is idempotent for a revoked credential. Rotation and revocation
must use the same stopped-server procedure above. Keep public ingress closed
until acceptance is complete. No command accepts a caller-supplied token.

Creation and rotation require an interactive terminal and present the token
once after durable commit; the wrapper must not capture or log that output.
The one-shot container must disable Docker logging with the `none` logging
driver and attach the private terminal directly. The wrapper must not use a
logging or tee subprocess. They print secrets to the attached terminal, not
container logs or stdout pipelines. If terminal delivery fails after commit, report uncertainty and
instruct the operator to rotate; never roll back a successfully committed
credential or disclose it on a later read. Revocation prints only confirmation.
Underlying `slipstream-server access-create|access-rotate|access-revoke` uses
the same existing storage environment and exit 0 for confirmed success, 1 for
failure. No operation may modify Originals or clear Library state.

The configured Web/CLI origin must remain reachable from the operator host.
[CLI Reference](cli-reference.md) owns credential-file selection. Retain the minimal
unauthenticated `/healthz` probe for container readiness.

## Storage Rules

- For `up`, Library Expansion, and access administration, `SLIPSTREAM_IMAGE` must occur exactly once as
  an unquoted, literal immutable image reference. Its repository path uses
  lowercase components and may include a registry port. It must end in
  `@sha256:` followed by 64 lowercase hexadecimal characters. Shell expansion,
  Compose interpolation, and mutable tags are not supported. Supported Compose
  operations run that digest-pinned image only; they never build an image from
  the repository as a fallback.
- For ordinary `up`, Library Expansion, and access administration, the Originals, state, and cache directories
  must already exist. Each must occur exactly once as `KEY=/absolute/path` in
  the environment file. QA runs derive these three paths from the explicit
  QA ID instead; they reject storage declarations in the QA environment file. Its value must be an unquoted, valid UTF-8 literal. It
  may contain ordinary internal spaces, but must not have leading or trailing
  whitespace, tabs, carriage returns, control characters, `#`, quotes,
  backslashes, backticks, or `$`. Shell expansion and Compose variable
  interpolation are not supported for these three values. The same rule
  applies after resolving a symlink or lexical alias and to returned mount
  paths, so a safe-looking alias cannot hide an ambiguous target path or byte
  encoding. Before parsing any line, the entry point rejects a raw NUL byte
  anywhere in the environment file, including keys, values, or trailing
  content.
- The Originals directory must be readable by UID 1000. State and cache
  directories must be writable by UID 1000.
- Originals, state, and cache must be pairwise disjoint after symlinks,
  lexical aliases, and the complete Linux nested-mount source coordinates
  resolve. No pair may equal, contain, or be contained by the other.
- Never run two Slipstream processes against one SQLite database.

`GET /healthz` returns `200 {"status":"ok"}` after storage admission, Preview
startup, and HTTP bind. `GET /api/status` separately reports Library
publication and scan state.

## Compose Entry Point

Run supported repository Compose operations through `scripts/compose`. It
requires one environment file and always uses this repository's `compose.yaml`;
for a QA instance it also uses only the repository-owned `compose.qa.yaml`
override:

```sh
./scripts/compose --env-file /path/to/slipstream.env up -d
```

The entry point requires Bash, GNU `realpath`, `tr`, and `cmp`, `iconv`, and
Linux `findmnt`. The one environment file supplies the QA image, origin, and loopback endpoint;
the QA storage paths are derived and must not appear in it. The bounded
operator surface supports only these command forms:

```sh
./scripts/compose --env-file /path/to/slipstream.env up
./scripts/compose --env-file /path/to/slipstream.env up -d
./scripts/compose --env-file /path/to/expanded-library.env run --rm --no-deps slipstream expand-library
./scripts/compose --env-file /path/to/slipstream.env down
./scripts/compose --env-file /path/to/slipstream.env access-create
./scripts/compose --env-file /path/to/slipstream.env access-rotate
./scripts/compose --env-file /path/to/slipstream.env access-revoke
./scripts/compose --env-file /path/to/slipstream.env --qa 0123456789ab up -d
./scripts/compose --env-file /path/to/slipstream.env --qa 0123456789ab down
```

For `up`, Library Expansion, and access administration, it validates the required image input and
resolves the three host storage sources and captures each endpoint's complete
nested mount hierarchy before it invokes Compose. It passes every resolved
source both as the container path and as the server configuration value, so
Docker mounts the sources that the preflight checked and the server sees their
real topology. It rejects any equal, nested, symbolic-link-alias, or Linux
bind-mount-alias pair—including aliases exposed through a nested mount on
another filesystem—before it invokes Compose or changes the Originals tree or
content.
It also rejects alternate Compose files (except its own QA override),
additional environment files, mount, environment, or entrypoint overrides,
and unsupported Compose commands. The
server retains its storage admission as a second safety boundary. The entry
point fixes the ordinary Compose project name as `slipstream`; the explicit
[Short-Lived QA Instance](#short-lived-qa-instance) selects only its own
bounded project name. The entry point rejects any `COMPOSE_*` or `DOCKER_*`
declaration in the environment file.
For the finite startup Compose configuration surface, the environment file also
wins over ambient `SLIPSTREAM_IMAGE`, `SLIPSTREAM_BIND_ADDRESS`,
`SLIPSTREAM_PORT`, `SLIPSTREAM_PUBLIC_ORIGIN`, and `SLIPSTREAM_DATABASE_BASENAME` values. It also clears ambient
`SLIPSTREAM_LIBRARY_ROOT`, `SLIPSTREAM_STATE_DIRECTORY`, and
`SLIPSTREAM_CACHE_DIRECTORY` before startup exports its checked canonical
values. An absent `SLIPSTREAM_PUBLIC_ORIGIN` selects the localhost HTTP default;
a malformed value must fail rather than use an ambient origin for startup. Access administration
must likewise clear ambient origin values; its offline state operation does not
require a public origin. These commands use the same image/storage preflight
and precedence, with interactive and logging behavior from Access Administration.

`compose.yaml` intentionally declares no Docker restart policy. Every container
start must be initiated through `scripts/compose` so the host storage preflight
runs first. If automatic recovery is needed in the future, it must be provided
by a host supervisor that uses a preflight-aware start path; do not enable
Docker autonomous restarts that can bypass this wrapper.

This contract supports only a Linux host using its local Docker Engine. Do not
set `DOCKER_HOST` or `DOCKER_CONTEXT`; the entry point rejects them and rejects
a Docker default context that is not a local Unix socket. Direct `docker
compose` invocation is outside the supported deployment contract. The
ordinary `down` form intentionally skips image and storage-source preflight, so an
operator can stop an existing container after a source path or image input
becomes unavailable or unsafe. While Compose parses that fixed stop operation,
the entry point supplies fixed internal stop-only image and storage values
instead of using the environment file values. They do not select, pull, build,
or run an image, and they do not access storage paths. This exception applies
only to a readable, Compose-parseable environment file; its image and storage
values may be missing or unsafe. The operation still uses the repository
Compose file and local Docker context.

The operator must trust the local Docker daemon and socket, and that daemon
must use the same host mount namespace as `scripts/compose`. The three storage
directories and their parent paths must remain stable between the preflight and
Docker admission. The preflight does not eliminate this time-of-check/
time-of-use window; it rejects unsafe topology that is present when it checks.

## Short-Lived QA Instance

A qualification instance may run beside the ordinary service on the same
trusted local Docker Engine. Supply `--qa ID` immediately after the environment
file argument, before the action. `ID` must be exactly 12 lowercase hexadecimal
characters; the launcher selects only the `slipstream-qa-ID` Compose project.
A misplaced, repeated, missing, or invalid QA ID fails before Docker and never
falls through to an ordinary action. Omitting `--qa` retains the ordinary
`slipstream` project. Neither identity is selected by ambient Compose variables
or by the environment file.

```sh
./scripts/compose --env-file /path/to/qa.env --qa 0123456789ab access-create
./scripts/compose --env-file /path/to/qa.env --qa 0123456789ab up -d
./scripts/compose --env-file /path/to/qa.env --qa 0123456789ab down
```

The operator creates a new `dist/qa-ID/` fixture root in the entry point's
own checkout, with existing `originals`, `state`, and `cache` subdirectories.
Use a fresh ID for each qualification run: reusing an ID also reuses its
credential, scan state, and cache. For QA startup and administration, the
launcher derives these three canonical storage paths from its own checkout
and ID, rather than trusting paths supplied by the caller. It rejects an
environment file declaring any of the three storage keys before Docker.
The fixture root and its ancestors below the checkout must not be symlinks or
bind mounts; no mount may appear inside it. The launcher verifies this before
Docker, so a QA path cannot be an alias into production storage. The operator
must never copy production state or Originals into a QA fixture.

For QA `up` and `up -d`, the environment file must satisfy the ordinary
immutable-image and optional HTTP/HTTPS-origin grammar. The launcher checks an
explicit `SLIPSTREAM_BIND_ADDRESS=127.0.0.1` and an explicit decimal
`SLIPSTREAM_PORT` from 1024 through 65535 before Docker. The port must differ
from the ordinary instance's published port; Docker refuses an occupied port
at startup. Other QA actions do not publish a port; `access-revoke` does not
require an origin. The repository-owned `compose.qa.yaml` is the only
additional Compose file. It adds no services, mounts, or restart policies and
applies memory (2 GiB), CPU (2), and PID (256) limits to both `slipstream` and
`slipstream-admin`, without changing the ordinary configuration.

The supported QA actions are the same as the ordinary bounded entry point.
QA `down` requires a valid ID, readable environment file, and the usual
rejection of `COMPOSE_*`/`DOCKER_*` declarations. It uses the same stop-only
sentinels and the fixed QA Compose files, without checking unavailable fixture
paths, image, or origin. It targets only the named QA project and never removes
host directories. Access creation and rotation retain the interactive, no-log
token-delivery contract. Qualification may use direct loopback HTTP or a private
HTTPS proxy with a trusted client, exercising the selected transport.
Containers share the local Docker daemon and host capacity: this mode does not
turn an untrusted workload into an isolated security domain.

## Image

Build from a clean checkout of the exact commit:

```sh
commit=$(git rev-parse HEAD)
docker buildx build --platform linux/amd64 --load \
  --build-arg "SLIPSTREAM_VCS_REF=$commit" --tag slipstream:local .
```

Record the image digest. Verify the image user is `1000:1000` and that no
Node, Bun, npm, Sharp, or Node-API runtime artifact is present before an
operator-controlled deployment. Operators may script these checks; any
equivalent inspection is acceptable.

## Reproducible Inputs

Slipstream supports Linux amd64 release images. The Dockerfile fixes every
non-scratch base image by digest. Its Ubuntu build and runtime inputs use the
official snapshot and direct package locks in [`../docker/apt/`](../docker/apt/).
The RAW decoder uses the upstream LibRaw `0.22.2` archive with its SHA-256
verified during the image build; the image's isolated library must match its
build headers. The [container input design](../design/container-inputs.md)
defines ownership and update rules.

Use the explicit `docker buildx build --platform linux/amd64` command above
for release qualification. Supported Compose does not build from this checkout:
it runs only the digest-pinned image named by `SLIPSTREAM_IMAGE`, with no source
fallback. A normal `docker compose` build is not qualification evidence.
The GitHub Actions `ubuntu-latest` is a source and test runner. It is not a
release-image input, and its native test dependencies do not prove the native
packages in a release container.

These checked-in inputs provide source reproducibility: rebuilding one exact
candidate selects the same base image indexes and native package inputs. They
do not promise a byte-identical output image. A resulting digest, timestamp,
or other output record establishes a single build's traceability only.

Change an image digest, the Ubuntu snapshot, either direct package lock, or
the LibRaw release and its verified checksum in one reviewed dependency update.
Do not substitute a moving tag, archive, or fallback mirror.

## Qualification Evidence

Release qualification is operator work. It must use the exact candidate and
the image command above. Keep its evidence outside the repository with the
private deployment material.

Record all of the following:

- exact candidate SHA and the `linux/amd64` build command;
- OCI revision and immutable digest from the final image;
- final native package versions from `dpkg-query` in that image;
- the generated SBOM and the tool identity used to generate it; and
- advisory scanner identity, advisory database timestamp, findings, and their
  disposition.

The advisory database timestamp states what data the scanner used for that one
qualification. It does not freeze future advisory results or replace a later
advisory review.

## Backup

The deployment precondition for image cutover, expansion, and any state
recovery is a transactionally consistent backup of both the Library state
database and the Access database. Stop the service first so the two SQLite
stores share one quiescent recovery point. Use SQLite's backup API (or a
filesystem snapshot with equivalently proven consistency), and verify that
both databases can be restored together without losing the credential.
Retained Export artifacts are not reconstructible from these databases; an
operator backup that excludes them must refuse while any are retained.
Restore into a proven isolated copy, never over live state. The 0.1 support
boundary, rollback criteria, and rollback-artifact retirement rules are defined
in [`0.1-support-and-release.md`](0.1-support-and-release.md).

## Library Expansion

Library Expansion replaces the current Library Folder with a canonical
ancestor that contains the same current Folder. It does not support an
unrelated move, multiple roots, or per-file relinking.

1. Stop every Slipstream process using the state database. Preserve sidecars
   for recovery instead of deleting them.
2. Create and record a verified canonical schema-v6 backup (see Backup).
3. Change `SLIPSTREAM_LIBRARY_ROOT` to the proposed canonical ancestor. Keep
   the state directory and database basename unchanged. Ensure the proposed
   Folder is mounted read-only at the same absolute path inside the container.
4. Run the candidate image once without starting the service:

   ```sh
   ./scripts/compose --env-file /path/to/expanded-library.env \
     run --rm --no-deps slipstream expand-library
   ```

The offline command rejects a running database, sidecars, non-v6 state, an
unrelated Folder, descriptor mismatch, invalid remembered Locations, and
scan-limit failures. It commits the binding and Location changes in one
admitted transaction, then completes a normal scan before reporting success.
It never binds HTTP and uses the same storage configuration regardless of
listener settings.

If preflight or the transaction fails, the prior Folder binding is unchanged.
If the post-commit scan fails, do not expose the service as ready: correct the
failure and retry startup, or restore the verified pre-expansion backup and
prior Folder.

## Cutover and verification

1. Confirm the target image digest and the verified state backup.
2. Start the digest-pinned image with the prepared environment:

   ```sh
   ./scripts/compose --env-file /path/to/slipstream.env up -d
   ```

3. Verify `GET /healthz`, the Library Overview, one bounded File Location
   read, an Album browse read, one state mutation, undo, and a derivative read.
   Compare the SQLite schema and
   database binding with the pre-cutover record.
4. Confirm Original File hashes are unchanged.

Operators verify exact persisted state by comparing a bounded protocol
traversal or an offline read-only state projection against the pre-cutover
record; no HTTP route materializes the complete Library, complete Folder tree,
complete recursive Folder membership, or complete Album membership. Acceptance
tooling is operator-provided.
