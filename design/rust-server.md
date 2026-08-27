# Rust Server Architecture

Slipstream needs one production server that keeps Original Files read-only while it owns indexing, durable review state, Preview generation, and browser delivery. The production server is Rust. Bun and TypeScript own the Web application, browser tests, and repository tooling only.

This decision corrects an earlier language-boundary drift. Selecting Bun for Web tooling did not select TypeScript as the production server language.

## Design Drivers

- Original Files are irreplaceable and require descriptor-confined access.
- Existing SQLite review state and browser behavior must survive the migration.
- LibRaw and image processing are blocking native work and must remain bounded.
- One Photographer and one Photo Library do not justify distributed services, an ORM, or an actor framework.
- The migration must remain reversible until the Rust service passes the existing production browser gate.

## Ownership Model

The Rust service is one modular monolith with these boundaries:

- **Application and HTTP** own configuration, startup order, request limits, protocol mapping, static Web delivery, readiness, and graceful shutdown.
- **Library and Confinement** own the canonical Library descriptor, deterministic traversal, Original capabilities, pairing, identity, and revision facts. Paths do not confer authority.
- **Persistence** owns one SQLite connection on one dedicated thread with a bounded typed command queue. It owns schema validation, migration, sidecar admission, transactions, and durable state.
- **Preview and Native** own bounded matching-JPEG reads and a narrow C/C++ LibRaw plus libjpeg shim. The shim accepts an already-confined descriptor adapter, enumerates embedded JPEG candidates, fully validates JPEG bytes, and exposes no sensor unpack or RAW development operation.
- **Derivative and Cache** own orientation, color, resize, encoding, scheduling, identity, atomic publication, stale fallback, and immutable delivery facts.

The modules exchange domain values and typed failures. HTTP types do not enter Persistence or Native modules. Native error text and filesystem paths do not cross the protocol boundary.

## Lifecycle

Startup proceeds in one direction:

1. Parse and validate configuration.
2. Open the canonical Library, state, and cache directories.
3. Admit and open SQLite, validate or migrate it, and start its bounded owner thread.
4. Complete the initial scan.
5. Start bounded Preview workers.
6. Bind HTTP and report readiness.

The opt-in Rust server exposes `GET /healthz` for deployment health checks. It returns `200` with the exact path-free JSON body `{"status":"ok"}` only after the initial scan, Preview worker startup, and HTTP bind have completed. Startup failures never expose a ready listener. Shutdown stops HTTP admission before closing Preview and Library resources.

A failure closes resources in reverse order. Shutdown stops admission, drains already accepted mutations, stops Preview publication, closes SQLite, and then completes. Repeated shutdown requests share one completion path.

SQLite startup accepts the configured `DELETE` journal policy only from a sidecar-free state. If a journal, WAL, or shared-memory sidecar remains after another process or an unclean stop, startup must return a recovery-required failure before opening SQLite and must leave the database and every sidecar unchanged. Recovery uses an operator-controlled copy rather than letting startup checkpoint or rewrite state whose schema and Library binding may exist only in WAL.

Blocking SQLite, LibRaw, JPEG, and derivative work must not run on asynchronous HTTP executor threads. Queue saturation is explicit backpressure, not unbounded memory growth.

## Compatibility and Cutover

The checked-in files under [`../compatibility/`](../compatibility/) are the migration authority for deterministic identities, JSON omission behavior, startup configuration, and canonical SQLite v2 shape. Existing TypeScript tests and Rust tests consume the same vectors. The real Playwright suite remains the final browser authority.

During migration:

- the TypeScript server remains behaviorally frozen and rollback-capable;
- Rust slices use copied state, cache, and generated Original fixtures;
- the implementations must not write one SQLite database concurrently;
- HTTP responses, SQLite v2, cache records, and deterministic identities remain compatible unless a later Design Spec defines a lossless transition;
- cutover occurs only after Rust passes protocol, migration, security, Preview, browser, and real-sample safety gates;
- Docker switches the process while preserving bind-mounted state and cache;
- rollback restores the prior process against the same verified schema;
- the Node server and Node-API addon are removed only after cutover and rollback verification.

Golden JSON and SQL fixtures are the source of truth during migration. Speculative shared code generation is rejected because the current protocol is small and generated bindings would create another build and compatibility boundary before demonstrated duplication.

## Selected Technology Direction

### HTTP: Axum and Tokio

Axum `0.8.9` and Tokio `1.53.1` are pinned for the compatibility crate. They compile on the pinned Rust `1.97.1` toolchain with narrow features. Axum maps directly to the service-owned Tokio lifecycle and ordinary module state.

Actix Web `4.15.0` was considered. It supports the required HTTP surface, but its additional server/runtime vocabulary and worker model do not remove a current Slipstream boundary. It remains rejected unless a later exact protocol probe demonstrates substantially safer request limiting, file delivery, or shutdown with less code.

