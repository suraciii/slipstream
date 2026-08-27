#include "raw_preview.h"

#include <algorithm>
#include <csetjmp>
#include <cstdint>
#include <ctime>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <memory>
#include <string>
#include <vector>

#include <jpeglib.h>
#include <libraw/libraw.h>

#ifdef __linux__
#include <cerrno>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

namespace {
constexpr std::uint64_t kMaximumJpegBytes = 128ULL * 1024 * 1024;
constexpr std::uint64_t kMaximumPixels = 100ULL * 1000 * 1000;
constexpr unsigned kMaximumLibRawMemoryMb = 256;

struct JpegErrorManager {
  jpeg_error_mgr base;
  std::jmp_buf jump;
};

void JpegErrorExit(j_common_ptr info) {
  auto *error = reinterpret_cast<JpegErrorManager *>(info->err);
  std::longjmp(error->jump, 1);
}

void JpegEmitMessage(j_common_ptr info, int message_level) {
  if (message_level < 0) JpegErrorExit(info);
}

bool DecodeJpeg(const std::uint8_t *bytes, std::uint64_t length,
                std::uint64_t maximum_bytes, std::uint64_t maximum_pixels,
                std::uint32_t &width, std::uint32_t &height) {
  if (bytes == nullptr || length == 0 || length > maximum_bytes ||
      length > std::numeric_limits<unsigned long>::max()) {
    return false;
  }

  jpeg_decompress_struct decoder{};
  JpegErrorManager error{};
  bool created = false;
  decoder.err = jpeg_std_error(&error.base);
  error.base.error_exit = JpegErrorExit;
  error.base.emit_message = JpegEmitMessage;
  if (setjmp(error.jump) != 0) {
    if (created) jpeg_destroy_decompress(&decoder);
    return false;
  }

  jpeg_create_decompress(&decoder);
  created = true;
  jpeg_mem_src(&decoder, const_cast<unsigned char *>(bytes),
               static_cast<unsigned long>(length));
  if (jpeg_read_header(&decoder, TRUE) != JPEG_HEADER_OK) {
    jpeg_destroy_decompress(&decoder);
    return false;
  }

  const auto pixels = static_cast<std::uint64_t>(decoder.image_width) *
                      static_cast<std::uint64_t>(decoder.image_height);
  if (decoder.image_width == 0 || decoder.image_height == 0 ||
      pixels > maximum_pixels) {
    jpeg_destroy_decompress(&decoder);
    return false;
  }

  decoder.out_color_space = JCS_RGB;
  if (jpeg_start_decompress(&decoder) == FALSE ||
      decoder.output_components != 3) {
    jpeg_destroy_decompress(&decoder);
    return false;
  }
  const auto row_bytes = static_cast<std::uint64_t>(decoder.output_width) *
                         static_cast<std::uint64_t>(decoder.output_components);
  if (row_bytes == 0 || row_bytes > maximum_bytes ||
      row_bytes > std::numeric_limits<JDIMENSION>::max()) {
    jpeg_destroy_decompress(&decoder);
    return false;
  }
  auto rows = (*decoder.mem->alloc_sarray)(
      reinterpret_cast<j_common_ptr>(&decoder), JPOOL_IMAGE,
      static_cast<JDIMENSION>(row_bytes), 1);
  while (decoder.output_scanline < decoder.output_height) {
    if (jpeg_read_scanlines(&decoder, rows, 1) != 1) {
      jpeg_destroy_decompress(&decoder);
      return false;
    }
  }
  if (jpeg_finish_decompress(&decoder) == FALSE) {
    jpeg_destroy_decompress(&decoder);
    return false;
  }

  width = decoder.output_width;
  height = decoder.output_height;
  jpeg_destroy_decompress(&decoder);
  return true;
}

SlipstreamPreviewStatus ReadDescriptor(int fd, std::uint64_t maximum_bytes,
                                       std::vector<std::uint8_t> &bytes) {
#ifdef __linux__
  const int duplicate = fcntl(fd, F_DUPFD_CLOEXEC, 0);
  if (duplicate < 0) return SLIPSTREAM_PREVIEW_IO_ERROR;
  struct stat facts {};
  const bool valid = fstat(duplicate, &facts) == 0 && S_ISREG(facts.st_mode) &&
                     facts.st_size >= 0;
  if (!valid) {
    close(duplicate);
    return SLIPSTREAM_PREVIEW_IO_ERROR;
  }
  const auto size = static_cast<std::uint64_t>(facts.st_size);
  if (size == 0 || size > maximum_bytes ||
      size > std::numeric_limits<std::size_t>::max()) {
    close(duplicate);
    return size > maximum_bytes ? SLIPSTREAM_PREVIEW_RESOURCE_LIMIT
                                : SLIPSTREAM_PREVIEW_MALFORMED;
  }
  try {
    bytes.resize(static_cast<std::size_t>(size));
  } catch (...) {
    close(duplicate);
    return SLIPSTREAM_PREVIEW_RESOURCE_LIMIT;
  }
  std::size_t consumed = 0;
  while (consumed < bytes.size()) {
    const auto count = pread(duplicate, bytes.data() + consumed,
                             bytes.size() - consumed,
                             static_cast<off_t>(consumed));
    if (count <= 0) {
      close(duplicate);
      bytes.clear();
      return SLIPSTREAM_PREVIEW_IO_ERROR;
    }
    consumed += static_cast<std::size_t>(count);
  }
  close(duplicate);
  return SLIPSTREAM_PREVIEW_OK;
#else
  (void)fd;
  (void)maximum_bytes;
  (void)bytes;
  return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
#endif
}

void ResetResult(SlipstreamPreviewResult *result) {
  if (result == nullptr) return;
  result->candidate_index = -1;
  result->width = 0;
  result->height = 0;
  result->bytes = nullptr;
  result->length = 0;
}

SlipstreamPreviewStatus CopyResult(const std::vector<std::uint8_t> &bytes,
                                   std::uint32_t width, std::uint32_t height,
                                   int candidate,
                                   SlipstreamPreviewResult *result) {
  if (bytes.empty() || bytes.size() > std::numeric_limits<std::uint64_t>::max()) {
    return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
  }
  auto *owned = static_cast<std::uint8_t *>(std::malloc(bytes.size()));
  if (owned == nullptr) return SLIPSTREAM_PREVIEW_RESOURCE_LIMIT;
  std::memcpy(owned, bytes.data(), bytes.size());
  result->candidate_index = candidate;
  result->width = width;
  result->height = height;
  result->bytes = owned;
  result->length = bytes.size();
  return SLIPSTREAM_PREVIEW_OK;
}

const char *ProcFdPath(int fd) {
  static thread_local std::string path;
  path = "/proc/self/fd/" + std::to_string(fd);
  return path.c_str();
}

const char *NativeErrorKind(int error) {
  switch (error) {
  case LIBRAW_FILE_UNSUPPORTED:
    return "unsupported";
  case LIBRAW_IO_ERROR:
    return "io-error";
  case LIBRAW_DATA_ERROR:
  case LIBRAW_REQUEST_FOR_NONEXISTENT_THUMBNAIL:
    return "malformed";
  case LIBRAW_UNSUFFICIENT_MEMORY:
  case LIBRAW_TOO_BIG:
  case LIBRAW_MEMPOOL_OVERFLOW:
    return "resource-limit";
  default:
    return "internal-error";
  }
}

SlipstreamPreviewStatus StatusForLibRawError(int error) {
  const auto kind = NativeErrorKind(error);
  if (std::strcmp(kind, "unsupported") == 0)
    return SLIPSTREAM_PREVIEW_UNSUPPORTED;
  if (std::strcmp(kind, "io-error") == 0)
    return SLIPSTREAM_PREVIEW_IO_ERROR;
  if (std::strcmp(kind, "malformed") == 0)
    return SLIPSTREAM_PREVIEW_MALFORMED;
  if (std::strcmp(kind, "resource-limit") == 0)
    return SLIPSTREAM_PREVIEW_RESOURCE_LIMIT;
  return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
}

struct LibRawCloser {
  void operator()(libraw_data_t *raw) const {
    if (raw != nullptr) libraw_close(raw);
  }
};
using LibRawHandle = std::unique_ptr<libraw_data_t, LibRawCloser>;

struct Selection {
  int index = -1;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  std::vector<std::uint8_t> bytes;
};

bool ConsiderCandidate(Selection &selection, int index,
                       const unsigned char *bytes, std::size_t length,
                       std::uint64_t maximum_bytes,
                       std::uint64_t maximum_pixels) {
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  if (!DecodeJpeg(bytes, length, maximum_bytes, maximum_pixels, width, height))
    return false;
  const auto area = static_cast<std::uint64_t>(width) * height;
  const auto selected_area = static_cast<std::uint64_t>(selection.width) *
                             selection.height;
  if (selection.index >= 0 && area <= selected_area) return true;
  selection.index = index;
  selection.width = width;
  selection.height = height;
  selection.bytes.assign(bytes, bytes + length);
  return true;
}

} // namespace

