#include <napi.h>
#include <jpeglib.h>
#include <libraw/libraw.h>

#include <algorithm>
#include <csetjmp>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <exception>
#include <limits>
#include <memory>
#include <new>
#include <string>
#include <vector>
#ifdef __linux__
#include <fcntl.h>
#include <linux/openat2.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <dirent.h>
#include <cerrno>
#endif

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

Napi::Object ExtractOpened(Napi::Env env, const std::string &path) {
  LibRawHandle raw(libraw_init(0));
  if (!raw) return Outcome(env, "resource-limit", "LibRaw could not allocate a reader");
  raw->rawparams.max_raw_memory_mb = kMaximumLibRawMemoryMb;
  const int open_error = libraw_open_file(raw.get(), path.c_str());
  if (open_error != LIBRAW_SUCCESS) return Outcome(env, ErrorKind(open_error), libraw_strerror(open_error));
  Selection selection;
  ActionableFailure failure;
  const int count = std::clamp(raw->thumbs_list.thumbcount, 0, LIBRAW_THUMBNAIL_MAXCOUNT);
  for (int index = 0; index < count; ++index) {
    const auto &candidate = raw->thumbs_list.thumblist[index];
    if (candidate.tlength == 0 || candidate.tlength > kMaximumJpegBytes) continue;
    const int unpack_error = libraw_unpack_thumb_ex(raw.get(), index);
    if (unpack_error != LIBRAW_SUCCESS) { RecordUnpackFailure(failure, unpack_error); continue; }
    if (raw->thumbnail.tformat != LIBRAW_THUMBNAIL_JPEG || raw->thumbnail.tlength == 0 || raw->thumbnail.tlength > kMaximumJpegBytes) continue;
    ConsiderCandidate(selection, index, reinterpret_cast<unsigned char *>(raw->thumbnail.thumb), raw->thumbnail.tlength);
  }
  return SelectionOutcome(env, selection, failure);
}

#ifdef __linux__
bool SameOriginalRevision(const struct stat &before, const struct stat &after) {
  return before.st_dev == after.st_dev && before.st_ino == after.st_ino &&
         before.st_size == after.st_size &&
         before.st_mtim.tv_sec == after.st_mtim.tv_sec &&
         before.st_mtim.tv_nsec == after.st_mtim.tv_nsec;
}
#endif

#ifdef SLIPSTREAM_TEST_ADDON
Napi::Value ExtractImpl(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 1 || !info[0].IsString()) {
    Napi::TypeError::New(env, "Expected one RAW file path").ThrowAsJavaScriptException();
    return env.Undefined();
  }

  return ExtractOpened(env, info[0].As<Napi::String>().Utf8Value());
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

#ifdef __linux__
Napi::Value TestReadWholeWithMutation(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 2 || !info[0].IsString() || !info[1].IsFunction())
    return Outcome(env, "io-error", "Invalid test whole-file request");
  const auto path = info[0].As<Napi::String>().Utf8Value();
  const int fd = open(path.c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  if (fd < 0) return Outcome(env, "io-error", "Test Original could not be opened");
  struct stat before{};
  if (fstat(fd, &before) != 0 || !S_ISREG(before.st_mode) || before.st_size < 0 ||
      static_cast<std::uint64_t>(before.st_size) > kMaximumJpegBytes) {
    close(fd);
    return Outcome(env, "io-error", "Test Original is invalid");
  }
  info[1].As<Napi::Function>().Call({});
  std::vector<unsigned char> bytes(static_cast<std::size_t>(before.st_size));
  std::size_t consumed = 0;
  while (consumed < bytes.size()) {
    const auto count = pread(fd, bytes.data() + consumed, bytes.size() - consumed,
                             static_cast<off_t>(consumed));
    if (count <= 0) { close(fd); return Outcome(env, "io-error", "Test Original read failed"); }
    consumed += static_cast<std::size_t>(count);
  }
  struct stat after{};
  if (fstat(fd, &after) != 0 || !SameOriginalRevision(before, after)) {
    close(fd);
    return Outcome(env, "io-error", "Original File changed during read");
  }
  close(fd);
  auto result = Outcome(env, "file", "");
  result.Set("bytes", Napi::Buffer<unsigned char>::Copy(env, bytes.data(), bytes.size()));
  return result;
}

