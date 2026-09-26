"""Pinned Finished JPEG encoding for Film qualification and adapter checks."""

from film_identity import FINISHED_JPEG_QUALITY

JPEG_QUALITY = FINISHED_JPEG_QUALITY


def save_finished_jpeg(filename, image_data, *, workspace_bytes):
    """Write encoded sRGB Film pixels with the fixed quality and ICC profile."""
    import numpy as np
    import OpenImageIO as oiio

    from spektrafilm.utils.bounded_output import jpeg_row_batches, plan_jpeg_workspace
    from spektrafilm.utils.io import _load_icc_profile

    if (type(image_data) is not np.ndarray or image_data.ndim != 3
            or image_data.shape[2] != 3):
        raise ValueError("Finished JPEG requires an H x W x 3 array")
    height, width, channels = image_data.shape
    plan = plan_jpeg_workspace(image_data, workspace_bytes)
    profile = _load_icc_profile("sRGB", True)
    if profile is None:
        raise IOError("Pinned sRGB ICC profile is unavailable")
    spec = oiio.ImageSpec(width, height, channels, oiio.TypeDesc("uint8"))
    spec.attribute("Compression", f"jpeg:{JPEG_QUALITY}")
    profile_array = np.frombuffer(profile, dtype=np.uint8)
    spec.attribute(
        "ICCProfile",
        oiio.TypeDesc(f"uint8[{profile_array.size}]"),
        profile_array,
    )
    output = oiio.ImageOutput.create(filename)
    if not output:
        raise IOError("Could not create Finished JPEG: " + filename)
    try:
        if not output.open(filename, spec):
            raise IOError("Could not open Finished JPEG: " + output.geterror())
        for first, last, pixels in jpeg_row_batches(image_data, plan):
            if not output.write_scanlines(first, last, 0, pixels):
                raise IOError("Could not write Finished JPEG: " + output.geterror())
    except BaseException:
        output.close()
        raise
    if not output.close():
        raise IOError("Could not close Finished JPEG: " + output.geterror())
