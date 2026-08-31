# Preview Pipeline

Slipstream must turn large, mostly browser-incompatible Original Files into trustworthy JPEG Previews without owning a RAW development engine or changing the Photographer's files.

## Design Drivers

- A matching camera JPEG is the clearest available camera-produced representation.
- Most RAW files contain one or more embedded JPEG images, but dimensions and metadata vary by camera.
- Selection must begin before the whole Photo Library is processed.
- Mobile browsers should not download RAW files or perform RAW decoding.
- Detail Review must disclose the real resolution limit.
- Cache generation may fail or be interrupted without damaging a valid prior derivative.
- The implementation should integrate maintained open-source libraries, not shell out to user-facing CLI tools.

## Model

### Source Candidate

A Source Candidate is JPEG content that may represent a Photo:

- the matching JPEG Original;
- one JPEG embedded in the RAW Original.

Candidates are ordered by product rules in [Photo Previews](../docs/previews.md). Embedded candidates within one RAW are ordered by pixel area, largest first.

### Source Inspection

Source Inspection determines:

- whether JPEG bytes can be obtained;
- encoded width and height;
- visible orientation;
- ICC profile presence and validity;
- byte size;
- extraction or decode failure.

Inspection must not apply creative processing.

### Derivative

A Derivative is an immutable cached JPEG produced for a target class:

- `thumbnail-512`;
- `review-2560`.

A Derivative records the selected source, actual dimensions, color handling, and cache revision. It does not become an Original File. Completed Derivatives persist in the configured cache directory across browser reload and server restart until an operator removes rebuildable cache data.

### Preview Capability

Preview Capability describes what the selected source can support:

- `normal-review` when it can reasonably fill Photo View;
- `detail-limited` when magnification reaches source pixels before useful focus inspection;
- `unavailable` when no allowed source can be decoded.

The first implementation may derive `detail-limited` from actual source and viewport dimensions. It must not invent a universal camera-independent quality score.

## Semantics

### Source Selection

For a Photo with a matching JPEG Original, the Preview module first inspects that JPEG. If it is valid, it is selected.

If the matching JPEG is absent or invalid and a RAW Original exists, the module asks LibRaw for embedded JPEG candidates and selects the largest valid candidate.

If one embedded candidate fails to extract or decode, the module may try the next smaller candidate.

No sensor-data unpacking, demosaicing, camera-profile rendering, or generic fallback is part of this pipeline.

### Native Library Boundary

LibRaw is the authoritative RAW container and embedded-preview extraction dependency. Slipstream uses a narrow owned C/C++ wrapper around LibRaw and libjpeg. The wrapper accepts a borrowed descriptor adapter, enumerates embedded JPEG candidates, validates complete JPEG bytes, returns bounded fixed-width results, and releases all native resources on success, failure, or cancellation. Rust retains descriptor ownership and performs the same-descriptor post-operation revision check. The wrapper must not expose LibRaw structs or native allocation ownership across the Preview module boundary.

A second narrow owned C wrapper around libvips owns JPEG inspection, orientation, bounded resize, ICC handling, and encoding. It receives bytes or a confined file handle, not an unconstrained browser path, and does not expose `VipsImage`, GLib references, or variadic option ownership to Rust. libvips is initialized once per process before Preview work and remains initialized until process exit; requests never initialize or shut down the library. The process-global lifecycle is not tied to an individual Library or PreviewService.

Preview processing is bounded by 128 MiB input JPEG bytes, 100 million decoded pixels, 64 MiB output JPEG bytes, and 256 MiB LibRaw native memory. The Preview queue defaults to two concurrent jobs per cache directory; inspection and generation consume the same bounded budget.

### Orientation

The output Derivative must display in the same visible orientation as the selected source under normal browser rendering.

The Rust libvips implementation bakes orientation into output pixels and removes orientation metadata. It must use this one tested rule consistently and must not rotate twice.

### Color

When a valid source ICC profile exists, the pipeline preserves it if the output path can preserve it correctly. Otherwise it converts pixels to sRGB and embeds or identifies sRGB correctly.

When no source profile exists, the pipeline treats source RGB as sRGB. This is an interoperability default, not a claim about unknown source colorimetry.

The pipeline must never copy an invalid or mismatched profile into the derivative.

### Resize

The pipeline preserves aspect ratio and never enlarges the source.

For each target class:

```text literal
scale = min(1, target_long_edge / source_long_edge)
```

The encoder applies no creative adjustment. Ordinary resize filtering and encoder behavior are allowed; exposure, contrast, saturation, sharpening, denoising, and lens correction are not.

### Cache Identity

A Derivative cache identity includes:

- Photo identity;
- selected Original Location;
- selected Original File size;
- selected Original File modification time;
- embedded-candidate identity when the source is inside RAW;
- target class;
- pipeline version.

The first implementation does not hash whole RAW files. A changed size or modification time invalidates dependent derivatives.

