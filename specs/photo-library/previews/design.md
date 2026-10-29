# Preview Pipeline

Slipstream MUST turn large, mostly browser-incompatible Original Files into trustworthy JPEG Previews without owning a RAW development engine or changing the Photographer's files.

[Photo Library Design](../design.md) owns Original and Photo ownership; [Photo Previews](spec.md) owns observable Preview behavior.

## Design Drivers

- A JPEG Original's own content is the clearest available camera-produced representation.
- Most RAW files contain one or more embedded JPEG images, but dimensions and metadata vary by camera.
- Selection MUST begin before the whole Photo Library is processed.
- Mobile browsers SHOULD NOT download RAW files or perform RAW decoding.
- Magnified inspection MUST disclose the real resolution limit.
- Cache generation can fail or be interrupted without damaging a valid prior derivative.
- The implementation SHOULD integrate maintained open-source libraries, not shell out to user-facing CLI tools.

## Model

### Source Candidate

A Source Candidate is JPEG content that can represent a Photo:

- the Photo's own JPEG Original content; or
- one JPEG embedded in the Photo's own RAW Original.

A Photo's candidates never include another Original File's content. Candidates are ordered by product rules in [Photo Previews](spec.md). Embedded candidates within one RAW are ordered by pixel area, largest first.

### Preview Source State

Each Photo's allowed Preview Source follows from its own Original kind: a JPEG Original is its own source (`jpeg-original`), and a RAW Original is its own embedded-JPEG source (`raw-embedded-jpeg`).

A Photo has exactly one allowed source. A sibling file never provides a second candidate.

One persisted inspection state belongs to each Photo:

- `inspection-pending` when no completed inspection exists for the current source revision;
- `ready` when a usable source produced a current Derivative;
- `failed` for a transient service or native failure that MAY be retried; and
- `unavailable` when no allowed source is usable for the current revision.

A Preview fact is seeded only against the source revision it was produced from. A seed whose expected source revision no longer matches is ignored.

A changed source revision returns the Photo to `inspection-pending`. A relocation invalidates the location-derived derivative identity even when the bytes are equal.

### Source Inspection

Source Inspection determines:

- whether JPEG bytes can be obtained;
- encoded width and height;
- visible orientation;
- ICC profile presence and validity;
- byte size;
- extraction or decode failure.

A RAW Source Candidate also carries an optional container orientation from the same LibRaw inspection that produced its JPEG bytes. This is fallback metadata, not a second transform or another source.

It remains paired with that retained Original descriptor and its post-operation revision check. It is not persisted as Photo identity or exposed as a new browser rotation control.

Inspection MUST NOT apply creative processing.

### Derivative

A Derivative is an immutable cached JPEG produced for a target class:

- `thumbnail-512`;
- `review-2560`.

A Derivative records the selected source, actual dimensions, color handling, and cache revision. It does not become an Original File.

Completed Derivatives persist in the configured cache directory across browser reload and server restart until an operator removes rebuildable cache data.

### Preview Capability

Preview Capability describes what the selected source can support:

- `normal-review` when it can reasonably fill Photo View;
- `detail-limited` when magnification reaches source pixels before useful focus inspection;
- `unavailable` when no allowed source can be decoded.

Preview Capability is a source fact derived only from the delivered derivative's width and height against its review target class. `review-2560` normally means `normal-review`; dimensions below the target long edge mean `detail-limited`.

The review target class is the target the delivered derivative was produced for, not the Photographer's chosen zoom.

Client viewport, Preview-area geometry, window size, device pixel ratio, and zoom state MUST NOT participate in the derivation. The same Photo reports the same capability to the browser, CLI, and Agent.

The capability MUST NOT invent a universal camera-independent quality score.

A Preview that fills a small Preview area at Fit remains `detail-limited` when its pixels are below the review target. A large Preview is not `detail-limited` merely because the current window is small.

### Web Display Condition

The Web display condition is a presentation fact owned by the Preview and zoom presentation boundary. It is true when the measured image pixels, Preview-area geometry, and zoom state require enlarging source pixels to fill the area.

The display condition is distinct from Preview Capability. It is derived from browser-measured values and can change when the Photographer resizes the window or zooms.

The display condition says nothing about the Photo's own source. It MUST NOT be persisted, sent on the wire, or change a Photo fact or Preview Capability.

A surface needing the capability obtains it from the Preview response. A surface needing the display condition measures its own rendering.

The Web MUST recompute the display condition on Preview load, Preview-area resize, Fit recomputation, manual, wheel, and pinch zoom, and return to Fit.

