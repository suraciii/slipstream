# Processing tooling

This directory builds and exercises optional processing modules of the single
Slipstream application container: native darktable and standalone SpektraFilm. The
[local executor design](../../design/processing-executor.md) owns the
execution and failure lifetime; the
[darktable integration](../../design/darktable-integration.md) and
[development color](../../design/development-color.md) designs own the
semantic requests and output contracts.

There is no host processing launcher, processing systemd unit, worker
container, launcher socket, or processing Compose overlay: each module is
detected from the application image at startup. An image without a module keeps
Library operations available and reports that module unavailable.

## Build

Build the application image with the native extension attached. The helper
first builds (or reuses) the normal application runtime from the repository
`Dockerfile`, then extends it through `photo/Dockerfile` with the pinned
native engine, the discovered MCP metadata, the ICC output profile, and the
deterministic bundle manifest:

```sh
python3 tools/processing/photo/build.py \
  --darktable-source /absolute/path/to/darktable-native \
  --darktable-commit <40-lowercase-hex-commit> \
  --tag slipstream:photo-local
```

The helper requires a clean native source checkout at the exact commit
(including initialized submodules), verifies the repository's pinned ICC
asset, uses BuildKit's named `darktable` context so the image never depends
on a host `.git` path, and builds twice: the first pass discovers the exact
MCP tools/modules/schemas and derives the bundle identity, the second pass
pins that identity in the image label. It refuses an image that does not
extend the exact application runtime layers or that does not keep the
Slipstream server entrypoint.

It prints the application image ID, the extended image ID, the 64-character
bundle digest, and the native commit:

```json
{
  "app": "sha256:…",
  "bundle": "…",
  "darktable_commit": "…",
  "image": "sha256:…"
}
```

Use the extended image's immutable ID as the deployment's `SLIPSTREAM_IMAGE`
and the bundle digest for acceptance identity checks. Pass
`--app-image sha256:…` to extend an already-built immutable application image
instead of rebuilding the runtime, or `--app-tag` to build it under a
different local tag.

## Deploy

Start the normal Compose deployment with the extended image digest; there is
no processing-specific command or overlay:

```sh
SLIPSTREAM_IMAGE=<extended-image-digest> \
  ./scripts/compose --env-file /srv/slipstream/instance.env up -d
```

The container carries the documented finite shared allocation for Web plus
the engine (memory 8 GiB, CPU 4, PID 512, engine OpenMP threads 4); an engine
over-run fails that container instead of the host. The container never
receives the Docker socket. `SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES`
defaults to 8 GiB in `compose.yaml` and may be set in the environment file.

At startup the server verifies the bundle manifest identity and every named
asset digest under `/opt/slipstream-photo`. Missing or invalid assets leave
Library operations available and report Development unavailable. The default
`SLIPSTREAM_PHOTO_DEVELOPMENT=auto` admits the extension exactly when the
installed assets validate; `disabled` turns it off, and an absolute
`SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY` points a smoke or development run at a
bundle root outside the image.

## Film extension

`film/build.py` attaches the standalone SpektraFilm Film stage to the
application image. The extension adds the pinned standalone numerical
runtime — the pinned interpreter under `/opt/python`, the locked
environment under `/opt/runtime`, the patched pinned SpektraFilm source
under `/opt/spektrafilm`, and the qualified fixed-recipe modules under
`/opt/probe` — plus the local runner, the runtime-generated complete
default parameter tree, and the deterministic bundle manifest under
`/opt/slipstream-film`. It never invokes darktable: the film stage
consumes the retained linear ProPhoto Development TIFF handoff directly,
so it is installable independently of the native darktable extension:

```sh
python3 tools/processing/film/build.py --tag slipstream:film-local
```

Like the photo helper it first builds (or reuses via `--app-image`) the
normal application runtime, then builds twice so the second pass pins the
derived bundle digest in the image label, and refuses an image that does
not extend the exact application runtime layers or that does not keep the
Slipstream server entrypoint. It prints the explicit build artifacts —
the application image ID, the extended image ID, the 64-character bundle
digest, and the pinned SpektraFilm commit:

```json
{
  "app": "sha256:…",
  "bundle": "…",
  "image": "sha256:…",
  "spektrafilm_commit": "…"
}
```

The shared fixed-recipe digest covers the numerical parameters, stocks, and
seed. It excludes only processing-bundle provenance and bounded gamut workspace
allocation; both build assets and the recorded processing bundle remain covered
by the exact adapter bundle digest. The runner refuses changed, missing, or
unknown recipe fields before rendering. A build change requires a new configured
bundle digest even when the saved fixed recipe remains identical.

