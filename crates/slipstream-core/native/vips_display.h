#ifndef SLIPSTREAM_VIPS_DISPLAY_H
#define SLIPSTREAM_VIPS_DISPLAY_H

#include <stddef.h>
#include <stdint.h>

#include "vips_preview.h"

#ifdef __cplusplus
extern "C" {
#endif

/// Interleaved float32 RGB planes plus the embedded source profile of the
/// Development TIFF they were decoded from.
typedef struct SlipstreamVipsLinearResult {
  uint32_t width;
  uint32_t height;
  float *pixels;
  size_t length_bytes;
  uint8_t *profile;
  size_t profile_length;
} SlipstreamVipsLinearResult;

/// Decode one float32 RGB TIFF from `fd`, resample it in its own (linear) light
/// to `target_long_edge`, and hand the interleaved planes to the caller.
/// The descriptor is only read; the caller keeps ownership of `fd`.
int32_t slipstream_vips_linear_from_fd(
    int32_t fd, uint32_t target_long_edge, uint64_t maximum_bytes,
    uint64_t maximum_pixels, SlipstreamVipsLinearResult *result) noexcept;

void slipstream_vips_linear_result_free(
    SlipstreamVipsLinearResult *result) noexcept;

/// Encode one interleaved 8-bit sRGB frame as a JPEG carrying `profile`.
int32_t slipstream_vips_encode_srgb8(
    const uint8_t *pixels, uint32_t width, uint32_t height,
    const uint8_t *profile, size_t profile_length,
    uint64_t maximum_output_bytes, SlipstreamVipsResult *result) noexcept;

#ifdef __cplusplus
}
#endif

#endif
