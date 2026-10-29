# Deployment

The supported deployment is a Linux amd64 host with the repository-owned Docker image and Compose entry point. The image contains the Rust server, built Web application, native runtime libraries, and `curl` for `/healthz`; it does not contain Node, Bun, npm, Sharp, or Node-API runtime artifacts.

The image runs as UID and GID `1000:1000`. [Container Build Inputs](container-inputs.md) owns reproducible native inputs. [Support and Release](support.md) distinguishes the `0.1` artifact from target capabilities.

## Optional Standard Metadata Save

The default read-only Compose Library supports Read Metadata, not Save. Save requires the exclusive deployment in [Standard Metadata design](../photo-library/metadata/design.md#supported-deployment-shape).

The host supervisor is a trusted root component. The Web service MUST receive only its private Unix socket and a read-only Library Folder bind; it MUST NOT receive the backing tree directly, host root privilege, the systemd bus, or Docker socket.

The writer identity MUST be distinct from Web and reserved for this instance's Samba workers and metadata helper. External applications MUST use only the isolated SMB share `library`.

The host MUST provide:

- systemd with cgroup v2;
- Python 3;
- Samba (`smbd`, `testparm`, and `smbpasswd`);
- util-linux `findmnt`.

The backing filesystem MUST be ext2, ext3, ext4, XFS, or Btrfs. `fs.protected_hardlinks` MUST be `1`.

Samba MUST recognize `smb3 directory leases`; an unknown parameter makes Save unavailable. The supplied profile disables oplocks, kernel oplocks, SMB and directory leases, durable handles, and clustering; status checks effective `testparm` values and rejects unrecognized configuration.

The supplied [supervisor](../../tools/metadata/supervisor.py), [provisioner](../../tools/metadata/provision.py), and systemd [supervisor](../../systemd/slipstream-metadata-supervisor@.service) and [Samba](../../systemd/slipstream-metadata-smb@.service) templates implement this deployment. Installed shared templates and supervisor code MUST be identical across provisioned instances.

The units MUST be installed below `/usr/local/lib/systemd/system` so a runtime mask can override them. Operators MUST NOT install template overrides, higher-priority instance or template units under `/etc/systemd/system`, or replace `KillMode=control-group` and `ExecStopPost` recovery.

The helper MUST be a root-owned executable with root-owned, non-writable ancestors. The Web account MUST already exist and MUST be non-root.

Provisioning copies the source into a new private instance without changing the source or an existing instance. It MUST NOT stop the host's ordinary Samba service.

Provisioning MUST reject symlinks in every source path component before resolution, hardlinks, and special files. It creates these private instance objects:

| Object | Ownership or mode |
| --- | --- |
| Writer account | Dedicated, non-login |
| Directories | Root-owned, sticky/setgid `3770` |
| Originals | Root-owned `0440` |
| Sidecars | Writer-owned |

The private backing parent MUST be exactly root:writer `0710`. Host-side ancestors MUST be traversable without extended attributes.

Inside the tree only Samba's `user.DOSATTRIB` extended attribute is admitted. Other extended attributes and ACLs MUST be removed before Save is admitted.

The server receives `SLIPSTREAM_METADATA_SUPERVISOR` naming the instance socket and a root-created read-only bind of its private Library. The container MUST preserve the configured numeric Web UID/GID, and the socket bind MUST be restricted to that Web deployment.

A non-disposable deployment MUST select a private or Tailscale SMB listen address and restrict its port. Operators MUST NOT grant direct backing access to another account, reuse the writer in another service or container, enable extra shares, or give Web Samba restart authority.

The provisioned copy is the admitted Library. New Originals or directories created through SMB are not automatically admitted.

Before scanning new files, the administrator MUST stop the instance's SMB service and supervisor, inspect the additions, and re-own Originals as root:writer `0440` and directories as root:writer `3770`. Sidecars remain writer-owned with no access for others.

Links, nested filesystems, ACLs, and unadmitted extended attributes MUST be rejected. Status and Save MUST reinspect the tree and refuse until its invariants hold.

The writer MUST NOT rewrite, replace, unlink, or hardlink Originals; Originals remain readable. SQLite state and the backing tree MUST be backed up together under [Backup](#backup).

### Supervisor Protocol and Recovery

The root-owned immutable `/etc/slipstream-metadata/INSTANCE/config.json` contains exactly:

- `backingRoot`;
- `writerUid` and `writerGid`;
- `webUid` and `webGid`;
- `serviceUnit`;
- `helper`;
- `smbConfig`.

Paths are absolute and the service unit is `slipstream-metadata-smb@INSTANCE.service`.

Requests MUST NOT select an executable, unit, or path. Only the configured Web UID passes Unix `SO_PEERCRED` admission; other peers are disconnected without a response.

Each connection accepts one UTF-8 JSON object and newline, at most 2 MiB including the newline. It returns one JSON object and newline, at most 32 MiB; input and output have a five-second timeout.

| Request | Response |
| --- | --- |
| `{"operation":"status"}` | `{"available":true}` or `{"available":false,"reason":"actionable prerequisite failure"}` |
| `{"operation":"save","request":HELPER_REQUEST}` | The helper's single `{"ok":VALUE}` or `{"error":{"code":CODE,"message":MESSAGE,"details":VALUE}}` object. |

Status checks prerequisites, not environment qualification. The helper owns validation of the opaque nested save request.

Supervisor-generated errors use `details:null`:

| Condition | Code |
| --- | --- |
| Malformed request | `invalid_input` |
| Failure before helper invocation | `save_unavailable` |
| Timeout, crash, malformed or oversized helper output, or recovery failure after invocation | `outcome_unknown` |

A lost connection does not prove no change. The caller MUST refresh metadata evidence before another Save.

Save MUST hold an instance-level `flock`, create a recovery marker, runtime-mask the SMB service, verify the mask, and stop the service with a bounded timeout. It MUST prove its retained cgroup-v2 `cgroup.events` reports `populated=0` before helper invocation.

After rechecking identities and the tree, the supervisor MUST launch the fixed helper as writer without supplementary groups. An inherited Unix socketpair lease MUST identify root through peer credentials; the supervisor holds its peer until helper exit.

Helper input and output are bounded and the helper deadline is 30 seconds. Each systemctl operation has a ten-second deadline.

Before releasing the mask and restarting SMB, recovery MUST kill the helper's entire descendant cgroup and observe it empty. Supervisor SIGKILL invokes systemd `ExecStopPost`; ordinary restarts use the same recovery.

Before helper start, the supervisor MUST atomically write a root-owned publication record in its private runtime directory. The record pins the lease-derived temporary name and parent device and inode.

Recovery MAY remove only that exact regular single-link artifact after reproving the fence and parent identity. It MUST NOT glob temporary-looking names.

A missing or invalid record MUST be retained and reported as an actionable refusal. The record is removed before service release; an interrupted record is discarded only from the private runtime directory.

If descendants cannot be stopped, recovery MUST retain the mask and refuse rather than admit concurrent writers. The persistent runtime marker also drives recovery after supervisor restart.

A reachable socket, `available:true`, or active units alone do not qualify the environment for writable use. Lightroom interoperability MUST NOT be claimed without exercising Lightroom.

## Optional Photo Development

Photo Development is optional within the same application image. The image MAY carry the native darktable-mcp runtime, native assets and module/tool metadata, processing manifest, and ICC profile.

A normal start uses one application container; no host processing launcher, processing systemd unit, worker container, processing control socket, or Compose overlay is required. The server verifies bundle identity and every named asset digest, including the manifest's recorded SHA-256, at startup.

A missing or invalid extension leaves Library operations available and reports development unavailable. HTTP requests MUST NOT change startup configuration.

| `SLIPSTREAM_PHOTO_DEVELOPMENT` | Behavior |
| --- | --- |
| `enabled` | Forces the capability on, subject to valid bundle verification. |
| `disabled` | Forces the capability off. |
| `auto` or absent | Enables it exactly when the installed bundle verifies. |

`SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY` MAY select an absolute bundle root for development and smoke runs. Production images carry the bundle at `/opt/slipstream-photo`; the variable is not a production processing endpoint.

`SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES` bounds retained Development TIFF output in bytes. Exhaustion refuses export admission with `resource_unavailable` rather than filling the state volume; Compose defaults to `8589934592` and the environment file MAY override it.

The ordinary Compose container has 8 GiB memory, 4 CPUs, 512 PIDs, and 4 engine OpenMP threads. It mounts no Docker socket.

Photo Workspace directories are durable state outside cache and native scratch. Receipt expiry MUST NOT remove them; operators MUST account for their disk usage, and allowance exhaustion refuses new preparation or output without deleting old work.

[Photo Workspace](../photo-development/workspace/spec.md) owns file lifetime and recovery. Independent tools MUST use the disclosed execution namespace or authorized filesystem access with explicit path mapping; a remote API client MUST NOT assume a server path is local.

Service-supervised development requests serialize, with at most one running and later requests queued. Independent CLI processes obey their own resource bounds and are not serialized by the service queue.

The service and engine share finite container memory and CPU without per-attempt isolation. An engine failure can stop the application; the ordinary restart path recovers. [Processing Memory](../photo-development/processing/memory.md) owns resource details.

The Compose entry point rejects legacy `SLIPSTREAM_PROCESSING_INSTANCE`, `SLIPSTREAM_PROCESSING_SOCKET`, `SLIPSTREAM_PROCESSING_POLICY_SHA256`, and `SLIPSTREAM_PROCESSING_BUNDLE_SHA256` declarations. The retired launcher endpoint no longer exists.

`GET /api/processing/modules` reports independent module availability and refusal reasons after asset verification. Readiness does not admit every input, parameter, or resource combination, and one module's state does not establish another's.

`/healthz` is Library readiness, not processing readiness. A qualification harness profile MUST NOT become product admission merely through discovery.

The [processing tools](../../tools/processing/README.md) own bundle-building and qualification procedures. Standalone SpektraFilm is installed and qualified independently; qualification of its Preview and Export is specific to the tested bundle, parameters, input contract, geometry, and finite memory allowance.

## Configuration

The Web application and API MUST share one HTTP or HTTPS origin. Local use defaults to `http://localhost:3000` without a certificate, domain, or reverse proxy.

Direct HTTP on an explicitly configured interface is supported; browser and CLI warn that network observers can read photos and credentials. HTTPS is RECOMMENDED for public or untrusted networks. [Installed Web Application](../web-application/installation/spec.md) owns secure-context requirements.

`SLIPSTREAM_BIND_ADDRESS` controls host publication and defaults to loopback; setting an origin does not change it. The server listens on the container network and Docker controls host publication.

For direct HTTP on a trusted network, the operator MUST explicitly set the bind address and an HTTP origin reachable from those devices. The configured Web/CLI origin MUST remain reachable from the operator host.

[Instance Access](../access/spec.md) owns token provisioning and access behavior. An absent token MUST NOT enable anonymous access, including on loopback; forwarded headers do not establish identity.

For HTTPS, the operator MUST keep the proxy backend private and disable proxy caching for private responses. Slipstream does not manage certificates or listen with TLS itself.

During an authentication upgrade, the operator MUST keep public ingress closed, back up application state, and provision authentication locally with the server stopped. Private route rejection MUST be verified before ingress opens.

Old publicly cacheable image paths and proxy caches MUST be retired under [Access Design](../access/design.md). Restore requires token rotation before access reopens; rollback to a binary without authentication MUST remain isolated from public traffic.

Private API probes MUST use the credential. `/healthz` remains a minimal readiness check, not authentication evidence.

## Access Administration

When supplied, `SLIPSTREAM_PUBLIC_ORIGIN` MUST be a canonical HTTP or HTTPS origin with no userinfo, query, fragment, or path other than `/`. It contains no secret; a trailing `/` is canonicalized and matching uses parsed scheme, host, and effective port.

When omitted, the origin defaults to `http://localhost:<port>` using the published host port. The environment file selects the origin.

The Compose entry point supports `access-create`, `access-rotate`, and `access-revoke` with the same single environment file, immutable image, and host storage preflight as startup. Each starts a one-shot `slipstream-server` container, not the HTTP service or a Library scan.

Administration uses admitted existing state or initializes only a valid new state directory. It MUST acquire the exclusive process lock shared with server startup and Library Expansion; an active owner fails without touching state.

`access-create` refuses an already configured credential. `access-rotate` requires existing authentication state, including revoked state; `access-revoke` is idempotent for a revoked credential.

Rotation and revocation require a stopped server. Public ingress MUST remain closed until acceptance completes; no command accepts a caller-supplied token.

Creation and rotation require an interactive terminal and present the token once after durable commit. The wrapper MUST NOT capture or log it, use a logging or tee subprocess, or send it to stdout pipelines.

The one-shot container MUST disable Docker logging with the `none` driver and attach the private terminal directly. If delivery fails after commit, it MUST report uncertainty and instruct rotation without rolling back or redisclosing the committed credential.

Revocation prints only confirmation. The underlying `slipstream-server access-create|access-rotate|access-revoke` uses the existing storage environment and exits `0` for confirmed success or `1` for failure.

Access operations MUST NOT modify Originals or clear Library state. [CLI Reference](../command-line/reference.md) owns credential-file selection.

## Storage Rules

For `up`, Library Expansion, and access administration, `SLIPSTREAM_IMAGE` MUST occur exactly once as an unquoted literal immutable image reference. Its repository path uses lowercase components, MAY include a registry port, and ends in `@sha256:` followed by 64 lowercase hexadecimal characters.

Image input MUST NOT use shell expansion, Compose interpolation, or mutable tags. Supported operations run only that image and MUST NOT build from repository sources as a fallback.

For ordinary `up`, Library Expansion, and access administration, each of `SLIPSTREAM_LIBRARY_ROOT`, `SLIPSTREAM_STATE_DIRECTORY`, and `SLIPSTREAM_CACHE_DIRECTORY` MUST occur exactly once as `KEY=/absolute/path` in the environment file. Each directory MUST already exist.

Each storage value MUST be an unquoted valid UTF-8 literal. Internal spaces are allowed.

Storage values MUST NOT contain:

- Leading or trailing whitespace.
- Tabs, carriage returns, or control characters.
- `#`, quotes, backslashes, backticks, or `$`.

Shell expansion and Compose interpolation are not supported for storage values. The same grammar applies after resolving symlink or lexical aliases and to returned mount paths.

Before parsing any line, the entry point MUST reject a raw NUL byte anywhere in the environment file. QA derives its storage paths and rejects declarations of these three keys.

The Library directory MUST be readable by UID 1000; state and cache MUST be writable by UID 1000. The three roles MUST be pairwise disjoint under the [host storage preflight](storage-preflight.md).

Two Slipstream processes MUST NOT use one SQLite database. `GET /healthz` returns `200 {"status":"ok"}` after storage admission, Preview startup, and HTTP bind; `GET /api/status` separately reports Library publication and scan state.

## Compose Entry Point

Supported repository operations MUST use `scripts/compose` with one environment file and the repository's `compose.yaml`. QA additionally uses only the repository-owned `compose.qa.yaml` override.

The host requires Bash, GNU `realpath`, `tr`, and `cmp`, `iconv`, and Linux `findmnt`. The supported command forms are:

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

The entry point MUST reject:

- Alternate Compose files except its own QA override.
- Additional environment files.
- Mount, environment, or entrypoint overrides.
- Unsupported commands.
- Every `COMPOSE_*` and `DOCKER_*` declaration in the environment file.

The ordinary project name is fixed as `slipstream`; [QA](#short-lived-qa-instance) selects its own project. Neither identity is selected by ambient Compose variables or the environment file.

For startup, the environment file takes precedence over these ambient values:

- `SLIPSTREAM_IMAGE`
- `SLIPSTREAM_BIND_ADDRESS`
- `SLIPSTREAM_PORT`
- `SLIPSTREAM_PUBLIC_ORIGIN`
- `SLIPSTREAM_DATABASE_BASENAME`

The entry point clears ambient storage values before exporting the checked canonical sources.

A missing startup origin selects the localhost HTTP default; a malformed one MUST fail rather than fall back to an ambient value. Access administration also clears ambient origin values and its offline state operation does not require a public origin.

[Storage Preflight](storage-preflight.md) owns host topology proof and checked mount delivery. Every container start MUST traverse this preflight; `compose.yaml` MUST NOT declare autonomous restart.

Only a local Docker Engine on Linux is supported. The entry point MUST reject `DOCKER_HOST`, `DOCKER_CONTEXT`, and a Docker default context that is not a local Unix socket; direct `docker compose` invocation is outside the supported contract.

Ordinary `down` skips image and storage-source preflight. It supplies internal stop-only image and storage values that do not select, pull, build, or run an image or access storage paths.

This exception requires a readable, Compose-parseable environment file, but image and storage values MAY be missing or unsafe. The fixed repository Compose file and local Docker context still apply.

## Short-Lived QA Instance

A short-lived fixture-only QA instance MAY run beside the ordinary service on the same trusted local Docker Engine. `--qa ID` MUST appear immediately after the environment file argument and before the action.

`ID` MUST contain exactly 12 lowercase hexadecimal characters; it selects only project `slipstream-qa-ID`. A misplaced, repeated, missing, or invalid ID MUST fail before Docker without falling through to an ordinary action.

The operator MUST create `dist/qa-ID/` in the entry point's checkout with existing `originals`, `state`, and `cache` subdirectories. A fresh ID is required for each qualification run; reuse also reuses its credential, scan state, and cache.

The launcher derives the three canonical paths from its checkout and ID and rejects storage declarations in the QA environment file.

The fixture root and its ancestors below the checkout MUST NOT be symlinks or bind mounts. A mount MUST NOT appear inside the fixture root.

The launcher MUST verify fixture topology before Docker. The operator MUST NOT copy production state or Originals into a QA fixture.

For QA `up` and `up -d`, the environment file MUST satisfy ordinary immutable-image and optional HTTP/HTTPS-origin grammar. It MUST explicitly set `SLIPSTREAM_BIND_ADDRESS=127.0.0.1` and a decimal `SLIPSTREAM_PORT` from 1024 through 65535.

The QA port MUST differ from the ordinary instance's published port; Docker refuses an occupied port at startup. Other QA actions do not publish a port; `access-revoke` does not require an origin.

The fixed `compose.qa.yaml` adds no services, mounts, or restart policies. It limits both `slipstream` and `slipstream-admin` to 2 GiB memory, 2 CPUs, and 256 PIDs without changing ordinary configuration.

QA supports the ordinary bounded entry-point actions. QA `down` requires a valid ID, readable environment file, and rejection of `COMPOSE_*` and `DOCKER_*` declarations.

QA `down` uses stop-only sentinels and fixed QA Compose files without checking unavailable fixture paths, image, or origin. It targets only the named QA project and never removes host directories.

QA access creation and rotation retain interactive, no-log token delivery. QA MAY use direct loopback HTTP or a private HTTPS proxy with a trusted client; shared daemon and host capacity do not isolate an untrusted workload.

## Backup

Image cutover, expansion, and state recovery require one consistent recovery point covering:

- Library and Access databases;
- workspace identity and provenance records;
- protected input snapshots and canonical tool state;
- required resources and kept outputs.

Resources outside the workspace MUST be included or retained immutably with verified recovery access. Retained Export artifacts cannot be reconstructed from SQLite; a backup missing required state or bytes MUST NOT be accepted.

A file-copy backup requires the service stopped and every writer of captured directories and dependencies quiesced. This includes independent CLI processes and shared-filesystem writers; new writes MUST be prevented until the complete copy finishes.

Stopping Slipstream or observing an idle service queue alone is insufficient. If exclusion cannot be established, backup MUST be refused with the unresolved writer or storage scope and a recovery action; operators MUST NOT kill a render or discard saved work merely to obtain a backup.

A proven consistent filesystem snapshot covering the same recovery point is an alternative. Separate live copies or snapshots of different volumes, SQLite's backup API alone, and per-file atomic renames do not establish cross-file consistency.

Before a backup is accepted, the restored service MUST start in an isolated copy with the same canonical read-only Library Folder and pass the deployment's state checks. Both databases MUST open together without losing the credential.

Recovery verification MUST enumerate saved edit revisions and check their input and resource availability, retained output bytes, and engine identities. A dependency available at capture MUST remain available after restore.

Where an executable saved edit exists, verification MUST reopen one with its masks and resources and inspect a kept output without reading the live Library Original or substituting defaults. Pre-existing missing dependencies or incompatible historical state MUST retain their exact intent and disclosed limitations.

Preserving pre-existing limitations does not establish executable recovery. Newly missing bytes, altered revisions, or silent default substitution MUST fail recovery verification with the affected identities.

Restore MUST NOT overwrite live state. [Support and Release](support.md#rollback-artifact-retention) owns rollback-artifact retirement.

Restored retained uploads and downloads MUST have their saved SHA-256 and byte length verified before replacing host-local inode or time observations. Different restored bytes MUST be refused.

## Library Expansion

[Library Identity](../photo-library/identity.md) owns the ancestor-only expansion and transaction. Expansion requires all processes using the state database stopped, existing sidecars preserved, and a verified current-schema backup.

The operator MUST retain the state directory and database basename and configure the proposed canonical ancestor as `SLIPSTREAM_LIBRARY_ROOT`. The proposed Folder MUST be mounted read-only at the same absolute path inside the container.

The offline command rejects a running database, sidecars, unsupported schema, unrelated Folder, descriptor mismatch, invalid remembered Locations, and scan-limit failures. It commits the binding and Location changes in one admitted transaction, then completes a normal scan before reporting success.

The command never binds HTTP and uses the same storage configuration regardless of listener settings. A preflight or transaction failure leaves the prior Folder binding unchanged.

After a post-commit scan failure, the service MUST NOT be exposed as ready. Correct the failure and retry startup, or restore the verified pre-expansion backup and prior Folder.

## Cutover and Verification

Image cutover requires the target image digest and verified state backup. Operator-provided acceptance MUST verify readiness, Library Overview, one bounded File Location read, one Album browse read, a state mutation and undo, and a derivative read.

Cutover acceptance MUST compare schema and database binding with the pre-cutover record and confirm Original File hashes are unchanged.

Exact persisted state verification uses a bounded protocol traversal or offline read-only state projection against the pre-cutover record. No HTTP route materializes the complete Library, Folder tree, recursive Folder membership, or Album membership.
