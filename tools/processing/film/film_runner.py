"""Standalone SpektraFilm local adapter (production peer, Issue #496).

Stage two of an artifact-bound `spektrafilm` Processing Step. The retained
Development TIFF one peer published — IEEE float32 RGB samples, bounded full
geometry, and the exact embedded linear ProPhoto ICC bytes of the shared Film
identity — is consumed directly: this adapter never invokes darktable, and no
implicit chain, conversion, or camera-preview fallback exists. The step's
exact complete parameter tree is forwarded to the pinned simulator only after
it is verified to be exactly the pinned fixed recipe; any deviation is a
pinned-identity refusal before a single pixel is rendered. The published
artifact is the quality-85 sRGB Finished JPEG of the shared identity.

`preview` bounds the render geometry before simulation: the frame is decoded,
resampled once to the disclosed bounded geometry, freed, and only then
simulated. No full-resolution handoff is produced, and effects, randomness,
color interpretation, and encoding defaults never change between Preview and
Export.

The fixed recipe, seed behavior, and Finished JPEG encoding are reused from
the qualified development runtime (`film.py`, `finished_jpeg.py`); every
pinned identity comes from the one authoritative `film_identity.py`. The
numerical runtime itself is confined by the application's process-group
supervisor: cancellation and the deadline kill the whole group, so this
process performs no signal handling of its own.
"""

import argparse
import hashlib
import json
import os
import stat
import sys
from pathlib import Path

# In the extended application image the qualified development runtime lives at
# /opt/probe; on a development host it is the sibling tools/development tree
# of this repository.
_PROBE = Path("/opt/probe")
if (_PROBE / "film_identity.py").is_file():
    sys.path.insert(0, str(_PROBE))
else:
    sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "development"))

from film_identity import (  # noqa: E402
    FILM_RECIPE_SHA256,
    FINISHED_JPEG_QUALITY,
    INPUT_ICC_SHA256,
    OUTPUT_ICC_SHA256,
    recipe_digest,
)

PRODUCED = 0
ENGINE_FAILED = 1
# The pinned-identity refusal: the attempt is rejected before any engine
# work, exactly like the historical production film adapter's handoff-profile
# refusal.
REFUSED = 71

# The qualified geometry bounds, mirroring the plans the pinned workspace
# models were computed for, so no attempt can hand the pinned numerical
# runtime an unbounded frame.
MAX_DECODED_BYTES = 2 * 1024**3
MAX_EDGE = 9568
MAX_COORDINATE_SUM = 175_000
NUMBA_CACHE = Path(os.environ.get("NUMBA_CACHE_DIR", "/work/numba"))

# The runtime-owned manifest groups of the pinned fixed recipe, exactly the
# groups `film.make_simulator` returns, and the tree's camelCase spellings.
GROUPS = (
    "camera", "enlarger", "scanner", "io", "settings", "debug",
    "film_render", "print_render", "taps",
)
TREE_KEYS = {
    "camera": "camera",
    "enlarger": "enlarger",
    "scanner": "scanner",
    "io": "io",
    "settings": "settings",
    "debug": "debug",
    "film_render": "filmRender",
    "print_render": "printRender",
    "taps": "taps",
}
PINNED_OUTPUT = {
    "format": "jpeg",
    "precisionBits": 8,
    "colorSpace": "srgb",
    "transferFunction": "srgb",
    "geometry": "input-preserving",
    "encoding": "quality-85-baseline",
}


class Refusal(Exception):
    """A pinned-identity violation: no engine work may be trusted after it."""


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def geometry_bounds(width, height):
    """The bounded frame the pinned workspace plans are computed for."""
    return (
        1 <= width <= MAX_EDGE
        and 1 <= height <= MAX_EDGE
        and width * height * 12 <= MAX_DECODED_BYTES
        and 3 * max(width, height) <= MAX_COORDINATE_SUM
    )


def profile_is_pinned(profile, expected_sha256):
    """Exact embedded profile bytes; a lookalike profile is not evidence."""
    return (
        profile is not None
        and 0 < len(profile) <= 16384
        and sha256_bytes(bytes(profile)) == expected_sha256
    )


def input_spec_is_pinned(spec, sample_format, profile):
    """One full-frame float32 RGB image with the pinned handoff profile."""
    return (
        spec.nchannels == 3
        and not spec.channelformats
        and sample_format == "float"
        and spec.depth == 1
        and spec.x == 0
        and spec.y == 0
        and spec.z == 0
        and spec.full_width == spec.width
        and spec.full_height == spec.height
        and spec.full_depth == 1
        and spec.get_int_attribute("Orientation", 1) == 1
        and geometry_bounds(spec.width, spec.height)
        and profile_is_pinned(profile, INPUT_ICC_SHA256)
    )


