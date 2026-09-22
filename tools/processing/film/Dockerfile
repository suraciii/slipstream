FROM rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --locked --release --package slipstream-processing --bin slipstream-processing-film-worker

# build.py checks this retained local parent against its immutable image ID;
# its complete numerical source/package inventory is checked below as well.
FROM slipstream:358-pre-grain-reclaim AS adapter-source
COPY tools/processing/film/requirements.lock /opt/film-measurement/requirements.lock
RUN uv --no-cache pip install --python /opt/runtime/bin/python --target /opt/film-validator-deps \
    --require-hashes --no-compile-bytecode -r /opt/film-measurement/requirements.lock
COPY tools/processing/film/adapter.py tools/processing/film/contract.py \
     tools/processing/film/package.py tools/processing/film/Dockerfile /opt/film-measurement/
COPY design/schemas/processing-film-measurement.schema.json /opt/film-measurement/schema.json
RUN /opt/runtime/bin/python /opt/probe/bundle.py --verify

FROM adapter-source AS film
COPY --from=build /src/target/release/slipstream-processing-film-worker /usr/local/bin/slipstream-processing-film-worker
COPY LICENSE THIRD-PARTY-NOTICES.md RUST-LICENSES.html /usr/share/doc/slipstream/
RUN /opt/runtime/bin/python /opt/film-measurement/package.py
LABEL slipstream.processing.film.numerical_bundle="eeaae45f9cd198d8362cc9b5262e8da53b36ef11d5eefc3441bb0beb5b7aa649" \
      slipstream.processing.film.recipe="b1429da34fb4cd8ef08d1140da946f8a23ee8b68de1d66a58bd3797242527dd9" \
      slipstream.processing.film.input_icc_sha256="7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe" \
      slipstream.processing.film.output_icc_sha256="b44e86e44d44993a3a9a880626f9832e9c37e2234caba501548f9114bada6d21" \
      slipstream.processing.film.procedure="film-once-empty-cache-v1"
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/slipstream-processing-film-worker"]

FROM film AS adapter-checks
USER 0:0
COPY tools/processing/film/test_contract.py tools/processing/film/test_adapter.py \
     tools/processing/film/local-plan-vectors.json tools/processing/film/references.json \
     /opt/film-checks/
COPY design/schemas/processing-film-measurement-canonical-vectors.json /opt/film-checks/canonical-vectors.json
ENV PYTHONPATH=/opt/film-measurement:/opt/spektrafilm/src
ENTRYPOINT ["/opt/runtime/bin/python", "-m", "unittest", "discover", "-s", "/opt/film-checks", "-v"]

FROM film AS qualification
