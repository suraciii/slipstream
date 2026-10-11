# Local Photo Executor

A real Photo Workspace can be edited by a standalone tool while Slipstream is
absent. Slipstream also needs bounded Web execution. One embedded parameter
engine cannot serve both lifetimes honestly. The selected executor supervises an
independent file/state executable; that tool owns its private native children.

[Photo Development Architecture](../design.md) owns captured state,
run receipts and attribution; [Photo Workspace](../workspace/design.md) owns files
and ready indexing. [Processing Memory](memory.md) owns finite
resource policy; [Native darktable Integration](../darktable/design.md) owns
native preparation. This document owns process lifetime and supervision.

## Model

The Web service owns one bounded heavy-work admission queue and a supervisor
for its permitted tool executables. Initially at most one supervised attempt
runs; previews and exports share bounded fair admission. The lock covers that
supervisor's work only. Independent commands are not server tasks and must not
be presented as serialized by it. Per-edit tool locks prevent conflicting state
writes without locking the whole workspace.

Each standalone command owns its input/state capture, finite resource policy,
private native process group, scratch and synchronous terminal result. Web starts
the exact same tool against authorized workspace paths and retains a captured
run receipt. No independent daemon, host launcher, generic plugin registration,
arbitrary-shell API or per-request container is introduced. The executable's
own pinned bundle is usable without Slipstream.

Optional tool bundles identify engine/runtime, semantic schemas, profiles and
native assets. Startup/discovery verifies manifest identity and digests per tool.
Missing bundles leave Library and completed workspace files usable. Discovery
and bundle presence do not qualify arbitrary controls or source/output classes.
Service startup policy controls permitted executables and resource capacity;
HTTP cannot select arbitrary binaries or raise execution limits.

## Execution

The Web supervisor resolves confined file/state paths and invokes an argument
vector with a unique run identity and bounded structured output. No shell
interpretation occurs. A remote caller receives the true execution host/path
facts and must use an environment with access; no implicit remote mount or sync
is supplied. Standalone callers use accessible local paths without HTTP/Photo IDs.

The tool captures canonical revision and input content evidence under its local
state rules. Existing protected input bytes are reused, not staged every render.
Mutable external input is first admitted as one protected snapshot under the
workspace contract. Native children receive read-only inputs/resources and
fresh private config/catalog/cache. Original/XMP, desktop presets and service
database are outside native writes. Ambient sidecar/crawler/display behavior is
disabled. Private protocol values, diagnostics and result sizes are bounded;
malformed, truncated, oversized or incompatible responses fail explicitly.

Finite deadlines cover tool/native computation and blocked exchanges. Thread
limits cover darktable, NumPy/BLAS, Numba and FFTW. Cancel/deadline terminates all
owned descendants, reaps children and settles scratch before releasing the Web
processing slot. The tool itself supervises its native group; Web additionally
owns the tool group. Unexpected parent exit must terminate that owner's native
children through a qualified parent-death/process-group mechanism. A surviving
standalone tool is not a child of the service and is unaffected by service exit.
Unconfirmed termination stops further affected heavy admission; it is not a
successful cancellation or permission to reuse scratch.

The tool validates actual output and writes private temp+close+atomic no-replace
final publication. A cancelled/failed run leaves prior complete files and saved
state intact. The index can discover a completed regular file without any
callback/registration. A Web receipt succeeds only from matching validated
invocation/file evidence; process exit zero, a filename or a new watcher event
alone is insufficient. Late old-revision Preview files cannot replace the current
canvas. Native failure never substitutes another tool or a camera result.

For retained workspace-backed adapters, a successful Export status read must
confirm the completed output's aliased workspace revision and exact retained
byte evidence through the workspace's pinned download validation and lease.
The lease must remain held until the temporary execution input dependency is
released, before returning `succeeded`. Failed or cancelled status reads release
that dependency after durable settlement. Output-owned dependencies remain
retained. A registry alias alone does not establish current output availability.
Changed, truncated, missing or removed aliased outputs produce an unconfirmed
result and preserve any remaining input pin; reconciliation must not recreate
them from historical private bytes. The historical work record may remain
`succeeded`, but a new status response must not claim verified availability
without current evidence. Releasing the pin before settlement would permit
removal while the attempt still needs its input.

## Restart

The service reconciles its own durable queued/running receipts. Queued work
rechecks captured revision, input, resources, bundle and current limits before
dispatch. Interrupted running work fails explicitly unless exact validated
completed-file and invocation evidence proves success. Do not blindly rerun an
expensive uncertain command or attach to an unidentified prior process.

Tool-local state/receipts reconcile interrupted mutations independently. Atomic
state installation keeps the prior or complete new revision; private partial
outputs are not results. Abandoned scratch is removed only when ownership and
absence of live users are established. The service cannot sweep a workspace's
entire `.tmp/` because an external CLI may still be working there.

Service restart neither cancels nor adopts standalone work. A CLI may complete
while the service is offline; read/list reconciliation later indexes its files.
Absent verified local invocation evidence they remain ordinary ready files,
not invented successful server Exports. Workspace input/current state/kept output
retention is independent of transient job-receipt expiration.

## Options

Selected: one optional Web supervisor invoking an independent tool with fresh
private native children. The existing application container can bundle both
service and tool; direct CLI also works in another qualified finite environment.
This keeps one editing implementation and clear ownership without a second daemon.

Rejected: embedded service parameter/executor authority. It prevents independent
editing or requires a second writable state plus import/submit synchronization.

Rejected: host launcher and per-attempt containers. Privileged Docker/cgroup
management and an additional durable journal do not serve this personal product.
Finite execution remains required through the qualified environment.

Rejected: shared persistent native daemon/catalog. Reuse adds cross-request state
and reset proofs; fresh native children provide explicit lifetime isolation.

## Verification

Future evidence must exercise the same executable standalone without Slipstream
and supervised against the same state, including Export before Preview. Cover
finite admission, blocked protocol, cancellation of descendants, deadline,
engine failure, no-overwrite completion, stale results, crash/restart and scratch
ownership with concurrent external work. Confirm original/external-XMP invariance,
real RAW/color and full-size handoff separately from synthetic failure checks.
