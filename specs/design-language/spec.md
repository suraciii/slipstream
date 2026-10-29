# Product Design Language

Slipstream is a photo selection and organization workspace for a Photographer and the Photographer's own Agent. This document defines the product design language: how Slipstream should look, feel, speak, and behave as one product across human and programmatic use.

[Product Vision](../spec.md) defines why Slipstream exists. [CONTEXT.md](../../CONTEXT.md) defines domain language. Feature specifications define exact behavior and outcomes. This document defines the shared design direction without replacing those contracts.

## Product Reading

### The product's job

Slipstream helps a Photographer find, compare, organize, and decide what to keep in a large Photo Library while preserving trustworthy evidence and Original safety. Its central loop is:

```text diagram
See the Photo and its evidence
  → make a deliberate decision
  → continue without losing place
  → inspect the result or recover uncertainty
```

The interface is a quiet instrument around the photograph. The Photo is the subject; the interface makes evidence, decisions, and consequences understandable.

### The people and systems involved

- The Photographer judges the image and sets the purpose.
- Slipstream presents current facts, enforces preconditions, preserves identity, and reports effects.
- The external Agent interprets a request, combines bounded capabilities, and reports its work.

The Photographer and Agent use different surfaces, but they must share Photo identity, Album semantics, Selection State, Rating, Original safety, and truthful outcomes. The Web must not become a visual-only authority, and the CLI must not become a second product model.

### Design tensions

The design language resolves these tensions without creating extra product concepts:

- **Rich visual evidence vs. bounded work:** show useful Photos early; do not require a whole Library transfer or complete Preview generation.
- **Direct decisions vs. Original safety:** keep review actions close to the Photo; keep Originals read-only and separately review irreversible record cleanup.
- **Large Library density vs. attention:** keep information compact and stable; reserve emphasis for the current Photo, current state, and next meaningful action.
- **Gesture speed vs. discoverability:** gestures accelerate visible controls; they never replace visible, keyboard, and programmatic paths.
- **Technical truth vs. calm presentation:** keep revision, transport, and implementation details available when useful, but do not make them the primary language of a Photographer's task.

## Design Thesis

**Calm, direct, inspectable review.**

Slipstream must feel quiet enough for visual judgment, direct enough for repeated decisions, and explicit enough that a Photographer or Agent can tell what happened. Every visual emphasis, interaction, and message must support one of four purposes:

1. show evidence;
2. enable a decision;
3. preserve continuity;
4. explain an outcome or recovery path.

Decoration, novelty, and hidden automation do not justify a new visual or interaction pattern.

## Design Principles

### Photograph first

The Preview and its surrounding facts must receive the strongest visual priority. Controls must frame the Photo rather than compete with it. A Photo must not be treated as a decorative tile behind unrelated controls.

### Truth before reassurance

Slipstream must show what it knows, what it is doing, and what remains uncertain. It must not use a cheerful success treatment to hide a refused, partial, stale, failed, or unknown outcome. It must not invent counts, progress, Preview quality, or completion.

### Decisions stay close

Pick, Reject, Rating, navigation, and Undo must be reachable from the current review context. A Photographer must not leave Photo View for ordinary review decisions. A supporting surface may contain secondary actions, but it must have a clear name, entry, and return path.

### Continuity is a feature

Source, order, current Photo, position, Selection State, Rating, and saved Album position must remain understandable as the Photographer moves. Opening a supporting surface, changing layout, or recovering from a failure must not silently discard the current task or context.

### Progressive disclosure, not hidden capability

Primary actions remain visible. Secondary actions may be grouped in a named supporting surface. Technical evidence may be disclosed when it helps inspection or recovery. A feature must not hide a necessary action behind an unexplained icon, gesture, hover state, or transient message.

### Explicit effects

An action must make its effect and scope clear before it is committed. Reversible decisions may be direct when their owner and outcome are visible. Grouped view changes may be staged and applied. Trash Record Cleanup must clearly name the irreversible loss of selected application records and require review and separate confirmation; it never deletes files. Unknown completion must remain unknown until reconciled.

