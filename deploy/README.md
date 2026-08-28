# Slipstream deployment and rollback

This procedure deploys the Rust server image without modifying Photographer-owned
Original Files. The repository contains only the Rust production server; the
sealed rollback artifact used by the completed live rollback proof is maintained
outside this source tree under `/data/slipstream`. It is intentionally
operator-controlled: do not use it as an automated live deployment from CI.
The supported 0.1 deployment boundary, release notes, and rollback-artifact
retirement criteria are in
[`../docs/0.1-support-and-release.md`](../docs/0.1-support-and-release.md).

## Configuration

Create a deployment environment file outside the repository (or export these
variables in the shell that runs Compose):

```dotenv
SLIPSTREAM_IMAGE=slipstream:<immutable-release-tag>
SLIPSTREAM_VCS_REF=<exact-merged-commit>
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

Do not place state or cache inside the Library Folder. SQLite stores the admitted canonical Library Folder path, so `SLIPSTREAM_LIBRARY_ROOT` must already be the canonical absolute path, not a symlink or lexical alias. Compose mounts the Folder read-only at that same path inside the container. Change the Folder only through the stopped, verified Library Expansion procedure below. State and cache mount at `/state` and `/cache` as separate application-owned persistent directories.

## Pre-cutover checks

Run these checks from the repository checkout used to render Compose:

```sh
./scripts/verify-container.sh
SLIPSTREAM_IMAGE="$SLIPSTREAM_IMAGE" ./scripts/verify-container.sh
```

Build or pull the exact image tag, then inspect it before starting the service:

```sh
docker build \
  --build-arg "SLIPSTREAM_VCS_REF=$SLIPSTREAM_VCS_REF" \
  --tag "$SLIPSTREAM_IMAGE" .