extern "C" int32_t slipstream_inspect_jpeg_fd(
    int fd, std::uint64_t maximum_bytes, std::uint64_t maximum_pixels,
    SlipstreamPreviewResult *result) noexcept {
  ResetResult(result);
  if (result == nullptr || fd < 0 || maximum_bytes == 0 || maximum_pixels == 0)
    return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
  try {
    std::vector<std::uint8_t> bytes;
    const auto read_status = ReadDescriptor(fd, maximum_bytes, bytes);
    if (read_status != SLIPSTREAM_PREVIEW_OK) return read_status;
    std::uint32_t width = 0;
    std::uint32_t height = 0;
    if (!DecodeJpeg(bytes.data(), bytes.size(), maximum_bytes, maximum_pixels,
                    width, height))
      return SLIPSTREAM_PREVIEW_MALFORMED;
    return CopyResult(bytes, width, height, -1, result);
  } catch (const std::bad_alloc &) {
    return SLIPSTREAM_PREVIEW_RESOURCE_LIMIT;
  } catch (...) {
    return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
  }
}

extern "C" int32_t slipstream_extract_embedded_jpeg_fd(
    int fd, std::uint64_t maximum_jpeg_bytes, std::uint64_t maximum_pixels,
    std::uint32_t maximum_libraw_memory_mb,
    SlipstreamPreviewResult *result) noexcept {
  ResetResult(result);
  if (result == nullptr || fd < 0 || maximum_jpeg_bytes == 0 ||
      maximum_pixels == 0 || maximum_libraw_memory_mb == 0)
    return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
#ifdef __linux__
  int duplicate = -1;
  try {
    duplicate = fcntl(fd, F_DUPFD_CLOEXEC, 0);
    if (duplicate < 0) return SLIPSTREAM_PREVIEW_IO_ERROR;
    struct stat facts {};
    if (fstat(duplicate, &facts) != 0 || !S_ISREG(facts.st_mode)) {
      close(duplicate);
      return SLIPSTREAM_PREVIEW_IO_ERROR;
    }

    LibRawHandle raw(libraw_init(0));
    if (!raw) {
      close(duplicate);
      return SLIPSTREAM_PREVIEW_RESOURCE_LIMIT;
    }
    raw->rawparams.max_raw_memory_mb = maximum_libraw_memory_mb;
    const int open_error = libraw_open_file(raw.get(), ProcFdPath(duplicate));
    if (open_error != LIBRAW_SUCCESS) {
      close(duplicate);
      return StatusForLibRawError(open_error);
    }

    Selection selection;
    SlipstreamPreviewStatus strongest_failure = SLIPSTREAM_PREVIEW_NO_USABLE_PREVIEW;
    const int count = std::clamp(raw->thumbs_list.thumbcount, 0,
                                 LIBRAW_THUMBNAIL_MAXCOUNT);
    for (int index = 0; index < count; ++index) {
      const auto &candidate = raw->thumbs_list.thumblist[index];
      if (candidate.tlength == 0 ||
          static_cast<std::uint64_t>(candidate.tlength) > maximum_jpeg_bytes)
        continue;
      // Deliberately call only thumbnail extraction. Sensor unpacking and RAW
      // development are outside Slipstream's Preview trust boundary.
      const int unpack_error = libraw_unpack_thumb_ex(raw.get(), index);
      if (unpack_error != LIBRAW_SUCCESS) {
        const auto status = StatusForLibRawError(unpack_error);
        if (status == SLIPSTREAM_PREVIEW_RESOURCE_LIMIT ||
            status == SLIPSTREAM_PREVIEW_IO_ERROR ||
            status == SLIPSTREAM_PREVIEW_INTERNAL_ERROR)
          strongest_failure = status;
        continue;
      }
      if (raw->thumbnail.tformat != LIBRAW_THUMBNAIL_JPEG ||
          raw->thumbnail.thumb == nullptr || raw->thumbnail.tlength == 0 ||
          static_cast<std::uint64_t>(raw->thumbnail.tlength) > maximum_jpeg_bytes)
        continue;
      const bool usable = ConsiderCandidate(
          selection, index,
          reinterpret_cast<const unsigned char *>(raw->thumbnail.thumb),
          raw->thumbnail.tlength, maximum_jpeg_bytes, maximum_pixels);
      if (!usable) {
        // A malformed candidate is intentionally ignored so a smaller valid
        // candidate can still represent the RAW.
      }
    }
    close(duplicate);
    if (selection.index < 0) {
      if (strongest_failure != SLIPSTREAM_PREVIEW_NO_USABLE_PREVIEW)
        return strongest_failure;
      return SLIPSTREAM_PREVIEW_NO_USABLE_PREVIEW;
    }
    return CopyResult(selection.bytes, selection.width, selection.height,
                      selection.index, result);
  } catch (const std::bad_alloc &) {
    if (duplicate >= 0) close(duplicate);
    return SLIPSTREAM_PREVIEW_RESOURCE_LIMIT;
  } catch (...) {
    if (duplicate >= 0) close(duplicate);
    return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
  }
#else
  (void)fd;
  (void)maximum_jpeg_bytes;
  (void)maximum_pixels;
  (void)maximum_libraw_memory_mb;
  return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
#endif
}

