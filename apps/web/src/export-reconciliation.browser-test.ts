import { expect, test } from "@playwright/test";
import { setupBrowserSmoke } from "./browser-test-support/fixtures.js";
import { openPhotoToolsView } from "./browser-test-support/surfaces.js";
import {
  capturedJob,
  commandsFor,
  editFixture,
  exportButton,
  jobRow,
  json,
  navigate,
  openWorkspace,
  retainFile,
  setExposure,
  setupWorkspace,
  windowOf,
} from "./browser-test-support/workspace-edit-fixtures.js";

setupBrowserSmoke();

for (const lostResponse of [
  "connection",
  "unreadable",
  "server-unknown",
] as const) {
  test(`uncertain export (${lostResponse}) replays the exact captured request for its Photo`, async ({
    page,
  }) => {
    const state = await setupWorkspace(page);
    const [first, second] = state.photos as [string, string];
    state.command = async (route, captured) => {
      if (
        captured.photoId !== first ||
        captured.command.operation !== "export create"
      )
        return false;
      if (commandsFor(state, first, "export create").length === 1) {
        // Admission happened; only the acknowledgement was lost. Replaying must not capture again.
        state.jobs
          .get(first)!
          .push(
            capturedJob(
              first,
              String(captured.command.arguments.requestId),
              String(captured.command.arguments.revision),
            ),
          );
        if (lostResponse === "connection")
          return route.abort("connectionreset");
        if (lostResponse === "unreadable")
          return route.fulfill({
            status: 202,
            contentType: "application/json",
            body: "{",
          });
        return json(
          route,
          {
            error: {
              code: "outcome_unknown",
              effect: "unknown",
              message: "Admission acknowledgement unavailable",
            },
          },
          500,
        );
      }
      return json(route, state.jobs.get(first)![0], 202);
    };
    await openWorkspace(page, state);
    await exportButton(page).click();
    await expect(
      page.getByRole("button", { name: "Replay exact request" }),
    ).toBeVisible();
    await expect(exportButton(page)).toBeDisabled();
    await navigate(page, "Next");
    await expect(exportButton(page)).toBeEnabled();
    await setExposure(page, "0.5");
    await expect.poll(() => state.edits.get(second)?.revision).toBe("r2");
    await exportButton(page).click();
    await expect
      .poll(() => commandsFor(state, second, "export create").length)
      .toBe(1);
    const secondRequest = commandsFor(state, second, "export create")[0]!;
    await expect(
      jobRow(page, String(secondRequest.command.arguments.requestId)),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Replay exact request" }),
    ).toHaveCount(0);
    await navigate(page, "Previous");
    await page.getByRole("button", { name: "Replay exact request" }).click();
    await expect(exportButton(page)).toBeEnabled();
    const requests = commandsFor(state, first, "export create");
    expect(requests).toHaveLength(2);
    expect(requests[1]!.body).toBe(requests[0]!.body);
    expect(requests[0]!.command).toMatchObject({
      operation: "export create",
      editId: `edit-${first}`,
      arguments: { revision: "r1" },
    });
    expect(secondRequest.command.arguments.requestId).not.toBe(
      requests[0]!.command.arguments.requestId,
    );
    expect(state.jobs.get(first)).toHaveLength(1);
    await expect(
      jobRow(page, String(requests[0]!.command.arguments.requestId)),
    ).toBeVisible();
    await expect(
      jobRow(page, String(secondRequest.command.arguments.requestId)),
    ).toHaveCount(0);
  });
}

