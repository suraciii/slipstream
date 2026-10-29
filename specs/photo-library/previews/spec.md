# Photo Previews

A browser cannot display most RAW Originals directly. Slipstream needs a Preview that is fast enough for repeated selection and faithful enough to represent the camera-produced appearance without becoming a RAW development application.

[Photo Library and Albums](../spec.md) owns Photo identity and Original File boundaries.

## Preview Source Order

Slipstream MUST select the Preview Source from the Photo's own Original File:

```text
if the Original is JPEG:
    use its own JPEG content if usable
else if the Original is RAW:
    inspect its embedded JPEG candidates by descending pixel area
    use the first usable candidate
if no allowed candidate is usable:
    report the Preview unavailable
```

A Preview is usable when Slipstream can decode it, determine its dimensions and orientation, and produce a browser-displayable result.

Slipstream MUST NOT generate a new interpretation from RAW sensor data in the first product. It MUST NOT use another Original File's content.

A RAW Photo MUST NOT use a same-basename JPEG as a fallback. If the Photo's own source is not usable, the Photo has no Preview.

## Trust Boundary

A JPEG Original or embedded RAW JPEG can carry camera-produced appearance, including white balance, picture style, film simulation, tone treatment, crop, and orientation. Slipstream preserves that appearance as the basis for selection.

Slipstream does not promise:

- access to all recoverable RAW highlight or shadow data;
- parity with later RAW development;
- full RAW sensor resolution;
- monitor calibration beyond the browser and operating system's normal color handling;
- identical appearance across uncalibrated displays.

Photo View MUST identify `JPEG` or `RAW embedded JPEG` as the Preview Source. The wire value is `jpeg-original` or `raw-embedded-jpeg`.

The legacy names `matching-jpeg` and `embedded-raw-jpeg` existed only in schema version 5 and MUST NOT appear as current values.

Photo View MUST identify limited detail using both Preview Capability and the Web display condition. It MUST NOT merge the source fact with the current presentation fact.

Preview Capability describes what the delivered source supports for review. It MUST NOT depend on the client's viewport, Preview area, or zoom state.

The Web display condition describes whether the current Preview area and zoom require enlarging source pixels. It is recomputed from browser measurements and does not change the Photo or its Preview Capability.

[Preview Pipeline](design.md#preview-capability) defines the capability derivation and [Web display condition](design.md#web-display-condition).

## Preview Normalization

Slipstream MAY decode, orient, scale, and re-encode source JPEG data for browser delivery. It MUST preserve the visible orientation.

Slipstream MUST NOT apply creative color, exposure, contrast, sharpening, denoising, lens, or crop changes beyond those already present in the source.

For a RAW Photo, a valid orientation in the selected embedded JPEG takes precedence over the RAW container's orientation. If the JPEG has no valid orientation, Slipstream MUST use a valid orientation from that same RAW container.

This fallback also applies when the JPEG contains other EXIF metadata but no valid orientation. For a JPEG Photo, only its own orientation applies.

An explicit normal orientation is valid and MUST prevent an additional RAW container rotation. If neither source supplies a valid orientation, Slipstream MUST preserve the encoded pixel order.

Slipstream MUST NOT guess orientation from image dimensions, filenames, or image content. Rotation and reflection MUST apply exactly once in thumbnails and review Previews.

Slipstream MUST NOT overwrite or append data to the Original File.

The first product provides two derivative sizes:

- a grid thumbnail with a maximum long edge of 512 pixels;
- a review Preview with a maximum long edge of 2560 pixels.

If the source is smaller, Slipstream MUST retain its actual dimensions and MUST NOT upscale it for storage.

The server MAY provide the Photo's own JPEG Original bytes or selected embedded JPEG bytes for magnified inspection when doing so is safe and useful. Full-resolution tiling is not required initially.

## Color Handling

Slipstream MUST preserve an embedded ICC profile when its output encoder supports doing so. If a source JPEG has no profile, Slipstream MUST treat its encoded RGB values as sRGB for browser delivery.

The first product does not convert Previews to Display P3, provide soft proofing, or manage a display profile itself.

If normalization cannot preserve a valid source profile, Slipstream MUST convert the derivative to sRGB rather than attach an incorrect profile.

## Loading and Cache Behavior

Visible Grid cells MAY request thumbnails progressively. Grid loading MUST NOT
wait for thumbnails outside the current viewport and bounded look-ahead.

The current Photo's review Preview has highest priority. After it is ready, the
immediately next and previous Photos MAY load in the background. Adjacent work
MUST NOT delay a newly requested current Photo.

A thumbnail can appear while the review Preview loads. Slipstream MUST NOT
change Selection State because a higher-quality Preview becomes available.

After a Library scan publishes a new or changed usable Photo, Slipstream MUST best-effort queue that Photo's `review-2560` Preview at background priority.

This warmup MUST NOT block Library publication, browsing, Selection State, Rating, navigation, or editing. It MUST reuse the persistent Derivative Cache and the existing cache identity.

The `thumbnail-512` demand path remains unchanged; Slipstream MUST NOT perform a full-Library warmup.

A generated thumbnail or review Preview MUST remain in the configured derivative cache across browser reload and server restart. A current cache hit MUST NOT re-extract or reprocess the Original File.

Derivative delivery MUST follow [Instance Access](../../access/spec.md) and its private-response cache policy. Server-side cache reuse MUST remain available independently of browser caching.

Successful private Preview derivative responses MAY use the standard HTTP cache
policy `private, max-age=3600, must-revalidate`, vary by `Cookie` and
`Authorization`, and identify the cache identity through `ETag`. Session,
metadata, mutation, editing, and Export responses remain `no-store`.

Cache reuse MUST NOT present a derivative from an older version of the Original
File as current. The cache remains rebuildable: deleting cached derivatives can
require regeneration but MUST NOT remove Photo state or modify Original Files.

Warmup failure or interruption remains retryable through a later scan or an
ordinary foreground request. It MUST NOT be reported as Original unreadable.

## Failure Behavior

If a JPEG Photo's own content is corrupt, Slipstream MUST mark that Photo's Preview unavailable. It MUST NOT substitute a same-basename RAW Original's embedded JPEG.

If a RAW Photo's largest embedded candidate is unusable, Slipstream MAY try the next smaller candidate. If no candidate is usable, Slipstream MUST mark the Photo Preview unavailable.

The interface MUST report the source it ultimately uses.

If extraction, decoding, orientation, or derivative generation fails for every allowed source, Slipstream MUST mark the Photo Preview unavailable. It MUST NOT substitute an unrelated file or generic RAW development without identifying a different contract.

A low-resolution embedded JPEG is not a load failure. Slipstream MUST show it and identify that Preview detail is limited.

A Preview generation failure MUST NOT modify the Original File or remove an existing valid cached Preview until a replacement is complete.

