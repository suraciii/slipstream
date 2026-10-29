import { expect, test } from "bun:test";
import type { EditorViewModel } from "../model/editor-controller-contract.js";
import { editorFactRows, saveStateRow } from "./editor-facts.js";
import { WorkspaceApiError } from "../api/workspace-edit.js";

test("processing job fact counts accepted and executing jobs as active", () => {
  const model = {
    files: [],
    draft: [],
    jobs: [
      { requestId: "a", status: "accepted" },
      { requestId: "b", status: "executing" },
      { requestId: "c", status: "failed" },
    ],
  } as unknown as EditorViewModel;
  expect(
    editorFactRows(model).find((row) => row.label === "Processing jobs")?.value,
  ).toBe(
    "2 jobs in progress. A finished file appears only after its job settles.",
  );
});

test("processing job fact reports a failed job after active work settles", () => {
  const model = {
    files: [],
    draft: [],
    jobs: [{ requestId: "a", status: "failed" }],
  } as unknown as EditorViewModel;
  expect(
    editorFactRows(model).find((row) => row.label === "Processing jobs")?.value,
  ).toBe(
    "1 job failed. The saved settings are unaffected; inspect the job's own reason.",
  );
});

test("save failure, conflict and unknown remain distinct from Preview uncertainty", () => {
  const model = {
    draft: [{ type: "set" }],
    blocked: true,
    saving: false,
    edit: { revision: "r1" },
    error: { code: "permission_denied" },
  } as unknown as EditorViewModel;
  expect(saveStateRow(model).value).toContain("failed");
  model.error = new WorkspaceApiError("state_conflict", "Conflict", "none", {});
  expect(saveStateRow(model).value).toContain("confirmed state changed");
  model.uncertain = { operation: "edit apply", arguments: {}, editId: "edit" };
  expect(saveStateRow(model).value).toContain("may have been written");
  model.uncertain = {
    operation: "preview render",
    arguments: {},
    editId: "edit",
  };
  model.draft = [];
  expect(saveStateRow(model).value).toContain("saved");
});
