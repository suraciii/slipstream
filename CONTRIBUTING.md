# Contributing

## Prerequisites

The native Preview boundary is currently verified on Linux only.

- Bun `1.4.0`
- Node.js `22.23.1` (required by `node-gyp`)
- A C++17 compiler and Python 3
- `pkg-config`, LibRaw development headers, and libjpeg-turbo development headers

On Debian/Ubuntu, install native dependencies with:

```sh
sudo apt-get install build-essential pkg-config libraw-dev libjpeg-dev
```

Install the exact Bun version recorded in `package.json`, make the exact Node.js version available, then install dependencies from the lockfile:

```sh
curl -fsSL https://bun.com/install | bash -s "bun-v1.4.0"
node --version # v22.23.1
bun install --frozen-lockfile
```

The workspace install builds only the production LibRaw addon. `test:fast` explicitly builds a separate LibRaw test addon; the Server TypeScript build does not rebuild either artifact. Sharp/libvips owns JPEG decode, orientation, Lanczos resize, ICC conversion or preservation, and encoding. Derivative processing is bounded to two concurrent jobs per cache directory.

## Verification

Use the repository commands rather than invoking individual tools in CI or reviews:

```sh
bun run test:fast
bun run verify
```

`test:fast` runs linting, type checking, the native build, and unit tests. `verify` also checks formatting and builds the server and Web applications. GitHub Actions invokes the same `verify` command.

## Photo fixtures

Do not commit real photographs, RAW files, generated Previews, SQLite databases, or Slipstream runtime state. Tests that require a real camera file must accept an explicit local path and skip with a clear reason when the file is unavailable. Repository fixtures must be generated, minimal, redistributable, and contain no private photography.

Run the opt-in LibRaw integration test with an explicit local Original File path:

```sh
SLIPSTREAM_RAW_SAMPLE=/absolute/path/to/sample.ARW bun run test apps/server/src/preview/libraw-preview.test.ts
```

The test hashes the Original before and after extraction and fails if its bytes change.