def check_environment():
    """The deterministic one-thread Numba runtime with a fresh private cache."""
    expected = {
        "NUMBA_CACHE_DIR": str(NUMBA_CACHE),
        "NUMBA_NUM_THREADS": "1",
        "OMP_NUM_THREADS": "4",
        "OPENBLAS_NUM_THREADS": "4",
        "NUMEXPR_NUM_THREADS": "4",
        "PYTHONDONTWRITEBYTECODE": "1",
    }
    if any(os.environ.get(key) != value for key, value in expected.items()):
        return False
    try:
        metadata = NUMBA_CACHE.lstat()
        empty = not any(NUMBA_CACHE.iterdir())
    except OSError:
        return False
    return stat.S_ISDIR(metadata.st_mode) and empty


def output_destination_is_available(output_path):
    """A missing or service-reserved destination, and nothing else.

    The service reserves the published destination by creating an empty
    regular file — the same replaceable-file invariant the local darktable
    peer accepts. A missing path or an empty non-symlink regular file is
    therefore admitted; any nonempty file, any symlink (even one naming an
    empty regular file), and any other destination type may be user data
    and keeps the refusal.
    """
    try:
        metadata = output_path.lstat()
    except FileNotFoundError:
        return True
    except OSError:
        return False
    return stat.S_ISREG(metadata.st_mode) and metadata.st_size == 0


def development_pixels(input_path):
    """Read the retained Development TIFF and verify the handoff."""
    import OpenImageIO as oiio

    metadata = input_path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
        raise Refusal("development source identity is invalid")
    reader = oiio.ImageInput.open(str(input_path))
    if reader is None:
        raise Refusal("development source is not readable")
    try:
        spec = reader.spec()
        if not input_spec_is_pinned(spec, str(spec.format), spec.getattribute("ICCProfile")):
            raise Refusal("development source misses the pinned handoff identity")
        if reader.seek_subimage(1, 0):
            raise Refusal("development source must be a single-frame TIFF")
        if not reader.seek_subimage(0, 0):
            raise Refusal("development source is not readable")
        pixels = reader.read_image(format=oiio.FLOAT)
    finally:
        closed = reader.close()
    if not closed:
        raise Refusal("development source is not readable")
    import numpy as np

    if (
        type(pixels) is not np.ndarray
        or pixels.dtype != np.dtype(np.float32)
        or pixels.shape != (spec.height, spec.width, 3)
        or not pixels.flags.c_contiguous
    ):
        raise Refusal("development source is not a float32 RGB frame")
    return pixels, (spec.width, spec.height)


def check_recipe(manifest):
    """Re-verify the fixed recipe of the shared engine against the identity."""
    import film

    if recipe_digest(manifest) != FILM_RECIPE_SHA256:
        raise Refusal("film recipe identity does not match the pinned fixed recipe")
    camera, io, settings, debug = (
        manifest["camera"],
        manifest["io"],
        manifest["settings"],
        manifest["debug"],
    )
    grain = manifest["film_render"]["grain"]
    if (
        camera["auto_exposure"]
        or camera["exposure_compensation_ev"] != 0.0
        or io["input_color_space"] != "ProPhoto RGB"
        or io["input_cctf_decoding"]
        or io["output_color_space"] != "sRGB"
        or not io["output_cctf_encoding"]
        or settings["preview_mode"]
        or settings["use_fast_stats"]
        or not settings["use_enlarger_lut"]
        or not settings["use_scanner_lut"]
        or settings["lut_resolution"] != 33
        or debug["deactivate_spatial_effects"]
        or debug["deactivate_stochastic_effects"]
        or not grain["active"]
        or not grain["sublayers_active"]
        or manifest["seed"] != film.SEED
        or FINISHED_JPEG_QUALITY != 85
    ):
        raise Refusal("film recipe does not pin the qualified procedure")