extern "C" int32_t slipstream_inspect_raw_capture_time_fd(
    int fd, std::uint32_t maximum_libraw_memory_mb,
    SlipstreamCaptureTimeResult *result) noexcept {
  if (result == nullptr) return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
  *result = {};
  if (fd < 0 || maximum_libraw_memory_mb == 0)
    return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
#ifdef __linux__
  int duplicate = -1;
  try {
    duplicate = fcntl(fd, F_DUPFD_CLOEXEC, 0);
    if (duplicate < 0) return SLIPSTREAM_PREVIEW_IO_ERROR;
    struct stat facts {};
    if (fstat(duplicate, &facts) != 0 || !S_ISREG(facts.st_mode)) {
      close(duplicate);
      return SLIPSTREAM_PREVIEW_IO_ERROR;
    }
    LibRawHandle raw(libraw_init(0));
    if (!raw) {
      close(duplicate);
      return SLIPSTREAM_PREVIEW_RESOURCE_LIMIT;
    }
    raw->rawparams.max_raw_memory_mb = maximum_libraw_memory_mb;
    // This is intentionally metadata open only. Do not call unpack,
    // unpack_thumb, dcraw_process, or any sensor-development API here.
    const int open_error = libraw_open_file(raw.get(), ProcFdPath(duplicate));
    close(duplicate);
    duplicate = -1;
    if (open_error != LIBRAW_SUCCESS) return StatusForLibRawError(open_error);
    const auto timestamp = raw->other.timestamp;
    if (timestamp <= 0) return SLIPSTREAM_PREVIEW_OK;
    std::tm camera_local {};
    if (localtime_r(&timestamp, &camera_local) == nullptr)
      return SLIPSTREAM_PREVIEW_MALFORMED;
    const int year = camera_local.tm_year + 1900;
    if (year <= 0 || camera_local.tm_mon < 0 || camera_local.tm_mon > 11 ||
        camera_local.tm_mday < 1 || camera_local.tm_mday > 31 ||
        camera_local.tm_hour < 0 || camera_local.tm_hour > 23 ||
        camera_local.tm_min < 0 || camera_local.tm_min > 59 ||
        camera_local.tm_sec < 0 || camera_local.tm_sec > 59)
      return SLIPSTREAM_PREVIEW_MALFORMED;
    result->has_timestamp = 1;
    result->year = year;
    result->month = camera_local.tm_mon + 1;
    result->day = camera_local.tm_mday;
    result->hour = camera_local.tm_hour;
    result->minute = camera_local.tm_min;
    result->second = camera_local.tm_sec;
    return SLIPSTREAM_PREVIEW_OK;
  } catch (const std::bad_alloc &) {
    if (duplicate >= 0) close(duplicate);
    return SLIPSTREAM_PREVIEW_RESOURCE_LIMIT;
  } catch (...) {
    if (duplicate >= 0) close(duplicate);
    return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
  }
#else
  (void)fd;
  (void)maximum_libraw_memory_mb;
  return SLIPSTREAM_PREVIEW_INTERNAL_ERROR;
#endif
}

extern "C" void slipstream_preview_result_free(
    SlipstreamPreviewResult *result) noexcept {
  if (result == nullptr) return;
  std::free(result->bytes);
  ResetResult(result);
}
