# darktable Integration

A Photographer needs saved corrections to produce the same intended photograph
in an Edit Preview and an Export, without operating a second application's
catalog. Engine upgrades and newly available modules must not silently change
those corrections or expose unsupported controls.

[Photo Development](photo-development.md) owns the editing workflow, controls,
output targets, and exclusions. This specification owns the product relationship
between Slipstream and darktable. It does not expand the supported editing scope
or the [release boundary](0.1-support-and-release.md).

## Relationship

Slipstream must remain the application the Photographer uses. It must own Photos,
saved Edit Recipes, editing conflicts, and Export outcomes. darktable must act as
the development engine, not a second Photo Library or an external editing
workspace that the Photographer must manage.

The Photographer must not need to install, open, configure, or synchronize a
personal darktable desktop catalog. Using development must not import a desktop
editing history or save corrections to an external XMP Sidecar.

The Web and CLI must use the same service-owned settings and capability
boundaries. Neither client may bypass Slipstream by connecting directly to the
engine. This specification does not add a public MCP interface or new CLI syntax.

## Local Availability

Slipstream is a personal, single-machine application. Photo Development must be
an optional capability inside the same application installation, not a separately
operated engine service. A normal local start must not require a host processing
launcher, processing systemd unit, worker container, or processing control socket.

Missing or invalid engine assets must leave the Library usable and report
Development unavailable. The application must serialize development requests.
Cancellation and timeout must finish engine cleanup before the next request runs.
Film must remain unavailable unless separately qualified and admitted.


## Available Controls

An engine module being installed or discoverable must not make it a supported
Slipstream control. Only controls admitted for the deployed processing bundle,
Photo source, and requested output may be offered as executable.

Exposure and white balance retain the semantics in
[Development Controls](photo-development.md#development-controls). The fixed
Film Recipe remains separate from darktable development. Additional controls
require a product definition of their meaning, default, reset, valid settings,
interaction with other corrections, and supported sources and outputs before
admission. This integration alone must not expose arbitrary processing graphs,
module ordering, or a general darktable module browser.

For an admitted control, the workspace must show the Photo's actual current
setting and reset target. Camera-dependent values must come from that Photo's
initialization, not a generic engine default or an invented camera setting.

## Automatic Adjustments

An admitted automatic adjustment must be an explicit action on the selected
Processing Step. The request must identify the module operation and carry only
the module-owned automatic instruction; it must not ask Slipstream to implement
the correction algorithm.

The native engine must evaluate the instruction against the Photo's actual
initialized state and return the concrete parameter values that it used.
Slipstream must capture those values in the guarded Processing Recipe before
Preview or Export uses them. A later Preview or Export must not recompute the
automatic correction independently.

The first admitted automatic action is exposure deflicker. Its percentile and
target level use the native exposure module's valid ranges. The captured result
is a manual exposure value with the qualified baseline controls preserved.

The qualified color-calibration action detects an illuminant through the native
`channelmixerrgb` operation (edge or surface instruction) and captures concrete
custom chromaticity, temperature, and CAT16 adaptation values. It is admitted
only for the qualified RAW input and is stored as a concrete color-calibration
entry; the detection mode itself must never reach Preview or Export.
Automatic evaluation must be refused when the source or engine cannot produce
a deterministic result, and refusal must leave the saved Recipe unchanged.

Automatic evaluation and Recipe saving are one guarded client operation:
stale Recipe or source revisions must reject the captured result. A failed or
stale evaluation must not publish a Preview, Export, or partially updated
Recipe.

A deployment upgrade must not silently enable a new correction, reset saved
settings, or reinterpret an unsupported setting as another setting. Saved
settings must remain readable when they cannot currently be executed. The
workspace must explain the unavailable capability and preserve those settings.

## Editing and Outputs

The service must capture the settings used for each processing request. An Edit
Preview and an Export requested from the same settings must apply the same
corrections against the same processing baseline. Their resolution and display
conversion may differ under the existing output contracts.

An Export must not require a prior preview request to materialize its edits.
Opening a Photo or requesting an output must not change its saved Edit Recipe.
A result for older settings must not replace a result for newer settings.
[Autosave and Reversible Editing](photo-development.md#autosave-and-reversible-editing)
and the existing Export rules own save conflicts and captured Export state.

A retained Development Proxy must not claim to execute a correction it cannot
represent. [Original Availability and Recovery](photo-development.md#original-availability-and-recovery)
owns its availability and source restrictions; this integration does not grant
a proxy new editing capabilities.

## Failure Behavior

An invalid setting must be refused without rounding it into a different enum
choice, truncating an integer, substituting zero, or ignoring the correction.
The refusal must identify the affected control or unavailable capability.
Recipe saves must remain atomic and independent from rendering. If an engine
request fails after a save was confirmed, the confirmed Edit Recipe must remain
saved; only the requested output fails. Rendering must not partially save,
replace, or roll back editing intent.

If an engine is unavailable, incompatible, canceled, or fails, Slipstream must
report the affected operation without claiming a successful matching output.
It must not substitute a camera Preview, an uncorrected result, or a different
engine version. The existing editing workflow owns retained previews, retry,
and uncertain-outcome reconciliation.

Original File and external XMP safety remains governed by
[Photo Development](photo-development.md). Normal browsing, Selection State,
Rating, and Album membership must not depend on engine availability.

## Examples

- A bundle contains a curve module, but Slipstream has not admitted a curve
  control. The Photographer sees the supported exposure and white-balance
  controls, not an automatically expanded editor.
- A Photographer changes exposure and immediately requests a Development TIFF.
  The Export uses the captured correction without requiring a preview first.
- A new deployment cannot execute a saved white-balance mode. Slipstream keeps
  the saved values readable and explains the unavailable mode; it must not
  silently process the Photo with as-shot white balance.
- A programmatic client submits an invalid numeric type. The service refuses
  the setting; the engine must not turn it into a successful zero correction.