Napi::Value TestExtractWithMutation(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 2 || !info[0].IsString() || !info[1].IsFunction())
    return Outcome(env, "io-error", "Invalid test extraction request");
  const auto path = info[0].As<Napi::String>().Utf8Value();
  const int fd = open(path.c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  if (fd < 0) return Outcome(env, "io-error", "Test Original could not be opened");
  struct stat before{};
  if (fstat(fd, &before) != 0 || !S_ISREG(before.st_mode)) {
    close(fd);
    return Outcome(env, "io-error", "Test Original is invalid");
  }
  info[1].As<Napi::Function>().Call({});
  auto result = ExtractOpened(env, "/proc/self/fd/" + std::to_string(fd));
  struct stat after{};
  if (fstat(fd, &after) != 0 || !SameOriginalRevision(before, after)) {
    close(fd);
    return Outcome(env, "io-error", "Original File changed during Preview extraction");
  }
  close(fd);
  return result;
}
#endif

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

#ifdef __linux__
bool ValidRelativePath(const std::string &path, bool allow_empty = false) {
  if ((!allow_empty && path.empty()) || (!path.empty() && path.front() == '/') || path.find('\0') != std::string::npos) return false;
  std::size_t start = 0;
  while (start <= path.size()) {
    const auto end = path.find('/', start);
    const auto part = path.substr(start, end == std::string::npos ? std::string::npos : end - start);
    if (part.empty() || part == "." || part == "..") return allow_empty && path.empty();
    if (end == std::string::npos) break;
    start = end + 1;
  }
  return true;
}

int OpenConfined(int root_fd, const std::string &relative_path, bool directory = false) {
  if (!ValidRelativePath(relative_path)) return -1;
  open_how how{};
  how.flags = O_RDONLY | O_CLOEXEC | O_NOFOLLOW | (directory ? O_DIRECTORY : 0);
  how.resolve = RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS;
  return static_cast<int>(syscall(SYS_openat2, root_fd, relative_path.c_str(), &how, sizeof(how)));
}

int OpenConfinedDirectory(int root_fd, const std::string &relative_path) {
  if (relative_path.empty()) return openat(root_fd, ".", O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_DIRECTORY);
  return OpenConfined(root_fd, relative_path, true);
}

Napi::Value ListConfinedDirectory(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 3 || !info[0].IsNumber() || !info[1].IsString() || !info[2].IsNumber()) return Outcome(env, "io-error", "Invalid directory request");
  const auto path = info[1].As<Napi::String>().Utf8Value();
  const double maximum_value = info[2].As<Napi::Number>().DoubleValue();
  if (!std::isfinite(maximum_value) || maximum_value < 1 || maximum_value > 4294967295.0 ||
      maximum_value != std::floor(maximum_value))
    return Outcome(env, "resource-limit", "Directory entry limit is invalid");
  const auto maximum = static_cast<std::uint32_t>(maximum_value);
  int fd = OpenConfinedDirectory(info[0].As<Napi::Number>().Int32Value(), path);
  if (fd < 0) return Outcome(env, "io-error", "Directory could not be opened safely");
  DIR *directory = fdopendir(fd);
  if (!directory) { close(fd); return Outcome(env, "io-error", "Directory could not be enumerated safely"); }
  struct Entry { std::string name; std::string kind; };
  std::vector<Entry> entries;
  errno = 0;
  while (auto *item = readdir(directory)) {
    std::string name(item->d_name);
    if (name == "." || name == "..") continue;
    if (entries.size() >= maximum) { closedir(directory); return Outcome(env, "resource-limit", "Directory exceeds entry limit"); }
    struct stat facts{};
    std::string kind = "other";
    if (fstatat(dirfd(directory), name.c_str(), &facts, AT_SYMLINK_NOFOLLOW) == 0) {
      if (S_ISREG(facts.st_mode)) kind = "file";
      else if (S_ISDIR(facts.st_mode)) kind = "directory";
      else if (S_ISLNK(facts.st_mode)) kind = "symlink";
    }
    entries.push_back({name, kind});
  }
  const int error = errno;
  closedir(directory);
  if (error != 0) return Outcome(env, "io-error", "Directory enumeration failed safely");
  std::sort(entries.begin(), entries.end(), [](const Entry &a, const Entry &b) { return a.name < b.name; });
  auto result = Napi::Object::New(env); result.Set("kind", "directory");
  auto list = Napi::Array::New(env, entries.size());
  for (std::size_t i = 0; i < entries.size(); ++i) { auto value = Napi::Object::New(env); value.Set("name", entries[i].name); value.Set("kind", entries[i].kind); list.Set(i, value); }
  result.Set("entries", list); return result;
}

