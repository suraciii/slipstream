#ifndef SLIPSTREAM_RAW_PREVIEW_H
#define SLIPSTREAM_RAW_PREVIEW_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum SlipstreamPreviewStatus {
  SLIPSTREAM_PREVIEW_OK = 0,
  SLIPSTREAM_PREVIEW_UNSUPPORTED = 1,
  SLIPSTREAM_PREVIEW_MALFORMED = 2,
  SLIPSTREAM_PREVIEW_IO_ERROR = 3,
  SLIPSTREAM_PREVIEW_RESOURCE_LIMIT = 4,
  SLIPSTREAM_PREVIEW_INTERNAL_ERROR = 5,
  SLIPSTREAM_PREVIEW_NO_USABLE_PREVIEW = 6,
};

typedef struct SlipstreamPreviewResult {
  int32_t candidate_index;
  uint32_t width;
  uint32_t height;
  uint8_t *bytes;
  uint64_t length;
} SlipstreamPreviewResult;

int32_t slipstream_inspect_jpeg_fd(int fd, uint64_t maximum_bytes,
                                   uint64_t maximum_pixels,
                                   SlipstreamPreviewResult *result) noexcept;
int32_t slipstream_extract_embedded_jpeg_fd(
    int fd, uint64_t maximum_jpeg_bytes, uint64_t maximum_pixels,
    uint32_t maximum_libraw_memory_mb,
    SlipstreamPreviewResult *result) noexcept;
void slipstream_preview_result_free(SlipstreamPreviewResult *result) noexcept;

#ifdef __cplusplus
}
#endif

#endif
