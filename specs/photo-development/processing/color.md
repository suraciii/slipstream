# Development Color Pipeline

A scene-referred module input and a browser display image have different contracts. Separating them prevents display choices from changing scene-referred exposure and white balance.

[Photo Development](../spec.md) owns user-visible controls and composition. [Processing Modules](design.md) owns invocation and artifact boundaries. [darktable Editing](../darktable/editing.md#output-and-export) owns selectable darktable output behavior; [darktable CLI](../darktable/cli.md) owns grammar.

## Model

A processing bundle identifies exact engine builds, adapter schemas, camera/film/paper profiles, ICC assets, and fixed settings for each admitted module configuration. The bundle is part of result identity, not an interchangeable installation.

Module-owned parameters determine each output's format, precision, color space, transfer function, geometry, and encoding. The contracts below qualify concrete module outputs, not global product output concepts. A module that does not admit a contract MUST refuse it rather than reinterpret it.

The qualified scene-linear ProPhoto TIFF configuration admits exactly two embedded ICC byte identities:

| Profile                                        | SHA-256                                                            |
| ---------------------------------------------- | ------------------------------------------------------------------ |
| Unchanged pinned independent native tool asset | `df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed` |
| Historical darktable-normalized profile        | `7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe` |

The profiles have identical tag sets and tag payloads except `desc`, including white point, colorants, and linear transfer curves. They describe the same qualified transform, and existing historical TIFF bytes remain valid unchanged.

Consumers MUST check one of these exact digests and retain the actual embedded digest in byte evidence. The bundle MUST identify profile asset bytes and embedded profile bytes separately; a matching name or appearance is insufficient.

## RAW Development

The adapter MUST define a documented baseline for raw black/white levels, demosaic, camera input interpretation, required camera corrections, orientation, and exposure-bias handling. It MUST NOT inherit desktop configuration or scene-dependent automatic presets.

Exposure compensation MUST apply against the baseline without another automatic scene exposure decision. As-shot white balance MUST preserve actual camera coefficients; rounded temperature/tint MUST NOT replace them when custom white balance was not selected.

Custom white balance MUST use one qualified darktable adaptation path accounting for white balance, input color profile, and color calibration. It MUST NOT apply the same correction twice. Missing required camera information MUST fail the affected capability rather than invent a baseline.

RAW qualification is scoped to source class, capture mode, white-balance mode, and processing bundle. An extension or camera-family label alone does not establish support. Full-resolution Export requires full-resolution qualification; reduced-size evidence is insufficient.

Custom temperature/tint processing requires an independent reference for its mapping, range, and direction for the source class and bundle. The independent tool MUST reject an unqualified source, mode, or WB mapping before processing. It MUST NOT silently substitute as-shot WB or another mapping.

[darktable Editing](../darktable/editing.md#discovering-editing-capabilities) owns general native WB editing.

[Processing Modules](design.md#concrete-module-shapes) owns explicit module ordering, parameter versions, and encoding. [Native darktable Integration](../darktable/design.md) owns private native history construction from captured canonical state and the qualified baseline.

Each tool supplies a complete bounded request and refuses unsupported versions. Ambient XMP and shared desktop database use are forbidden.

For scene-linear output, the development path MUST disable filmic, sigmoid, base curve, AgX, and other display/look mappings. A conflicting saved combination MUST be refused; selecting output MUST NOT silently disable creative modules. Advertised display-referred outputs MAY retain explicitly selected look mapping.

Required technical camera processing remains distinct from optional artistic processing. Initialization MUST NOT silently add sharpening, denoising, lens effects, or contrast presets from desktop defaults.

## Qualified Scene-Referred Module Output

Scene-linear float32 ProPhoto TIFF is a required advertised darktable output combination, not a universal output. Its rules apply when selected, including explicit handoff to standalone SpektraFilm. Other advertised formats, profiles, precision, transfer functions, and encodings have separately validated contracts captured in the edit revision.

For this TIFF, the ICC profile MUST describe actual ProPhoto primaries and white point with a linear transfer function. A profile name or extension is insufficient.

The TIFF MUST use IEEE float32 RGB samples. The writer MUST NOT quantize to integers, clamp to the display interval, or convert through an 8-bit preview. Lossless compression MUST round-trip through the qualified reader.

Dimensions MUST represent full requested output after selected crop, rotation, orientation, and other admitted geometry. Hidden upscale or crop is forbidden. Default geometry retains the full developed image; explicit crop is saved intent and MUST agree with mask coordinate frames and output metadata.

Compressed strip layout MUST account for declared full geometry. The qualified writer emits each Deflate strip through its own buffer: a strip can extend one byte beyond its declared count, and the artifact can end a few bytes beyond its last declared strip. This bounded padding is not payload.

The qualified strip shape has one entry per declared row block. Validation MUST bound parsed arrays by qualified geometry and decode the artifact to establish that compressed strips inflate to that geometry. An artifact outside that shape MUST fail validation and MUST NOT be offered or published.

The qualified path MUST define treatment of negative, over-range, and out-of-gamut values and preserve recoverable highlight information at handoff. Validation MUST distinguish source clipping, development transformations, film behavior, and display clipping. Float storage cannot recover previously clipped sensor data.

## Qualified Standalone SpektraFilm Output

The initial Film Recipe combines Kodak Portra 400 and Kodak Portra Endura. Fixed settings MUST identify the complete negative, print, and scan configuration, including grain, halation, print normalization, scanner settings, profile versions, and stochastic policy. Profile names alone MUST NOT define a saved look.

For the initial qualified configuration, the adapter MUST select ProPhoto RGB input and disable input transfer-function decoding for an already-linear admitted artifact. It MUST disable SpektraFilm camera auto exposure and keep the additional camera exposure offset at zero. Print/reference normalization MUST be fixed in the bundle rather than inferred from upstream defaults.

The adapter MUST invoke the core simulation API and explicitly select output space and encoding. An output already encoded for display MUST NOT receive the same transfer function again during file encoding. Engine-private optimization LUTs MAY be used only when included in the qualified recipe.

Full simulation MUST retain the recipe's spatial and stochastic behavior. A per-pixel external LUT is not an equivalent replacement for grain and halation.

Repeated rendering under the same bundle and geometry MUST follow a qualified reproducibility policy with explicit random-state and threading ownership. A global NumPy seed alone is insufficient; uncontrolled variation MUST be corrected or resolved before qualification.

LUT qualification compares the internal LUT-enabled recipe with direct spectral evaluation under the same input, bundle, geometry, and complete Film effects. The criterion is D65 CIEDE2000 on encoded sRGB outputs converted to CIE Lab using pinned sRGB colorimetry.

Across every pixel in the accepted representative corpus, nearest-rank p95 MUST be at most 0.005 and maximum MUST be at most 0.01. Mean difference is diagnostic, not an acceptance substitute.

The corpus MUST include neutral, saturated, negative, over-range, structured, textured, and representative camera-derived inputs. A small synthetic engine-check corpus is a diagnostic smoke check, not full Film quality acceptance. LUT use additionally requires exact in-process, fresh-process, and LUT/direct/LUT A/B/A repeatability under the accepted runtime.

## Display and Comparison

The qualified scene-referred output's display derivative MUST operate on a copy through a fixed versioned linear ProPhoto RGB to sRGB conversion. It converts to linear sRGB using pinned profile primaries, white points, and chromatic adaptation. It then clips each linear sRGB channel to zero through one and applies the sRGB transfer function.

Clipping defines display behavior for negative, over-range, and out-of-gamut values and can change displayed hue or brightness. It MUST NOT alter the module output. This display branch MUST NOT enter scene-referred output or another module's input.

For other advertised output contracts, Preview uses the separately identified display conversion appropriate to the captured profile and transfer function. It MUST NOT apply the linear-ProPhoto transform to already encoded pixels.

Statistics identify source, working, output, or display values and clipping scope. Pixel inspection discloses scale and coordinate frame; bounded Preview is not full-resolution spatial evidence.

The destination uses standard IEC 61966-2-1 sRGB colorimetry with a D65 white point. The bundle MUST pin:

- Exact source and destination ICC bytes.
- Color-management implementation and version.
- Rendering intent.
- Adaptation method and state.
- Conversion algorithm version.

An implicit library default MUST NOT choose intent or adaptation. Display capability is unavailable until the complete transform identity is qualified and included in the bundle.

The destination profile is `sRGB-elle-V2-srgbtrc.icc`, SHA-256 `b44e86e44d44993a3a9a880626f9832e9c37e2234caba501548f9114bada6d21`. Transform `display-transform-v1` uses Bradford D50-to-D65 adaptation with ICC PCS white points and relative colorimetric intent. The intent is insensitive because both profiles are matrix/TRC.

The transform uses this fixed matrix derived from the pinned profile colorants. A library MAY parse profiles and encode files, but MUST NOT choose intent, adaptation, or output scaling.

```text
[ 2.034390615588929, -0.727658826450461, -0.306731789138469]
[-0.228838423556639,  1.231758946896254, -0.002920523339614]
[-0.008543280951811, -0.153257113180394,  1.161800394132204]
```

The derivative resamples in linear space, applies the matrix, clips each linear sRGB channel, applies the sRGB transfer function, and quantizes to 8-bit once. Samples are column vectors: `linear_sRGB = M * linear_ProPhoto`, with matrix rows producing red, green, and blue. `M` includes the single Bradford D50-to-D65 adaptation and primaries conversion; no further adaptation or conversion is applied.

The following fixed chain uses double precision from each resampled linear sample:

1. `linear[i] = M[i][0] * ProPhoto[0] + M[i][1] * ProPhoto[1] + M[i][2] * ProPhoto[2]`
2. `clipped[i] = min(max(linear[i], 0.0), 1.0)`
3. `display[i] = 12.92 * clipped[i]` when `clipped[i] <= 0.0031308`; otherwise `1.055 * clipped[i] ** (1.0 / 2.4) - 0.055`.
4. `byte[i] = round(display[i] * 255.0)`, rounding halves away from zero.

The transform contract is the pre-encoding byte frame. Derivative bytes also depend on the resampling kernel, sample-center and edge handling, encoder configuration, and encoder version.

Caches and validators MUST key on recorded display conversion identity. Derivative bytes are comparable only within one recorded implementation version; a different resampler or encoder is a different display conversion.

The qualified implementation resamples in linear light with Lanczos3 and MUST NOT upscale beyond source geometry. It encodes 8-bit JPEG at quality 85 with the destination profile embedded. The bundle MUST record implementation and version alongside profile bytes.

Standalone SpektraFilm display MUST use its module result's defined output encoding and a matching ICC profile. Its initial qualified output is full-developed-dimension sRGB JPEG at quality 85 with pinned encoder configuration and embedded destination profile. Comparison MUST hold selected module, geometry, bundle, and display conversion fixed while changing compared step settings.

Reduced-resolution simulation MUST carry its own geometry and cache identity. Grain, halation, and physical scale MUST follow qualified engine behavior. It MUST NOT claim pixel equivalence to a downsampled full-size simulation.

Full-detail inspection MUST derive from a completed full-resolution result; a separate crop/tile implementation requires its own spatial-context contract.

Memory optimizations MUST preserve color and reproducibility. [Processing Memory](memory.md#engine-memory-efficiency) owns bounded computation, buffer lifetime, and qualification of spatial or precision changes.