void SetOriginalFacts(Napi::Env env, Napi::Object &value, const struct stat &facts) {
  auto source = Napi::Object::New(env);
  source.Set("size", Napi::Number::New(env, static_cast<double>(facts.st_size)));
  source.Set("mtimeMs", Napi::Number::New(env, static_cast<double>(facts.st_mtim.tv_sec) * 1000.0 + facts.st_mtim.tv_nsec / 1e6));
  source.Set("device", Napi::BigInt::New(env, static_cast<std::uint64_t>(facts.st_dev)));
  source.Set("inode", Napi::BigInt::New(env, static_cast<std::uint64_t>(facts.st_ino)));
  value.Set("sourceFacts", source);
}

Napi::Value ExtractFromLibrary(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 2 || !info[0].IsNumber() || !info[1].IsString()) return Outcome(env, "io-error", "Invalid confined RAW request");
  const int fd = OpenConfined(info[0].As<Napi::Number>().Int32Value(), info[1].As<Napi::String>().Utf8Value());
  if (fd < 0) return Outcome(env, "io-error", "Original File could not be opened safely");
  struct stat facts{};
  if (fstat(fd, &facts) != 0 || !S_ISREG(facts.st_mode)) {
    close(fd);
    return Outcome(env, "io-error", "Original File is not a regular file");
  }
  const std::string path = "/proc/self/fd/" + std::to_string(fd);
  auto result = ExtractOpened(env, path);
  struct stat after{};
  if (fstat(fd, &after) != 0 || !SameOriginalRevision(facts, after)) {
    close(fd);
    return Outcome(env, "io-error", "Original File changed during Preview extraction");
  }
  SetOriginalFacts(env, result, facts);
  close(fd);
  return result;
}

Napi::Value ConfinedOriginalFacts(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 2 || !info[0].IsNumber() || !info[1].IsString()) {
    Napi::TypeError::New(env, "Expected root descriptor and relative Original path").ThrowAsJavaScriptException();
    return env.Undefined();
  }
  const int fd = OpenConfined(info[0].As<Napi::Number>().Int32Value(), info[1].As<Napi::String>().Utf8Value());
  if (fd < 0) return Outcome(env, "io-error", "Original File could not be opened safely");
  struct stat facts{};
  const int result = fstat(fd, &facts);
  close(fd);
  if (result != 0 || !S_ISREG(facts.st_mode)) return Outcome(env, "io-error", "Original File is not a regular file");
  auto value = Napi::Object::New(env);
  value.Set("kind", "facts");
  value.Set("size", Napi::Number::New(env, static_cast<double>(facts.st_size)));
  value.Set("mtimeMs", Napi::Number::New(env, static_cast<double>(facts.st_mtim.tv_sec) * 1000.0 + facts.st_mtim.tv_nsec / 1e6));
  value.Set("mode", Napi::Number::New(env, facts.st_mode));
  return value;
}

Napi::Value ReadConfinedOriginalWhole(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 3 || !info[0].IsNumber() || !info[1].IsString() || !info[2].IsNumber())
    return Outcome(env, "io-error", "Invalid confined whole-file request");
  const double maximum_value = info[2].As<Napi::Number>().DoubleValue();
  if (!std::isfinite(maximum_value) || maximum_value < 0 || maximum_value > 128 * 1024 * 1024 || maximum_value != std::floor(maximum_value))
    return Outcome(env, "resource-limit", "Confined whole-file read exceeds limits");
  const int fd = OpenConfined(info[0].As<Napi::Number>().Int32Value(), info[1].As<Napi::String>().Utf8Value());
  if (fd < 0) return Outcome(env, "io-error", "Original File could not be opened safely");
  struct stat facts{};
  if (fstat(fd, &facts) != 0 || !S_ISREG(facts.st_mode) || facts.st_size < 0 ||
      static_cast<std::uint64_t>(facts.st_size) > static_cast<std::uint64_t>(maximum_value)) {
    close(fd);
    return Outcome(env, "resource-limit", "Original File exceeds whole-file read limit");
  }
  std::vector<unsigned char> bytes(static_cast<std::size_t>(facts.st_size));
  std::size_t consumed = 0;
  while (consumed < bytes.size()) {
    const auto count = pread(fd, bytes.data() + consumed, bytes.size() - consumed, static_cast<off_t>(consumed));
    if (count <= 0) {
      close(fd);
      return Outcome(env, "io-error", "Original File could not be read completely");
    }
    consumed += static_cast<std::size_t>(count);
  }
  struct stat after{};
  if (fstat(fd, &after) != 0 || !SameOriginalRevision(facts, after)) {
    close(fd);
    return Outcome(env, "io-error", "Original File changed during read");
  }
  auto value = Napi::Object::New(env);
  value.Set("kind", "file");
  value.Set("bytes", Napi::Buffer<unsigned char>::Copy(env, bytes.data(), bytes.size()));
  SetOriginalFacts(env, value, facts);
  close(fd);
  return value;
}

