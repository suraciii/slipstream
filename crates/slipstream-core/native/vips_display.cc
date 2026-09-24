#include "vips_display.h"

#include <sys/stat.h>
#include <unistd.h>

#include <algorithm>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>

#include <glib.h>
#include <vips/vips.h>

namespace {

// Hard ceilings. A caller may ask for less, never for more.
constexpr std::uint64_t kMaximumInputBytes = 1024ULL * 1024 * 1024;
constexpr std::uint64_t kMaximumPixels = 200ULL * 1000 * 1000;
constexpr std::uint64_t kMaximumOutputBytes = 64ULL * 1024 * 1024;
constexpr int kJpegQuality = 85;

void ResetLinearResult(SlipstreamVipsLinearResult *result) {
  if (result == nullptr) return;
  result->width = 0;
  result->height = 0;
  result->pixels = nullptr;
  result->length_bytes = 0;
  result->profile = nullptr;
  result->profile_length = 0;
}

/// A decode failure is a resource problem only when libvips says so; every
/// other failure is a malformed or unsupported artifact.
int LinearStatus() {
  const char *message = vips_error_buffer();
  if (message != nullptr &&
      (std::strstr(message, "memory") != nullptr ||
       std::strstr(message, "too large") != nullptr ||
       std::strstr(message, "limit") != nullptr))
    return SLIPSTREAM_VIPS_RESOURCE_LIMIT;
  return SLIPSTREAM_VIPS_MALFORMED;
}

bool PixelCountWithin(const VipsImage *image, std::uint64_t maximum_pixels) {
  if (image == nullptr) return false;
  const auto width = static_cast<std::uint64_t>(vips_image_get_width(image));
  const auto height = static_cast<std::uint64_t>(vips_image_get_height(image));
  if (width == 0 || height == 0) return false;
  if (width > std::numeric_limits<std::uint64_t>::max() / height) return false;
  return width * height <= maximum_pixels;
}

std::uint8_t *CopyBytes(const void *bytes, size_t length) {
  if (bytes == nullptr || length == 0) return nullptr;
  auto *owned = static_cast<std::uint8_t *>(std::malloc(length));
  if (owned == nullptr) return nullptr;
  std::memcpy(owned, bytes, length);
  return owned;
}

/// Copies an encoded frame into the shared result shape. `encoded` stays owned
/// by the caller of this function.
int CopyEncoded(VipsImage *image, void *encoded, size_t length,
                std::uint64_t maximum_output_bytes,
                SlipstreamVipsResult *result) {
  const auto bounded =
      std::min<std::uint64_t>(maximum_output_bytes, kMaximumOutputBytes);
  if (encoded == nullptr || length == 0)
    return SLIPSTREAM_VIPS_INTERNAL_ERROR;
  if (static_cast<std::uint64_t>(length) > bounded)
    return SLIPSTREAM_VIPS_OUTPUT_LIMIT;
  auto *owned = CopyBytes(encoded, length);
  if (owned == nullptr) return SLIPSTREAM_VIPS_RESOURCE_LIMIT;
  result->width = static_cast<std::uint32_t>(vips_image_get_width(image));
  result->height = static_cast<std::uint32_t>(vips_image_get_height(image));
  // The derivative is the fixed sRGB display conversion; the caller embeds
  // the pinned sRGB profile bytes, so the frame is self-describing.
  result->profile = SLIPSTREAM_VIPS_PROFILE_SRGB;
  result->bytes = owned;
  result->length = length;
  return SLIPSTREAM_VIPS_OK;
}

}  // namespace

