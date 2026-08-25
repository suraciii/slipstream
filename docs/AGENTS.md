# Agents: Writing Product Specs

`docs/` defines what Slipstream must satisfy. Write for Photographers and implementers who do not read source code.

## Rules

- Write the spec before implementation. The body defines the target product; implementation follows the spec.
- Lead with the Photographer's problem and the constraint or trade-off that requires a product rule.
- Explain the product in product and domain language. Do not translate classes, handlers, storage steps, or source call chains into prose.
- Define each rule once. Other documents link to it instead of copying it.
- State observable behavior, boundaries, ordering, invalid states, and failure outcomes explicitly.
- Keep terms consistent with [`../CONTEXT.md`](../CONTEXT.md).
- Keep temporary implementation state, experiments, measurements, and known divergence out of Product Specs. Record them in the governing Issue or change review.
- Write active prose in English. Use short sentences, active voice, American spelling, and stable terms.
- Use `must`, `may`, and `must not` for requirements, options, and prohibitions. Preserve exact product names and identifiers.
- Give each section one purpose. Prefer a short list to a dense paragraph. Do not use tables.
- Use `text diagram` for ASCII diagrams and `text literal` for other preformatted text. Do not use bare `text` fences.
- Do not use raw HTML. Keep examples minimal and independently verifiable when tooling exists.
- Delete empty sections. Structure follows the behavior, not a fixed document shape.

## Minimal Shape

```markdown
# Capability Name

The Photographer's problem and why a product rule is necessary.

## Behavior

Observable rules and boundaries.

## Failure Behavior

Errors, invalid states, and recovery visible to the Photographer.

## Examples

Inputs and expected outcomes that resolve ambiguity.
```
