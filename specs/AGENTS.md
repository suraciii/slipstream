# Specification Instructions

## Structure

```text
specs/
├── README.md              # Contract index
├── AGENTS.md              # This file
├── spec.md                # Product vision and shared behavior
├── design.md              # Shared system boundaries and invariants
└── {topic}/
    ├── VISION.md          # Optional topic vision
    ├── spec.md            # Behavior owned by this topic
    ├── design.md          # Boundaries and invariants owned by this topic
    ├── {reference}.md     # Optional specialist contract
    └── {sub-topic}/       # Same shape, nested on demand
```

- Nest topics to any depth when a child owns a distinct contract scope.
- Each topic MUST have `spec.md`, `design.md`, or both. Create only files with substantive contracts.
- Keep specialist contracts beside their owner, including at the root.
- List every contract in the [index](README.md) with its scope.

## Rules

- Follow [documentation language rules](../eng/documentation-language.md).
- Write durable target state only.
- Rewrites MUST preserve the target contract. Treat requirement changes as separate product decisions.
- Define shared domain terms in [CONTEXT.md](../CONTEXT.md). Define topic-specific concepts in the owning contract and link to shared terms.
- Higher layers constrain lower layers. Never the reverse.
- Follow the repository's [context and artifact rules](../AGENTS.md#context-management) for required reading and transient material.
- Resolve decisions in the governing Issue and update affected contracts before implementation.
- Put durable non-contract conclusions in the relevant Issue or pull request.
- Do not use `docs/` as a competing product or design authority.
- **Authority** (optional) — name the owning capability and link to dependencies.
- **Laws** (optional) — state numbered, directly testable invariants that MUST always hold.

## Detail Placement

- `spec.md` states durable, observable product behavior and invariants. It MUST NOT contain implementation or test procedures.
- Tests own concrete use cases, Given/When/Then scenarios, fixtures, exact tolerances, byte-identity checks, failure injection, and step sequences.
- Help, schemas, and code own exhaustive field, option, and parameter inventories.
- Issues and pull requests own migration steps, implementation gaps, temporary counts and dates, command transcripts, evidence matrices, and rejected alternatives.
- `## Acceptance` is optional and states at most three observable outcomes. It does not prescribe how to test them.

## Presentation

Choose the smallest form that makes the target contract clear:

| Content                                        | Form                                      |
| ---------------------------------------------- | ----------------------------------------- |
| Branching behavior or decision order           | Non-executable, contract-level pseudocode |
| States and transitions                         | Mermaid state diagram                     |
| Interactions or data flow                      | Mermaid sequence or flow diagram          |
| Ownership or hierarchy                         | Shallow text tree or Mermaid              |
| Fields, independent conditions, or comparisons | Markdown table                            |
| Simple requirement or exception                | Short sentence                            |

- A prose paragraph carries one rule and MUST NOT exceed three sentences or 60 words.
- A bullet carries one rule and MUST NOT exceed two sentences.
- A sentence MUST NOT exceed 40 words.
- Three or more parallel items use a table or a list, never a comma-separated sentence.
- Diagrams, tables, and pseudocode state the contract; do not repeat them in prose.
- Combine diagrams and pseudocode only when they express distinct parts of the contract.
- A route set, state matrix, ownership map, or selection order uses the matching form above; prose carries only constraints the form does not express.
- Make conditions, outcomes, scope, and failure effects explicit.
- Show only contract-relevant actors, states, branches, and boundaries; preserve ownership and order.
- Keep visuals in the owning Markdown document, with scope statements and constraints beside the content they qualify.

## Ownership

A topic directory is owned by the capability it names. Its `spec.md` owns observable product behavior; its `design.md` owns boundaries and invariants. Write each rule in its owning document and link from dependent topics.

Read applicable ancestor contracts before changing a nested topic.

## Context Layers

- **Product Vision** ([spec.md](spec.md)) → why the product exists.
- **Domain Language** ([CONTEXT.md](../CONTEXT.md)) → what each term is.
- **Spec Documents** (`spec.md` / `design.md`) → what the product does.
- **Code** → the implementation.