Napi::Value ReadConfinedOriginalRange(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 4 || !info[0].IsNumber() || !info[1].IsString() || !info[2].IsNumber() || !info[3].IsNumber()) {
    Napi::TypeError::New(env, "Expected root descriptor, relative path, offset and length").ThrowAsJavaScriptException();
    return env.Undefined();
  }
  const double offset_value = info[2].As<Napi::Number>().DoubleValue();
  const double length_value = info[3].As<Napi::Number>().DoubleValue();
  if (!std::isfinite(offset_value) || !std::isfinite(length_value) || offset_value < 0 ||
      length_value < 0 || offset_value > 9007199254740991.0 || length_value > 16 * 1024 * 1024 ||
      offset_value != std::floor(offset_value) || length_value != std::floor(length_value))
    return Outcome(env, "resource-limit", "Confined read exceeds limits");
  const auto offset = static_cast<off_t>(offset_value);
  const auto length = static_cast<std::size_t>(length_value);
  const int fd = OpenConfined(info[0].As<Napi::Number>().Int32Value(), info[1].As<Napi::String>().Utf8Value());
  if (fd < 0) return Outcome(env, "io-error", "Original File could not be opened safely");
  std::vector<unsigned char> bytes(length);
  const auto count = pread(fd, bytes.data(), length, offset);
  close(fd);
  if (count < 0) return Outcome(env, "io-error", "Original File could not be read safely");
  return Napi::Buffer<unsigned char>::Copy(env, bytes.data(), static_cast<std::size_t>(count));
}

bool SafeStateFile(const struct stat &facts) {
  return S_ISREG(facts.st_mode) && facts.st_uid == geteuid() &&
         (facts.st_mode & 0022) == 0 && facts.st_nlink == 1;
}

void SetStateIdentity(Napi::Env env, Napi::Object &value, const struct stat &facts) {
  value.Set("device", Napi::BigInt::New(env, static_cast<std::uint64_t>(facts.st_dev)));
  value.Set("inode", Napi::BigInt::New(env, static_cast<std::uint64_t>(facts.st_ino)));
  value.Set("uid", Napi::Number::New(env, facts.st_uid));
  value.Set("mode", Napi::Number::New(env, facts.st_mode));
  value.Set("linkCount", Napi::Number::New(env, facts.st_nlink));
}

bool MatchesStateIdentity(const Napi::Object &expected, const struct stat &facts) {
  bool device_lossless = false, inode_lossless = false;
  const auto device = expected.Get("device").As<Napi::BigInt>().Uint64Value(&device_lossless);
  const auto inode = expected.Get("inode").As<Napi::BigInt>().Uint64Value(&inode_lossless);
  return device_lossless && inode_lossless &&
         device == static_cast<std::uint64_t>(facts.st_dev) &&
         inode == static_cast<std::uint64_t>(facts.st_ino) &&
         expected.Get("uid").As<Napi::Number>().Uint32Value() == facts.st_uid &&
         expected.Get("mode").As<Napi::Number>().Uint32Value() == facts.st_mode &&
         expected.Get("linkCount").As<Napi::Number>().Uint32Value() == facts.st_nlink;
}

Napi::Value PrepareStateFile(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 2 || !info[0].IsNumber() || !info[1].IsString())
    return Outcome(env, "io-error", "Invalid state file request");
  const auto name = info[1].As<Napi::String>().Utf8Value();
  if (!ValidRelativePath(name) || name.find('/') != std::string::npos)
    return Outcome(env, "io-error", "SQLite database name is invalid");
  const int fd = openat(info[0].As<Napi::Number>().Int32Value(), name.c_str(),
                        O_RDWR | O_CREAT | O_CLOEXEC | O_NOFOLLOW, 0600);
  if (fd < 0) return Outcome(env, "io-error", "SQLite database could not be created safely");
  struct stat facts{};
  const int result = fstat(fd, &facts);
  close(fd);
  if (result != 0 || !SafeStateFile(facts))
    return Outcome(env, "io-error", "SQLite database inode is not safely owned");
  auto value = Napi::Object::New(env);
  value.Set("kind", "prepared");
  SetStateIdentity(env, value, facts);
  return value;
}

