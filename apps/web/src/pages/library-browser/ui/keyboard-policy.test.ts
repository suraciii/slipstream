import { describe, expect, test } from "bun:test";
import {
  resolveKeyIntent,
  type KeyboardContext,
  type KeyboardIntent,
  type KeyEventFacts,
  type KeyboardTargetFacts,
} from "./keyboard-policy.js";
import type { ViewSelectionState } from "./library-browser-view.js";

/// One keydown with every fact the policy reads, defaulting to a bare key
/// with no target fact applying while the Photo View is showing.
const key = (
  values: Partial<KeyEventFacts> & { key: string },
): KeyEventFacts => ({
  altKey: false,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  isComposing: false,
  ...values,
});

/// Target facts of one keydown, defaulting to nothing applying.
const target = (
  values: Partial<KeyboardTargetFacts> = {},
): KeyboardTargetFacts => ({
  contentEditable: false,
  editableControl: false,
  textInput: false,
  rangeInput: false,
  withinGridView: false,
  ...values,
});

/// Live context of one keydown, defaulting to the Photo View with a bound,
/// measurable zoom and no modal or Grid multi-selection.
const context = (values: Partial<KeyboardContext> = {}): KeyboardContext => ({
  modalBlocking: false,
  gridViewActive: false,
  gridMultiActive: false,
  zoomPresent: true,
  zoomMeasurableImage: true,
  currentSelection: "unflagged",
  ...values,
});

/// Expected intents spelled once, so each row states only what differs.
const ignore: KeyboardIntent = { kind: "ignore" };
const undo: KeyboardIntent = { kind: "undo", preventDefault: true };
const multiClear: KeyboardIntent = {
  kind: "grid-multi-clear",
  preventDefault: true,
};
const gridKey: KeyboardIntent = { kind: "grid-key", preventDefault: false };
const zoom = (action: "in" | "out" | "fit" | "detail"): KeyboardIntent => ({
  kind: "zoom",
  action,
  preventDefault: true,
});
const navigate = (direction: "previous" | "next"): KeyboardIntent => ({
  kind: "navigate",
  direction,
  preventDefault: false,
});
const select = (
  value: ViewSelectionState,
  advance: boolean,
): KeyboardIntent => ({
  kind: "select-photo",
  value,
  advance,
  preventDefault: false,
});
const rate = (value: number): KeyboardIntent => ({
  kind: "rate-photo",
  value,
  preventDefault: false,
});

/// One precedence row: name, key facts, target facts, context overrides,
/// and the exact intent the policy must resolve to.
type Row = Readonly<
  [
    name: string,
    key: Partial<KeyEventFacts> & { key: string },
    target: Partial<KeyboardTargetFacts>,
    context: Partial<KeyboardContext>,
    expected: KeyboardIntent,
  ]
>;

/// Registers one named test per row over the default facts above.
const precedence = (rows: readonly Row[]) => {
  for (const [name, keyFacts, targetFacts, contextFacts, expected] of rows)
    test(name, () =>
      expect(
        resolveKeyIntent(
          key(keyFacts),
          target(targetFacts),
          context(contextFacts),
        ),
      ).toEqual(expected),
    );
};

describe("native targets and modal blocking", () => {
  precedence([
    [
      "composition beats undo",
      { key: "z", ctrlKey: true, isComposing: true },
      {},
      {},
      ignore,
    ],
    [
      "contentEditable target",
      { key: "p" },
      { contentEditable: true },
      {},
      ignore,
    ],
    [
      "textarea, select, contenteditable=true",
      { key: "f" },
      { editableControl: true },
      {},
      ignore,
    ],
    ["non-range input", { key: "x" }, { textInput: true }, {}, ignore],
    ["Alt combinations", { key: "f", altKey: true }, {}, {}, ignore],
    ["Alt with arrows", { key: "ArrowRight", altKey: true }, {}, {}, ignore],
    [
      "modal blocks photo shortcuts",
      { key: "f" },
      {},
      { modalBlocking: true },
      ignore,
    ],
    [
      "modal blocks undo",
      { key: "z", ctrlKey: true },
      {},
      { modalBlocking: true },
      ignore,
    ],
    [
      "Escape behind a modal is the dialog's own",
      { key: "Escape" },
      { withinGridView: true },
      { modalBlocking: true, gridViewActive: true, gridMultiActive: true },
      ignore,
    ],
  ]);
});

describe("undo and grid view delegation", () => {
  precedence([
    [
      "ctrl+z undoes in the photo view",
      { key: "z", ctrlKey: true },
      {},
      {},
      undo,
    ],
    [
      "cmd+Z undoes in the grid view",
      { key: "Z", metaKey: true },
      {},
      { gridViewActive: true },
      undo,
    ],
    [
      "shift keeps ctrl+z from undoing",
      { key: "z", ctrlKey: true, shiftKey: true },
      {},
      {},
      ignore,
    ],
    [
      "shift keeps ctrl+z from undoing in the grid",
      { key: "z", ctrlKey: true, shiftKey: true },
      {},
      { gridViewActive: true },
      ignore,
    ],
    [
      "undo outranks grid delegation",
      { key: "z", ctrlKey: true },
      { withinGridView: true },
      { gridViewActive: true },
      undo,
    ],
    [
      "Escape clears multi-selection on the grid surface",
      { key: "Escape" },
      { withinGridView: true },
      { gridViewActive: true, gridMultiActive: true },
      multiClear,
    ],
    [
      "multi-selection Escape outside the grid delegates",
      { key: "Escape" },
      {},
      { gridViewActive: true, gridMultiActive: true },
      gridKey,
    ],
    [
      "multi-selection Escape outranks modifiers",
      { key: "Escape", ctrlKey: true, shiftKey: true },
      { withinGridView: true },
      { gridViewActive: true, gridMultiActive: true },
      multiClear,
    ],
    [
      "bare keys delegate to the grid presenter",
      { key: "p" },
      {},
      { gridViewActive: true },
      gridKey,
    ],
    [
      "Escape without multi-selection delegates",
      { key: "Escape" },
      { withinGridView: true },
      { gridViewActive: true },
      gridKey,
    ],
    [
      "shift never delegates",
      { key: "ArrowRight", shiftKey: true },
      {},
      { gridViewActive: true },
      ignore,
    ],
    [
      "modifiers never delegate",
      { key: "ArrowRight", ctrlKey: true },
      {},
      { gridViewActive: true },
      ignore,
    ],
  ]);
});

