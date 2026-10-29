import { expect, test } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { setupBrowserSmoke } from "./browser-test-support/fixtures.js";
import {
  openPhotoToolsView,
  startReview,
} from "./browser-test-support/surfaces.js";
import {
  capturedJob,
  commandsFor,
  editFixture,
  exportButton,
  fileRow,
  jobRow,
  json,
  navigate,
  openWorkspace,
  openWorkspaceFiles,
  retainFile,
  setExposure,
  setupWorkspace,
} from "./browser-test-support/workspace-edit-fixtures.js";

setupBrowserSmoke();

test("exports capture the saved revision before newer edits and can be cancelled while queued", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const first = state.photos[0]!;
  await openWorkspace(page, state);
  await setExposure(page, "0.5");
  await expect.poll(() => state.edits.get(first)?.revision).toBe("r2");
  await exportButton(page).click();
  await expect
    .poll(() => commandsFor(state, first, "export create").length)
    .toBe(1);
  const request = commandsFor(state, first, "export create")[0]!;
  const job = state.jobs
    .get(first)!
    .find((item) => item.requestId === request.command.arguments.requestId)!;
  await expect(
    jobRow(page, job.requestId).getByRole("button", {
      name: "Cancel",
      exact: true,
    }),
  ).toBeEnabled();
  await setExposure(page, "1");
  await expect.poll(() => state.edits.get(first)?.revision).toBe("r3");
  await expect(jobRow(page, job.requestId)).toContainText("saved revision r2");
  expect(request.command.arguments.revision).toBe("r2");
  expect(job.capture).toMatchObject({ revision: "r2" });
  await jobRow(page, job.requestId)
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  await expect(jobRow(page, job.requestId)).toContainText("cancelled");
  await expect(
    jobRow(page, job.requestId).getByRole("button", {
      name: "Retry captured settings",
    }),
  ).toBeEnabled();
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toHaveValue(
    "1",
  );
});

test("retry retains failed export capture instead of exporting the current settings", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const first = state.photos[0]!;
  const failed = capturedJob(first, "retained-failure", "r0", "failed");
  failed.result = { fileCommitted: false, reason: "output_budget_exceeded" };
  state.jobs.get(first)!.push(failed);
  await openWorkspace(page, state);
  await setExposure(page, "0.8");
  await expect.poll(() => state.edits.get(first)?.revision).toBe("r2");
  await jobRow(page, failed.requestId)
    .getByRole("button", { name: "Retry captured settings" })
    .click();
  await expect
    .poll(() => commandsFor(state, first, "export retry").length)
    .toBe(1);
  const retry = commandsFor(state, first, "export retry")[0]!;
  const newId = String(retry.command.arguments.requestId);
  expect(retry.command.arguments).toEqual({
    id: failed.requestId,
    requestId: newId,
  });
  expect(newId).not.toBe(failed.requestId);
  const retried = state.jobs
    .get(first)!
    .find((job) => job.requestId === newId)!;
  expect(retried.capture).toEqual(failed.capture);
  await expect(jobRow(page, newId)).toContainText("saved revision r0");
  await expect(
    jobRow(page, newId).getByRole("button", { name: "Cancel", exact: true }),
  ).toBeEnabled();
  expect(commandsFor(state, first, "export create")).toHaveLength(0);
  await jobRow(page, newId)
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  await expect(jobRow(page, newId)).toContainText("cancelled");
});

