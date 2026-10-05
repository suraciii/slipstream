/// Keyboard policy for the Library Browser root listener: which semantic
/// action one keydown resolves to. Pure and DOM-free: the listener reads the
/// facts below from the live event and presenters, then executes the
/// returned intent; the Grid presenter keeps its own geometry and
/// prevent-default for delegated events, so the root never prevents twice.
import type { ViewSelectionState } from "./library-browser-view.js";

/// The KeyboardEvent facts the precedence reads.
export interface KeyEventFacts {
  readonly key: string;
  readonly altKey: boolean;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly shiftKey: boolean;
  readonly isComposing: boolean;
}

/// What the listener reads from the actual event target; a keydown with no
/// target reports every fact false.
export interface KeyboardTargetFacts {
  /// The target's isContentEditable property.
  readonly contentEditable: boolean;
  /// Matches "textarea, select, [contenteditable=true]".
  readonly editableControl: boolean;
  /// An <input> whose type is not "range".
  readonly textInput: boolean;
  /// Matches "input[type=range]" (the zoom slider).
  readonly rangeInput: boolean;
  /// Contained by the Grid View element.
  readonly withinGridView: boolean;
}

/// The live semantic state the precedence reads.
export interface KeyboardContext {
  readonly modalBlocking: boolean;
  /// True while the Grid View is showing and the Photo View is hidden.
  readonly gridViewActive: boolean;
  /// Grid multi-selection is armed or holds at least one entry.
  readonly gridMultiActive: boolean;
  /// A Photo zoom controller is bound.
  readonly zoomPresent: boolean;
  /// The zoom has a measurable image to operate on.
  readonly zoomMeasurableImage: boolean;
  /// The current Photo's selection state.
  readonly currentSelection: ViewSelectionState;
}

/// The action one keydown resolves to. `preventDefault` is the root
/// listener's own decision; `grid-key` delegates the raw event so the Grid
/// presenter decides, and `ignore` lets native handling proceed untouched.
export type KeyboardIntent =
  | Readonly<{ kind: "ignore" }>
  | Readonly<{ kind: "undo"; preventDefault: true }>
  | Readonly<{ kind: "grid-multi-clear"; preventDefault: true }>
  | Readonly<{ kind: "grid-key"; preventDefault: false }>
  | Readonly<{
      kind: "zoom";
      action: "in" | "out" | "fit" | "detail";
      preventDefault: true;
    }>
  | Readonly<{
      kind: "navigate";
      direction: "previous" | "next";
      preventDefault: false;
    }>
  | Readonly<{
      kind: "select-photo";
      value: ViewSelectionState;
      advance: boolean;
      preventDefault: false;
    }>
  | Readonly<{ kind: "rate-photo"; value: number; preventDefault: false }>;

/// Precedence: composition/editable/Alt targets and blocking modals stay
/// native; Ctrl/Cmd-Z without Shift undoes in both surfaces; Grid View
/// clears multi-selection on Escape or delegates bare keys; the Photo View
/// answers zoom step keys, leaves a focused range slider its adjustment
/// keys, drops remaining Shift/modifier combinations, then navigates, fits,
/// toggles detail, decides, or rates the current Photo.
export const resolveKeyIntent = (
  key: KeyEventFacts,
  target: KeyboardTargetFacts,
  context: KeyboardContext,
): KeyboardIntent => {
  if (
    key.isComposing ||
    target.contentEditable ||
    key.altKey ||
    target.editableControl ||
    target.textInput
  )
    return { kind: "ignore" };
  // A modal surface owns the keyboard; its own cancel listener handles
  // Escape, so no branch here re-implements it.
  if (context.modalBlocking) return { kind: "ignore" };
  const modifier = key.ctrlKey || key.metaKey;
  if (modifier && !key.shiftKey && key.key.toLowerCase() === "z")
    return { kind: "undo", preventDefault: true };
  if (context.gridViewActive) {
    if (
      key.key === "Escape" &&
      context.gridMultiActive &&
      target.withinGridView
    )
      return { kind: "grid-multi-clear", preventDefault: true };
    // Grid View keys act only while the Grid owns keyboard focus.
    if (!modifier && !key.shiftKey)
      return { kind: "grid-key", preventDefault: false };
    return { kind: "ignore" };
  }
  if (modifier) return { kind: "ignore" };
  if (!context.zoomPresent) return { kind: "ignore" };
  // Zoom step keys outrank the focused slider and the Shift filter below.
  if (key.key === "+" || key.key === "=") {
    if (!context.zoomMeasurableImage) return { kind: "ignore" };
    return { kind: "zoom", action: "in", preventDefault: true };
  }
  if (key.key === "-" || key.key === "_") {
    if (!context.zoomMeasurableImage) return { kind: "ignore" };
    return { kind: "zoom", action: "out", preventDefault: true };
  }
  // A focused zoom slider keeps its adjustment keys; other shortcuts stay
  // available while it holds focus.
  if (target.rangeInput && rangeAdjustmentKey(key.key))
    return { kind: "ignore" };
  if (key.shiftKey) return { kind: "ignore" };
  if (key.key === "ArrowLeft")
    return { kind: "navigate", direction: "previous", preventDefault: false };
  if (key.key === "ArrowRight")
    return { kind: "navigate", direction: "next", preventDefault: false };
  const letter = key.key.toLowerCase();
  if (letter === "f")
    return { kind: "zoom", action: "fit", preventDefault: true };
  if (letter === "d")
    return { kind: "zoom", action: "detail", preventDefault: true };
  if (letter === "p")
    return {
      kind: "select-photo",
      value: "picked",
      advance: true,
      preventDefault: false,
    };
  if (letter === "x")
    return {
      kind: "select-photo",
      value: "rejected",
      advance: true,
      preventDefault: false,
    };
  if (letter === "u" && context.currentSelection !== "unflagged")
    return {
      kind: "select-photo",
      value: "unflagged",
      advance: false,
      preventDefault: false,
    };
  if (/^[0-5]$/.test(key.key))
    return {
      kind: "rate-photo",
      value: Number(key.key),
      preventDefault: false,
    };
  return { kind: "ignore" };
};

/// Keys a focused range input handles itself: arrows, Home, End, and Page.
const rangeAdjustmentKey = (key: string): boolean =>
  key.startsWith("Arrow") ||
  key.startsWith("Page") ||
  key === "Home" ||
  key === "End";
