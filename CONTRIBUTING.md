# Contributing

## Language and Writing

Write project documentation, Issue titles and bodies, pull request titles and
descriptions, and commit messages in English. This rule applies to submissions
made through the GitHub UI, CLI, API, or automation. Conversation with contributors
may use their preferred language.

Use short sentences, active voice, American spelling, and consistent terms.
Prefer plain technical English inspired by ASD-STE100; formal compliance is not
required. Use the domain terms in [`CONTEXT.md`](CONTEXT.md). Preserve exact
identifiers, filenames, user-provided names, and quoted diagnostic output in their
original language. Explain non-English evidence in English when needed.

## Issues

Use the Bug Report form for incorrect behavior and the Improvement Proposal form
for a product, architecture, or workflow change. Search existing Issues first and
link related work. Keep each Issue focused on one coherent outcome; related fixes
may share an Issue when they have the same acceptance criteria.

Describe the observed problem before proposing a solution. Separate measurements
from assumptions. Record the affected version and reproduction steps when known;
state what is unknown instead of guessing. Remove secrets and private photo data
from attachments and logs.

Before implementation, agree on scope and observable acceptance criteria, and
update the affected Product and Design Specs as required by
[`AGENTS.md`](AGENTS.md). For work that spans several Issues, use a parent Issue
with links and a completion checklist. Keep investigation evidence and progress
in the Issue or pull request, not in durable specifications.

## Prerequisites

The native Preview boundary is currently verified on Linux only.

- Rust `1.97.1` with Cargo, Clippy, and rustfmt
- Bun `1.4.0`
- Docker CLI with the Compose plugin supporting `docker compose config --format json`
- A C++17 compiler, a C compiler, CMake, and Python 3
- `pkg-config`, LibRaw, libjpeg-turbo, libvips, and LittleCMS development headers

The canonical `verify` gate uses Docker Compose only for its daemon-free
configuration parser check; it does not require a running Docker daemon.

On Debian/Ubuntu, install native dependencies with:

```sh
sudo apt-get install build-essential cmake pkg-config libraw-dev libjpeg-dev libvips-dev liblcms2-dev
```

Install the exact Rust and Bun versions recorded in `rust-toolchain.toml` and `package.json`, then prepare the checkout:

```sh
rustup toolchain install 1.97.1 --profile minimal --component clippy --component rustfmt
rustc --version # 1.97.1
curl -fsSL https://bun.com/install | bash -s "bun-v1.4.0"
bun --version # 1.4.0
./scripts/prepare-worktree.sh
```

## Worktree setup

Create Issue worktrees from a fetched `origin/main` commit to avoid an outdated
local branch. This does not change files in the original checkout:

```sh
repo=$(git rev-parse --show-toplevel)
worktree="$repo/../slipstream-issue-XYZ"
branch=feat/XYZ

git -C "$repo" fetch --prune origin main
base=$(git -C "$repo" rev-parse origin/main)
git -C "$repo" worktree add "$worktree" -b "$branch" "$base"

"$worktree/scripts/prepare-worktree.sh" "$worktree"
```

Run `scripts/prepare-worktree.sh [worktree-root]` from the target checkout's
root, or supply its root path. The default is the current directory. Ordinary
clones and linked worktrees use the same command.

The script reads the expected versions from that checkout's `package.json` and
`rust-toolchain.toml`, checks Bun, Rust, rustfmt, and Clippy, then runs
`bun install --frozen-lockfile` followed by `cargo fetch --locked`. It checks
that neither lockfile changed and runs `git diff --check`.

Repeat the command when dependencies change. Existing edits need not be committed
or stashed first; whitespace errors in those edits still fail `git diff --check`.
On failure, fix the reported prerequisite and rerun. The script does not reset,
clean, format, commit, install host packages or browsers, or run the full gate.
It does not undo a failed install or discard edits. Run `bun run verify`
separately before handoff.

Run the focused preparation checks with `bun run test:worktree-preflight`.
They use temporary Git repositories and stub tool commands, without downloading
dependencies. They also run as part of `test:fast` and `verify`.

Bun owns Web builds and browser-test tooling; the production server and Preview pipeline are Rust. The Rust Preview boundary uses owned C wrappers around LibRaw/libjpeg and libvips, with one process-global libvips lifecycle. Derivative processing is bounded to two concurrent jobs per cache directory, with 128 MiB input JPEG, 100 million decoded-pixel, 64 MiB output JPEG, and 256 MiB LibRaw native-memory limits.

