# Compose Host Storage Preflight

Compose bind mounts can expose one host directory at several unrelated
container paths. The Rust server can reject unsafe paths it sees inside the
container, but it cannot determine whether separate container paths resolve to
the same or nested host source. Slipstream therefore needs one host-owned
check before Compose creates or starts a container.

## Design Drivers

- Original Files are irreplaceable and must remain read-only.
- The same validation must protect ordinary startup and offline Library
  Expansion.
- The checked host sources must be the exact sources that Docker mounts.
- Canonical host paths alone do not reveal Linux bind-mount aliases.
- A failed check must not invoke Compose or change the Originals tree or
  content.
- Docker must not autonomously restart the service around the host check.
- The existing Rust storage admission remains a separate container-visible
  defense.
- The operator contract has one authority in [Deployment](../docs/deployment.md).

## Model

The repository owns one Compose entry point, `scripts/compose`. It owns the
host storage preflight, the Compose configuration it executes, and the
canonical host sources supplied to Docker. [Deployment](../docs/deployment.md)
defines the supported operator invocation, image and storage input grammar,
host prerequisites, configuration precedence, and bounded QA instance identity.

A short-lived QA instance may share the trusted local Docker Engine with the
ordinary instance, but not its Compose project, network, published port, or
storage. Its explicit ID selects an isolated Compose project; the ordinary
invocation continues to select only the existing `slipstream` project. The QA
Library, state, and cache are separate operator-created directories directly
under the entry point's own `dist/qa-ID/` fixture root. A fresh ID is needed for
each qualification run because an existing ID retains its credential and
state. The launcher derives the three QA source paths from that root, rejects
caller-provided storage declarations, and checks the existing host preflight
before starting either instance. The QA fixture root must not contain symlinks
or nested mounts that could alias production storage. Identity and fixture
checks apply to all QA preflight actions; only startup checks an explicit
loopback endpoint before Compose. An ID is required again to stop or
administer that QA instance; no ambient Compose variable can choose either
project.

For startup and Library Expansion, the preflight works over exactly three host
storage roles: Library, state, and cache. It canonicalizes each source without
creating it and derives a proof set from the endpoint and its complete nested
mount hierarchy. A proof coordinate pairs a filesystem identity with an
effective path inside that filesystem.

Every pair among Library, state, and cache is incompatible when their canonical
paths are equal or nested, or when any coordinates in their proof sets have
the same filesystem identity and equal or nested paths. This compares all
cross-role source roots, not only the endpoints' current filesystems, so a
different-filesystem mount nested below the Originals source cannot hide a
state or cache bind alias. State-cache overlap is also incompatible: it could
let SQLite state and rebuildable cache overwrite one another through different
container mount targets.

After a successful check, the entry point passes the three canonical source
values to Docker Compose. Docker mounts the sources that the preflight checked,
and the server receives those same values so its existing layout admission can
see the ordinary nesting relationship.

## Semantics

1. Every supported startup and Library Expansion traverses the entry point.
   It cannot reach Compose unless the required immutable image and the
   three-source invariant are established. The entry point runs Compose with
   build disabled, so the repository cannot become an image fallback.
2. A preflight rejects every equal, ancestor, descendant, symbolic-link alias,
   or mount-coordinate alias pair. Ambiguous source or mount information also
   fails closed.
3. A failed startup reports the incompatible roles and exits before it invokes
   Docker Compose. It does not change the Originals tree or content.
4. After a successful check, Docker mounts the checked canonical sources at the
   same paths the server receives. The server's existing storage admission
   remains a second safety boundary.
5. The supported stop operation is not a startup path. It may run without
   image, source-existence, or topology checks so an operator can stop a
   container after its configuration becomes unavailable or unsafe.
6. Docker autonomous restart is prohibited. Any future automatic recovery must
   use a preflight-aware host start path.

7. QA starts and access administration use the same image, storage, and
   credential checks, with finite container resources. A rejected QA invocation
   cannot call Compose for either the QA or ordinary project. A QA stop targets
   only its explicit project using the stop-only sentinels even after fixture
   paths or image disappear; it checks no fixture paths and neither stop
   operation removes host directories.

