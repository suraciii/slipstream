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

`film/build.py` attaches the standalone `spektrafilm-rs` fork to the
application image. The extension adds the pinned `spektrafilm-f64` CLI, its
profile-data tree, the module-owned default parameter tree, and the
deterministic bundle manifest under `/opt/slipstream-film`. It never invokes
darktable: the Film stage consumes the retained linear ProPhoto Development
TIFF handoff directly, so it is installable independently of the native
darktable extension:

```sh
python3 tools/processing/film/build.py --tag slipstream:film-local
```

Like the photo helper it first builds (or reuses via `--app-image`) the normal
application runtime, then builds twice so the second pass pins the derived
bundle digest in the image label. It prints the application image ID, extended
image ID, bundle digest, and fork commit:

```json
{
  "app": "sha256:…",
  "bundle": "…",
  "image": "sha256:…",
  "spektrafilm_fork_commit": "…"
}
```

The fork CLI owns the machine-readable local contract:

```sh
spektrafilm-f64 describe --format json
spektrafilm-f64 render --input INPUT.tiff --recipe RECIPE.json --output OUTPUT.jpg
spektrafilm-f64 inspect --input OUTPUT.jpg --format json
spektrafilm-f64 parity --corpus CORPUS.json --report REPORT.json
```

`render` refuses an existing output, uses the explicit recipe schema and
profile data root, and writes through a private temporary file before commit.
`inspect` is read-only. `parity` records each input/recipe/output identity and
only reports pass when a declared reference digest matches; missing references
remain explicitly unqualified.

At startup the server verifies the format-2 manifest identity, executable fork
binary, default parameter tree, native runtime libraries, and every profile
data digest. Missing or invalid assets leave Library operations available and
report the Film stage unavailable. `SLIPSTREAM_FILM_MODULE=auto` admits the
stage only when the installed assets validate; `disabled` turns it off, and an
absolute `SLIPSTREAM_FILM_BUNDLE_DIRECTORY` points a smoke or development run
at a bundle root outside the image. A configured but absent directory reports
`film-runtime-missing`; a present bundle that fails verification reports
`film-bundle-unavailable`.

Module readiness does not establish full-resolution resource admission. A new
Film Export is refused before acceptance if the known minimum live memory
exceeds the effective finite cgroup allowance, or that allowance cannot be
read. Preview remains separately bounded. Historical
`spektrafilm-params-1` trees remain readable; discovery and new fork recipes
use `spektrafilm-rs-params-1`.


## Operator end-to-end acceptance

`acceptance.py` drives a dedicated deployment through the selected-step HTTP
surface: `GET /api/processing/modules`, approved RAW Photo resolution,
`GET/POST /api/photos/{id}/processing-recipe`, a guarded +1 EV save and exact
intent reversal, explicit selected darktable Export via
`POST /api/photos/{id}/processing-exports` (202), durable status reconciliation,
and `GET /api/processing-artifacts/{id}` plus `/bytes`. It reopens the saved
recipe and checks the bounded selected-step PNG from
`GET /api/photos/{id}/processing-preview/{step_id}`. Every required missing
route fails acceptance; dependent steps cannot turn that failure into success.

The TIFF inspection retains digest, byte length, full geometry, float32 linear
ProPhoto RGB framing, pinned embedded ICC identity, every inflated Deflate
strip, and bounded trailing padding checks. Captured Photo, input byte evidence,
step, module, complete parameters, adapter version and bundle must match across
the receipt, status, immutable artifact and download headers. Original and
external XMP bytes, size, mode and modification time must remain unchanged.

Run only with an explicitly approved fixture on a dedicated acceptance instance:

```sh
python3 tools/processing/acceptance.py \
  --base-url https://acceptance.example.com \
  --token-file /run/secrets/slipstream-cli-token \
  --fixture /absolute/private/fixtures/approved-sony.ARW \
  --output-dir /absolute/private/acceptance-downloads \
  --max-download-bytes 4294967296 \
  --request-timeout 30 \
  --settlement-timeout 900 \
  --preview-timeout 180 \
  --poll-interval 2 \
  --i-acknowledge-this-is-an-acceptance-instance \
  --expected-bundle-sha256 BUNDLE_SHA256
```

The bearer comes only from a bounded regular token file that is not group- or
other-writable. Redirects are refused. Prefer HTTPS; HTTP prints an explicit
unencrypted-connection warning. All timeout arguments must be finite, positive
and at most 3600 seconds. The output directory must be private (no group/other
permissions), cannot be a symlink or contain the fixture, and downloads create
exclusive mode-0600 files. `--max-download-bytes` admits the deployment's retained
output allowance up to the service's 4 GiB single-output maximum. The expected
bundle is compared with the captured Export bundle, since module discovery does
not expose bundle identity. Exit codes are 0 for complete success, 1 for failure,
and 2 for refused invocation or a blocked run.

Qualification is finite: the approved fixture, pinned `darktable-adapter-1` /
`darktable-params-1` identity, discovered exact manual-exposure baseline tree,
0/+1 EV controls, source-preserving float32 linear ProPhoto TIFF, bounded Preview,
and admitted deployment resources. It does not qualify arbitrary RAW cameras,
native controls, stack combinations or future schema versions. Existing selected
intent outside this qualified tree is refused without substitution. Reversal
preserves the exact prior intent; processing then explicitly restores +1 EV.

Standalone SpektraFilm is an independent optional qualification and this runner
reports it not covered. Run a separate Film qualification only when its concrete
immutable TIFF input, fixed recipe, engine bundle and finite resources have been
explicitly admitted; darktable success does not widen that claim. Focused local
HTTP fault fixtures and byte inspection tests establish runner safety, never
real engine qualification. The prior real RAW/module/Preview/Export/restart/XMP
evidence remains the baseline; migrating this tooling does not require rerunning
that whole acceptance.

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
