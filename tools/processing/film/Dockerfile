# Standalone spektrafilm-rs extension of the immutable Slipstream runtime.
ARG APP_IMAGE
ARG SPEKTRAFILM_FORK_COMMIT

FROM rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS fork-build
WORKDIR /src
RUN apt-get update \
    && apt-get install --no-install-recommends --yes \
       build-essential ca-certificates libexiv2-dev libglib2.0-dev \
       liblensfun-dev libopenblas-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
ADD --checksum=sha256:de86b035655accff8d4010f1a221fdf50d353cb7b1422ba26f14a0db92612cfa \
    https://www.libraw.org/data/LibRaw-0.22.2.tar.gz /tmp/libraw.tar.gz
RUN tar -xzf /tmp/libraw.tar.gz -C /tmp \
    && cd /tmp/LibRaw-0.22.2 \
    && ./configure --prefix=/opt/slipstream-libraw --disable-static --disable-examples --disable-openmp --disable-lcms \
    && make -j2 \
    && make install \
    && rm -rf /tmp/LibRaw-0.22.2 /tmp/libraw.tar.gz
ENV PKG_CONFIG_PATH=/opt/slipstream-libraw/lib/pkgconfig
COPY third_party/spektrafilm-rs /src/spektrafilm-rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/spektrafilm-rs/target,sharing=locked \
    cargo build --manifest-path /src/spektrafilm-rs/Cargo.toml --locked --release \
      --features precision-f64 -p spektrafilm-cli --bin spektrafilm-f64
RUN mkdir -p /opt/spektrafilm-libs \
    && ldd /src/spektrafilm-rs/target/release/spektrafilm-f64 \
       | awk '$3 ~ /^\// {print $3}' \
       | sort -u \
       | xargs -r -I{} cp -L "{}" /opt/spektrafilm-libs/

FROM ubuntu:26.04@sha256:2260313b31c8c011cd2eebe728008efac1b3982be73eb71348ea2648d2c0e09b AS film-bundle
ARG SPEKTRAFILM_FORK_COMMIT
RUN apt-get update \
    && apt-get install --no-install-recommends --yes python3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=fork-build /src/spektrafilm-rs/target/release/spektrafilm-f64 /opt/slipstream-film/spektrafilm-f64
COPY --from=fork-build /opt/spektrafilm-libs /opt/slipstream-film/lib
COPY third_party/spektrafilm-rs/data /opt/slipstream-film/data
COPY crates/slipstream-processing/src/spektrafilm-recipe-tree.json /opt/slipstream-film/parameters-default.json
COPY tools/processing/film/manifest.py /opt/manifest.py
RUN SPEKTRAFILM_FORK_COMMIT="${SPEKTRAFILM_FORK_COMMIT}" python3 /opt/manifest.py

FROM ${APP_IMAGE} AS film
ARG SPEKTRAFILM_FORK_COMMIT
USER 0:0
COPY --from=film-bundle /opt/slipstream-film /opt/slipstream-film
ENV SPEKTRAFILM_FORK_COMMIT="${SPEKTRAFILM_FORK_COMMIT}" \
    LD_LIBRARY_PATH=/opt/slipstream-film/lib:/usr/local/lib/slipstream-film:/usr/local/lib/slipstream
LABEL slipstream.processing.film.spektrafilm="standalone-fork" \
      slipstream.processing.film.spektrafilm.commit="${SPEKTRAFILM_FORK_COMMIT}"
ARG FILM_BUNDLE
LABEL slipstream.processing.film.bundle="${FILM_BUNDLE}"
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/slipstream-server"]