### One set of facts

A visual state, accessible name, CLI result, and Agent-readable result must describe the same domain fact. Presentation may differ by surface; meaning, identity, and outcome must not.

### Bounded attention

A large Library must not make the Photographer wait for irrelevant work. Loading, retention, animation, and disclosure must stay bounded. The interface must keep the current range, current Photo, and current action understandable while background work continues.

### Quiet confidence

Slipstream should use restraint instead of theatrical branding. Photographic character comes from the image, tonal hierarchy, and precise language—not from film decoration, dashboard ornament, or promotional cards.

## Visual Language

### Composition and hierarchy

Every surface must establish this order unless a feature contract has a stronger reason:

```text literal
Photo or current content
  current facts and state
    primary action
      navigation and supporting actions
        technical evidence and recovery detail
```

Grid View is a field of Photos with stable cell geometry. Photo View is a focused inspection surface with the Preview, current facts, primary decisions, navigation, and supporting surfaces. Sources, options, Rating choices, Photo tools, and recovery review are supporting surfaces; they are not additional product pages by default.

### Tone and surfaces

- The default visual tone is neutral, dark, and low-glare so the Preview remains the strongest visual object.
- Surfaces must be separated primarily by tonal steps, spacing, and hierarchy; borders and shadows are supporting signals, not decoration.
- An accent color may identify focus, current selection, or an available primary action. It must not carry several conflicting meanings at once.
- Success, warning, failure, unavailable, Picked, Rejected, and Rating states must have distinct semantic roles. A destructive color belongs to destructive consequence, not to the ordinary Reject decision.
- Gradients, ornamental texture, animated backgrounds, and persistent brand rows must not compete with the review task.
- A Photo View may use a dimmed, blurred copy of the same Preview as a scrim inside the Preview well. The scrim must not add detail, imply a second Photo, appear in Grid, or remain when the Preview is unavailable. The sharp, complete Preview remains the content and the strongest visual object.
- A state must never depend on color alone. Use text, iconography, geometry, position, or an accessible state description as a second channel.

### Typography and numbers

