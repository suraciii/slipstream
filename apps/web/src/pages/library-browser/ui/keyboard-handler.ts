import { resolveKeyIntent, type KeyboardIntent } from "./keyboard-policy.js";
import type { ViewSelectionState } from "./library-browser-view.js";

/// The subset of page intents the root keyboard listener emits.
type KeyboardSendIntent =
  | Readonly<{ kind: "undo" }>
  | Readonly<{ kind: "grid-multi-clear" }>
  | Readonly<{ kind: "previous" }>
  | Readonly<{ kind: "next" }>
  | Readonly<{
      kind: "photo-mutation";
      field: "selectionState";
      value: ViewSelectionState;
      advance: boolean;
    }>
  | Readonly<{
      kind: "photo-mutation";
      field: "rating";
      value: number;
      advance: false;
    }>;

type KeyboardHandlerDeps = Readonly<{
  alive: () => boolean;
  gridView: HTMLElement;
  photoView: HTMLElement;
  surfaces: Readonly<{ blocking(): boolean }>;
  multiCount: () => number;
  multiMode: () => boolean;
  zoom:
    | Readonly<{
        hasMeasurableImage(): boolean;
        zoomIn(): void;
        zoomOut(): void;
        applyFit(): void;
        toggleDetail(): void;
      }>
    | undefined;
  currentSelection: () => ViewSelectionState;
  handleGridKey: (event: KeyboardEvent) => void;
  send: (intent: KeyboardSendIntent) => void;
}>;

/// The root keyboard listener: it reads live facts, resolves one semantic
/// intent, then executes that intent. The Grid presenter keeps its own
/// geometry and prevent-default for delegated events, so this listener never
/// prevents twice.
export const createKeyboardHandler =
  (deps: KeyboardHandlerDeps) =>
  (event: KeyboardEvent): void => {
    if (!deps.alive()) return;
    const target = event.target as HTMLElement | null;
    const intent = resolveKeyIntent(
      {
        key: event.key,
        altKey: event.altKey,
        ctrlKey: event.ctrlKey,
        metaKey: event.metaKey,
        shiftKey: event.shiftKey,
        isComposing: event.isComposing,
      },
      {
        contentEditable: target?.isContentEditable === true,
        editableControl: Boolean(
          target?.matches("textarea, select, [contenteditable=true]"),
        ),
        textInput:
          target?.matches("input") === true &&
          (target as HTMLInputElement).type !== "range",
        rangeInput: target?.matches("input[type=range]") === true,
        withinGridView: deps.gridView.contains(target),
      },
      {
        modalBlocking: deps.surfaces.blocking(),
        gridViewActive: deps.photoView.hidden,
        gridMultiActive: deps.multiCount() > 0 || deps.multiMode(),
        zoomPresent: Boolean(deps.zoom),
        zoomMeasurableImage: deps.zoom?.hasMeasurableImage() ?? false,
        currentSelection: deps.currentSelection(),
      },
    );
    if (intent.kind === "ignore") return;
    executeIntent(intent, event, deps);
  };

const executeIntent = (
  intent: KeyboardIntent,
  event: KeyboardEvent,
  deps: KeyboardHandlerDeps,
): void => {
  if (intent.kind === "ignore") return;
  if (intent.preventDefault) event.preventDefault();
  switch (intent.kind) {
    case "undo":
      deps.send({ kind: "undo" });
      return;
    case "grid-multi-clear":
      deps.send({ kind: "grid-multi-clear" });
      return;
    case "grid-key":
      deps.handleGridKey(event);
      return;
    case "zoom":
      if (intent.action === "in") deps.zoom?.zoomIn();
      else if (intent.action === "out") deps.zoom?.zoomOut();
      else if (intent.action === "fit") deps.zoom?.applyFit();
      else deps.zoom?.toggleDetail();
      return;
    case "navigate":
      deps.send(
        intent.direction === "previous"
          ? { kind: "previous" }
          : { kind: "next" },
      );
      return;
    case "photo-decision":
      deps.send({
        kind: "photo-mutation",
        field: "selectionState",
        value: intent.value,
        advance: intent.advance,
      });
      return;
    case "rate-photo":
      deps.send({
        kind: "photo-mutation",
        field: "rating",
        value: intent.value,
        advance: false,
      });
      return;
  }
};
