# Native darktable MCP production image. The darktable source is supplied as
# the BuildKit `darktable` context and is checked by build.py before this file runs.
ARG DARKTABLE_COMMIT

FROM rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS rust-build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN CARGO_BUILD_JOBS=2 cargo build --locked --release --package slipstream-processing --bin slipstream-processing-photo-worker

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
      -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/opt/darktable \
      -DBINARY_PACKAGE_BUILD=ON -DUSE_SDL2=OFF -DBUILD_CMSTEST=OFF \
      -DUSE_MCP=ON -DUSE_OPENCL=OFF -DUSE_LUA=OFF -DUSE_COLORD=OFF \
      -DUSE_GMIC=OFF -DUSE_GRAPHICSMAGICK=OFF -DUSE_MAP=OFF \
      -DBUILD_RS_IDENTIFY=OFF -DBUILD_TESTING=OFF -DVALIDATE_APPDATA_FILE=OFF \
    && cmake --build build --parallel 2 \
    && DESTDIR=/opt/darktable-root cmake --install build

FROM slipstream:344-buffer-lifetimes AS photo
ARG DARKTABLE_COMMIT
USER 0:0
ENV DEBIAN_FRONTEND=noninteractive \
    PYTHONPATH=/opt/spektrafilm/src:/opt/probe:/opt/slipstream-photo
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       python3 ca-certificates libgomp1 libgtk-3-0t64 libjson-glib-1.0-0 libsqlite3-0 \
       libcurl3t64-gnutls libpng16-16t64 libexiv2-28 libpotrace0 libpugixml1v5 libarchive13t64 \
       liblensfun1 libjpeg-turbo8 libtiff6 liblcms2-2 libxml2-16 libxslt1.1 librsvg2-2 zlib1g \
    && dpkg-query -W > /opt/os-packages.txt \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust-build /src/target/release/slipstream-processing-photo-worker /usr/local/bin/slipstream-processing-photo-worker
COPY --from=darktable-build /opt/darktable-root/opt/darktable /opt/darktable
COPY --from=darktable-build /src/darktable/LICENSE /usr/share/doc/slipstream/darktable-LICENSE
COPY --from=darktable-build /src/darktable-commit /opt/slipstream-photo/darktable-commit
COPY tools/processing/photo/discover.py tools/processing/photo/manifest.py /opt/slipstream-photo/
COPY tools/processing/photo/film_adapter.py /opt/slipstream-photo/film_adapter.py
COPY --from=slipstream:344-buffer-lifetimes /opt/spektrafilm /opt/spektrafilm
COPY --from=slipstream:344-buffer-lifetimes /opt/python /opt/python
COPY --from=slipstream:344-buffer-lifetimes /opt/runtime /opt/runtime
COPY --from=slipstream:344-buffer-lifetimes /opt/probe/film.py /opt/probe/bundle.py /opt/probe/
COPY tools/development/finished_jpeg.py tools/development/film_identity.py /opt/probe/
COPY --from=slipstream:344-buffer-lifetimes /opt/processing-bundle.json /opt/processing-bundle.json
COPY --from=slipstream:344-buffer-lifetimes /opt/spektrafilm/src/spektrafilm/data/icc/ellelstone/LargeRGB-elle-V2-g10.icc /opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc
RUN /opt/runtime/bin/python3 -c 'import sys; sys.path.insert(0, "/opt/slipstream-photo"); from film import make_simulator; from film_adapter import check_recipe; _, recipe = make_simulator(); check_recipe(recipe)' \
    && python3 /opt/slipstream-photo/discover.py /opt/darktable/bin/darktable-mcp \
    && DARKTABLE_COMMIT="$DARKTABLE_COMMIT" python3 /opt/slipstream-photo/manifest.py \
    && rm -rf /work/config
LABEL slipstream.processing.photo.darktable="native-mcp" \
      slipstream.processing.photo.darktable.commit="${DARKTABLE_COMMIT}"
ARG PHOTO_BUNDLE
LABEL slipstream.processing.photo.bundle="${PHOTO_BUNDLE}"
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/slipstream-processing-photo-worker"]
