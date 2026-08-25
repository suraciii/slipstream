{
  "variables": {
    "slipstream_build_test_addon%": 0,
    "slipstream_native_include_dirs": [
      "<!@(node -p \"require('node-addon-api').include\")",
      "<!@(pkg-config --cflags-only-I libraw libjpeg | sed 's/-I//g')"
    ],
    "slipstream_native_libraries": ["<!@(pkg-config --libs libraw libjpeg)"],
    "slipstream_native_cflags": ["<!@(pkg-config --cflags-only-other libraw libjpeg)"]
  },
  "targets": [
    {
      "target_name": "libraw_preview",
      "product_name": "raw_preview",
      "sources": ["native/libraw_preview.cc"],
      "include_dirs": ["<@(slipstream_native_include_dirs)"],
      "libraries": ["<@(slipstream_native_libraries)"],
      "cflags": ["<@(slipstream_native_cflags)"],
      "cflags_cc": ["-std=c++17", "-fexceptions"],
      "cflags_cc!": ["-fno-exceptions"],
      "defines": ["NAPI_CPP_EXCEPTIONS"]
    },
    {
      "target_name": "libraw_preview_test",
      "product_name": "raw_preview_test",
      "sources": ["native/libraw_preview.cc"],
      "include_dirs": ["<@(slipstream_native_include_dirs)"],
      "libraries": ["<@(slipstream_native_libraries)"],
      "cflags": ["<@(slipstream_native_cflags)"],
      "cflags_cc": ["-std=c++17", "-fexceptions"],
      "cflags_cc!": ["-fno-exceptions"],
      "defines": ["NAPI_CPP_EXCEPTIONS", "SLIPSTREAM_TEST_ADDON"],
      "conditions": [
        ["slipstream_build_test_addon==0", {"type": "none", "sources": []}]
      ]
    }
  ]
}
