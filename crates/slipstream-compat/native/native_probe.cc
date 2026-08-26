#include <cstddef>
#include <cstdio>

#include <jpeglib.h>
#include <libraw/libraw.h>
#include <vips/vips.h>

extern "C" const char *slipstream_probe_libraw_version() {
  return libraw_version();
}

extern "C" int slipstream_probe_jpeg_version() {
  return JPEG_LIB_VERSION;
}

extern "C" int slipstream_probe_vips_lifecycle() {
  if (vips_init("slipstream-probe") != 0) return 0;
  // libvips is process-global. Keep it initialized until process exit; the
  // production wrapper never exposes or calls vips_shutdown between jobs.
  const auto version = vips_version(0);
  return version >= 8 ? 1 : 0;
}

extern "C" int slipstream_probe_thumbnail_api() {
  libraw_data_t *raw = libraw_init(0);
  if (raw == nullptr) return 0;
  raw->rawparams.max_raw_memory_mb = 256;
  // Deliberately reference only thumbnail extraction. Sensor unpack/development
  // functions are outside the production shim contract.
  auto unpack_thumb = &libraw_unpack_thumb_ex;
  const int supported = unpack_thumb != nullptr && LIBRAW_THUMBNAIL_MAXCOUNT > 0;
  libraw_close(raw);
  return supported;
}
