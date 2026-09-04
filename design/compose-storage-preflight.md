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
- The existing Rust storage admission remains a separate container-visible
  defense.
- The operator contract has one authority in [Deployment](../docs/deployment.md).

## Model

The repository owns one Compose entry point, `scripts/compose`. It accepts one
operator-controlled environment file and one supported command form. It owns
the host storage preflight and the exact Compose file it executes.

The preflight reads the literal absolute directory values for
`SLIPSTREAM_LIBRARY_ROOT`, `SLIPSTREAM_STATE_DIRECTORY`, and
`SLIPSTREAM_CACHE_DIRECTORY`. For startup and expansion, each key must occur
exactly once as `KEY=/absolute/path` in the environment file. Values are
unquoted literals and may contain ordinary internal spaces. The preflight
requires every input value to be valid UTF-8 and rejects leading or trailing
whitespace, tabs, carriage returns, control characters, `#`, quotes,
backslashes, backticks, `$`, shell syntax, Compose interpolation, duplicate
keys, and any value that is not an existing directory. It does not evaluate
the environment file or reimplement general Compose dotenv syntax. It requires
the environment file to be readable and canonicalizes that file before
passing it to Docker.

The preflight canonicalizes every source with GNU `realpath -e`, resolving
symbolic links and lexical aliases without creating a directory. It then uses
Linux `findmnt --submounts` for each canonical endpoint to capture the complete
mount hierarchy rooted at the endpoint's owning mount. Every returned
`TARGET`, `FSROOT`, and `MAJ:MIN` value must be unambiguous, valid UTF-8, and
well-formed. The endpoint coordinate is the lexical relative path beneath its
owning `TARGET`, appended to that mount's `FSROOT`, together with `MAJ:MIN` as
its filesystem identity. The proof set for each storage source also includes
the `FSROOT` coordinate of every nested mount target beneath that source.

Every pair among Library, state, and cache is incompatible when their
canonical paths are equal or nested, or when any coordinates in their proof
sets have the same `MAJ:MIN` and equal or nested paths. This compares all
cross-role source roots, not only the endpoints' current devices, so a
different-filesystem mount nested below the Originals source cannot hide a
state or cache bind alias. This includes state-cache overlap: it could let
SQLite state and rebuildable cache overwrite one another through different
container mount targets. The same literal-path character rules apply to each
canonical result, so a safe input alias cannot hide an ambiguous target path.

After a successful check, the entry point passes the three canonical source
values to Docker Compose. `compose.yaml` mounts each source at that same
canonical absolute path and gives the server the same value. The process values
take precedence over the environment-file values, so Docker mounts the sources
that the preflight checked and Rust's existing layout admission can see the
ordinary nesting relationship.

## Semantics

1. The entry point requires `--env-file` followed by one readable environment
   file and exactly one supported command form. It does not accept a
   caller-selected Compose file or a second environment file.
2. The only startup forms are `up`, `up -d`, and the exact offline command
   `run --rm --no-deps slipstream expand-library`. The only retained stop form
   is `down`. No command form accepts a bind mount, environment, entrypoint,
   service, profile, or arbitrary Compose option override.
3. The entry point supports only a Linux host and the local Docker default
   context. It rejects `DOCKER_HOST`, `DOCKER_CONTEXT`, and a default context
   whose endpoint is not a local Unix socket before invoking Compose.
4. For every startup form, it canonicalizes the readable environment file and
   accepts only one literal declaration for each required storage value. It
   rejects invalid UTF-8, duplicates, ambiguous characters, variable
   references, non-absolute paths, and missing or non-directory sources
   without evaluating any file content.
5. For every startup form, it canonicalizes all three sources and captures
   their complete nested mount hierarchies. It rejects an equal, descendant,
   or ancestor relationship for every pair, including a relationship exposed
   by any cross-filesystem mount source coordinate.
