# Photo Previews

A browser cannot display most RAW Originals directly. Slipstream needs a Preview that is fast enough for repeated selection and faithful enough to represent the camera-produced appearance without becoming a RAW development application.

## Preview Source Order

Slipstream must choose the first usable Preview Source in this order:

1. a matching JPEG Original;
2. the RAW Original's largest usable embedded JPEG.

A Preview is usable when Slipstream can decode it, determine its dimensions and orientation, and produce a browser-displayable result.

Slipstream must not generate a new interpretation from RAW sensor data in the first product. If neither source is usable, the Photo has no Preview.

## Trust Boundary

A matching JPEG Original or embedded RAW JPEG may contain camera white balance, picture style, film simulation, tone treatment, crop, and orientation. Slipstream preserves this camera-produced appearance as the basis for selection.

Slipstream does not promise:

- access to all recoverable RAW highlight or shadow data;
- parity with later RAW development;
- full RAW sensor resolution;
- monitor calibration beyond the browser and operating system's normal color handling;
- identical appearance across uncalibrated displays.

Photo View must identify `JPEG` or `RAW embedded JPEG` as the Preview Source. It must identify limited detail when the Preview is smaller than the display or requested zoom requires.

## Preview Normalization

Slipstream may decode, orient, scale, and re-encode source JPEG data for browser delivery. It must preserve the visible orientation and must not apply creative color, exposure, contrast, sharpening, denoising, lens, or crop changes beyond those already present in the source.

Slipstream must not overwrite or append data to the Original File.

The first product provides two derivative sizes:

- a grid thumbnail with a maximum long edge of 512 pixels;
- a review Preview with a maximum long edge of 2560 pixels.

If the source is smaller, Slipstream must retain its actual dimensions and must not upscale it for storage.

The server may provide the original matching or embedded JPEG bytes for Detail Review when doing so is safe and useful. Full-resolution tiling is not required initially.

## Color Handling

Slipstream must preserve an embedded ICC profile when its output encoder supports doing so. If a source JPEG has no profile, Slipstream must treat its encoded RGB values as sRGB for browser delivery.

The first product does not convert Previews to Display P3, provide soft proofing, or manage a display profile itself.

If normalization cannot preserve a valid source profile, Slipstream must convert the derivative to sRGB rather than attach an incorrect profile.

## Loading and Cache Behavior

Visible Grid cells may request thumbnails progressively. Grid loading must not wait for thumbnails outside the current viewport and bounded look-ahead.

The current Photo's review Preview has highest priority. After it is ready, the immediately next and previous Photos may load in the background. Adjacent work must not delay a newly requested current Photo.

A thumbnail may appear while the review Preview loads. Slipstream must not change Selection State because a higher-quality Preview becomes available.

A generated thumbnail or review Preview must remain in the configured derivative cache across browser reload and server restart. A current cache hit must not re-extract or reprocess the Original File. Derivative responses must use identity-bearing immutable browser caching so reconnect and reload may reuse valid bytes.

Cache reuse must not present a derivative from an older version of the Original File as current. The cache remains rebuildable: deleting cached derivatives may require regeneration but must not remove Photo state or modify Original Files.

Slipstream must not automatically generate every Library Preview. Demand-driven generation and bounded nearby prefetch keep mounted-storage I/O, native work, and cache growth proportional to actual browsing.

## Failure Behavior

If a matching JPEG is corrupt and a RAW Original exists, Slipstream must try the RAW Original's embedded JPEG candidates, largest first. If no embedded candidate is usable, Slipstream must mark the Photo Preview unavailable. The interface must report the source it ultimately uses.

If extraction, decoding, orientation, or derivative generation fails for every allowed source, Slipstream must mark the Photo Preview unavailable. It must not substitute an unrelated file or generic RAW development without identifying a different contract.

A low-resolution embedded JPEG is not a load failure. Slipstream must show it and identify that Detail Review is limited.

A Preview generation failure must not modify the Original File or remove an existing valid cached Preview until a replacement is complete.

## Examples

`DSCF0001.RAF` and `DSCF0001.JPG` form one Photo. Slipstream derives its Preview from `DSCF0001.JPG` and labels the source `JPEG`.

`DSCF0002.RAF` has no matching JPEG and contains a 6240-by-4160 embedded JPEG. Slipstream uses that embedded JPEG and labels the source `RAW embedded JPEG`.

`DSCF0003.RAF` contains only a 640-by-480 embedded thumbnail. Slipstream may show it, but Detail Review must identify that focus inspection is limited.
