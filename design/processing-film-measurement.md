# Film Resource Measurement

A native allocation fixture proves containment and recovery, but does not account
for Film image arrays, cold compilation, decoding, encoding, or charged image
storage. Measuring those costs requires the actual image engine inside the same
launcher boundary. An unrestricted diagnostic command or a mutable input mount
would bypass the property being measured.

This specification defines a closed, administrator-only Film measurement
profile. It runs one registered fixture, compares complete output with an
independently established reference, and returns bounded evidence. It does not
produce an Export or establish a supported Film memory or camera envelope.
[Processing Memory](processing-memory.md) owns admission and qualification;
[Processing Executor](processing-executor.md) owns host authority, containment,
identity, recovery, and cleanup. The separate
[native qualification protocol](processing-executor-protocol.md) retains its
version-1 configuration, fixtures, and behavior.

## Model and Authority

A **measurement fixture** is an operator-registered synthetic image or private
pre-staged Development TIFF, its full geometry, and its exact reference result.
An opaque fixture ID selects this immutable record. It is not a Photo ID, Original
Location, path, byte upload, script, or arbitrary engine parameter set.

A **reference result** binds the float32 input pixel digest, float64 Film pixel
digest after bundled gamut/CCTF processing and before JPEG conversion, complete
JPEG digest and length, dimensions, and output ICC. Its evidence
must independently establish complete, valid JPEG output under the captured
numerical bundle and fresh-cache procedure. A hash produced by the measurement
being judged cannot establish its own reference. Every accepted catalogue entry,
including a synthetic entry, requires this reference.

A **resource model** supplies reviewed byte bounds or explicit unknown terms for
those finite fixtures. It selects no arithmetic or algorithm. The bundle's Rust
planner owns checked formulas; the engine validates the resulting local plans.
A successful measurement does not turn unknown terms into qualified bounds.

