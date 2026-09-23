# Deployment

The Library service ships as one Docker image containing the Rust server, the built Web
application, native runtime libraries, and `curl` for the `/healthz` check.
There is no Node, Bun, Sharp, or Node-API runtime in the image. This guide
defines the supported Linux-local Docker deployment contract. Backup,
acceptance, and rollback step-by-step procedures are operator material and live
with the deployment, not in this repository.

## Optional Photo Processing

Photo processing requires a separate digest-pinned engine image and an
operator-owned host launcher. The supported host must provide cgroup v2,
systemd, and a local Docker daemon using the systemd cgroup driver. The launcher
is a privileged component trusted by the operator. The Web container receives
only its private Unix socket; it must not receive the Docker socket, host root
privilege, or a writable cgroup mount.

The operator must configure an explicit finite processing allocation, a
qualified control-service allowance, bounded private workspace storage, and
finite receipt retention. The launcher and Library service must remain outside
the entire processing allocation. The launcher must retain accounting until
attempt settlement and reconcile unfinished ownership on restart.

Enabling this capability requires operator tooling to exercise the same launch,
limit, OOM, cancellation, restart, storage-exhaustion, and cleanup path used by
real processing. Verify Library operations during a contained processing failure
and a successful subsequent attempt. A healthy engine image or an open launcher
socket alone does not establish readiness. Missing or unqualified processing
must leave ordinary Library operations available. Processing capability and
source/bundle availability must be reported separately from `/healthz`.

[Processing Executor](../design/processing-executor.md) defines the private
execution contract. The
[qualification protocol reference](../design/processing-executor-protocol.md)
defines the explicit fixture-only launcher configuration and command. This mode
must report photo processing unavailable. A fixture qualification run, installed
launcher binary, systemd-active unit or reachable socket is not production
readiness.

Install the production launcher as a root-owned systemd service with a
root-owned host configuration and persistent journal. The service must reconcile
its durable attempt and manager identities before accepting new work. Its
private socket may be reachable during reconciliation, but admission must stay
unavailable until recovery succeeds; socket reachability is not readiness. A
restart must not erase blocked ownership or reset an admission
watermark. A policy or immutable image change requires the processing-enabled
Web service to stop first. Keep the launcher running until all accepted
attempts have settled and cleanup is confirmed, then stop the launcher before
atomically replacing its host configuration. Restart it and reconcile the
existing journal under the new policy before allowing admission or restarting
the processing-enabled Web service.
If reconciliation or any host, image, policy, headroom or resource check fails,
processing remains unavailable and the Library service remains usable.

The supported opt-in is the dedicated wrapper command:

```sh
./scripts/compose --env-file /srv/slipstream/instance.env processing-up -d
```

This command selects a repository-owned, fixed processing Compose overlay from
inside the wrapper. The operator cannot supply a Compose file, override, extra
mount or entrypoint. Ordinary `up` uses only the base Compose file and never
mounts a launcher endpoint, even if a processing socket path is present in the
operator environment. `processing-up` validates the configured endpoint identity
and mounts the whole dedicated runtime directory read-only. The directory may
contain only the fixed bounded Unix socket and its root-only persistent owner
claim; it exposes no launcher configuration, journal, Docker socket, systemd
control socket, host root or writable cgroup mount. The directory and its
ancestors must be canonical, symlink-free, root-owned and not writable by the
Web UID. That UID may search the directory and connect to the socket through a
named ACL, but cannot list the directory or read the claim. The whole-directory
mount makes a replacement socket pathname visible after launcher restart;
mounting only one socket inode would not. The launcher also checks peer
credentials and the exact instance, policy and bundle on every request.
Neither this command nor any environment value enables work outside launcher
admission.

If the launcher endpoint or policy is unavailable, `processing-up` must fail
closed without weakening Web isolation or changing an already-running Library
service. The ordinary `up` command remains the recovery path for Library
browsing. `/healthz` continues to report only Library service health; processing
capability reports whether the operator disabled the
path, whether an opted-in path is unavailable and why, or whether the exact
deployed launcher, policy, bundle and resource boundary are available. It
reports source and bundle availability separately and never advertises the
qualification profile as production capability.

