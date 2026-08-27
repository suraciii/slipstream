FROM oven/bun:1.4.0 AS web-build
WORKDIR /src

# The Web imports the shared protocol types from the rollback package, but does
# not need the rollback package's runtime or native dependencies to build.
COPY package.json bun.lock tsconfig.json ./
COPY apps/web/package.json apps/web/package.json
COPY apps/server/src/protocol.ts apps/server/src/protocol.ts
COPY apps/web apps/web
RUN bun install --frozen-lockfile --ignore-scripts
RUN bun run --cwd apps/web build

FROM rust:1.97.1-bookworm AS rust-toolchain

FROM ubuntu:26.04 AS rust-build
WORKDIR /src

COPY --from=rust-toolchain /usr/local/cargo /usr/local/cargo
COPY --from=rust-toolchain /usr/local/rustup /usr/local/rustup
ENV PATH=/usr/local/cargo/bin:$PATH \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo

RUN apt-get update \
    && apt-get install --no-install-recommends --yes \
        build-essential \
        ca-certificates \
        libjpeg-dev \
        liblcms2-dev \
        libraw-dev \
        libvips-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY compatibility compatibility
RUN cargo build --release --locked -p slipstream-server

FROM ubuntu:26.04 AS runtime

RUN apt-get update \
    && apt-get install --no-install-recommends --yes \
        ca-certificates \
        curl \
        libjpeg-turbo8 \
        liblcms2-2 \
        libraw23t64 \
        libvips42t64 \
    && rm -rf /var/lib/apt/lists/* \
    && existing_group=$(getent group 1000 | cut -d: -f1) \
    && if [ -z "$existing_group" ]; then groupadd --gid 1000 slipstream; existing_group=slipstream; fi \
    && existing_user=$(getent passwd 1000 | cut -d: -f1) \
    && if [ -z "$existing_user" ]; then useradd --uid 1000 --gid 1000 --create-home --home-dir /home/slipstream --shell /usr/sbin/nologin slipstream; fi \
    && install -d -o 1000 -g 1000 -m 0700 /state /cache /tmp/slipstream

COPY --from=rust-build /src/target/release/slipstream-server /usr/local/bin/slipstream-server
COPY --from=web-build /src/apps/web/dist /app/web
RUN chown -R 1000:1000 /app /usr/local/bin/slipstream-server

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
