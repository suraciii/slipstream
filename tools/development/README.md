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

- `smoke`: real RAW decoding, float TIFF/ICC inspection, zero and nonzero EV
  history, custom raw white-balance coefficients, one-path/manual-exposure
  history checks, database reload, small-image repeated full simulation, A/B/A
  state checks, and the active grain microstructure branch.
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

The database round trip reuses the imported generated history. The custom
white-balance case verifies one enabled darktable temperature module with fixed
channel coefficients and a manual exposure module with camera-bias compensation
off. It does not verify a temperature/tint-to-camera mapping or serve as an
independently authored darktable reference; those remain open in #329.

Grain and print glare retain their effects. The harness uses one Numba thread
and resets Numba's own RNG inside a compiled function before every simulation.
It does not use Spektrafilm's `preview_mode`, which disables grain. The engine's
internal enlarger and scanner LUTs are fixed at resolution 33.

The engine-checks image also runs `lut_quality_probe.py` over a fixed neutral,
structured-range, and textured-range corpus. It renders the same full-effect
recipe with both internal LUTs enabled and both LUTs disabled for direct
spectral evaluation, verifies both paths' repeatability, and reports
pointwise RGB and D65 CIEDE2000 difference metrics. The runner gives it a fresh
private empty Numba cache and the report records that cache state plus the
adapter and qualification recipe identities. An identity mismatch is a blocker,
not accepted evidence. The probe does not declare acceptance: #338 must choose
and independently justify the visual criterion before these measurements can
qualify the LUT optimization.

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

It then applies `patches/0002-buffer-lifetimes-and-output.patch` with zero fuzz.
This patch releases expired taps along the existing ordered topology, including
cyclic frames left by cold numerical compilation. Collection happens at stage
handoffs and after retaining the final result; it preserves caller and view
ownership and does not change global GC settings. `memory.reclaim` reports this
time separately from node computation. Whole-run elapsed time includes both.
The patch also removes the redundant preprocessing, image-loading, and unused
density allocations and batches display encoding and JPEG conversion.

`/opt/processing-bundle.json` records the source archive and commit, numerical
dependency versions and lock digest, native package inventory, complete profile
asset tree digest, patch digest, and modified source/adapter digests. Each probe
emits this identity and saves it with the complete Film Recipe. The inspected
container image ID remains the identity of the complete tested filesystem.
The manifest records patches in application order and includes the additional
`bounded_output.py` engine module.
The Film recipe, ICC, and procedure identities consumed by adapter grant
validation are defined once in `film_identity.py`. The qualification image and
Film adapter image carry that same module; the numerical bundle digest remains
owned by `bundle.py` and `package.py`.

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

## Display encoding, JPEG, and observers

`srgb-cctf-f64-v1` uses the same nonempty native float64 contiguous RGB layout
and owns a separate 24-byte-per-pixel destination. Its numerical scratch model
is 4 MiB plus 256 bytes per batch pixel, capped at 262,144 pixels. It calls the
original same-profile `colour.RGB_to_RGB` operation in each batch, retaining
the matrix operation and transfer-function arithmetic.

`jpeg-uint8-rows-v1` accepts native float32 or float64 RGB, including strided
input. Its scratch model is 1 MiB plus 64 bytes per batch pixel, capped at
262,144 pixels. The default numeric allowance is 17 MiB. The model must admit
one full-width row before a file is opened. Conversion retains the original
clip, multiply, and uint8 truncation sequence; OIIO receives consecutive
scanline batches with the original ICC and encoder settings. Failed open,
write, and close operations propagate. This numerical model excludes native
encoder storage, which still requires whole-stage measurement and admission.

The OIIO float32 input now retains the storage returned by `read_image` instead
of copying it. The pinned v3.1.17.0 binding [allocates an independent buffer](https://github.com/AcademySoftwareFoundation/OpenImageIO/blob/v3.1.17.0/src/python/py_imageinput.cpp#L26)
and [attaches its deleter to the ndarray through a capsule](https://github.com/AcademySoftwareFoundation/OpenImageIO/blob/v3.1.17.0/src/python/py_oiio.h#L504).
Closing the image reader does not release that buffer. Regression checks use
the returned pixels after closing, unlinking the file, and collecting garbage.

The harness now checks finite samples and hashes exact C-order bytes in bounded
chunks of at most 786,432 samples, including strided arrays. It no longer
creates a whole-image Boolean mask or `tobytes` copy. Peak comparisons must
disclose this observer change separately from engine buffer savings. The
optional checks cover cold and warm stage ownership, views, injection and
collection, callback failures, exact CCTF and JPEG arithmetic, actual encoded
bytes and ICC, IO failures, and fresh-process numeric scratch allocations.

Complete-pipeline references run in separate fresh processes with an explicitly
empty JIT cache, then with the compiled cache from that first process. The
pinned upstream engine has different pre-encoding float64 results between
these cache contexts; each test uses the exact reference for its own context.
JPEG identity is also checked. A cache policy is part of qualification, and
these checks do not authorize cross-attempt cache reuse in production.
