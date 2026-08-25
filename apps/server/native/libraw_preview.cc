#include <napi.h>
#include <jpeglib.h>
#include <libraw/libraw.h>

#include <algorithm>
#include <csetjmp>
#include <cstdint>
#include <cstdlib>
#include <exception>
#include <limits>
#include <memory>
#include <new>
#include <string>
#include <vector>

namespace {
constexpr std::size_t kMaximumJpegBytes = 128 * 1024 * 1024;
constexpr std::uint64_t kMaximumPixels = 100'000'000;
constexpr unsigned kMaximumLibRawMemoryMb = 256;
#ifdef SLIPSTREAM_TEST_ADDON
constexpr std::uint64_t kMaximumTestPixels = 16'000'000;
#endif

struct LibRawCloser {
  void operator()(libraw_data_t *raw) const {
    if (raw != nullptr) {
      libraw_close(raw);
    }
  }
};
using LibRawHandle = std::unique_ptr<libraw_data_t, LibRawCloser>;

struct JpegDimensions {
  std::uint32_t width;
  std::uint32_t height;
};

struct JpegErrorManager {
  jpeg_error_mgr base;
  std::jmp_buf jump;
};

void JpegErrorExit(j_common_ptr info) {
  auto *error = reinterpret_cast<JpegErrorManager *>(info->err);
  std::longjmp(error->jump, 1);
}

void JpegEmitMessage(j_common_ptr info, int message_level) {
  if (message_level < 0) {
    JpegErrorExit(info);
  }
}

bool DecodeJpeg(const unsigned char *bytes, std::size_t length, JpegDimensions &dimensions) {
  if (bytes == nullptr || length == 0 || length > kMaximumJpegBytes ||
      length > std::numeric_limits<unsigned long>::max()) {
    return false;
  }

  jpeg_decompress_struct decoder{};
  JpegErrorManager error{};
  decoder.err = jpeg_std_error(&error.base);
  error.base.error_exit = JpegErrorExit;
  error.base.emit_message = JpegEmitMessage;
  if (setjmp(error.jump) != 0) {
    jpeg_destroy_decompress(&decoder);
    return false;
  }

  jpeg_create_decompress(&decoder);
  jpeg_mem_src(&decoder, bytes, static_cast<unsigned long>(length));
  if (jpeg_read_header(&decoder, TRUE) != JPEG_HEADER_OK) {
    jpeg_destroy_decompress(&decoder);
    return false;
  }

  const auto pixels = static_cast<std::uint64_t>(decoder.image_width) * decoder.image_height;
  if (decoder.image_width == 0 || decoder.image_height == 0 || pixels > kMaximumPixels) {
    jpeg_destroy_decompress(&decoder);
    return false;
  }

  decoder.out_color_space = JCS_RGB;
  if (jpeg_start_decompress(&decoder) == FALSE || decoder.output_components != 3) {
    jpeg_destroy_decompress(&decoder);
    return false;
  }

  const auto row_bytes = static_cast<std::uint64_t>(decoder.output_width) * decoder.output_components;
  if (row_bytes == 0 || row_bytes > kMaximumJpegBytes) {
    jpeg_destroy_decompress(&decoder);
    return false;
  }
  auto rows = (*decoder.mem->alloc_sarray)(reinterpret_cast<j_common_ptr>(&decoder), JPOOL_IMAGE,
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

  dimensions = {decoder.output_width, decoder.output_height};
  jpeg_destroy_decompress(&decoder);
  return true;
}

Napi::Object Outcome(Napi::Env env, const char *kind, const std::string &message) {
  auto result = Napi::Object::New(env);
  result.Set("kind", kind);
  result.Set("message", message);
  return result;
}

const char *ErrorKind(int error) {
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

struct ActionableFailure {
  int rank = 0;
  const char *kind = nullptr;
  std::string message;
};

void RecordUnpackFailure(ActionableFailure &failure, int error) {
  if (error == LIBRAW_SUCCESS || error == LIBRAW_DATA_ERROR ||
      error == LIBRAW_REQUEST_FOR_NONEXISTENT_THUMBNAIL) {
    return;
  }
  const char *kind = ErrorKind(error);
  const int rank = std::string(kind) == "resource-limit" ? 3 :
                   std::string(kind) == "io-error" ? 2 : 1;
  if (rank > failure.rank) {
    failure.rank = rank;
    failure.kind = kind;
    failure.message = libraw_strerror(error);
  }
}

struct Selection {
  int index = -1;
  JpegDimensions dimensions{};
  std::vector<unsigned char> bytes;
};

void ConsiderCandidate(Selection &selection, int index, const unsigned char *bytes, std::size_t length) {
  JpegDimensions dimensions{};
  if (!DecodeJpeg(bytes, length, dimensions)) {
    return;
  }
  const auto area = static_cast<std::uint64_t>(dimensions.width) * dimensions.height;
  const auto selected_area =
      static_cast<std::uint64_t>(selection.dimensions.width) * selection.dimensions.height;
  if (selection.index < 0 || area > selected_area) {
    selection.index = index;
    selection.dimensions = dimensions;
    selection.bytes.assign(bytes, bytes + length);
  }
}

Napi::Object SelectionOutcome(Napi::Env env, const Selection &selection,
                              const ActionableFailure &failure = {}) {
  if (selection.index < 0) {
    if (failure.kind != nullptr) {
      return Outcome(env, failure.kind, failure.message);
    }
    return Outcome(env, "no-usable-preview", "The RAW contains no usable embedded JPEG");
  }
  auto result = Outcome(env, "preview", "");
  result.Set("candidateIndex", selection.index);
  result.Set("width", selection.dimensions.width);
  result.Set("height", selection.dimensions.height);
  result.Set("jpeg", Napi::Buffer<unsigned char>::Copy(env, selection.bytes.data(), selection.bytes.size()));
  return result;
}

Napi::Value ExtractImpl(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 1 || !info[0].IsString()) {
    Napi::TypeError::New(env, "Expected one RAW file path").ThrowAsJavaScriptException();
    return env.Undefined();
  }

  LibRawHandle raw(libraw_init(0));
  if (!raw) {
    return Outcome(env, "resource-limit", "LibRaw could not allocate a reader");
  }
  raw->rawparams.max_raw_memory_mb = kMaximumLibRawMemoryMb;

  const auto path = info[0].As<Napi::String>().Utf8Value();
  const int open_error = libraw_open_file(raw.get(), path.c_str());
  if (open_error != LIBRAW_SUCCESS) {
    return Outcome(env, ErrorKind(open_error), libraw_strerror(open_error));
  }

  Selection selection;
  ActionableFailure failure;
  const int count = std::clamp(raw->thumbs_list.thumbcount, 0, LIBRAW_THUMBNAIL_MAXCOUNT);
  for (int index = 0; index < count; ++index) {
    const auto &candidate = raw->thumbs_list.thumblist[index];
    if (candidate.tlength == 0 || candidate.tlength > kMaximumJpegBytes) {
      continue;
    }
    const int unpack_error = libraw_unpack_thumb_ex(raw.get(), index);
    if (unpack_error != LIBRAW_SUCCESS) {
      RecordUnpackFailure(failure, unpack_error);
      continue;
    }
    if (raw->thumbnail.tformat != LIBRAW_THUMBNAIL_JPEG || raw->thumbnail.tlength == 0 ||
        raw->thumbnail.tlength > kMaximumJpegBytes) {
      continue;
    }
    ConsiderCandidate(selection, index, reinterpret_cast<unsigned char *>(raw->thumbnail.thumb),
                      raw->thumbnail.tlength);
  }
  return SelectionOutcome(env, selection, failure);
}

Napi::Value Extract(const Napi::CallbackInfo &info) {
  try {
    return ExtractImpl(info);
  } catch (const std::bad_alloc &) {
    return Outcome(info.Env(), "resource-limit", "Native preview extraction exceeded its memory limit");
  } catch (const std::exception &) {
    return Outcome(info.Env(), "internal-error", "Native preview extraction failed internally");
  } catch (...) {
    return Outcome(info.Env(), "internal-error", "Native preview extraction failed internally");
  }
}

#ifdef SLIPSTREAM_TEST_ADDON
int InjectedUnpackError(const std::string &outcome) {
  if (outcome == "success") return LIBRAW_SUCCESS;
  if (outcome == "data") return LIBRAW_DATA_ERROR;
  if (outcome == "resource") return LIBRAW_UNSUFFICIENT_MEMORY;
  if (outcome == "io") return LIBRAW_IO_ERROR;
  return LIBRAW_CANCELLED_BY_CALLBACK;
}

Napi::Value TestSelectCandidates(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  try {
    if (info.Length() != 1 || !info[0].IsArray()) {
      Napi::TypeError::New(env, "Expected candidate array").ThrowAsJavaScriptException();
      return env.Undefined();
    }
    const auto candidates = info[0].As<Napi::Array>();
    auto extracted = Napi::Array::New(env);
    std::uint32_t extracted_count = 0;
    Selection selection;
    ActionableFailure failure;
    for (std::uint32_t index = 0; index < candidates.Length(); ++index) {
      const auto candidate = candidates.Get(index).As<Napi::Object>();
      const auto declared = candidate.Get("declaredLength").As<Napi::Number>().Int64Value();
      if (declared <= 0 || static_cast<std::uint64_t>(declared) > kMaximumJpegBytes) {
        continue;
      }
      extracted.Set(extracted_count++, index);
      const auto unpack_outcome = candidate.Has("unpackOutcome")
                                      ? candidate.Get("unpackOutcome").As<Napi::String>().Utf8Value()
                                      : "success";
      const int unpack_error = InjectedUnpackError(unpack_outcome);
      if (unpack_error != LIBRAW_SUCCESS) {
        RecordUnpackFailure(failure, unpack_error);
        continue;
      }
      const auto jpeg = candidate.Get("jpeg").As<Napi::Buffer<unsigned char>>();
      if (jpeg.Length() > kMaximumJpegBytes) {
        continue;
      }
      ConsiderCandidate(selection, static_cast<int>(index), jpeg.Data(), jpeg.Length());
    }
    auto result = SelectionOutcome(env, selection, failure);
    result.Set("extractedCandidateIndexes", extracted);
    return result;
  } catch (const std::bad_alloc &) {
    return Outcome(env, "resource-limit", "Native preview test selection exceeded its memory limit");
  } catch (const std::exception &) {
    return Outcome(env, "internal-error", "Native preview test selection failed internally");
  }
}

struct TestJpegOutput {
  unsigned char *bytes;
  unsigned long length;
};

Napi::Value TestCreateJpeg(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 2 || !info[0].IsNumber() || !info[1].IsNumber()) {
    Napi::TypeError::New(env, "Expected JPEG width and height").ThrowAsJavaScriptException();
    return env.Undefined();
  }
  const auto width = info[0].As<Napi::Number>().Uint32Value();
  const auto height = info[1].As<Napi::Number>().Uint32Value();
  if (width == 0 || height == 0 ||
      static_cast<std::uint64_t>(width) * height > kMaximumTestPixels) {
    Napi::RangeError::New(env, "Invalid test JPEG dimensions").ThrowAsJavaScriptException();
    return env.Undefined();
  }

  auto *output = static_cast<TestJpegOutput *>(std::calloc(1, sizeof(TestJpegOutput)));
  if (output == nullptr) {
    return Outcome(env, "resource-limit", "Could not allocate test JPEG output state");
  }
  jpeg_compress_struct encoder{};
  JpegErrorManager error{};
  encoder.err = jpeg_std_error(&error.base);
  error.base.error_exit = JpegErrorExit;
  if (setjmp(error.jump) != 0) {
    jpeg_destroy_compress(&encoder);
    std::free(output->bytes);
    std::free(output);
    return Outcome(env, "internal-error", "Could not create test JPEG");
  }
  jpeg_create_compress(&encoder);
  jpeg_mem_dest(&encoder, &output->bytes, &output->length);
  encoder.image_width = width;
  encoder.image_height = height;
  encoder.input_components = 3;
  encoder.in_color_space = JCS_RGB;
  jpeg_set_defaults(&encoder);
  jpeg_set_quality(&encoder, 75, TRUE);
  jpeg_start_compress(&encoder, TRUE);
  auto rows = (*encoder.mem->alloc_sarray)(reinterpret_cast<j_common_ptr>(&encoder), JPOOL_IMAGE,
                                          width * 3, 1);
  std::fill(rows[0], rows[0] + width * 3, 127);
  while (encoder.next_scanline < encoder.image_height) {
    jpeg_write_scanlines(&encoder, rows, 1);
  }
  jpeg_finish_compress(&encoder);
  if (output->length > kMaximumJpegBytes) {
    jpeg_destroy_compress(&encoder);
    std::free(output->bytes);
    std::free(output);
    return Outcome(env, "resource-limit", "Test JPEG exceeded its output limit");
  }
  auto result = Napi::Buffer<unsigned char>::Copy(env, output->bytes, output->length);
  jpeg_destroy_compress(&encoder);
  std::free(output->bytes);
  std::free(output);
  return result;
}
#endif
} // namespace

Napi::Object Init(Napi::Env env, Napi::Object exports) {
  exports.Set("extractLargestEmbeddedJpeg", Napi::Function::New(env, Extract));
#ifdef SLIPSTREAM_TEST_ADDON
  exports.Set("__testSelectCandidates", Napi::Function::New(env, TestSelectCandidates));
  exports.Set("__testCreateJpeg", Napi::Function::New(env, TestCreateJpeg));
#endif
  return exports;
}

#ifdef SLIPSTREAM_TEST_ADDON
NODE_API_MODULE(libraw_preview_test, Init)
#else
NODE_API_MODULE(libraw_preview, Init)
#endif
