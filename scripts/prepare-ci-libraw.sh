#!/usr/bin/env bash

set -euo pipefail

fail() {
  printf 'prepare-ci-libraw: %s\n' "$*" >&2
  exit 1
}

prefix="${RUNNER_TEMP:?}/libraw-022"
[[ ! -L "$prefix" ]] || fail "installation prefix must not be a symbolic link"
if [[ ! -e "$prefix" ]]; then
  archive="$RUNNER_TEMP/LibRaw-0.22.2.tar.gz"
  curl -fsSL --retry 3 -o "$archive" https://www.libraw.org/data/LibRaw-0.22.2.tar.gz
  printf 'de86b035655accff8d4010f1a221fdf50d353cb7b1422ba26f14a0db92612cfa  %s\n' "$archive" \
    | sha256sum -c -
  tar -xzf "$archive" -C "$RUNNER_TEMP"
  (
    cd "$RUNNER_TEMP/LibRaw-0.22.2"
    ./configure \
      --prefix="$prefix" \
      --disable-static \
      --disable-examples \
      --disable-openmp \
      --disable-lcms
    make -j"$(nproc)"
    make install
  )
  (
    cd "$prefix"
    find . -type f ! -name checksums.sha256 -print0 \
      | LC_ALL=C sort -z \
      | xargs -0 sha256sum > checksums.sha256
  )
  printf 'LibRaw 0.22.2 installation built\n'
else
  printf 'Checking restored LibRaw 0.22.2 installation\n'
fi

[[ -f "$prefix/checksums.sha256" && -s "$prefix/checksums.sha256" ]] \
  || fail "installation checksum manifest is missing or empty"
(cd "$prefix" && sha256sum --check --strict checksums.sha256)

export PKG_CONFIG_PATH="$prefix/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
export LD_LIBRARY_PATH="$prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
for package in libraw libraw_r; do
  pkg-config --exact-version=0.22.2 "$package"
  [[ "$(pkg-config --variable=prefix "$package")" == "$prefix" ]] \
    || fail "$package resolves outside the installation prefix"
  [[ "$(pkg-config --variable=libdir "$package")" == "$prefix/lib" ]] \
    || fail "$package resolves an unexpected library directory"
  [[ "$(pkg-config --variable=includedir "$package")" == "$prefix/include" ]] \
    || fail "$package resolves an unexpected include directory"
  for name in "$package.so" "$package.so.25"; do
    library="$(realpath -e -- "$prefix/lib/$name")"
    [[ -f "$library" && "$library" == "$prefix/lib/$package.so.25.0.0" ]] \
      || fail "$name does not resolve to its verified shared library"
  done
done
{
  printf 'PKG_CONFIG_PATH=%s\n' "$PKG_CONFIG_PATH"
  printf 'LD_LIBRARY_PATH=%s\n' "$LD_LIBRARY_PATH"
} >> "${GITHUB_ENV:?}"
