# Design Language Contract

This document defines the technical boundary for Slipstream's product design language. It turns the user-facing rules in [Product Design Language](spec.md) into a small model of semantic layers, surface responsibilities, state transitions, and conformance checks.

It does not define a component API, CSS framework, UI library, route, wire format, or database model. [Web Frontend Architecture](../web-application/design.md) owns source structure and Web module boundaries. Feature contracts own exact behavior. [`CONTEXT.md`](../../CONTEXT.md) owns domain vocabulary.

## Problem and Drivers

A design language is useful only if it prevents the same product fact from acquiring different meanings in different surfaces. Slipstream needs a boundary that:

- keeps the Preview and the Photographer's visual judgment primary;
- gives Pick, Reject, Rating, Album membership, and Select distinct meanings;
- presents the same Photo identity, facts, and outcomes to the Web, CLI, and an external Agent;
- preserves Original safety and truthful pending, partial, failed, and unknown outcomes;
- supports a large Library without making every operation wait for unrelated work;
- works across pointer, touch, keyboard, assistive technology, and programmatic use; and
- remains small enough that a feature does not need a new design subsystem to add one coherent surface.

The design language is not a collection of visual preferences. It is a constraint on how product meaning crosses from domain facts to a human or machine surface.

## Authority and Context

The two design-language documents have separate responsibilities:

- [Product Design Language](spec.md) owns product character, hierarchy, interaction grammar, content language, feedback and recovery language, responsive principles, accessibility language, and shared review questions.
- This document owns the semantic-layer model, surface responsibility boundaries, state invariants, and implementation-independent conformance method.
- A Feature behavior contract owns exact user-visible behavior, bounds, ordering, persistence, failure, recovery, and feature-specific exceptions. It must link to the shared design language rather than restate it.
- A Feature design contract owns technical boundaries, lifecycle, identity, and ownership for that Feature. It must not redefine the product's domain vocabulary.
- [`CONTEXT.md`](../../CONTEXT.md) defines terms. It must not become a second source for behavior or visual rules.
- [Web Frontend Architecture](../web-application/design.md) owns Web code ownership and dependency direction. It may realize these responsibilities with different modules as the implementation evolves.
- `docs/` explains how to use and install the product. It is not a design authority.

When a feature needs a behavior that is not covered here, its behavior contract is the acceptance authority for that behavior. The feature must preserve the shared meanings, outcome distinctions, and safety invariants unless the governing Issue records an explicit product decision to change them.

The Product Design Language's shared rule is a presentation and meaning constraint, not a replacement for a feature's interaction mechanics. A feature contract may define a different carrier, scope, or lifecycle when that is part of its product behavior; the feature must link the exception and preserve the shared domain meaning.

## Selected Design

Two reasonable alternatives were considered:

- **Rules in each Feature contract:** each Feature would define its own hierarchy, tokens, states, and feedback. This keeps each document locally complete, but duplicates cross-feature rules and lets Pick, unavailable, pending, or unknown acquire different meanings.
- **A component catalog first:** the project would begin with reusable buttons, cards, dialogs, and CSS values. This may improve mechanical reuse, but it chooses implementation shapes before the product's visual and interaction meaning. It also encourages one-off composition to look consistent while behaving differently.
- **A product language plus a small technical contract:** the product document defines meaning and the technical document defines the translation boundary. Feature contracts keep exact behavior, while implementation remains free to choose the smallest appropriate structure.

Slipstream uses the third option. It minimizes duplicated authority without committing the product to a component framework. A new implementation abstraction is justified only when it hides a current responsibility or preserves a current invariant.

## Semantic Model

The design language translates product intent through stable layers:

```text diagram
Photographer goal or Agent request
  → domain fact and requested effect
  → interaction class
  → surface responsibility
  → semantic presentation role
  → platform-specific rendering or output
```

