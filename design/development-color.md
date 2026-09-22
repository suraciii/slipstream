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

## Display and Comparison

Development display must operate on a copy of the Development Result through
a fixed, versioned conversion to sRGB. Any required view mapping must remain in
that display branch. It must never enter the Development TIFF or Film input.

Film display must use the Film Result's defined output encoding and a matching
ICC profile. Comparison must keep the stage, geometry, bundle, and display
conversion fixed while changing only the compared development settings.

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

## Verification

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