## Verification

Use the repository commands rather than invoking individual tools in CI or reviews:

```sh
bun run test:rust
bun run test:cli
bun run test:cli-package
bun run test:container-input
bun run test:fast
bun run verify
```

`test:cli` runs the focused command parser, output, and real CLI-to-service tests. It covers `status`, Library checks, Folder, Album, and Photo commands, including Preview downloads, without running the complete repository gate.

`test:cli-package` verifies the source-bound Linux amd64 candidate archive, its checksum and fixed metadata, and no-replace behavior using synthetic inputs. It runs in `test:fast` and `verify`. To build an actual candidate from a clean committed tree, run `python3 scripts/package-cli.py`; see [CLI Candidate Installation](docs/cli-install.md). Packaging does not publish a tag or upload a release.

Install the Playwright Chromium browser once before running the gates:

```sh
bun x playwright install chromium
```

If the host platform is newer than the Playwright browser installer supports, point the test at an existing compatible Chromium binary:

```sh
PLAYWRIGHT_CHROMIUM_EXECUTABLE=/absolute/path/to/chrome bun run test:browser
```

`test:browser` runs all browser scenarios against the Rust `slipstream-server` binary. It builds the Web assets, starts the binary on a private loopback TCP port behind a local HTTPS proxy, and gives it independent temporary state and cache directories. Fixtures provision a generated Access Token and establish real sessions; the checked-in test certificate and key under `tools/test-tls/` are for synthetic fixtures only. The combined CLI-to-Web scenario runs the compiled `slipstream` client the same way. Use `SLIPSTREAM_SERVER_BINARY`, `SLIPSTREAM_CLI_BINARY`, or `SLIPSTREAM_WEB_ROOT` only when testing separately built Rust binaries or a Web directory. `test:rust` checks formatting, denies Clippy warnings, and runs Rust tests serially because the native Preview stack has one process-global libvips lifecycle. `test:fast` adds Bun/TypeScript linting and type checking plus the Rust-only browser suite. `verify` also checks repository formatting and builds Rust plus the Web application. GitHub Actions invokes the same `verify` command.

The Rust workspace contains the production Library/Preview core and HTTP server in `crates/slipstream-server`. The production-language contract is in [`design/rust-server.md`](design/rust-server.md). Shared JSON and SQL vectors live in [`compatibility/`](compatibility/); Rust compatibility tests consume them.

**Fixture coverage boundary.** The shared validation protocol vectors run against an empty fixture Library and pin validation, error, and contract-envelope shapes. The Browse vectors use a generated RAW/JPEG pair and Album to pin non-empty windows, ordering, Preview and Thumbnail hydration, metadata, status, membership, Browse position, and Album mutations. The cache vectors seed one Photo and one web asset and pin derivative and web-asset response headers, ETag identity, and revalidation. Every file under `compatibility/` must have an executing consumer; the compatibility inventory test enforces this rule.

### Continuous integration

[`verify.yml`](.github/workflows/verify.yml) runs `bun run verify` once on a
GitHub-hosted `ubuntu-latest` runner. That single job is the required `verify`
check, so the gate keeps one runner and reuses work instead of adding runners:

- The job restores a Rust cache keyed on `Cargo.lock` and
  `rust-toolchain.toml`: `~/.cargo/registry`, `~/.cargo/git`, and `target`.
  A restored cache removes dependency compilation. Formatting, Clippy, every
  test, the Web build, and the browser suite still run in full.
- `CARGO_PROFILE_DEV_DEBUG` and `CARGO_PROFILE_TEST_DEBUG` are `0`. The job
  never sets `RUST_BACKTRACE`, so nothing there reads debug information;
  leaving it out shortens compilation and linking and keeps the cache near
  220 MB compressed instead of 530 MB. To debug a CI-only Rust failure with
  backtraces, reproduce it locally without those variables.