extern "C" int32_t slipstream_vips_linear_from_fd(
    int32_t fd, std::uint32_t target_long_edge, std::uint64_t maximum_bytes,
    std::uint64_t maximum_pixels, SlipstreamVipsLinearResult *result) noexcept {
  ResetLinearResult(result);
  if (result == nullptr || fd < 0 || target_long_edge == 0 ||
      maximum_bytes == 0 || maximum_pixels == 0)
    return SLIPSTREAM_VIPS_INTERNAL_ERROR;

  struct stat status = {};
  if (::fstat(fd, &status) != 0) return SLIPSTREAM_VIPS_INTERNAL_ERROR;
  if (!S_ISREG(status.st_mode)) return SLIPSTREAM_VIPS_UNSUPPORTED;
  const auto declared = static_cast<std::uint64_t>(status.st_size);
  const auto bounded_bytes =
      std::min<std::uint64_t>(maximum_bytes, kMaximumInputBytes);
  if (declared == 0 || declared > bounded_bytes)
    return SLIPSTREAM_VIPS_RESOURCE_LIMIT;
  if (slipstream_vips_initialize() != SLIPSTREAM_VIPS_OK)
    return SLIPSTREAM_VIPS_INTERNAL_ERROR;

  vips_error_clear();
  // libvips loads the TIFF through its own descriptor, so hand it the procfs
  // path of the descriptor it was given. The caller keeps ownership of `fd`.
  char path[64] = {};
  const int written = std::snprintf(path, sizeof(path), "/proc/self/fd/%d", fd);
  if (written <= 0 || static_cast<size_t>(written) >= sizeof(path))
    return SLIPSTREAM_VIPS_INTERNAL_ERROR;
  VipsImage *input = vips_image_new_from_file(
      path, "access", VIPS_ACCESS_SEQUENTIAL, "fail_on", VIPS_FAIL_ON_WARNING,
      nullptr);
  if (input == nullptr) return LinearStatus();
  VipsImage *resized = nullptr;

  const auto pixel_bound =
      std::min<std::uint64_t>(maximum_pixels, kMaximumPixels);
  const auto fail = [&](int code) {
    if (resized != nullptr) g_object_unref(resized);
    g_object_unref(input);
    return code;
  };

  if (vips_image_get_format(input) != VIPS_FORMAT_FLOAT ||
      vips_image_get_bands(input) != 3) {
    return fail(SLIPSTREAM_VIPS_MALFORMED);
  }
  if (!PixelCountWithin(input, pixel_bound))
    return fail(SLIPSTREAM_VIPS_RESOURCE_LIMIT);
  const void *embedded = nullptr;
  size_t embedded_length = 0;
  if (vips_image_get_blob(input, VIPS_META_ICC_NAME, &embedded,
                          &embedded_length) != 0 ||
      embedded == nullptr || embedded_length == 0) {
    return fail(SLIPSTREAM_VIPS_MALFORMED);
  }
  auto *profile = CopyBytes(embedded, embedded_length);
  if (profile == nullptr) return fail(SLIPSTREAM_VIPS_RESOURCE_LIMIT);

  // The samples are linear light, so resampling them directly is the display
  // resample; no transfer function is applied before or after it.
  const auto long_edge =
      std::max(vips_image_get_width(input), vips_image_get_height(input));
  const double scale =
      std::min(1.0, static_cast<double>(target_long_edge) /
                        static_cast<double>(long_edge));
  if (vips_resize(input, &resized, scale, "kernel", VIPS_KERNEL_LANCZOS3,
                  nullptr) != 0 ||
      resized == nullptr) {
    std::free(profile);
    return fail(LinearStatus());
  }
  if (!PixelCountWithin(resized, pixel_bound)) {
    std::free(profile);
    return fail(SLIPSTREAM_VIPS_RESOURCE_LIMIT);
  }

  size_t length = 0;
  void *memory = vips_image_write_to_memory(resized, &length);
  if (memory == nullptr || length == 0) {
    if (memory != nullptr) g_free(memory);
    std::free(profile);
    return fail(LinearStatus());
  }
  const auto expected = static_cast<std::uint64_t>(vips_image_get_width(resized)) *
                        static_cast<std::uint64_t>(vips_image_get_height(resized)) * 3ULL *
                        sizeof(float);
  if (static_cast<std::uint64_t>(length) != expected) {
    g_free(memory);
    std::free(profile);
    return fail(SLIPSTREAM_VIPS_INTERNAL_ERROR);
  }
  auto *planes = static_cast<float *>(std::malloc(length));
  if (planes == nullptr) {
    g_free(memory);
    std::free(profile);
    return fail(SLIPSTREAM_VIPS_RESOURCE_LIMIT);
  }
  std::memcpy(planes, memory, length);
  g_free(memory);

  result->width = static_cast<std::uint32_t>(vips_image_get_width(resized));
  result->height = static_cast<std::uint32_t>(vips_image_get_height(resized));
  result->pixels = planes;
  result->length_bytes = length;
  result->profile = profile;
  result->profile_length = embedded_length;

  g_object_unref(resized);
  g_object_unref(input);
  return SLIPSTREAM_VIPS_OK;
}

extern "C" void slipstream_vips_linear_result_free(
    SlipstreamVipsLinearResult *result) noexcept {
  if (result == nullptr) return;
  std::free(result->pixels);
  std::free(result->profile);
  ResetLinearResult(result);
}

extern "C" int32_t slipstream_vips_encode_srgb8(
    const std::uint8_t *pixels, std::uint32_t width, std::uint32_t height,
    const std::uint8_t *profile, size_t profile_length,
    std::uint64_t maximum_output_bytes, SlipstreamVipsResult *result) noexcept {
  if (result == nullptr) return SLIPSTREAM_VIPS_INTERNAL_ERROR;
  result->width = 0;
  result->height = 0;
  result->profile = SLIPSTREAM_VIPS_PROFILE_SRGB;
  result->bytes = nullptr;
  result->length = 0;
  if (pixels == nullptr || width == 0 || height == 0 || profile == nullptr ||
      profile_length == 0 || maximum_output_bytes == 0)
    return SLIPSTREAM_VIPS_INTERNAL_ERROR;
  const auto pixel_count =
      static_cast<std::uint64_t>(width) * static_cast<std::uint64_t>(height);
  if (pixel_count == 0 || pixel_count > kMaximumPixels)
    return SLIPSTREAM_VIPS_RESOURCE_LIMIT;
  const auto count = pixel_count * 3ULL;
  if (slipstream_vips_initialize() != SLIPSTREAM_VIPS_OK)
    return SLIPSTREAM_VIPS_INTERNAL_ERROR;

  vips_error_clear();
  VipsImage *image = vips_image_new_from_memory_copy(
      pixels, static_cast<size_t>(count), width, height, 3, VIPS_FORMAT_UCHAR);
  if (image == nullptr) return LinearStatus();
  void *encoded = nullptr;
  size_t encoded_length = 0;
  vips_image_set_blob_copy(image, VIPS_META_ICC_NAME, profile, profile_length);
  if (vips_jpegsave_buffer(image, &encoded, &encoded_length, "Q", kJpegQuality,
                           "subsample_mode", VIPS_FOREIGN_SUBSAMPLE_OFF, "keep",
                           VIPS_FOREIGN_KEEP_ICC, nullptr) != 0) {
    if (encoded != nullptr) g_free(encoded);
    g_object_unref(image);
    return LinearStatus();
  }
  const int status =
      CopyEncoded(image, encoded, encoded_length, maximum_output_bytes, result);
  if (encoded != nullptr) g_free(encoded);
  g_object_unref(image);
  return status;
}
