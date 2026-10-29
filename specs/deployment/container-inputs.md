# Container Build Inputs

The supported Linux amd64 image MUST resolve the same native inputs for the same candidate. This is source reproducibility, not a promise of byte-identical output images.

## Input Ownership

- The [Dockerfile](../../Dockerfile) selects each non-scratch base image index by immutable digest.
- The [Ubuntu source configuration](../../docker/apt/ubuntu.sources) selects a signed official Ubuntu snapshot for amd64.
- The [build and runtime locks](../../docker/apt/) pin direct native packages; the snapshot and base digest fix transitive choices.
- The Dockerfile pins the upstream LibRaw release archive by SHA-256 for both build headers and the service's runtime library.

Ubuntu 26.04 supplies the native build and runtime packages. The Rust image supplies the CA bundle used to contact the HTTPS snapshot before Ubuntu installs its locked `ca-certificates` package.

APT MUST verify the archive signature and TLS certificate. A certificate failure, unavailable snapshot, partial update, or missing package version MUST fail the build instead of using a stale index or fallback source.

The LibRaw archive MUST be hash-verified during the build. An unavailable archive or mismatched hash MUST fail the build.

The service MUST resolve the matching LibRaw runtime from its isolated `LD_LIBRARY_PATH`. Ubuntu's LibRaw package MAY satisfy other native package dependencies but MUST NOT silently replace the service's decoder.

A LibRaw pin change MUST advance the persisted RAW decoder revision so previously unavailable RAW Photos are re-inspected once. This boundary avoids mixing headers and runtime binaries while allowing the decoder to change independently of the Ubuntu snapshot.

## Updates and Qualification

A change to a base digest, snapshot timestamp or suite, direct package lock, or LibRaw pin and checksum MUST be reviewed as one dependency update. The update MUST cover every affected input and recheck the input contract.

Mutable image tags, moving archives, mirrors, and fallback sources MUST NOT replace the pinned inputs. Release qualification MUST build the repository Dockerfile for `linux/amd64` and record the resulting image separately from its source inputs.

The ordinary CI runner is not an image input or release-image qualification. Supported Compose runs the digest-pinned image without a build or source fallback, as defined by [Deployment](spec.md#storage-rules).
