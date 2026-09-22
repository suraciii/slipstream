# Processing Executor

Native processing requires an enforceable memory boundary that survives worker
failure. The Web container must not control Docker or host cgroups, and a worker
must not be able to raise its own limits. A process-local wrapper cannot retain
terminal evidence after its container runtime removes the worker cgroup.

[Processing Memory](processing-memory.md) owns the memory policy and engine
workspace model. [Photo Development Architecture](photo-development.md) owns
source access, Export lifecycle, and publication. This specification owns the
private execution boundary. The
[qualification protocol](processing-executor-protocol.md) defines its initial
closed fixture interface and operator configuration; it does not enable RAW
or Film workloads.

## Ownership

The Rust control service owns Photo resolution, Edit Recipes, queue admission,
Export state, source guards, and validated artifact publication. The initial
scheduler still runs one heavy attempt per instance.

An operator-owned Rust launcher starts a fresh, pinned processing container for
each attempt. It runs outside the complete processing resource subtree. It owns
only restricted execution and operational receipts, not a second Library, queue
of Exports, or public processing API. Engine processes remain fresh per attempt.

The operator configures an instance identity, canonical private workspace root,
fixed digest-addressed processing image, finite memory policy, and a private
Unix-domain control socket. The Web service receives access to this socket, not
Docker credentials or a writable host cgroup hierarchy. The launcher exposes no
public HTTP endpoint. The launcher is a privileged component in the operator
trust boundary: access to the local Docker daemon and systemd carries host
authority. The private protocol restricts what the Web can request; it does not
make the Docker socket itself a restricted capability.

## Restricted Launch Authority

The launcher accepts only a versioned, bounded protocol with fixed operations:
start an attempt, inspect an attempt, cancel an attempt, and reconcile owned
attempts. A request identifies a validated attempt and an admitted workload kind.
It cannot provide a shell expression, executable, arbitrary argv, image, mount,
network, controller path, or host file path. The launcher derives all runtime
arguments from its fixed configuration and the qualified bundle.

Input/output paths are derived beneath a launcher-owned instance root from
validated opaque identifiers. The root and every mount-source ancestor must be
stable and non-writable by the Web, engine, or other untrusted identities. Reject
traversal, links, foreign ownership, and stale incarnation references. A one-time
path validation followed by a bind of a mutable Web directory is insufficient.

Admission copies the input and manifest into a launcher-owned immutable snapshot
and verifies the captured input size and digest before engine release. Use
confined descriptors to acquire supplied bytes; never re-resolve an untrusted
path for a privileged bind mount. The sealing operation must leave no writable
alias or open writer owned by the sender. An untrusted hard link is insufficient.
Inputs are read-only in the worker; outputs have a separate private writable
mount. Engines receive neither an Original Library mount nor the launcher socket
or controller-write access. The control service still owns validated Original
resolution and source stability checks.

The processing allocation includes bounded staging helpers and charged file
storage. All writable worker locations require an enforced aggregate byte and
inode bound, not only a pathname check or per-file size limit. A launcher-owned
size- and inode-capped tmpfs is the initial storage mechanism. Its page charges
belong in the retained attempt budget; its producer must run inside that boundary.
It stays mounted through terminal evidence and artifact handoff. A bounded reader
may stream completed artifacts to the control service, whose output retention
remains independently bounded. Do not release the attempt slot until temporary
storage has been reclaimed or charged to an explicit retained-output allowance.

Worker containers use Docker's `none` log driver; they must not inherit an
unbounded daemon log default. Fixed-size result metadata lives in the private
workspace and is read with a byte bound before cleanup. Native stdout/stderr
cannot allocate retained host log files. The launcher bounds manager-command
stdout/stderr, concurrent IPC connections, diagnostics, and operational receipts;
a noisy worker or repeated malformed request must not grow a second unbounded
log or in-memory queue.

