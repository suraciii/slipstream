FROM oven/bun:1.4.0@sha256:5ff609364c049b54eb0ff560ec96319729a972078ef2c755d758f0c6ef89c2d6 AS web-build
WORKDIR /src

# Bun is used only to build the Web application. The Rust service is built in
# the following stages and is the only production server.
COPY package.json bun.lock tsconfig.json ./
COPY apps/web/package.json apps/web/package.json
COPY apps/web apps/web
RUN bun install --frozen-lockfile --ignore-scripts
RUN bun run --cwd apps/web build

FROM rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS rust-toolchain

FROM ubuntu:26.04@sha256:2260313b31c8c011cd2eebe728008efac1b3982be73eb71348ea2648d2c0e09b AS rust-build
WORKDIR /src

# The pinned Ubuntu rootfs has no CA bundle. Bootstrap HTTPS from the already
# digest-pinned Rust stage, then install the snapshot-pinned Ubuntu package.
COPY --from=rust-toolchain /etc/ssl/certs/ca-certificates.crt /usr/local/share/slipstream-ca-certificates.crt
COPY docker/apt/bootstrap-ca.conf /etc/apt/apt.conf.d/00slipstream-bootstrap-ca
COPY docker/apt/ubuntu.sources /etc/apt/sources.list.d/ubuntu.sources
COPY docker/apt/build-amd64.lock /tmp/apt-packages.lock
COPY --from=rust-toolchain /usr/local/cargo /usr/local/cargo
COPY --from=rust-toolchain /usr/local/rustup /usr/local/rustup
ENV PATH=/usr/local/cargo/bin:$PATH \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo

RUN --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    --mount=type=cache,target=/var/cache/apt,sharing=locked \
    apt-get update --error-on=any \
    && apt-get install --no-install-recommends --yes $(cat /tmp/apt-packages.lock) \
    && rm --force /etc/apt/apt.conf.d/00slipstream-bootstrap-ca /usr/local/share/slipstream-ca-certificates.crt /tmp/apt-packages.lock

COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY compatibility compatibility
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo build --release --locked -p slipstream-server

FROM ubuntu:26.04@sha256:2260313b31c8c011cd2eebe728008efac1b3982be73eb71348ea2648d2c0e09b AS runtime-rootfs

COPY --from=rust-toolchain /etc/ssl/certs/ca-certificates.crt /usr/local/share/slipstream-ca-certificates.crt
COPY docker/apt/bootstrap-ca.conf /etc/apt/apt.conf.d/00slipstream-bootstrap-ca
COPY docker/apt/ubuntu.sources /etc/apt/sources.list.d/ubuntu.sources
COPY docker/apt/runtime-amd64.lock /tmp/apt-packages.lock
RUN --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    --mount=type=cache,target=/var/cache/apt,sharing=locked \
    apt-get update --error-on=any \
    && apt-get install --no-install-recommends --yes $(cat /tmp/apt-packages.lock) \
    && rm --force /etc/apt/apt.conf.d/00slipstream-bootstrap-ca /usr/local/share/slipstream-ca-certificates.crt /tmp/apt-packages.lock \
    && rm --force /usr/bin/pebble \
    && rm --recursive --force /var/lib/pebble \
    && existing_group=$(getent group 1000 | cut -d: -f1) \
    && if [ -z "$existing_group" ]; then groupadd --gid 1000 slipstream; existing_group=slipstream; fi \
    && existing_user=$(getent passwd 1000 | cut -d: -f1) \
    && if [ -z "$existing_user" ]; then useradd --uid 1000 --gid 1000 --create-home --home-dir /home/slipstream --shell /usr/sbin/nologin slipstream; fi \
    && install -d -o 1000 -g 1000 -m 0700 /home/slipstream /state /cache /tmp/slipstream

COPY --from=rust-build /src/target/release/slipstream-server /usr/local/bin/slipstream-server
COPY --from=web-build /src/apps/web/dist /app/web
COPY LICENSE THIRD-PARTY-NOTICES.md RUST-LICENSES.html /usr/share/doc/slipstream/
RUN chown -R 1000:1000 /app /usr/local/bin/slipstream-server \
    && chmod 0444 /usr/share/doc/slipstream/*

FROM scratch AS runtime

ARG SLIPSTREAM_VCS_REF=unknown
LABEL org.opencontainers.image.revision=$SLIPSTREAM_VCS_REF

# Copy the prepared filesystem as one layer so the final image neither inherits
# Ubuntu's Pebble layer nor distributes a whiteout for the removed binary.
COPY --from=runtime-rootfs / /

ENV SLIPSTREAM_LIBRARY_ROOT=/originals \
    SLIPSTREAM_STATE_DIRECTORY=/state \
    SLIPSTREAM_CACHE_DIRECTORY=/cache \
    SLIPSTREAM_WEB_ROOT=/app/web \
    SLIPSTREAM_HOST=0.0.0.0 \
    SLIPSTREAM_PORT=3000 \
    HOME=/home/slipstream \
    TMPDIR=/tmp/slipstream \
    XDG_CACHE_HOME=/tmp/slipstream \
    XDG_CONFIG_HOME=/tmp/slipstream \
    XDG_DATA_HOME=/tmp/slipstream

EXPOSE 3000
USER 1000:1000
STOPSIGNAL SIGTERM

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "http://127.0.0.1:3000/healthz"]
ENTRYPOINT ["/usr/local/bin/slipstream-server"]
