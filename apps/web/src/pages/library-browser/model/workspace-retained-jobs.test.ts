import { expect, test } from "bun:test";
import { retainedExportJobs } from "./workspace-retained-jobs.js";

test("the tool's retained export job is recovered with its captured revision", () => {
  const jobs = retainedExportJobs(
    [
      {
        requestId: "req-1",
        status: "succeeded",
        revision: "file-r1",
        capture: { revision: "r2" },
      },
    ],
    "edit",
    [],
  );
  expect(jobs).toEqual([
    {
      requestId: "req-1",
      body: {
        requestId: "req-1",
        status: "succeeded",
        revision: "file-r1",
        capture: { revision: "r2" },
      },
      operation: "export create",
      editId: "edit",
      revision: "r2",
    },
  ]);
});

test("an already retained job is never recovered again", () => {
  const reported = [
    { requestId: "req-1", status: "succeeded", revision: "file-r1" },
  ];
  expect(
    retainedExportJobs(reported, "edit", [{ requestId: "req-1" }]),
  ).toEqual([]);
  // A newer job for the same edit is still recovered.
  const recovered = retainedExportJobs(
    [
      ...reported,
      { requestId: "req-2", status: "failed", revision: "file-r2" },
    ],
    "edit",
    [{ requestId: "req-1" }],
  );
  expect(recovered.map((job) => job.requestId)).toEqual(["req-2"]);
});

test("a malformed or non-array export list recovers nothing", () => {
  expect(retainedExportJobs(undefined, "edit", [])).toEqual([]);
  expect(
    retainedExportJobs([null, "x", { status: "succeeded" }], "edit", []),
  ).toEqual([]);
});