The Web and CLI may use different carriers at the final step. They must converge before that step on the same domain fact, effect, scope, and outcome. A browser button, keyboard shortcut, touch accelerator, CLI command, and Agent operation are different ways to express an intent; they are not different product meanings.

The layers have these boundaries:

- **Domain layer:** Photo identity, Original File, Album, Selection State, Rating, Preview Source, and operation outcome. The domain layer is defined by the Product Vision, feature contracts, and `CONTEXT.md`.
- **Intent layer:** read or navigate, direct reversible decision, staged view choice, explicit recovery, or irreversible consequence. The Product Design Language owns these action classes.
- **Surface layer:** context shell, content surface, fact and state group, decision group, supporting surface, outcome and recovery surface, or technical evidence disclosure. This contract owns their responsibilities.
- **Semantic token layer:** stable meanings such as primary content, supporting content, focus, pending, unavailable, rejected, or destructive consequence. This contract owns the mapping rules.
- **Carrier layer:** Web structure and style, touch and keyboard affordances, assistive-technology semantics, CLI output, or Agent-readable result. Feature and platform contracts own the carrier details.

A lower layer must not infer a higher-layer fact from appearance. For example, a red accent does not mean that a Photo is Rejected, and a completed HTTP request does not mean that a decision was applied.

## Token Layers

Tokens are names for presentation meaning, not a second domain model. The exact storage and syntax of tokens are implementation choices. The conceptual layers are stable.

### Reference tokens

Reference tokens are implementation primitives: palette values, type sizes, spacing steps, radii, borders, elevation, and motion durations. They are useful for a rendering system but have no product meaning by themselves.

Reference tokens must be owned by the rendering boundary. Feature code and feature specifications must not select a raw color, arbitrary spacing value, or animation duration when a semantic role exists. A reference scale may change to satisfy contrast, density, or platform constraints without changing product meaning.

### Semantic tokens

Semantic tokens name a product-facing presentation role. They are the only tokens that a surface should choose directly. A semantic token must have one meaning and must remain distinguishable from nearby meanings.

The initial semantic vocabulary includes these groups:

- **Structure:** canvas, panel, raised surface, boundary, overlay, and scrim.
- **Content:** primary content, supporting content, muted content, technical content, and inverse content.
- **Action:** primary action, secondary action, focus indication, disabled action, and destructive consequence.
- **Domain state:** Unflagged, Picked, Rejected, selected, No Rating, and unavailable.
- **Operation state:** pending, applied, partial, refused, failed, stale, changed elsewhere, and unknown.
- **Evidence:** Preview content, Preview unavailable, Original unavailable, retained content, and technical detail.

These names describe roles. They do not require a particular color, icon, font, shape, or CSS variable. `Rejected` is a Selection State; `destructive consequence` is an effect class. They must not be represented by one shared semantic token merely because both may use a warning color in one theme.

### Surface tokens

A surface may define local aliases for semantic tokens when that makes its hierarchy explicit. A Photo View may have a Preview role, a current-facts role, and a decision-action role. A Grid may have a cell-content role and a cell-state role.

Surface tokens must satisfy all of these rules:

- They map to existing semantic meanings rather than inventing a private synonym.
- They do not expose a raw reference token to feature behavior.
- They do not make a secondary action look like an irreversible action or make a pending result look applied.
- They may change composition or density at an available width, but they do not change the underlying meaning.
- They remain local to the surface unless another surface has the same responsibility and can reuse the semantic role directly.

### Token invariants

- One token has one meaning. A token must not mean both focus and success, or both Picked and permanent deletion.
- Every state that affects a decision has a non-color channel: visible text, icon, geometry, position, or an exposed accessible state.
- `pending`, `stale`, `failed`, `unavailable`, and `unknown` must not collapse into one generic muted treatment.
- Destructive consequence tokens are reserved for effects that can remove or permanently alter data. Reject is a reversible Selection State and is not itself a destructive consequence.
- Token mapping must preserve readable contrast and visible focus. A theme or contrast mode may alter values, not meanings.
- Motion tokens may shorten or disappear under reduced motion. They must not be the only evidence of a state transition.
- CLI and Agent surfaces do not need visual tokens, but their labels and result classes must preserve the corresponding semantic meanings.