test("retained files and terminal jobs restore after reload and remain isolated by Photo", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const first = state.photos[0]!;
  const retained = retainFile(
    state,
    first,
    "retained-output",
    Buffer.from("immutable output"),
  );
  const missing = retainFile(
    state,
    first,
    "missing-output",
    Buffer.from("missing bytes"),
  );
  missing.state = "missing";
  state.jobs
    .get(first)!
    .push(
      capturedJob(first, "failed-export", "r0", "failed"),
      capturedJob(first, "cancelled-export", "r1", "cancelled"),
      capturedJob(first, "finished-export", "r1", "succeeded"),
    );
  await openWorkspace(page, state);
  await openWorkspaceFiles(page);
  await expect(
    fileRow(page, retained.artifactId).getByRole("link", {
      name: "Download",
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    fileRow(page, missing.artifactId).getByRole("link", {
      name: "Download",
      exact: true,
    }),
  ).toHaveCount(0);
  await expect(
    jobRow(page, "failed-export").getByRole("button", {
      name: "Retry captured settings",
    }),
  ).toBeEnabled();
  await page.reload();
  await expect(page.locator("[data-review]")).toBeVisible();
  await openPhotoToolsView(page, "edit");
  await openWorkspaceFiles(page);
  await expect(jobRow(page, "finished-export")).toBeVisible();
  await navigate(page, "Next");
  await expect(jobRow(page, "finished-export")).toHaveCount(0);
  await expect(fileRow(page, retained.artifactId)).toHaveCount(0);
  await navigate(page, "Previous");
  await openWorkspaceFiles(page);
  await expect(jobRow(page, "finished-export")).toBeVisible();
  await expect(fileRow(page, retained.artifactId)).toBeVisible();
});

test("download names the exact retained artifact and revision after newer edits and reload", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const first = state.photos[0]!;
  const oldBytes = Buffer.from("retained first output bytes");
  const newerBytes = Buffer.from("different newer output bytes");
  const selected = retainFile(
    state,
    first,
    "chosen-output",
    oldBytes,
    "chosen-r1",
  );
  retainFile(state, first, "newer-output", newerBytes, "newer-r2");
  await openWorkspace(page, state);
  await setExposure(page, "1.25");
  await expect.poll(() => state.edits.get(first)?.revision).toBe("r2");
  await page.reload();
  await openPhotoToolsView(page, "edit");
  await openWorkspaceFiles(page);
  const link = fileRow(page, selected.artifactId).getByRole("link", {
    name: "Download",
    exact: true,
  });
  const exactPath = `/api/workspaces/workspace-${first}/files/${selected.artifactId}/bytes?revision=${selected.revision}`;
  await expect(link).toHaveAttribute("href", exactPath);
  await expect(link).toHaveAttribute("download", "chosen-output.tif");
  // Chromium bypasses Playwright routing for a[download]; the simulated
  // attachment's Content-Disposition must initiate this actual download.
  // https://github.com/microsoft/playwright/issues/30656
  await link.evaluate((anchor) => anchor.removeAttribute("download"));
  const received = page.waitForEvent("download");
  await link.click();
  const download = await received;
  expect(download.suggestedFilename()).toBe("chosen-output.tif");
  const downloadedPath = await download.path();
  if (!downloadedPath) throw new Error("Download did not produce a local file");
  expect(await readFile(downloadedPath)).toEqual(oldBytes);
  expect(state.downloads.at(-1)).toBe(exactPath);
  expect(state.downloads.some((path) => path.includes("newer-output"))).toBe(
    false,
  );
});

test("Reload workspace discovers independent CLI edits, exports and retained files", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const first = state.photos[0]!;
  await openWorkspace(page, state);
  state.edits.set(first, editFixture(first, -0.75, "r2"));
  const file = retainFile(
    state,
    first,
    "cli-output",
    Buffer.from("CLI retained output"),
    "cli-r1",
  );
  state.jobs
    .get(first)!
    .push(capturedJob(first, "cli-export", "r2", "succeeded"));
  await expect(fileRow(page, file.artifactId)).toHaveCount(0);
  await page
    .getByRole("button", { name: "Reload workspace", exact: true })
    .click();
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toHaveValue(
    "-0.75",
  );
  await openWorkspaceFiles(page);
  await expect(fileRow(page, file.artifactId)).toBeVisible();
  await expect(jobRow(page, "cli-export")).toContainText("saved revision r2");
  await expect(
    fileRow(page, file.artifactId).getByRole("link", {
      name: "Download",
      exact: true,
    }),
  ).toBeVisible();
  expect(commandsFor(state, first, "edit apply")).toHaveLength(0);
  expect(commandsFor(state, first, "export create")).toHaveLength(0);
});

test("removal targets the reviewed revision and reports named dependent-work blockers", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const first = state.photos[0]!;
  const protectedFile = retainFile(
    state,
    first,
    "protected-output",
    Buffer.from("protected"),
    "protected-r1",
  );
  const unrelated = retainFile(
    state,
    first,
    "unrelated-output",
    Buffer.from("other"),
    "other-r1",
  );
  let blocked = true;
  state.remove = async (route, photoId, artifactId, body) => {
    if (blocked)
      return json(
        route,
        {
          error: {
            code: "file_in_use",
            effect: "none",
            message: "Dependent work prevents removal",
            details: {
              blockers: [
                {
                  name: "Print preparation",
                  action: "Remove its input reference",
                },
                { name: "Queued export", action: "Cancel the captured export" },
              ],
            },
          },
        },
        409,
      );
    if (
      artifactId !== protectedFile.artifactId ||
      body.revision !== protectedFile.revision
    )
      throw new Error("Removal did not target reviewed file revision");
    state.files.set(
      photoId,
      state.files
        .get(photoId)!
        .filter((file) => file.artifactId !== artifactId),
    );
    return json(route, { removed: true });
  };
  await openWorkspace(page, state);
  await openWorkspaceFiles(page);
  await expect(
    page.getByRole("button", {
      name: "Remove this reviewed file",
      exact: true,
    }),
  ).toHaveCount(0);
  await fileRow(page, protectedFile.artifactId)
    .getByRole("button", { name: "Inspect / Remove…", exact: true })
    .click();
  await expect(page.locator("[data-workspace-file-inspection]")).toContainText(
    protectedFile.relativePath,
  );
  await page
    .getByRole("button", { name: "Remove this reviewed file", exact: true })
    .click();
  await expect(page.locator("[data-workspace-error]")).toContainText(
    "Print preparation: Remove its input reference",
  );
  await expect(page.locator("[data-workspace-error]")).toContainText(
    "Queued export: Cancel the captured export",
  );
  await expect(fileRow(page, protectedFile.artifactId)).toBeVisible();
  await expect(fileRow(page, unrelated.artifactId)).toBeVisible();
  expect(state.removals[0]).toMatchObject({
    artifactId: protectedFile.artifactId,
    body: { revision: "protected-r1" },
  });
  blocked = false;
  await page
    .getByRole("button", { name: "Remove this reviewed file", exact: true })
    .click();
  await expect(fileRow(page, protectedFile.artifactId)).toHaveCount(0);
  await expect(fileRow(page, unrelated.artifactId)).toBeVisible();
  await expect(fileRow(page, `input-${first}`)).toBeVisible();
  await expect(page.locator("[data-workspace-file-inspection]")).toBeEmpty();
  expect(state.removals).toHaveLength(2);
});

