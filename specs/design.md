# Foundational Architecture

Slipstream uses one server-owned modular monolith for a Photographer and their external Agent. [Product Vision](spec.md) owns product scope; [Rust Server Architecture](server.md) owns the production runtime and lifecycle.

## Ownership

| Owner                    | Canonical state                                                                                                 |
| ------------------------ | --------------------------------------------------------------------------------------------------------------- |
| Filesystem               | Photographer-owned Original Files.                                                                              |
| SQLite                   | Photo identity, Library facts, Albums, decisions, saved Album positions, and derivative metadata.               |
| Browser                  | Transient gestures, Grid and source position, zoom, pan, animations, and one-level Undo.                        |
| Slipstream CLI           | Invocation-local input, output, and downloads.                                                                  |
| Independent editing tool | Persistent Edit State and processing files within the [Photo Workspace](photo-development/workspace/design.md). |

Clients MUST NOT become additional owners of durable Library state or an Agent runtime. [Command-Line Architecture](command-line/design.md) owns the machine-client boundary.

## Application Model

One server process owns application configuration and resource lifetime. [Rust Server Architecture](server.md#lifecycle) defines startup admission, bounded work queues, and shutdown ordering.

Module imports MUST NOT perform:

- file discovery;
- storage creation;
- listener binding.

[Instance Access](access/design.md) owns authentication at the HTTP boundary. [Deployment](deployment/spec.md) owns transport and network exposure without introducing accounts or changing Library ownership.

### Photo Library Scope

The [Photo Library](photo-library/design.md) owns discovery and organization beneath the admitted Library Folder. The Folder is a location boundary, not Library identity.

[Library Identity](photo-library/identity.md) owns identity-preserving Location Recovery and ancestor expansion. Persisted Original File and Photo IDs MUST remain opaque after creation.

Every Original read MUST use the server's confinement boundary. Application record cleanup MUST NOT acquire filesystem write or delete authority.

### Library Browser Scope

[Library Browsing](library-browser/design.md) owns hidden Browse Snapshots and bounded windows of current Photo facts. [Capture-Time Ordering](photo-library/ordering.md) owns Library and Original Folder order; Albums retain explicit membership order.

Only the last Photo shown in an Album's Photo View is a durable browsing position. Other navigation and interaction state remains browser-local.

Selection State and Rating are server-authoritative. [Library Browser](library-browser/spec.md) owns gesture outcomes, persistence feedback, and guarded Undo.

### Indexing

Indexing MUST NOT decode every RAW or generate every Preview before the Library becomes usable. Indexing and Preview work MUST NOT block gesture interaction.

[Photo Library](photo-library/spec.md) owns per-file discovery and recovery outcomes. [Scan and Browse Lifecycle](photo-library/ordering.md#scan-and-browse-lifecycle) owns complete-snapshot publication, replacement scans, and initialization visibility.

## Durable State

SQLite owns explicit mutable Library state. Original contents and extracted JPEG bytes MUST remain outside SQLite.

| State group          | Durable facts                                                                                                                   |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Admission            | Schema version and admitted Library Folder binding.                                                                             |
| Originals and Photos | Stable identities, the single-Original relationship, Location, kind, revision, availability, and Capture Time inspection facts. |
| Previews             | Source, dimensions, cache revision, and failure state.                                                                          |
| Albums               | Identity, name, explicit order, membership, and saved Photo position.                                                           |
| Decisions            | Photo Selection State and Rating.                                                                                               |

[Current Photo Facts](library-browser/design.md#current-photo-facts) owns mutation transactions and compare-and-set Undo. [Photo Organization](photo-library/design.md) owns Album deletion without loss of Photo state or Original content.

[Metadata](photo-library/metadata/design.md) owns the explicit XMP write boundary. It does not make Original Files writable or turn sidecars into a second Library decision store.

## Client Boundary

The Web and CLI use the same service and persistence owners. Ordinary review receives displayable derivatives and MUST NOT require transfer of RAW Originals to a client.

| Surface                                  | Contract owner                                       |
| ---------------------------------------- | ---------------------------------------------------- |
| Library Overview and scan state          | [Photo Library](photo-library/spec.md)               |
| Direct-child Original Folder navigation  | [Photo Organization](photo-library/design.md)        |
| Browse Snapshots and bounded windows     | [Library Browsing](library-browser/design.md)        |
| Photo decisions and saved Album position | [Library Browser](library-browser/spec.md)           |
| Album membership                         | [Photo Library](photo-library/spec.md)               |
| Derivatives and Preview readiness        | [Preview Pipeline](photo-library/previews/design.md) |
| Checked machine queries and writes       | [Command-Line Architecture](command-line/design.md)  |

Protocol values MUST NOT expose:

- database rows;
- native library objects;
- absolute Original File paths;
- internal errors.

Request or forwarded headers alone MUST NOT authenticate a caller. Original File paths MUST NOT become arbitrary download parameters.

## Module Boundaries

| Logical module   | Responsibility                                                                      |
| ---------------- | ----------------------------------------------------------------------------------- |
| Application      | Compose startup, shutdown, storage, queues, and server resources.                   |
| Library          | Own confinement, discovery, fingerprints, Location Recovery, and availability.      |
| Preview          | Own source selection, extraction, normalization, cache invalidation, and delivery.  |
| Browsing         | Own Library Overview, Browse Snapshots, bounded windows, and source navigation.     |
| Selection        | Own Selection State, Rating, and saved Album position.                              |
| Album            | Own virtual group identity, ordering, and membership.                               |
| Original Folders | Derive read-only navigation from Published Library Locations.                       |
| Protocol         | Own client-server schemas.                                                          |
| Web              | Own presentation and transient interactions.                                        |
| CLI              | Own command parsing, bounded service calls, structured results, and local delivery. |

These are ownership boundaries, not required packages or separately deployed services. Native process isolation is justified only by process safety or demonstrated resource contention.

## Technology Direction

[Rust Server Architecture](server.md) owns the production Rust boundary. Bun and TypeScript serve the Web application, browser tests, and repository tooling, not a second production server.

The Web uses ordinary image display and pointer or touch events. An established gesture library MAY be used when it reduces interaction complexity.

Native Preview extraction stays beside storage to avoid transferring large RAW files or duplicating per-device processing. Browsing uses camera-produced JPEG content; independent editing remains a separate capability.

Established libraries own image codecs and color transforms. Slipstream MUST NOT implement custom replacements for those libraries.

Selected technologies MUST support maintained library integration and bounded image-processing resources on supported deployment targets.

## Photo Development Extension

[Photo Workspace](photo-development/workspace/design.md) owns persistent directories and contained-file state. [Photo Development](photo-development/design.md) integrates independent tools and optional supervised execution into the service.

[Processing Modules](photo-development/processing/design.md) owns caller-controlled invocation without a workflow planner. [Development Color](photo-development/processing/color.md) owns processing image contracts independently of camera Preview selection.