## Surface Responsibilities

These are product responsibilities, not a requirement for one file, class, or DOM component per item. A surface may combine adjacent responsibilities when doing so keeps one coherent task and one state owner. It must not combine responsibilities merely to share markup.

An implementation component may realize one or more adjacent responsibilities. A component boundary is justified only when it hides meaningful state or policy behind a narrow interface and has one owner. A wrapper that only forwards markup or events is not a design boundary. Components must follow these responsibilities without creating a second domain-state owner.

### Context shell

The context shell establishes where the Photographer is: current source, current Photo, position, destination, and valid return path. It owns navigation affordances and continuity cues.

It must not:

- write Selection State, Rating, Album membership, or Original state;
- imply that a source or position is current before destination admission succeeds; or
- replace a feature's recovery result with generic connection status.

### Content surface

The content surface presents the Photo or Library content and the evidence that qualifies it. It preserves identity, aspect ratio, Preview Source boundaries, and the distinction between valid content, retained content, and unavailable content.

It must not infer a Photo identity from a filename, replace one Photo's Preview with another Photo's Preview, or invent detail that the evidence does not contain. Grid and Photo View may compose content differently, but they must expose the same underlying facts.

### Fact and state group

The fact and state group presents current position, Selection State, Rating, availability, Capture Time, Album membership, or other facts owned by a feature contract. It identifies whether a fact is current, pending, stale, unavailable, or unknown when that distinction affects a decision.

It must not display a speculative write as a committed fact. It must not turn a missing fact into a default value that looks authoritative.

### Decision group

The decision group presents the primary reversible actions for the current task: Pick, Reject, Rating, Select where the task is multi-selection, and eligible Undo. It communicates scope, current state, availability, and the settled outcome.

It emits semantic intent to the owning state boundary. It does not own durable Photo state merely because it renders the control. Visible, keyboard, touch, and programmatic paths must converge on the same action class and persistence rules.

### Supporting surface

A supporting responsibility groups secondary actions such as Sources, View options, Rating choices, Photo details, Album membership, tools, or recovery review. It may be persistent alongside content or presented as a transient or modal surface. In either mode it has one coherent purpose and preserves an already admitted operation's settlement rules.

When presented modally, it has a named entry, moves focus into the surface, prevents background interaction while active, provides Close or Cancel as appropriate, and returns focus to the invoker or nearest valid control. A persistent supporting region follows the owning feature's navigation and focus contract; it is not required to trap focus or block content interaction.

It must not create a second navigation model, duplicate an action owner, hide the only path to a consequential operation, or claim that closing the surface cancelled an admitted operation.

### Outcome and recovery surface

An outcome and recovery surface explains pending, applied, partial, refused, failed, stale, changed-elsewhere, unavailable, and unknown states. It names scope and offers the valid next step when one exists.

It owns presentation of the outcome, not the underlying operation. A request being sent is not an applied result. A transient notice may supplement a consequential outcome, but it must not be the only durable record of a partial, uncertain, or recoverable effect.

### Technical evidence disclosure

Technical evidence includes Preview Source, detail limits, revisions, paths, identifiers, and diagnostic detail. It is available when inspection, recovery, or Agent coordination requires it, but it remains secondary to the human task language.

Technical disclosure must not change the domain fact it explains. It must not expose an internal identifier as a replacement for a Photo name or use an implementation error string as the primary recovery instruction.

## State and Ownership Invariants

The design language assumes the ownership model in [Foundational Architecture](../design.md) and [Web Async Ownership](../web-application/async-ownership.md). These invariants apply regardless of the eventual UI structure:

