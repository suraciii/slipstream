# Design Specs

`design/` explains why architecture boundaries exist and records contracts that implementations must preserve. It is not a tour of the current code.

Follow the scoped instructions in [`AGENTS.md`](AGENTS.md) when writing or changing a Design Spec. Product behavior belongs in [`../docs/`](../docs/README.md), and shared language belongs in [`../CONTEXT.md`](../CONTEXT.md).

## Foundations

- [Foundational Architecture](architecture.md): local Web deployment, file ownership, Photo identity, state ownership, and the first vertical slice
- [Compose Host Storage Preflight](compose-storage-preflight.md): host bind-source topology, operator entry-point ownership, and Original-safety failure boundary
- [Photo Library Identity and Expansion](library-identity.md): stable Original File and Photo identity, one Library Folder, explicit ancestor expansion, failure behavior, and rejected asset-management abstractions
- [Physical File Locations and Virtual Albums](photo-organization.md): read-only Original Folder projection, Album ownership, source semantics, bounded Folder navigation, and the v4-to-v5 terminology migration
- [Rust Server Architecture](rust-server.md): production language, module ownership, dependency direction, compatibility, cutover, and rollback
- [Container Build Inputs](container-inputs.md): immutable base images, Ubuntu native package inputs, and release qualification boundaries
- [Capture-Time Library Ordering](capture-time-ordering.md): metadata authority, deterministic Library and Original Folder order, explicit Album order, persistence, rescan lifecycle, migration, and rollback
- [Scalable Library Browsing](library-browsing.md): lightweight overview, progressively loaded Grid and Photo views, stable hidden browse snapshots, background scan status, and persistent Preview caching
- [Preview Pipeline](preview-pipeline.md): matching-JPEG and embedded-JPEG selection, extraction, normalization, caching, and delivery
- [Web Async Ownership](web-async-ownership.md): read scopes, write settlement, and commit-ordered convergence in the Web client
- [Web Frontend Architecture](web-frontend-architecture.md): incremental Feature-Sliced Design layers, Library Browser ownership, dependency direction, and migration constraints
