# Foundational Architecture

Slipstream needs the smallest server-owned application boundary that lets one Photographer review an existing Photo Library from a browser, group Photos, and retain selection decisions while Original Files remain unchanged.

## Design Drivers

- The primary interface is a browser on a phone, tablet, or desktop.
- Original Files may be large RAW files on local storage or a mounted network share.
- Browsers cannot display most RAW formats directly.
- The first product needs camera-produced Previews, not a general RAW development engine.
- The first product serves one Photographer and does not require accounts, collaboration, or Internet deployment.
- Selection changes must persist with low latency and survive server restart.
- File indexing and Preview work must not block gesture interaction.
- Original Files are irreplaceable and remain read-only.

Three kinds of state have separate owners:

1. The filesystem owns Original Files.
2. SQLite owns Photo identity, Photo Sets, Selection State, Rating, review progress, and derivative metadata.
3. The browser owns transient gesture, zoom, navigation, and one-level undo state.

No layer may become the only owner of another layer's state.

## Application Model

### Application Scope

One server process owns:

- configuration and application-data paths;
- SQLite initialization and migration;
- one configured Photo Library root;
- indexing and Preview work queues;
- the HTTP server and browser event transport;
- process shutdown.

Importing a module must not scan files, create storage, or bind a port.

The first deployment binds loopback by default and may explicitly bind a LAN interface so a phone or tablet on the same trusted network can connect. Public Internet exposure, accounts, and authorization are not part of the first architecture.

### Photo Library Scope

One Photo Library Scope owns:

- the canonical configured root;
- read-only path containment;
- supported-file discovery;
- deterministic RAW/JPEG pairing;
- Photo records and availability;
- Photo Set membership;
- Preview requests.

Every Original File read must resolve under the canonical root. Slipstream must reject traversal and symbolic-link escape. The Original File interface exposes no write or delete operation.

### Review Scope

A browser Review Session refers to one ordered Photo query and one current Photo. Filtered Library queries own Capture Time order, while Photo Set membership owns explicit Photo Set order. A Session snapshots the ordered Photo IDs when it starts. [`capture-time-ordering.md`](capture-time-ordering.md) defines metadata authority, rescan behavior, and deterministic ties.

Durable progress records only the last reviewed Photo for a Photo Set. Drag position, active animation, zoom, pan, and undo are browser-local.

The server is authoritative for Selection State and Rating. The browser must reconcile failed mutations instead of assuming an animation committed them.

### Indexing

Indexing discovers supported files and records lightweight facts needed for pairing and invalidation. It does not decode every RAW or generate every Preview before the Photo Library becomes usable.

A scan proceeds incrementally:

1. Walk paths below the configured root.
2. Classify recognized RAW and JPEG files.
3. Record relative path, size, modification time, and availability.
4. Pair an unambiguous same-directory, same-base-name RAW and JPEG.
5. Queue thumbnail work only as needed for visible browsing.

A failure for one file is recorded for that file and does not roll back successfully indexed files.

## Durable State

SQLite uses explicit mutable state. A generic event log is not required.

At minimum, SQLite stores:

- schema version;
- configured Photo Library identity and canonical runtime path;
- Original File relative path, kind, size, modification time, availability, and derived Capture Time inspection facts;
- Photo identity and RAW/JPEG references;
- Preview source, dimensions, cache revision, and failure state;
- Photo Set identity, name, order, and membership;
- Photo Selection State and Rating;
- per-Photo-Set last reviewed Photo.

Original bytes, matching JPEG bytes, and embedded RAW JPEG bytes do not belong in SQLite.

Selection State and Rating update in one database transaction per Photo mutation. A mutation may update that Photo Set's review progress in the same transaction. The browser holds one undo description containing the affected Photo, field, prior value, and expected current value; undo uses a compare-and-set transaction so it cannot overwrite a newer change. Photo Set deletion removes membership and progress, not Photo state or filesystem content.

The first architecture does not write XMP sidecars. Export or synchronization requires a later design because it introduces a second writable source of metadata.

## Local Web Boundary

The browser is a client of the server. It receives browser-displayable derivatives and never needs the RAW Original to perform ordinary review.

The server exposes the smallest required surfaces:

- queries for Photo Sets, ordered Photos, current Photo state, and Preview metadata;
- mutations for Photo Set membership, Selection State, Rating, and review progress;
- a rescan command;
- derivative responses for thumbnails and review Previews;
- transient scan and Preview-ready notifications when useful.

Protocol types must not expose database rows, native library objects, absolute Original File paths, or internal errors.