Napi::Value VerifyStateFile(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 3 || !info[0].IsNumber() || !info[1].IsString() || !info[2].IsObject())
    return Outcome(env, "io-error", "Invalid state verification request");
  const auto name = info[1].As<Napi::String>().Utf8Value();
  if (!ValidRelativePath(name) || name.find('/') != std::string::npos)
    return Outcome(env, "io-error", "SQLite database name is invalid");
  const int fd = openat(info[0].As<Napi::Number>().Int32Value(), name.c_str(),
                        O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  if (fd < 0) return Outcome(env, "io-error", "SQLite database could not be verified safely");
  struct stat facts{};
  const int result = fstat(fd, &facts);
  close(fd);
  if (result != 0 || !SafeStateFile(facts) ||
      !MatchesStateIdentity(info[2].As<Napi::Object>(), facts))
    return Outcome(env, "io-error", "SQLite database inode changed before startup");
  auto value = Napi::Object::New(env);
  value.Set("kind", "verified");
  return value;
}

bool AdmitOptionalStateFile(int state_fd, const std::string &name) {
  const int fd = openat(state_fd, name.c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  if (fd < 0) return errno == ENOENT;
  struct stat facts{};
  const int result = fstat(fd, &facts);
  close(fd);
  return result == 0 && SafeStateFile(facts);
}

Napi::Value AdmitStateSidecars(const Napi::CallbackInfo &info) {
  const auto env = info.Env();
  if (info.Length() != 2 || !info[0].IsNumber() || !info[1].IsString())
    return Outcome(env, "io-error", "Invalid SQLite sidecar admission request");
  const int state_fd = info[0].As<Napi::Number>().Int32Value();
  const auto name = info[1].As<Napi::String>().Utf8Value();
  if (!ValidRelativePath(name) || name.find('/') != std::string::npos)
    return Outcome(env, "io-error", "SQLite database name is invalid");
  for (const char *suffix : {"-journal", "-wal", "-shm"}) {
    if (!AdmitOptionalStateFile(state_fd, name + suffix))
      return Outcome(env, "io-error", "SQLite sidecar is not safely owned");
  }
  auto value = Napi::Object::New(env);
  value.Set("kind", "admitted");
  return value;
}
#endif

Napi::Object Init(Napi::Env env, Napi::Object exports) {
#ifdef __linux__
  exports.Set("confinedOriginalFacts", Napi::Function::New(env, ConfinedOriginalFacts));
  exports.Set("readConfinedOriginalRange", Napi::Function::New(env, ReadConfinedOriginalRange));
  exports.Set("readConfinedOriginalWhole", Napi::Function::New(env, ReadConfinedOriginalWhole));
  exports.Set("listConfinedDirectory", Napi::Function::New(env, ListConfinedDirectory));
  exports.Set("extractLargestEmbeddedJpegFromLibrary", Napi::Function::New(env, ExtractFromLibrary));
  exports.Set("prepareStateFile", Napi::Function::New(env, PrepareStateFile));
  exports.Set("verifyStateFile", Napi::Function::New(env, VerifyStateFile));
  exports.Set("admitStateSidecars", Napi::Function::New(env, AdmitStateSidecars));
#endif
#ifdef SLIPSTREAM_TEST_ADDON
  exports.Set("extractLargestEmbeddedJpeg", Napi::Function::New(env, Extract));
  exports.Set("__testSelectCandidates", Napi::Function::New(env, TestSelectCandidates));
  exports.Set("__testCreateJpeg", Napi::Function::New(env, TestCreateJpeg));
#ifdef __linux__
  exports.Set("__testReadWholeWithMutation", Napi::Function::New(env, TestReadWholeWithMutation));
  exports.Set("__testExtractWithMutation", Napi::Function::New(env, TestExtractWithMutation));
#endif
#endif
  return exports;
}

#ifdef SLIPSTREAM_TEST_ADDON
NODE_API_MODULE(libraw_preview_test, Init)
#else
NODE_API_MODULE(libraw_preview, Init)
#endif