test("a file changed after inspection requires reviewing its new revision before removal", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const first = state.photos[0]!;
  const file = retainFile(
    state,
    first,
    "changing-output",
    Buffer.from("first bytes"),
    "file-r1",
  );
  await openWorkspace(page, state);
  await openWorkspaceFiles(page);
  await fileRow(page, file.artifactId)
    .getByRole("button", { name: "Inspect / Remove…", exact: true })
    .click();
  await expect(page.locator("[data-workspace-file-inspection]")).toContainText(
    "file-r1",
  );
  // The inspected snapshot stays at r1 while an independent writer replaces the file.
  file.revision = "file-r2";
  state.bytes.set(
    `${file.artifactId}:file-r2`,
    Buffer.from("replacement bytes"),
  );
  await page
    .getByRole("button", { name: "Remove this reviewed file", exact: true })
    .click();
  await expect(page.locator("[data-workspace-error]")).toBeVisible();
  await expect(fileRow(page, file.artifactId)).toBeVisible();
  expect(state.removals[0]!.body.revision).toBe("file-r1");
  await fileRow(page, file.artifactId)
    .getByRole("button", { name: "Inspect / Remove…", exact: true })
    .click();
  await expect(page.locator("[data-workspace-file-inspection]")).toContainText(
    "file-r2",
  );
  await page
    .getByRole("button", { name: "Remove this reviewed file", exact: true })
    .click();
  await expect(fileRow(page, file.artifactId)).toHaveCount(0);
  expect(state.removals[1]!.body.revision).toBe("file-r2");
});

test("retained workspace files remain downloadable when editing tool execution is unavailable", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const first = state.photos[0]!;
  const edit = state.edits.get(first)!;
  edit.availability = {
    read: true,
    write: true,
    execute: false,
    reason: "Tool bundle unavailable",
    recovery: "Install the recorded bundle",
  };
  const bytes = Buffer.from("retained bytes require no render");
  const file = retainFile(state, first, "offline-output", bytes, "offline-r1");
  // Open without openWorkspace's enabled Export expectation: ready files have their own owner.
  await startReview(page, state.url, "All Photos");
  await openPhotoToolsView(page, "edit");
  await openWorkspaceFiles(page);
  const link = fileRow(page, file.artifactId).getByRole("link", {
    name: "Download",
    exact: true,
  });
  const exactPath = `/api/workspaces/workspace-${first}/files/${file.artifactId}/bytes?revision=${file.revision}`;
  await expect(link).toHaveAttribute("href", exactPath);
  await expect(link).toHaveAttribute("download", "offline-output.tif");
  // Chromium bypasses Playwright routing for a[download]; use the simulated
  // attachment's Content-Disposition without changing its URL or bytes.
  // https://github.com/microsoft/playwright/issues/30656
  await link.evaluate((anchor) => anchor.removeAttribute("download"));
  const received = page.waitForEvent("download");
  await link.click();
  const download = await received;
  expect(download.suggestedFilename()).toBe("offline-output.tif");
  const localPath = await download.path();
  if (!localPath) throw new Error("Retained file did not download");
  expect(await readFile(localPath)).toEqual(bytes);
  expect(state.downloads.at(-1)).toBe(exactPath);
  expect(commandsFor(state, first, "export create")).toHaveLength(0);
  expect(commandsFor(state, first, "preview render")).toHaveLength(0);
});
