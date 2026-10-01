# Local Photo Executor

Slipstream is a personal, single-machine application. A host processing service and
per-request containers add a second operational lifecycle without serving this
product. The application owns development inside its one container.

[Photo Development Architecture](photo-development.md) owns source guards and
publication. [Development Color Pipeline](development-color.md) owns output
validation. [Native darktable Integration](darktable-integration.md) owns semantic
engine requests. This document owns local execution and failure lifetime.

## Model

The server owns one PhotoExecutor, one shared processing admission lock, and an
application-owned scratch directory outside the Library. Preview, Development
Proxy builds, and Export share this resource. At most one engine request runs.
Queued requests recheck cancellation and captured source facts after admission.

The optional bundled extension contains the pinned darktable-mcp executable,
native assets, module/tool metadata, manifest, and ICC profile. Startup verifies
manifest identity and all named asset digests. Missing or invalid assets leave the
Library usable and Development unavailable. Startup configuration cannot be
changed by an HTTP request. Film remains unavailable; engine discovery does not
admit new controls or source classes.

## Execution

Each request uses a fresh darktable child supervised in the application process's
private process group, a private catalog/config/cache, and unique temporary output
identity. The engine receives only a copied and hashed Original, never the Library
directory, state database, desktop catalog, or external XMP. Ambient presets,
sidecars, and crawler initialization are disabled.
The closed semantic intent remains exposure 0 through +1 EV and as-shot white
balance for qualified source profiles.

The processing lock remains held through engine termination and scratch cleanup.
Private stdio carries bounded MCP JSON-RPC values; malformed, oversized,
truncated, or incompatible responses fail the operation. A deadline bounds all
engine work, including a blocked protocol exchange. Cancellation and deadline
kill the entire engine process group, reap the supervisor child, and remove
scratch before releasing the lock. The supervisor receives the kernel parent-death
signal and kills the group if the application exits unexpectedly. Normal shutdown
cancels active processing before closing the Library.

Results are validated for bytes, geometry, scene-linear float32 samples, ICC
identity, and orientation before publication. Preview checks current intent and
source identity before serving. Export retains its captured intent; a changed
source or superseded attempt cannot publish. Engine failure never substitutes a
camera Preview or an uncorrected result.

## Restart

The application store owns Export settlement, not an execution journal. Startup
recovers a valid artifact tied to a durable publication claim; unfinished running
work without such an artifact fails as interrupted. It does not attach to or
replace an old engine process. Queued work may run through ordinary admission.
Startup removes abandoned scratch under its exclusive application-owned lock.
There are no launcher receipts, sockets, host processing services, worker
containers, or per-attempt cgroups to reconcile.

## Options

Selected: one container with serialized local children. The container's finite
memory/CPU limits are shared by Web and engine; an engine OOM can stop the whole
application. This trade-off fits the personal local deployment, not a public or
multi-user service.

Rejected: retain the host launcher and per-attempt isolation. Its privileged
service, Docker authority, cgroup management, and journal are unnecessary for the
local product. It must not remain a second production path.

Rejected: shared persistent darktable daemon. A fresh child has a direct state
lifetime proof; reuse requires separate evidence that image, module, and history
state cannot leak between requests.

## Verification

Exercise real qualified RAW development through Preview and Export, including an
Export before Preview. Check the complete Development TIFF and downstream handoff
contracts and unchanged Original and external-XMP bytes and metadata. Exercise
serialization, cancellation, deadline, malformed MCP, engine failure, stale
results, restart, scratch cleanup, and the next successful request. Verify one
application container and no host processing dependency. Synthetic checks prove
failure policy, not RAW or Film qualification.