The initial browser mutation boundary assumes one Photographer on a trusted local network. It still must reject cross-origin mutations and must not expose Original File paths as arbitrary download parameters.

## Module Boundaries

The initial codebase has these logical modules:

- **Application** composes startup, shutdown, storage, queues, and server resources.
- **Library** owns root containment, discovery, pairing, and Photo availability.
- **Preview** owns source selection, embedded JPEG extraction, normalization, cache invalidation, and delivery.
- **Selection** owns Selection State, Rating, and review progress.
- **Photo Set** owns group identity, ordering, and membership.
- **Protocol** owns browser-server schemas.
- **Web** owns presentation, gestures, Detail Review, and one-level undo.

These are ownership boundaries, not required packages or services. The first implementation uses a modular monolith and one process unless native-library isolation proves necessary for process safety.

## Technology Direction

The production server is a Rust modular monolith. Rust owns HTTP, application lifecycle, SQLite, Photo Library indexing and confinement, Preview extraction, derivative caching, and durable mutations. Bun and TypeScript own the Web application, browser tests, and repository tooling; they are not a production server runtime. [`rust-server.md`](rust-server.md) defines the module and compatibility contracts.

The browser uses ordinary Web platform image display and pointer/touch events, with a small established gesture library only if it reduces tested interaction complexity.

LibRaw owns RAW container support and embedded JPEG extraction. It does not own a first-product RAW development path.

An established image library owns JPEG decode, orientation, resize, ICC preservation or conversion to sRGB, and derivative encoding. Slipstream must not build custom image codecs or color transforms.

The selected server and Web technologies must support direct maintained library integration, bounded image-processing resources, and the supported deployment targets. This design does not require a specific application framework.

## First Vertical Slice

The first implementation proves this complete path:

1. Start the server with one Photo Library directory.
2. Open the browser review surface.
3. Index a directory containing one RAW-only Photo and one RAW/JPEG Photo.
4. Create one Photo Set and add both Photos.
5. Display the matching JPEG for the paired Photo.
6. Extract and display the largest usable embedded JPEG for the RAW-only Photo.
7. Right-swipe one Photo to `selected` and left-swipe the other to `rejected`.
8. Set one Rating and undo one decision.
9. Restart the server and restore the Photo Set, decisions, Rating, and review progress.
10. Prove that Original File bytes and metadata are unchanged.

Everything else is deferred until this slice works.

## Options

### Selected: Server-Side Native Preview Extraction

The server reads RAW files through a native open-source library and sends JPEG derivatives to the browser. This keeps large RAW processing near storage, avoids mobile-browser memory pressure, and provides one deterministic extraction path.

### Rejected: Browser WebAssembly RAW Processing

Browser-side RAW processing would transfer large files to each device, duplicate caches, consume mobile memory and battery, and complicate color and format support. It adds no value when the Photo Library already resides beside the server.

### Selected: Embedded or Matching JPEG Only

Camera-produced JPEG content satisfies the current selection problem with less complexity and more predictable appearance than a generic RAW development pipeline.

### Rejected: General RAW Development Pipeline

Demosaicing, profiles, highlight recovery, lens correction, and tone processing expand the product into RAW editing. They are not required to judge most selection decisions and would create a large, ongoing camera-quality responsibility.

### Selected: Scoped Modular Monolith

One process keeps indexing, selection transactions, cache ownership, and lifecycle understandable. A separate Preview worker is justified only if native-library crashes or measured resource contention require isolation.

### Rejected: Independent Web, API, and Worker Services

Multiple deployable services add coordination, queues, health handling, and operational burden before one user and one Photo Library demonstrate a need.

## Verification

The first-slice gate must prove:

- configured-root containment, including traversal and symbolic-link escape rejection;
- indexing never writes, renames, moves, or deletes Original Files;
- deterministic unambiguous RAW/JPEG pairing;
- correct Preview Source order;
- embedded JPEG extraction for each supported sample camera;
- visible orientation and dimensions match the camera Preview;
- derivatives preserve a valid ICC profile or are correctly converted to sRGB;
- a low-resolution source is not upscaled or misrepresented;
- selection and Rating transactions survive restart;
- swipe actions do not fire while Detail Review is zoomed;
- failed mutations do not silently advance;
- cache invalidation follows source path, size, modification time, and derivative version;
- the complete path runs through the real browser-server protocol.

A camera sample corpus must include the Photographer's actual RAW formats. Unsupported targets and unavailable devices must be reported rather than inferred.
