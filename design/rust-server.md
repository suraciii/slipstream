# Rust Server Architecture

Slipstream needs one production server that keeps Original Files read-only while it owns indexing, durable review state, Preview generation, and browser delivery. The production server is Rust. Bun and TypeScript own the Web application, browser tests, and repository tooling only.

This decision corrects an earlier language-boundary drift. Selecting Bun for Web tooling did not select TypeScript as the production server language.

## Design Drivers

- Original Files are irreplaceable and require descriptor-confined access.
- Existing SQLite review state and browser behavior must remain stable.
- LibRaw and image processing are blocking native work and must remain bounded.
- One Photographer and one Photo Library do not justify distributed services, an ORM, or an actor framework.
- The service must support operator-controlled restart and rollback without modifying Original Files.

## Ownership Model

The Rust service is one modular monolith with these boundaries:

- **Application and HTTP** own configuration, startup order, request limits, protocol mapping, static Web delivery, readiness, and graceful shutdown.
- **Library and Confinement** own the current Library Folder descriptor, deterministic traversal, Original capabilities, stable persisted identity, Original Locations, pairing, and revision facts. Paths do not confer authority or identity.
- **Persistence** owns one SQLite connection on one dedicated thread with a bounded typed command queue. It owns schema validation, migration, sidecar admission, transactions, and durable state.
- **Preview and Native** own bounded matching-JPEG reads and a narrow C/C++ LibRaw plus libjpeg shim. The shim accepts an already-confined descriptor adapter, enumerates embedded JPEG candidates, fully validates JPEG bytes, and exposes no sensor unpack or RAW development operation.
- **Derivative and Cache** own orientation, color, resize, encoding, scheduling, identity, atomic publication, stale fallback, and immutable delivery facts.

The modules exchange domain values and typed failures. HTTP types do not enter Persistence or Native modules. Native error text and filesystem paths do not cross the protocol boundary.

## Lifecycle

Startup proceeds in one direction:

1. Parse and validate configuration.
2. Open the canonical Library Folder, state directory, and cache directory.
3. Admit and open SQLite, validate or migrate it, and start its bounded owner thread.
4. Complete the initial scan, including required Capture Time inspection or schema-v2 backfill for available Originals.
5. Start bounded Preview workers.
6. Bind HTTP and report readiness.

The Rust server exposes `GET /healthz` for deployment health checks. It returns `200` with the exact path-free JSON body `{"status":"ok"}` only after the initial scan, Preview worker startup, and HTTP bind have completed. Startup failures never expose a ready listener. Shutdown stops HTTP admission before closing Preview and Library resources.

A failure closes resources in reverse order. Shutdown stops admission, drains already accepted mutations, stops Preview publication, closes SQLite, and then completes. Repeated shutdown requests share one completion path.

SQLite startup accepts the configured `DELETE` journal policy only from a sidecar-free state. If a journal, WAL, or shared-memory sidecar remains after another process or an unclean stop, startup must return a recovery-required failure before opening SQLite and must leave the database and every sidecar unchanged. Recovery uses an operator-controlled copy rather than letting startup checkpoint or rewrite state whose schema and Library binding may exist only in WAL.

Blocking SQLite, LibRaw, JPEG, and derivative work must not run on asynchronous HTTP executor threads. Queue saturation is explicit backpressure, not unbounded memory growth.

## Compatibility

The checked-in files under [`../compatibility/`](../compatibility/) are the authority for JSON omission behavior, startup configuration, canonical SQLite migration inputs and shapes, Capture Time vectors, ordering examples, and legacy deterministic IDs that must survive migration. Rust compatibility tests consume these vectors, and the real Playwright suite remains the final browser authority.

HTTP response shapes, SQLite v2 and v3 migration inputs, cache records, and existing persisted IDs remain compatible. Deterministic v3 identity vectors define preserved legacy values, not the allocator for new v4 records. SQLite v4 is the writable identity fence defined by [Photo Library Identity and Expansion](library-identity.md); older binaries reject it. Docker preserves bind-mounted state and cache while running the Rust service. The Rust service and Web application are the only production paths; Bun and TypeScript remain limited to Web, browser tests, and repository tooling.

Golden JSON and SQL fixtures are the source of truth. Speculative shared code generation is rejected because the current protocol is small and generated bindings would create another build and compatibility boundary before demonstrated duplication.

## Selected Technology Direction

### HTTP: Axum and Tokio

Axum `0.8.9` and Tokio `1.53.1` are pinned for the compatibility crate. They compile on the pinned Rust `1.97.1` toolchain with narrow features. Axum maps directly to the service-owned Tokio lifecycle and ordinary module state.

Actix Web `4.15.0` was considered. It supports the required HTTP surface, but its additional server/runtime vocabulary and worker model do not remove a current Slipstream boundary. It remains rejected unless a later exact protocol probe demonstrates substantially safer request limiting, file delivery, or shutdown with less code.

### SQLite: rusqlite with bundled SQLite

rusqlite `0.40.2` is pinned with `bundled`. The probe executes the canonical v4 schema and reports a working SQLite runtime without depending on a deployment host's SQLite version or compile options. One dedicated bounded owner thread preserves the current serialization and direct `BEGIN IMMEDIATE` control.

sqlx `0.9.0` was considered. Its pool, async facade, macro/offline metadata, and generic migration layer do not replace Slipstream's exact schema-shape validation and sidecar admission. It is rejected until concurrent connections or cross-database support become measured requirements.

### Original confinement and LibRaw

