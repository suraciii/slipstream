import type { EditorViewModel } from "./editor-view-contract.js";

export function editorSaveRecovery(
  model: Readonly<{
    primary?: Pick<
      NonNullable<EditorViewModel["primary"]>,
      "saveState" | "note"
    >;
    conflict: EditorViewModel["conflict"];
  }>,
) {
  const state = model.primary?.saveState;
  const failed = state === "failed" || state === "unknown";
  return {
    visible: Boolean(model.conflict) || failed,
    message:
      model.conflict?.message ?? (failed ? (model.primary?.note ?? "") : ""),
    canUseSaved: Boolean(model.conflict),
    canReapply: state !== "unknown",
    canDiscard: state !== "unknown",
  };
}