Authenticate local callers using configured socket ownership and peer identity.
A bounded transport or authentication failure must never become permission to
start unrestricted work. Compatibility, request size, unknown fields, numeric
bounds and supported workload kinds have an executable validator before any
allocation or subprocess start.

## Identity and Start Ordering

Each request binds the instance, unique attempt identity, captured policy,
processing bundle and immutable workload manifest. The launcher compares the
request with its configured authority before launch. A resource limit change
requires draining or cancelling active work; new starts must use the current
policy. The service cannot raise the operator's allocation through a request.

A launcher holds an exclusive, crash-released instance lock before it can serve
requests or reconcile runtime objects. Its journal stores a persistent registry
incarnation, a monotonic admission sequence, the active slot, and attempt
receipts. Only one launcher incarnation may change instance execution state.

Persist and synchronize the launch intent and reserve the active slot atomically
before any systemd or Docker creation. The intent binds the request identity,
manifest digest, captured policy, registry incarnation, and unpredictable launch
identity. Persist the exact manager-created unit/container identities before
releasing the bootstrap. A crash at any step leaves a recoverable intent rather
than an unowned worker. Runtime ownership requires the journal binding together
with exact manager identity and launch metadata; names or labels alone do not
justify killing a process. Ambiguous creation outcomes block further admission
until reconciliation establishes ownership and settlement.

Start is idempotent for the same admitted identity and exact manifest; a duplicate
returns its receipt, and conflicting reuse fails. A lost response is resolved by
inspection. A replay below the admission watermark whose receipt has expired
returns an explicit expired result. It cannot launch again. A missing or reset
registry uses a new incarnation and rejects earlier identities; it must reconcile
or quarantine remaining runtime objects before admission. Intentional retry
requires a new identity. The bounded receipt period is operator configuration;
expiry never removes active ownership, cancellation intent, or unsettled evidence.

Create and verify the finite aggregate processing boundary and a fresh retained
attempt accounting boundary before any worker imports or decodes image data.
The latter accounts the entire container, including startup/bootstrap overhead,
and remains available after container exit until evidence settlement.

Create the container with a fixed image and read-only filesystem, unprivileged
engine identity, no network/GPU/display, dropped capabilities, no-new-privileges,
bounded writable storage and qualified CPU/PID limits. A bounded blocked
bootstrap permits host-side placement/readback before engine exec; bootstrapping
itself must already be under the memory allocation. Release execution only after
all descendant containment and controller-write isolation checks succeed.

The supported Linux backend uses systemd with Docker's systemd cgroup driver:

```text diagram
processing slice (finite aggregate allocation)
  retained attempt slice (finite full-container allocation)
    Docker delegated scope
      workload leaf (group OOM; engine and all descendants)
```

Systemd owns slice properties through its unit APIs. Each explicitly started
attempt slice has `StopWhenUnneeded=no`; an ordinary exited scope does not retain
accounting. Docker owns its scope. The launcher creates and controls only the
workload leaf within that delegated scope. It moves the blocked bootstrap into
the leaf, enables the required controllers, applies and reads back limits, and
checks membership before release. Future engine descendants inherit the leaf.
The finite enclosing attempt slice already accounts for the bootstrap and
charges that do not migrate with its PID. Both launcher and Web remain outside
the entire processing slice.

The worker has no writable cgroup mount. The leaf sets `memory.oom.group=1`;
the retained attempt and aggregate slices enforce the captured memory ceiling
and zero swap. CPU/task limits cover bootstrap and descendants as well. No
controller write may race systemd or Docker over a manager-owned ancestor.

## Evidence and Settlement

Observe the retained attempt boundary for memory peak and terminal events.
Docker may remove its scope and invalidate open cgroup descriptors immediately
after exit, so neither polling nor pre-opening those descriptors is sufficient.
The retained slice supplies the authoritative whole-attempt peak and hierarchical
events. Capture `memory.events.local` as well as hierarchical events at the
attempt and aggregate boundaries to locate pressure; leaf-local evidence may
vanish. Distinguish ancestor and host pressure and correlate event deltas with
the exact owned runtime identity. Retain terminal container
state plus kernel evidence before stopping the accounting boundary.