def verify_complete_tree(tree, manifest):
    """The forwarded tree must be exactly the pinned recipe's own groups.

    The tree is module-owned and preserved verbatim: every group is compared
    against the runtime manifest's JSON representation, so an unknown
    field, a changed control, or a missing group is a refusal — never a
    merge, default, or reinterpretation. `output`, when present, must be the
    pinned Finished JPEG contract.
    """
    if type(tree) is not dict:
        raise Refusal("the complete parameter tree is not an object")
    allowed = set(TREE_KEYS.values()) | {"output"}
    if set(tree) - allowed:
        raise Refusal("the complete parameter tree carries unknown groups")
    # JSON represents the runtime recipe's tuples as arrays, just as the
    # emitted defaults and pinned recipe identity do.
    manifest = json.loads(json.dumps(manifest))
    for group, key in TREE_KEYS.items():
        if key not in tree:
            raise Refusal(f"the complete parameter tree misses the `{group}` group")
        if type(tree[key]) is not dict:
            raise Refusal(f"the `{group}` group is not an object")
        if tree[key] != manifest[group]:
            raise Refusal(f"the `{group}` group is not the pinned fixed recipe")
    if "output" in tree and tree["output"] != PINNED_OUTPUT:
        raise Refusal("the output options are not the pinned Finished JPEG")


def default_tree():
    """The complete default tree of the pinned fixed recipe, as saved by a
    caller creating a `spektrafilm` step."""
    from film import make_simulator

    _, manifest = make_simulator()
    tree = {key: manifest[group] for group, key in TREE_KEYS.items()}
    tree["output"] = dict(PINNED_OUTPUT)
    return tree


def check_plans(width, height):
    """Recompute the bounded workspace plans before any frame allocation."""
    import numpy as np
    from spektrafilm.utils.bounded_gamut import (
        MAX_WORKSPACE_BYTES,
        plan_gamut_workspace,
    )
    from spektrafilm.utils.bounded_output import (
        JPEG_WORKSPACE_BYTES,
        plan_cctf_workspace,
        plan_jpeg_workspace,
    )

    # A read-only, zero-stride view carries geometry without allocating 24*N.
    shape_only = np.broadcast_to(
        np.zeros((1, 1, 3), dtype=np.float64), (height, width, 3)
    )
    for plan in (
        plan_gamut_workspace(width * height, MAX_WORKSPACE_BYTES),
        plan_cctf_workspace(width * height, MAX_WORKSPACE_BYTES),
        plan_jpeg_workspace(shape_only, JPEG_WORKSPACE_BYTES),
    ):
        if plan.scratch_bytes > plan.workspace_allowance_bytes or plan.batch_pixels <= 0:
            raise Refusal("the pinned workspace does not admit this geometry")


def bounded_frame(pixels, max_edge):
    """Resample the decoded frame to the bounded render geometry.

    The bounded geometry is derived before simulation and preserves aspect.
    The resampler's owned source buffer is freed before the bounded frame is
    returned; the transient decode-and-resample peak is bounded admission
    work the caller covers, and no full-resolution frame is ever handed to
    the simulator.
    """
    import numpy as np
    import OpenImageIO as oiio
    from spektrafilm.utils.bounded_output import samples_are_finite

    if type(max_edge) is not int or not 1 <= max_edge <= MAX_EDGE:
        raise Refusal("the preview geometry bound is invalid")
    height, width, _ = pixels.shape
    scale = min(1.0, max_edge / max(width, height))
    bounded_width = max(1, round(width * scale))
    bounded_height = max(1, round(height * scale))
    if not geometry_bounds(bounded_width, bounded_height):
        raise Refusal("the bounded preview geometry is outside the pinned bounds")
    source = oiio.ImageBuf(pixels)
    del pixels
    bounded = oiio.ImageBuf(
        oiio.ImageSpec(bounded_width, bounded_height, 3, oiio.FLOAT)
    )
    try:
        if not oiio.ImageBufAlgo.resample(bounded, source, interpolate=True):
            raise RuntimeError("the bounded preview geometry could not be rendered")
        frame = bounded.get_pixels(oiio.FLOAT)
    finally:
        del source, bounded
    if (
        type(frame) is not np.ndarray
        or frame.dtype != np.dtype(np.float32)
        or frame.shape != (bounded_height, bounded_width, 3)
        or not frame.flags.c_contiguous
        or not samples_are_finite(frame)
    ):
        raise RuntimeError("the bounded preview frame is not a float32 RGB frame")
    return frame, (bounded_width, bounded_height)


def verify_finished(output_path, width, height):
    """The published artifact must decode as the pinned sRGB finished JPEG."""
    import OpenImageIO as oiio

    metadata = output_path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size == 0:
        raise RuntimeError("finished JPEG was not written")
    reader = oiio.ImageInput.open(str(output_path))
    if reader is None:
        raise RuntimeError("finished JPEG is not readable")
    try:
        spec = reader.spec()
        if not finished_spec_is_pinned(
            reader.format_name(),
            spec,
            spec.getattribute("ICCProfile"),
            width,
            height,
        ):
            raise RuntimeError("finished JPEG misses the pinned output identity")
    finally:
        closed = reader.close()
    if not closed:
        raise RuntimeError("finished JPEG is not readable")


