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
  await expect(
    page.locator("[data-photo-editor-advanced]"),
  ).not.toHaveAttribute("open", "");
  const reference = page.locator("[data-photo-editor-camera-reference]");
  await expect(reference).toBeEnabled();
  await expect(
    page.locator("[data-photo-editor-export-submit]"),
  ).toBeDisabled();
  await expect(page.locator("[data-photo-editor-preview-image]")).toBeHidden();
  await expect(page.locator("[data-photo-editor-render-status]")).toContainText(
    "no Edit State",
  );
  await expect(page.locator("[data-photo-editor-preview-note]")).toBeHidden();
  const dialog = page.locator("[data-photo-tools]");
  const region = page.locator(".photo-editor-image-region");
  const controls = page.locator(".photo-editor-controls");
  for (const viewport of [
    { width: 1280, height: 800 },
    { width: 768, height: 1024 },
    { width: 390, height: 844 },
    { width: 667, height: 375 },
  ]) {
    await page.setViewportSize(viewport);
    await expect(dialog.locator("[data-photo-tools-close]")).toBeInViewport();
    await expect(region).toBeInViewport();
    expect(
      (await page.locator(".photo-editor-image-frame").boundingBox())?.height,
    ).toBeGreaterThanOrEqual(100);
    const geometry = await dialog.evaluate((node) => {
      const image = node
        .querySelector(".photo-editor-image-region")!
        .getBoundingClientRect();
      const controls = node
        .querySelector(".photo-editor-controls")!
        .getBoundingClientRect();
      return {
        fits: node.scrollWidth <= node.clientWidth,
        image: { x: image.x, bottom: image.bottom, right: image.right },
        controls: { x: controls.x, y: controls.y },
      };
    });
    expect(geometry.fits).toBe(true);
    if (viewport.width >= 900 || viewport.height <= 500)
      expect(geometry.image.right).toBeLessThanOrEqual(geometry.controls.x);
    else expect(geometry.image.bottom).toBeLessThanOrEqual(geometry.controls.y);
    await expect(page.locator("[data-photo-editor-undo]")).toBeVisible();
    expect(
      await page
        .locator("[data-photo-editor-undo]")
        .evaluate((node) => node.getBoundingClientRect().height),
    ).toBeGreaterThanOrEqual(44);
    await page.locator("[data-photo-editor-advanced] > summary").click();
    const before = await region.boundingBox();
    await controls.evaluate((node) => {
      node.scrollTop = node.scrollHeight;
    });
    expect(await region.boundingBox()).toEqual(before);
    await expect(dialog.locator("[data-photo-tools-close]")).toBeInViewport();
    expect(
      await controls.evaluate((node) => node.scrollWidth <= node.clientWidth),
    ).toBe(true);
    await page.locator("[data-photo-editor-advanced] > summary").click();
    await controls.evaluate((node) => {
      node.scrollTop = 0;
    });
  }
  await page.setViewportSize({ width: 390, height: 844 });
  await page.evaluate(() => {
    document.documentElement.style.fontSize = "200%";
  });
  await expect(dialog.locator("[data-photo-tools-close]")).toBeInViewport();
  await page.locator("[data-photo-editor-refresh]").scrollIntoViewIfNeeded();
  await expect(page.locator("[data-photo-editor-refresh]")).toBeInViewport();
  expect(
    await controls.evaluate((node) => node.scrollWidth <= node.clientWidth),
  ).toBe(true);
  await page.evaluate(() => {
    document.documentElement.style.fontSize = "";
  });
  await reference.click();
  await expect(reference).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("[data-photo-editor-render-status]")).toContainText(
    "Camera Preview",
  );
  const referenceImage = page.locator("[data-photo-editor-reference-image]");
  await expect(referenceImage).toBeVisible();
  await expect
    .poll(() =>
      referenceImage.evaluate(
        (node) =>
          node instanceof HTMLImageElement &&
          node.complete &&
          node.naturalWidth > 0,
      ),
    )
    .toBe(true);
  expect(await referenceImage.getAttribute("src")).toBe(
    await page.locator("[data-stage] img").first().getAttribute("src"),
  );
  await reference.click();
  await expect(reference).toHaveAttribute("aria-pressed", "false");
  await expect(page.locator("[data-photo-editor-preview-image]")).toBeHidden();
  await expect(referenceImage).toBeHidden();
  await expect(referenceImage).not.toHaveAttribute("src");
  await dialog.locator("[data-photo-tools-close]").click();
  await expect(dialog).toBeHidden();
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
  await expect(
    page.getByLabel("Current exposure (EV)", { exact: true }),
  ).toBeVisible();
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
  await expect(
    page.locator("[data-photo-editor-reference-image]"),
  ).toBeVisible();
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
