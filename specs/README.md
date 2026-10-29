# Specifications

`specs/` is the authority for Slipstream's durable product behavior, design decisions, and implementation contracts. [`CONTEXT.md`](../CONTEXT.md) defines domain vocabulary. [Usage and installation guides](../docs/README.md) explain how to use the product. Follow [contract writing rules](AGENTS.md) when changing specifications.

Use this index to locate the affected topic and its contracts. Follow the [context-reading rules](../AGENTS.md#context-management) and [topic organization rules](AGENTS.md#structure).

The Product Design Language and Design Language Contract govern product design and Web implementation review. The Visual System reference fixes the Web reference scale within that authority.

Topic contracts own exact behavior within their scope. Web Frontend Architecture owns code boundaries and dependency direction.

## Product and System

- [Product Vision](spec.md): audience, shared human and machine use, Preview trust, and product boundaries.
- [Foundational Architecture](design.md): shared service boundary, state ownership, module responsibilities, and client integration.
- [Rust Server](server.md): runtime ownership, startup and shutdown, compatibility, and native boundaries.

## Design Language

- [Product Design Language](design-language/spec.md): product character, interaction grammar, feedback, responsive composition, and accessibility.
- [Design Language Contract](design-language/design.md): semantic layers, surface responsibilities, state ownership, and carrier invariants.
- [Visual System](design-language/visual-system.md): Web reference scale, type voices, surface presentation, Preview matte, and composition.

## Access

- [Instance Access](access/spec.md): token entry, browser sessions, CLI access, sign-out, and recovery.
- [Access Design](access/design.md): credential generations, Bearer admission, revocation, CSRF, and private delivery.

## Command-Line

- [Command-Line Use](command-line/spec.md): delegated workflows, shared state, Preview delivery, effects, and recovery.
- [Command-Line Design](command-line/design.md): thin client, bounded queries, mutation guards, transport uncertainty, and Web handoff.
- [CLI Reference](command-line/reference.md): command semantics, input boundaries, results, errors, and composition.

### Target Wire Schemas

- [Query Results](../schemas/cli/results-query.schema.json): Library, Photo, Folder, and Album query shapes.
- [Mutation Results](../schemas/cli/results-mutations.schema.json): checked decisions, Album mutations, recovery, removal, and Restore shapes.
- [Processing Recipes](../schemas/cli/processing-recipe.schema.json): captured steps, bindings, and recipe mutation inputs.
- [Processing Exports](../schemas/cli/processing-export.schema.json): immutable outputs, work records, and download exchanges.
- [Processing Discovery](../schemas/cli/processing-discovery.schema.json): engine and image-contract discovery shapes.
- [Workspace](../schemas/cli/workspace.schema.json): workspace file and mutation result shapes.
- [Errors](../schemas/cli/errors.schema.json): structured failure shapes.

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

#### Rejected Cleanup

- [Rejected Photo Cleanup](photo-library/removal/rejected-cleanup/spec.md): rejected-photo review, confirmation, and recovery through shared removal.
- [Rejected Cleanup Design](photo-library/removal/rejected-cleanup/design.md): Snapshot admission and the browser review boundary.

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
- [Photo Development Design](photo-development/design.md): tool-owned state, file handoff, supervised execution, and concurrency.

### Workspace

- [Photo Workspace](photo-development/workspace/spec.md): real persistent directory, protected inputs, direct tool output, file discovery, downloads, continued processing, and safe removal.
- [Directory Photo Workspace](photo-development/workspace/design.md): preparation, containment, indexing, tool-owned state, retention, and consistent backup.
- [Photo Editor Workspace](photo-development/workspace/web.md): browser ownership, tool composition, freshness, conflict, and asynchronous lifecycle.

### darktable

- [darktable Integration](photo-development/darktable/spec.md): engine relationship, capability coverage, saved intent, matching outputs, and upgrade failures.
- [darktable Integration Design](photo-development/darktable/design.md): independent editor, native boundary, canonical persistence, and initialization.
- [Headless darktable Editing](photo-development/darktable/editing.md): independent file/state editing, instances, masks, resources, Presets, Styles, history, and result delivery.
- [darktable CLI Reference](photo-development/darktable/cli.md): standalone `darktable-edit` and optional `slipstream darktable` grammar, catalog scope, guards, structured results, and recovery.

### Processing

- [Processing Modules](photo-development/processing/design.md): peer Engine adapters, discovery, explicit inputs, bounded Preview, and immutable Export Artifacts.
- [Development Color](photo-development/processing/color.md): module-owned image contracts, scene-referred output, simulation, display separation, and reproducibility.
- [Processing Executor](photo-development/processing/execution.md): application container, serialized engine children, scratch lifetime, cleanup, and restart settlement.
- [Processing Memory](photo-development/processing/memory.md): shared memory/CPU boundary, engine workspace planning, buffer ownership, and resource-failure recovery.

## Web Application

- [Web Frontend](web-application/design.md): Feature-Sliced Design layers, ownership, dependency direction, and public APIs.
- [Web Async Ownership](web-application/async-ownership.md): read scopes, write settlement, and commit-ordered Web convergence.

### Installation

- [Installed Web Application](web-application/installation/spec.md): online installation, launch, and supported deployment.
- [Web Installation Design](web-application/installation/design.md): manifest ownership and installation resource delivery.

## Deployment

- [Deployment](deployment/spec.md): image, host storage, and supported Compose operator contract.
- [Support](deployment/support.md): supported environments, file boundaries, recovery, limitations, and qualified-release compatibility.
- [Container Build Inputs](deployment/container-inputs.md): immutable base images and native dependency ownership.
- [Storage Preflight](deployment/storage-preflight.md): host bind-source topology, operator entry-point ownership, and Original-safety failure boundary.