def finished_spec_is_pinned(format_name, spec, profile, width, height):
    """The finished JPEG keeps the input geometry and pins the output sRGB."""
    return (
        format_name == "jpeg"
        and spec.nchannels == 3
        and spec.width == width
        and spec.height == height
        and profile_is_pinned(profile, OUTPUT_ICC_SHA256)
    )


def save_preview_png(output_path, image_data, *, workspace_bytes):
    """Write the bounded display rendition as an 8-bit sRGB PNG."""
    import numpy as np
    import OpenImageIO as oiio

    from spektrafilm.utils.bounded_output import plan_jpeg_workspace
    from spektrafilm.utils.io import _load_icc_profile

    if (type(image_data) is not np.ndarray or image_data.ndim != 3
            or image_data.shape[2] != 3):
        raise ValueError("the bounded preview requires an H x W x 3 array")
    height, width, channels = image_data.shape
    plan_jpeg_workspace(image_data, workspace_bytes)
    profile = _load_icc_profile("sRGB", True)
    if profile is None:
        raise IOError("Pinned sRGB ICC profile is unavailable")
    spec = oiio.ImageSpec(width, height, channels, oiio.TypeDesc("uint8"))
    profile_array = np.frombuffer(profile, dtype=np.uint8)
    spec.attribute(
        "ICCProfile",
        oiio.TypeDesc(f"uint8[{profile_array.size}]"),
        profile_array,
    )
    # The service names the preview destination after its workspace suffix
    # (.tiff), so the encoder must be pinned explicitly: deriving it from
    # the pathname would publish a TIFF where the contract is the bounded
    # sRGB PNG.
    output = oiio.ImageOutput.create("png")
    if not output:
        raise IOError("Could not create the bounded preview: " + str(output_path))
    try:
        if not output.open(str(output_path), spec):
            raise IOError("Could not open the bounded preview: " + output.geterror())
        display = np.clip(image_data, 0.0, 1.0)
        display = (display * 255.0 + 0.5).astype(np.uint8)
        # The pinned runtime's bindings expose the array-only overload;
        # the samples are already the pinned 8-bit values, so they are
        # written exactly as converted.
        if not output.write_image(display):
            raise IOError("Could not write the bounded preview: " + output.geterror())
    except BaseException:
        output.close()
        raise
    if not output.close():
        raise IOError("Could not close the bounded preview: " + output.geterror())


def verify_preview_png(output_path, width, height):
    """The bounded rendition must decode as the pinned sRGB PNG frame."""
    import OpenImageIO as oiio

    metadata = output_path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size == 0:
        raise RuntimeError("bounded preview was not written")
    reader = oiio.ImageInput.open(str(output_path))
    if reader is None:
        raise RuntimeError("bounded preview is not readable")
    try:
        spec = reader.spec()
        if (
            reader.format_name() != "png"
            or spec.nchannels != 3
            or spec.width != width
            or spec.height != height
            or str(spec.format) != "uint8"
            or not profile_is_pinned(
                spec.getattribute("ICCProfile"), OUTPUT_ICC_SHA256
            )
        ):
            raise RuntimeError("bounded preview misses the pinned output identity")
    finally:
        closed = reader.close()
    if not closed:
        raise RuntimeError("bounded preview is not readable")


def load_tree(parameters_path):
    """Read the frozen complete parameter tree of the selected step."""
    try:
        metadata = parameters_path.lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size == 0:
            raise Refusal("the complete parameter tree is missing")
        tree = json.loads(parameters_path.read_text())
    except OSError as error:
        raise Refusal(f"the complete parameter tree is unreadable: {error}") from error
    except ValueError as error:
        raise Refusal(f"the complete parameter tree is not JSON: {error}") from error
    return tree


def simulator_and_recipe():
    """The pinned simulator with its verified fixed-recipe manifest."""
    from film import make_simulator

    simulator, manifest = make_simulator()
    check_recipe(manifest)
    return simulator, manifest


