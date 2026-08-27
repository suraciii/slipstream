# Contributing

## Prerequisites

The native Preview boundary is currently verified on Linux only.

- Rust `1.97.1` with Cargo, Clippy, and rustfmt
- Bun `1.4.0`
- Node.js `22.23.1` (transitional server and `node-gyp` only)
- A C++17 compiler and Python 3
- `pkg-config`, LibRaw, libjpeg-turbo, libvips, and LittleCMS development headers

On Debian/Ubuntu, install native dependencies with:

```sh
sudo apt-get install build-essential pkg-config libraw-dev libjpeg-dev libvips-dev liblcms2-dev
```

Install the exact Rust and Bun versions recorded in `rust-toolchain.toml` and `package.json`, make the transitional Node.js version available, then install dependencies from the lockfiles:

```sh
rustup toolchain install 1.97.1 --profile minimal --component clippy --component rustfmt
rustc --version # 1.97.1
curl -fsSL https://bun.com/install | bash -s "bun-v1.4.0"
node --version # v22.23.1
bun install --frozen-lockfile
cargo fetch --locked
```

The workspace install builds only the transitional production LibRaw addon. `test:fast` explicitly builds a separate LibRaw test addon; the Server TypeScript build does not rebuild either artifact. The Rust Preview boundary uses owned C wrappers around LibRaw/libjpeg and libvips, with one process-global libvips lifecycle. Derivative processing is bounded to two concurrent jobs per cache directory, with 128 MiB input JPEG, 100 million decoded-pixel, 64 MiB output JPEG, and 256 MiB LibRaw native-memory limits.

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

`test:rust` checks formatting, denies Clippy warnings, and runs the Rust compatibility tests. `test:fast` adds TypeScript linting and type checking, the transitional native build, unit/integration tests, and the real Chromium browser test. `verify` also checks repository formatting and builds Rust plus the transitional server and Web applications. GitHub Actions invokes the same `verify` command.

The Rust workspace contains the production Library/Preview core and an opt-in Rust HTTP server in `crates/slipstream-server`. The TypeScript server remains the rollback baseline until Docker cutover. The production-language and cutover contract is in [`design/rust-server.md`](design/rust-server.md). Shared JSON and SQL vectors live in [`compatibility/`](compatibility/); both Rust and TypeScript tests consume them.

## Photo fixtures

Do not commit real photographs, RAW files, generated Previews, SQLite databases, or Slipstream runtime state. Tests that require a real camera file must accept an explicit local path and skip with a clear reason when the file is unavailable. Repository fixtures must be generated, minimal, redistributable, and contain no private photography.

Run the opt-in LibRaw integration test with an explicit local Original File path:

```sh
SLIPSTREAM_RAW_SAMPLE=/absolute/path/to/sample.ARW bun run test apps/server/src/preview/libraw-preview.test.ts
```

The test hashes the Original before and after extraction and fails if its bytes change.

## Server startup

Build the workspace, then configure one Library and application-owned state locations with absolute paths. The opt-in Rust server requires built Web assets and may receive their absolute location through `SLIPSTREAM_WEB_ROOT`:

```sh
bun run --cwd apps/web build
SLIPSTREAM_LIBRARY_ROOT=/photos \
SLIPSTREAM_STATE_DIRECTORY=/var/lib/slipstream \
SLIPSTREAM_CACHE_DIRECTORY=/var/cache/slipstream \
SLIPSTREAM_WEB_ROOT="$PWD/apps/web/dist" \
SLIPSTREAM_HOST=127.0.0.1 \
SLIPSTREAM_PORT=3000 \
cargo run --locked -p slipstream-server
```

The transitional rollback server still starts with `node apps/server/dist/main.js` using the same existing variables. Never start both servers against one SQLite database. `SLIPSTREAM_DATABASE_BASENAME` defaults to `library.sqlite`. The host defaults to loopback; set `SLIPSTREAM_HOST=0.0.0.0` only for an explicitly trusted LAN deployment. `GET /healthz` reports readiness after the initial scan, Preview startup, and HTTP bind.
