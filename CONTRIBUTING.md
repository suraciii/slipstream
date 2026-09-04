# Contributing

## Prerequisites

The native Preview boundary is currently verified on Linux only.

- Rust `1.97.1` with Cargo, Clippy, and rustfmt
- Bun `1.4.0`
- A C++17 compiler and Python 3
- `pkg-config`, LibRaw, libjpeg-turbo, libvips, and LittleCMS development headers

On Debian/Ubuntu, install native dependencies with:

```sh
sudo apt-get install build-essential pkg-config libraw-dev libjpeg-dev libvips-dev liblcms2-dev
```

Install the exact Rust and Bun versions recorded in `rust-toolchain.toml` and `package.json`, then install dependencies from the lockfiles:

```sh
rustup toolchain install 1.97.1 --profile minimal --component clippy --component rustfmt
rustc --version # 1.97.1
curl -fsSL https://bun.com/install | bash -s "bun-v1.4.0"
bun --version # 1.4.0
bun install --frozen-lockfile
cargo fetch --locked
```

Bun owns Web builds and browser-test tooling; the production server and Preview pipeline are Rust. The Rust Preview boundary uses owned C wrappers around LibRaw/libjpeg and libvips, with one process-global libvips lifecycle. Derivative processing is bounded to two concurrent jobs per cache directory, with 128 MiB input JPEG, 100 million decoded-pixel, 64 MiB output JPEG, and 256 MiB LibRaw native-memory limits.

## Verification

Use the repository commands rather than invoking individual tools in CI or reviews:

```sh
bun run test:rust
bun run test:fast
bun run verify
```

Install the Playwright Chromium browser once before running the gates:

```sh
bun x playwright install chromium
```

If the host platform is newer than the Playwright browser installer supports, point the test at an existing compatible Chromium binary:

```sh
PLAYWRIGHT_CHROMIUM_EXECUTABLE=/absolute/path/to/chrome bun run test:browser
```

`test:browser` runs all browser scenarios against the Rust `slipstream-server` binary. It builds the Web assets, starts the binary on a real loopback TCP port, and gives it independent temporary state and cache directories. Use `SLIPSTREAM_SERVER_BINARY` or `SLIPSTREAM_WEB_ROOT` only when testing a separately built Rust binary or Web directory. `test:rust` checks formatting, denies Clippy warnings, and runs Rust tests serially because the native Preview stack has one process-global libvips lifecycle. `test:fast` adds Bun/TypeScript linting and type checking plus the Rust-only browser suite. `verify` also checks repository formatting and builds Rust plus the Web application. GitHub Actions invokes the same `verify` command.

The Rust workspace contains the production Library/Preview core and HTTP server in `crates/slipstream-server`. The production-language contract is in [`design/rust-server.md`](design/rust-server.md). Shared JSON and SQL vectors live in [`compatibility/`](compatibility/); Rust compatibility tests consume them.

## Photo fixtures

Do not commit real photographs, RAW files, generated Previews, SQLite databases, or Slipstream runtime state. Tests that require a real camera file must accept an explicit local path and skip with a clear reason when the file is unavailable. Repository fixtures must be generated, minimal, redistributable, and contain no private photography.

Ordinary `test:rust` and `verify` runs report the real-camera tests as ignored. Run the opt-in RAW safety gate with the configured Sony Original File path:

```sh
SLIPSTREAM_RAW_SAMPLE=/absolute/path/to/sample.ARW bun run test:raw
```

`test:raw` runs every real-camera scenario serially and fails clearly when `SLIPSTREAM_RAW_SAMPLE` is absent or does not identify the configured sample. Each scenario compares the operated Original's SHA-256 digest and stable filesystem metadata before and after its read, LibRaw, or Preview workflow. A scenario that copies the sample into an isolated Library checks both that copy and the source sample.

## Server startup

Build the workspace, then configure one Library and application-owned state locations with absolute paths. The Rust server requires built Web assets and may receive their absolute location through `SLIPSTREAM_WEB_ROOT`:

```sh
bun run --cwd apps/web build
SLIPSTREAM_LIBRARY_ROOT=/photos \
SLIPSTREAM_STATE_DIRECTORY=/var/lib/slipstream \
SLIPSTREAM_CACHE_DIRECTORY=/var/cache/slipstream \
SLIPSTREAM_WEB_ROOT="$PWD/apps/web/dist" \
SLIPSTREAM_HOST=127.0.0.1 \
SLIPSTREAM_PORT=3000 \
SLIPSTREAM_PUBLIC_ORIGIN=http://127.0.0.1:3000 \
cargo run --locked -p slipstream-server
```

`SLIPSTREAM_DATABASE_BASENAME` defaults to `library.sqlite`. The host defaults
to loopback; set `SLIPSTREAM_HOST` to the listener address. Set the required
`SLIPSTREAM_PUBLIC_ORIGIN` to the exact browser-visible `http` or `https`
origin, including a non-default port. It is not inferred from the listener,
request Host, or forwarded headers. Other than the local `GET /healthz` or
`HEAD /healthz` readiness probe, normal origin-form requests must use its Host
authority and absolute targets must match it; browser `POST` and `DELETE`
requests must also use it as `Origin`. `GET /api/status` separately reports
Library initialization, scan, and publication state.

To expand a stopped schema-v5 Library to an ancestor Folder, first create and
record a verified consistent backup with the service stopped (see
[`docs/deployment.md`](docs/deployment.md)). Then set
`SLIPSTREAM_LIBRARY_ROOT` to the proposed canonical ancestor while retaining
the same state, cache, and database settings, and run:

```sh
cargo run --locked -p slipstream-server -- expand-library
```

The offline command rejects a running database, sidecars, non-v5 state, an
unrelated Folder, descriptor mismatch, invalid remembered Locations, and
scan-limit failures. It never opens HTTP and does not require
`SLIPSTREAM_PUBLIC_ORIGIN`. It commits the binding and Location changes once,
then completes a normal scan before reporting success.

## Container verification

The production image uses Bun only while building the Web, Rust `1.97.1` to build the server, and an Ubuntu runtime containing the Rust binary, Web assets, native runtime libraries, and curl for the `/healthz` check. It has no Node, Bun, Sharp, or Node-API runtime. Build an image before an operator-controlled deployment:

```sh
commit=$(git rev-parse HEAD)
docker build --build-arg "SLIPSTREAM_VCS_REF=$commit" --tag slipstream:local .
```

Inspect the digest, image user, and runtime contents; the deployment contract
is in [`docs/deployment.md`](docs/deployment.md).

The bind address exposed on the host is configured with `SLIPSTREAM_BIND_ADDRESS` in [`compose.yaml`](compose.yaml), defaulting to loopback. Use a host Tailscale address when exposing the application only through Tailscale. The host-agnostic deployment contract is [`docs/deployment.md`](docs/deployment.md).