for (const lostResponse of ["connection", "unreadable"] as const) {
  test(`uncertain save (${lostResponse}) blocks new intent and replays the same guarded changes`, async ({
    page,
  }) => {
    const state = await setupWorkspace(page);
    const first = state.photos[0]!;
    state.command = async (route, captured) => {
      if (captured.command.operation !== "edit apply") return false;
      if (commandsFor(state, first, "edit apply").length === 1) {
        state.edits.set(first, editFixture(first, 0.5, "r2"));
        if (lostResponse === "connection") return route.abort("failed");
        return route.fulfill({
          status: 200,
          contentType: "application/json",
          body: "{",
        });
      }
      return json(route, {
        revision: "r2",
        stateToken: "state-r2",
        current: { revision: "r2" },
      });
    };
    await openWorkspace(page, state);
    await setExposure(page, "0.5");
    await expect(
      page.getByRole("button", { name: "Replay exact request" }),
    ).toBeVisible();
    await expect(exportButton(page)).toBeDisabled();
    await expect(
      page.getByLabel("Exposure (EV)", { exact: true }),
    ).toBeDisabled();
    await expect(
      page.getByRole("button", { name: "Discard draft", exact: true }),
    ).toBeDisabled();
    await navigate(page, "Next");
    await expect(exportButton(page)).toBeEnabled();
    await navigate(page, "Previous");
    await page.getByRole("button", { name: "Replay exact request" }).click();
    await expect(exportButton(page)).toBeEnabled();
    await expect(page.getByLabel("Exposure (EV)", { exact: true })).toHaveValue(
      "0.5",
    );
    const saves = commandsFor(state, first, "edit apply");
    expect(saves).toHaveLength(2);
    expect(saves[1]!.body).toBe(saves[0]!.body);
    expect(saves[0]!.command.arguments).toMatchObject({
      ifState: "state-r1",
      input: {
        changes: [
          {
            type: "set",
            instanceId: "exposure-1",
            parameterId: "exposure",
            value: 0.5,
          },
        ],
      },
    });
  });
}

for (const resolution of ["reapply", "discard"] as const) {
  test(`a conflicting draft requires inspection before ${resolution} against independent saved changes`, async ({
    page,
  }) => {
    const state = await setupWorkspace(page);
    const first = state.photos[0]!;
    await openWorkspace(page, state);
    // An independent CLI writer advances the saved edit before the Web's guarded write.
    state.edits.set(first, editFixture(first, -1, "r2"));
    await setExposure(page, "0.75");
    await expect(page.locator("[data-workspace-error]")).toBeVisible();
    await expect(exportButton(page)).toBeDisabled();
    const reapply = page.getByRole("button", {
      name: "Reapply retained draft to inspected state",
    });
    await expect(reapply).toBeDisabled();
    await page
      .getByRole("button", { name: "Inspect current saved settings" })
      .click();
    await expect(reapply).toBeEnabled();
    if (resolution === "reapply") {
      await reapply.click();
      await expect.poll(() => state.edits.get(first)?.revision).toBe("r3");
      expect(
        commandsFor(state, first, "edit apply")[1]!.command.arguments.ifState,
      ).toBe("state-r2");
      await expect(
        page.getByLabel("Exposure (EV)", { exact: true }),
      ).toHaveValue("0.75");
    } else {
      await page
        .getByRole("button", { name: "Discard draft", exact: true })
        .click();
      await expect(
        page.getByLabel("Exposure (EV)", { exact: true }),
      ).toHaveValue("-1");
      expect(commandsFor(state, first, "edit apply")).toHaveLength(1);
    }
    await expect(exportButton(page)).toBeEnabled();
  });
}

test("late export admission stays with its Photo while another Photo saves and exports", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const [first, second] = state.photos as [string, string];
  const held = Promise.withResolvers<void>();
  const settled = Promise.withResolvers<void>();
  state.command = async (route, captured) => {
    if (
      captured.photoId !== first ||
      captured.command.operation !== "export create"
    )
      return false;
    await held.promise;
    const job = capturedJob(
      first,
      String(captured.command.arguments.requestId),
      String(captured.command.arguments.revision),
    );
    state.jobs.get(first)!.push(job);
    await json(route, job, 202);
    settled.resolve();
  };
  await openWorkspace(page, state);
  await exportButton(page).click();
  await expect
    .poll(() => commandsFor(state, first, "export create").length)
    .toBe(1);
  try {
    await navigate(page, "Next");
    await setExposure(page, "1");
    await expect.poll(() => state.edits.get(second)?.revision).toBe("r2");
    await exportButton(page).click();
    await expect
      .poll(() => commandsFor(state, second, "export create").length)
      .toBe(1);
    held.resolve();
    await settled.promise;
    const firstId = String(
      commandsFor(state, first, "export create")[0]!.command.arguments
        .requestId,
    );
    const secondId = String(
      commandsFor(state, second, "export create")[0]!.command.arguments
        .requestId,
    );
    await expect(jobRow(page, secondId)).toBeVisible();
    await expect(jobRow(page, firstId)).toHaveCount(0);
    await expect(page.getByLabel("Exposure (EV)", { exact: true })).toHaveValue(
      "1",
    );
    await navigate(page, "Previous");
    await expect(jobRow(page, firstId)).toBeVisible();
    await expect(page.getByLabel("Exposure (EV)", { exact: true })).toHaveValue(
      "0",
    );
  } finally {
    held.resolve();
  }
});

