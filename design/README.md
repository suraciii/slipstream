# Design Specs

`design/` explains why architecture boundaries exist and records contracts that implementations must preserve. It is not a tour of the current code.

Follow the scoped instructions in [`AGENTS.md`](AGENTS.md) when writing or changing a Design Spec. Product behavior belongs in [`../docs/`](../docs/README.md), and shared language belongs in [`../CONTEXT.md`](../CONTEXT.md).

## Photo Development

- [Photo Development Architecture](photo-development.md): recipe ownership, headless processing, concurrency, export snapshots, source safety, and recovery
- [Development Color Pipeline](development-color.md): RAW interpretation, linear TIFF handoff, fixed film simulation, display separation, and reproducibility
- [Processing Executor](processing-executor.md): private host launcher, restricted execution authority, retained accounting, and restart settlement
- [Film Resource Measurement](processing-film-measurement.md): closed operator fixtures, sealed inputs, fixed resource terms, exact references, and descriptor-only evidence
- [Processing Memory](processing-memory.md): task memory isolation, deployment budgets, engine workspace planning, buffer ownership, and resource-failure recovery

## Foundations

- [Instance Access Architecture](access.md): credential generations, browser sessions, Bearer admission, revocation, CSRF, and private delivery
- [Web Installation](web-installation.md): manifest ownership and installation resource delivery

- [Foundational Architecture](architecture.md): shared Web/CLI service boundary, file ownership, Photo identity, state ownership, and the first vertical slice
- [Command-Line Architecture](command-line.md): thin client, bounded queries, shared mutation guards, transport uncertainty, Preview files, and Web handoff
- [Compose Host Storage Preflight](compose-storage-preflight.md): host bind-source topology, operator entry-point ownership, and Original-safety failure boundary
- [Photo Library Identity and Expansion](library-identity.md): stable Original File and Photo identity, content fingerprints, exact-content Location Recovery, one Library Folder, explicit ancestor expansion, failure behavior, and rejected asset-management abstractions
- [Physical File Locations and Virtual Albums](photo-organization.md): read-only Original Folder projection, Album ownership, source semantics, bounded Folder navigation, recovery application, and the v5-to-v6 independent-Photo migration
- [Rust Server Architecture](rust-server.md): production language, module ownership, dependency direction, compatibility, cutover, and rollback
- [Container Build Inputs](container-inputs.md): immutable base images, Ubuntu native package inputs, and release qualification boundaries
- [Capture-Time Library Ordering](capture-time-ordering.md): metadata authority, deterministic Library and Original Folder order, explicit Album order, persistence, rescan lifecycle, migration, and rollback
- [Scalable Library Browsing](library-browsing.md): lightweight overview, progressively loaded Grid and Photo views, stable hidden browse snapshots, background scan status, and persistent Preview caching
- [Preview Pipeline](preview-pipeline.md): own-file JPEG and embedded-JPEG selection, extraction, normalization, caching, and delivery
- [Web Async Ownership](web-async-ownership.md): read scopes, write settlement, and commit-ordered convergence in the Web client
- [Browser Navigation and Responsive Surfaces](browser-navigation.md): URL codec, history entries, bounded restoration, destination lifecycle, and modal ownership
- [Web Frontend Architecture](web-frontend-architecture.md): incremental Feature-Sliced Design layers, Library Browser ownership, dependency direction, and migration constraints
