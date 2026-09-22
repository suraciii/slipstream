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
or Film workloads. [Film Resource Measurement](processing-film-measurement.md)
defines a separate administrator-only image measurement profile under this same
execution boundary; it does not enable ordinary Film admission.

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

## Launcher Lifecycle

Install one root-owned systemd service instance for each configured processing
instance. Its executable, host-only configuration and persistent journal are
outside the Library, state, cache and complete processing subtree. The service
runs outside that subtree with only the host authority needed to manage its
fixed processing boundary. It needs a writable host cgroup v2 view to configure
the bounded attempt cgroups and place the worker process in its workload leaf;
`ProtectControlGroups=yes` would make those writes fail and must remain disabled.
The Web process and Compose wrapper cannot start, stop, reconfigure or signal the
launcher through an additional control path, and the Web container receives no
cgroup mount.

The configuration binds the instance and journal root to the socket path and
peer identity, immutable launcher and worker identities, and an operator policy
whose exact finite limits and shared-ancestor allowance have already passed the
qualified admission contract. It is root-owned and is not passed through Web
environment variables or mounted into a container. Policy and image identities
are fixed for an admitted attempt; a request cannot override them.

At service start, acquire the instance and journal locks, validate the complete
host and manager boundary, and reconcile every recorded attempt before allowing
new admissions. The socket may be bound during reconciliation, but every new
Start request remains unavailable until recovery succeeds. A reachable socket is not
readiness. A restart never clears a journal, instance claim, blocked receipt or
unknown runtime. If a prior attempt or manager operation is ambiguous, leave
admission unavailable and require operator reconciliation. A supervisor restart
policy may restart the executable, but it cannot bypass this ordering or turn a
failed reconciliation into readiness.

The socket parent and every ancestor are canonical, symlink-free, root-owned
and not writable by the Web UID. The parent contains only the fixed socket and
its root-only persistent owner-claim file. Give the Web UID search permission
on the parent and read/write permission on the socket through a named ACL,
without directory listing or claim-file access; retain peer-credential checks
on each connection. Mount the whole parent read-only into the Web container so
a launcher restart can replace the socket pathname without leaving the Web
bound to a stale inode. Do not bind-mount one socket inode or place the launcher
configuration, journal or manager controls in that directory.

Do not apply a resource policy change to a running instance. Stop the
processing-enabled Web deployment to prevent new admission, leave the launcher
running until all accepted attempts have settled and cleanup is confirmed, then
stop the launcher service before atomically installing the changed configuration.
Restart it and reconcile the existing journal under the new policy before
allowing admission or starting the processing-enabled Web deployment. A
missing or mismatched journal, manager identity, image identity or policy leaves
processing unavailable; operators must not clear the fence by replacing state
or silently adopting runtime objects. The configuration change path and
recovery evidence belong to the operator procedure, not a Web or launcher IPC
operation.

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

Before manager effects, a launcher holds both its journal-root lock and an
exclusive nonblocking host-global flock on the fixed root-owned file
`/var/lib/slipstream-processing/instances/<instance>.claim`. This bounded claim
stores the immutable canonical journal-root binding; synchronize the file and
parent directory before manager effects. Validate the stable root-owned namespace
and single-link regular file without following links. Hold the same descriptor
for the launcher lifetime and never unlink it on ordinary exit. A different root
for that instance, a torn claim, or an existing claim with a missing operational
registry requires quarantine even if runtime inventory is empty. A delayed old
manager request can still complete. Never replace an existing claim or create a
fresh journal to clear this fence. A crash between first claim synchronization
and journal creation may conservatively require operator reconciliation and a
fresh instance. This is a host-wide ownership claim, not a second attempt journal.

A launcher holds these exclusive locks before it serves requests or reconciles
runtime objects. Its journal stores a persistent registry
incarnation, a monotonic admission sequence, the active slot, and attempt
receipts. Only one launcher incarnation may change instance execution state.

The aggregate boundary also has a durable manager identity. Initial provisioning
refuses any pre-existing unbound aggregate unit or cgroup, including an empty
one. Fence provisioning as a pending manager operation before effects; after
confirmed fresh creation, persist its systemd InvocationID and cgroup inode
before clearing the fence or admitting an attempt. On restart and policy change,
verify that exact binding before any controller change or manager effect on
owned descendants, including recovery and cleanup. The aggregate's
own `cgroup.procs` must be empty, and every child must match the exact retained
identity of an unsettled owned attempt. Settled receipts do not authorize a
recreated unit or container. A missing or recreated bound aggregate, or a missing
registry with a pre-existing aggregate, requires quarantine. Never automatically
adopt, recreate, reconfigure, or delete an unknown boundary. A host reboot that
loses the bound runtime identity also requires operator reconciliation; operators
must prove all prior work stopped and preserve its journal/evidence before
provisioning a fresh instance identity. A healthy socket alone cannot clear this
condition.