test("Camera reference remains selected when a current preview finishes later", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const first = state.photos[0]!;
  await openWorkspace(page, state);
  const original = await page.locator("[data-stage] img").getAttribute("src");
  await page.getByRole("button", { name: "Preview", exact: true }).click();
  await expect
    .poll(() => commandsFor(state, first, "preview render").length)
    .toBe(1);
  await page
    .getByRole("button", { name: "Camera reference", exact: true })
    .click();
  const file = retainFile(
    state,
    first,
    "late-preview",
    Buffer.from("preview bytes"),
    "preview-r1",
    "preview",
  );
  const job = state.jobs
    .get(first)!
    .find((item) => item.operation === "preview render")!;
  job.status = "succeeded";
  job.result = {
    artifact: { artifactId: file.artifactId, revision: file.revision },
  };
  await jobRow(page, job.requestId)
    .getByRole("button", { name: "Check status" })
    .click();
  await expect(jobRow(page, job.requestId)).toContainText("succeeded");
  const preview = page.locator("[data-photo-editor-preview-image]");
  const reference = page.locator("[data-photo-editor-reference-image]");
  await expect(preview).toBeHidden();
  await expect(reference).toBeVisible();
  await expect(reference).toHaveJSProperty("src", original!);
  await expect(page.locator("[data-stage] img")).toHaveAttribute(
    "src",
    original!,
  );
});

test("session Undo and Redo each perform and persist their own guarded edit operation", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const first = state.photos[0]!;
  await openWorkspace(page, state);
  await setExposure(page, "1");
  await expect.poll(() => state.edits.get(first)?.revision).toBe("r2");
  const actions = page.getByRole("group", { name: "Primary actions" });
  const undo = actions.getByRole("button", { name: "Undo", exact: true });
  const redo = actions.getByRole("button", { name: "Redo", exact: true });
  await expect(undo).toBeEnabled();
  await setExposure(page, "-1");
  await expect.poll(() => state.edits.get(first)?.revision).toBe("r3");
  await expect(page.locator("[data-photo-editor-status]")).toHaveText("Ready");
  await expect(undo).toBeEnabled();
  await expect(redo).toBeDisabled();
  const applied = commandsFor(state, first, "edit apply");
  expect(applied).toHaveLength(2);
  await undo.click();
  await expect.poll(() => state.edits.get(first)?.revision).toBe("r4");
  const reversed = commandsFor(state, first, "edit apply");
  expect(reversed).toHaveLength(3);
  const undoRequest = reversed[2]!.command.arguments;
  expect(undoRequest.requestId).not.toBe(
    applied[1]!.command.arguments.requestId,
  );
  expect(undoRequest.ifState).toBe("state-r3");
  expect((undoRequest.input as { changes: unknown[] }).changes).toEqual([
    {
      type: "set",
      instanceId: "exposure-1",
      parameterId: "exposure",
      value: 1,
    },
  ]);
  await expect(redo).toBeEnabled();
  const readsBeforeReopen = commandsFor(state, first, "edit show").length;
  await page.locator("[data-photo-tools-close]").click();
  await openPhotoToolsView(page, "edit");
  await expect
    .poll(() => commandsFor(state, first, "edit show").length)
    .toBeGreaterThan(readsBeforeReopen);
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toHaveValue(
    "1",
  );
  await expect(redo).toBeEnabled();
  await redo.click();
  await expect.poll(() => state.edits.get(first)?.revision).toBe("r5");
  const redone = commandsFor(state, first, "edit apply");
  expect(redone).toHaveLength(4);
  const redoRequest = redone[3]!.command.arguments;
  expect(redoRequest.requestId).not.toBe(undoRequest.requestId);
  expect(redoRequest.ifState).toBe("state-r4");
  expect((redoRequest.input as { changes: unknown[] }).changes).toEqual([
    {
      type: "set",
      instanceId: "exposure-1",
      parameterId: "exposure",
      value: -1,
    },
  ]);
  // Both reversals settled as ordinary saves, and the tool's retained history
  // was never relabelled as Undo or Redo.
  expect(commandsFor(state, first, "history restore")).toHaveLength(0);
  await expect(page.locator("[data-photo-editor-status]")).toHaveText("Ready");
  await page.reload();
  await openWorkspace(page, state);
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toHaveValue(
    "-1",
  );
});

