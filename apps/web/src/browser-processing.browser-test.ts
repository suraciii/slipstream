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
  await expect(page.locator("[data-photo-editor-render-status]")).toContainText(
    "Edit preview has not been rendered",
  );
  await page.locator("[data-workspace-files] summary").click();
  await expect(page.locator("[data-workspace-file-groups]")).toContainText(
    "Inputs",
  );
  expect(operations).toEqual([]);
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
  await expect(page.getByLabel("Saved edit")).toHaveValue(edit.editId, {
    timeout: 300_000,
  });
  expect(await originalSnapshot(original)).toEqual(before);
  expect(await originalSnapshot(sidecar)).toEqual(xmpBefore);
});
