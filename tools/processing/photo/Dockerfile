# Native darktable Photo Development extension of the Slipstream application
# image. The normal application runtime (built from the repository Dockerfile)
# is the parent: the extended image runs the same Slipstream server entrypoint
# and only adds the pinned native engine, its discovered metadata, the ICC
# output profile, and the deterministic bundle manifest. The darktable source
# is supplied as the BuildKit `darktable` context and is checked by build.py
# before this file runs.
ARG DARKTABLE_COMMIT
ARG APP_IMAGE

FROM ubuntu:26.04@sha256:da6fc2be547864451aa253836dd926da33623312df4a9a243e35dc877c378a78 AS darktable-build
ARG DARKTABLE_COMMIT
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       build-essential cmake ninja-build git gettext intltool perl python3 python3-pil python3-tifffile \
       libgtk-3-dev libjson-glib-dev libsqlite3-dev libcurl4-gnutls-dev libpng-dev libexiv2-dev \
       libpotrace-dev libpugixml-dev libarchive-dev liblensfun-dev libjpeg-dev libtiff-dev liblcms2-dev \
       libxml2-dev libxslt1-dev libxml2-utils xsltproc librsvg2-dev zlib1g-dev libgomp1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=darktable / /src/darktable
WORKDIR /src/darktable
RUN test -n "$DARKTABLE_COMMIT" \
    && printf '%s\n' "$DARKTABLE_COMMIT" > /src/darktable-commit
RUN cmake -S . -B build -G Ninja \
      -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/opt/darktable-edit/native -DCMAKE_INSTALL_LIBDIR=lib \
      -DBUILD_GUI=OFF -DBINARY_PACKAGE_BUILD=ON -DUSE_SDL2=OFF -DBUILD_CMSTEST=OFF \
      -DUSE_MCP=ON -DUSE_OPENCL=OFF -DUSE_LUA=OFF -DUSE_COLORD=OFF \
      -DUSE_GMIC=OFF -DUSE_GRAPHICSMAGICK=OFF -DUSE_MAP=OFF \
      -DBUILD_RS_IDENTIFY=OFF -DBUILD_TESTING=OFF -DVALIDATE_APPDATA_FILE=OFF \
    && cmake --build build --parallel 2 \
    && DESTDIR=/opt/darktable-root cmake --install build

FROM ${APP_IMAGE} AS photo
ARG DARKTABLE_COMMIT
USER 0:0
ENV DEBIAN_FRONTEND=noninteractive
# python3 is a build-time tool only: it runs the MCP discovery and manifest
# helpers that pin this image's engine contract. The server owns the runtime
# protocol; there is no Python adapter or fallback path.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       python3 libgomp1 libicu78 libgtk-3-0t64 libjson-glib-1.0-0 libsqlite3-0 \
       libcurl3t64-gnutls libpng16-16t64 libexiv2-28 libpotrace0 libpugixml1v5 libarchive13t64 \
       liblensfun1 libjpeg-turbo8 libtiff6 liblcms2-2 libxml2-16 libxslt1.1 librsvg2-2 zlib1g \
    && dpkg-query -W > /opt/os-packages.txt \
    && rm -rf /var/lib/apt/lists/*
COPY --from=darktable-build /opt/darktable-root/opt/darktable-edit/native /opt/darktable-source
COPY tools/processing/photo/icc/LargeRGB-elle-V2-g10.icc /opt/darktable-source/share/darktable/color/out/LargeRGB-elle-V2-g10.icc
COPY crates/slipstream-core/assets/srgb-iec61966-2-1.icc /opt/darktable-source/share/darktable/color/out/srgb-iec61966-2-1.icc
COPY --from=darktable-build /src/darktable/LICENSE /usr/share/doc/slipstream/darktable-LICENSE
COPY --from=darktable-build /src/darktable-commit /opt/slipstream-photo/darktable-commit
COPY tools/processing/photo/discover.py tools/processing/photo/manifest.py /opt/slipstream-photo/
COPY tools/processing/photo/icc/LargeRGB-elle-V2-g10.icc /opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc
COPY tools/processing/photo/edit_manifest.py tools/processing/photo/runtime_libs.py /opt/slipstream-photo/
RUN mkdir -p /opt/darktable-edit/native \
    && cp -aL /opt/darktable-source/. /opt/darktable-edit/native/ \
    && cp -aL /opt/darktable-source /opt/darktable \
    && rm -rf /opt/darktable-source \
    && ln -s /opt/darktable /opt/slipstream-photo/darktable \
    && cp /opt/os-packages.txt /opt/slipstream-photo/os-packages.txt \
    && python3 /opt/slipstream-photo/runtime_libs.py \
    && LD_LIBRARY_PATH=/opt/darktable-edit/lib python3 /opt/slipstream-photo/discover.py /opt/darktable/bin/darktable-mcp \
    && ENGINE_METADATA=/opt/darktable-edit/engine-metadata.json LD_LIBRARY_PATH=/opt/darktable-edit/lib python3 /opt/slipstream-photo/discover.py /opt/darktable-edit/native/bin/darktable-mcp \
    && cp /opt/os-packages.txt /opt/darktable-edit/os-packages.txt \
    && DARKTABLE_COMMIT="$DARKTABLE_COMMIT" python3 /opt/slipstream-photo/edit_manifest.py \
    && test -s /opt/darktable-edit/edit-manifest.json \
    && DARKTABLE_COMMIT="$DARKTABLE_COMMIT" python3 /opt/slipstream-photo/manifest.py \
    && test ! -e /opt/darktable-edit/native/bin/darktable \
    && rm -rf /work/config
LABEL slipstream.processing.photo.darktable="native-mcp" \
      slipstream.processing.photo.darktable.commit="${DARKTABLE_COMMIT}"
ARG PHOTO_BUNDLE
LABEL slipstream.processing.photo.bundle="${PHOTO_BUNDLE}"
# The extended image is the application: same server entrypoint, same user,
# same health contract as the normal runtime parent.
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/slipstream-server"]
