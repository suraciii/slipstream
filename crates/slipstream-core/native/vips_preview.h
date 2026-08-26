#ifndef SLIPSTREAM_VIPS_PREVIEW_H
#define SLIPSTREAM_VIPS_PREVIEW_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

enum SlipstreamVipsStatus {
  SLIPSTREAM_VIPS_OK = 0,
  SLIPSTREAM_VIPS_UNSUPPORTED = 1,
  SLIPSTREAM_VIPS_MALFORMED = 2,
  SLIPSTREAM_VIPS_RESOURCE_LIMIT = 3,
  SLIPSTREAM_VIPS_INTERNAL_ERROR = 4,
  SLIPSTREAM_VIPS_OUTPUT_LIMIT = 5,
};

enum SlipstreamVipsProfile {
  SLIPSTREAM_VIPS_PROFILE_SRGB = 0,
  SLIPSTREAM_VIPS_PROFILE_PRESERVED_ICC = 1,
};

typedef struct SlipstreamVipsResult {
  uint32_t width;
  uint32_t height;
  int32_t profile;
  uint8_t *bytes;
  uint64_t length;
} SlipstreamVipsResult;

int32_t slipstream_vips_initialize(void) noexcept;
int32_t slipstream_vips_process_jpeg(
    const uint8_t *bytes, size_t length, uint32_t target_long_edge,
    uint64_t maximum_input_bytes, uint64_t maximum_pixels,
    uint64_t maximum_output_bytes, SlipstreamVipsResult *result) noexcept;
void slipstream_vips_result_free(SlipstreamVipsResult *result) noexcept;

#ifdef __cplusplus
}
#endif

#endif
