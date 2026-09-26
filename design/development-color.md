# Development Color Pipeline

The browser needs a viewable image, while Spektrafilm needs scene-referred
input. Using one encoded image for both purposes would bake display choices
into the film simulation and make exposure and white balance unpredictable.

[Photo Development](../docs/photo-development.md) owns user-visible controls
and output targets. This specification owns the processing contract between
Original input, the Development Result, the Film Result, and display derivatives.

## Model

A processing bundle identifies an exact darktable build, adapter schema,
Spektrafilm commit, camera/film/paper profiles, ICC assets, and fixed processing
settings. The bundle is part of result identity, not an interchangeable tool
installation.

An Edit Recipe contains semantic exposure and white-balance intent plus the
fixed Film Recipe reference. Engine module parameters are private derived data.
The domain model must not contain darktable history blobs or Python objects.

The initial Development TIFF uses the `LargeRGB-elle-V2-g10.icc` profile asset
with SHA-256
`df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed`. The
profile's primaries, D50 white point, and linear transfer curves are part of
that identity; a profile name alone is insufficient.

The pinned darktable run embedded an ICC profile with SHA-256
`7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe`. Its bytes
differ from the profile asset above only in legacy description-tag
normalization. The processing bundle must identify the asset bytes and the
exact embedded profile bytes separately; matching a name or appearance is
not enough.

## RAW Development

The adapter must define one documented baseline for raw black/white levels,
demosaic, camera input interpretation, required camera corrections, orientation,
and exposure-bias handling. It must not inherit a photographer's desktop
configuration or scene-dependent automatic presets.

Exposure compensation must apply against that baseline without an additional
automatic scene exposure decision. As-shot white balance must preserve the
actual camera coefficients. A rounded temperature/tint representation must not
replace those coefficients when the user has not selected custom white balance.

Custom white balance must use one qualified darktable adaptation path. The
adapter must account for the relationship between white balance, input color
profile, and color calibration. It must not apply the same correction twice.
Temperature and tint mapping, range, and direction must be validated against
reference engine output for supported cameras. Missing required camera
information must fail the affected capability rather than invent a baseline.