def render_frame(simulator, pixels):
    import film
    from spektrafilm.utils.bounded_output import samples_are_finite

    # Seed NumPy and Numba immediately before the simulation: the fixed
    # recipe is qualified for exactly-once seeding per render, so neither a
    # reused simulator nor a later attempt may inherit earlier random draws.
    film.reset_random_state()
    result = simulator.process(pixels)
    if result.shape != pixels.shape or not samples_are_finite(result):
        raise RuntimeError("film processing returned an invalid frame")
    return result


def produce(input_path, output_path, tree):
    if not check_environment():
        raise Refusal("engine environment is not the pinned deterministic one")

    pixels, (width, height) = development_pixels(input_path)
    from spektrafilm.utils.bounded_output import samples_are_finite

    if not samples_are_finite(pixels):
        raise RuntimeError("development source carries non-finite samples")
    check_plans(width, height)

    simulator, manifest = simulator_and_recipe()
    verify_complete_tree(tree, manifest)
    result = render_frame(simulator, pixels)
    del pixels
    # Python startup ignores SIGXFSZ; restore the kernel's default so a file
    # limit is reported instead of silently truncating the finished artifact.
    import signal

    signal.signal(signal.SIGXFSZ, signal.SIG_DFL)
    from finished_jpeg import save_finished_jpeg

    output_path.parent.mkdir(parents=True, exist_ok=True)
    from spektrafilm.utils.bounded_output import JPEG_WORKSPACE_BYTES

    save_finished_jpeg(str(output_path), result, workspace_bytes=JPEG_WORKSPACE_BYTES)
    verify_finished(output_path, width, height)


def preview(input_path, output_path, tree, max_edge):
    if not check_environment():
        raise Refusal("engine environment is not the pinned deterministic one")

    pixels, _ = development_pixels(input_path)
    frame, (width, height) = bounded_frame(pixels, max_edge)
    del pixels
    check_plans(width, height)

    # The bounded rendition runs the identical fixed recipe: no effect,
    # randomness policy, or quality control may differ from the Export.
    simulator, manifest = simulator_and_recipe()
    verify_complete_tree(tree, manifest)
    result = render_frame(simulator, frame)
    del frame
    import signal

    signal.signal(signal.SIGXFSZ, signal.SIG_DFL)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    from spektrafilm.utils.bounded_output import JPEG_WORKSPACE_BYTES

    save_preview_png(str(output_path), result, workspace_bytes=JPEG_WORKSPACE_BYTES)
    verify_preview_png(output_path, width, height)


def parse_arguments(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--emit-default-parameters", type=Path, default=None)
    subcommands = parser.add_subparsers(dest="command")
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--input", type=Path, required=True)
    common.add_argument("--output", type=Path, required=True)
    common.add_argument("--parameters", type=Path, required=True)
    subcommands.add_parser("produce", parents=[common])
    bounded = subcommands.add_parser("preview", parents=[common])
    bounded.add_argument("--max-edge", type=int, required=True)
    return parser.parse_args(argv)


def main(argv=None):
    arguments = parse_arguments(argv)

    if arguments.emit_default_parameters is not None:
        if arguments.command is not None:
            print("default parameters are emitted without a render", file=sys.stderr)
            return REFUSED
        target = arguments.emit_default_parameters
        if target.exists():
            print(f"refusing to overwrite {target}", file=sys.stderr)
            return REFUSED
        tree = default_tree()
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(tree, sort_keys=True, separators=(",", ":")) + "\n")
        return PRODUCED

    if arguments.command not in ("produce", "preview"):
        print("a render command is required", file=sys.stderr)
        return REFUSED
    input_path = arguments.input
    output_path = arguments.output
    # Refuse before any engine or numerical import: without the retained
    # handoff source, a destination that is not the service's own empty
    # reservation, or the deterministic environment there is no trusted
    # identity, so no purported result is written.
    if not input_path.is_file() or not output_destination_is_available(output_path):
        print(f"refusing to render without the sealed development source: {input_path}",
              file=sys.stderr)
        return REFUSED
    if not check_environment():
        print("refusing to render outside the pinned deterministic engine environment",
              file=sys.stderr)
        return REFUSED
    tree = load_tree(arguments.parameters)
    try:
        if arguments.command == "produce":
            produce(input_path, output_path, tree)
        else:
            preview(input_path, output_path, tree, arguments.max_edge)
    except Refusal as error:
        print(f"refusing the pinned film contract: {error}", file=sys.stderr)
        return REFUSED
    except (RuntimeError, OSError, MemoryError, ValueError) as error:
        print(f"film stage failed: {error}", file=sys.stderr)
        return ENGINE_FAILED
    return PRODUCED


if __name__ == "__main__":
    raise SystemExit(main())
