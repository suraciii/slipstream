# Contributing

## Prerequisites

- Bun `1.4.0`

Install the exact version recorded in `package.json`, then install dependencies from the lockfile:

```sh
curl -fsSL https://bun.com/install | bash -s "bun-v1.4.0"
bun install --frozen-lockfile
```

## Verification

Use the repository commands rather than invoking individual tools in CI or reviews:

```sh
bun run test:fast
bun run verify
```

`test:fast` runs linting, type checking, and unit tests. `verify` also checks formatting and builds the server and Web applications. GitHub Actions invokes the same `verify` command.

## Photo fixtures

Do not commit real photographs, RAW files, generated Previews, SQLite databases, or Slipstream runtime state. Tests that require a real camera file must accept an explicit local path and skip with a clear reason when the file is unavailable. Repository fixtures must be generated, minimal, redistributable, and contain no private photography.
