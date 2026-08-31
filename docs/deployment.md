# Deployment

Slipstream ships as one Docker image containing the Rust server, the built Web
application, native runtime libraries, and `curl` for the `/healthz` check.
There is no Node, Bun, Sharp, or Node-API runtime in the image. This guide is
host-agnostic: it describes the product's deployment contract. Backup,
acceptance, and rollback step-by-step procedures are operator material and live
with the deployment, not in this repository.

## Configuration

Create an environment file outside the repository (or export these variables in
the shell that runs Compose):

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
defaults to loopback. Binding topology is the operator's choice; Slipstream
0.1 ships no accounts or authorization, so protecting an exposed listener is
the operator's responsibility. The Rust process listens on the container
network; Docker's host-side publication is the exposure boundary.

Storage rules:

- The Originals directory must already exist, be the canonical absolute path
  (not a symlink or lexical alias), and be readable by UID 1000; Compose mounts
  it read-only at the same path inside the container.
- State and cache directories must exist and be writable by UID 1000.
- Never place state or cache inside the Library Folder.
- Never run two Slipstream processes against one SQLite database.

`GET /healthz` returns `200 {"status":"ok"}` after storage admission, Preview
startup, and HTTP bind. `GET /api/status` separately reports Library
publication and scan state.

## Image

Build from a clean checkout of the exact commit:

```sh
commit=$(git rev-parse HEAD)
docker build --build-arg "SLIPSTREAM_VCS_REF=$commit" --tag slipstream:local .
```

Record the image digest. Verify the image user is `1000:1000` and that no
Node, Bun, npm, Sharp, or Node-API runtime artifact is present before an
operator-controlled deployment. Operators may script these checks; any
equivalent inspection is acceptable.

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
2. Create and record a verified canonical schema-v4 backup (see Backup).
3. Change `SLIPSTREAM_LIBRARY_ROOT` to the proposed canonical ancestor. Keep
   the state directory and database basename unchanged. Ensure the proposed
   Folder is mounted read-only at the same absolute path inside the container.
4. Run the candidate image once without starting the service:

   ```sh
   docker compose --env-file /path/to/expanded-library.env -f compose.yaml \
     run --rm --no-deps slipstream expand-library
   ```

The offline command rejects a running database, sidecars, non-v4 state, an
unrelated Folder, descriptor mismatch, invalid remembered Locations, and
scan-limit failures. It commits the binding and Location changes in one
admitted transaction, then completes a normal scan before reporting success.

If preflight or the transaction fails, the prior Folder binding is unchanged.
If the post-commit scan fails, do not expose the service as ready: correct the
failure and retry startup, or restore the verified pre-expansion backup and
prior Folder.

## Cutover and verification

1. Confirm the target image digest and the verified state backup.
2. Start the tagged image with the prepared environment:

   ```sh
   docker compose --env-file /path/to/slipstream.env -f compose.yaml up -d
   ```

3. Verify `GET /healthz`, the Library Overview, a Photo Set browse read, one
   state mutation, undo, and a derivative read. Compare the SQLite schema and
   database binding with the pre-cutover record.
4. Confirm Original File hashes are unchanged.

Operators verify exact persisted state by comparing a bounded protocol
traversal or an offline read-only state projection against the pre-cutover
record; no HTTP route materializes the complete Library or complete Photo Set
membership. Acceptance tooling is operator-provided.