The host preflight is not a replacement for Rust storage admission. The server
continues to fail closed on the paths visible inside its container. Its mount
coordinate proof relies on the trusted local Docker daemon/socket using the
same host mount namespace as the entry point, and on the source directories
and their parent mount topology remaining stable from preflight through Docker
admission. These are operator preconditions in
[Deployment](../docs/deployment.md); the preflight does not eliminate that
time-of-check/time-of-use window.

## Options

### Selected: Host-Owned Bash Entry Point

A small Bash launcher can resolve the actual bind sources before Docker sees
them, derive their complete Linux mount hierarchy coordinates, reject unsafe
topology without creating a container, and force the validated source values
into the Compose process. It adds no production language runtime. The narrow
operator surface remains defined in [Deployment](../docs/deployment.md), rather
than treating operator configuration as shell code.

### Selected: Explicit QA Instance on the Same Engine

An explicit, bounded QA identity keeps the normal operator interface unchanged
while allowing a fixture-only instance beside it. The launcher owns the project
name, derived fixture paths, explicit `127.0.0.1` address and decimal port
(1024-65535) on startup, and the fixed `compose.qa.yaml` override. That
repository-owned override adds no services, mounts, or restarts and limits
both service and admin containers to 2 GiB memory, 2 CPUs, and 256 PIDs.
Docker owns project-network separation and rejection of an occupied host port;
the launcher cannot preflight the ordinary instance's port without its
independent environment file. Shared daemon capacity and host mount aliases
remain operator preconditions, not guarantees supplied by container
namespaces. The QA root must not be mounted over production storage.

### Rejected: Ambient Compose Project Override

`COMPOSE_PROJECT_NAME` alone can change the target of a stop operation without
changing the validated storage or endpoint. That defeats the bounded launcher
surface and makes an accidental production operation plausible.

### Rejected: Mandatory Separate Docker Host

A separate host supplies stronger daemon and resource isolation, but it is not
necessary for short-lived fixture-only QA when the launcher constrains its
project, bind sources, endpoint, and resources. It remains appropriate for
untrusted workloads or adversarial isolation tests.

### Rejected: Container Startup Check Only

By container startup, Docker has already accepted the host binds. The server
sees container mount targets rather than the original host-source
relationships, so it cannot prove a host alias or reverse nesting is safe
before a write-capable mount exists.

### Rejected: Compose Hook or Second Service

Compose has no host-side hook that runs before bind creation. A helper
container would run after Compose begins and would add lifecycle and image
surface without providing the required zero-start failure behavior.

### Rejected: Source the Environment File in Bash

Sourcing would make the preflight execute operator-controlled shell code and
would give shell expansion different semantics from Compose. Parsing just the
three topology values is smaller and proves the input it validates.

### Rejected: General Compose Argument Forwarding

Forwarding arbitrary arguments would let a caller replace checked mounts or
server values after validation. The bounded operator surface preserves the
proof with less parser surface.

## Verification

Automated entry-point tests prove that:

- disjoint canonical host directories invoke Compose with identical canonical
  host sources, container targets, and server values;
- equal, forward-nested, reverse-nested, and symbolic-link alias layouts fail
  for every pair among Library, state, and cache;
- exact bind aliases and bind aliases of a source ancestor or descendant fail
  for every pair among Library, state, and cache;
- unsafe or ambiguous source and mount information fails closed before Docker;
- a nested mount on another filesystem whose source coordinate aliases a
  different storage role fails closed before Docker; and
- each failed startup avoids the Docker invocation and leaves the Originals
  tree and content unchanged; and
- Library Expansion uses the same failed preflight.

The supported stop-operation test proves that it reaches the fixed Compose path
even when a configured storage source no longer exists.

The Compose contract test also proves that the entry point fixes the
repository Compose configuration, rejects replacement configuration and
topology overrides, runs a digest-only service with build disabled, and leaves
no autonomous restart policy. QA tests must also prove that valid startup,
access administration, and stop target only the QA project; malformed IDs,
unsafe fixture paths, and non-loopback endpoints fail before Docker; the
ordinary invocation retains its exact project and Compose arguments; QA
`down` with unavailable storage values still selects its own project; and the
QA-only override enforces the exact resource limits on both containers
without changing the ordinary service.