Persist and synchronize the launch intent and reserve the active slot atomically
before any systemd or Docker creation. The intent binds the request identity,
canonical manifest identity, captured policy, registry incarnation, and unpredictable launch
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
registry cannot clear an existing instance claim. Only a genuinely new instance
claim and registry establish a new incarnation; claimed instances with a missing
registry remain quarantined. Intentional retry
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

Systemd owns slice properties through its unit APIs. Each attempt uses a fresh
transient slice created by `StartTransientUnit` with job mode `fail`, no auxiliary
units, and only the captured memory, zero-swap, task, CPU quota/period and
`StopWhenUnneeded=no` properties. Aggregate provisioning remains separate.
Before creation, prove that the derived attempt name has no loaded unit, cgroup
or unit configuration through bounded, non-loading inventory.
Persist the pending slice operation before creation; clear it only with the
confirmed active transient unit's InvocationID and cgroup inode. Before worker
creation, verify `Transient=yes`, the expected ControlGroup and the exact
`/run/systemd/transient/<attempt-unit>` FragmentPath, retention and actual limits.
Require empty DropInPaths; inherited or unit-specific overrides are not part of
the closed attempt configuration.
A failed or ambiguous creation never permits adoption or a second creation call.
An ordinary exited scope does not retain accounting. Docker owns its scope.
The launcher creates and controls only the
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
Before releasing engine work, the launcher must confirm that the I/O controller
is available at the owned attempt boundary and that `io.stat` is readable there.
If either check fails, work remains unreleased. This launch precondition is
separate from whether complete terminal per-attempt I/O counters survive
settlement; diagnostic evidence is recorded as unavailable when it does not.

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

Confirm an OOM when the retained attempt has a positive checked `oom_kill`
delta and terminal Docker state reports `OOMKilled=true`. Also confirm it for
exit 137 when that kill delta accompanies limit pressure inside the owned
boundary: a positive checked hierarchical `oom` delta at the attempt, or a
positive checked `local_oom` delta at its exact owned processing parent.
Docker's flag can remain false for a descendant leaf OOM. Missing or regressing
counters cannot supply a positive delta. Exit 137 alone and kill counters
without terminal confirmation are insufficient; a kill counter alone does not
locate pressure because it can include an unrelated host OOM.

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

Attempt settlement must not request a global systemd reload. After terminal
evidence is durable, verify the exact bound InvocationID, cgroup inode and
transient provenance before stopping the attempt slice. Persist a pending stop
operation before the manager call and clear it only after its confirmed return;
an unresolved stop keeps the slot blocked, including after launcher restart.
The existing five-second manager-command limit remains unchanged. Do not repeat
an uncertain stop, revert unit files, remove unit configuration directly, or
raise a timeout to complete settlement.

After confirmed stop, observe removal for at most five seconds using only
bounded read operations. All queries and waits share this single absolute
observation deadline; individual reads do not receive a new interval.
An observation must prove that the exact attempt has
no cgroup, no loaded unit in non-loading manager inventory, and no matching unit
fragment or drop-in directory in any manager UnitPath, including runtime and
persistent locations. A matching symlink also counts as configuration; unreadable
or malformed inventory cannot prove absence. A still-loaded unit may remain
pending only while its captured InvocationID, transient FragmentPath and
provenance match, and it is inactive or deactivating. Read its properties through
the object path addressed by its captured InvocationID, which cannot load or
recreate a unit by name. Never query a missing unit in a way that loads or
synthesizes it. A failed property read provides no absence evidence. Only after
a durably confirmed stop with no pending manager effect may a fresh, successful,
complete non-loading inventory plus cgroup and all UnitPath checks independently
prove absence within the same deadline. Do not classify human command-error text
or retry a mutation. A positively observed foreign generation or configuration,
an incomplete or unavailable final observation, or expiry leaves cleanup
uncertain; only confirmed absence releases the slot. Recovery after confirmed partial cleanup performs the same readback
and never recreates the unit. Surviving legacy nontransient attempt units or
their configuration require operator reconciliation; the launcher does not
migrate, adopt or remove them. Aggregate identity and policy rules are unchanged.

## Capability and Failure

Startup and operator verification must exercise the actual namespace, UID,
controller placement, ancestor limits and private transport used by processing.
A missing launcher, incompatible protocol, unavailable controller or invalid
allocation disables processing while normal browsing and saved edits remain
available. Availability is not proven by an open socket or a healthy container.

