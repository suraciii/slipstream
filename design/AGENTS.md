# Agents: Writing Design Specs

Design Specs state why Slipstream has a boundary and which contracts implementation must preserve. They describe target design, not a transcription of current code.

## Rules

- Start with the problem and the forces that require a design decision.
- Define the model before APIs, fields, persistence, or algorithms.
- State ownership, scope, identity, lifecycle, invariants, ordering, and failure behavior.
- Compare at least two reasonable options for a non-trivial decision. Record why the selected option fits and why rejected options fail the current constraints.
- Keep exact mechanics only when they form a durable contract or remove ambiguity.
- Use one authoritative source for each rule. Link instead of duplicating it.
- Describe behavior before its JSON, API, database, or source-code representation.
- Do not add extension points, compatibility paths, or concepts for hypothetical future needs.
- Keep temporary implementation state, experiments, measurements, and open task tracking out of Design Specs. Record them in the governing Issue or change review.
- Have an independent reviewer derive the expected behavior from the spec before implementation.

## Minimal Shape

```markdown
# Name

The problem and why a design decision is necessary.

## Design Drivers

Constraints, forces, and trade-offs.

## Model

Concepts, ownership, references, and invariants.

## Semantics

Ordering, state changes, interfaces, errors, and failure behavior.

## Options

Selected and rejected approaches with reasons.

## Verification

Evidence that proves the contract.
```