- Every mutable domain fact has one authoritative owner. A presenter, token, gesture, or supporting surface may reflect it but must not create a competing copy.
- Every admitted operation has one settlement owner. Closing a surface, navigating away, or changing layout must not silently cancel it or claim success.
- An action must meet known client-side readiness requirements before activation when those requirements affect the outcome. Server admission remains authoritative and may refuse, conflict, or report uncertainty; the surface must expose that outcome.
- Before admission, an interaction may show intent or a candidate state. After admission, it must show pending until the owning operation settles.
- A settled outcome must identify the actual effect and scope. `Applied`, `Partial`, `Refused`, `Failed`, and `Unknown` are different outcomes.
- A stale result must not overwrite a newer result without explicit revalidation or an explicit user choice.
- Failure recovery repeats the failed or explicitly revalidated operation only. It does not replay unrelated successful work.
- The last trustworthy content and unrelated facts remain available when the feature contract permits them. A local failure must not erase them to produce an empty success-looking view.
- Undo is available only for the operation classes and scope that the owning feature contract defines. It must not imply that an unrelated or irreversible operation can be undone.
- A destructive effect requires exact identity and scope, separate confirmation, settlement reporting, and a valid inspection or recovery path.

## Interaction and Presentation Mapping

A feature review should map each user-facing operation through this sequence:

```text literal
identify the domain fact and requested effect
name the interaction class
choose the surface responsibility
choose semantic state and action roles
provide visible, accessible, and programmatic paths
show pending and settled outcomes
check recovery, continuity, and responsive composition
```

The mapping must be concrete. For example:

### Pick

