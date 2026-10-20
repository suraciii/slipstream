# Agents: Writing Contracts

`specs/` defines Slipstream's durable product behavior, design decisions, and implementation contracts. Use the [index](README.md) to find the affected feature and its contracts. Follow the repository-wide context and artifact rules in [`../AGENTS.md`](../AGENTS.md).

## Structure

```text diagram
specs/
├── README.md       # Contract index
├── AGENTS.md       # This file
├── spec.md         # Product Vision and product-wide behavior
├── design.md       # Shared system architecture
├── server.md       # Root specialist contract
└── {feature}/
    ├── spec.md         # Observable behavior
    ├── design.md       # Technical decisions
    ├── {specialist}.md # Additional scoped contract or reference
    └── {capability}/
        ├── spec.md
        ├── design.md
        └── {specialist}.md
```

## Context Layers

- **Product Vision** ([spec.md](spec.md)) → why the product exists.
- **Domain Language** ([CONTEXT.md](../CONTEXT.md)) → what each term means.
- **Spec Documents** ([index](README.md), root and nested contracts) → what the product does and which technical boundaries govern it.
- **Usage Guidance** ([docs/](../docs/README.md)) → how a Photographer uses the product; it is not a contract authority.
- **Implementation and Tooling** (`apps/`, `crates/`, `scripts/`, and `tools/`) → how the product and repository tooling implement the contracts.

Higher layers constrain lower layers. A lower layer must not redefine or contradict a higher one.

## Feature Organization

- `specs/spec.md` owns product-wide behavior; `specs/design.md` owns shared system architecture. Root specialist references, such as `specs/server.md`, stay beside these contracts.
- Keep each Feature's contracts in one or two files under `specs/<feature>/` by default.
- Add `specs/<feature>/<capability>/` only when the Feature's contracts become too large to read and maintain clearly in one or two files.
- Each level may own `spec.md` for observable behavior, `design.md` for technical decisions, or both. Create only files with substantive contracts.
- Parent contracts own shared rules; child contracts own their specific rules and link to the parent. Read applicable root and Feature contracts when working on a child capability.
- Keep specialist references beside their owning contracts and link to them. List every contract in the index with its scope.
- Before implementation, update affected behavior contracts. Also update design contracts when work changes boundaries, ownership, lifecycle, data contracts, ordering, failure behavior, or other durable technical decisions. Resolve decisions in the governing Issue and contracts before editing implementation code.

## Common Writing Rules

- State the problem and the constraints or trade-offs that require a rule or decision.
- Define behavior before its API, JSON, database, or source-code representation. Include exact mechanics only when they form a durable contract or remove ambiguity.
- State observable behavior, boundaries, invariants, ordering, invalid states, and failure outcomes explicitly.
- Use `must`, `may`, and `must not` for requirements, options, and prohibitions. Preserve exact product names and identifiers.
- Give each section one purpose. Prefer a short list to a dense paragraph. Do not use tables.
- Use `text diagram` for ASCII diagrams and `text literal` for other preformatted text. Do not use bare `text` fences or raw HTML.
- Treat examples as contracts. Keep them minimal and independently checkable; they must parse or run as written when tooling exists.
- Delete empty sections. Structure follows the contract rather than a fixed document shape.
- **Authority** (optional): name the owning module and cross-capability dependencies at the top of a contract. This makes contract and code drift visible.
- **Laws** (optional): end a contract with numbered invariants that are directly testable. Laws complement behavior with what must always hold.

## Product Behavior

Write for Photographers and implementers who do not read source code. Lead with the Photographer's problem. Explain behavior in product and domain language rather than transcribing classes, handlers, storage steps, or source call chains.

A behavior contract typically states the problem, observable rules and boundaries, failure and recovery behavior, and examples with inputs and expected outcomes that resolve ambiguity.

## Design Decisions

Describe target design and why its boundaries exist. Define the model before APIs, fields, persistence, or algorithms. State ownership, scope, identity, lifecycle, invariants, ordering, and failure behavior.

For each non-trivial decision, compare at least two reasonable options. Record why the selected option fits and why rejected options fail the current constraints. Add concepts, extension points, and compatibility paths only for current requirements.

A design contract typically states the problem, design drivers, model, semantics, selected and rejected options, and evidence that proves the contract. Have an independent reviewer derive the expected behavior from the contract before implementation.
