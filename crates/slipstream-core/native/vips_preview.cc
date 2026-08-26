#include "vips_preview.h"

#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <mutex>

#include <glib.h>
#include <vips/vips.h>

namespace {
constexpr std::uint64_t kMaximumInputBytes = 128ULL * 1024 * 1024;
constexpr std::uint64_t kMaximumPixels = 100ULL * 1000 * 1000;
constexpr std::uint64_t kMaximumOutputBytes = 64ULL * 1024 * 1024;
constexpr int kMaximumCacheMemory = 128 * 1024 * 1024;
constexpr int kMaximumCacheFiles = 32;

std::once_flag lifecycle_once;
int lifecycle_status = SLIPSTREAM_VIPS_INTERNAL_ERROR;

void ResetResult(SlipstreamVipsResult *result) {
  if (result == nullptr) return;
  result->width = 0;
  result->height = 0;
  result->profile = SLIPSTREAM_VIPS_PROFILE_SRGB;
  result->bytes = nullptr;
  result->length = 0;
}

void InitializeOnce() {
  if (vips_init("slipstream") != 0) return;
  vips_concurrency_set(1);
  vips_cache_set_max_mem(kMaximumCacheMemory);
  vips_cache_set_max_files(kMaximumCacheFiles);
  lifecycle_status = SLIPSTREAM_VIPS_OK;
}

bool HasIccProfile(const VipsImage *image) {
  return vips_image_get_typeof(image, VIPS_META_ICC_NAME) != 0;
}

bool IsSaneIccProfile(const VipsImage *image, bool rgb_only) {
  const void *icc = nullptr;
  size_t length = 0;
  if (vips_image_get_blob(image, VIPS_META_ICC_NAME, &icc, &length) != 0 ||
      icc == nullptr || length < 132)
    return false;
  const auto *bytes = static_cast<const std::uint8_t *>(icc);
  const auto declared = (static_cast<std::uint32_t>(bytes[0]) << 24) |
                        (static_cast<std::uint32_t>(bytes[1]) << 16) |
                        (static_cast<std::uint32_t>(bytes[2]) << 8) |
                        static_cast<std::uint32_t>(bytes[3]);
  if (declared != length || std::memcmp(bytes + 36, "acsp", 4) != 0)
    return false;
  if (rgb_only && std::memcmp(bytes + 16, "RGB ", 4) != 0)
    return false;
  const auto tags = (static_cast<std::uint32_t>(bytes[128]) << 24) |
                    (static_cast<std::uint32_t>(bytes[129]) << 16) |
                    (static_cast<std::uint32_t>(bytes[130]) << 8) |
                    static_cast<std::uint32_t>(bytes[131]);
  if (tags > 4096 || 132ULL + static_cast<std::uint64_t>(tags) * 12ULL > length)
    return false;
  for (std::uint32_t index = 0; index < tags; ++index) {
    const auto offset = 132ULL + static_cast<std::uint64_t>(index) * 12ULL;
    const auto data_offset = (static_cast<std::uint32_t>(bytes[offset + 4]) << 24) |
                             (static_cast<std::uint32_t>(bytes[offset + 5]) << 16) |
                             (static_cast<std::uint32_t>(bytes[offset + 6]) << 8) |
                             static_cast<std::uint32_t>(bytes[offset + 7]);
    const auto data_length = (static_cast<std::uint32_t>(bytes[offset + 8]) << 24) |
                             (static_cast<std::uint32_t>(bytes[offset + 9]) << 16) |
                             (static_cast<std::uint32_t>(bytes[offset + 10]) << 8) |
                             static_cast<std::uint32_t>(bytes[offset + 11]);
    if (data_offset > length || data_length > length - data_offset) return false;
  }
  return true;
}

void RemoveOrientation(VipsImage *image) {
  vips_image_remove(image, VIPS_META_ORIENTATION);
  vips_image_remove(image, "exif-ifd0-Orientation");
  vips_image_remove(image, "exif-Orientation");
}

bool PixelCountWithin(const VipsImage *image, std::uint64_t maximum_pixels) {
  const auto width = static_cast<std::uint64_t>(vips_image_get_width(image));
  const auto height = static_cast<std::uint64_t>(vips_image_get_height(image));
  return width > 0 && height > 0 && width <= maximum_pixels / height;
}

SlipstreamVipsStatus ErrorStatus() {
  const char *message = vips_error_buffer();
  if (message != nullptr &&
      (std::strstr(message, "memory") != nullptr ||
       std::strstr(message, "too large") != nullptr ||
       std::strstr(message, "limit") != nullptr))
    return SLIPSTREAM_VIPS_RESOURCE_LIMIT;
  return SLIPSTREAM_VIPS_MALFORMED;
}

SlipstreamVipsStatus CopyResult(VipsImage *image, void *encoded, size_t length,
                                int profile, std::uint64_t maximum_output_bytes,
                                SlipstreamVipsResult *result) {
  if (encoded == nullptr || length == 0) {
    if (encoded != nullptr) g_free(encoded);
    return SLIPSTREAM_VIPS_INTERNAL_ERROR;
  }
  if (static_cast<std::uint64_t>(length) > maximum_output_bytes) {
    g_free(encoded);
    return SLIPSTREAM_VIPS_OUTPUT_LIMIT;
  }
  auto *owned = static_cast<std::uint8_t *>(std::malloc(length));
  if (owned == nullptr) {
    g_free(encoded);
    return SLIPSTREAM_VIPS_RESOURCE_LIMIT;
  }
  std::memcpy(owned, encoded, length);
  g_free(encoded);
  result->width = static_cast<std::uint32_t>(vips_image_get_width(image));
  result->height = static_cast<std::uint32_t>(vips_image_get_height(image));
  result->profile = profile;
  result->bytes = owned;
  result->length = length;
  return SLIPSTREAM_VIPS_OK;
}

}  // namespace

