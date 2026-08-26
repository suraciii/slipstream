# Design Specs

`design/` explains why architecture boundaries exist and records contracts that implementations must preserve. It is not a tour of the current code.

Follow the scoped instructions in [`AGENTS.md`](AGENTS.md) when writing or changing a Design Spec. Product behavior belongs in [`../docs/`](../docs/README.md), and shared language belongs in [`../CONTEXT.md`](../CONTEXT.md).

## Foundations

- [Foundational Architecture](architecture.md): local Web deployment, file ownership, Photo identity, state ownership, and the first vertical slice
- [Rust Server Architecture](rust-server.md): production language, module ownership, dependency direction, compatibility, cutover, and rollback
- [Preview Pipeline](preview-pipeline.md): matching-JPEG and embedded-JPEG selection, extraction, normalization, caching, and delivery