6. A fixed `down` skips storage-source parsing, existence, canonicalization,
   and topology checks. It retains the environment-file, repository Compose
   file, local-context, and exact-argument constraints so an operator can stop
   an existing container after a source becomes missing or unsafe.
7. On a startup rejection it reports the incompatible source roles and exits
   before it invokes Docker Compose. It does not change the Originals tree or
   content.
8. On success it invokes `docker --context default compose` with the repository `compose.yaml`,
   the supplied environment file, and canonicalized source values. The Compose
   mount targets and server environment use those same values. It clears
   Compose process variables that could select another file, environment file,
   profile, or project, fixes the project name as `slipstream`, and rejects
   every `COMPOSE_*` or `DOCKER_*` declaration in the environment file. It
   also clears ambient `SLIPSTREAM_IMAGE`, `SLIPSTREAM_VCS_REF`,
   `SLIPSTREAM_BIND_ADDRESS`, `SLIPSTREAM_PORT`, `SLIPSTREAM_PUBLIC_ORIGIN`,
   and `SLIPSTREAM_DATABASE_BASENAME`, plus all three ambient storage values.
   Startup exports the checked canonical storage values; fixed `down` leaves
   storage configuration to the environment file without checking it.

Every retained start form traverses this entry point. The exact `run` form
includes the offline `expand-library` command, so a Library Expansion cannot
bypass host topology preflight. The narrow command surface prevents a caller
from replacing the checked mounts or server values after the preflight.

The host preflight is not a replacement for Rust storage admission. The server
continues to fail closed on the paths visible inside its container. Its mount
coordinate proof relies on the trusted local Docker daemon/socket using the
same host mount namespace as the entry point, and on the source directories
and their parent mount topology remaining stable from preflight through Docker
admission. These are operator preconditions in
[Deployment](../docs/deployment.md); the preflight does not eliminate that
time-of-check/time-of-use window.

## Options

### Selected: Bash Entry Point with GNU `realpath -e` and `findmnt`

A small Bash launcher can resolve the actual bind sources before Docker sees
them, derive their complete Linux mount hierarchy coordinates, reject unsafe
topology without creating a container, and force the validated source values
into the Compose process. Bash, GNU coreutils `realpath` and `iconv`, and
util-linux `findmnt` are part of the supported Linux host baseline, so this
adds no language runtime.
Requiring one explicit environment file, a narrow literal grammar, and exact
command forms keeps the input bounded and auditable without treating operator
configuration as shell code.

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
three topology values under a strict literal grammar is smaller and proves the
input it validates.

### Rejected: General Compose Argument Forwarding

Forwarding arbitrary arguments would let a caller add bind mounts, environment
values, an entrypoint, a second environment file, or a second Compose file
after validation. The current deployment needs one service startup, one
offline expansion, and one stop operation, so exact command forms preserve the
proof with less parser surface.

## Verification

Automated entry-point tests prove that:

- disjoint canonical host directories invoke Compose with identical canonical
  host sources, container targets, and server values;
- equal, forward-nested, reverse-nested, and symbolic-link alias layouts fail
  for every pair among Library, state, and cache;
- exact bind aliases and bind aliases of a source ancestor or descendant fail
  for every pair among Library, state, and cache;
- raw non-UTF-8 input and canonical or mount-coordinate paths fail closed
  before Docker;
- a nested mount on another filesystem whose source coordinate aliases a
  different storage role fails closed before Docker; and
- each failed startup avoids the Docker invocation and leaves the Originals
  tree and content unchanged; and
- the offline `run ... expand-library` path uses the same failed preflight.

The fixed `down` test proves that it reaches the local Compose invocation even
when a configured storage source no longer exists.

The Compose contract test also proves that the supported entry point fixes the
repository `compose.yaml`, rejects an alternate source file, and rejects
ambiguous storage declarations, topology overrides, and remote Docker contexts
rather than accepting a caller-selected replacement.