Derivative creation writes to a temporary cache path and publishes the completed file atomically within the cache filesystem. A failed replacement leaves the previous valid Derivative available but stale; the protocol must identify whether a stale result is being shown.

### Scheduling

Preview work is demand-driven.

Priority order is:

1. the current Photo in Photo View;
2. the immediately next and previous Photos after the current Preview is ready;
3. visible Grid View Photos;
4. bounded Grid look-ahead work explicitly justified by measured benefit.

A scan does not automatically generate every `review-2560` Derivative. Duplicate requests for one cache identity share one in-flight job.

Leaving a Photo does not require cancelling extraction if completion is near and the result remains reusable. The scheduler may cancel queued work that has no remaining consumer.

### Delivery

The browser requests a derivative by Photo identity and target class. It does not provide an Original File path.

A current server-cache hit reads completed derivative bytes without reopening or reprocessing the Original File. Currentness is evaluated against the Published Library's selected source facts: candidate, Original Location, size, and modification time. Rescan is the invalidation boundary for those facts. Preview requests validate both the manifest and complete derivative bytes against that published bundle before returning a current result. Browse Window URL hydration may use only the bounded manifest, key, source-fact, and file-metadata checks; the subsequent Preview or derivative request remains the correctness-critical byte-validation boundary.

A derivative response uses ordinary HTTP cache validation tied to the cache identity, including an immutable identity-bearing URL and `ETag`. Reconnect and reload may reuse browser-cached data when the identity remains current.

The response also makes Preview Source, actual dimensions, and Preview Capability available to Photo View. These facts must not be inferred only from the derivative URL.

## Failure Behavior

A native extraction failure is bounded to the affected Photo and candidate. The worker releases native memory and records an actionable internal error without exposing absolute paths or native stack data to the browser.

If all candidates fail, Preview state becomes `unavailable`. The scheduler must not retry continuously. A source change, explicit rescan, or explicit retry may make it eligible again.

If the server stops during generation, a temporary file is not considered a valid Derivative. Startup or later cache maintenance may remove abandoned temporary files.

A malformed or adversarial file must not cause unbounded allocation based only on claimed dimensions. The implementation must enforce explicit input, pixel, output, memory, and concurrency limits.

## Options

### Selected: Owned LibRaw/libjpeg and libvips Wrappers

This combination has a small responsibility split. The owned LibRaw/libjpeg wrapper knows RAW containers, embedded images, and complete JPEG validation. The owned libvips wrapper knows JPEG inspection, orientation, resize, profiles, and encoding. Slipstream owns source order, limits, process-global library lifecycle, caching, and product semantics.

### Rejected: LibRaw Basic RAW Conversion

LibRaw's sensor-data conversion would create a new generic appearance that is less representative of the camera preview and introduces RAW-development policy. It is not a useful fallback for the first product.

### Rejected: Exif-Only Thumbnail Extraction

Metadata libraries can expose Preview offsets for many formats, but RAW container behavior and camera support vary. Using LibRaw as the extraction authority keeps this responsibility with the specialized library. A metadata library may still read ordinary EXIF later if product requirements justify it.

### Rejected: Returning Embedded JPEG Bytes Without Normalization

Direct return is attractive, but camera JPEG metadata, orientation, profiles, very large dimensions, and browser behavior vary. A small normalization step provides predictable delivery and bounded resource use. The original embedded bytes may later be an explicit Detail Review optimization after compatibility evidence.

### Rejected: Precompute Every Derivative During Indexing

Full precomputation delays first use and performs expensive I/O for Photos the Photographer may never review. Demand-driven generation serves current browsing and selection with less work.

## Verification

The checked-in minimal fixture contract must cover generated, redistributable JPEG inputs for orientation values 1 through 8, small and large dimensions, unprofiled RGB, valid RGB ICC, invalid ICC, CMYK conversion, corrupt and truncated input, and both derivative targets. The contract is executable by the compatibility tests and is the evidence gate for selecting a Rust pipeline algorithm version. It must not contain real photographs or generated binaries.

Implementation tests must prove:

- a matching JPEG wins over an embedded RAW JPEG;
- an invalid matching JPEG falls back to the largest valid embedded JPEG;
- embedded candidates are ordered by actual dimensions;
- no code path unpacks RAW sensor pixels;
- portrait and rotated samples display exactly once in the correct orientation;
- output never exceeds source dimensions or target long edge;
- valid profile preservation and sRGB conversion behave as specified;
- source changes invalidate the cache;
- concurrent duplicate requests perform one generation job;
- completed thumbnail and review Derivatives survive process restart and reuse cache bytes without reopening the Original;
- current work outranks adjacent and Grid work, while adjacent prefetch remains bounded;
- interrupted generation does not publish a partial file;
- malformed inputs remain within resource limits;
- failures stay isolated to one Photo;
- actual camera samples provide sufficient normal-review and Detail Review dimensions.

Tests should compare decoded output properties and representative pixels where stable. They must not use compressed JPEG byte equality as a visual correctness assertion.
