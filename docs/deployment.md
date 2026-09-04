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
SLIPSTREAM_PUBLIC_ORIGIN=http://127.0.0.1:3000
```

`SLIPSTREAM_BIND_ADDRESS` controls the host-side published interface and
defaults to loopback. Binding topology is the operator's choice; Slipstream
0.1 ships no accounts or authorization, so protecting an exposed listener is
the operator's responsibility. The Rust process listens on the container
network; Docker's host-side publication is the exposure boundary.

`SLIPSTREAM_PUBLIC_ORIGIN` is required. It is the exact `http` or `https`
origin at which the Photographer opens Slipstream, including a non-default
port. It has no userinfo, path, query, or fragment. A malformed value prevents
the online server from starting. It is separate from the listener and the
host-side bind address: a TLS-terminating proxy or Tailscale address must set
the browser-visible `https` or `http` origin explicitly.

Only an absent port uses the scheme default. An explicit nonnumeric, signed,
or out-of-range port is malformed and does not fall back to that default.

Except for exact `GET /healthz` and `HEAD /healthz`, Slipstream accepts a
normal origin-form request only when its Host authority matches
`SLIPSTREAM_PUBLIC_ORIGIN`. An absolute request target must use the configured
scheme and authority, and any Host it supplies must match too. A state-changing
browser request must also send that exact origin in `Origin`. A missing
origin-form Host, malformed Host or absolute request target, or malformed
Origin on a state-changing request fails with `400` before static files,
Library reads, request-body parsing, or state writes. A well-formed authority
outside the configured origin, or a missing or different Origin on a
state-changing request, fails with `403`. Authority comparison canonicalizes
ASCII host case and default `http:80` and `https:443` ports. Forwarded request
headers never establish trust. The health endpoint is the only authority
exception so a container-local probe can use `127.0.0.1`; it returns only the
fixed readiness response and accepts no state change. After the header-size
limit and health exception, authority admission precedes method and route
policy: an untrusted authority receives `403` even when that method or route
would otherwise receive `405` or `404`.

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
2. Create and record a verified canonical schema-v5 backup (see Backup).
3. Change `SLIPSTREAM_LIBRARY_ROOT` to the proposed canonical ancestor. Keep
   the state directory and database basename unchanged. Ensure the proposed
   Folder is mounted read-only at the same absolute path inside the container.
4. Run the candidate image once without starting the service:

   ```sh
   docker compose --env-file /path/to/expanded-library.env -f compose.yaml \
     run --rm --no-deps slipstream expand-library
   ```

The offline command rejects a running database, sidecars, non-v5 state, an
unrelated Folder, descriptor mismatch, invalid remembered Locations, and
scan-limit failures. It commits the binding and Location changes in one
admitted transaction, then completes a normal scan before reporting success.
It never binds HTTP and does not require `SLIPSTREAM_PUBLIC_ORIGIN`; Compose
permits an empty value so this offline command can run, while an online server
still rejects it during startup.

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