Deploy the extended image with the ordinary Compose command — there is no
film-specific command or overlay — and use its immutable ID as the
deployment's `SLIPSTREAM_IMAGE` and the bundle digest for identity
checks. At startup the server verifies the bundle manifest identity and
every asset it names: the runner, the generated default parameter tree
(admitted by the module boundary's own fixed-recipe shape), the recorded
processing-bundle identity, the locked requirements, the installed
package list, and every file of the runtime, source, and probe trees. The
default `SLIPSTREAM_FILM_MODULE=auto` admits the stage exactly when the
installed assets validate; `disabled` turns it off, and an absolute
`SLIPSTREAM_FILM_BUNDLE_DIRECTORY` points a smoke or development run at a
bundle root outside the image. A configured but absent bundle directory
reports `film-runtime-missing`, and a present bundle that fails
verification reports `film-bundle-unavailable`; either way the Library,
Photo Development, and every other operation stay available and the
capability reports the Film stage unavailable.

Module readiness does not establish full-resolution resource admission. A new
standalone Film Export is refused before acceptance if the known minimum live
memory exceeds the effective finite cgroup allowance, or that allowance cannot
be read. Bounded Preview remains separately executable. The admission and
retained-receipt rules are owned by
[Processing Modules](../../design/processing-modules.md#explicit-materialization).

## Operator end-to-end acceptance

`acceptance.py` drives a deployed instance through the real HTTP surface of
the merged wire contract (`design/photo-development.md`, Service Surface):
capability read, resolution of the Photo for an approved-profile RAW fixture,
Edit Recipe read, a guarded exposure save and its guarded reversal, the
`develop` Edit Preview, a submitted `development-tiff` Export through
terminal settlement, and artifact download with byte-level validation
(digest, byte length, geometry, content type, the embedded float32 linear
ProPhoto RGB framing, pinned source-profile identity, and every Deflate
strip). It hashes the fixture Original and any external XMP sidecar before
and after the run and fails if bytes, size, mode, or modification time
changed; the only files it creates are downloaded artifacts inside the
explicit output directory.

The runner must never target an operator's live library. It refuses to start
without an explicit acknowledgement flag and is meant for a dedicated
acceptance deployment started with the ordinary Compose command above:

```sh
python3 tools/processing/acceptance.py \
  --base-url https://acceptance.example.com \
  --token-file /run/secrets/slipstream-cli-token \
  --fixture /absolute/private/fixtures/approved-camera.raw \
  --output-dir /absolute/private/acceptance-downloads \
  --max-download-bytes 4294967296 \
  --i-acknowledge-this-is-an-acceptance-instance \
  --expected-bundle-sha256 BUNDLE_SHA256
```

`--base-url` must be HTTPS (plain HTTP is accepted only for loopback hosts),
`--token-file` holds the bearer token and must not be group- or
other-writable, and `--max-download-bytes` must carry the deployment's
retained-output allowance (`SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES`) so the
qualified artifact's declared size is admitted; it is capped at the service's
hard output maximum (`MAX_OUTPUT_BYTES`,
`crates/slipstream-processing/src/local_photo.rs`). `--expected-bundle-sha256` is
the bundle digest printed by the image build and is checked against the
capability report's `bundleId`. Exit codes: `0` when every step that ran
passed, `1` when any step failed, and `2` when the run was blocked or the
invocation was refused.

The runner reports honestly what it could not run. The Film
(`finished-jpeg`) service stage remains a separate qualification gate and is
recorded as not covered; a route the deployment does not serve yet makes
dependent steps skipped with `route-not-deployed` instead of failing. The
logic is tested without a deployment by `test_acceptance.py`'s in-process
stub deployment.

## Native reference qualification

`photo/qualify.py` qualifies the built image's engine against an explicit
independent RAW reference. The reference command must independently produce
the requested Development TIFF and an adjacent `<output>.json` document
recording the reference's exact `identity` and initialized `temperature`
fields; the qualifier substitutes the three named placeholders, compares
full-resolution zero and +1 EV output, checks generic parameter updates, and
exercises restart and cancellation. The reference is qualification evidence,
not a production fallback. Its output directory must not exist before the
run:

```sh
python3 tools/processing/photo/qualify.py \
  --engine-image slipstream:photo-local \
  --fixture /absolute/private/fixtures/approved-camera.raw \
  --reference-cli '/absolute/path/reference --input {input} --output {output} --exposure-milli-ev {exposure_milli_ev}' \
  --icc tools/processing/photo/icc/LargeRGB-elle-V2-g10.icc \
  --output /absolute/private/native-qualification
```

The qualifier runs the image's `/opt/darktable/bin/darktable-mcp` entry
directly in a locked-down container (no network, read-only root, dropped
capabilities), so it exercises the same engine bytes the application will
run.

## Qualification-only toolkits

[`../development/`](../development/README.md) holds the engine measurement
probes (bounded gamut, buffer lifetimes, LUT quality, history) used while
qualifying native changes; it is independent of the application image.

`film/` is the standalone Film extension build kit described above, not a
qualification tool: it packages the already-qualified pinned runtime into
the application image.

The former host-launcher qualification and deployment-verification tools
(`verify.py`, `verify-film.py`, `verify-qualified-film.py`,
`verify-deployment.py`) were removed with the launcher path they exercised;
the single-container boundary they probed (per-attempt cgroups, launcher
journals, sockets) no longer exists.

## Focused coverage

Run the tooling tests without root or Docker:

```sh
python3 -m unittest discover -s tools/processing -p 'test_*.py' -v
```

Engine-side executor behavior is covered by the Rust suite:

```sh
cargo test --locked -p slipstream-processing
cargo clippy --locked -p slipstream-processing --all-targets -- -D warnings
```

Native worker qualification is separate. From the native checkout configured
with `BUILD_TESTING=ON`, run:

```sh
cmake --build build --target darktable-mcp test_mcp_paths test_mcp_params
ctest --test-dir build --output-on-failure -R mcp
```