RAW qualification is scoped to an explicit source class, capture mode, white-
balance mode, and processing bundle. A recognized file extension or camera-
family label alone does not grant development support. Full-resolution Export
support requires a full-resolution qualification; reduced-size evidence does
not establish it. Custom temperature/tint processing requires an independent
reference for its mapping, range, and direction under that source class and
bundle. The service must reject an unqualified source, mode, or WB mapping
before processing admission. It must not silently substitute as-shot WB or
another mapping. The adjustable-WB product target remains defined by the
[Product Spec](../docs/photo-development.md#development-controls).

Module ordering, parameter versions, and encoding must be explicit in the
adapter. Unsupported module versions must fail. The adapter must generate a
complete bounded internal history and explicitly supply it to darktable.
Ambient XMP discovery and use of a shared desktop database are forbidden.

The development path must disable filmic, sigmoid, base curve, AgX, and any
other display/look mapping. Required technical camera processing must be
explicitly distinguished from optional artistic processing. The adapter must
not silently add sharpening, denoising, lens effects, or contrast presets merely
because a desktop default contains them.

## Development Result

The handoff must use the Development TIFF contract in the Product Spec. Its
ICC profile must describe both the actual ProPhoto primaries/white point and a
linear transfer function. A profile name or TIFF extension alone is insufficient
evidence.

TIFF output must use IEEE float32 RGB samples. The writer must not quantize to
integers, clamp values to the display interval, or convert through an 8-bit
preview. Lossless compression must round-trip through the qualified reader.
Dimensions must describe the full developed image and its already-applied
orientation, with no hidden upscale or user crop.

The qualified path must define treatment of negative, over-range, and
out-of-gamut values and preserve recoverable highlight information at the
handoff. Float storage does not recover sensor data that was already clipped.
Validation must distinguish source clipping, development transformations,
film behavior, and display clipping.

## Film Simulation

The initial Film Recipe combines Kodak Portra 400 and Kodak Portra Endura.
Its fixed settings must identify the complete negative, print, and scan
configuration, including grain, halation, print normalization, scanner settings,
profile versions, and the stochastic policy. Profile names alone must not define
a saved look.

The adapter must explicitly select ProPhoto RGB input and disable input
transfer-function decoding for the already-linear Development Result. It must
disable Spektrafilm camera auto exposure and keep the additional camera exposure
offset at zero. Print/reference normalization must be fixed in the bundle and
must not be inferred from changing upstream defaults.

The adapter must invoke the core simulation API and explicitly set output
space and encoding. An output already encoded for display must not receive the
same transfer function again during file encoding. Engine-private optimization
LUTs may be used only when the qualified recipe includes that mode.

Full simulation must retain the recipe's spatial and stochastic behavior. A
per-pixel external LUT is not an equivalent replacement for grain and halation.
Repeated rendering under the same bundle and geometry must follow a tested
reproducibility policy. The adapter must establish random-state ownership and
threading behavior; setting a global NumPy seed alone is not sufficient proof.
Uncontrolled variation must be corrected or resolved in the governing Issue
before that processing configuration is qualified.

LUT qualification compares the internal LUT-enabled recipe with direct
spectral evaluation of the same input, bundle, geometry, and complete Film
effects. The fixed visual criterion is D65 CIEDE2000 on the encoded sRGB
outputs after conversion to CIE Lab with the pinned sRGB colorimetry. Across
every pixel in the accepted representative corpus, the nearest-rank p95 must
be at most 0.005 and the maximum must be at most 0.01. Mean difference is
reported for diagnosis but is not an acceptance substitute. The corpus must
include neutral, saturated, negative, over-range, structured, textured, and
representative camera-derived inputs; the small synthetic engine-check corpus
is a diagnostic smoke check, not full Film quality acceptance. LUT use is not
qualified until the criterion passes together with exact in-process,
fresh-process, and LUT/direct/LUT A/B/A repeatability under the accepted
runtime.

## Display and Comparison

Development display must operate on a copy of the Development Result through
a fixed, versioned conversion from linear ProPhoto RGB to sRGB. Convert to
linear sRGB using the pinned profile primaries, white points, and chromatic
adaptation. Clip each linear sRGB channel independently to the interval from
zero to one, then apply the sRGB transfer function. This clipping is the
defined display behavior for negative, over-range, and out-of-gamut values; it
may change their hue or brightness in the view. It must not alter the
Development Result. This display branch must never enter the Development TIFF
or Film input.

The destination must use standard sRGB colorimetry defined by IEC 61966-2-1
with a D65 white point. This identifies the target colorimetry, not the profile
bytes or D50-to-D65 adaptation. The processing bundle must pin the exact source
and destination ICC profile bytes, the color-management implementation and
version, rendering intent, adaptation method and state, and conversion
algorithm version. An implicit library default must not choose the rendering
intent or adaptation. Display capability is unavailable until the complete
transform identity is qualified and included in the bundle.

The destination profile asset is `sRGB-elle-V2-srgbtrc.icc` with SHA-256
`b44e86e44d44993a3a9a880626f9832e9c37e2234caba501548f9114bada6d21`. The
transform identity is version `display-transform-v1`: a Bradford D50-to-D65
adaptation using the ICC PCS white points, the rendering intent `relative
colorimetric` (intent-insensitive because both profiles are matrix/TRC), and
the fixed matrix below derived from the two pinned profiles' colorants. A
library may supply profile parsing and file encoding, but it must not choose
the intent, adaptation, or output scaling. The committed identity is this
matrix:

```
[ 2.034390615588929, -0.727658826450461, -0.306731789138469],
[-0.228838423556639,  1.231758946896254, -0.002920523339614],
[-0.008543280951811, -0.153257113180394,  1.161800394132204],
```

A display derivative resamples in linear space, applies the matrix, clips each
linear sRGB channel, applies the sRGB transfer function, and quantizes to 8-bit
once. Samples are column vectors, `linear_sRGB = M * linear_ProPhoto`, with
matrix row one producing red, row two green, and row three blue. `M` already
carries the single Bradford D50-to-D65 adaptation and the source-to-destination
primaries conversion; no further adaptation or conversion is applied before or
after it. The chain is fixed, with the matrix product, clip, transfer function,
and quantization evaluated in double precision from the resampled linear sample:

1. `linear[i] = M[i][0] * ProPhoto[0] + M[i][1] * ProPhoto[1] + M[i][2] * ProPhoto[2]`
2. `clipped[i] = min(max(linear[i], 0.0), 1.0)`
3. `display[i] = 12.92 * clipped[i]` when `clipped[i] <= 0.0031308`, otherwise
   `1.055 * clipped[i] ** (1.0 / 2.4) - 0.055`
4. `byte[i] = round(display[i] * 255.0)`, rounding halves away from zero

These vectors are the contract for that arithmetic: for each linear ProPhoto
sample the conversion must produce exactly these sRGB bytes before JPEG
encoding, and every `display-transform-v1` implementation must reproduce all
ten, including the channel-isolating and over-range cases.

```
[ 0.0,  0.0,  0.0] -> [  0,   0,   0]
[ 0.18, 0.18, 0.18] -> [118, 118, 118]
[ 1.0,  1.0,  1.0] -> [255, 255, 255]
[ 1.0,  0.0,  0.0] -> [255,   0,   0]
[ 0.0,  1.0,  0.0] -> [  0, 255,   0]
[ 0.0,  0.0,  1.0] -> [  0,   0, 255]
[ 2.0,  0.5,  0.125] -> [255, 111,  64]
[-0.25, 0.5,  0.5] -> [  0, 214, 189]
[ 0.5,  0.5,  2.0] -> [ 56, 187, 255]
[ 0.9,  0.2,  0.05] -> [255,  57,  38]
```

The transform contract is that pre-encoding byte frame. Derivative bytes also
depend on the implementation that produced the samples and the file: the
resampling kernel and its sample-center and edge handling, and the encoder
configuration and version, including subsampling, optimization, metadata, and
quality. Caches and validators must therefore key on the recorded display
conversion identity, and derivative bytes are comparable only within one
recorded version of that implementation: a different resampler or encoder is a
different display conversion, not a re-certification of the same one. The
qualified implementation resamples in linear light with a Lanczos3 kernel,
never upscales a target beyond the source geometry, and encodes 8-bit JPEG at
quality 85 with the destination profile embedded; the processing bundle must
record its implementation and version alongside the profile bytes.

Film display must use the Film Result's defined output encoding and a matching
ICC profile. The initial Finished JPEG is full developed dimensions, encoded
as sRGB at fixed JPEG quality 85 with the pinned encoder configuration and
embedded destination profile.
Comparison must keep the stage, geometry, bundle, and display conversion fixed
while changing only the compared development settings.

A reduced-resolution simulation must carry its own geometry and cache identity.
Grain, halation, and physical scale must follow the qualified engine behavior.
It must not claim pixel equivalence to a downsampled full-size simulation.
Full-detail inspection must derive from a completed full-resolution result;
a separate crop/tile implementation requires its own spatial-context contract.

## Options

### Selected: Scene-Linear TIFF Handoff

A concrete interoperable file follows the author's workflow, can be inspected
independently, and permits Development TIFF delivery before integrated simulation.
Its disk and memory costs are explicit bounded processing resources.

### Rejected: Browser Preview as Simulation Input

An encoded JPEG loses precision and mixes display rendering with input data.
It cannot establish the linear development contract even if it looks plausible.

### Selected: Versioned Engine Adapter

One narrow adapter owns module versions and semantic parameter mapping. It
keeps engine representation out of Web, CLI, and persistence contracts.

### Rejected: Arbitrary XMP or Python Parameters

Exposing engine internals would make clients responsible for module ordering,
color defaults, compatibility, and unsafe input. The current workflow needs only
exposure, white balance, and a fixed Film Recipe.

### Selected: Fixed Display-Only sRGB Conversion

The editor needs a deterministic view for a scene-linear ProPhoto image.
Clipping after conversion to linear sRGB defines predictable handling of
display-boundary values without changing the TIFF handoff or Film input.

### Rejected: Perceptual Gamut Mapping in the Development Handoff

A perceptual mapper would introduce another look into the scene-referred
pipeline. The Development view is for inspection; its bounded conversion must
remain isolated from saved image data and downstream processing.

### Rejected: Reuse the Display Rendition as TIFF or Film Input

The sRGB conversion clips scene-linear values and adds a display transfer
function. Reusing it would discard information and violate the Development TIFF
and Film input contracts.

### Selected: Bundle-Pinned ICC Transform

The profile bytes, color-management implementation, rendering intent, and
white-point adaptation are all part of output identity. Pinning them in the
processing bundle makes the display transform reproducible across hosts and
upgrades.

### Rejected: Implicit Color-Management Defaults

Default profiles, rendering intents, or adaptation settings can vary with
installed libraries and host configuration. A color-space label alone cannot
make the view repeatable.

## Verification

Memory optimizations must preserve this color and reproducibility contract.
[Processing Memory](processing-memory.md#engine-memory-efficiency) defines
bounded pointwise computation, buffer lifetime, and the qualification required
before changing spatial processing or numerical precision.

Qualification must establish:

- actual TIFF sample format, dimensions, orientation, ICC bytes, and transfer
  curve, including compressed round-trip behavior;
- neutral, saturated, negative, and over-range synthetic image behavior;
- EV scaling before film simulation and no unintended automatic exposure;
- as-shot coefficient preservation and custom temperature/tint mapping on the
  supported camera corpus;
- no duplicate adaptation or transfer-function conversion;
- reference darktable output versus generated internal history;
- reference Spektrafilm output versus the adapter with the same complete bundle;
- repeated CPU output, grain random-state behavior, and preview/detail limits;
- output profile, metadata, and JPEG encoding correctness; and
- explicit rejection when a required engine version or profile is missing.

Use the [author's preparation workflow](https://github.com/andreavolpato/spektrafilm#preparing-input-images-manually-with-darktable)
and [darktable's CLI reference](https://docs.darktable.org/usermanual/5.6/en/special-topics/program-invocation/darktable-cli/)
as upstream input to qualification. Exact build identities and measured results
belong in the governing Issue, not this specification.