The launcher serves this profile only with `peer_uid: 0`. Its capability is
`film-measurement-only`, never ordinary Film admission. The Web cannot select the
profile or exercise its experimental authority. Neither public IPC nor the
catalogue supplies commands, paths, arguments, environment, images, mounts,
budget overrides, spatial tiling, or precision changes. The
[Development Color Pipeline](development-color.md#film-simulation) still owns
Film mathematics, profiles, physical scale, effects, and stochastic behavior.

## Syntax Authority

[The JSON Schema](schemas/processing-film-measurement.schema.json) is the sole
syntax authority. Its named definitions cover configuration, catalogue, resource
model, all four request operations, responses, receipts, engine grants, stage
exchange, producer observations, and native worker results. Objects are closed and all listed fields are
required; nullable fields are explicit. No additional compatibility syntax exists.

The parser must also reject duplicate keys, trailing JSON, invalid UTF-8,
non-finite numbers, unpaired UTF-16 surrogates, and integer tokens written as
fractions or exponents. Surrogates are invalid in every key and value, including
configuration paths. Schema
validation sees parsed values and cannot establish those lexical properties.
Unsigned counters and byte arithmetic use checked 64-bit integers; Booleans and
negative values cannot substitute for integers. Cross-record identity, uniqueness,
path, sum, and state checks below are semantic validation after syntax validation.

This profile uses protocol and configuration version 2. Catalogue/model documents
have their own version 1. Reject a v1 request at a Film instance and a v2 request
at a native-fixture instance; this does not upgrade or reinterpret v1 receipts.
Profile selection is fixed for the instance lifetime; use a fresh instance to
change between native qualification and Film measurement.
Use the existing launcher binary surface and
[bounded IPC transport](processing-executor-protocol.md#ipc), including peer
credentials, frame limits, deadlines, connection cap, and rejection of all public
ancillary descriptors. Responses use the same result/error envelope at version 2.

## Operator Configuration and Immutable Documents

The host configuration is at most 16 KiB. The schema fixes the Film profile to
4 CPUs, 256 tasks, zero swap, one 4 GiB tmpfs with 4096 inodes, the existing
10-second initial placement deadline, and a 900-second absolute execution
deadline. Execution includes staging, rendering, and result validation. Deadline
expiry starts termination; uncertain cleanup can retain ownership beyond it.
Every individual transport and manager operation remains bounded by the executor
contract. No deadline forces release of an unsettled slot.

`memory_bytes` is exactly one of 8, 12, 16, 24, or 32 GiB. These are operator
experiment settings, not accepted production minimums or recommendations. An
operator policy change drains or cancels existing work and changes the captured
policy identity. No attempt retries automatically or increases its limit.

The existing executor rules govern immutable image resolution, directory and
socket authority, retention, ancestor limits, control-service placement, instance
claims, exact aggregate identity, and quarantine. The Film profile adds no
exception to those rules, including when it expects an OOM.

Two fixed root-owned files reside beneath the sealed instance root:

- `catalogue.json`, at most 32 KiB, with 1–16 unique fixture IDs;
- `resource-model.json`, at most 128 KiB, with exactly one case per fixture.

Read each with its bound, reject links or writable aliases, verify the configured
SHA-256 over exact file bytes, parse, and retain the captured value. These bounded
metadata reads are control work. Configuration, catalogue, and model changes take
effect only through a drained restart; an attempt never rereads a mutable model.
The model must bind the captured catalogue digest and numerical bundle. Case IDs
must equal the catalogue IDs, with no duplicate, missing, or extra case.

The catalogue binds one recipe, input/output ICC identity, numerical bundle,
reference image, and `film-once-empty-cache-v1` procedure. These must match the
packaged adapter. The executable bundle additionally identifies the launcher
worker and adapter; changing either invalidates its prior launcher qualification.
No implicit image pull occurs during an attempt.

## Input Variants and Geometry

A Development TIFF entry supplies only file length and content SHA-256. The
launcher derives `fixtures/<fixture_id>.tif` beneath the private operator root.
The registered files are pre-staged qualification inputs, disjoint from Library,
state, and cache. They are never Originals and are never resolved through a Photo
or Library Location. The engine receives no source-directory mount.

Acquire the source using a retained directory descriptor and confined `openat2`
resolution with `BENEATH`, `NO_SYMLINKS`, `NO_MAGICLINKS`, and `NO_XDEV`. Require a
root-owned regular file with one link, no group/other write, and exact positive
length at most 2 GiB. Retain that read-only descriptor. Capture its device, inode,
size, modification time, and change time before copying and compare them after
copying. A source path must not be re-resolved for a Docker bind.

The TIFF must satisfy the
[Development Result contract](development-color.md#development-result): IEEE
float32 RGB, full already-oriented dimensions, finite samples, and the captured
linear ProPhoto ICC. Negative and over-range values are allowed. Inspect metadata
under the attempt limit before allocating decoded pixels. Require exactly the
catalogue geometry, channels, sample type, and ICC, then check actual decoded
shape/dtype again; decoder metadata cannot enlarge the allocation authority.
Hash decoded pixels in bounded C-order traversal against the reference.

A synthetic entry uses `linear-rgb-f32-v1` and a closed pattern: dark, bright,
red, green, blue, gradient, or noise. Its generator runs inside the attempt and
owns one C-contiguous native float32 RGB destination. The bundled NumPy version
and the following operation order define its samples:

- Dark is `(0, 0, 0)`; bright is `(4, 4, 4)`; each primary has one channel at 4
  and the others at zero. Their seed must be zero.
- Gradient assigns all three channels at pixel index `i` the float32 conversion
  of the float64 value `4 * i / max(N - 1, 1)`, with `i` in C-order. Its seed is
  zero. Integer-to-float conversion, multiplication, division, then float32 cast
  occur in that order, without fused operations.
- Noise creates `Generator(PCG64(seed))` and fills the flat float32 destination
  using `random(dtype=float32, out=...)` in consecutive 16384-sample blocks,
  followed by the short tail. It then multiplies each block by float32 4 in place.
  No other draw or random generator shares this state.

The bundle owns these exact algorithms and its separate Film random-state policy.
The generator uses at most 16384 temporary float64 samples; it must not flatten a
strided image by making another frame. Its generated C-order digest must match
the independently established entry reference before simulation. No synthetic
source file or unbounded manifest is created outside the attempt.

For both variants, width and height are integers from 1 through 9568. Check
`N = width * height`, `12N`, `24N`, row bytes, and local batch arithmetic before
starting an attempt; decoded float32 input must not exceed 2 GiB. These are finite
measurement guards, not a claim that every rectangle in that range is qualified.
Only the exact registered geometry with an independently established reference
can run. Unsupported geometry fails `unsupported-fixture` before reservation,
without resizing or dropping effects.

The compiled recipe guard requires camera film format 35 mm, crop disabled,
upscale factor 1, preview mode disabled, camera/enlarger diffusion filters
inactive, both spectral LUTs enabled at resolution 33, and active sublayer grain
with microstructure `(0.2, 30)`. Require the complete recipe digest as well; these
checks do not replace it. Let `L = max(width, height)`. The pinned physical scale
is `35000 / L` micrometers per pixel. Its microstructure sigma is therefore
`0.03 * L / 35000`; the inactive-branch predicate is checked as `3 * L <= 175000`.
The 9568 edge cap keeps sigma below 0.008202, safely below the upstream activation
condition `sigma > 0.05`. Inactive diffusion filters exclude their FFT path.

Both existing Gaussian routes are eligible: FIR for sigma below 3, IIR for sigma
at least 3, in the pinned operation order. The compiled array inventory reserves
the union needed by either route, including width-dependent FIR strips and IIR
state; an unproven term stays unknown. Exact per-fixture references establish
numerical behavior for the actual route, not a whole shape family. The
[fixed recipe adapter](../tools/development/film.py) and its pinned resize, grain,
and Gaussian sources establish these guards; a recipe or source change requires
reviewing them rather than inheriting this predicate. Native and allocator terms
can remain unqualified while this closed measurement profile gathers evidence.

## Staging and Sealing

The [executor's input ownership contract](processing-executor.md#restricted-launch-authority)
is authoritative. This profile uses one capped tmpfs with a root-owned,
non-writable root and input directory, a UID-1000 work directory, and a separate
UID-1000 output directory. A host-only native-result directory is root-owned and outside every container
mount; its preallocated result inode is on the same capped tmpfs. The container
sees `/input` read-only from creation,
`/work` and `/output` writable, and `/tmp` plus `/dev/shm` as aliases of work only.
All writable storage shares the actual aggregate byte/inode cap. An engine cannot
chmod, unlink, or reopen the input through a writable ancestor or alias.

For TIFF, the launcher creates the exclusive `/input/input.tif` destination inode,
opens its writer,
then makes it root-owned 0444. Record its device/inode and tmpfs mount identity.
Only empty-file and bounded metadata creation happens outside the attempt.
The launcher also writes the canonical `EngineGrant` bytes, at most 16 KiB, to
`/input/grant.json` before container creation. Its inode is root-owned 0444 beneath
the sealed input directory; close its writer before binding `/input` read-only.
`grant_sha256` hashes those exact bytes. Native bootstrap and Python each read
this one fixed file with a 16 KiB bound, verify that digest and strict schema, and
check launch/manifest/plan identity before use. The launcher captures its inode
and mount with the other sealed metadata; the engine never receives an alternate
path, caller-supplied grant, or writable grant alias.
The separate snapshot writer can populate `input.tif` despite the read-only
container mount; only the verified native bootstrap receives that capability.

Use an attempt-private `SOCK_SEQPACKET` stage channel beneath the owned control
directory. It is separate from public IPC. Authenticate the exact bootstrap UID,
host PID/pidfd, and verified workload-leaf membership. Messages are at most 16 KiB.
Packet send/receive is nonblocking with a two-second operation bound; truncation,
partial send, or invalid framing fails the exchange. The initial bootstrap
connection/placement must meet its ten-second deadline. Waiting for a staged or
engine-start acknowledgement is a separate supervised phase wait bounded by the
captured 900-second absolute execution deadline, not the public IPC request's
two-second deadline. Polls remain interruptible for cancellation, container exit,
and the absolute deadline; no successful packet resets that deadline. An elapsed
phase wait triggers settlement and never permits an unobserved engine start.
A `StageOffer` is the first permit. It accompanies exactly three `SCM_RIGHTS`
descriptors for TIFF, ordered as source read-only, snapshot writer, and native
result writer. Synthetic offers carry only the result writer, zero source bytes,
and null source hash/destination identity fields. The result inode is a root-owned
0444 regular file under the host-only directory, identified by the offer's device
and inode. The native bootstrap checks rights, access modes, identity, truncation,
and exact roles. Receive with `MSG_CMSG_CLOEXEC`; all capabilities remain
close-on-exec. Native PID 1 becomes non-dumpable before receiving capabilities;
the engine has no ptrace capability. A same-UID child must be unable to obtain the
writer through `/proc`, `pidfd_getfd`, or ptrace. Host-side audits still require
the launcher's qualified observation authority. Reject extra or out-of-order packets/rights and close every received
descriptor on failure. Only the four schema-defined message kinds are allowed: an
offer, its acknowledgement, the engine permit, and engine-start acknowledgement.

The launcher closes all writer duplicates immediately after a successful send.
It retains a read-only descriptor for the exact native result inode.
Inside the verified attempt, the bootstrap copies and hashes in at most 64 KiB
chunks, checks exact length, EOF, metadata stability, and captured digest, then
closes the source descriptor and all snapshot writers before its `StageAck`.
The distinct native result writer is intentionally retained. The acknowledgement
has the same launch, grant, source, and destination facts as the offer. A synthetic
acknowledgement confirms only its bounded immutable manifest, not generated pixels.
The bootstrap can retain a read-only snapshot descriptor and the distinct native
result writer; it stays at the second gate and must not import Python or spawn
children. `StageAck` repeats the offer's source/snapshot/grant facts; it grants no
new rights.

Pause the exact owned container through the executor's durable manager protocol.
Revalidate held process identity, sole-process membership, snapshot inode/mount,
root ownership/mode/length, descriptor access flags, and absence of a writable
shared mapping. Include launcher-held duplicates in the writer-closure audit. The distinct native
result writer is permitted to remain open; no snapshot writer is permitted.
A failed close, outstanding writer, source change, bad hash, or uncertain audit
prevents sealing. The root reopens only the confined snapshot read-only. It does
not copy or rehash image bytes outside the attempt. No sender, bootstrap, engine,
or launcher retains a writable snapshot alias at engine release.

## Two Permits and Recovery

The executor owns durable intent, host-global instance claim, aggregate identity,
manager-pending fences, exact runtime identity, idempotence, cancellation,
watermarks, terminal outcomes, and slot release. Apply those rules to both permits;
profile-specific phase records do not replace pending manager operations.

The durable Film sequence is:

1. Reserve intent and slot with the complete manifest, plan, and absolute deadline.
   Create the bounded storage and exact bootstrap. Verify placement before source
   reads, imports, generation, or decoding.
2. Persist stage release intent, then transfer the launch-bound stage offer
   and descriptors. Receipt phase becomes `staging` only when release
   is observed. The bootstrap copies or validates the synthetic manifest.
3. Receive the acknowledgement and complete the paused closure audit. Persist
   `sealed`; receipt phase `sealed` proves staging validation, not engine start.
4. Recheck cancellation/deadline. Persist engine release intent; unpause through
   the manager protocol and send `EnginePermit` on the connected stage channel.
   Unlink the listening endpoint before this permit; no child can open another
   connection. The native worker execs the fixed child with the channel and native
   result capabilities closed. A close-on-exec error pipe establishes exec success;
   native PID 1 then sends `EngineStarted` and closes its stage channel. Only that
   observation advances receipt phase to `engine`; a lost acknowledgement retains
   uncertainty rather than proving the child never started.
5. Native PID 1 supervises the fixed child and reaps all descendants. After they
   exit, phase `validating` hashes the complete output inside the attempt. Persist
   validated result and execution outcome, then settle storage/runtime ownership.

Phase `execution-finished` requires a recorded terminal execution observation;
otherwise the schema phase is the last observed execution phase, not an inference from
intent or a manager response. State may become `settling` or `blocked` at any
phase. A crash after release intent but before observed start retains uncertainty.
Recovery settles the interrupted attempt; it never reissues either permit,
continues an old render, or trusts a leftover channel as authorization.

Before observed `EngineStarted`, genuine loss of the fixed private controller
connection closes every source, snapshot, native-result, and channel capability
and terminates and reaps any spawned child. Native PID 1 remains blocked for
settlement by the exact owner or its original absolute timer. It never reconnects,
reissues or infers a permit, or extends a deadline. A live but nonresponsive
controller still must meet the initial ten-second placement deadline; a lost
connection grants no later placement or execution opportunity. Malformed,
truncated, or schema-invalid packets remain failures and are distinct from
transport loss.

Film operator fault barriers use the native protocol's sealed operator-only
mechanism. Its additional closed phases are `after-stage-release-intent`,
`after-stage-ack`, `after-snapshot-sealed`, `after-engine-release-intent`, and
`after-validated-result`. They do not expose a request/configuration path or make
barriers available to a future production mode. Cancellation before engine
release prevents it. Cancellation or deadline during any phase terminates the
whole owned subtree; an uncertain operation retains the slot beyond the deadline.

## Fixed Resource Terms and Checked Planning

The model document contains one bound for each fixed stage and each of four
terms: runtime, native workspace, allocator retention, and kernel charges.
Each is either `unknown` or `qualified` with unsigned bytes and an evidence digest.
The exact 12 stages and fields are in the schema. Each case contains every stage
once in schema order. A zero bound needs evidence that the term is absent; missing
or unknown does not mean zero. Evidence binds exact numerical bundle, fixture,
thread/cache/procedure policy, and the stated allocation ownership. An operator
number without that reviewed evidence cannot become a qualified bound.

`film-live-storage-v1` is a compiled formula identity, not an expression language.
The model cannot supply operators, slopes, programs, profiles, branch selectors,
or per-request coefficients. Compiled array inventories count the union of
simultaneously live backing allocations at each stage, including carried caller
input, views, and expression temporaries. An unproven inventory returns an unknown
`owned-arrays` term instead of a guessed coefficient. Fixed profile/LUT storage
belongs in runtime; temporary native work, unreclaimed arenas, and kernel charges
have separate owners and must not be counted twice.

For a stage with all terms qualified, checked arithmetic computes:

```text
stage = storage_reserve + source_cache + owned_arrays + local_scratch
      + runtime + native + allocator_retention + kernel
required = maximum(stage for each sequential stage)
```

Reserve the full 4 GiB shared storage cap in every stage. `source_cache` reserves
the TIFF source byte length in addition to the snapshot, or zero for synthetic
input. Existing external cache residency is not assumed to reduce headroom.
This conservative fixed storage reservation includes input/output/cache/result
pages; do not add the same tmpfs pages again as owned image arrays. The generator's
heap destination is an owned array. Staging also owns its bounded copy/hash buffer.
The formula includes validation and carried state, not just simulation peaks.

For unknown terms, `known_required_bytes` is the maximum of the checked known
per-stage reservation subtotals, plus a separate list of every missing stage/term
pair. Never sum sequential stages merely because their terms are incomplete. This subtotal is not a measured lower bound on actual memory use.
Prediction is `unqualified` even if that subtotal already exceeds the limit;
`known_terms_exceed_limit` records that fact. With no unknown terms,
use `fits-model` or `exceeds-model` according to the captured memory limit. Neither
label makes this profile ordinary admission or proves an attempt cannot OOM.

The measurement procedure grants the packaged, qualified maximum local allowance
for gamut/CCTF and the packaged JPEG row allowance. These are bundle constants,
not operator/request values or inferred native headroom. Gamut and CCTF retain
their shared allowance and separate local models. Compute both plans from exact
geometry; JPEG must admit at least one full-width row. Validate range, scratch,
batch, destination bytes, dtype/layout, tail, and integer bounds before allocation.
The fixed local models are defined by the
[engine workspace contract](processing-memory.md#bounded-pointwise-computation),
not copied into the operator document.

Rust includes those exact plans and prediction in `EngineGrant`. The adapter
recomputes local plans and rejects any difference before image allocation. It
cannot raise an allowance or change precision. The root-only measurement profile
may deliberately run `unqualified` or `exceeds-model` cases under the finite kernel
limit to obtain evidence. This authority is the fixed profile, not an admission
bypass flag or automatic fallback. Ordinary Film remains subject to the separate
qualified admission contract.

## Fresh Process and Result Handoff

The packaged native worker is PID 1 and invokes only
`/opt/runtime/bin/python /opt/film-measurement/adapter.py`, with no additional
arguments and a fixed sanitized environment. Its fixed
`SLIPSTREAM_FILM_GRANT_SHA256` variable carries the native-verified StageOffer
digest; the adapter compares the grant file against this value before parsing.
It is derived bootstrap data, never an operator/request environment override.
The native worker remains alive to supervise and
validate the child; it is not an arbitrary exec service. The engine sees the sealed
input and captured grant, not source descriptors or the stage endpoint.

Before Python import, create new private runtime directories and an empty
`/work/numba`. Pin one Numba thread and the qualified four-thread library policy;
clear conflicting inherited variables. The image must contain no compiled Numba
cache in any engine/source cache location. `film-once-empty-cache-v1` performs one
simulator/render call in its fixed order with no warm-up or cross-attempt cache.
The numerical bundle owns the seed and RNG algorithm. Cache/procedure changes
require a different reference and qualification; cleanup destroys all cache files.

The adapter verifies the input and writes only `/output/finished.jpg` and bounded
producer metadata on descriptor 3.
Set the child's `RLIMIT_FSIZE` to 512 MiB after staging and before exec. Together
with the shared tmpfs cap, this bounds output files without limiting a larger TIFF
copy. `EFBIG` or `SIGXFSZ` is an output-limit failure. This is a storage constraint,
not the memory-enforcement mechanism.

After the child and descendants exit, native validation opens the fixed output
through a confined descriptor. Reject links, wrong type, extra output artifacts,
or oversize files. Stream its SHA-256 in at most 64 KiB chunks and require the
catalogue's exact JPEG length and digest, not merely the producer's reported hash.
This establishes byte identity with a previously independently validated complete
JPEG, including its geometry and ICC. No new JPEG parser or decoder is needed.
Require producer input/Film pixel digests and geometry/ICC metadata to match the
same reference. The Film digest remains a bounded engine observation; the native
validator does not claim to recompute the simulation independently.

Preallocate a 16 KiB native result record so storage exhaustion can still be
reported. Its native-only descriptor and host-only directory prevent the child from
writing, unlinking, or replacing this record. The child receives only an anonymous
write-only pipe at fixed descriptor 3 for producer metadata. Native PID 1 drains it
concurrently with child supervision. It carries exactly one four-byte big-endian
length followed by that many UTF-8 JSON bytes, at most 16380 payload bytes. No
ancillary rights are possible on this pipe. Require the complete frame and EOF;
reject premature EOF, extra/trailing bytes, oversized length, and invalid schema.
A blocked or partial producer frame never extends the absolute execution deadline.

The schema's closed `ProducerResult` is either `produced` with pixel/geometry/ICC
observations and unique engine-stage timings, or a typed failure. It binds launch,
manifest, and plan identities. It has no JPEG digest, output path, or native
completion authority. Native PID 1 checks those observations against the reference,
adds its own complete JPEG length/hash validation, and alone writes the final
`WorkerResult`. Its execution time includes staging and native validation; producer
timings cannot impersonate those stages. Validate all identities and reference
equality before durable handoff. Both records and channels are at most 16 KiB. The native record contains a four-byte big-endian payload length, UTF-8
JSON of that length (at most 16380 bytes), and zero padding to 16384 bytes. Length
starts at zero; the native writer synchronizes the completed payload before
publishing its length. The launcher reads its retained original inode and verifies
the captured device/inode/mount, size, ownership, and mode, never a replaced path.
Interrupted/partial/invalid records cannot establish completion. An
unexpectedly absent timing stays absent rather than becoming an invented zero;
reclaim time is separate from stage computation time.

The receipt contains only the verified artifact descriptor and bounded evidence.
It returns no image bytes, output path, lease, download handle, or publication
permission. Persist the result, confirm subtree termination, and reclaim all
input/output/cache storage before slot release. Close all launcher-held source,
snapshot, result, and mount descriptors after bounded evidence capture and before
confirmed storage reclamation; an unmounted name alone cannot prove that retained
descriptors released charged pages. No image bytes survive settlement.
This handoff cannot mark a product Export successful.

## Identity, Receipts, and Failures

Policy identity binds profile, configured limits, retention, instance authority,
and the resolved image. Its canonical typed value is `PolicyIdentity` in the
schema, containing the complete validated configuration and exact local image ID.
Bundle identity binds packaged worker/adapter and the
numerical bundle. The manifest binds the selected fixture/reference, recipe,
catalogue/model digests, procedure, bundle, captured policy, and computed plan.
For these derived digests, use SHA-256 over canonical UTF-8 JSON without a BOM.
Sort object keys recursively by ASCII order (all schema keys are ASCII), preserve
array order and required nulls, omit insignificant whitespace, and encode integers
in decimal without a sign or leading zero except the value `0`. For every string,
escape quote and backslash as `\"` and `\\`; encode U+0008, U+0009, U+000A, U+000C,
and U+000D using `\b`, `\t`, `\n`, `\f`, and `\r`. Encode each other U+0000–U+001F
control as `\u00xx` with lowercase hexadecimal digits. Emit every other Unicode
scalar literally as UTF-8, including slash, without Unicode normalization. Reject
unpaired surrogates; never replace them. These rules match `serde_json` and Python
`ensure_ascii=False` for the permitted typed values. They also cover Unicode
configuration paths inside `PolicyIdentity`; the narrower manifest has no paths.
[Canonical golden vectors](schemas/processing-film-measurement-canonical-vectors.json)
pin the byte encoding and hashes for both implementations. The schema defines the captured typed records;
the canonical manifest is the schema's `CanonicalManifest`, holding the validated
values. `plan_sha256` uses the same encoding of the complete `Plan` object.

Start verifies the caller's policy, bundle, catalogue, and model identities against
the captured instance. Unknown fixture or unsupported geometry fails before
reservation. For an already known identity, compare with its captured accepted request and
return the existing receipt; do not resolve that identity through a newly loaded
catalogue/model. Changed intent at the same identity conflicts. Inspect, cancel, expiry, and capacity follow
the native protocol's semantics. Reconcile adds catalogue/model identities and
advertises only `film-measurement-only`. Availability establishes this closed
operator capability, not a supported Photo or general Film envelope.

Receipt base fields retain the
[native receipt meanings](processing-executor-protocol.md#responses-and-receipts).
The schema adds observed phase, captured manifest/catalogue/model, exact plan,
validated worker result, and a closed detail code. `running` begins with observed
stage release and does not imply Python ran. `completed` requires a verified
success result. No outcome follows merely from a zero Python exit.

Source/hash drift, a confirmed failure to seal input, unsupported decoded input,
plan disagreement, and invalid artifact produce `engine-failed` with their
respective detail. A confirmed sealing failure uses `source-mismatch`; unavailable
audit evidence cannot establish that detail and remains an interrupted or
uncertain execution under the executor's settlement contract. Output file-size
exhaustion produces `storage-full` with `output-limit`. Preflight overflow or
unsupported contracts refuse a start; runtime allocation failure and confirmed
cgroup OOM remain distinct. Missing model terms are a prediction, not a fabricated
runtime error or permission to claim normal admission. Kernel OOM evidence takes
precedence over a partial worker record. Missing evidence remains unknown.

Outcome and cleanup remain separate. Once a completed execution is recorded,
a later cancel cannot rewrite it; its storage still needs settlement. A lost
control connection does not cancel accepted work. Recovery cannot publish orphaned
work, recreate an ambiguous aggregate, or clear an ownership fence by choosing a
new journal. Cleanup uncertainty blocks admission and retains owned resources.

## Options

### Selected: Closed Measurement Profile and Exact References

A separate profile can collect missing bounds without declaring unsupported work
admitted. A finite catalogue and exact output references make every accepted
experiment concrete. Descriptor-only handoff supplies reproducibility evidence
without creating a second retained-output store or Export publication lifecycle.

### Rejected: Extend Native Fixtures with Commands or Mutable Paths

A command field, Docker override, or mutable read-only source bind would enlarge
launcher authority and bypass source sealing. A separate staging container adds
another uncertain manager lifecycle. The verified native bootstrap can stage
through two narrowly typed descriptors inside the existing attempt instead.

### Selected: Capped Tmpfs and Closed Byte Terms

The snapshot and all writable aliases share one enforceable byte/inode bound.
Compiled formulas and fixed evidence terms keep planning reviewable. A memfd alone
would sit outside that tmpfs quota; arbitrary model expressions would require an
interpreter and a second admission language.

## Verification

Validate every example and all positive/negative syntax vectors against the schema.
Parser tests separately cover duplicate keys, integer token syntax, frame limits,
and ancillary rejection. Both implementations consume the canonical golden
vectors, including invalid surrogates and unsigned integer boundaries. Semantic tests cover fixture/stage uniqueness, document
hash binding, cross-record identities, checked arithmetic, unknown terms, and exact
Rust/adapter plan agreement. No test upgrades an unknown model to qualified.

Actual launcher evidence must establish placement before copy/import, immutable
sealing before engine release, complete writer closure, shared byte/inode caps,
private empty cache, both source variants, exact complete output identity, retained
peak/event evidence, and ensuing successful work after cleanup. Exercise source
mutation, retained duplicate writers, child attempts to acquire the native result
writer through process interfaces, output/storage exhaustion, cancellation and
restart at both permits, and uncertain manager outcomes. All native qualification
checks remain independently applicable.

The operator can exercise writer rejection with the fixed
`faults/retain-snapshot-writer.json` file. It follows the executor fault files'
root ownership, permissions, no-link, regular-file, and 1024-byte restrictions
and contains exactly `incarnation` and `sequence`. Only a matching Film TIFF
attempt may act on it: immediately before its first permit, the launcher retains
one duplicate of its existing snapshot writer until the sealing audit or terminal
cleanup. The ordinary audit must reject that writer before engine release. The
existing `after-stage-ack` barrier allows the verifier to observe the exact inode
and writable descriptor before continuing. No public request, worker mount,
configurable path, or reconnect exposes this fault action; it never creates a
new source or executable authority. Restart never repeats the action or releases
an old attempt, and every terminal path closes the duplicate before storage
reclamation. Other modes reject the fault file. This one closed action tests the
real audit without introducing a general fault program or alternate worker.

Synthetic content/geometry coverage and private full-resolution references prove
only their registered cases. General camera support, resource coefficients,
minimum/recommended budgets, control-service headroom, latency, and image quality
require their own qualification. Keep measurements, delivery tracking, and corpus
coverage reports in the governing Issue and change review, not this specification.

## Executable Syntax Examples

These examples use illustrative identities. They validate as the named schema
definitions; operating an instance additionally requires the real files, hashes,
image, reference evidence, and semantic checks above. No placeholder identity is
accepted as operational evidence.

### Config

```json
{
  "version": 2,
  "mode": "film-measurement",
  "instance": "0123456789abcdef0123456789abcdef",
  "root": "/var/lib/slipstream-processing/0123456789abcdef0123456789abcdef",
  "socket": "/run/slipstream-processing/0123456789abcdef0123456789abcdef.sock",
  "peer_uid": 0,
  "image": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
  "memory_bytes": 17179869184,
  "receipt_retention_seconds": 86400,
  "catalogue_sha256": "2222222222222222222222222222222222222222222222222222222222222222",
  "resource_model_sha256": "3333333333333333333333333333333333333333333333333333333333333333"
}
```

### Catalogue

```json
{
  "version": 1,
  "numerical_bundle": "8888888888888888888888888888888888888888888888888888888888888888",
  "recipe": "9999999999999999999999999999999999999999999999999999999999999999",
  "procedure": "film-once-empty-cache-v1",
  "reference_image": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
  "input_icc_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "output_icc_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "fixtures": [
    {
      "id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "width": 19,
      "height": 17,
      "source": {
        "kind": "synthetic-rgb",
        "generator": "linear-rgb-f32-v1",
        "pattern": "dark",
        "seed": 0
      },
      "reference": {
        "input_pixels_sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "film_pixels_sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        "jpeg_sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "jpeg_bytes": 4096,
        "evidence_sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
      }
    },
    {
      "id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "width": 19,
      "height": 17,
      "source": {
        "kind": "development-tiff",
        "bytes": 8192,
        "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
      },
      "reference": {
        "input_pixels_sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        "film_pixels_sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "jpeg_sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "jpeg_bytes": 4096,
        "evidence_sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
      }
    }
  ]
}
```

### StageBounds

A complete resource-model case contains every schema stage once. This is one
stage's closed value; zero-byte qualified terms would require reviewed evidence.

```json
{
  "stage": "development",
  "runtime": { "status": "unknown" },
  "native": { "status": "unknown" },
  "allocator_retention": { "status": "unknown" },
  "kernel": { "status": "unknown" }
}
```

### Start

```json
{
  "version": 2,
  "instance": "0123456789abcdef0123456789abcdef",
  "op": "start",
  "incarnation": "11111111111111111111111111111111",
  "sequence": 1,
  "policy": "4444444444444444444444444444444444444444444444444444444444444444",
  "bundle": "5555555555555555555555555555555555555555555555555555555555555555",
  "catalogue": "2222222222222222222222222222222222222222222222222222222222222222",
  "resource_model": "3333333333333333333333333333333333333333333333333333333333333333",
  "workload": {
    "kind": "film-fixture",
    "fixture_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  }
}
```

### Response

```json
{
  "version": 2,
  "result": {
    "kind": "capability",
    "capability": "film-measurement-only",
    "instance": "0123456789abcdef0123456789abcdef",
    "incarnation": "11111111111111111111111111111111",
    "next_sequence": 1,
    "policy": "4444444444444444444444444444444444444444444444444444444444444444",
    "bundle": "5555555555555555555555555555555555555555555555555555555555555555",
    "catalogue": "2222222222222222222222222222222222222222222222222222222222222222",
    "resource_model": "3333333333333333333333333333333333333333333333333333333333333333",
    "availability": "available",
    "active": null
  }
}
```

### WorkerFailure

```json
{
  "version": 2,
  "kind": "film-measurement-result",
  "outcome": "engine-failed",
  "detail": "source-mismatch",
  "phase": "staging",
  "launch_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "manifest": "6666666666666666666666666666666666666666666666666666666666666666",
  "plan_sha256": "7777777777777777777777777777777777777777777777777777777777777777",
  "execution_us": 750
}
```

### WorkerSuccess

```json
{
  "version": 2,
  "kind": "film-measurement-result",
  "outcome": "completed",
  "launch_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "manifest": "6666666666666666666666666666666666666666666666666666666666666666",
  "plan_sha256": "7777777777777777777777777777777777777777777777777777777777777777",
  "artifact": {
    "input_pixels_sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "film_pixels_sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    "jpeg_sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    "jpeg_bytes": 4096,
    "width": 19,
    "height": 17,
    "icc_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "reference_evidence_sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
  },
  "execution_us": 123456,
  "stages": [
    { "stage": "staging", "elapsed_us": 750, "reclaim_us": 0 },
    { "stage": "validation", "elapsed_us": 900, "reclaim_us": 0 }
  ]
}
```
