# Slipstream

Slipstream is a browser-based workspace for reviewing and selecting photos without modifying the original RAW or JPEG files.

The product contract is in [`docs/`](docs/README.md). Durable architecture contracts are in [`design/`](design/README.md).

The production server architecture is Rust; Bun and TypeScript own the browser application, browser tests, and repository tooling. The existing TypeScript server remains a compatibility and rollback implementation during the staged migration. See [`design/rust-server.md`](design/rust-server.md) for the binding boundary, [`deploy/README.md`](deploy/README.md) for the Docker cutover and rollback procedure, and [`CONTRIBUTING.md`](CONTRIBUTING.md) for setup and the canonical `bun run verify` gate.
