# Slipstream

Slipstream is a browser-based workspace for reviewing and selecting photos without modifying the original RAW or JPEG files.

The product contract is in [`docs/`](docs/README.md). Durable architecture contracts are in [`design/`](design/README.md).

The production server architecture is Rust; Bun and TypeScript own the Web application, browser tests, and repository tooling. The repository contains no Node rollback server; the sealed rollback artifact is maintained outside the source tree by the operator. See [`design/rust-server.md`](design/rust-server.md) for the binding boundary, [`deploy/README.md`](deploy/README.md) for the Docker deployment and rollback procedure, and [`CONTRIBUTING.md`](CONTRIBUTING.md) for setup and the canonical `bun run verify` gate.
