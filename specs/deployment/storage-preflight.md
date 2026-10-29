# Compose Host Storage Preflight

The host preflight protects the Original Files against aliases that the server cannot see after Docker maps separate host directories to container paths. [Deployment](spec.md) owns operator grammar, host prerequisites, configuration precedence, and QA identity.

## Storage Proof

The repository-owned `scripts/compose` entry point MUST perform host preflight before supported startup, Library Expansion, and access administration. It MUST use the fixed repository Compose configuration and disable builds.

The preflight MUST canonicalize the existing Library, state, and cache sources without creating them. For each source it MUST derive a proof set from the endpoint and its complete nested mount hierarchy, pairing filesystem identity with effective path within that filesystem.

Two roles are incompatible if their canonical paths are equal or nested. They are also incompatible if any of their proof coordinates have the same filesystem identity and equal or nested paths.

This comparison includes nested mounts on different filesystems. It protects both Originals and the separation between durable state and rebuildable cache.

Ambiguous source or mount information MUST fail closed. A rejected startup MUST identify incompatible roles and exit before invoking Compose or changing the Originals tree or content.

After admission, the entry point MUST pass the checked canonical sources as Docker mount sources, container paths, and server configuration values. The server's container-visible storage admission remains a separate defense.

The proof requires a trusted local Docker daemon and socket in the entry point's host mount namespace. The operator MUST keep the source directories and parent mount topology stable between preflight and Docker admission; preflight does not eliminate this time-of-check/time-of-use window.

Docker MUST NOT restart the service autonomously around preflight. Any automatic recovery MUST use a preflight-aware host start path.

## QA Isolation

A [short-lived QA instance](spec.md#short-lived-qa-instance) MAY share the trusted local Docker Engine with the ordinary instance. It MUST NOT share its Compose project, network, published port, or storage.

The launcher MUST derive the three sources from the explicit QA ID and reject caller-provided storage declarations. It MUST apply identity and fixture checks to QA preflight actions, and check the explicit loopback endpoint on startup.

QA startup and access administration MUST use the same image, storage, and credential checks as the ordinary instance. A rejected QA invocation MUST NOT call Compose for either project.

The QA fixture root MUST NOT contain symlinks or nested mounts that could alias production storage. Shared daemon capacity and host mount aliases remain operator preconditions; this is not an isolation boundary for untrusted workloads.

## Stop Exception

The supported stop operation is not a startup path. It MAY run without image, source-existence, or topology checks so an operator can stop a container after configuration becomes unavailable or unsafe.

QA stop MUST target only its explicit project using stop-only sentinels, even when fixture paths or image disappear. Neither stop operation removes host directories.