test("a retained revision restore is its own operation and is never Undo or Redo", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  const first = state.photos[0]!;
  state.command = async (route, captured) => {
    if (captured.command.operation === "history restore") {
      expect(captured.command.arguments.ifState).toBe("state-r2");
      expect(captured.command.arguments.revision).toBe("r1");
      state.edits.set(first, editFixture(first, 0, "r3"));
      return json(route, { revision: "r3", stateToken: "state-r3" });
    }
    if (captured.command.operation !== "history list") return false;
    return json(route, {
      ...windowOf([
        {
          revision: "r1",
          createdAt: "2026-10-28T00:00:00Z",
          summary: "confirmed before any change",
        },
      ]),
    });
  };
  await openWorkspace(page, state);
  await setExposure(page, "1");
  await expect.poll(() => state.edits.get(first)?.revision).toBe("r2");
  const actions = page.getByRole("group", { name: "Primary actions" });
  await expect(
    actions.getByRole("button", { name: "Undo", exact: true }),
  ).toBeEnabled();
  const revisions = page.locator("[data-workspace-revisions]");
  await revisions.locator("summary").click();
  const restore = revisions.getByRole("button", {
    name: /Restore retained revision r1/,
  });
  await expect(restore).toBeVisible();
  await restore.click();
  await expect
    .poll(() => commandsFor(state, first, "history restore").length)
    .toBe(1);
  const restoreRequest = commandsFor(state, first, "history restore")[0]!;
  expect(restoreRequest.command.arguments.ifState).toBe("state-r2");
  expect(restoreRequest.command.arguments.revision).toBe("r1");
  expect(restoreRequest.command.arguments.requestId).toBeTruthy();
  await expect.poll(() => state.edits.get(first)?.revision).toBe("r3");
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toHaveValue(
    "0",
  );
  // A retained revision restore clears the session's reversible actions: it is
  // a separate history action, not a session reversal.
  await expect(
    actions.getByRole("button", { name: "Undo", exact: true }),
  ).toBeDisabled();
  await expect(
    actions.getByRole("button", { name: "Redo", exact: true }),
  ).toBeDisabled();
});

test("Library progress, the Original and the editing tool are disclosed as independent facts", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  await openWorkspace(page, state);
  const facts = page.locator("[data-workspace-facts]");
  await expect(facts.locator('[data-fact="Library"]')).toContainText(
    "The Library scan is not running.",
  );
  await expect(facts.locator('[data-fact="Original input"]')).toContainText(
    "is the selected input for this edit",
  );
  await expect(facts.locator('[data-fact="Editing tool"]')).toContainText(
    "ready at revision r1",
  );
  await expect(facts.locator('[data-fact="Saved state"]')).toContainText(
    "confirmed at revision r1",
  );
  await expect(facts.locator('[data-fact="Preview"]')).toContainText(
    "No Preview has been rendered",
  );
  await expect(facts.locator('[data-fact="Processing jobs"]')).toContainText(
    "No processing job is running",
  );
  // The primary action strip holds save state, session Undo, session Redo,
  // Preview and Export as peers; Files, jobs, catalogs, resources and raw
  // technical details are named supporting disclosures instead.
  const actions = page.locator("[data-workspace-editor-actions]");
  for (const name of ["Undo", "Redo", "Preview", "Export"])
    await expect(
      actions.getByRole("button", { name, exact: true }),
    ).toBeVisible();
  await expect(page.locator("[data-workspace-jobs]")).toBeAttached();
  await expect(page.locator("[data-workspace-catalog]")).toBeAttached();
  await expect(page.locator("[data-workspace-files]")).toBeAttached();
  await expect(
    page.locator("details", { has: page.locator("[data-workspace-details]") }),
  ).toBeAttached();
});
