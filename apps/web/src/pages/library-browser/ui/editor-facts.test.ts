import { expect, test } from "bun:test";
import type { EditorViewModel } from "../model/editor-controller-contract.js";
import { editorFactRows } from "./editor-facts.js";

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