extern "C" int32_t slipstream_vips_initialize() noexcept {
  std::call_once(lifecycle_once, InitializeOnce);
  return lifecycle_status;
}

extern "C" int32_t slipstream_vips_process_jpeg(
    const std::uint8_t *bytes, size_t length, std::uint32_t target_long_edge,
    std::uint64_t maximum_input_bytes, std::uint64_t maximum_pixels,
    std::uint64_t maximum_output_bytes, SlipstreamVipsResult *result) noexcept {
  ResetResult(result);
  if (result == nullptr || bytes == nullptr || length == 0 || target_long_edge == 0 ||
      maximum_input_bytes == 0 || maximum_pixels == 0 || maximum_output_bytes == 0)
    return SLIPSTREAM_VIPS_INTERNAL_ERROR;
  if (static_cast<std::uint64_t>(length) > maximum_input_bytes ||
      static_cast<std::uint64_t>(length) > kMaximumInputBytes)
    return SLIPSTREAM_VIPS_RESOURCE_LIMIT;
  if (slipstream_vips_initialize() != SLIPSTREAM_VIPS_OK)
    return SLIPSTREAM_VIPS_INTERNAL_ERROR;

  vips_error_clear();
  VipsImage *input = nullptr;
  VipsImage *upright = nullptr;
  VipsImage *colored = nullptr;
  VipsImage *resized = nullptr;
  void *encoded = nullptr;
  size_t encoded_length = 0;
  int profile = SLIPSTREAM_VIPS_PROFILE_SRGB;
  auto cleanup = [&]() {
    if (encoded != nullptr) g_free(encoded);
    if (resized != nullptr) g_object_unref(resized);
    if (colored != nullptr) g_object_unref(colored);
    if (upright != nullptr) g_object_unref(upright);
    if (input != nullptr) g_object_unref(input);
  };

  if (vips_jpegload_buffer(const_cast<std::uint8_t *>(bytes), length, &input,
                           "fail_on", VIPS_FAIL_ON_WARNING,
                           "access", VIPS_ACCESS_SEQUENTIAL, nullptr) != 0 ||
      input == nullptr) {
    cleanup();
    return ErrorStatus();
  }
  // Reject claimed dimensions before forcing libvips to materialize the full
  // pixel buffer. This keeps hostile headers from turning copy_memory into an
  // unbounded allocation.
  if (!PixelCountWithin(input, std::min(maximum_pixels, kMaximumPixels))) {
    cleanup();
    return SLIPSTREAM_VIPS_RESOURCE_LIMIT;
  }
  // Force the lazy decoder to consume the complete JPEG before any metadata or
  // derivative is published. This catches truncated entropy data deterministically.
  {
    VipsImage *decoded = vips_image_copy_memory(input);
    if (decoded == nullptr) {
      cleanup();
      return ErrorStatus();
    }
    g_object_unref(input);
    input = decoded;
  }

  if (vips_autorot(input, &upright, nullptr) != 0 || upright == nullptr) {
    cleanup();
    return ErrorStatus();
  }
  RemoveOrientation(upright);
  if (!PixelCountWithin(upright, std::min(maximum_pixels, kMaximumPixels))) {
    cleanup();
    return SLIPSTREAM_VIPS_RESOURCE_LIMIT;
  }

  const bool has_icc = HasIccProfile(upright);
  const bool is_cmyk =
      vips_image_get_interpretation(upright) == VIPS_INTERPRETATION_CMYK;
  const bool compatible_rgb =
      has_icc && vips_image_get_bands(upright) == 3 && !is_cmyk &&
      IsSaneIccProfile(upright, true) &&
      [&]() {
        const void *icc = nullptr;
        size_t icc_length = 0;
        return vips_image_get_blob(upright, VIPS_META_ICC_NAME, &icc, &icc_length) ==
                   0 &&
               vips_icc_is_compatible_profile(upright, icc, icc_length) &&
               vips_icc_transform(upright, &colored, "srgb", "embedded", TRUE,
                                  "depth", 8, nullptr) == 0 &&
               colored != nullptr;
      }();
  if (colored != nullptr) {
    g_object_unref(colored);
    colored = nullptr;
  }

  if (compatible_rgb) {
    colored = static_cast<VipsImage *>(g_object_ref(upright));
    profile = SLIPSTREAM_VIPS_PROFILE_PRESERVED_ICC;
  } else if (is_cmyk && !has_icc) {
    // JPEG CMYK is a valid camera/print interchange format even without an
    // embedded profile. libvips has a deterministic built-in CMYK -> sRGB
    // conversion for this case; ICC transforms cannot infer an input profile.
    if (vips_colourspace(upright, &colored, VIPS_INTERPRETATION_sRGB, nullptr) != 0 ||
        colored == nullptr) {
      cleanup();
      return ErrorStatus();
    }
    profile = SLIPSTREAM_VIPS_PROFILE_SRGB;
  } else {
    const int transform_status = vips_icc_transform(
        upright, &colored, "srgb", "embedded", has_icc,
        "input_profile", has_icc ? nullptr : "srgb", "depth", 8, nullptr);
    if (transform_status != 0 || colored == nullptr) {
      // An invalid profile must never be copied. Treat RGB pixels as sRGB after
      // removing the rejected profile. CMYK with an invalid profile uses the
      // same built-in conversion as unprofiled CMYK.
      if (has_icc) vips_image_remove(upright, VIPS_META_ICC_NAME);
      if (is_cmyk) {
        if (vips_colourspace(upright, &colored, VIPS_INTERPRETATION_sRGB, nullptr) != 0 ||
            colored == nullptr) {
          cleanup();
          return ErrorStatus();
        }
      } else if (vips_image_get_bands(upright) != 3 ||
                 vips_icc_transform(upright, &colored, "srgb", "input_profile", "srgb",
                                    "depth", 8, nullptr) != 0 ||
                 colored == nullptr) {
        cleanup();
        return ErrorStatus();
      }
    }
    profile = SLIPSTREAM_VIPS_PROFILE_SRGB;
  }
  RemoveOrientation(colored);
  if ((profile == SLIPSTREAM_VIPS_PROFILE_PRESERVED_ICC &&
       (!HasIccProfile(colored) || !IsSaneIccProfile(colored, true)))) {
    cleanup();
    return SLIPSTREAM_VIPS_INTERNAL_ERROR;
  }
  if (!PixelCountWithin(colored, std::min(maximum_pixels, kMaximumPixels))) {
    cleanup();
    return SLIPSTREAM_VIPS_RESOURCE_LIMIT;
  }

  const auto long_edge = std::max(vips_image_get_width(colored),
                                  vips_image_get_height(colored));
  const double scale = std::min(1.0, static_cast<double>(target_long_edge) /
                                         static_cast<double>(long_edge));
  if (vips_resize(colored, &resized, scale, "kernel", VIPS_KERNEL_LANCZOS3,
                  nullptr) != 0 || resized == nullptr) {
    cleanup();
    return ErrorStatus();
  }
  RemoveOrientation(resized);
  const auto keep = static_cast<int>(VIPS_FOREIGN_KEEP_ICC);
  if (vips_jpegsave_buffer(resized, &encoded, &encoded_length, "Q", 85,
                           "subsample_mode", VIPS_FOREIGN_SUBSAMPLE_OFF,
                           "keep", keep, nullptr) != 0) {
    cleanup();
    return ErrorStatus();
  }
  const auto status = CopyResult(resized, encoded, encoded_length, profile,
                                 std::min(maximum_output_bytes, kMaximumOutputBytes),
                                 result);
  encoded = nullptr;
  cleanup();
  return status;
}

extern "C" void slipstream_vips_result_free(SlipstreamVipsResult *result) noexcept {
  if (result == nullptr) return;
  std::free(result->bytes);
  ResetResult(result);
}