### SQLite: rusqlite with bundled SQLite

rusqlite `0.40.2` is pinned with `bundled`. The probe executes the canonical v2 schema and reports a working SQLite runtime without depending on a deployment host's SQLite version or compile options. One dedicated bounded owner thread preserves the current serialization and direct `BEGIN IMMEDIATE` control.

sqlx `0.9.0` was considered. Its pool, async facade, macro/offline metadata, and generic migration layer do not replace Slipstream's exact schema-shape validation and sidecar admission. It is rejected until concurrent connections or cross-database support become measured requirements.

### Original confinement and LibRaw

Rust owns Linux `openat2` confinement and same-descriptor pre/post `fstat` revision checks. A narrow owned C/C++ wrapper built with `cc` owns LibRaw and libjpeg error handling. Its production ABI uses opaque handles, fixed-width values, owned byte buffers, and normalized outcomes. Broad generated LibRaw bindings and `libraw-sys` are rejected because the available crate does not prove the required thumbnail enumeration, fd boundary, memory policy, or maintenance contract.

The wrapper must expose only embedded-JPEG extraction and complete JPEG validation. Node-API, whole-RAW buffering, sensor unpack, demosaic, and general RAW development are outside the boundary. The wrapper receives a borrowed descriptor adapter; Rust retains descriptor ownership and performs the post-operation revision check.

### Derivative image and color processing

The final runtime must not depend on Sharp or Node. Reusing Sharp during migration is rejected because it would preserve the unintended runtime boundary.

The selected implementation is a narrow owned C wrapper around libvips, with LittleCMS used where an explicit profile transform is required. The wrapper owns the libvips object graph, translates failures to bounded Rust outcomes, and exposes only JPEG inspection, orientation normalization, bounded resize, profile handling, and JPEG encoding. It must not expose libvips objects or GLib ownership conventions to the rest of the Rust application.

libvips initialization is process-global: the first Preview operation calls `vips_init` once, and libvips remains initialized until process exit. Individual requests, Libraries, and Preview services must not call `vips_shutdown`. The wrapper must use libvips' bounded operation options and the application owns the Preview queue with a default concurrency of 2 jobs per cache directory. Input JPEG bytes are capped at 128 MiB, decoded pixels at 100 million, output JPEG bytes at 64 MiB, and LibRaw native memory at 256 MiB.

`image 0.25.10` with `lcms2 6.1.1` remains a probe-only alternative. It compiles and proves basic Lanczos and sRGB primitives, but it is not selected unless the complete Issue #22 fixture matrix passes orientation 1–8, ICC, CMYK, corruption, memory, and representative pixel checks within the resource budget. `image` alone is rejected.

The Rust derivative uses cache algorithm version `rust-vips-v1`; semantic parity with the existing Sharp implementation is not yet proven, so `sharp-v2` derivatives are rebuildable stale entries rather than byte-compatible outputs. A future retention of `sharp-v2` requires the checked-in fixture contract to prove orientation, dimensions, representative decoded pixels, ICC classification, and manifest behavior.

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

The migration preserves:

- the current HTTP routes, statuses, path-free JSON, same-origin mutation rule, 16 KiB decoded header bound, and 64 KiB streamed mutation-body bound;
- strong derivative ETags derived from cache identity, immutable derivative caching, revalidatable `index.html`, and no API-to-SPA fallback;
- SQLite schema version 2, canonical-root binding, exact migration rejection, `foreign_keys=ON`, fixed journal policy, admitted sidecars, and one admitted `BEGIN IMMEDIATE` transaction per write;
- Original and Photo SHA-256 identities, source revision text, and cache/manifest identity serialization, including Unicode and fractional modification times;
- matching JPEG before largest usable embedded RAW JPEG;
- descriptor confinement, resource limits, atomic cache publication, truthful stale source, and Original zero mutation;
- required absolute startup paths, loopback default, startup cleanup, signals, and idempotent close.

Implementation details may improve standards compliance, such as parsing an `If-None-Match` list, only when the old accepted behavior remains accepted and executable compatibility tests define the change.

## Verification

Every migration slice must run the shared compatibility crate and TypeScript vector verifier. The full gate includes Rust formatting, Clippy with warnings denied, Rust tests/build, existing Bun checks, native tests, and real Chromium tests.

The checked-in compatibility suite covers representative v0/v1 migration success and rejection rollback, exact v2 schema shape, request/status/body/header vectors, derivative ETag revalidation, immutable delivery, index revalidation, and API no-SPA-fallback behavior. Before production cutover, verification must additionally cover copied production SQLite cross-open and rollback, Linux traversal and inode attacks, every exact HTTP body/header boundary, bind and shutdown failures, cache cross-read, all eight EXIF orientations, ICC conversion vectors, concurrency and memory limits, and the configured Sony sample with unchanged Original hash.
