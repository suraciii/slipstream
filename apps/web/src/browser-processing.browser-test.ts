import { expect, test } from "@playwright/test";
import { copyFile, writeFile } from "node:fs/promises";
import { extname, join } from "node:path";
import { fixtureFetch } from "./browser-server.js";
import {
  setupBrowserSmoke,
  sample,
  processingEnvironmentOverrides,
  missingProcessingEnvironment,
  fixture,
  writePhotos,
  server,
  browseIds,
  originalSnapshot,
  readableRegularFile,
} from "./browser-test-support/fixtures.js";
import {
  startReview,
  openPhotoToolsView,
} from "./browser-test-support/surfaces.js";
import {
  setupWorkspace,
  json,
  retainFile,
} from "./browser-test-support/workspace-edit-fixtures.js";
setupBrowserSmoke();

test("Edit prepares a real input without initializing or rendering a tool", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const operations: string[] = [];
  page.on("request", (request) => {
    if (request.url().endsWith("/darktable/commands")) {
      const body = request.postDataJSON() as { operation?: string } | null;
      operations.push(String(body?.operation));
    }
  });
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await openPhotoToolsView(page, "edit");
  await expect(page.locator("[data-photo-editor-input]")).toContainText(
    "input/",
  );
  await expect(
    page.getByRole("button", { name: "Edit with darktable", exact: true }),
  ).toBeVisible();
  await page.locator("[data-workspace-files] summary").click();
  await expect(page.locator("[data-workspace-file-groups]")).toContainText(
    "Inputs",
  );
  expect(operations.every((operation) => operation === "modules list")).toBe(
    true,
  );
});

