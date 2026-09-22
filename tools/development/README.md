# Headless Development Qualification

This opt-in harness exercises real darktable and Spektrafilm processes for
[#329](https://github.com/suraciii/slipstream/issues/329). It does not enable
editing in the service. Measurements, camera acceptance, open failures, and the
go/no-go decision belong in that Issue. The processing contract is
[Development Color Pipeline](../../design/development-color.md).

Build from this directory only. The build context admits four named files and
cannot include an Original or a previous probe result:

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
