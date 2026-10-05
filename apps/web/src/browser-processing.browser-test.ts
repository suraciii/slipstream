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
  openPhotoEditorAdvanced,
} from "./browser-test-support/surfaces.js";
import { processingModuleDefaultsFixture } from "./browser-test-support/processing-fixtures.js";
setupBrowserSmoke();

test("an empty recipe shows no processing result and keeps Camera Preview separate", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const work: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (
      path.includes("processing-preview/") ||
      path.endsWith("/edit/preview") ||
      (request.method() === "POST" && path.endsWith("processing-exports"))
    )
      work.push(path);
  });
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await openPhotoToolsView(page, "edit");
  await expect(page.locator("[data-photo-editor-advanced]")).not.toHaveAttribute(
    "open",
    "",
  );
  const reference = page.locator("[data-photo-editor-camera-reference]");
  await expect(reference).toBeEnabled();
  await expect(
    page.locator("[data-photo-editor-export-submit]"),
  ).toBeDisabled();
  await expect(page.locator("[data-photo-editor-preview-image]")).toBeHidden();
  await reference.click();
  await expect(reference).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("[data-photo-editor-render-status]")).toContainText(
    "Camera Preview",
  );
  await reference.click();
  await expect(reference).toHaveAttribute("aria-pressed", "false");
  await expect(page.locator("[data-photo-editor-preview-image]")).toBeHidden();
  expect(work).toEqual([]);
});

test("real-processing: current Edit Preview, explicit Export, retained reopen, and explicit artifact input", async ({
  page,
}) => {
  const missing = missingProcessingEnvironment();
  test.skip(
    missing.length > 0,
    `Set ${missing.join(", ")} for the composable processing smoke`,
  );
  const cameraSample = sample!;
  test.skip(
    !(await readableRegularFile(cameraSample)),
    "SLIPSTREAM_RAW_SAMPLE must identify a readable regular file",
  );
  test.setTimeout(900_000);
  const { base, root } = await fixture();
  const original = join(root, `camera${extname(cameraSample)}`);
  const sidecar = join(root, "camera.xmp");
  await copyFile(cameraSample, original);
  await writeFile(sidecar, '<x:xmpmeta xmlns:x="adobe:ns:meta/"/>');
  const before = await originalSnapshot(original);
  const xmpBefore = await originalSnapshot(sidecar);
  const running = await server(base, root, processingEnvironmentOverrides);
  const [photoId] = await browseIds(running.url);
  if (!photoId) throw new Error("The smoke requires one Photo");
  const modules = await fixtureFetch(`${running.url}/api/processing/modules`);
  expect(modules.status).toBe(200);
  const darktable = processingModuleDefaultsFixture(
    await modules.json(),
    "darktable",
  );
  expect(darktable.names).toEqual(["darktable", "spektrafilm"]);
  const current = await fixtureFetch(
    `${running.url}/api/photos/${photoId}/processing-recipe`,
  );
  const read = (await current.json()) as {
    sourceRevision: string;
    recipe: { revision: string } | null;
  };
  const first = {
    stepId: "step-1",
    module: "darktable",
    input: { kind: "original", photoId, sourceRevision: read.sourceRevision },
    parameters: {
      schemaVersion: darktable.schemaVersion,
      tree: darktable.defaultTree,
    },
  };
  const seeded = await fixtureFetch(
    `${running.url}/api/photos/${photoId}/processing-recipe`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        requestId: "browser-step-1",
        expectedRecipeRevision: read.recipe?.revision ?? null,
        expectedSourceRevision: read.sourceRevision,
        currentStepId: first.stepId,
        steps: [first],
      }),
    },
  );
  expect(seeded.status).toBe(201);
  await startReview(page, running.url, "All Photos");
  await openPhotoToolsView(page, "edit");
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toBeVisible();
  await openPhotoEditorAdvanced(page);
  await expect(
    page.locator("[data-photo-editor-composable-steps]"),
  ).toContainText("step-1");
  await expect(page.locator("[data-photo-editor-preview-image]")).toBeVisible({
    timeout: 300_000,
  });
  const currentPreview = await page
    .locator("[data-photo-editor-preview-image]")
    .getAttribute("src");
  const compare = page.locator("[data-photo-editor-compare]");
  await compare.click();
  await expect(compare).toHaveAttribute("aria-pressed", "true", {
    timeout: 300_000,
  });
  await expect(
    page.locator("[data-photo-editor-preview-image]"),
  ).not.toHaveAttribute("src", currentPreview ?? "");
  await compare.click();
  await expect(compare).toHaveAttribute("aria-pressed", "false");
  await expect(
    page.locator("[data-photo-editor-preview-image]"),
  ).toHaveAttribute("src", currentPreview ?? "");
  const reference = page.locator("[data-photo-editor-camera-reference]");
  await reference.click();
  await expect(page.locator("[data-photo-editor-preview-image]")).toBeHidden();
  await reference.click();
  await expect(page.locator("[data-photo-editor-preview-image]")).toBeVisible();
  await page.locator("[data-photo-editor-export-submit]").click();
  await expect(
    page.locator("[data-photo-editor-composable-artifact-list]"),
  ).toContainText("Artifact", { timeout: 300_000 });
  const artifactResponse = await fixtureFetch(
    `${running.url}/api/photos/${photoId}/processing-exports`,
  );
  const artifactBody = (await artifactResponse.json()) as {
    artifacts: Array<{
      artifactId: string;
      outputContract: Record<string, unknown>;
    }>;
  };
  const artifact = artifactBody.artifacts[0];
  if (!artifact)
    throw new Error("The explicit Export must publish an artifact");
  await page.reload();
  await openPhotoToolsView(page, "edit");
  await expect(
    page.locator("[data-photo-editor-composable-artifact-list]"),
  ).toContainText(artifact.artifactId);
  await page
    .locator("[data-photo-editor-composable-module]")
    .selectOption("spektrafilm");
  await page
    .locator("[data-photo-editor-new-step-input]")
    .selectOption(`artifact:${artifact.artifactId}`);
  await page.locator("[data-photo-editor-composable-add]").click();
  await expect(
    page.locator("[data-photo-editor-composable-steps]"),
  ).toContainText("spektrafilm");
  await expect(
    page.locator("[data-photo-editor-composable-steps]"),
  ).toContainText(artifact.artifactId);
  expect(await originalSnapshot(original)).toEqual(before);
  expect(await originalSnapshot(sidecar)).toEqual(xmpBefore);
});