for (const viewport of [
  { width: 375, height: 667 },
  { width: 390, height: 844 },
]) {
  test(`no-edit reference and available tool share the first view at ${viewport.width}×${viewport.height}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const state = await setupWorkspace(page);
    for (const workspace of state.workspaces.values()) workspace.edits = [];
    await page.route("**/api/processing/modules", (route) =>
      json(route, { error: "processing registry unavailable" }, 503),
    );
    await startReview(page, state.url, "All Photos");
    const review = page.locator("[data-stage] img");
    await expect(review).toBeVisible();
    const reviewSource = await review.getAttribute("src");
    await openPhotoToolsView(page, "edit");
    const reference = page.locator("[data-photo-editor-reference-image]");
    const create = page.getByRole("button", {
      name: "Edit with darktable",
      exact: true,
    });
    await expect(reference).toBeVisible();
    await expect(reference).toHaveJSProperty("src", reviewSource!);
    await expect(create).toBeEnabled();
    await expect(
      page.locator("[data-photo-editor-render-status]"),
    ).toContainText("Camera reference");
    await expect(page.locator("[data-photo-editor-input]")).toContainText(
      "input",
    );
    const boxes = await Promise.all([
      reference.boundingBox(),
      create.boundingBox(),
    ]);
    expect(
      boxes.every(
        (box) => box && box.y >= 0 && box.y + box.height <= viewport.height,
      ),
    ).toBe(true);
    const controls = await page.locator(".photo-editor-controls").boundingBox();
    expect(
      controls &&
        boxes[1] &&
        boxes[1].y + boxes[1].height <= controls.y + controls.height,
    ).toBeTruthy();
    expect(
      state.commands.some(
        (entry) =>
          entry.command.operation === "preview render" ||
          entry.command.operation === "edit create",
      ),
    ).toBe(false);
    await page.evaluate(() => {
      document.documentElement.style.fontSize = "200%";
    });
    expect(
      await page.evaluate(
        () =>
          document.documentElement.scrollWidth <=
          document.documentElement.clientWidth,
      ),
    ).toBe(true);
  });
}

test("saved edit never silently substitutes camera reference for its missing result", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  state.command = async (route, captured) => {
    if (captured.command.operation !== "preview render") return false;
    await json(
      route,
      {
        error: {
          code: "capacity_unavailable",
          effect: "none",
          message:
            "Preview capacity unavailable. Try again when capacity is available.",
        },
      },
      503,
    );
    return true;
  };
  await startReview(page, state.url, "All Photos");
  await openPhotoToolsView(page, "edit");
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toBeVisible();
  const reference = page.locator("[data-photo-editor-reference-image]");
  await expect(reference).toBeHidden();
  await expect(page.locator("[data-photo-editor-image-empty]")).toContainText(
    "No Edit Preview",
  );
  expect(
    state.commands.some(
      (entry) => entry.command.operation === "preview render",
    ),
  ).toBe(false);
  await page.getByRole("button", { name: "Preview", exact: true }).click();
  await expect(page.locator("[data-workspace-error]")).toContainText(
    "Preview capacity unavailable",
  );
  await expect(reference).toBeHidden();
  await page
    .getByRole("button", { name: "Camera reference", exact: true })
    .click();
  await expect(reference).toBeVisible();
  await page.getByRole("button", { name: "Current edit", exact: true }).click();
  await expect(reference).toBeHidden();
  await expect(page.locator("[data-photo-editor-image-empty]")).toContainText(
    "No Edit Preview",
  );
  expect(
    state.commands.filter(
      (entry) => entry.command.operation === "preview render",
    ),
  ).toHaveLength(1);
});

test("unknown tool discovery is retryable without blocking retained downloads", async ({
  page,
}) => {
  const state = await setupWorkspace(page);
  for (const workspace of state.workspaces.values()) workspace.edits = [];
  retainFile(state, state.photos[0]!, "retained-output", Buffer.from("output"));
  let availability: "unknown" | "unavailable" | "ready" = "unknown";
  await page.route("**/api/darktable/commands", (route) =>
    availability === "ready"
      ? json(route, {
          bundleId: "fixture-darktable",
          schemaVersion: "v1",
          items: [],
          total: 0,
          nextCursor: null,
        })
      : json(
          route,
          {
            error: {
              code:
                availability === "unavailable"
                  ? "engine_unavailable"
                  : "unknown_result",
              message:
                availability === "unavailable"
                  ? "Engine missing"
                  : "Check failed",
              effect: "none",
            },
          },
          503,
        ),
  );
  await startReview(page, state.url, "All Photos");
  await openPhotoToolsView(page, "edit");
  const create = page.getByRole("button", {
    name: "Edit with darktable",
    exact: true,
  });
  await expect(create).toBeDisabled();
  await expect(page.locator("[data-workspace-selection]")).toContainText(
    "could not be checked",
  );
  await page.locator("[data-workspace-files] summary").click();
  await expect(
    page.locator('[data-artifact-id="retained-output"] a', {
      hasText: "Download",
    }),
  ).toBeVisible();
  availability = "unavailable";
  await page.getByRole("button", { name: "Retry tool check" }).click();
  await expect(page.locator("[data-workspace-selection]")).toContainText(
    "Engine missing",
  );
  await expect(create).toBeDisabled();
  availability = "ready";
  await page.getByRole("button", { name: "Retry tool check" }).click();
  await expect(create).toBeEnabled();
  expect(
    state.commands.some((entry) => entry.command.operation === "edit create"),
  ).toBe(false);
});

test("real-processing: native settings save, explicit preview and export remain in workspace after reopen", async ({
  page,
}) => {
  const missing = missingProcessingEnvironment();
  test.skip(
    missing.length > 0,
    `Set ${missing.join(", ")} for native processing smoke`,
  );
  const cameraSample = sample!;
  test.skip(
    !(await readableRegularFile(cameraSample)),
    "RAW sample must be a readable regular file",
  );
  test.setTimeout(900_000);
  const { base, root } = await fixture();
  const original = join(root, `camera${extname(cameraSample)}`);
  const sidecar = join(root, "camera.xmp");
  await copyFile(cameraSample, original);
  await writeFile(sidecar, '<x:xmpmeta xmlns:x="adobe:ns:meta/"/>');
  const before = await originalSnapshot(original),
    xmpBefore = await originalSnapshot(sidecar);
  const running = await server(base, root, processingEnvironmentOverrides);
  const [photoId] = await browseIds(running.url);
  if (!photoId) throw new Error("Smoke requires a Photo");
  await startReview(page, running.url, "All Photos");
  await openPhotoToolsView(page, "edit");
  await page
    .getByRole("button", { name: "Edit with darktable", exact: true })
    .click();
  await expect(
    page.locator("[data-workspace-controls] [data-instance-id]").first(),
  ).toBeVisible({ timeout: 300_000 });
  await expect(page.locator("[data-photo-editor-status]")).toHaveText("Ready", {
    timeout: 300_000,
  });
  const show = await fixtureFetch(
    `${running.url}/api/photos/${photoId}/darktable/commands`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        operation: "edit show",
        editId: null,
        arguments: {},
      }),
    },
  );
  expect(show.ok).toBe(true);
  const edit = (await show.json()) as {
    revision: string;
    editId: string;
    instances: {
      items: {
        instanceId: string;
        moduleId: string;
        availability: { write: boolean };
      }[];
    };
  };
  // Select a native-discovered numeric field without assuming an operation ID.
  let selector: string | undefined;
  for (const instance of edit.instances.items) {
    if (!instance.availability.write) continue;
    const described = await fixtureFetch(
      `${running.url}/api/photos/${photoId}/darktable/commands`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          operation: "modules describe",
          editId: edit.editId,
          arguments: { moduleId: instance.moduleId },
        }),
      },
    );
    const descriptor = (await described.json()) as {
      parameters?: {
        properties?: Record<
          string,
          {
            type?: string;
            minimum?: number;
            maximum?: number;
            readOnly?: boolean;
            availability?: { write: boolean };
          }
        >;
      };
    };
    const entry = Object.entries(descriptor.parameters?.properties ?? {}).find(
      ([, schema]) =>
        schema.type === "number" &&
        !schema.readOnly &&
        schema.availability?.write !== false,
    );
    if (!entry) continue;
    const candidateSelector = `[data-instance-id="${instance.instanceId}"] [data-parameter-id="${entry[0]}"] input[type="number"]`;
    const disclosure = page.locator(
      `details[data-instance-id="${instance.instanceId}"]`,
    );
    if (!(await disclosure.evaluate((node) => node.hasAttribute("open"))))
      await disclosure.locator("summary").first().click();
    const control = page.locator(candidateSelector).first();
    if (!(await control.isVisible()) || !(await control.isEnabled())) continue;
    const current = Number(await control.inputValue());
    const candidate = Math.min(
      entry[1].maximum ?? Infinity,
      Math.max(entry[1].minimum ?? -Infinity, current + 0.1),
    );
    if (candidate === current) continue;
    await control.fill(String(candidate));
    await control.dispatchEvent("change");
    selector = candidateSelector;
    await expect
      .poll(
        async () => {
          const response = await fixtureFetch(
            `${running.url}/api/photos/${photoId}/darktable/commands`,
            {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({
                operation: "edit show",
                editId: edit.editId,
                arguments: {},
              }),
            },
          );
          const current = (await response.json()) as { revision: string };
          return current.revision;
        },
        { timeout: 300_000 },
      )
      .not.toBe(edit.revision);
    break;
  }
  expect(selector).toBeDefined();
  await expect(page.locator("[data-photo-editor-status]")).toHaveText(
    /Ready|Rendering preview/,
    { timeout: 300_000 },
  );
  await page.getByRole("button", { name: "Preview", exact: true }).click();
  await expect(page.locator("[data-photo-editor-preview-image]")).toBeVisible({
    timeout: 300_000,
  });
  await expect(
    page.locator("[data-photo-editor-preview-image]"),
  ).toHaveAttribute(
    "src",
    /\/api\/workspaces\/.+\/files\/.+\/bytes\?revision=/,
    { timeout: 300_000 },
  );
  await expect(
    page.locator("[data-photo-editor-reference-image]"),
  ).toBeHidden();
  await page.getByRole("button", { name: "Export", exact: true }).click();
  await page.locator("[data-workspace-files] summary").click();
  await expect(
    page
      .locator("[data-workspace-jobs] article")
      .filter({ hasText: "export create" })
      .last(),
  ).toContainText("succeeded", { timeout: 300_000 });
  await expect(
    page
      .locator("[data-workspace-file-groups] section")
      .filter({
        has: page.getByRole("heading", { name: "Outputs", exact: true }),
      })
      .getByRole("link", { name: "Download" })
      .first(),
  ).toBeVisible({ timeout: 300_000 });
  await page.reload();
  await openPhotoToolsView(page, "edit");
  await expect(
    page.getByRole("combobox", { name: "Saved edit", exact: true }),
  ).toHaveValue(edit.editId, { timeout: 300_000 });
  expect(await originalSnapshot(original)).toEqual(before);
  expect(await originalSnapshot(sidecar)).toEqual(xmpBefore);
});
