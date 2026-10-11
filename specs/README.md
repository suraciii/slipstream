# Specifications

`specs/` is the authority for Slipstream's durable product behavior, design decisions, and implementation contracts. [`CONTEXT.md`](../CONTEXT.md) defines domain vocabulary. [Usage and installation guides](../docs/README.md) explain how to use the product. Follow [contract writing rules](AGENTS.md) when changing specifications.

Before implementing or reviewing a behavior change, read the affected capability's behavior contract. For changes to boundaries, ownership, lifecycle, data, ordering, or failures, also read its design and named specialist contracts. Follow links between capabilities when the change crosses their shared boundaries.

## Product and Access

- [Product Vision](product-vision/spec.md): human and machine use, audience, goals, principles, and product boundaries.
- [Command-Line Use](command-line/spec.md): delegated workflows, shared state, Preview delivery, effects, and recovery.
- [CLI Reference](command-line/reference.md): authoritative syntax, inputs, structured results, errors, and composition examples.
- [Command-Line Design](command-line/design.md): thin client, bounded queries, shared mutation guards, transport uncertainty, Preview files, and Web handoff.
- [Instance Access](access/spec.md): token entry, browser sessions, CLI access, sign-out, and recovery.
- [Access Design](access/design.md): credential generations, Bearer admission, revocation, CSRF, and private delivery.

## Library and Browsing

- [Photo Library and Albums](photo-library/spec.md): indexing, read-only Original Folders, virtual Albums, independent RAW and JPEG Photos, and Location Recovery.
- [Photo Library Design](photo-library/design.md): physical Folder projection, Album ownership, source semantics, navigation, and recovery application.
- [Library Identity](photo-library/identity.md): stable Original File and Photo identity, fingerprints, exact-content recovery, Library Folder expansion, and failure behavior.
- [Capture-Time Ordering](photo-library/ordering.md): metadata authority, deterministic source order, Album order, persistence, and rescan lifecycle.
- [Library Browser Experience](library-browser/spec.md): responsive surfaces, contextual controls, destinations, history restoration, and space acceptance.
- [Browser Navigation Design](library-browser/design.md): URL codec, history entries, restoration, destination lifecycle, and modal ownership.
- [Library Browsing and Selection](library-browsing/spec.md): progressive views, source order, loading, gestures, selection, rating, detail inspection, and undo.
- [Library Browsing Design](library-browsing/design.md): lightweight overview, progressive Grid and Photo views, browse snapshots, scan status, and persistent Preview caching.
- [Source-Scoped Library Filters](library-filters/spec.md): source scope, Selection State and metadata conditions, stable filtered work, and Web/CLI handoff.
- [Rejected Photo Cleanup](rejected-photo-cleanup/spec.md): marking, reversible Library removal, Restore, and Original File safety.
- [Rejected Cleanup Design](rejected-photo-cleanup/design.md): Photo-row removal state, snapshot admission, confirmed-review operations, compare-and-set Restore, and recovery listing.
- [Photo Previews](previews/spec.md): sources, provenance, quality limits, caching behavior, and failures.
- [Preview Pipeline Design](previews/design.md): JPEG selection, extraction, normalization, caching, and delivery.

## Photo Development and Processing

- [Photo Development](photo-development/spec.md): RAW correction, autosave, film simulation, stage previews, Export, shared client behavior, and recovery.
- [Photo Development Design](photo-development/design.md): Edit State ownership, headless processing, concurrency, Export snapshots, source safety, and recovery.
- [Photo Editing Model](photo-development/model.md): editing vocabulary and direct Edit State to Artifact handoff.
- [Photo Editor Workspace](photo-development/workspace.md): browser ownership, module composition, freshness, conflict, and cross-step lifecycle.
- [Development Color](development-color/design.md): module-owned image contracts, scene-referred output, simulation, display separation, and reproducibility.
- [Processing Modules](processing-modules/design.md): peer Engine adapters, discovery, explicit inputs, bounded Preview, and immutable Export Artifacts.
- [Processing Executor](processing-executor/design.md): application container, serialized engine children, scratch lifetime, cleanup, and restart settlement.
- [Processing Memory](processing-memory/design.md): shared memory/CPU boundary, serialized execution, engine workspace planning, buffer ownership, and resource-failure recovery.
- [darktable Integration](darktable-integration/spec.md): engine relationship, admitted controls, saved intent, matching outputs, and upgrade failures.
- [darktable Integration Design](darktable-integration/design.md): private MCP interaction, parameter discovery, image initialization, fresh engine children, and cutover.

## Library Management

- [Removal and Restore](library-removal/spec.md): explicitly identified Photos, reversible effects, stale-state protection, and recoverable outcomes.
- [Removal and Restore Design](library-removal/design.md): explicit identity and evidence, durable attempt receipts, replay, ownership, and compare-and-set Restore.
- [Trash and Permanent Deletion](trash/spec.md): complete Trash browsing, Restore, explicit Original deletion, and truthful batch outcomes.
- [Trash Design](trash/design.md): reviewed deletion identity, durable operation receipts, confined filesystem effects, partial settlement, and crash reconciliation.
- [Read and Save Metadata](metadata/spec.md): supported fields, provenance, checked XMP Sidecar saves, and external interoperability.
- [Metadata Design](metadata/design.md): Photo and Sidecar ownership, read evidence, bounded parsing, preserved XMP content, and exclusive Save admission.

## Installation, Deployment, and Support

- [Installed Web Application](installed-web-app/spec.md): online installation, launch, and supported deployment.
- [Web Installation Design](installed-web-app/design.md): manifest ownership and installation resource delivery.
- [Support and Release](support-and-release/spec.md): supported environment, files and cameras, recovery, limitations, rollback-artifact retention, and 0.1.0 release notes.
- [Deployment](deployment/spec.md): image, host storage, and supported Compose operator contract.
- [Container Build Inputs](container-inputs/design.md): immutable base images, Ubuntu native packages, and release qualification.
- [Storage Preflight](storage-preflight/design.md): host bind-source topology, operator entry-point ownership, and Original-safety failure boundary.

## Shared Architecture

- [Foundational Architecture](architecture/design.md): shared Web/CLI service boundary, file ownership, Photo identity, state ownership, and vertical slices.
- [Rust Server](rust-server/design.md): production language, module ownership, dependency direction, compatibility, cutover, and rollback.
- [Web Async Ownership](web-async-ownership/design.md): read scopes, write settlement, and commit-ordered Web convergence.
- [Web Frontend](web-frontend/design.md): Feature-Sliced Design layers, Library Browser ownership, dependency direction, and migration constraints.