describe("photo zoom and the focused slider", () => {
  precedence([
    ["+ zooms in", { key: "+" }, {}, {}, zoom("in")],
    ["= zooms in", { key: "=" }, {}, {}, zoom("in")],
    ["- zooms out", { key: "-" }, {}, {}, zoom("out")],
    ["_ zooms out", { key: "_" }, {}, {}, zoom("out")],
    [
      "+ without a measurable image stays native",
      { key: "+" },
      {},
      { zoomMeasurableImage: false },
      ignore,
    ],
    [
      "- without a measurable image stays native",
      { key: "-" },
      {},
      { zoomMeasurableImage: false },
      ignore,
    ],
    [
      "+ outranks the focused slider",
      { key: "+" },
      { rangeInput: true },
      {},
      zoom("in"),
    ],
    [
      "_ with Shift outranks the Shift filter",
      { key: "_", shiftKey: true },
      {},
      {},
      zoom("out"),
    ],
    [
      "f applies without the measurable-image gate",
      { key: "f" },
      {},
      { zoomMeasurableImage: false },
      zoom("fit"),
    ],
    [
      "d applies without the measurable-image gate",
      { key: "d" },
      {},
      { zoomMeasurableImage: false },
      zoom("detail"),
    ],
    [
      "unbound zoom leaves + native",
      { key: "+" },
      {},
      { zoomPresent: false },
      ignore,
    ],
    [
      "unbound zoom leaves f native",
      { key: "f" },
      {},
      { zoomPresent: false },
      ignore,
    ],
    [
      "other modifiers stay native in the photo view",
      { key: "f", ctrlKey: true },
      {},
      {},
      ignore,
    ],
    [
      "slider keeps ArrowLeft",
      { key: "ArrowLeft" },
      { rangeInput: true },
      {},
      ignore,
    ],
    [
      "slider keeps ArrowRight",
      { key: "ArrowRight" },
      { rangeInput: true },
      {},
      ignore,
    ],
    [
      "slider keeps ArrowUp",
      { key: "ArrowUp" },
      { rangeInput: true },
      {},
      ignore,
    ],
    [
      "slider keeps ArrowDown",
      { key: "ArrowDown" },
      { rangeInput: true },
      {},
      ignore,
    ],
    [
      "slider keeps PageUp",
      { key: "PageUp" },
      { rangeInput: true },
      {},
      ignore,
    ],
    [
      "slider keeps PageDown",
      { key: "PageDown" },
      { rangeInput: true },
      {},
      ignore,
    ],
    ["slider keeps Home", { key: "Home" }, { rangeInput: true }, {}, ignore],
    ["slider keeps End", { key: "End" }, { rangeInput: true }, {}, ignore],
    [
      "f stays available on the slider",
      { key: "f" },
      { rangeInput: true },
      {},
      zoom("fit"),
    ],
    [
      "p stays available on the slider",
      { key: "p" },
      { rangeInput: true },
      {},
      select("picked", true),
    ],
  ]);
});

describe("photo navigation, selection, and rating", () => {
  precedence([
    [
      "ArrowLeft navigates without prevent-default",
      { key: "ArrowLeft" },
      {},
      {},
      navigate("previous"),
    ],
    [
      "ArrowRight navigates without prevent-default",
      { key: "ArrowRight" },
      {},
      {},
      navigate("next"),
    ],
    ["p picks and advances", { key: "p" }, {}, {}, select("picked", true)],
    ["x rejects and advances", { key: "x" }, {}, {}, select("rejected", true)],
    [
      "u clears the flag from a picked photo without advancing",
      { key: "u" },
      {},
      { currentSelection: "picked" },
      select("unflagged", false),
    ],
    [
      "u clears the flag from a rejected photo without advancing",
      { key: "u" },
      {},
      { currentSelection: "rejected" },
      select("unflagged", false),
    ],
    ["u leaves an unflagged photo native", { key: "u" }, {}, {}, ignore],
    ["0 rates the photo", { key: "0" }, {}, {}, rate(0)],
    ["3 rates the photo", { key: "3" }, {}, {}, rate(3)],
    ["5 rates the photo", { key: "5" }, {}, {}, rate(5)],
    ["6 stays native", { key: "6" }, {}, {}, ignore],
    ["9 stays native", { key: "9" }, {}, {}, ignore],
    ["shift letters stay native", { key: "F", shiftKey: true }, {}, {}, ignore],
    [
      "shift selections stay native",
      { key: "P", shiftKey: true },
      {},
      {},
      ignore,
    ],
  ]);
});
