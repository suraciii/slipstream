# Visual System

This contract fixes the Web reference token scale and shared surface presentation. It realizes the roles in the [Design Language Contract](design.md) and the behavior in [Product Design Language](spec.md). Feature contracts own behavior, state, and recovery; the CLI and Agent retain their own carriers.

The rendering boundary owns reference values. Feature code and behavior MUST select semantic roles rather than raw values when a role exists. A theme or contrast mode MAY adjust reference values while preserving semantic roles and contrast requirements.

[Library Browser Experience](../library-browser/experience/spec.md#accessibility-and-space-acceptance) owns the Grid header budget and narrow targets. [Photo View](../library-browser/experience/spec.md#photo-view) owns the Preview region and review controls; [Grid Composition](../library-browser/spec.md#grid-composition-and-orientation) and [Grid Thumbnail Size](../library-browser/spec.md#grid-thumbnail-size) own cell composition and thumbnail defaults.

## Reference Scale

### Color

The dark reference scale is:

| Role                    | Value                       | Reference uses                                                                      |
| ----------------------- | --------------------------- | ----------------------------------------------------------------------------------- |
| Canvas                  | `#121110`                   | Application field                                                                   |
| Surface                 | `#181614`                   | Review dock, mobile dock, balanced bars                                             |
| Raised                  | `#211E1A`                   | Dialogs, select tray, hovered rows, active segment                                  |
| Inset                   | `#0A0908`                   | Preview well, inputs, thumbnail beds                                                |
| Boundary                | `rgba(244, 236, 224, 0.08)` | Region hairlines, row separators                                                    |
| Boundary strong         | `rgba(244, 236, 224, 0.16)` | Input edges, reference frame boundary                                               |
| Primary content         | `#F3EDE3`                   | Titles, filenames, current values                                                   |
| Supporting content      | `#B9B0A3`                   | Body text, quiet controls                                                           |
| Muted content           | `#8D8478`                   | Metadata, counts, disabled text, technical detail                                   |
| Attention               | `#E3BE8A`                   | Focus ring, current Photo outline, active source rule, single primary dialog action |
| Picked                  | `#A3C3AB`                   | Picked state                                                                        |
| Rejected                | `#D99B90`                   | Rejected state                                                                      |
| Rating                  | `#E8CA8F`                   | Rating                                                                              |
| Warning                 | `#D9A45E`                   | Warning                                                                             |
| Destructive consequence | `#E06A5D`                   | Effects that remove or permanently alter data                                       |

Soft state fills MUST use the role hue at 12% alpha. The attention accent MUST NOT mark Picked, Rejected, or Rating; Rating color MUST NOT mark focus or the current outline.

Muted text for available information MUST meet 4.5:1 contrast on its actual surface. Disabled text MAY be dimmer and MUST also be marked disabled. [Accessibility Language](spec.md#accessibility-language) owns general contrast and non-color state requirements.

### Type

| Voice         | Reference stack                                                                    | Uses                                                                        |
| ------------- | ---------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| Display serif | `"Iowan Old Style", "Palatino Linotype", Palatino, "Book Antiqua", Georgia, serif` | Wordmark, source titles, supporting-surface and dialog titles only          |
| UI sans       | `Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif`           | Ordinary controls, body, state and field labels, messages, notes            |
| Mono          | `ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace`      | Filenames, positions, counts, Capture Time, numeric values, technical facts |

The display serif MUST NOT carry a control, state label, action, or message. Mono MUST NOT carry an action.

| Reference text                                                 | Size                                                               |
| -------------------------------------------------------------- | ------------------------------------------------------------------ |
| Source title                                                   | 30px desktop; 22px narrow                                          |
| Wordmark                                                       | 17px desktop; 14px in a narrow supporting surface that displays it |
| Dialog title                                                   | 20px                                                               |
| Photo filename                                                 | 15.5px desktop; 12.5px narrow                                      |
| Grid filename                                                  | 13px; 12.5px narrow                                                |
| Routine Grid state and availability, form labels, outcome text | 12px in UI sans                                                    |
| Other routine state and action labels                          | 12px or larger in UI sans                                          |
| Ordinary body and control text                                 | 12–13px, including narrow navigation                               |
| Positions, counts, numeric facts                               | 11px with tabular numerals                                         |
| Incidental technical micro-labels                              | 10px; never required action, state, or outcome text                |

Role-specific sizes keep routine labels readable while preserving compact numeric and technical values within region budgets.

Product text MUST NOT be smaller than 10px. Counts and positions MUST use tabular numbers.

Labels, headings, buttons, messages, and mono micro-labels MUST use sentence case. Domain names, format names, and exact identifiers MUST retain their exact case.

### Space, Shape, and Targets

The spacing scale is 4, 8, 12, 16, 24, 32, and 48px. Reference composition measures are 28px desktop page inset, 16px narrow page inset, 22px Grid gap, and 10px caption gap; these are not general spacing tokens.

| Relationship              | Reference spacing |
| ------------------------- | ----------------- |
| Control and icon or label | 8px               |
| Groups in a bar           | 16px              |
| Sections                  | 24px              |

| Reference composition                                     | Measure                                                                                                                                                |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Desktop application bar                                   | 46px                                                                                                                                                   |
| Desktop source header                                     | 88px                                                                                                                                                   |
| Photo View header, fact line, filmstrip, review dock      | 60px, 32px, 82px, 64px respectively                                                                                                                    |
| Narrow Grid header                                        | 88px, two 44px rows, no separate brand row                                                                                                             |
| Narrow Photo View context, navigation, and review regions | Within the [owning viewport budgets](../library-browser/experience/spec.md#accessibility-and-space-acceptance), not a sum of independent fixed heights |
| Grid                                                      | Reference Large step with width-derived columns; illustrated counts are not fixed layout requirements                                                  |

The reference does not change the Medium thumbnail default or feature-owned column count.

Every interactive target in a narrow or short layout MUST be at least 44 by 44 CSS pixels. A visible capsule MAY be smaller if its hit area meets that minimum; the reference Pick capsule is 38px inside a 44px target.

Desktop controls MAY be 32–36px high. Safe-area insets MUST NOT reduce a target.

| Shape                               | Radius    |
| ----------------------------------- | --------- |
| Controls, inputs, rows, Grid media  | 6px       |
| Preview well                        | 12px      |
| Dialogs, select tray                | 16px      |
| Chips, Pick, segments, zoom control | Full pill |

Hairlines MUST separate regions. Buttons, chips, and Grid cells MUST NOT have a resting border.

| Elevation       | Reference value and scope                              |
| --------------- | ------------------------------------------------------ |
| Print           | `0 24px 46px rgba(0,0,0,0.5)`; sharp Preview only      |
| Floating        | `0 24px 60px rgba(0,0,0,0.5)`; dialogs and select tray |
| Grid hover lift | `0 18px 40px rgba(0,0,0,0.38)`                         |
| Review dock     | Surface tone and inset top highlight, no outer shadow  |

Hover, opacity, and lift use 160ms ease-out. Reduced motion MUST set duration to 0ms while preserving the end state.

## Presentation

### Context Shell and Headers

The desktop application bar MUST contain the serif wordmark, compact status with a dot and `Ready`, and account. It MUST NOT repeat Library or source counts.

The narrow Grid MUST use two 44px header rows within 88px and no separate brand row. The rows contain:

1. The source title, count, and compact status.
2. The complete `View options` and `Select mode` labels.

The compact filter-group count and non-default-order indication belong to the View options entry under [Grid View](../library-browser/experience/spec.md#grid-view). Full conditions MUST remain readable in the opened surface and its accessible description; an internally scrolling condition strip is not part of the reference.

Complete labels MUST remain visible at every supported width and MUST NOT be shortened, relabeled, or clipped.

The header budget MUST NOT compress targets below 44px. A truncated source title MUST remain complete to assistive technology and on pointer hover where available.

The narrow Photo View header has a 50px reference height and MUST contain the back action, mono filename, and state chip. Its back target MUST be at least 44px. Its composition MUST remain within the owning viewport budgets.

Photo View MUST omit the application bar at every width. The Photo header owns the back action and current Photo.

The active source in the desktop rail MUST use a 2px leading attention rule, primary content color, and attention-colored count. It MUST NOT use a filled rounded row.

### Grid Cells

A cell MUST contain media followed by a caption. The media uses a stable 3:2 reference box, inset bed, 6px radius, and complete unstretched image.

The caption starts with the mono filename and right-aligned mono position, followed by a state dot, sans Selection State, Rating stars, and kind or availability. Unflagged remains a text fact without a decision badge; zero Rating retains No Rating in its accessible meaning.

The caption MUST use at least two lines and gain another when required for readable state and Rating. Kind MUST yield before those facts at constrained widths; the placeholder and accessible fact MUST retain availability.

All cells and placeholders MUST share one caption height for the current width, step, text scale, and selection gutter under [Grid presentation state](../web-application/design.md#grid-presentation-state). Another Photo's text or loading state MUST NOT alter row height. At 200% text, further uniform vertical growth MAY prevent clipping.

Selection State, Rating, kind, availability, and multi-selection checks MUST NOT overlay the image. The current Photo uses a 2px attention outline at a 3px offset outside the media.

Hover MUST lift media with the Grid hover shadow without adding a border.

In Select mode every caption MUST reserve a 16px leading gutter. Selected cells MUST show the attention check in the gutter; unselected cells MUST retain the empty gutter to align filenames. The check is not a button and the image MUST remain unchanged.

When the Original or Preview is unavailable, the kind slot, when present, MUST name the fact. At every width the media MUST use the [Grid text placeholder](../library-browser/spec.md#grid-composition-and-orientation), and the accessible fact MUST retain the unavailable meaning. A failed image MUST NOT leave a broken-image icon or rely on color or dimming alone.

### Photo View

The Photo View MUST present one complete, uncropped Preview in an inset well with 12px radius and one inner hairline. Its layers are:

1. A blurred, dimmed copy of the same Preview covering the well.
2. A radial matte darkening the edges.
3. The sharp, contained Preview with print shadow.

The sharp Preview is the content. The matte MUST NOT add detail, imply another Photo, be lighter than the sharp Preview's shadows, or create a bright edge halo.

The matte MUST NOT appear in Grid or remain when the Preview is unavailable. It MUST NOT require another Preview derivative or request; the blurred copy MUST be `aria-hidden`.

| Surface | Matte                                        | Radial scrim                            |
| ------- | -------------------------------------------- | --------------------------------------- |
| Desktop | `blur(90px) brightness(0.7) saturate(1.18)`  | `rgba(10,9,8,0)` to `rgba(10,9,8,0.46)` |
| Narrow  | `blur(100px) brightness(0.55) saturate(1.2)` | 0.5 opacity                             |

The fact line MUST use UI sans for state labels and mono for numeric or technical values. [Photo View](../library-browser/experience/spec.md#photo-view) owns which facts are primary or disclosed.

The filmstrip uses 72 by 48 thumbnails at 65% opacity and a 1px boundary. The current thumbnail MUST use full opacity and a 2px attention outline. The filmstrip MUST yield to the Preview and review dock in short viewports.

The review dock has three zones:

1. Previous, position, and next.
2. Reject, Rating, and Pick.
3. Undo and More.

Pick MUST be the only filled decision control and use a pill with sage soft fill. Rating MUST retain a visible label at the routine text scale, inline with gold stars on desktop or stacked on narrow surfaces.

Undo MUST remain visible and be disabled only when unavailable. It uses the muted color when disabled. The dock MUST retain actions required by the [Photo View contract](../library-browser/experience/spec.md#photo-view); secondary actions MAY use its defined disclosure.

### Selection Mode

Select mode MUST replace the normal source switch and header actions. Its header MUST show the source name, `N / 100 selected`, and a quiet `Done`; normal View options and Select mode controls MUST NOT appear while selecting.

The selection tray MUST appear only while multi-selection is nonempty and contain Pick, Reject, and the on-demand Add to Album entry. It MUST NOT add Clear; Done is the visible exit. [Multi-selection](../library-browser/experience/spec.md#multi-selection) owns selection behavior.

Actions, persistent outcome, and scoped compensation MUST have distinct groups under the experience contract. The tray MUST use a raised floating surface and floating shadow. Grid MUST reserve its measured height as scrollable clearance so no final-row caption is trapped.

The caption check and header count carry selection; the image MUST remain unchanged.

### Supporting Surfaces

Dialogs and the select tray MUST use raised tone, 16px radius, a hairline, and floating shadow. Dialog titles use display serif and bodies use sans.

The shell MUST supply one current-task title; repeated body titles MUST NOT consume another heading row. Navigation entries MUST align to a common leading edge and group by task. This treatment MUST preserve named groups and local return paths in [Supporting Surfaces](../library-browser/experience/spec.md#supporting-surfaces).

Inputs use inset tone and strong boundary. Focus uses a 2px attention ring at a 2px offset.

The segmented control uses an inset capsule track, 3px padding, and a raised active segment. The active segment MUST NOT use attention fill.
