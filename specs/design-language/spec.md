# Product Design Language

This contract owns shared observable presentation, interaction, content, and accessibility behavior. Feature contracts own exact effects, scope, and recovery. [CONTEXT.md](../../CONTEXT.md) defines domain language; the [Design Language Contract](design.md) owns implementation boundaries and the [Visual System](visual-system.md) owns the Web reference scale.

## Product Reading

The interface MUST present the Photo as the subject and make evidence, decisions, consequences, and recovery understandable. It MUST support review without losing the Photographer's place.

The Web, CLI, and Agent surfaces MUST share domain identity, facts, effects, scope, and outcomes, even when their affordances differ. They MUST preserve [Original safety](../spec.md#product-boundaries).

## Design Thesis

Visual emphasis, interaction, and messages MUST support evidence, a decision, continuity, or an outcome and recovery path. Decoration, novelty, and hidden automation MUST NOT justify a new visual or interaction pattern.

## Design Principles

### Photograph first

The Preview and its surrounding facts MUST receive the strongest visual priority. Controls MUST frame the Photo rather than compete with it.

### Truth before reassurance

The interface MUST distinguish known facts, work in progress, and uncertainty. It MUST NOT invent counts, progress, Preview quality, or completion, or present a refused, partial, stale, failed, or unknown outcome as success.

### Decisions stay close

Pick, Reject, Rating, navigation, and eligible Undo MUST be reachable in the current review context. Ordinary review decisions MUST NOT require leaving Photo View.

### Continuity is a feature

Source, order, current Photo, position, Selection State, Rating, and saved Album position MUST remain understandable during navigation. Supporting surfaces, layout changes, and failure recovery MUST NOT silently discard the task or its context.

### Progressive disclosure, not hidden capability

Primary actions MUST remain visible. Secondary actions MAY be grouped in a named supporting surface, and technical evidence MAY be disclosed for inspection or recovery.

A necessary action MUST NOT be hidden behind an unexplained icon, gesture, hover state, or transient message.

### Explicit effects

An action MUST make its effect and scope clear before commitment. A reversible decision MAY be direct when its owner and outcome are visible; grouped view choices MAY be staged.

Trash Record Cleanup MUST name the irreversible loss of selected application records and require review and separate confirmation. It MUST NOT delete files.

### One set of facts

Visible state, accessible names, CLI results, and Agent-readable results MUST describe the same domain fact, identity, and outcome.

An Agent MUST be able to observe and reproduce the same domain effect without depending on browser presentation state.

### Bounded attention

Loading, retention, animation, and disclosure MUST remain bounded. The current range, Photo, and action MUST remain understandable while background work continues.

### Quiet confidence

The interface SHOULD be restrained. Film decoration, dashboard ornament, and promotional cards MUST NOT compete with the review task.

## Visual Language

### Composition and hierarchy

Unless a feature contract gives a stronger task-specific reason, surfaces MUST order emphasis as follows:

1. Photo or current content.
2. Current facts and state.
3. Primary action.
4. Navigation and supporting actions.
5. Technical evidence and recovery detail.

Grid View MUST use stable cell geometry. Photo View MUST present the Preview, current facts, primary decisions, navigation, and supporting surfaces as a focused inspection surface.

Sources, options, Rating choices, Photo tools, and recovery review are supporting surfaces, not additional product pages by default.

### Tone and surfaces

- The default tone MUST be neutral, dark, and low-glare.
- Surfaces MUST be separated primarily by tonal steps, spacing, and hierarchy; borders and shadows are supporting signals.
- An accent MAY identify focus, current selection, or an available primary action, but MUST NOT carry conflicting meanings at once.
- Success, warning, failure, unavailable, Picked, Rejected, and Rating MUST have distinct semantic roles. Destructive color MUST NOT represent ordinary Reject.
- Gradients, ornamental texture, animated backgrounds, and persistent brand rows MUST NOT compete with review.
- Photo View MAY use the same Preview as a dimmed, blurred scrim inside its well. The sharp, complete Preview MUST remain the strongest visual object; the scrim MUST NOT add detail, imply another Photo, appear in Grid, or remain when the Preview is unavailable.
- State MUST NOT depend on color alone; text, iconography, geometry, position, or an accessible state MUST provide a second channel.

### Typography and numbers

- Labels, headings, buttons, and messages MUST use sentence case and the exact names in [CONTEXT.md](../../CONTEXT.md).
- Ordinary UI MUST use a readable sans-serif voice. Counts, positions, Capture Time, and other compared values MUST use a distinct numeric treatment.
- A single display serif MAY distinguish the wordmark, source titles, and supporting-surface titles; it MUST NOT carry a control, state label, action, or message.
- Mono MAY carry filenames, numeric values, and technical facts. Ordinary state and field labels, actions, and messages MUST use UI sans.
- Incidental technical micro-labels MAY use tracking but MUST remain sentence case. [Visual System](visual-system.md#type) owns role-specific reference sizes.
- Technical identifiers, revisions, and paths MUST remain secondary or disclosed unless they are the current task's object.
- Typography alone MUST NOT imply success; outcome text MUST name the outcome.

### Icons and imagery

- Familiar, simple icons SHOULD reinforce a visible label or state. A consequential action MUST NOT rely on an icon as its only explanation.
- Each icon meaning MUST remain stable; Pick, Select, and a generic checkmark MUST NOT share a symbol when their effects differ.
- Photos MUST preserve complete composition, aspect ratio, and stated Preview Source. [Grid Composition](../library-browser/spec.md#grid-composition-and-orientation) and [Preview Zoom and Fit](../library-browser/spec.md#preview-zoom-and-fit) own composition and magnification behavior.
- RAW Preview source and derivative dimensions follow [Preview Normalization](../photo-library/previews/spec.md#preview-normalization); a sibling JPEG MUST NOT substitute for a RAW Preview.
- Placeholder imagery MUST name the unavailable or failed fact without resembling a valid Photo.

### Density, shape, and rhythm

- The layout SHOULD use a compact, stable rhythm without visual fatigue.
- Grid loading MUST NOT cause placeholder churn, unpredictable jumps, or changes to cell geometry.
- Surface shapes MUST remain a small family rather than accumulating one-off card treatments.
- Whitespace MUST group facts and actions rather than create decorative emptiness.
- Primary touch targets MUST remain comfortable and separated. Compact layouts MAY reduce secondary controls before reducing primary targets or legibility.

### State visibility

Interactive objects MUST distinguish each applicable state:

- Resting.
- Hover or pointing when supported.
- Keyboard focus.
- Pressed or committed.
- Current or selected.
- Disabled or unavailable.
- Pending.
- Stale.
- Failed or refused.
- Unknown or awaiting reconciliation.

Applied, partial, refused, failed, and unknown outcomes MUST remain distinguishable.

Pending MUST NOT look committed. Stale MUST NOT look current. An unavailable Original MUST NOT look like a Removed Photo.

A resting enabled control MUST retain its ordinary label weight and accessible state. It MUST NOT be dimmed, muted, or hidden to imply unavailability, and MUST meet ordinary text contrast.

### Motion

Motion MUST explain continuity, spatial relationship, or state transition. It MUST be short, interruptible, and unnecessary for understanding the outcome.

Loading motion MUST NOT imply unmeasurable progress. Reduced motion MUST remove nonessential movement without removing state or feedback.

## Interaction Language

### Core action classes

| Class | Contract |
| --- | --- |
| Read or navigate | Changes the view without silently changing Selection State, Rating, Album membership, or destructive state. A feature MAY define a continuity effect such as [saved position](../library-browser/spec.md#saved-position). |
| Direct reversible decision | Records a bounded effect and reports pending and settled outcomes. |
| Staged view choice | Applies filter, order, or presentation changes only on explicit Apply or equivalent commit; Cancel or Close discards the draft. |
| Explicit recovery | Reviews uncertainty or unavailability; bounded reads and retries MAY repeat directly, while consequential recovery shows proposed effects and requires explicit Apply. |
| Irreversible consequence | Identifies exact scope and consequence, requires separate confirmation, and reports the final effect. |

A feature MUST NOT silently change an action's class. A review shortcut MUST NOT become an irreversible operation.

### Visible controls and accelerators

Gestures and keyboard shortcuts MAY accelerate visible Web capabilities but MUST NOT create hidden Web capabilities. Visible controls or equivalent accessible paths MUST remain available.

CLI and Agent capabilities MAY differ from Web capabilities. They MUST be discoverable through [machine-facing help and contracts](../command-line/spec.md#starting-and-discovering-capabilities) and preserve shared meaning, scope, and outcome.

Library Browser [touch gestures](../library-browser/spec.md#touch-gestures) and [visible controls](../library-browser/experience/spec.md#photo-view) MUST remain equivalent paths for feature-defined decisions and navigation.

Undo MUST identify its operation scope and be understandable as an eligible operation's inverse. [Library Browser Undo](../library-browser/spec.md#undo) and [Photo Development history](../photo-development/spec.md#autosave-and-reversible-editing) own their distinct rules.

Controls MUST communicate client-known state and availability before activation when these affect the outcome. The surface MUST report authoritative refusal or conflict.

### Supporting surfaces

A supporting surface MUST have a clear name, invoker, and coherent purpose. Closing it MUST preserve an already admitted operation's settlement rules.

A modal supporting surface MUST move focus inside, prevent background interaction, offer Close or Cancel as appropriate, and return focus to the invoker or nearest valid control. A persistent region follows its feature's navigation and focus contract without modal trapping.

At most one modal supporting surface SHOULD be active. Related details SHOULD replace its content rather than form an unexplained stack; [Supporting Surfaces](../library-browser/experience/spec.md#supporting-surfaces) owns Browser mechanics.

### Continuity and destinations

Meaningful source, order, filter, or current Photo destinations MUST be addressable. Browser history MUST represent destinations, not temporary panels; returning from Photo View MUST restore meaningful source context.

Source, order, or filter changes MUST state whether they preserve the current Photo, reset to a source default, or require a new destination. A stale destination MUST NOT be silently reinterpreted against new Library facts. [Destinations and Browser History](../library-browser/experience/spec.md#destinations-and-browser-history) owns exact behavior.

## Content Language

### Voice

Copy MUST use short, active, concrete sentences and name the fact, effect, or next action before implementation detail. Failure messages MUST explain the next available action.

Visible labels, accessible names, and CLI result descriptions MUST use the same domain distinctions. Carrier-specific enum spellings remain in the [CLI Reference](../command-line/reference.md).

### Stable vocabulary

Use the names and distinctions in [CONTEXT.md](../../CONTEXT.md), including Pick versus Select, No Rating, Original unavailable, Library Check, and Capture Time. The recovery entry MUST read **Review unavailable Originals** without implying the Photo was deleted.

User-facing copy MUST NOT call Library Check a scan or rescan, or Capture Time file time or Copy Time.

### Labels and messages

- Action labels MUST use verbs; state labels MUST use stable nouns or adjectives naming the specific state.
- Messages MUST combine condition, effect, and next action when one exists.
- Generic labels such as “Done” MUST NOT replace a nameable effect.
- Internal errors, request IDs, storage paths, and implementation labels MUST NOT be primary messages unless the user is inspecting technical details.
- Counts MUST identify what they count and use established pluralization. Progress MUST use a known total; otherwise it MUST name the phase without inventing a number.

## Feedback and Recovery

### Feedback is a fact

Feedback MUST identify the operation or state, outcome, affected scope, and available next action. Routine success MAY use compact feedback; consequential, partial, uncertain, or recoverable outcomes MAY use persistent feedback.

A transient notice MUST NOT be the only record of a consequential, partial, uncertain, or recoverable outcome.

### Preserve the last trustworthy state

When the feature contract permits, pending replacement content MUST retain the last trustworthy content and identify it as stale. A failed refresh MUST NOT replace valid prior state with an empty success-looking surface.

Failure MUST remain local and MUST NOT erase unrelated Photo facts, decisions, Album membership, saved position, or retained workspace outputs. Retry MUST repeat only the failed or explicitly revalidated operation.

[Session Loss and Disconnection](../access/spec.md#session-loss-and-disconnection) distinguishes temporary unreachability from confirmed loss of access to private views.

### Reconcile uncertainty

An uncertain result MUST NOT be reported as success or failure. The interface MUST request inspection or reconciliation, preserve request identity when available, and prevent dependent actions from assuming commitment.

Concurrent changes MUST be described as changed elsewhere or conflict. A stale draft MAY be reapplied explicitly but MUST NOT silently overwrite newer state.

### Destructive and recovery flow

A destructive or identity-affecting flow MUST identify the exact object and scope, show consequences and retained facts, and require explicit confirmation. It MUST report the bounded operation's actual outcome and offer a valid recovery or inspection path.

Irreversible consequences MUST NOT use the same presentation as reversible review decisions.

## Responsive Language

- Composition MUST follow available space and content constraints, not device labels or user-agent detection.
- Wide layouts MAY place source navigation beside content and supporting surfaces beside or over content.
- Narrow layouts MAY group secondary controls in a named surface but MUST keep the current source, Photo, primary decisions, navigation, and status understandable.
- Narrow layouts MUST group or remove secondary controls before shrinking the primary task. Short viewports MUST preserve Preview inspection and primary actions before supporting content.
- Grid cells, filenames, facts, and action targets MUST NOT create horizontal overflow. Truncated text MUST remain available to assistive technology.
- Operations MUST have equivalent pointer, touch, keyboard, and programmatic paths where the surface supports each modality.

## Accessibility Language

- Each primary view MUST use semantic structure and one clear main landmark.
- Every action MUST have a stable accessible name and expose state when it affects activation.
- Focus MUST be visible. Modal focus behavior follows [Supporting surfaces](#supporting-surfaces).
- Required text and controls MUST meet WCAG AA contrast. State, error, selection, and progress MUST NOT depend on color alone.
- Keyboard access MUST cover navigation, decisions, dialogs, Apply, Cancel, Close, and recovery actions.
- Touch targets MUST be usable without a precise gesture; gestures MUST have visible alternatives.
- Live regions MAY announce committed outcomes, errors, and consequential status transitions, but MUST NOT announce every background loading detail.
- Reduced motion MUST preserve state and feedback.
- Truncated names, positions, ratings, unavailable facts, and result details MUST remain available to assistive technology.
- Text and controls MUST remain readable under text enlargement and reflow. Equivalent supported input paths MUST NOT depend on timing, hover, precision, or a hidden shortcut.
