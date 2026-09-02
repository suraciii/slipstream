# Agent Instructions

## Project Overview

Slipstream is in initial product definition.

- `docs/`: product specifications and user-visible behavior
- `design/`: architecture decisions and implementation contracts
- `CONTEXT.md`: canonical product and domain language

## Engineering Principles

- Follow KISS and YAGNI. Implement the smallest complete behavior required now.
- Prefer established open-source libraries and repository patterns over custom infrastructure.
- Keep models small and ownership boundaries explicit.
- Add abstractions only for demonstrated duplication or a clear boundary benefit.
- Grow the product in working end-to-end increments.
- Do not preserve obsolete paths unless a specification requires compatibility.
- Treat original photo files as irreplaceable user data. Default to read-only access and fail without modifying them.
- When a change retires or renames an HTTP route or wire value, confirm the deployment host's operator tooling still verifies the deployment end to end before merging.

## Context Management

- Read this file first, then `CONTEXT.md`.
- Follow the scoped `AGENTS.md` in `docs/` or `design/` when working in those directories.
- Read the governing Issue and affected specifications before implementing a change.
- Keep this file limited to repository-wide rules. Put scoped guidance beside the files it governs.
- Keep Product and Design Specs limited to durable target behavior, boundaries, decisions, and contracts.
- Put temporary measurements, probes, implementation progress, and one-off evidence in the governing Issue or change review, not in Specs or `CONTEXT.md`.
- Define each rule in one authoritative document. Link to it instead of copying it.

## Spec-First Development

- Specify user-visible behavior in `docs/` before implementation.
- Add or update a Design Spec in `design/` when work changes boundaries, ownership, lifecycle, data contracts, failure behavior, or other durable technical decisions.
- Resolve product and architecture decisions in the governing Issue and specifications before editing implementation code.
- Specifications describe the durable target. Do not add implementation-progress sections or temporary gaps to them.
- Treat examples as contracts. They must parse or run as written when tooling exists.

## Verification

- Once an implementation toolchain exists, define focused checks and the full local gate in `CONTRIBUTING.md`.
- Every behavior change requires focused automated coverage.
- Run applicable focused checks during development and every defined full gate before handoff.
- Record exact commands and results in the change review and pull request.
- Report missing tools, environment limits, inapplicable checks, and failed checks. Do not hide them.
