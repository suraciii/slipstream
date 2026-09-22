# Headless Development Qualification

This opt-in harness exercises real darktable and Spektrafilm processes for
[#329](https://github.com/suraciii/slipstream/issues/329). It does not enable
editing in the service. Measurements, camera acceptance, open failures, and the
go/no-go decision belong in that Issue. The processing contract is
[Development Color Pipeline](../../design/development-color.md).

Build from this directory only. The build context admits named engine, patch,
and check files through an allowlist. It cannot include an Original or a previous
probe result:

```sh
docker build --platform linux/amd64 -t slipstream:development-qualification tools/development
```

Python dependencies, Python, source archive, base image, and darktable package
are pinned. Ubuntu's transitive package resolution can change; the exact built
image ID and its `/opt/os-packages.txt` identify the tested native environment.
Do not transfer qualification between different image IDs without checking it.
The image retains upstream source and license notices. It installs numerical
dependencies without the GUI application dependencies.

Both writers use the pinned linear ProPhoto ICC asset from that source archive.
The probe checks exact input and output hashes, primaries, white point and linear
TRCs. darktable's LittleCMS reader normalizes the legacy description tag, so its
serialized output has a separate pinned hash. Its color tags remain unchanged.
The probe does not use darktable's generated profile, whose header changes
between invocations.

Run with an explicit local RAW fixture and a **new directory outside the
repository**. No real photograph or generated artifact may be committed.

```sh
python3 tools/development/run.py \
  --raw /absolute/path/to/sample.ARW \
  --output /tmp/slipstream-development-smoke \
  --mode smoke --repetitions 2
```

The runner copies and hashes the fixture, checks its stability, mounts only
that copy read-only, and verifies the Original and adjacent external XMP again
after processing. It runs the exact inspected image ID as the invoking user,
without runtime networking, GPU devices, display variables, capabilities, or a
writable container filesystem. Private work and cache directories are writable.
The container is limited to four CPUs, 32 GiB without swap, and 256 tasks. These
are probe limits, not supported production defaults.

The modes are:

- `smoke`: real RAW decoding, float TIFF/ICC inspection, generated exposure
  history checked after an engine database round trip, small-image repeated full
  simulation, A/B/A state checks, and the active grain microstructure branch.
- `benchmark`: the same RAW checks plus approximately 1 MP and 2 MP full-effect
  simulations. The default is one first call and 20 warm calls per geometry
  in one reused simulator. This does not measure a fresh process per geometry.
- `full`: adds full-resolution Development TIFF and full-resolution Film output.
  Large camera files can consume the full memory allowance. An OOM or nonzero
  child exit remains a failure in `report.json`.

Use `--stage raw --mode full` to qualify full-size RAW/TIFF independently of
Film processing. The default `--stage pipeline` runs both stages. An interrupted
runner stops its owned container and preserves the terminal state before removal.

`probe.jsonl`, the exact Film Recipe, private engine logs, and artifacts stay in
the output directory. The report intentionally says qualification is incomplete:
a successful probe is evidence for its checks, not acceptance of every condition
in #329. In particular, resized TIFF does not establish full-resolution negative
sample preservation, and a repeated in-process result does not establish
fresh-process or cross-hardware reproducibility.

The database round trip reuses the imported generated history. It is not an
independently authored reference for nonzero exposure or camera white balance.

Grain and print glare retain their effects. The harness uses one Numba thread
and resets Numba's own RNG inside a compiled function before every simulation.
It does not use Spektrafilm's `preview_mode`, which disables grain. The engine's
internal enlarger and scanner LUTs are fixed at resolution 33; their difference
from direct spectral evaluation still needs separate image-quality acceptance.

These expensive probes are separate from `bun run verify`, like the existing
real-camera safety gate. Run that full repository gate before handing off a
harness change as well.

The host-side failure and signal-settlement checks use a controlled Docker
stand-in and need no engine image:

```sh
python3 -m unittest discover -s tools/development -p 'test_*.py' -v
```

## Bounded output-gamut bundle

The build applies `patches/0001-bounded-output-gamut.patch` with zero fuzz to the
pinned upstream archive. It adds an explicit transform workspace argument to the
simulator and scanner, and installs `bounded_gamut.py` as an engine module.
There is no runtime monkey patch. `film.make_simulator` always requests bounded
processing; the unchanged upstream call without this argument is retained only
as the numerical reference and is not a fallback from a rejected bounded call.
Parameter updates preserve the captured workspace allowance.

`/opt/processing-bundle.json` records the source archive and commit, numerical
dependency versions and lock digest, native package inventory, complete profile
asset tree digest, patch digest, and modified source/adapter digests. Each probe
emits this identity and saves it with the complete Film Recipe. The inspected
container image ID remains the identity of the complete tested filesystem.

The local `cam16ucs-srgb-f64-v1` workspace model admits only the fixed CAM16-UCS
compression recipe, sRGB output, and nonempty C-contiguous native float64 RGB
arrays. It rejects strided arrays rather than silently copying them. The caller
reserves 24 bytes per pixel for one independent destination, in addition to its
input and other stage buffers. Scratch uses a conservative 64 MiB fixed allowance
for cold color-table construction plus 2,048 bytes per batch pixel, with batches
from 1 through 262,144 pixels. The qualification harness grants the maximum
603,979,776-byte scratch allowance. Smaller valid grants select a smaller batch;
an allowance below 67,110,912 bytes fails before destination allocation.

These coefficients describe this pinned transform, not total process memory or
a production admission model. Engine/runtime headroom, live pipeline buffers,
file cache, and encoding still need separate accounting under
[Processing Memory](../../design/processing-memory.md). A different bundle or
transform mode needs its own qualification; a successful small-image check does
not establish a supported camera or deployment envelope.

The optional test image adds hash-locked test tools outside the runtime
installation. It verifies bundle integrity, exact pointwise and complete seeded
pipeline equivalence, layout and budget failures, fresh-process cold scratch
measurements, and applicable upstream gamut/runtime/topology tests. No camera
fixture is used. The test image applies a separate test-only patch replacing
an upstream assertion on the removed `mult_usm_amount` field with a behavioral
check that nonzero microstructure cannot run when grain is inactive. All
upstream tests still execute; no failure is excluded or marked expected. The
check log records that test patch's digest:

```sh
docker build --platform linux/amd64 --target engine-checks \
  -t slipstream:development-engine-checks tools/development
mkdir -p /tmp/slipstream-development-engine-checks
docker run --rm --user "$(id -u):$(id -g)" --network none \
  --cpus 4 --memory 4g --memory-swap 4g --pids-limit 256 \
  --cap-drop ALL --security-opt no-new-privileges --read-only \
  --tmpfs /tmp:rw,size=512m \
  --mount type=bind,src=/tmp/slipstream-development-engine-checks,dst=/work \
  slipstream:development-engine-checks
```

Tracked numerical allocations demonstrate the transform model. They are
separate from whole-attempt cgroup peaks and OOM evidence. Full developed-size
camera completion must still run under the same hard limit as its baseline and
verify geometry, ICC, output identity, and enabled effects. Where the unbounded
full-size reference fails, small-image exactness and full-size completion remain
two distinct claims.