- The bundled browser suite keeps the default worker layout, so its scenarios
  still run one file at a time. Running it with `fullyParallel` and all four
  runner CPUs is 2.2× faster (6.5m to 3.0m on the runner), but the window,
  scroll, and rendered-thumbnail scenarios then fail intermittently: at four
  workers, run [36050307251](https://github.com/suraciii/slipstream/actions/runs/36050307251)
  failed `out-of-order window settlements render every loaded Photo position`,
  and three local runs on four CPUs failed two to seven scenarios each. Every
  failure was a time or rendered-state assertion (`expect` default 5s,
  `page.clock` targets) that holds under the serial layout. Enable full
  parallelism only after those scenarios tolerate concurrent load.

## Photo fixtures

Do not commit real photographs, RAW files, generated Previews, SQLite databases, or Slipstream runtime state. Tests that require a real camera file must accept an explicit local path and skip with a clear reason when the file is unavailable. Repository fixtures must be generated, minimal, redistributable, and contain no private photography.

Ordinary `test:rust` and `verify` runs report the real-camera tests as ignored. Run the opt-in RAW safety gate with the configured Sony Original File path:

```sh
SLIPSTREAM_RAW_SAMPLE=/absolute/path/to/sample.ARW bun run test:raw
```

`test:raw` runs every native, service, and browser real-camera scenario serially and fails clearly when `SLIPSTREAM_RAW_SAMPLE` is absent or does not identify the configured sample. Each scenario compares the operated Original's SHA-256 digest and stable filesystem metadata before and after its read, LibRaw, or Preview workflow. A scenario that copies the sample into an isolated Library checks both that copy and the source sample.

The opt-in [development qualification harness](tools/development/README.md)
exercises pinned darktable and Spektrafilm processes in an isolated CPU container.
Real-camera probes require an explicit local RAW fixture and write private
evidence outside the repository. The documented `engine-checks` image target
uses synthetic inputs to check the pinned engine patch, exact image equivalence,
and transform workspace model without a camera. Run it after an engine or bundle
change. Its host-side failure and cancellation regressions run with
`bun run test:development-runner` and are included in `test:fast` and `verify`.
Successful probes do not establish processing support beyond their recorded
checks; the governing qualification Issues own acceptance.

The independent [processing isolation qualifier](tools/processing/README.md)
exercises the actual host Rust launcher, pinned native fixture container,
systemd/cgroup limits, bounded storage, and durable recovery. Run its focused
non-root coverage with `cargo test --locked -p slipstream-processing`; workspace
Rust checks include it. Its opt-in kernel verifier requires root and explicit
local worker/Web image IDs. It uses only a private synthetic Library and is not
part of the daemon-free `verify` gate. Passing fixture qualification does not
enable photo editing or qualify an image engine's memory envelope.

The closed [Film measurement profile](design/processing-film-measurement.md)
runs the pinned image engine inside that boundary against explicit operator
fixtures and independent output references. Its opt-in verifier returns resource
and image-identity evidence, not Exports or accepted production budgets. Host
fixture-preparation checks run with `bun run test:processing-tools` and are part
of `test:fast` and `verify`; actual engine and kernel qualification remain separate.

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
SLIPSTREAM_PUBLIC_ORIGIN=https://photos.example.com \
cargo run --locked -p slipstream-server
```

`SLIPSTREAM_DATABASE_BASENAME` defaults to `library.sqlite`. The host defaults
to loopback; set `SLIPSTREAM_HOST` to the private listener address. Configure
the canonical HTTPS origin and provision an Access Token with the stopped-server
[access administration procedure](docs/deployment.md#access-administration).
Without a token, only the public shell, access status, and health check are
available. `GET /api/status` requires valid access and reports Library
initialization, scan, and publication state.

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
listener configuration. It commits the binding and Location changes once, then
completes a normal scan before reporting success.

## Container verification

The production image uses Bun only while building the Web, Rust `1.97.1` to
build the server, and an Ubuntu runtime containing the Rust binary, Web assets,
native runtime libraries, and curl for the `/healthz` check. It has no Node,
Bun, Sharp, or Node-API runtime. `bun run test:container-input` checks the
fixed container inputs without building an image. The explicit Linux amd64
`docker buildx build` command, qualification-output boundary, and supported
digest-only Compose operation are defined by
[`docs/deployment.md`](docs/deployment.md).

The bind address exposed on the host is configured with `SLIPSTREAM_BIND_ADDRESS` in [`compose.yaml`](compose.yaml), defaulting to loopback. The backend must remain private behind the configured HTTPS proxy. Supported Compose operations use [`scripts/compose`](scripts/compose); their input grammar and Linux-local Docker deployment contract are defined by [`docs/deployment.md`](docs/deployment.md).
