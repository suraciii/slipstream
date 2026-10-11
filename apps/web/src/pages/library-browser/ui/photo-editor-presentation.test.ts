import { expect, test } from "bun:test";
import { editorSaveRecovery } from "./photo-editor-presentation.js";

test("failed saves expose retained command recovery without a conflict", () => {
  expect(
    editorSaveRecovery({
      primary: {
        saveState: "failed",
        note: "Save failed. Your exposure change is retained.",
      },
      conflict: null,
    }),
  ).toEqual({
    visible: true,
    message: "Save failed. Your exposure change is retained.",
    canUseSaved: false,
    canReapply: true,
    canDiscard: true,
  });
});

test("uncertain saves require reconciliation before reapply or discard", () => {
  const recovery = editorSaveRecovery({
    primary: {
      saveState: "unknown",
      note: "Check the save result before continuing.",
    },
    conflict: null,
  });
  expect(recovery.visible).toBe(true);
  expect(recovery.message).toBe("Check the save result before continuing.");
  expect(recovery.canReapply).toBe(false);
  expect(recovery.canDiscard).toBe(false);
});

test("a service conflict exposes saved-state choice and explicit draft retry", () => {
  expect(
    editorSaveRecovery({
      primary: { saveState: "conflict", note: "Saving stopped." },
      conflict: { message: "Another client changed this Photo." },
    }),
  ).toEqual({
    visible: true,
    message: "Another client changed this Photo.",
    canUseSaved: true,
    canReapply: true,
    canDiscard: true,
  });
});

test("no state and successful saves have no recovery alert", () => {
  for (const saveState of ["none", "draft", "saving", "saved"] as const)
    expect(
      editorSaveRecovery({ primary: { saveState, note: "" }, conflict: null })
        .visible,
    ).toBe(false);
});
