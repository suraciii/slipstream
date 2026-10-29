# Specifications

`specs/` is the authority for Slipstream's durable product behavior, design decisions, and implementation contracts. [`CONTEXT.md`](../CONTEXT.md) defines domain vocabulary. [Usage and installation guides](../docs/README.md) explain how to use the product. Follow [contract writing rules](AGENTS.md) when changing specifications.

Use this index to locate the affected Feature and its contracts. The Product Design Language and Design Language Contract are the authorities for product design and Web implementation review; the Visual System reference fixes the Web reference scale within that authority. Feature contracts remain authoritative for exact behavior, and Web Frontend Architecture remains authoritative for code ownership and dependency direction. Follow the [context-reading rules](../AGENTS.md#context-management) and [feature organization rules](../AGENTS.md#feature-organization). Root contracts govern the product and shared system; Feature contracts own their scope, and optional capability contracts reference applicable parent constraints. Each level may have behavior, design, or both; specialist references stay beside their owner.

## Product and System

- [Product Vision](spec.md): audience, goals, principles, human and machine use, and product boundaries.
- [Product Design Language](design-language/spec.md): product character, visual and interaction grammar, content, feedback and recovery, responsive composition, and accessibility.
- [Design Language Contract](design-language/design.md): semantic layers, surface responsibilities, state ownership, carrier invariants, and design review evidence.
- [Visual System](design-language/visual-system.md): Web reference token scale, type voices, surface presentation, Preview matte, and reference composition.
- [Foundational Architecture](design.md): shared Web/CLI service boundary, file and state ownership, Photo identity, and vertical slices.
- [Rust Server](server.md): production language, module ownership, dependency direction, compatibility, cutover, and rollback.

## Access

- [Instance Access](access/spec.md): token entry, browser sessions, CLI access, sign-out, and recovery.
- [Access Design](access/design.md): credential generations, Bearer admission, revocation, CSRF, and private delivery.

## Command-Line

- [Command-Line Use](command-line/spec.md): delegated workflows, shared state, Preview delivery, effects, and recovery.
- [Command-Line Design](command-line/design.md): thin client, bounded queries, mutation guards, transport uncertainty, and Web handoff.
- [CLI Reference](command-line/reference.md): authoritative syntax, inputs, structured results, errors, and composition examples.

## Photo Library

- [Photo Library and Albums](photo-library/spec.md): indexing, read-only Original Folders, virtual Albums, independent RAW and JPEG Photos, and Location Recovery.
- [Photo Library Design](photo-library/design.md): physical Folder projection, Album ownership, source semantics, navigation, and recovery application.
- [Library Identity](photo-library/identity.md): stable Original File and Photo identity, fingerprints, exact-content recovery, and Library Folder expansion.
- [Capture-Time Ordering](photo-library/ordering.md): metadata authority, deterministic source and Album order, persistence, and rescan lifecycle.

### Previews

- [Photo Previews](photo-library/previews/spec.md): sources, provenance, quality limits, caching behavior, and failures.
- [Preview Pipeline Design](photo-library/previews/design.md): JPEG selection, extraction, normalization, caching, and delivery.

### Metadata

- [Read and Save Metadata](photo-library/metadata/spec.md): supported fields, provenance, checked XMP Sidecar saves, and external interoperability.
- [Metadata Design](photo-library/metadata/design.md): Photo and Sidecar ownership, read evidence, bounded parsing, preserved XMP content, and Save admission.

### Removal

- [Removal and Restore](photo-library/removal/spec.md): explicitly identified Photos, reversible effects, stale-state protection, and recoverable outcomes.
- [Removal and Restore Design](photo-library/removal/design.md): identity and evidence, durable attempt receipts, replay, ownership, and compare-and-set Restore.

### Rejected Cleanup

- [Rejected Photo Cleanup](photo-library/rejected-cleanup/spec.md): marking, reversible Library removal, Restore, and Original File safety.
- [Rejected Cleanup Design](photo-library/rejected-cleanup/design.md): Photo-row removal state, snapshot admission, confirmed-review operations, and recovery listing.

### Trash

- [Trash and Record Cleanup](photo-library/trash/spec.md): complete Trash browsing, Location output, Restore, external file deletion, and explicit missing-record cleanup.
- [Trash Design](photo-library/trash/design.md): captured inspection scope, read-only absence checks, transactional record cleanup, and durable outcome recovery.

## Library Browser

- [Library Browsing and Selection](library-browser/spec.md): progressive views, source order, loading, gestures, selection, rating, detail inspection, and undo.
- [Library Browsing Design](library-browser/design.md): lightweight overview, progressive Grid and Photo views, browse snapshots, scan status, and Preview caching.

### Experience

- [Library Browser Experience](library-browser/experience/spec.md): responsive surfaces, contextual controls, destinations, history restoration, and space acceptance.
- [Browser Navigation Design](library-browser/experience/design.md): URL codec, history entries, restoration, destination lifecycle, and modal ownership.

### Filters

- [Source-Scoped Library Filters](library-browser/filters/spec.md): source scope, Selection State and metadata conditions, stable filtered work, and Web/CLI handoff.

## Photo Development

- [Photo Development](photo-development/spec.md): shared editing state, peer tools, previews, exports, and recovery.
- [Photo Development Design](photo-development/design.md): independent tool integration, supervised execution, canonical file state, concurrency, and migration.
- [Photo Editing Model](photo-development/model.md): tool-owned Edit State, directory ownership, and explicit file handoff.

### Workspace

- [Photo Workspace](photo-development/workspace/spec.md): real persistent directory, protected inputs, direct tool output, file discovery, downloads, continued processing, and safe removal.
- [Directory Photo Workspace](photo-development/workspace/design.md): preparation, containment, indexing, tool-owned state, retention, consistent backup, and cutover.
- [Photo Editor Workspace](photo-development/workspace/web.md): browser ownership, tool composition, freshness, conflict, and asynchronous lifecycle.

### darktable

- [darktable Integration](photo-development/darktable/spec.md): engine relationship, capability coverage, saved intent, matching outputs, and upgrade failures.
- [darktable Integration Design](photo-development/darktable/design.md): independent editor, private native MCP interaction, canonical persistence, initialization, and cutover.
- [Headless darktable Editing](photo-development/darktable/editing.md): independent file/state editing, instances, masks, resources, Presets, Styles, history, and result delivery.
- [darktable CLI Reference](photo-development/darktable/cli.md): standalone `darktable-edit` and optional `slipstream darktable` grammar, catalog scope, guards, structured results, and recovery.

### Processing

- [Processing Modules](photo-development/processing/design.md): peer Engine adapters, discovery, explicit inputs, bounded Preview, and immutable Export Artifacts.
- [Development Color](photo-development/processing/color.md): module-owned image contracts, scene-referred output, simulation, display separation, and reproducibility.
- [Processing Executor](photo-development/processing/execution.md): application container, serialized engine children, scratch lifetime, cleanup, and restart settlement.
- [Processing Memory](photo-development/processing/memory.md): shared memory/CPU boundary, engine workspace planning, buffer ownership, and resource-failure recovery.

## Web Application

- [Web Frontend](web-application/design.md): Feature-Sliced Design layers, Library Browser ownership, dependency direction, and migration constraints.
- [Web Async Ownership](web-application/async-ownership.md): read scopes, write settlement, and commit-ordered Web convergence.

### Installation

- [Installed Web Application](web-application/installation/spec.md): online installation, launch, and supported deployment.
- [Web Installation Design](web-application/installation/design.md): manifest ownership and installation resource delivery.

## Deployment

- [Deployment](deployment/spec.md): image, host storage, and supported Compose operator contract.
- [Support and Release](deployment/support.md): supported environment, files and cameras, recovery, limitations, rollback-artifact retention, and 0.1.0 release notes.
- [Container Build Inputs](deployment/container-inputs.md): immutable base images, Ubuntu native packages, and release qualification.
- [Storage Preflight](deployment/storage-preflight.md): host bind-source topology, operator entry-point ownership, and Original-safety failure boundary.
