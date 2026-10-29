# Visual System

This reference fixes the Web reference token scale and the presentation of
Slipstream's shared surfaces. It realizes the semantic roles in the
[Design Language Contract](design.md) and the character in the
[Product Design Language](spec.md). It does not define product behavior,
state transitions, persistence, or a component API.

The reference scale is owned by the Web rendering boundary. Feature code and
feature behavior must select semantic roles, not raw values. A feature
contract keeps ownership of its exact behavior, bounds, and recovery. The
[Library Browser Experience](../library-browser/experience/spec.md#accessibility-and-space-acceptance)
owns the 88px Grid header budget and the 44px narrow target; the
[Photo View contract](../library-browser/experience/spec.md#photo-view) owns
the 650px Preview region and the review controls;
[Grid Composition and Orientation](../library-browser/spec.md#grid-composition-and-orientation)
owns cell composition; and [Grid Thumbnail Size](../library-browser/spec.md#grid-thumbnail-size)
owns the Small/Medium/Large default. This reference composes within those
contracts.

## Problem and Scope

A semantic token layer alone does not produce a coherent surface. Without a
selected reference scale, every feature can realize the same role with a
different temperature, type voice, spacing step, or elevation, and the
Preview can lose its editorial priority even while the token names remain
correct.

This reference applies to the Web. The CLI and Agent surfaces keep their
own carriers and the same semantic meanings. A later visual theme may change
reference values to satisfy contrast, platform, or input constraints; it
must not change the semantic roles or the presentation invariants below.

## Selected and Rejected Options

- **A separate reference scale versus per-feature visual values.** Per-feature
  values keep each surface locally complete but make the same role drift in
  color, type, and spacing. A separate reference fixes the scale once. The
  reference remains subordinate to the semantic contract and does not become
  a component framework.
- **Warm graphite versus cold blue-black.** A cold blue-black canvas puts the
  chrome and the Preview's shadows in the same narrow luminance range; the
  photograph loses its matte. Warm graphite separates the surface from the
  image and lets the photograph supply the color. The selected canvas is
  `#121110`.
- **Three type voices versus one sans voice.** One sans voice makes source
  titles, filenames, counts, and controls compete at similar weight. The
  selected voice uses a display serif for the wordmark and surface titles, a
  sans for ordinary UI and state labels, and a mono for filenames, numeric
  values, and technical facts. The serif never carries a control or message.
- **An ambient Preview matte versus a black letterbox or a crop.** Cropping
  violates complete composition. A black letterbox satisfies the 650px
  region but reads as empty chrome. The selected matte is a dimmed, blurred
  copy of the same Preview behind the sharp, uncropped image. It adds no
  detail and does not become a second Photo.
- **Indicators in the caption versus on the image.** The Grid composition
  contract forbids Selection State, Rating, and Photo state indicators over
  the image. The selected caption carries the state, Rating, kind, and
  multi-selection check. The image area stays uncontaminated.
- **The reference Large step versus changing the Medium default.** The
  selected reference frames use the Large thumbnail step so the caption and
  surface presentation are legible at review scale. The Grid Thumbnail Size
  contract still starts at Medium and owns column count; this reference does
  not change that default.
- **Complete labels in an 88px two-row header versus short labels and a brand
  row.** The experience contract requires View options and Select mode to
  keep their complete labels at every supported width and limits the narrow
  Grid header to 88px. The selected composition uses two 44px rows and no
  separate brand row. Short labels and a persistent brand row were rejected.
- **A left rule for the active source versus a filled row.** A filled row
  competes with the Grid's selected and current indicators. The selected
  active source uses a 2px attention rule, the source name, and the count.
- **Role-specific legibility versus global enlargement.** Routine states and
  labels use readable sans text; numeric and technical values retain their
  distinct compact voice. A global multiplier changes unrelated text and
  consumes region budgets without deciding which facts matter. Uniform
  captions may gain a line rather than hide state or shrink required text.
- **Even row fill versus fixed-width cells with a trailing void.** The chosen
  thumbnail step still controls density. Distributing remaining width across
  its columns fills the available row without switching steps or cropping.
  Incomplete rows keep that geometry rather than stretching their few cells.

## Reference Scale

### Color

The dark reference uses these values. A theme or contrast mode may adjust a
value; it must preserve the role and the contrast requirement.

- Canvas `#121110` for the application field.
- Surface `#181614` for the review dock, mobile dock, and balanced bars.
- Raised `#211E1A` for dialogs, the select tray, hovered rows, and the active
  segment.
- Inset `#0A0908` for the Preview well, inputs, and thumbnail beds.
- Boundary `rgba(244, 236, 224, 0.08)` for region hairlines and row
  separators.
- Boundary strong `rgba(244, 236, 224, 0.16)` for input edges and the
  reference frame boundary.
- Primary content `#F3EDE3` for titles, filenames, and current values.
- Supporting content `#B9B0A3` for body text and quiet controls.
- Muted content `#8D8478` for metadata, counts, disabled text, and technical
  detail.
- Attention accent `#E3BE8A` for the focus ring, the current Photo outline,
  the active source rule, and the single primary dialog action. The accent
  must not mark Picked, Rejected, or Rating.
- Picked `#A3C3AB`; Rejected `#D99B90`; Rating `#E8CA8F`; warning `#D9A45E`;
  destructive consequence `#E06A5D`.

Soft state fills use the role hue at 12% alpha. The attention accent and the
Rating gold are separate roles: the accent never appears on stars, and the
Rating color never appears on focus or the current outline.

On the canvas, the reference contrast is primary 16.2:1, supporting 8.8:1,
muted 5.1:1, attention 10.8:1, Picked 9.8:1, Rejected 8.1:1, Rating 11.9:1,
and destructive 5.7:1. Muted text used for available information must meet 4.5:1 on its actual
surface. Disabled text may be dimmer and must also be marked disabled.

Color is never the only carrier. Every domain state also carries its text,
dot, icon, geometry, or accessible state. Destructive consequence is reserved
for effects that remove or permanently alter data; Reject is reversible and
uses the Rejected role.

### Type

- Display serif:
  `"Iowan Old Style", "Palatino Linotype", Palatino, "Book Antiqua", Georgia, serif`.
  Use it for the wordmark, source titles, and supporting-surface or dialog
  titles only.
- UI sans:
  `Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif`.
  Use it for ordinary controls, body, state text, messages, and notes.
- Mono:
  `ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace`.
  Use it for filenames, positions, counts, Capture Time, numeric values, and
  technical facts. Ordinary state and field labels use UI sans.

The reference sizes are:

- source title 30px on desktop and 22px on narrow layouts;
- wordmark 17px on desktop; a narrow supporting surface that displays the
  wordmark uses 14px;
- dialog title 20px;
- photo filename 15.5px on desktop and 12.5px on narrow layouts;
- grid filename 13px with a 12.5px narrow equivalent;
- routine Grid state and availability text, form labels, and outcome text 12px
  in UI sans;
- ordinary body and control text 12–13px, including narrow navigation;
- positions, counts, and numeric facts 11px with tabular numerals; and
- incidental technical micro-labels 10px, never required action, state, or
  outcome text.

No product text may be smaller than 10px. Counts and positions use tabular
numbers. Product labels, headings, buttons, and messages use sentence case;
the exact domain names keep their capitalization. Format names and exact
identifiers, such as RAW, JPEG, and a filename extension, keep their case.
Mono micro-labels may use tracking but must not be uppercased.

The display serif must not carry a control, state label, action, or message.
Mono must not carry an action. The UI sans remains the ordinary voice.

### Space, Shape, and Targets

The spacing scale is 4, 8, 12, 16, 24, 32, and 48px. Use 8px between a
control and its icon or label, 16px between groups in a bar, and 24px between
sections. The reference layout measures use a 28px desktop page inset, a
16px narrow page inset, a 22px Grid gap, and a 10px caption gap; they are
composition measures, not general spacing tokens.

The reference composition uses:

- a 46px desktop application bar;
- an 88px desktop source header;
- a 60px Photo View header, a 32px fact line, an 82px filmstrip, and a 64px
  review dock;
- an 88px narrow Grid header with two 44px rows and no separate brand row;
- narrow Photo View context, navigation, and review regions within the
  [owning viewport budgets](../library-browser/experience/spec.md#accessibility-and-space-acceptance),
  rather than summing independent fixed header, fact-line, and dock heights;
- the reference Large step with width-derived columns. Its illustrated column
  count is not a fixed desktop or narrow layout requirement; Grid Thumbnail
  Size retains the chosen step and default.

Every interactive target on a narrow or short layout is at least 44 by 44
CSS pixels. A visible capsule may be smaller when its hit area is at least
44; the reference Pick capsule is 38px inside a 44px target. Desktop controls
may be 32–36px high. Safe-area insets add protection; they never reduce a
target.

Radii are 6px for controls, inputs, rows, and Grid media; 12px for the
Preview well; 16px for dialogs and the select tray; and a full pill for
chips, Pick, segments, and the zoom control.

Hairlines separate regions. Buttons, chips, and Grid cells have no resting
border. Elevation is reserved for floating surfaces and the print:

- print shadow `0 24px 46px rgba(0,0,0,0.5)`, sharp Preview only;
- floating shadow `0 24px 60px rgba(0,0,0,0.5)`, dialogs and select tray;
- Grid hover lift `0 18px 40px rgba(0,0,0,0.38)`;
- the review dock uses a surface tone and an inset top highlight, not an
  outer shadow.

Motion uses 160ms ease-out for hover, opacity, and lift. Reduced motion sets
the duration to 0ms with the same end state. Motion never carries a state,
outcome, or progress meaning alone.

## Presentation

### Context Shell and Headers

The desktop application bar keeps the serif wordmark, a compact status with a
dot and `Ready`, and the account. It does not repeat the Library or source
count.

The narrow Grid has no separate brand row. Its 88px header contains two 44px
rows: source title/count/status, then View options and Select mode. The compact
filter-group count and non-default-order indication belong to the View options
entry under [Grid View](../library-browser/experience/spec.md#grid-view).
An internally scrolling condition strip is not part of the reference. Full
conditions stay readable in the opened surface and its accessible description.

The complete labels must remain visible at every supported width. They may
not be shortened, relabeled, or clipped; the 88px budget is a region budget,
not permission to compress the controls below 44px. The source title may
truncate only when the complete name remains available to assistive
technology and, where a pointer exists, on pointer hover.

The narrow Photo View has no brand row. Its 50px header carries the back
action, the mono filename, and the state chip. The back target is at least
44px. Photo View omits the application bar at every width so the review gets
the full window; the Photo header owns the back action and current Photo.

The active source in the desktop rail uses a 2px attention rule at its
leading edge, the primary content color, and the attention-colored count. It
does not use a filled rounded row.

### Grid Cells

A cell is media, then a caption. The media keeps a stable 3:2 reference box
with the inset bed, a 6px radius, and the complete, unstretched image. The
caption starts with the mono filename and position, followed by sans Selection
State and Rating. Unflagged remains a text fact without a decision badge;
zero Rating retains No Rating in its accessible meaning.

The caption uses at least two lines and gains another line when required for
readable state and Rating. Kind yields before those facts at a constrained
width. Availability remains in the required placeholder and accessible fact;
it must not be lost when the kind slot yields. A different Photo's text or
loading state must not alter row height. All cells and placeholders share
one caption height for the current width, step, text scale, and selection
gutter under [Grid presentation state](../web-application/design.md#grid-presentation-state).
At 200% text, further uniform vertical growth is allowed instead of clipping.

Selection State, Rating, kind, availability, and the multi-selection check
must not overlay the image. The current Photo uses a 2px attention outline
at a 3px offset around the media, outside the image. Hover lifts the media
with the Grid hover shadow and does not add a border.

In Select mode, every caption reserves a 16px leading gutter. A selected
cell shows the attention check in that gutter; unselected cells keep the
empty gutter so the filenames stay aligned. The image is unchanged. The
check is not a button.

When the Original or its Preview is unavailable, the kind slot, when present,
must name that unavailable fact instead of the kind. At every width, the
required media placeholder and accessible fact retain the unavailable meaning
under the [Grid composition contract](../library-browser/spec.md#grid-composition-and-orientation).
A failed image must not leave a broken-image icon. The unavailable state must
not rely on color or on a dimmed image.

### Photo View

The Photo View presents one complete, uncropped Preview in the Preview well.
The well uses the inset tone, a 12px radius, and one inner hairline. Its
layers are:

1. a blurred, dimmed copy of the same Preview, covering the well;
2. a radial matte that darkens the edges; and
3. the sharp, contained Preview with the print shadow.

The sharp Preview is the content. The matte must not add detail, imply a
second Photo, appear lighter than the sharp Preview's own shadows, or create
a bright halo at the Preview edge. It must not appear in Grid. It must not
remain when the Preview is unavailable, and it must not require a separate
Preview derivative or request. The blurred copy is `aria-hidden`.

The reference matte is `blur(90px) brightness(0.7) saturate(1.18)` with a
radial `rgba(10,9,8,0)` to `rgba(10,9,8,0.46)` scrim on desktop. On a narrow
surface use `blur(100px) brightness(0.55) saturate(1.2)` and a 0.5 scrim so
the letterbox reads as a dim matte, not a light fog.

The fact line uses UI sans for state labels and mono for numeric or technical
values. Which facts are primary or disclosed remains owned by the
[Photo View contract](../library-browser/experience/spec.md#photo-view).

The filmstrip uses 72 by 48 thumbnails at 65% opacity, a 1px boundary, and the
current thumbnail at full opacity with a 2px attention outline. It is
supporting content and yields to the Preview and the review dock on a short
viewport.

The review dock has three zones: previous/position/next; Reject, Rating, and
Pick; and Undo and More. Pick is the only filled control and uses a pill with
a sage soft fill. Rating keeps a visible label with the routine text scale,
inline with stars on desktop or stacked on narrow surfaces. Undo is disabled
only when unavailable and is never hidden. The dock must not remove an action
the [Photo View contract](../library-browser/experience/spec.md#photo-view)
requires; secondary review actions use its defined disclosure.

### Selection Mode

Select mode replaces the normal source switch and header actions. The header
carries the source name, `N / 100 selected`, and a quiet `Done`. The normal
View options and Select mode controls are not visible while selecting. The
selection behavior follows the
[Library Browser Experience](../library-browser/experience/spec.md#multi-selection).

The selection tray appears only while the multi-selection is nonempty, with
Pick, Reject, and the on-demand Add to Album entry. Actions, persistent outcome,
and scoped compensation have distinct groups under the experience contract.
Done remains the visible exit. The tray is a raised floating surface; the Grid
reserves its measured height as scrollable clearance so no final-row caption
is trapped under it. The caption check and header count carry selection; the
image stays unchanged.

### Supporting Surfaces

Dialogs and the select tray use the raised tone, a 16px radius, a hairline,
and the floating shadow. A dialog title uses the display serif; the body uses
the sans voice. Inputs use the inset tone and the strong boundary; focus uses
the 2px attention ring at a 2px offset.
The shell supplies one current-task title; repeated body titles must not
consume another heading row. Navigation entries align to a common leading
edge and group by task, rather than appearing as a flat set of centered
buttons. This treatment must preserve the named groups and local return paths
in [Supporting Surfaces](../library-browser/experience/spec.md#supporting-surfaces).

The segmented control uses an inset capsule track, 3px padding, and a raised
active segment. The active segment does not use the attention fill.

## Conformance

A Web presentation conforms when all of these hold:

- Feature code selects semantic roles; raw palette, type, spacing, radius,
  elevation, and duration values stay in the rendering boundary.
- Primary, supporting, and muted text meet the contrast requirements; muted
  text for available information stays at or above 4.5:1 on its actual
  surface; disabled text is exempt but remains marked disabled.
- No state, selection, Rating, error, or progress meaning depends on color
  alone.
- Product labels use sentence case and the exact domain names; no product
  text is smaller than 10px; routine state, form, action, and outcome labels
  use their 12px-or-larger role, not the technical micro-label role.
- The display serif is limited to the wordmark, source titles, and
  supporting-surface or dialog titles; mono is limited to filenames, numeric
  values, and technical facts. State labels, ordinary field labels, actions,
  and messages use UI sans.
- The narrow Grid header fits 88px with complete labels, two 44px rows, and
  no separate brand row; no control is clipped or relabeled at 320, 360, 390,
  420, or 760px.
- Every narrow or short interactive target is at least 44px.
- The narrow Preview region keeps the owning contract's 650px reservation at
  390 by 844 and the owning minimum at 375 by 667; the complete composition
  stays uncropped.
- Grid state, Rating, kind, availability, and multi-selection indicators are
  outside the image; the current outline is outside the media.
- The ambient matte appears only in Photo View, is derived from the same
  Preview, adds no detail or request, and is omitted when the Preview is
  unavailable.
- The one attention accent is not used for Picked, Rejected, or Rating, and
  the filled Pick is the only filled decision control.
- Focus is visible; reduced motion preserves the end state; the dock, tray,
  and dialogs remain keyboard-operable.
- Required caption state and Rating survive every size step, selection gutter,
  narrow width, and text enlargement without per-Photo row heights; full rows
  divide available width and incomplete rows keep the same cell geometry.

The design-language review questions in the
[Design Language Contract](design.md#design-review-evidence) still apply. This
reference adds the reference-scale and presentation checks above; it does not
replace behavior evidence.

## Non-Goals

This reference does not:

- define feature behavior, state ownership, ordering, persistence, or
  recovery;
- change the 44px target, 88px Grid header, 650px Preview, Grid composition,
  or Grid Thumbnail Size contracts;
- introduce a component library, CSS naming convention, token service, theme
  runtime, or icon package;
- create a light theme or a second visual brand;
- give the CLI or Agent a visual token system; or
- replace the semantic token model in the [Design Language
  Contract](design.md).
