# Agent Instructions

## Project Context

- [`CONTEXT.md`](CONTEXT.md) defines canonical product and domain vocabulary.
- [`specs/`](specs/README.md) is the authority for product behavior, design decisions, and implementation contracts.
- [`docs/`](docs/README.md) provides usage and installation guidance.

## Engineering Principles

- Follow KISS and YAGNI. Implement the smallest complete behavior required now.
- Prefer established open-source libraries and repository patterns over custom infrastructure.
- Keep models small and ownership boundaries explicit.
- Add abstractions only for demonstrated duplication or a clear boundary benefit.
- Grow the product in working end-to-end increments.
- Do not preserve obsolete paths unless a specification requires compatibility.
- Treat original photo files as irreplaceable user data. Access them read-only; file deletion belongs to the user outside Slipstream. Follow the [Trash record-cleanup boundary](specs/photo-library/trash/spec.md) for application records.
- When a change retires or renames an HTTP route or wire value, confirm the deployment host's operator tooling still verifies the deployment end to end before merging.

## Context Management

- Read this file first, then [`CONTEXT.md`](CONTEXT.md).
- Before implementing or reviewing a behavior change, read the governing Issue and use the [`specs/` index](specs/README.md) to find and read the affected behavior contracts.
- When work changes boundaries, ownership, lifecycle, data contracts, ordering, or failure behavior, also read the affected design and named specialist contracts linked from the index and owning contracts.
- Before creating or changing a contract, follow [`specs/AGENTS.md`](specs/AGENTS.md). Before changing usage or installation guidance, follow [`docs/AGENTS.md`](docs/AGENTS.md).
- Follow [Language and Writing](CONTRIBUTING.md#language-and-writing) for project artifacts, including Issues, pull requests, and commits.
- Keep this file limited to repository-wide rules. Put scoped guidance beside the files it governs.
- Define each rule in one authoritative document. Link to it instead of copying it.
- Keep delivery plans, dependencies, implementation tracking, temporary measurements, probes, and one-off evidence in the governing GitHub Issue or pull request. Specifications describe durable targets; `CONTEXT.md` defines vocabulary.

## Verification

- Once an implementation toolchain exists, define focused checks and the full local gate in `CONTRIBUTING.md`.
- Every behavior change requires focused automated coverage.
- Run applicable focused checks during development and every defined full gate before handoff.
- Record exact commands and results in the change review and pull request.
- Report missing tools, environment limits, inapplicable checks, and failed checks. Do not hide them.
