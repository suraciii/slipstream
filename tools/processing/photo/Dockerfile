# Production Photo worker image: PID-1 launcher worker plus the pinned
# darktable engine and its adapter, and the fixed Film stage over the
# darktable handoff TIFF. The development adapter reproduces the
# qualification history pipeline from tools/development (baseline database,
# generated XMP history, final full-size run, imported-history validation);
# the film-jpeg workload renders the same handoff with the fixed Film recipe.
FROM rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --locked --release --package slipstream-processing --bin slipstream-processing-photo-worker

# Retain the numerical runtime whose complete manifest is bound into the fixed
# Film recipe. Refuse a moved local tag before copying any runtime payload.
FROM slipstream:344-buffer-lifetimes AS film-runtime
RUN echo '0bf4af15d4f5323d060d4e6543e0e97d1a2fe6014d27c1cd81db440e4f46a152  /opt/processing-bundle.json' | sha256sum --check \
    && /opt/runtime/bin/python /opt/probe/bundle.py --verify

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
COPY tools/processing/photo/film_adapter.py /opt/slipstream-photo/film_adapter.py
# The linear ProPhoto RGB handoff profile comes verbatim from the qualified
# image; the worker copies it into the container configdir before exec, and
# darktable only resolves --icc-file against that registered copy.
COPY --from=film-runtime \
     /opt/spektrafilm/src/spektrafilm/data/icc/ellelstone/LargeRGB-elle-V2-g10.icc \
     /opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc
# The fixed Film stage renders the handoff TIFF with the pinned numerical
# runtime. The locked interpreter (the venv and the uv-managed CPython it
# links), the patched spektrafilm source, and the shared development modules
# come verbatim from the verified numerical image. The final image checks the
# constructed recipe against film_identity.py before it can be published.
COPY --from=film-runtime /opt/python /opt/python
COPY --from=film-runtime /opt/runtime /opt/runtime
COPY --from=film-runtime /opt/spektrafilm /opt/spektrafilm
COPY --from=film-runtime \
     /opt/probe/film.py \
     /opt/probe/bundle.py \
     /opt/probe/
COPY tools/development/finished_jpeg.py \
     tools/development/film_identity.py \
     /opt/probe/
COPY --from=film-runtime \
     /opt/processing-bundle.json /opt/processing-bundle.json
RUN PYTHONPATH=/opt/slipstream-photo:/opt/spektrafilm/src:/opt/probe \
    /opt/runtime/bin/python -c 'import film, film_adapter; _, recipe = film.make_simulator(); film_adapter.check_recipe(recipe)'
RUN sha256sum /opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc \
        /opt/slipstream-photo/adapter.py /opt/slipstream-photo/film_adapter.py \
        /usr/local/bin/slipstream-processing-photo-worker \
        /opt/processing-bundle.json /opt/probe/film.py /opt/probe/bundle.py \
        /opt/probe/film_identity.py /opt/probe/finished_jpeg.py /opt/os-packages.txt \
        | sed 's#  /#  #g' | sha256sum | cut -d' ' -f1 > /opt/slipstream-photo/bundle \
    && cat /opt/slipstream-photo/bundle
# The deploy pipeline pins the numerical bundle recorded by the build:
#   docker build -f tools/processing/photo/Dockerfile -t slipstream:processing-photo .
#   PHOTO_BUNDLE=$(docker run --rm --entrypoint cat slipstream:processing-photo \
#     /opt/slipstream-photo/bundle)
# --entrypoint cat is required: the image entrypoint is the worker, so a plain
# `docker run <image> cat ...` passes the path to the worker and prints nothing.
ARG PHOTO_BUNDLE=unpinned
LABEL slipstream.processing.photo.bundle="${PHOTO_BUNDLE}"
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/slipstream-processing-photo-worker"]
