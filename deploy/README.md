# Slipstream deployment and rollback

This procedure deploys the Rust server image without modifying Photographer-owned
Original Files. The repository contains only the Rust production server; the
sealed rollback artifact used by the completed live rollback proof is maintained
outside this source tree under `/data/slipstream`. It is intentionally
operator-controlled: do not use it as an automated live deployment from CI.

## Configuration

Create a deployment environment file outside the repository (or export these
variables in the shell that runs Compose):

```dotenv
SLIPSTREAM_IMAGE=slipstream:<immutable-release-tag>
SLIPSTREAM_LIBRARY_ROOT=/srv/slipstream/originals
SLIPSTREAM_STATE_DIRECTORY=/srv/slipstream/state
SLIPSTREAM_CACHE_DIRECTORY=/srv/slipstream/cache
SLIPSTREAM_BIND_ADDRESS=127.0.0.1
SLIPSTREAM_PORT=3000
```

`SLIPSTREAM_BIND_ADDRESS` controls the host-side published interface and
defaults to `127.0.0.1` when it is not set. Keep that default for local use.
For a Tailscale-only deployment, set it to the host's Tailscale interface
address (for example, `100.64.0.12`) and do not publish the port on any other
interface. The Rust process still listens on the private container network
interface; Docker's host-side publication is the exposure boundary.

The Originals directory must already exist and be readable by UID 1000. The
state and cache directories must already exist and be writable by UID 1000:

```sh
test -d "$SLIPSTREAM_LIBRARY_ROOT" && test -r "$SLIPSTREAM_LIBRARY_ROOT"
test "$(realpath -- "$SLIPSTREAM_LIBRARY_ROOT")" = "$SLIPSTREAM_LIBRARY_ROOT"
install -d -o 1000 -g 1000 -m 0700 "$SLIPSTREAM_STATE_DIRECTORY" "$SLIPSTREAM_CACHE_DIRECTORY"
```

Do not place state or cache inside the Originals directory. The SQLite root binding stores the canonical Library path, so `SLIPSTREAM_LIBRARY_ROOT` must already be the canonical absolute path, not a symlink or lexical alias. Compose mounts Originals read-only at that same path inside the container. Do not change it after deployment. State and cache mount at `/state` and `/cache` as separate application-owned persistent directories.

## Pre-cutover checks

Run these checks from the repository checkout used to render Compose:

```sh
./scripts/verify-container.sh
SLIPSTREAM_IMAGE="$SLIPSTREAM_IMAGE" ./scripts/verify-container.sh
```

Build or pull the exact image tag, then inspect it before starting the service:

```sh
docker build --tag "$SLIPSTREAM_IMAGE" .
VERIFY_IMAGE=1 SLIPSTREAM_IMAGE="$SLIPSTREAM_IMAGE" ./scripts/verify-container.sh
```

Record the image digest, verify that the image user is `1000:1000`, and verify
that no `node`, `bun`, `npm`, Sharp, or Node-API runtime artifact is present. The
sealed rollback archive under `/data/slipstream` is external evidence and is not
part of the image or repository build.
Do not proceed if the image inspection or the Compose rendering check fails.

Before deployment, stop any existing Slipstream process that uses the target
state database. Never run two Slipstream processes against the same SQLite
database concurrently. Back up the state directory and
record the current Rust image and the location of the external rollback artifact:

```sh
tar --xattrs --acls -C "$(dirname "$SLIPSTREAM_STATE_DIRECTORY")" \
  -czf "slipstream-state-before-$(date -u +%Y%m%dT%H%M%SZ).tar.gz" \
  "$(basename "$SLIPSTREAM_STATE_DIRECTORY")"
docker compose --env-file /path/to/slipstream.env -f compose.yaml config >/dev/null
```

The backup is an operator recovery copy. Do not remove SQLite journal, WAL, or
shared-memory sidecars by hand; Slipstream's startup admission must classify
those files and require operator recovery when appropriate.

## Cutover

1. Confirm the target image digest and the state/cache backup.
2. Confirm the service is stopped if another process owns the target database.
3. Start the tagged Rust image:

   ```sh
   docker compose --env-file /path/to/slipstream.env -f compose.yaml up -d --no-build
   ```

4. Wait for the health check and inspect logs:

   ```sh
   docker compose --env-file /path/to/slipstream.env -f compose.yaml ps
   docker compose --env-file /path/to/slipstream.env -f compose.yaml logs --tail=100 slipstream
   curl --fail --silent --show-error \
     "http://${SLIPSTREAM_BIND_ADDRESS:-127.0.0.1}:${SLIPSTREAM_PORT:-3000}/healthz"
   ```

   The health endpoint must return exactly `{"status":"ok"}`. A healthy
   response proves process readiness; continue with the browser and state
   checks before declaring the deployment verified.

5. Check the browser review flow against the deployed service. Confirm that
   state mutations survive a restart, generated derivatives are written only
   under the cache mount, and an Original File's hash is unchanged.
6. Keep the previous Rust image and state backup available for rollback.

Compose enforces the deployment boundary: UID 1000, read-only root filesystem,
private tmpfs, all Linux capabilities dropped, `no-new-privileges`, an init
process, a 30-second stop grace period, SIGTERM shutdown, a restart policy,
read-only Originals, and a `/healthz` check that uses curl rather than Node.

## Rollback

Rollback is required if the Rust service does not become healthy, browser
behavior diverges, state cannot be reopened, or an Original safety check fails.
Do not delete state or cache during rollback.

1. Stop the Rust service and preserve its logs:

   ```sh
   docker compose --env-file /path/to/slipstream.env -f compose.yaml logs > /path/to/rollback-rust.log
   docker compose --env-file /path/to/slipstream.env -f compose.yaml down
   ```

2. Verify that no Rust process remains and that the target SQLite database is
   closed. Preserve any sidecars for operator recovery; do not checkpoint or
   rewrite them by starting a second process.
3. Restore the previously verified Rust image. Run it with the same
   `SLIPSTREAM_LIBRARY_ROOT`, state directory, database basename, and cache
   directory. Never start two Slipstream processes against one SQLite database:

   ```sh
   docker compose --env-file /path/to/slipstream.env -f compose.yaml up -d --no-build
   ```

4. Verify `GET /healthz`, photo-set reads, one state mutation, undo, and a
   derivative read. Compare the SQLite schema and database binding with the
   pre-cutover record. If startup reports recovery required, stop and use the
   state backup with the repository's controlled recovery process.
5. Confirm the Original hash is unchanged and record the rollback evidence.

Keep the previous Rust image and state backup until the rollback proof is
complete. This repository change does not perform a live deployment.
