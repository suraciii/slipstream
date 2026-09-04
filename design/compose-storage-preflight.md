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
defines the supported operator invocation, input grammar, host prerequisites,
and configuration precedence.

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
   It cannot reach Compose unless the preflight establishes the three-source
   invariant.
2. A preflight rejects every equal, ancestor, descendant, symbolic-link alias,
   or mount-coordinate alias pair. Ambiguous source or mount information also
   fails closed.
3. A failed startup reports the incompatible roles and exits before it invokes
   Docker Compose. It does not change the Originals tree or content.
4. After a successful check, Docker mounts the checked canonical sources at the
   same paths the server receives. The server's existing storage admission
   remains a second safety boundary.
5. The supported stop operation is not a startup path. It may run without
   source existence or topology checks so an operator can stop a container
   after its configuration becomes unavailable or unsafe.
6. Docker autonomous restart is prohibited. Any future automatic recovery must
   use a preflight-aware host start path.

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

The Compose contract test also proves that the entry point fixes the repository
Compose configuration, rejects replacement configuration and topology
overrides, and leaves no autonomous restart policy.
