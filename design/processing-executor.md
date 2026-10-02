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

Optional bundled extensions contain each module's pinned executable or runtime,
native assets, parameter metadata, manifest, and output profiles. Startup verifies
each manifest identity and its named asset digests independently. Missing or
invalid assets leave the Library and other available modules usable. Startup
configuration cannot be changed by an HTTP request. Engine discovery does not
admit new controls, source classes, or output contracts.

## Execution

Each request uses a fresh selected-engine child supervised in the application's
private process group, private engine state, and unique temporary output identity.
darktable uses a private catalog/config/cache. Engines receive only confined,
copied and hashed inputs, never the Library directory, state database, desktop
catalog, or external XMP. Ambient presets, sidecars, and crawler initialization
are disabled. Each module admits only its qualified parameter combinations under
[Processing Modules](processing-modules.md).

The processing lock remains held through engine termination and scratch cleanup.
Private stdio carries bounded MCP JSON-RPC values; malformed, oversized,
truncated, or incompatible responses fail the operation. A deadline bounds all
engine work, including a blocked protocol exchange. Cancellation and deadline
kill the entire engine process group, reap the supervisor child, and remove
scratch before releasing the lock. The supervisor receives the kernel parent-death
signal and kills the group if the application exits unexpectedly. Normal shutdown
cancels active processing before closing the Library.

Results are validated against the selected module's concrete output contract
before publication. Preview checks current intent and input identity before
serving. Export retains its captured intent; a changed input or superseded attempt
cannot publish. Engine failure never substitutes a camera Preview, another
module, or an uncorrected result.

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