Rust owns Linux `openat2` confinement and same-descriptor pre/post `fstat` revision checks. A narrow owned C/C++ wrapper built with `cc` owns LibRaw and libjpeg error handling. Its production ABI uses opaque handles, fixed-width values, owned byte buffers, and normalized outcomes. Broad generated LibRaw bindings and `libraw-sys` are rejected because the available crate does not prove the required thumbnail enumeration, fd boundary, memory policy, or maintenance contract.

The wrapper must expose only embedded-JPEG extraction and complete JPEG validation. Node-API, whole-RAW buffering, sensor unpack, demosaic, and general RAW development are outside the boundary. The wrapper receives a borrowed descriptor adapter; Rust retains descriptor ownership and performs the post-operation revision check.

### Derivative image and color processing

The final runtime does not depend on Sharp or Node. Bun and TypeScript are limited to the Web build, browser tests, and repository tooling.

The selected implementation is a narrow owned C wrapper around libvips, with LittleCMS used where an explicit profile transform is required. The wrapper owns the libvips object graph, translates failures to bounded Rust outcomes, and exposes only JPEG inspection, orientation normalization, bounded resize, profile handling, and JPEG encoding. It must not expose libvips objects or GLib ownership conventions to the rest of the Rust application.

libvips initialization is process-global: the first Preview operation calls `vips_init` once, and libvips remains initialized until process exit. Individual requests, Libraries, and Preview services must not call `vips_shutdown`. The wrapper must use libvips' bounded operation options and the application owns the Preview queue with a default concurrency of 2 jobs per cache directory. Input JPEG bytes are capped at 128 MiB, decoded pixels at 100 million, output JPEG bytes at 64 MiB, and LibRaw native memory at 256 MiB.

`image 0.25.10` with `lcms2 6.1.1` remains a probe-only alternative. It compiles and proves basic Lanczos and sRGB primitives, but it is not selected unless the complete Issue #22 fixture matrix passes orientation 1–8, ICC, CMYK, corruption, memory, and representative pixel checks within the resource budget. `image` alone is rejected.

The Rust derivative uses cache algorithm version `rust-vips-v1`. Cache entries from earlier implementations are not byte-compatible outputs and are rebuildable stale entries. The checked-in fixture contract proves orientation, dimensions, representative decoded pixels, ICC classification, and manifest behavior for the selected implementation. The identity vectors retain their historical `sharp-v2` algorithm label because it is part of the deterministic migration authority; it is not a runtime dependency, and new derivative records use `rust-vips-v1`.

## Exact Dependency Evidence

The compatibility workspace pins:

- Rust `1.97.1`;
- Axum `0.8.9` with `http1`, `json`, and `tokio`;
- Tokio `1.53.1` with runtime, macro, network, signal, and synchronization features;
- rusqlite `0.40.2` with bundled SQLite;
- `cc 1.4.4` and `pkg-config 0.3.32` for the native probe;
- `image 0.25.10` with JPEG only and no default formats or Rayon;
- `lcms2 6.1.1` with dynamic system linking.

`cargo info` provided crate metadata and repository URLs for the selected and rejected candidates. These results and a successful locked compile are selection evidence, not a guarantee of future maintenance. Security advisory and upstream activity review remains part of each delivery Issue when dependencies change.

The compatibility probe links system LibRaw `0.21.5`, libjpeg-compatible API `2.1.5`, libvips `8.18.0`, and LittleCMS `2.17`. These versions prove that the selected native headers and process-global libvips lifecycle are available in the build environment; they do not by themselves establish derivative parity. The checked-in Preview fixture contract and later Rust implementation tests are the algorithm-version evidence gate.

## Compatibility Contracts

The service preserves:

- the current HTTP routes, statuses, path-free JSON, same-origin mutation rule, 16 KiB decoded header bound, and 64 KiB streamed mutation-body bound;
- strong derivative ETags derived from cache identity, immutable derivative caching, revalidatable `index.html`, and no API-to-SPA fallback;
- canonical SQLite schema validation, the lossless v2-to-v3 migration history, canonical v3-to-v4 identity migration, fail-closed Library Folder admission, exact migration rejection, `foreign_keys=ON`, fixed journal policy, admitted sidecars, and one admitted `BEGIN IMMEDIATE` transaction per write;
- the explicit ancestor-expansion transaction defined by [Photo Library Identity and Expansion](library-identity.md), with canonical v3 as a preserved migration input and v4 as the required writable identity fence;
- exact preservation of existing Original File and Photo IDs as opaque values across migration and Library expansion, without recomputing them from the current Original Location;
- Location-independent, state-store-unique allocation for every new Original File and Photo ID under v4;
- source revision text and cache/manifest identity serialization, including Unicode and fractional modification times;
- matching JPEG before largest usable embedded RAW JPEG;
- descriptor confinement, resource limits, atomic cache publication, truthful stale source, and Original zero mutation;
- required absolute startup paths, loopback default, startup cleanup, signals, and idempotent close.

Implementation details may improve standards compliance, such as parsing an `If-None-Match` list, only when the accepted behavior remains compatible and executable compatibility tests define the change.

## Verification

The verification gate runs the shared compatibility crate, Rust formatting, Clippy with warnings denied, Rust tests/build, Bun Web checks, and real Chromium browser tests against the Rust server.

The checked-in compatibility suite covers representative v0/v1/v2 state migration success and rejection rollback, exact v3 and v4 schema shapes, legacy-ID preservation, Location-independent new-ID allocation, Capture Time parsing and deterministic ordering, request/status/body/header vectors, derivative ETag revalidation, immutable delivery, index revalidation, and API no-SPA-fallback behavior. The full gate also covers Linux traversal and inode attacks, every exact HTTP body/header boundary, bind and shutdown failures, cache cross-read, all eight EXIF orientations, ICC conversion vectors, concurrency and memory limits, browser review behavior, and the configured Sony sample with unchanged Original hash.
