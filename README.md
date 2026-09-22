# Slipstream

Slipstream is a photo selection and organization workspace for a Photographer and their own Agent. Its target product provides a visual Web application and a machine-friendly CLI over shared photo capabilities, without modifying the original RAW or JPEG files. Slipstream does not include an Agent.

The product contract is in [`docs/`](docs/README.md), including [Command-Line Use](docs/command-line.md) and the [CLI Reference](docs/cli-reference.md). Durable architecture contracts are in [`design/`](design/README.md). Delivery planning and progress are tracked in [GitHub Issues](https://github.com/suraciii/slipstream/issues).

The production server architecture is Rust; Bun and TypeScript own the Web application, browser tests, and repository tooling. The repository contains no Node rollback server; the sealed rollback artifact is maintained outside the source tree by the operator. See [`design/rust-server.md`](design/rust-server.md) for the binding boundary, [`docs/0.1-support-and-release.md`](docs/0.1-support-and-release.md) for the 0.1 support boundary and release notes, [`docs/deployment.md`](docs/deployment.md) for the Docker deployment contract, and [`CONTRIBUTING.md`](CONTRIBUTING.md) for setup and the canonical `bun run verify` gate.

## License

Slipstream is available under the [MIT License](LICENSE). Third-party components retain their own terms; see [Third-Party Notices](THIRD-PARTY-NOTICES.md) and the bundled [Rust component licenses](RUST-LICENSES.html).