Production acceptance tooling must exercise the packaged launcher and supported
Compose path on the target host. It checks exact Web, launcher, worker, bundle
and policy identities; socket and owner-claim ownership, parent mode/ACL,
symlink-free path, read-only whole-directory mount, replacement socket inode
after restart and peer UID; systemd/cgroup-v2/Docker
topology; that the launcher and control service remain outside the processing
subtree; and that complete attempts stay within their qualified finite memory,
zero-swap, CPU, task and storage limits. It retains and checks allocation,
OOM, cancellation, restart and cleanup evidence. It must also keep Library and
Album reads responsive during a contained processing failure, then complete an
exact subsequent successful attempt. A healthy endpoint, open socket, synthetic
fixture qualification or ordinary repository gate is not a substitute for
these deployment checks. Do not claim the RAW-to-TIFF-to-Film-to-JPEG workflow
until its separate product acceptance Issues pass.

## Configuration

For application installation, serve the Web application and its API on the
same trusted HTTPS origin through an operator-managed reverse proxy. Keep
the listener within the trusted-network boundary described below; TLS does
not add authentication. An HTTP LAN address is not a localhost development
exception. See [Installed Web Application](installed-web-app.md) for launch
and online-use behavior.

Create an environment file outside the repository:

```dotenv
SLIPSTREAM_IMAGE=registry.example.com:5000/slipstream/release@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
SLIPSTREAM_LIBRARY_ROOT=/srv/slipstream/originals
SLIPSTREAM_STATE_DIRECTORY=/srv/slipstream/state
SLIPSTREAM_CACHE_DIRECTORY=/srv/slipstream/cache
SLIPSTREAM_BIND_ADDRESS=127.0.0.1
SLIPSTREAM_PORT=3000
SLIPSTREAM_PUBLIC_ORIGIN=https://photos.example.com
```

`SLIPSTREAM_BIND_ADDRESS` controls the host-side published interface and
must remain private behind the HTTPS reverse proxy. The Rust process listens
on the container network; Docker publication and network policy must prevent
public clients from bypassing the proxy.

[Instance Access](access.md) defines token provisioning and access behavior.
[Instance Access Architecture](../design/access.md) owns credential enforcement,
canonical-origin validation, and private response caching. The operator must
configure a canonical HTTPS origin and provision a token before exposing the
Library. An absent token never enables anonymous access, including on loopback.
Forwarded headers do not establish identity. The proxy must disable caching for
private responses and expose only the intended origin with trusted TLS.

For upgrade, the operator must keep public ingress closed, back up application
state, provision authentication through local administration with the server
stopped, and verify private route rejection before opening ingress. Old publicly
cacheable image paths and proxy caches must be retired under the access design.
A restore requires token rotation before reopening access. A rollback to a
binary without authentication must remain isolated from public traffic.
Operator acceptance tooling must use the credential for private API probes;
`/healthz` remains a minimal readiness check and is not authentication evidence.

## Access Administration

`SLIPSTREAM_PUBLIC_ORIGIN` must contain the canonical HTTPS origin with no
userinfo, query, fragment, or path other than `/`. It is required at server
startup, including setup-required operation. It must be carried from the
operator environment file into the container. Trailing `/` is canonicalized;
matching uses parsed scheme, host, and effective port. It contains no secret.

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

The web/CLI HTTPS origin must remain usable from the operator host; authenticated
CLI access has no plaintext loopback exception. [CLI Reference](cli-reference.md)
owns credential-file selection. Retain the minimal unauthenticated `/healthz`
probe on the private hop for container readiness.

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
values. An absent or malformed `SLIPSTREAM_PUBLIC_ORIGIN` in the environment
file must not fall back to an ambient origin for startup. Access administration
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
immutable-image and canonical HTTPS-origin grammar. The launcher checks an
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
token-delivery contract. Qualification needs a private HTTPS proxy and a
separately trusted client; loopback HTTP alone is not a CLI endpoint.
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
The [container input design](../design/container-inputs.md) defines their
ownership and update rule.

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

Change an image digest, the Ubuntu snapshot, or either direct package lock in
one reviewed dependency update. Do not substitute a moving tag, archive, or
fallback mirror.

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
recovery is a transactionally consistent backup of the quiescent state
database, taken with the service stopped so no sidecar or in-flight write can
be torn. SQLite's backup API (or a filesystem snapshot with equivalently
proven consistency) is the requirement; the backup tool is operator-provided.
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