Where limited detail is explained, the Web accessibility tree MUST expose one combined explanation naming both the source capability and the current display condition.

## Semantics

### Source Selection

[Preview Source Order](spec.md#preview-source-order) owns source selection and fallback. Embedded candidates are ordered by actual pixel area, largest first.

The native boundary MUST NOT unpack sensor data, demosaic, render camera profiles, or offer generic RAW development.

### Native Library Boundary

LibRaw is the authoritative RAW container and embedded-preview extraction dependency. A narrow owned C/C++ wrapper around LibRaw and libjpeg accepts a borrowed descriptor adapter and enumerates embedded JPEG candidates.

The wrapper MUST fully validate JPEG bytes and return bounded fixed-width results with normalized outcomes. It MUST release all native resources on success, failure, or cancellation.

Rust MUST retain descriptor ownership and perform same-descriptor pre-operation and post-operation `fstat` revision checks. The wrapper MUST NOT expose LibRaw structs or native allocation ownership across the Preview module boundary.

The wrapper MUST expose only embedded-JPEG extraction and complete JPEG validation. Whole-RAW buffering and Node-API are outside this boundary.

A second narrow owned C wrapper around libvips owns JPEG inspection, orientation, bounded resize, ICC handling, and encoding. It receives bytes or a confined file handle, not an unconstrained browser path.

The wrapper MUST NOT expose `VipsImage`, GLib references, or variadic option ownership to Rust.

libvips is initialized once per process before Preview work and remains initialized until process exit. Requests never initialize or shut down the library; its process-global lifecycle is not tied to an individual Library or PreviewService.

Preview processing is bounded by:

- 128 MiB input JPEG bytes;
- 100 million decoded pixels;
- 64 MiB output JPEG bytes;
- 256 MiB LibRaw native memory.

The Preview queue defaults to two concurrent jobs per cache directory. Inspection and generation consume the same bounded budget.

### Orientation

[Preview Normalization](spec.md#preview-normalization) owns
orientation precedence and the behavior when neither source has a valid
orientation.

The RAW fallback authority is `libraw_data_t.sizes.flip`, captured after a
successful metadata open and before thumbnail extraction on the same retained
descriptor. Slipstream consumes LibRaw's decoded container transform rather than
independently parsing RAW orientation tags. The wrapper converts its recognized
bit-field values to standard EXIF orientation:

- `0` becomes `1` (normal);
- `1` becomes `2` (horizontal reflection);
- `2` becomes `4` (vertical reflection);
- `3` becomes `3` (180-degree rotation);
- `4` becomes `5` (transpose);
- `5` becomes `8` (90-degree counterclockwise rotation);
- `6` becomes `6` (90-degree clockwise rotation);
- `7` becomes `7` (transverse reflection).

Any other LibRaw value supplies no fallback. LibRaw can default absent orientation to `0`, which leaves encoded pixel order unchanged.

This mapping inverts LibRaw's [TIFF orientation conversion](https://github.com/LibRaw/LibRaw/blob/0.21.5/src/metadata/tiff.cpp). Its internal flip representation MUST NOT be passed directly as an EXIF value or reduced to a portrait/landscape flag.

Candidate `thumbs_list.thumblist[index].tflip` does not participate in this
fallback, whether zero, nonzero, or unknown. It is not a reliable indication
that the selected JPEG contains an explicit orientation: in particular, zero
alone does not establish that its pixels are already upright. The selected
contract uses the JPEG's own orientation and the decoded container transform,
not a third candidate-flip precedence rule.

Only JPEG EXIF orientation values 1 through 8 are valid; absent, malformed, or
out-of-range JPEG values supply no orientation.

The libvips wrapper reads the selected JPEG's orientation and applies the container fallback only when the JPEG has no valid value. This includes a JPEG with an EXIF block but without a valid Orientation field.

The wrapper normalizes pixels before sizing and encoding and removes orientation metadata from the output. Rust passes bounded metadata through the existing native boundary; there is no second RAW open or metadata-rewriting codec.

The frontend uses the corrected derivative dimensions and performs no compensating rotation.

### Color

When a valid source ICC profile exists, the pipeline preserves it if the output path can preserve it correctly. Otherwise it converts pixels to sRGB and embeds or identifies sRGB correctly.

When no source profile exists, the pipeline treats source RGB as sRGB. This is an interoperability default, not a claim about unknown source colorimetry.

The pipeline MUST NOT copy an invalid or mismatched profile into the derivative.

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

The Rust derivative uses cache algorithm version `rust-vips-v2`. Cache entries from earlier implementations are rebuildable stale entries and are not byte-compatible outputs.

Version 2 uses the RAW-container orientation fallback while retaining JPEG orientation precedence under [Preview Normalization](spec.md#preview-normalization). New derivative records use `rust-vips-v2`.

Historical identity vectors retain `sharp-v2`, `historicalCacheKey`, and `historicalManifestIdentity` as provenance for the earlier implementation. Those rebuildable cache and manifest identities are not migration authority.

Only the vectors' Original File IDs, Photo IDs, and source revisions are deterministic migration authority.

A change to orientation interpretation requires a new pipeline identity even
when Original size and modification time are unchanged. An older algorithm's
bytes or dimensions MUST NOT be reused as a current Derivative or hydrated
current URL. Replacement remains demand-driven and MUST NOT require a Library
scan, Original edits, or deletion of unrelated cache data.

Derivative creation writes to a temporary cache path and publishes the completed file atomically within the cache filesystem.

A failed replacement leaves the previous valid Derivative available but stale. The protocol MUST identify whether a stale result is being shown.

### Scheduling

Normal Preview work remains demand-driven. After a Library scan publishes a new
or changed usable Photo, the existing scheduler also accepts one
`review-2560` warmup request at `Background` priority.

Priority order is:

1. the current Photo in Photo View;
2. the immediately next and previous Photos after the current Preview is ready;
3. visible Grid View Photos;
4. bounded Grid look-ahead work explicitly justified by measured benefit;
5. new or changed `review-2560` warmup.

Warmup is best effort and rebuildable. It does not block Library publication or create a durable task table.

A current manifest and complete file is a cache hit. An interrupted or failed item can be retried by the next scan or an ordinary foreground request.

The first delivery does not backfill every existing Photo or warm `thumbnail-512`.

Duplicate requests for one cache identity share one in-flight job.
Leaving a Photo does not require cancelling extraction if completion is near and
the result remains reusable. The scheduler MAY cancel queued work that has no
remaining consumer.

### Delivery

The browser requests a derivative by Photo identity and target class. It does not provide an Original File path.

A current server-cache hit reads completed derivative bytes without reopening or reprocessing the Original File.

Currentness is evaluated against the Published Library's selected source facts: candidate, Original Location, size, and modification time. Rescan is the invalidation boundary for those facts.

Preview requests MUST validate both the manifest and complete derivative bytes against that published bundle before returning a current result.

Browse Window URL hydration MAY use only bounded manifest, key, source-fact, and file-metadata checks. The subsequent Preview or derivative request remains the correctness-critical byte-validation boundary.

A derivative response uses ordinary HTTP cache validation tied to the cache identity, including an immutable identity-bearing URL and `ETag`.

Successful private derivative responses use `Cache-Control: private, max-age=3600, must-revalidate` and `Vary: Cookie, Authorization`; errors remain non-cacheable. Reconnect and reload MAY reuse browser-cached data when the identity remains current.

The response also makes Preview Source, actual dimensions, and Preview Capability available to Photo View. These facts MUST NOT be inferred only from the derivative URL.

The display condition is not delivered. A browser consumer that needs it derives it locally from measured geometry.

A CLI consumer receives only the stable capability and the actual downloaded dimensions. It MUST NOT be told that a display needs more detail than the derivative holds, because it has no viewport or zoom state to measure.

## Failure Behavior

A native extraction failure is bounded to the affected Photo and candidate. The worker releases native memory and records an actionable internal error without exposing absolute paths or native stack data to the browser.

Only terminal no-usable-source outcomes (`Malformed`, `Unsupported`, and `NoUsablePreview`) seed durable `unavailable` state.

Hard native failures such as `ResourceLimit`, `Io`, and `Internal` remain transient service errors and MUST NOT seed durable `unavailable`. Later requests can retry them.

The scheduler MUST NOT retry continuously. A source change, explicit rescan, or explicit retry can make the Photo eligible again.

A RAW Photo's Preview failure MUST NOT substitute a same-basename JPEG, and a JPEG Photo's Preview failure MUST NOT substitute any other Original's content.

If the server stops during generation, a temporary file is not considered a valid Derivative. Startup or later cache maintenance MAY remove abandoned temporary files.

A malformed or adversarial file MUST NOT cause unbounded allocation based only on claimed dimensions. The implementation MUST enforce explicit input, pixel, output, memory, and concurrency limits.

## Boundary rationale

LibRaw owns RAW container interpretation and complete JPEG validation. libvips owns JPEG normalization, so one bounded container-orientation value crosses their boundary without rewriting JPEG metadata.

Demand-driven generation avoids delaying first use or processing every Photo before it is needed.