A launcher receipt records requested/captured policy, bundle and manifest
identity, owned runtime identities, actual limits, start/exit facts, resource
evidence, and settlement outcome. It does not mark an Export successful. The
control service validates the artifact against its guarded snapshot and remains
the sole publisher under the existing lifecycle contract.

On cancellation, deadline, OOM, connection loss or engine failure, the launcher
must retain execution ownership. Loss of the control connection alone does not
cancel accepted work. Persist cancellation as a monotonic intent before
termination; a duplicate start or restart must never release a cancelled
bootstrap. Terminate the complete owned workload when required and
confirm it is empty before releasing the instance slot. Preserve uncertain
settlement as an explicit blocked state; do not launch a replacement alongside
possible orphaned work.

The operational registry must survive launcher restart and reconcile receipts,
owned containers, and retained accounting boundaries before accepting new work.
Reconcile each partial phase: reserved intent, created slice, sealed inputs,
created container, bound runtime identity, released engine, exit, persisted
evidence, and partial cleanup. Deadlines use persisted absolute expiry so restart
cannot grant an attempt a fresh execution interval.
Never identify ownership solely by a PID, a mutable name, or an unvalidated label.
Unknown or ambiguous ownership requires an actionable quarantine result, not
termination of unrelated work. Surviving workers cannot publish by themselves.

Retain terminal evidence for the defined receipt period. Remove containers,
accounting boundaries and private temporary artifacts only after confirmed
settlement and evidence persistence; active valid artifacts follow the service's
leases and retention. Repeat cleanup must be safe after partial failure.

## Capability and Failure

Startup and operator verification must exercise the actual namespace, UID,
controller placement, ancestor limits and private transport used by processing.
A missing launcher, incompatible protocol, unavailable controller or invalid
allocation disables processing while normal browsing and saved edits remain
available. Availability is not proven by an open socket or a healthy container.

Failures distinguish invalid authority/request, incompatible bundle/policy,
already-owned conflicting attempt, capacity wait/rejection, runtime allocation
failure, confirmed OOM, deadline/cancellation, engine exit, and uncertain cleanup.
The semantic outcomes in Processing Memory and the shared Web/CLI error contract
remain authoritative. An exception, exit 137 or a lost connection alone must not
be reported as confirmed OOM or successful cancellation.

## Options

A host launcher with fresh attempt containers is selected because it leaves
control outside the failure boundary, makes runtime identities explicit and
preserves kernel enforcement during supervisor restart. Its private protocol is
limited to the fixed processing capability.

A writable cgroup subtree inside the Web container would additionally require
safe delegation, migration authority and separate controller rights for
same-UID engines. It is not the supported initial deployment mechanism.

A persistent processing helper inside the capped container shares the workload's
OOM boundary and needs external reconciliation anyway. A persistent Python
simulator also changes state/reset semantics. Neither is required for this
increment.

Giving the Web service unrestricted Docker access or privileged host mounts is
rejected because it expands its authority beyond the fixed processing task.

## Verification

Prove native and descendant OOM at both leaf and aggregate-parent limits, worker
write/escape denial, normal exit versus OOM classification, retained final peaks,
control-service survival and a following successful job. Exercise failed start
at each ordering boundary, idempotent start, lost responses, cancel/complete
races, launcher/control restart, foreign ownership, policy changes and cleanup
failure. Validate private protocol bounds and every derived runtime argument.

Prove that final accounting survives both the last worker exit and launcher
restart, and that controlled storage exhaustion and stdout/stderr flooding cannot fill the host filesystem
or remove retained evidence.

Run the exact packaged operator path with the supported Web image and processing
image before claiming deployment acceptance. Synthetic kernel tests, engine
image-quality checks and ordinary repository gates remain separate evidence.