- Use sentence case for labels, headings, buttons, and messages.
- Use the product's exact domain names: **Pick**, **Reject**, **Picked**, **Rejected**, **Unflagged**, **Select**, **Selection State**, **Rating**, **No Rating**, **Library Check**, **Capture Time**, **Original File**, **Original Location**, and **Album**.
- Use a stable, readable sans-serif voice for ordinary UI. Use a distinct numeric treatment for counts, positions, Capture Time, and other values that users compare.
- A single display serif may distinguish the wordmark, source titles, and supporting-surface titles. The display serif must not carry a control, state label, action, or message. Mono may carry filenames, numeric values, and technical facts; ordinary state and field labels, actions, and messages use the UI sans. Incidental technical micro-labels may use tracking but remain sentence case. [Visual System](visual-system.md#type) owns the role-specific reference sizes.
- Keep technical identifiers, revisions, and paths in a secondary or technical disclosure unless they are the object of the current task.
- Do not use typography alone to imply that an action succeeded. Outcome text must name the outcome.

### Icons and imagery

- Use familiar, simple icons that reinforce a visible label or state. An icon must not be the only explanation of a consequential action.
- Keep one icon meaning stable across the product. Do not use the same symbol for Pick, Select, and a generic checkmark if their effects differ.
- Preserve the complete composition, aspect ratio, and stated Preview Source. Grid composition must not crop or stretch a Photo. Display zoom may magnify the Preview only when it reports its actual zoom and does not imply additional source detail. See [Grid Composition and Orientation](../library-browser/spec.md#grid-composition-and-orientation) and [Preview Zoom and Fit](../library-browser/spec.md#preview-zoom-and-fit).
- Never substitute a sibling JPEG for a RAW Photo's Preview. Preview derivatives must not upscale a smaller source for storage; [Preview Normalization](../photo-library/previews/spec.md#preview-normalization) owns derivative dimensions and magnification evidence.
- Placeholder imagery must state the unavailable or failed fact without resembling a valid Photo.

### Density, shape, and rhythm

- Prefer a compact, stable rhythm that supports repeated review without visual fatigue.
- Keep Grid cells uniform and stable under bounded loading. Loading must not cause placeholder churn, unpredictable jumps, or changing cell geometry.
- Use a small family of surface shapes. A control, supporting surface, and destructive confirmation may be visually distinct, but the product must not accumulate one-off card treatments.
- Use whitespace to group facts and actions, not to create decorative emptiness around every control.
- Primary touch targets must remain comfortable and separated. A compact layout may reduce secondary controls before reducing the target or legibility of a primary action.

### State visibility

Every interactive object must have a distinguishable presentation for the states that apply to it:

- resting;
- hovered or pointed, when supported;
- keyboard-focused;
- pressed or committed;
- current or selected;
- disabled or unavailable;
- pending;
- stale or out of date;
- failed or refused; and
- unknown or awaiting reconciliation.

A pending state must not look like a committed state. A stale result must not look current. An unavailable Original must not look like a Removed Photo.

A resting state is not a disabled or unavailable state. A control that is merely waiting for the Photographer must keep an ordinary enabled presentation, its ordinary label weight, and its accessible state; dimming, muting, or hiding it to suggest unavailability is a false statement about a control that is ready. A resting control must meet the same contrast requirement as ordinary text, so a quiet control never becomes an unreadable one.

### Motion

Motion must explain continuity, spatial relationship, or state transition. It must be short, interruptible, and unnecessary for understanding the outcome. Loading motion must not imply progress that the product cannot measure. Reduced-motion preferences must remove nonessential movement without removing state or feedback.

## Interaction Language

### Core action classes

Every interaction belongs to one of these classes:

- **Read or navigate:** normally changes what is viewed without changing Photo or Library facts. A feature contract may define a continuity effect, such as saved Album position; navigation must not silently change Selection State, Rating, Album membership, or destructive state. See [Saved Position](../library-browser/spec.md#saved-position).
- **Direct reversible decision:** records a bounded Pick, Reject, Rating, or equivalent effect and reports truthful feedback while the effect is pending and after it settles.
- **Staged view choice:** changes filters, order, or presentation after an explicit Apply or equivalent commit; Cancel or Close discards the draft.
- **Explicit recovery:** reviews an uncertain or unavailable state. A bounded read or retry may repeat that operation directly; a consequential recovery shows proposed effects and requires explicit Apply.
- **Irreversible consequence:** identifies the exact scope and consequence, requires separate confirmation, and reports the final effect.

A feature must not silently change action class. Navigation must not silently record a decision; feature-defined continuity effects remain with their owning contract. A review shortcut must not become an irreversible operation.

### Visible controls and accelerators

- A gesture or keyboard shortcut may accelerate a visible capability; it must not create a hidden Web capability. Visible controls or equivalent accessible paths remain available.
- Agent and CLI capabilities may use different affordances or support capabilities without a Web counterpart. They must be discoverable through [machine-facing help and contracts](../command-line/spec.md#starting-and-discovering-capabilities) and preserve shared domain identity, effect, scope, and outcome.
- For Library Browser, [touch gestures](../library-browser/spec.md#touch-gestures) and [visible controls](../library-browser/experience/spec.md#photo-view) remain equivalent paths for feature-defined decisions and navigation. Other surfaces may define different accelerators.
- Undo must identify its operation scope and remain understandable as the inverse of an eligible operation. Library Browser one-level Undo follows [Library Browser Undo](../library-browser/spec.md#undo); Photo Development Undo/Redo follows [Photo Development history](../photo-development/spec.md#autosave-and-reversible-editing).
- A control must communicate its current client-known state and availability before activation when that state affects the outcome. Authoritative admission may still refuse or conflict, and the surface must report that outcome.

### Supporting surfaces

A supporting responsibility may be persistent alongside content or presented as a transient or modal surface. In either mode it must have a clear name and invoker, one coherent purpose, and preserve an already admitted operation's settlement rules.

When a supporting surface is modal, it must move focus into the surface, prevent background interaction while active, provide Close or Cancel as appropriate, and return focus to the invoker or nearest valid control. A persistent supporting region, such as wide-screen Sources, remains usable alongside content and follows [Library Browser screen structure](../library-browser/experience/spec.md#screen-structure) rather than modal focus trapping.

At most one modal supporting surface should be active. Moving between related details should replace the surface content rather than stack an unexplained modal tower. Exact modal behavior remains in [Supporting Surfaces](../library-browser/experience/spec.md#supporting-surfaces).

### Continuity and destinations

A destination must be addressable when it represents a meaningful source, order, filter, or current Photo. Browser history must represent destinations, not every temporary panel. Temporary surfaces must not create history entries. Returning from Photo View must return to the meaningful source context. Exact URL and history behavior belongs to [Destinations and Browser History](../library-browser/experience/spec.md#destinations-and-browser-history).

Changing a source, order, or filter must state whether it preserves the current Photo, resets to a source default, or requires a new destination. The interface must not silently reinterpret a stale destination against new Library facts.

## Content Language

### Voice

- Write for a Photographer first and an implementer second.
- Use short, active, concrete sentences.
- Name the fact, effect, or next action before the implementation detail.
- Explain what the Photographer can do next; do not only report that something failed.
- Use the same domain terms and distinctions in visible labels, accessible names, CLI result descriptions, and specifications. Carrier-specific enum spellings remain in the CLI Reference.

### Stable vocabulary

The following distinctions are part of the design language:

- **Pick** is the keep decision; **Picked** is its state.
- **Reject** is the reject decision; **Rejected** is its state.
- **Select** chooses Photos, sources, or a multi-selection; it is not the keep decision.
- **No Rating** describes zero Rating in facts and accessible names; a compact control may show `0★`.
- **Original unavailable** describes an unavailable Original File; **Photo unavailable** is not the cause of that state.
- **Library Check** is the product operation; do not use scan or rescan in user-facing copy.
- **Capture Time** is the camera-recorded ordering fact; do not call it file time or Copy Time.
- **Review unavailable Originals** is the recovery entry; do not imply that the Photo was deleted.

The design language prescribes how these terms are presented. [`CONTEXT.md`](../../CONTEXT.md) and the owning feature contracts remain authoritative for their definitions and behavior. This document must not add or redefine domain terms.

### Labels and messages

- Action labels use verbs: Pick, Reject, Apply, Retry, Restore, Clean up Trash records.
- State labels use stable nouns or adjectives: Picked, Rejected, Unflagged, Pending, Failed, No Rating, and the specific unavailable fact, such as Original unavailable or Preview unavailable.
- Messages combine condition, effect, and next action when a next action exists: "The Original is unavailable. Review unavailable Originals."
- Do not use generic labels such as "Done" when the product can name the actual effect.
- Do not expose internal error names, request IDs, storage paths, or implementation labels in the primary message unless the user is inspecting technical details.
- Counts must identify what they count and use the product's established pluralization. Progress must use a known total; otherwise name the phase without inventing a number.

## Feedback and Recovery

### Feedback is a fact

Feedback must answer the smallest useful set of questions:

1. What operation or state is this?
2. Is it pending, applied, refused, changed elsewhere, failed, unavailable, or unknown?
3. What scope does it affect?
4. What can the Photographer or Agent do next?

The product may use compact feedback for routine success and persistent feedback for consequential, partial, uncertain, or recoverable outcomes. A toast alone must not be the only record of a consequential result.

### Preserve the last trustworthy state

While a replacement Preview, source, or result is pending, keep the last trustworthy content visible when the feature contract permits it and identify it as stale or out of date. A failed refresh must not replace a valid previous state with an empty success-looking surface.

A failure must stay local to the operation that failed. It must not erase unrelated Photo facts, decisions, Album membership, saved position, or retained workspace outputs. Retry must repeat only the failed or explicitly revalidated operation.

Access is an explicit feature boundary: temporary unreachability may retain the current authorized view, while confirmed expiry, revocation, or rejection removes private views and offers Access entry. [Session Loss and Disconnection](../access/spec.md#session-loss-and-disconnection) owns that distinction; it is not a generic loading state.

### Reconcile uncertainty

An uncertain result is not success and is not failure. The interface must say that the outcome needs inspection or reconciliation, preserve the request identity when available, and prevent a dependent action from assuming that the effect committed.

Concurrent changes must be described as changed elsewhere or conflict, not as a generic failure. A stale draft may be reapplied explicitly; it must not silently overwrite newer state.

### Destructive and recovery flow

A destructive or identity-affecting flow must use this sequence:

```text diagram
identify the exact object and scope
  → show the consequence and retained facts
  → require explicit confirmation
  → perform the bounded operation
  → report applied, partial, refused, failed, or unknown outcome
  → offer the valid recovery or inspection path
```

The design language does not make permanent effects feel casual. It also does not make reversible review decisions feel dangerous by using the same visual treatment as irreversible Trash Record Cleanup.

## Responsive Language

Responsive design changes composition, not product meaning.

- Use available space and content constraints, not user-agent or device labels.
- Wide layouts may place source navigation beside content and supporting surfaces beside or over the content.
- Narrow layouts may collapse secondary controls into a named disclosure or supporting surface, but must keep the current source, current Photo, primary decisions, navigation, and status understandable.
- A narrow screen must not become a miniature desktop toolbar. Remove or group secondary controls before shrinking the primary task.
- Short viewports must preserve Preview inspection and primary actions; supporting content yields first.
- Grid cells, filenames, facts, and action targets must not create horizontal overflow. Truncated text must remain available to assistive technology.
- The same operation must have an equivalent pointer, touch, keyboard, and programmatic path where the surface supports that modality.

## Accessibility Language

Accessibility is part of the product's meaning, not a finishing layer.

- Use semantic structure and one clear main landmark for each primary view.
- Every action must have a stable accessible name and an exposed state when its state affects activation.
- Visible focus must remain clear. Focus must enter a supporting surface and return after it closes.
- Do not communicate state, error, selection, or progress through color alone.
- Required text and controls must meet WCAG AA contrast. State colors must remain distinguishable in grayscale or with a non-color channel.
- Keyboard access must cover navigation, decisions, dialogs, Apply, Cancel, Close, and recovery actions.
- Touch targets must remain usable without relying on a precise gesture. Gestures must have visible alternatives.
- Live regions may announce committed outcomes, errors, and consequential status transitions; they must not announce every background loading detail.
- Reduced-motion preferences must remove nonessential animation and preserve the same state and feedback.
- Truncated names, positions, ratings, unavailable facts, and result details must remain available to assistive technology.

## Non-Goals

This design language does not define:

- a replacement for feature behavior or design contracts;
- a component framework or a required CSS/TypeScript API;
- a brand campaign, marketing site, or decorative film theme;
- an automatic aesthetic score or Agent judgment model;
- a second Photo, Album, metadata, or editing-state model; or
- a requirement that every surface expose every capability at once.

## Conformance

A feature conforms to this design language when its review can answer yes to all applicable questions:

1. Is the Photographer's current visual or organizational job obvious?
2. Are the current Photo, facts, and primary action visually ordered?
3. Do visible, accessible, keyboard, touch, and programmatic paths share one meaning?
4. Are pending, applied, refused, stale, failed, unavailable, and unknown outcomes distinguishable?
5. Does failure preserve trustworthy unrelated state and offer a local next step?
6. Does the composition remain useful across available widths and short heights?
7. Does the copy use the canonical product language?
8. Are technical details disclosed without replacing the human task language?
9. Can an Agent observe and reproduce the same domain effect without depending on presentation state?

The owning feature specification remains the acceptance authority for exact behavior. This document is the design review authority for shared presentation and interaction language.
