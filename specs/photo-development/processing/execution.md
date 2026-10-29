# Local Photo Executor

The executor supervises an independent file/state executable; that tool owns its private native children. [Photo Development Architecture](../design.md) owns captured state and run attribution. [Photo Workspace](../workspace/design.md) owns files and indexing.

[Processing Memory](memory.md) owns finite resource policy, and [Native darktable Integration](../darktable/design.md) owns native preparation.

## Model

The Web service owns one bounded heavy-work queue and supervisor. At most one supervised attempt runs at a time; Preview and Export share bounded fair admission. This lock covers only supervised work, not independent commands.

Per-edit tool locks prevent conflicting state writes without locking the whole workspace.

Each standalone command owns its input/state capture, finite resource policy, private native process group, scratch, and synchronous terminal result. Web invokes the same tool against authorized workspace paths and retains a captured receipt. The executable's pinned bundle MUST be usable without Slipstream.

Optional bundles identify engine/runtime, semantic schemas, profiles, and native assets. Startup and discovery MUST verify manifest identity and digests per tool.

Missing bundles leave the Library and completed workspace files usable. Discovery and bundle presence alone MUST NOT qualify arbitrary controls or source/output classes.

Service startup policy controls permitted executables and resource capacity. HTTP MUST NOT select arbitrary binaries or raise execution limits. The boundary does not introduce an independent daemon, host launcher, generic plugin registration, arbitrary-shell API, or per-request container.

## Execution

The Web supervisor resolves confined paths and invokes an argument vector with a unique run identity and bounded structured output. Shell interpretation MUST NOT occur. A remote caller receives the true execution host/path facts and MUST use an environment with access; no implicit remote mount or synchronization is supplied.

The tool captures canonical revision and input content evidence under its local state rules. Protected inputs are reused; mutable external input first receives a protected snapshot under the [workspace contract](../workspace/design.md#stable-bytes-and-containment).

Native children receive read-only inputs and resources with fresh private config, catalog, and cache. Original/XMP, desktop presets, and the service database are outside native writes. Ambient sidecar, crawler, and display behavior MUST be disabled.

Private protocol values, diagnostics, and result sizes MUST be bounded. Malformed, truncated, oversized, or incompatible responses MUST fail explicitly.

Finite deadlines cover computation and blocked exchanges. Thread limits cover darktable, NumPy/BLAS, Numba, and FFTW. Cancellation or deadline MUST terminate all owned descendants, reap children, and settle scratch before the Web processing slot is released.

The tool supervises its native group; Web also owns the tool group. Unexpected parent exit MUST terminate that owner's native children through a qualified parent-death/process-group mechanism. A standalone tool is not a service child and is unaffected by service exit.

Unconfirmed termination MUST stop affected heavy admission; it is neither successful cancellation nor permission to reuse scratch.

Output publication follows [Export execution](../design.md#export-execution-and-recovery). A failed or cancelled run preserves saved state and prior completed files. Native failure MUST NOT substitute another tool or camera result.

For retained workspace-backed adapters, a successful Export status read MUST confirm the output's aliased workspace revision and exact retained byte evidence through pinned download validation and a lease. The lease MUST remain held until the temporary execution input dependency is released, before returning `succeeded`. Failed or cancelled status reads release that dependency after durable settlement; output-owned dependencies remain retained.

A registry alias alone MUST NOT establish current output availability. Changed, truncated, missing, or removed aliased outputs yield an unconfirmed result and preserve any remaining input pin. Reconciliation MUST NOT recreate them from historical private bytes.

A historical work record MAY remain `succeeded`, but a new status response MUST NOT claim verified availability without current evidence.

## Restart

The service reconciles its own durable queued and running receipts. Queued work MUST recheck captured revision, input, resources, bundle, and current limits before dispatch.

Interrupted running work MUST fail explicitly unless exact validated completed-file and invocation evidence proves success. It MUST NOT be blindly rerun or attached to an unidentified prior process.

Tool-local state and receipts reconcile interrupted mutations independently. Atomic installation retains the prior or complete new revision; private partial outputs are not results.

Abandoned scratch MAY be removed only when ownership and absence of live users are established. The service MUST NOT sweep an entire workspace `.tmp/` while an external CLI can still be working there.

Service restart neither cancels nor adopts standalone work. A CLI can complete while the service is offline; later reconciliation indexes completed files.

Without verified local invocation evidence those files remain ordinary ready files, not invented successful server Exports. Workspace input, current state, and kept-output retention is independent of transient receipt expiry.
