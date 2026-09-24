# Production development-tiff worker image: PID-1 launcher worker plus the
# pinned darktable engine and its adapter. The adapter reproduces the
# qualification history pipeline from tools/development (baseline database,
# generated XMP history, final full-size run, imported-history validation).
FROM rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --locked --release --package slipstream-processing --bin slipstream-processing-photo-worker

# Same pinned base and darktable version as the qualification image, so the
# module versions validated by the adapter are the ones the executor was
# qualified against.
FROM ubuntu:26.04@sha256:da6fc2be547864451aa253836dd926da33623312df4a9a243e35dc877c378a78 AS photo
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
       darktable=5.4.1-1 python3 ca-certificates libgomp1 \
    && dpkg-query -W > /opt/os-packages.txt \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/slipstream-processing-photo-worker /usr/local/bin/slipstream-processing-photo-worker
COPY tools/processing/photo/adapter.py /opt/slipstream-photo/adapter.py
# The linear ProPhoto RGB handoff profile comes verbatim from the qualified
# image; the worker copies it into the container configdir before exec, and
# darktable only resolves --icc-file against that registered copy.
COPY --from=slipstream:development-qualification \
     /opt/spektrafilm/src/spektrafilm/data/icc/ellelstone/LargeRGB-elle-V2-g10.icc \
     /opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc
RUN sha256sum /opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc /opt/slipstream-photo/adapter.py /opt/os-packages.txt \
        | sed 's#  /#  #g' | sha256sum | cut -d' ' -f1 > /opt/slipstream-photo/bundle \
    && cat /opt/slipstream-photo/bundle
# The deploy pipeline pins the numerical bundle recorded by the build:
#   PHOTO_BUNDLE=$(docker build ... --target bundle -q && docker run --rm \
#     <image> cat /opt/slipstream-photo/bundle)
ARG PHOTO_BUNDLE=unpinned
LABEL slipstream.processing.photo.bundle="${PHOTO_BUNDLE}"
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/slipstream-processing-photo-worker"]