VERIFY_IMAGE=1 \
SLIPSTREAM_IMAGE="$SLIPSTREAM_IMAGE" \
SLIPSTREAM_EXPECTED_COMMIT="$SLIPSTREAM_VCS_REF" \
./scripts/verify-container.sh
```

Record the image digest, verify that the image user is `1000:1000`, and verify
that no `node`, `bun`, `npm`, Sharp, or Node-API runtime artifact is present. The
sealed rollback archive under `/data/slipstream` is external evidence and is not
part of the image or repository build.
Do not proceed if the image inspection or the Compose rendering check fails.

Before deployment, stop every Slipstream process that uses the target state database. Never copy a running, unquiesced database and never run two Slipstream processes against one SQLite database. Create and verify the operator recovery backup:

```sh
export SLIPSTREAM_BACKUP_OUTPUT="/data/slipstream/backups/state-before-$(date -u +%Y%m%dT%H%M%SZ).tar.gz"
./scripts/backup-state.sh
docker compose --env-file /path/to/slipstream.env -f compose.yaml config >/dev/null
```

`backup-state.sh` uses SQLite's backup API to create a transactionally consistent snapshot instead of sequentially copying database bytes. It fails when a journal/WAL/SHM sidecar or any unexpected state entry exists, SQLite integrity or foreign-key checks fail, or the extracted archive differs from the verified snapshot. It never checkpoints, repairs, or rewrites the source database. Stopping the service remains the deployment precondition so the backup and image cutover share one quiescent boundary; the backup API also prevents a torn copy if that operational step is accidentally violated. A filesystem snapshot is acceptable only when it provides equivalently proven consistency and is verified before use.

The backup is an operator recovery copy. Do not remove SQLite journal, WAL, or shared-memory sidecars by hand; Slipstream's startup admission must classify those files and require operator recovery when appropriate.

## Library Expansion

Use this operation only to replace the current Library Folder with a canonical ancestor that contains the same current Folder. It does not support an unrelated move, multiple roots, or per-file relinking.

1. Stop every Slipstream process using the state database. Preserve sidecars for recovery instead of deleting them.
2. With `SLIPSTREAM_LIBRARY_ROOT` still set to the current Folder, create and record a verified canonical schema-v4 backup:

   ```sh
   export SLIPSTREAM_EXPECTED_SCHEMA_VERSION=4
   export SLIPSTREAM_BACKUP_OUTPUT="/data/slipstream/backups/state-before-expansion-$(date -u +%Y%m%dT%H%M%SZ).tar.gz"
   ./scripts/backup-state.sh
   ```

3. Change `SLIPSTREAM_LIBRARY_ROOT` to the proposed canonical ancestor. Keep the state directory and database basename unchanged. Ensure the proposed Folder is mounted read-only at the same absolute path inside the container.
4. Run the candidate image once without starting the service:

   ```sh
   docker compose --env-file /path/to/expanded-library.env -f compose.yaml run --rm --no-deps slipstream expand-library
   ```

   The command opens the old configured Folder from SQLite, proves it is the same descriptor-confined descendant of the proposed Folder, preflights the complete proposed Folder within scan limits, and then performs one admitted transaction. It prefixes all remembered Original Locations, updates Photo sort paths, changes the binding, and invalidates Location-derived Capture, Preview, and cache facts while retaining IDs and user-owned state. It completes a normal scan before reporting success.

5. Start the service and run production acceptance against schema version 4, the proposed Folder, the exact pre-expansion user-state projection, and representative Original hashes.

If preflight or the transaction fails, keep the prior Folder configuration; SQLite remains bound to it. If the post-commit scan fails, do not expose the service as ready. Correct the root-level failure and retry startup, or restore the verified pre-expansion v4 backup and prior Folder. To roll back the v3-to-v4 migration itself, restore the verified pre-migration v3 backup before starting the compatible v3 image; a v3 binary rejects v4 state.

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
   response proves process readiness only: storage, schema, sidecar, and
   cache admission, the Preview workers, and the HTTP bind. It does not prove
   that a Library scan has finished. Check the Library separately with
   `GET /api/status`, which reports `initializing`, `discovering`,
   `inspecting`, `applying`, `idle`, or `failed`; an existing published
   Library stays browsable while an ordinary background rescan runs. Then
   continue with the browser and state checks before declaring the
   deployment verified.

5. Run the fail-closed, read-only production acceptance command with exact expected values:

   ```sh
   export SLIPSTREAM_BASE_URL="http://${SLIPSTREAM_BIND_ADDRESS}:${SLIPSTREAM_PORT}"
   export SLIPSTREAM_DATABASE_BASENAME=${SLIPSTREAM_DATABASE_BASENAME:-library.sqlite}
   export SLIPSTREAM_EXPECTED_BIND_ADDRESS="$SLIPSTREAM_BIND_ADDRESS"
   export SLIPSTREAM_EXPECTED_PORT="$SLIPSTREAM_PORT"
   export SLIPSTREAM_EXPECTED_SCHEMA_VERSION=<current-schema-version>
   export SLIPSTREAM_EXPECTED_STATE_SNAPSHOT=/path/to/pre-backup-state.json
   export SLIPSTREAM_CONTAINER=slipstream-slipstream-1
   export SLIPSTREAM_EXPECTED_IMAGE_ID="$(docker image inspect --format '{{.Id}}' "$SLIPSTREAM_IMAGE")"
   export SLIPSTREAM_EXPECTED_COMMIT="$SLIPSTREAM_VCS_REF"
   export SLIPSTREAM_EXPECTED_PHOTO_COUNT=<known-count>
   export SLIPSTREAM_EXPECTED_PHOTO_SET=<known-name>
   export SLIPSTREAM_EXPECTED_MEMBER_COUNT=<known-count>
   export SLIPSTREAM_ORIGINAL_SAMPLE=/absolute/path/below/the/Library/root/sample.RAW
   export SLIPSTREAM_EXPECTED_ORIGINAL_SHA256=<known-sha256>
   # Required persisted real RAW Preview-fact assertion (does not generate a Preview):
   export SLIPSTREAM_PREVIEW_PHOTO_ID=<known-photo-id>
   export SLIPSTREAM_EXPECTED_PREVIEW_SOURCE=embedded-raw-jpeg
   ./scripts/verify-production.sh
   ```

   The expected state snapshot contains the exact `photos` and `photoSets` arrays recorded before backup. It binds Photo identities/order, availability, Selection State, Rating, Photo Set membership/order, review progress, and persisted Preview facts. The command validates exact health, waits for `GET /api/status` to report `idle` (configurable with `SLIPSTREAM_SCAN_WAIT_SECONDS`, default 3600, failing closed on `failed` or a timeout), that complete state snapshot, required persisted current Preview facts, sidecar absence, unchanged SQLite bytes, image tag/ID/revision, container health and hardening, the exact three mounts, restricted binding, runtime contents, and the Original SHA before and after. It deliberately does not call the demand-driven Preview endpoint because that GET may populate derived state or cache. Missing inputs or skipped checks fail closed.

6. Check the interactive browser review flow. Confirm that any intended state mutation survives a restart and generated derivatives remain below the cache mount.
7. Keep rollback artifacts according to the retirement criteria in the
   [0.1 Support and Release Contract](../docs/0.1-support-and-release.md).

Compose enforces the deployment boundary: UID 1000, read-only root filesystem,
private tmpfs, all Linux capabilities dropped, `no-new-privileges`, an init
process, a 30-second stop grace period, SIGTERM shutdown, a restart policy,
read-only Originals, and a `/healthz` check that uses curl rather than Node.

## Isolated restore rehearsal

A backup is not accepted until it starts and passes state checks in isolation. Never extract it over live state.

1. Create empty temporary state and cache directories owned by UID 1000.
2. Extract the verified archive into a separate restore root and use its state directory as `SLIPSTREAM_STATE_DIRECTORY`.
3. Keep the same canonical `SLIPSTREAM_LIBRARY_ROOT`, mounted read-only.
4. Select a non-production loopback port and a distinct Compose project name.
5. Start the exact candidate image with the restored state and empty cache.
6. Run `scripts/verify-production.sh` against the isolated listener with the expected Photo, Photo Set, member, image, Preview, and Original-SHA values.
7. Stop the isolated project cleanly and verify that no journal/WAL/SHM sidecars remain.
8. Delete only the temporary restored state and cache after recording the result. Do not delete the verified backup.

Example:

```sh
set -euo pipefail
restore_root=$(mktemp -d)
cleanup_restore() {
  if ! docker compose --project-name slipstream-restore \
    --env-file /path/to/restore.env -f compose.yaml down; then
    printf 'restore shutdown failed; preserve and inspect %s\n' "$restore_root" >&2
  fi
}
trap cleanup_restore EXIT
tar -xzf "$SLIPSTREAM_BACKUP_OUTPUT" -C "$restore_root"
export SLIPSTREAM_ISOLATED_RESTORE_ROOT="$restore_root"
export SLIPSTREAM_PRODUCTION_STATE_DIRECTORY=/data/slipstream/state
export SLIPSTREAM_PRODUCTION_CACHE_DIRECTORY=/data/slipstream/cache
export SLIPSTREAM_STATE_DIRECTORY="$restore_root/$(basename "$SLIPSTREAM_STATE_DIRECTORY")"
export SLIPSTREAM_CACHE_DIRECTORY="$restore_root/cache"
export SLIPSTREAM_ALLOW_DERIVED_WRITES=1
export SLIPSTREAM_DATABASE_BASENAME=${SLIPSTREAM_DATABASE_BASENAME:-library.sqlite}
export SLIPSTREAM_BIND_ADDRESS=127.0.0.1
export SLIPSTREAM_PORT=17330
install -d -o 1000 -g 1000 -m 0700 "$SLIPSTREAM_CACHE_DIRECTORY"
docker compose --project-name slipstream-restore \
  --env-file /path/to/restore.env -f compose.yaml up -d --no-build
# Set SLIPSTREAM_CONTAINER=slipstream-restore-slipstream-1, the restore URL,
# expected bind/port/schema/state snapshot, and SLIPSTREAM_ALLOW_DERIVED_WRITES=1.
# The isolated acceptance may generate and read the real Preview derivative;
# derived writes remain below restore_root and never touch live state.
./scripts/verify-production.sh
docker compose --project-name slipstream-restore \
  --env-file /path/to/restore.env -f compose.yaml down || {
    printf 'restore shutdown failed; preserve and inspect %s\n' "$restore_root" >&2
    exit 1
  }
for suffix in -journal -wal -shm; do
  if test -e "$SLIPSTREAM_STATE_DIRECTORY/$SLIPSTREAM_DATABASE_BASENAME$suffix"; then
    printf 'restore left a SQLite sidecar; preserve and inspect %s\n' "$restore_root" >&2
    exit 1
  fi
done
# Record successful state, Preview, and sidecar evidence before cleanup.
trap - EXIT
rm -rf -- "$restore_root"
```

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

Retain rollback artifacts according to the
[0.1 Support and Release Contract](../docs/0.1-support-and-release.md). This
repository change does not perform a live deployment.
