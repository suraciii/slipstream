# Design Language Contract

This contract owns the translation from product meaning to presentation, the responsibilities of surfaces, and state ownership invariants. [Product Design Language](spec.md) owns observable shared behavior; [Visual System](visual-system.md) owns the Web reference scale. Feature contracts own exact behavior, and [Web Frontend Architecture](../web-application/design.md) owns code boundaries and dependency direction.

## Authority and Context

A feature MAY use a different carrier, scope, or lifecycle when its behavior contract defines that choice. It MUST preserve shared domain meaning, outcome distinctions, and safety invariants.

The design language does not require a component API, CSS framework, route, wire format, or database model. A component boundary is justified when it hides meaningful state or policy behind a narrow interface with one owner; a markup or event forwarding wrapper is not a design boundary.

## Semantic Model

```text
Photographer goal or Agent request
  → domain fact and requested effect
  → interaction class
  → surface responsibility
  → semantic presentation role
  → platform-specific rendering or output
```

The Web and CLI MAY use different carriers but MUST converge on the same fact, effect, scope, and outcome. A lower layer MUST NOT infer a higher-layer fact from appearance or transport completion.

| Layer          | Authority                                                                           |
| -------------- | ----------------------------------------------------------------------------------- |
| Domain         | [CONTEXT.md](../../CONTEXT.md), Product Vision, and feature contracts               |
| Intent         | [Product action classes](spec.md#core-action-classes)                               |
| Surface        | The responsibilities in this contract                                               |
| Semantic token | Presentation meanings and mapping rules in this contract                            |
| Carrier        | Feature and platform contracts for Web, assistive technology, CLI, and Agent output |

## Token Layers

Tokens name presentation meanings, not domain facts. Token storage and syntax are implementation choices.

### Reference tokens

The rendering boundary MUST own primitive palette, type, spacing, radius, border, elevation, and motion values. The [Visual System](visual-system.md) fixes the Web reference scale.

Feature code and specifications MUST select semantic roles rather than raw values when a role exists. Reference values MAY change for contrast, density, or platform constraints without changing meaning.

### Semantic tokens

Surfaces SHOULD directly choose semantic tokens, each with one distinguishable meaning. The semantic vocabulary covers:

| Group           | Roles                                                                                          |
| --------------- | ---------------------------------------------------------------------------------------------- |
| Structure       | Canvas, panel, raised surface, boundary, overlay, scrim                                        |
| Content         | Primary, supporting, muted, technical, inverse                                                 |
| Action          | Primary, secondary, focus, disabled, destructive consequence                                   |
| Domain state    | Unflagged, Picked, Rejected, selected, No Rating, unavailable                                  |
| Operation state | Pending, applied, partial, refused, failed, stale, changed elsewhere, unknown                  |
| Evidence        | Preview content, Preview unavailable, Original unavailable, retained content, technical detail |

These roles do not prescribe a particular color, icon, font, shape, or CSS variable. Rejected and destructive consequence MUST NOT share one semantic token.

### Surface tokens

A surface MAY define local aliases for semantic tokens to express its hierarchy. Such aliases MUST map to existing semantic meanings without introducing private synonyms or exposing raw reference tokens to feature behavior.

Aliases MAY change composition or density with available width but MUST NOT change meaning. They MUST remain local unless another surface can reuse the same semantic responsibility directly.

### Token invariants

- One token MUST have one meaning. Focus and success, or Picked and irreversible cleanup, MUST NOT share a token.
- Pending, stale, failed, unavailable, and unknown MUST NOT collapse into one generic muted treatment.
- Destructive consequence tokens MUST be reserved for effects that remove or permanently alter data. Reject is a reversible Selection State, not a destructive consequence.
- Token mappings MUST preserve readable contrast and visible focus. Themes and contrast modes MAY change values, not meanings.
- Reduced-motion token changes MUST NOT remove the only evidence of a transition.
- CLI and Agent carriers need not use visual tokens, but their labels and result classes MUST preserve semantic meaning.
- A repeated presentation label or primitive MUST be named once and reused. A surface MAY compose local structure without requiring a shared component library.

## Surface Responsibilities

Responsibilities are not required files, classes, or DOM components. Adjacent responsibilities MAY be combined for one coherent task and state owner, but MUST NOT be combined merely to share markup.

### Context shell

The context shell owns navigation affordances and cues for the current source, Photo, position, destination, and return path. It MUST NOT write Selection State, Rating, Album membership, or Original state.

It MUST NOT imply that a source or position is current before destination admission succeeds. It MUST NOT replace a feature recovery result with generic connection status.

### Content surface

The content surface presents Photo or Library content and qualifying evidence. It MUST preserve identity, aspect ratio, Preview Source, and the distinction between valid, retained, and unavailable content.

It MUST NOT infer Photo identity from a filename, substitute another Photo's Preview, or invent detail. Grid and Photo View MAY compose content differently but MUST expose the same underlying facts.

### Fact and state group

The fact and state group presents feature-owned facts and identifies current, pending, stale, unavailable, or unknown status when it affects a decision. It MUST NOT present speculative writes as committed or missing facts as authoritative defaults.

### Decision group

The decision group presents the current task's reversible actions and communicates scope, state, availability, and settled outcome. It emits semantic intent to the owning state boundary and MUST NOT become a durable Photo state owner merely by rendering controls.

Visible, keyboard, touch, and programmatic paths MUST converge on the same action class and persistence rules where supported.

### Supporting surface

A supporting responsibility groups secondary actions and MAY be persistent, transient, or modal. It MUST NOT create another navigation model, duplicate an action owner, hide the only path to a consequential operation, or claim that closing cancelled an admitted operation.

[Supporting surface behavior](spec.md#supporting-surfaces) owns its user-visible naming, focus, and modal rules.

### Outcome and recovery surface

This surface presents operation outcomes and available next steps, not the underlying operation. A sent request MUST NOT be represented as an applied result.

### Technical evidence disclosure

Technical evidence includes Preview Source, detail limits, revisions, paths, identifiers, and diagnostics. It MUST be available when inspection, recovery, or Agent coordination requires it, and remain secondary to the human task.

Disclosure MUST NOT change the fact it explains. An internal identifier MUST NOT replace a Photo name, and an implementation error string MUST NOT be the primary recovery instruction.

## State and Ownership Invariants

[Foundational Architecture](../design.md) and [Web Async Ownership](../web-application/async-ownership.md) own the shared state and asynchronous boundaries.

- Every mutable domain fact MUST have one authoritative owner. Presenters, tokens, gestures, and supporting surfaces MUST NOT create competing copies.
- Every admitted operation MUST have one settlement owner. Closing a surface, navigation, or layout change MUST NOT silently cancel it or claim success.
- Before admission an interaction MAY show intent or a candidate state. After admission it MUST show pending until its owner settles the operation.
- A stale result MUST NOT overwrite a newer result without explicit revalidation or user choice.
- Undo MUST be limited to the operation classes and scope defined by the owning feature. It MUST NOT imply that an unrelated or irreversible operation can be undone.

## Responsive Composition

Responsive changes MUST NOT create a second operation owner, history model, or device-only gesture. Focus order MUST follow the composed task; a moved control MUST remain discoverable by name and accessible state.

A Grid MAY change cell count or density while preserving cell identity, facts, and action semantics. Photo View MAY change chrome while preserving Preview evidence and review continuity.

[Responsive behavior](spec.md#responsive-language) owns the shared presentation priorities. Feature contracts own viewport thresholds, cell geometry, target sizes, and feature-specific layouts; the [Visual System](visual-system.md) composes within them.

## Accessibility and Carrier Invariants

A CLI or Agent result MUST name the operation, scope, settled outcome, and next action without requiring browser presentation state. [Accessibility Language](spec.md#accessibility-language) owns shared visible and assistive behavior.

Photo Development revision restore MUST NOT be presented as session Undo or Redo, and session Undo and Redo MUST NOT be described as history restore. [Autosave and Reversible Editing](../photo-development/spec.md#autosave-and-reversible-editing) owns their distinct state and write semantics.
