FROM rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --locked --release --package slipstream-processing --bin slipstream-processing-worker

FROM ubuntu:26.04@sha256:2260313b31c8c011cd2eebe728008efac1b3982be73eb71348ea2648d2c0e09b
COPY --from=build /src/target/release/slipstream-processing-worker /usr/local/bin/slipstream-processing-worker
COPY LICENSE THIRD-PARTY-NOTICES.md RUST-LICENSES.html /usr/share/doc/slipstream/
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/slipstream-processing-worker"]
