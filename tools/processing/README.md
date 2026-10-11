# Processing tooling

This directory builds and exercises optional processing modules of the single
Slipstream application container: native darktable and standalone SpektraFilm. The
[local executor design](../../specs/photo-development/processing/execution.md) owns the
execution and failure lifetime; the
[darktable integration](../../specs/photo-development/darktable/design.md) and
[development color](../../specs/photo-development/processing/color.md) designs own the
semantic requests and output contracts.

The application image supervises the installed independent `darktable-edit`
executable. The same executable also runs on accessible arbitrary local files,
without HTTP, Photo identity, service state, or a desktop darktable catalog.

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

The native build passes `BUILD_GUI=OFF` and installs no desktop executable.
`/opt/darktable-edit` contains a dereferenced native tree, every ELF runtime
dependency, package inventory, source revision and exact native `edit_discover`
contract. `edit-manifest.json` pins every installed regular file by SHA-256;
`bundle` pins the exact manifest bytes. Discovery and rendering use the installed
deterministic `LargeRGB-elle-V2-g10.icc` and `srgb-iec61966-2-1.icc` assets in
`native/share/darktable/color/out`; discovery advertises their exact embedded ICC
hashes. The retained `/opt/slipstream-photo` bundle supports historical/non-darktable
adapters. When preparing an equivalent local native installation, copy the same
repository assets before discovery and manifest generation:

```sh
install -d "$DARKTABLE_EDIT_BUNDLE/native/share/darktable/color/out"
install -m 0644 tools/processing/photo/icc/LargeRGB-elle-V2-g10.icc "$DARKTABLE_EDIT_BUNDLE/native/share/darktable/color/out/"
install -m 0644 crates/slipstream-core/assets/srgb-iec61966-2-1.icc "$DARKTABLE_EDIT_BUNDLE/native/share/darktable/color/out/"
```

For an installation whose native data directory differs, use that actual
`share/darktable/color/out` directory. Generate the manifest only after all assets
and runtime libraries are installed; later asset changes invalidate its identity.

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

The standalone engine verifies `/opt/darktable-edit/edit-manifest.json` and all
named asset digests before execution. `DARKTABLE_EDIT_BUNDLE` selects that root.
Unavailable native assets leave retained editing metadata readable. Workspace
inputs and completed files survive startup and have no receipt expiry; edits
use explicit local or configured user catalogs with no cross-scope fallback.

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

`acceptance.py` now invokes the workspace/independent-tool acceptance helper.
Run it on the workspace execution host with the matching installed binaries,
a dedicated service and approved RAW fixture. Configure the usual Slipstream
origin/token environment first:

```sh
python3 tools/processing/acceptance.py \
  --slipstream /absolute/path/slipstream \
  --tool /absolute/path/run-darktable-edit \
  --photo PHOTO_ID \
  --fixture /absolute/private/fixtures/approved-camera.raw \
  --outside-directory /absolute/private/new-independent-acceptance \
  --i-acknowledge-this-is-an-acceptance-instance
```

It opens the workspace once, initializes independent state in `.edits/darktable`,
saves a discovered exposure instance, exports before any preview, inspects the
complete scene-linear float32 ProPhoto TIFF for the Film input contract, renders
a PNG, reopens state, and reconciles the completed output through workspace files.
It also opens and exports an arbitrary local fixture outside service state.
A missing command, inaccessible server path, unsuitable saved output contract,
invalid bytes or missing reconciled output fails. Historical adapter fault/byte
fixtures remain available for retained data. No unrun helper establishes native
qualification; reports cover the fixture, exact installed bundle and exercised
commands only. SpektraFilm processing still needs separately admitted resources.

## Consistent backup

Stop every Slipstream service process using the state root before copying its
SQLite databases. The backup helper obtains an exclusive Linux directory flock
on each captured root; tool operations hold shared ancestor leases for their
lifetime, so the helper refuses an active caller and excludes new CLI writers
throughout the copy. Copying uses held directory descriptors, refuses symlinks
and captures all workspace/state files, including SQLite WAL and dependencies.

```sh
python3 tools/processing/backup.py --state /srv/slipstream/state \
  --include /private/configured-darktable-catalog \
  --file /private/backups/slipstream-state.tar --service-stopped
```

Include every workspace or configured global catalog outside the state root;
do not include a nested root twice. The output must be new and outside every
captured root. Keep the immutable tool/image bundles and Originals backup
separately. Originals are read-only and not copied by this helper. Filesystem
snapshots are an alternative only when all state/workspace/catalog volumes share
one consistent snapshot boundary with the service database quiesced.

Restore into a new isolated directory. The helper verifies complete path/digest/
length coverage before copying, then verifies restored bytes before invoking the
server's offline observation rebase. That explicit operation retains artifact and
edit identities while rebinding verified filesystem observations and old paths;
ordinary startup never treats replacement inodes as unchanged managed files.

```sh
python3 tools/processing/restore.py --file /private/backups/slipstream-state.tar \
  --directory /private/new-restore --previous-state /srv/slipstream/state \
  --server /absolute/path/slipstream-server
```

The result discloses the restored state root. Start an isolated matching service
only after successful rebasing. A failed restore keeps its private evidence for
inspection and never overwrites live state. External retained dependencies must
be captured in included roots, or remain independently available and verified.

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