Processing capability is separate from Library readiness. Report `disabled`
when the operator has not selected the processing-enabled deployment,
`unavailable` when it is selected but the launcher, approved policy, bundle,
resource boundary or reconciliation is not ready, and `available` only when
all of those checks pass for the exact deployed identities. Report source and
bundle availability separately from launcher/resource capability. The
qualification profile reports `qualification-only`; it never reports
production processing as available.

`GET /api/processing/capability` reports this read-only state independently of
`/healthz` and the CLI contract endpoint. A base deployment reports
`disabled` without contacting a launcher. A processing-enabled Web service
receives the operator-pinned instance, policy digest and bundle digest as
read-only startup values and derives the fixed socket path from the instance.
It sends Reconcile to that socket and accepts launcher readiness only for the
version-1 `photo-processing` capability, the matching instance and digests, a
valid incarnation, a positive next sequence and `available` admission. A
blocked response, protocol mismatch, unsupported capability or transport
failure reports `unavailable` with a stable reason code; it never changes
Library readiness or starts an attempt. The endpoint does not return paths,
raw launcher errors or pinned digests.

The response has `state` (`disabled`, `unavailable` or `available`), `launcher`,
`source`, `bundle`, and `reason`. `launcher`, `source` and `bundle` each report
`disabled`, `available` or `unavailable`. Bundle availability requires the
exact configured digest to match the launcher response. Source availability
remains `unavailable` until the qualified source and Export path is connected,
so the overall state cannot report `available` before that boundary exists.
`reason` is null only when the overall state is `available`; otherwise it is
one of `operator-disabled`, `launcher-unavailable`,
`unsupported-capability`, `identity-mismatch`, `launcher-blocked`, or
`source-unavailable`.

This state could be added to `/api/capabilities`, but that endpoint is versioned
for static CLI contract support. A separate path keeps changing launcher
readiness independent from CLI compatibility and `/healthz` Library health.

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

A systemd-managed host service is selected for the launcher lifecycle. Starting
the privileged launcher from the Web process or from each Compose invocation
would couple its lifetime to a client/container and make restart reconciliation
dependent on that caller. Docker restart policy is also outside the manager that
owns the processing cgroups. Systemd can keep the launcher outside the processing
subtree and restart it without granting the Web process host management rights;
the launcher still must reconcile durable ownership before reopening its socket.

A writable cgroup subtree inside the Web container would additionally require
safe delegation, migration authority and separate controller rights for
same-UID engines. It is not the supported initial deployment mechanism.

A persistent processing helper inside the capped container shares the workload's
OOM boundary and needs external reconciliation anyway. A persistent Python
simulator also changes state/reset semantics. Neither is required for this
increment.

Giving the Web service unrestricted Docker access or privileged host mounts is
rejected because it expands its authority beyond the fixed processing task.

Transient attempt slices are selected because systemd owns their lifetime and
removes their configuration when they unload. Retained accounting still lasts
until the explicit stop after evidence persistence. Runtime property overrides
with ordinary `revert` add a global reload to every settlement. Suppressing that
reload still requires destructive configuration removal after the live unit
identity disappears, plus separate durable file provenance for safe recovery.
That additional ownership model is unnecessary for fresh attempt boundaries.

## Verification

Prove native and descendant OOM at both leaf and aggregate-parent limits, worker
write/escape denial, normal exit versus OOM classification, retained final peaks,
control-service survival and a following successful job. Exercise failed start
at each ordering boundary, idempotent start, lost responses, cancel/complete
races, launcher/control restart, foreign ownership, policy changes and cleanup
failure. Validate private protocol bounds and every derived runtime argument.
Confirm that ordinary settlement requests no manager reload and leaves no
attempt unit, cgroup or runtime/persistent configuration. Exercise interrupted
creation and stop, after-stop recovery, foreign replacements with and without
an active cgroup, and legacy nontransient units without mutating them.

Prove that final accounting survives both the last worker exit and launcher
restart, and that controlled storage exhaustion and stdout/stderr flooding cannot fill the host filesystem
or remove retained evidence.

Run the exact packaged operator path with the supported Web image and processing
image before claiming deployment acceptance. Synthetic kernel tests, engine
image-quality checks and ordinary repository gates remain separate evidence.
The production acceptance path also verifies the installed launcher and policy
identities, service restart and policy-change behavior, socket peer authority,
Web and launcher placement outside the processing subtree, complete attempt
limits and retained terminal evidence. During a contained failure it verifies
that Library and Album reads still work, then completes and validates a later
attempt against its exact admitted identity. Qualification-only results do not
substitute for this production deployment evidence.