- Domain fact: the current Photo's Selection State changes to Picked.
- Interaction class: direct reversible decision.
- Surface: decision group, with the current Selection State in the fact group.
- Presentation: Picked and pending have distinct state roles; the control does not report Picked before persistence confirmation. The [Library Browser Selection State contract](../library-browser/spec.md#selection-state) owns the exact write and confirmation behavior.
- Alternatives: the visible action, its keyboard path, its touch accelerator, and the Agent/CLI operation use the same effect and settlement meaning. Their affordances and availability may differ by surface.
- Recovery: a refused, failed, or unknown write retains the prior trustworthy state and exposes the owning [Selection State recovery rules](../library-browser/spec.md#selection-state).

Pick is not Select mode, a generic checkmark, a Rating, or a destructive deletion action.

### Original unavailable

- Domain fact: the Photo remains, but its Original File cannot be read at the remembered Original Location. [Photo Library availability](../photo-library/spec.md#photos) and [Manual Recovery](../photo-library/spec.md#manual-recovery) own the fact and recovery behavior.
- Interaction class: read-state with explicit recovery.
- Surface: content and fact groups show the Photo identity and `Original unavailable`; the recovery entry belongs to a supporting or outcome surface.
- Presentation: the unavailable fact is not represented as a Removed Photo, Photo deletion, or valid Preview. The state has text and an accessible description in addition to any color or icon.
- Recovery: the recovery surface names the affected identity, explains the evidence and scope, and uses the owning Location Recovery contract for any explicit apply step.

`Original unavailable` is not a new Photo state model. It is a presentation of the existing Photo and Original facts.

### Cross-surface examples

- A CLI query may expose a filter or complete Album reorder without adding a matching Web control. Its help and reference disclose the capability, and its result uses the same Photo identity, effect, scope, and outcome language as the Web. See [Finding Photos](../command-line/spec.md#finding-photos), [Managing Albums](../command-line/spec.md#managing-albums), and [Moving Between CLI and Web](../command-line/spec.md#moving-between-cli-and-web).
- Photo Development editing uses tool-owned state, guarded autosave, and tool-owned Undo/Redo. The design language requires visible current, stale, conflict, and failed distinctions, but does not reuse the Library Browser's one-level Undo contract. See [Autosave and Reversible Editing](../photo-development/spec.md#autosave-and-reversible-editing) and [Failure and Retention](../photo-development/spec.md#failure-and-retention).
- Access submission may be refused, and confirmed session loss removes private views. Temporary network loss may retain an authorized view. The design language treats these as distinct outcomes and leaves the exact transition to [Browser Entry](../access/spec.md#browser-entry) and [Session Loss and Disconnection](../access/spec.md#session-loss-and-disconnection).

## Responsive Composition

Responsive composition is a carrier decision over the same semantic model:

- Available width and height determine composition. Device labels and user-agent branches do not define product meaning.
- The priority order is content and evidence, current facts and state, primary decision, navigation and supporting actions, then technical detail and recovery detail. A feature may alter the order only when its contract states why the task requires it.
- A narrow surface may collapse or move supporting actions, but it must preserve the current source or destination, current Photo where applicable, primary decision, navigation, and status.
- A short viewport yields secondary content before shrinking primary targets, obscuring state, or introducing horizontal overflow.
- A Grid may change cell count or density while preserving cell identity, facts, and action semantics. A Photo View may change chrome while preserving Preview evidence and review continuity.
- Focus order follows the composed task, not the desktop coordinate order. A moved control remains discoverable by its name and accessible state.
- Responsive changes must not create a second operation owner, a second history model, or a device-only gesture.

Exact viewport thresholds, cell geometry, target sizes, and feature-specific layouts remain in the owning behavior or experience contract.

## Accessibility and Carrier Invariants

A carrier conforms when all applicable statements are true:

- It exposes one clear primary view and semantic landmarks for that view.
- Every action has a stable name, an exposed state where applicable, and a non-gesture path.
- Focus is visible, enters a supporting surface, stays contained while required, and returns to the invoker or nearest valid control.
- State, selection, error, progress, and outcome are not communicated by color alone.
- Text and controls remain readable at the applicable contrast level and under text enlargement or reflow.
- Keyboard, pointer, touch, and assistive-technology users can perform the same product effect without relying on timing, hover, precision, or a hidden shortcut.
- Reduced motion removes nonessential movement without removing state or outcome information.
- Truncation changes visual layout only; the complete relevant name, position, state, and result remain available to assistive technology or an explicit detail path.
- A CLI or Agent result names the operation, scope, settled outcome, and next action without requiring browser presentation state.

## Design Review Evidence

A feature review must record enough evidence to answer these questions:

1. What Photographer task or Agent request does this surface serve?
2. Which domain facts and effects are visible, and which authority owns them?
3. Which interaction class applies, and what is the exact scope?
4. Which surface responsibilities and semantic token roles are used?
5. How do visible, keyboard, touch, assistive, CLI, and Agent paths converge?
6. How are pending, stale, unavailable, failed, partial, refused, and unknown outcomes shown?
7. What survives a local failure, close, navigation, reload, or concurrent change?
8. How does the composition behave at narrow width, short height, text enlargement, reduced motion, and keyboard focus?
9. Which exact feature contract accepts the behavior, and are any rules duplicated instead of linked?

A screenshot or passing test alone is not conformance evidence. Evidence must show the semantic mapping and the outcome boundary. A browser check is useful for Web composition; it cannot prove Native Processing, iOS/Safari, physical safe-area behavior, or a CLI result unless those surfaces were exercised.

## Non-Goals

This contract does not:

- require a component library, design-tool file, CSS naming convention, or TypeScript interface;
- prescribe exact colors, fonts, breakpoints, radii, shadows, animation durations, or icon package;
- move feature behavior out of its owning contract;
- make the Web and CLI use the same layout or carrier;
- add a design token service, theme configuration, or runtime styling dependency; or
- turn design review evidence into a substitute for focused behavior tests and the full repository gate.

The smallest implementation that preserves the semantic layers and invariants is the preferred implementation.
