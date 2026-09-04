# Container Build Inputs

Slipstream supports one Linux amd64 container deployment. A release candidate
must not receive different native libraries merely because a base-image tag,
APT mirror, or CI runner changed after review.

## Design Drivers

- The supported image needs native build and runtime libraries from Ubuntu
  26.04.
- A normal dependency update must be visible in review.
- Release qualification must build the same container definition that an
  operator deploys.
- The repository must not add a release service or image publication path.

## Model

The checked-in container input contract has three owners:

- [`../Dockerfile`](../Dockerfile) selects the Bun, Rust, and Ubuntu image
  indexes by immutable digest.
- [`../docker/apt/ubuntu.sources`](../docker/apt/ubuntu.sources) selects one
  signed official Ubuntu snapshot for amd64.
- The build and runtime lock lists in [`../docker/apt/`](../docker/apt/) pin
  the direct native packages. The snapshot and digest-pinned base image fix
  their transitive package choices.

The Rust image supplies the CA bundle needed to contact the Ubuntu HTTPS
snapshot before Ubuntu installs its own locked `ca-certificates` package. That
bundle is itself an input of a digest-pinned stage. APT always verifies the
Ubuntu archive signature and TLS certificate. A failed certificate check,
missing package version, or unavailable snapshot fails the image build. APT
updates use `--error-on=any`, so a partial snapshot failure cannot leave a
stale index available to satisfy a lock.

## Semantics

Source reproducibility means the same candidate resolves the same base-image
indexes, Ubuntu snapshot, direct package versions, and APT dependency choices.
It does not promise a byte-identical final image from separate builds: build
tools and image assembly may add output-specific metadata.

Qualification builds the repository Dockerfile for `linux/amd64`. It records
the resulting image separately from the input contract. The ordinary GitHub
Actions runner may remain `ubuntu-latest` because it verifies repository source
and tests only. It is not a release-image input and it does not qualify a
container build. Qualification uses an explicit `docker buildx build` command
for `linux/amd64`; it does not use Compose. The Compose service declares
`linux/amd64` but has no `build` input: the supported entry point runs only the
digest-pinned `SLIPSTREAM_IMAGE` defined by [Deployment](../docs/deployment.md),
with no repository-source fallback.

Changing a base digest, snapshot timestamp or suite, or direct package lock is
one container dependency update. The same review must update every affected
input and rerun the container input contract. No mutable mirror, image tag, or
fallback source is permitted.

## Options

### Selected: Digest, Snapshot, and Direct Locks

An image digest makes the image index immutable. One Ubuntu snapshot fixes the
available package repository, while the two small direct locks show the native
libraries Slipstream intentionally requests. This keeps ordinary updates
reviewable without hand-maintaining a duplicate lock for every APT dependency.

### Rejected: Pin Direct Packages Against a Moving Archive

Exact direct versions alone leave index metadata and dependency resolution
dependent on archive timing. They can disappear or resolve differently after
review.

### Rejected: Treat `ubuntu-latest` as the Image Contract

The CI runner is a source-test environment owned by GitHub. Its operating
system and its installed APT dependencies neither select nor prove the Docker
image's native inputs.

## Verification

`bun run test:container-input` verifies the checked-in input contract. It runs
through the existing `test:fast` and `verify` gates. The Compose focused
contract verifies the digest-only service wiring. Release qualification builds
the same Dockerfile for Linux amd64 and records output traceability separately
from source reproducibility, as defined by the
[deployment guide](../docs/deployment.md).
