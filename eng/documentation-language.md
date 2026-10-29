# Documentation Language

These rules govern project documentation, Issue titles and bodies, pull request titles and descriptions, and commit messages. They apply to submissions through the GitHub UI, CLI, API, and automation.

## Language

- Write project artifacts in English. Conversation with contributors may use their preferred language.
- Use short sentences, active voice, American spelling, and stable terms.
- Use ASD-STE100 writing rules as a target; do not claim formal compliance.
- Use the domain terms in [CONTEXT.md](../CONTEXT.md).
- Preserve the exact spelling of domain terms, identifiers, filenames, field names, commands, serialized values, code symbols, user-provided names, and quoted diagnostics.
- Explain non-English evidence in English when needed.
- In `specs/`, use uppercase `MUST`, `MUST NOT`, `MAY`, `SHOULD`, and `SHOULD NOT` for requirements, prohibitions, options, and recommendations.
- Use lowercase `must`, `may`, and `should` in `docs/` and `eng/`.
- State one rule per sentence.
- Write requirements whose compliance can be checked. Delete generic motivation, repetition, and implementation tours.

## Structure

- State each normative rule once in the form that makes it clearest. Use numbered steps for a linear procedure.
- Define a term once and link to it.
- Product specifications state user-visible behavior. Design specifications state boundaries, invariants, and the trade-offs they protect.
- Describe the target state. Record current implementation gaps in the governing Issue or pull request instead of weakening the target contract.
- Explain product and domain concepts before implementation details. Do not turn classes, methods, call chains, or storage steps into prose.

## Examples and Diagrams

- Commands and executable examples must run or parse as written. Keep only examples that remove ambiguity.
- Draw a diagram only when it makes a structure or behavior clearer.
- Store diagrams as rendered text. Use adjacent prose only for constraints the diagram does not express.
- Do not use raw HTML. Use Markdown, short prose, or one concrete example.
