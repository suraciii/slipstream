import { expect, test } from "@playwright/test";
import { copyFile, open, writeFile } from "node:fs/promises";
import { extname, dirname, join, parse } from "node:path";
import { fixtureFetch } from "./browser-server.js";
import {
  setupBrowserSmoke,
  sample,
  processingEnvironmentOverrides,
  missingProcessingEnvironment,
  fixture,
  writePhotos,
  server,
  post,
  browseIds,
  originalSnapshot,
  optionalOriginalSnapshot,
  readableRegularFile,
} from "./browser-test-support/fixtures.js";
import {
  startReview,
  openPhotoToolsView,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

/// A deployment without processing leaves the Edit workspace reachable for
/// saved settings and retained downloads, but offers no processing actions.
test("the Edit surface explains a deployment without processing and attempts no processing work", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  // Processing work is a submitted output or a rendered Edit Preview. Reading
  // the Photo's retained outputs is a retained-artifact read, not work.
  const processing: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    const submitted = request.method() === "POST" && path.endsWith("/exports");
    if (submitted || path.includes("/edit-preview/"))
      processing.push(`${request.method()} ${path}`);
  });
  const running = await server(base, root);
  await startReview(page, running.url, "All Photos");
  await openPhotoToolsView(page, "edit");
  await page.locator(".photo-editor-details summary").click();
  await expect(page.locator("[data-photo-editor-stage='film']")).toBeDisabled();
  await expect(
    page.locator("[data-photo-editor-stage='camera']"),
  ).toBeEnabled();
  await expect(page.locator("[data-photo-editor-stage='develop']")).toHaveCount(
    0,
  );
  const originalReference = page.locator("[data-photo-editor-stage='camera']");
  await originalReference.click();
  await expect(originalReference).toHaveAttribute("aria-pressed", "true");
  await originalReference.click();
  await expect(originalReference).toHaveAttribute("aria-pressed", "false");
  await expect(page.locator("[data-photo-editor-exposure]")).toBeDisabled();
  // Comparison needs an edited rendition; Original reference stays independent.
  await expect(page.locator("[data-photo-editor-compare]")).toBeDisabled();
  await expect(
    page.locator(
      '[data-editor-output="development-tiff"] [data-output-action="submit"]',
    ),
  ).toBeDisabled();
  expect(processing).toEqual([]);
});

/// The opt-in counterpart of the refusal above: a deployment with an admitted
/// local bundle carries one RAW Original through a real Photo Edit Recipe. The
/// Photographer's own surface proves the saved exposure, the reopened recipe,
/// the baseline comparison, and the downloaded Development TIFF, and the
/// Original Files stay untouched.
test("real-processing: autosaves an exposure, reopens it, compares the baseline, and downloads the Development TIFF", async ({
  page,
}) => {
  const missingEnvironment = missingProcessingEnvironment();
  test.skip(
    missingEnvironment.length > 0,
    `Set ${missingEnvironment.join(", ")} for the RAW processing smoke`,
  );
  const cameraSample = sample!;
  // This scenario runs outside the RAW gate, which validates the sample for
  // its own runs, so the explicit path is checked here and reported as a skip
  // rather than as a read failure.
  test.skip(
    !(await readableRegularFile(cameraSample)),
    `SLIPSTREAM_RAW_SAMPLE must identify a readable regular file: ${cameraSample}`,
  );
  // A cold development of a 61 MP Original takes about a minute, and the
  // scenario renders twice: once as the comparison and once as the TIFF.
  test.setTimeout(900_000);
  const sourceBefore = await originalSnapshot(cameraSample);
  const sourceSidecarPath = join(
    dirname(cameraSample),
    `${parse(cameraSample).name}.xmp`,
  );
  const sourceSidecarBefore = await optionalOriginalSnapshot(sourceSidecarPath);
  const { base, root } = await fixture();
  const raw = join(root, `camera${extname(cameraSample)}`);
  await copyFile(cameraSample, raw);
  const isolatedSidecarPath = join(root, "camera.xmp");
  if (sourceSidecarBefore) {
    await copyFile(sourceSidecarPath, isolatedSidecarPath);
  } else {
    await writeFile(
      isolatedSidecarPath,
      `<?xpacket begin="\uFEFF" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Slipstream browser smoke">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="3"/>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
`,
    );
  }
  const copiedSidecarBefore = await originalSnapshot(isolatedSidecarPath);
  const copiedBefore = await originalSnapshot(raw);
  const running = await server(base, root, processingEnvironmentOverrides);
  await startReview(page, running.url, "All Photos");
  await openPhotoToolsView(page, "edit");
  // The application admits this Photo through its local native bundle, and
  // the RAW Original is a supported source class.
  await expect(page.locator("[data-photo-editor-processing]")).toHaveText(
    "Ready",
    { timeout: 60_000 },
  );
  await expect(page.locator("[data-photo-editor-support]")).toHaveText("Ready");
  const exposure = page.locator("[data-photo-editor-exposure]");
  await expect(exposure).toBeEnabled();
  // One committed adjustment is one autosave. The value and its events are one
  // gesture, so a render between them cannot clear the draft the change
  // commits, and the service's own acknowledgement is the evidence: a 200
  // whose body is not an acknowledgement leaves the local draft in place, and
  // a reload could then present that draft instead of the saved recipe.
  const saved = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname.endsWith("/edit-recipe"),
  );
  await exposure.evaluate((element: HTMLInputElement) => {
    element.value = "0.5";
    element.dispatchEvent(new Event("input", { bubbles: true }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
  });
  const acknowledgement = await saved;
  expect(acknowledgement.status()).toBe(200);
  const committed = (await acknowledgement.json()) as {
    outcome?: unknown;
    recipeVersion?: unknown;
    sourceRevision?: unknown;
  };
  expect(committed.outcome).toBe("saved");
  expect(typeof committed.recipeVersion).toBe("string");
  expect(typeof committed.sourceRevision).toBe("string");
  await expect(page.locator("[data-photo-editor-exposure-value]")).toHaveText(
    "0.500 EV",
  );
  // Reopening the Photo presents the saved recipe rather than a fresh one: the
  // service's acknowledgement above is what the reopened recipe reflects.
  await page.reload();
  await openPhotoToolsView(page, "edit");
  await expect(page.locator("[data-photo-editor-exposure-value]")).toHaveText(
    "0.500 EV",
  );
  // A second client moves the saved recipe. Autosave stops with an explicit
  // conflict instead of overwriting the other client's recipe, and the saved
  // side the surface adopts is the service's, not this client's older copy.
  const photoId = new URL(acknowledgement.url()).pathname.split("/")[3];
  const secondClient = await post(
    running.url,
    `/api/photos/${photoId}/edit-recipe`,
    {
      requestId: "browser-smoke-second-client",
      expectedRecipeVersion: committed.recipeVersion,
      expectedSourceRevision: committed.sourceRevision,
      settings: { exposureEv: 0.25, whiteBalance: { mode: "as-shot" } },
    },
  );
  expect(secondClient.status).toBe(200);
  await exposure.evaluate((element: HTMLInputElement) => {
    element.value = "0.75";
    element.dispatchEvent(new Event("input", { bubbles: true }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
  });
  const conflict = page.locator("[data-photo-editor-conflict]");
  await expect(conflict).toBeVisible();
  await expect(
    page.locator("[data-photo-editor-conflict-message]"),
  ).toContainText("The saved edit changed elsewhere. Autosave stopped;");
  await page.locator("[data-photo-editor-use-saved]").click();
  await expect(conflict).toBeHidden();
  await expect(page.locator("[data-photo-editor-exposure-value]")).toHaveText(
    "0.250 EV",
  );
  // The baseline comparison develops the same Original without the saved
  // exposure, so its own rendition is what the note and the image present.
  // A cold native development can take several minutes; keep the admitted
  // request alive while the UI polls its retained rendition.
  const previewNote = page.locator("[data-photo-editor-preview-note]");
  const compare = page.locator("[data-photo-editor-compare]");
  if ((await compare.getAttribute("aria-pressed")) !== "true") {
    await compare.click();
  }
  await expect(compare).toHaveAttribute("aria-pressed", "true");
  await expect(previewNote).toHaveText(
    /Comparison \d+×\d+: the unadjusted rendering\. The current settings are unchanged\./,
    { timeout: 300_000 },
  );
  // The note alone could describe a rendition the surface never presented, so
  // the comparison's own image is what the assertion ends on.
  const comparison = page.locator("[data-photo-editor-preview-image]");
  await expect(comparison).toBeVisible();
  expect(
    await comparison.evaluate(
      (image: HTMLImageElement) => image.complete && image.naturalWidth > 0,
    ),
  ).toBe(true);
  // The Development TIFF is the deployment's bounded work: the submission
  // settles, and the retained artifact becomes downloadable.
  const tiffCard = page.locator('[data-editor-output="development-tiff"]');
  await page
    .locator(
      '[data-editor-output="development-tiff"] [data-output-action="submit"]',
    )
    .click();
  const downloadButton = tiffCard.locator('[data-output-action="download"]');
  await expect(downloadButton).toBeVisible({ timeout: 300_000 });
  const pending = page.waitForEvent("download");
  await downloadButton.click();
  // The downloaded artifact is the observable completion; avoid pinning
  // incidental output-state wording.
  const artifact = await pending;
  const artifactPath = await artifact.path();
  expect(artifactPath).not.toBeNull();
  const handle = await open(artifactPath, "r");
  try {
    const header = Buffer.alloc(4);
    await handle.read(header, 0, header.byteLength, 0);
    // Either byte order of the TIFF signature: `II*\0` or `MM\0*`.
    expect([
      header.equals(Buffer.from([0x49, 0x49, 0x2a, 0x00])),
      header.equals(Buffer.from([0x4d, 0x4d, 0x00, 0x2a])),
    ]).toContain(true);
    const size = (await handle.stat()).size;
    expect(size).toBeGreaterThan(1_000_000);
  } finally {
    await handle.close();
  }
  // Both Original Files are unchanged: neither the sample nor its copy moved.
  expect(await originalSnapshot(cameraSample)).toEqual(sourceBefore);
  expect(await originalSnapshot(raw)).toEqual(copiedBefore);
  expect(await optionalOriginalSnapshot(sourceSidecarPath)).toEqual(
    sourceSidecarBefore,
  );
  expect(await originalSnapshot(isolatedSidecarPath)).toEqual(
    copiedSidecarBefore,
  );
});

/// The authenticated browser client can carry the composable contract through
/// discovery, a selected-step Preview, an explicit Export, immutable artifact
/// provenance, and a later step's explicit artifact binding. The native bundle
/// is an operator-supplied qualification prerequisite; this smoke only records
/// the delivered HTTP/Web behavior and never treats it as production
/// qualification.
test("real-processing: composes a selected step, exports its artifact, and binds it explicitly", async ({
  page,
}) => {
  const missingEnvironment = missingProcessingEnvironment();
  test.skip(
    missingEnvironment.length > 0,
    `Set ${missingEnvironment.join(", ")} for the composable processing smoke`,
  );
  const cameraSample = sample!;
  test.skip(
    !(await readableRegularFile(cameraSample)),
    `SLIPSTREAM_RAW_SAMPLE must identify a readable regular file: ${cameraSample}`,
  );
  test.setTimeout(900_000);
  const { base, root } = await fixture();
  await copyFile(cameraSample, join(root, `camera${extname(cameraSample)}`));
  const running = await server(base, root, processingEnvironmentOverrides);
  await page.goto(running.url);
  const [photoId] = await browseIds(running.url);
  if (!photoId) throw new Error("The composable smoke needs one Photo");

  const modules = await fixtureFetch(`${running.url}/api/processing/modules`);
  expect(modules.status).toBe(200);
  const moduleDocument = (await modules.json()) as {
    modules: Array<{ id: { name: string } }>;
  };
  expect(moduleDocument.modules.map((module) => module.id.name)).toEqual([
    "darktable",
    "spektrafilm",
  ]);

  const current = await fixtureFetch(
    `${running.url}/api/photos/${encodeURIComponent(photoId)}/processing-recipe`,
  );
  expect(current.status).toBe(200);
  const currentDocument = (await current.json()) as {
    sourceRevision: string;
    recipe: { revision: string } | null;
  };
  const sourceRevision = currentDocument.sourceRevision;
  const firstStep = {
    stepId: "develop-1",
    module: "darktable",
    input: { kind: "original", photoId, sourceRevision },
    parameters: {
      schemaVersion: "darktable-params-1",
      tree: {
        stack: [],
        output: {
          format: "tiff",
          precisionBits: 32,
          colorSpace: "prophoto-rgb",
          transferFunction: "linear",
        },
      },
    },
  };
  const recipe = await post(
    running.url,
    `/api/photos/${encodeURIComponent(photoId)}/processing-recipe`,
    {
      requestId: "browser-composable-recipe-1",
      expectedRecipeRevision: currentDocument.recipe?.revision ?? null,
      expectedSourceRevision: sourceRevision,
      currentStepId: "develop-1",
      steps: [firstStep],
    },
  );
  expect(recipe.status).toBe(201);
  const saved = (await recipe.json()) as {
    recipe: { revision: string };
  };

  const preview = await fixtureFetch(
    `${running.url}/api/photos/${encodeURIComponent(photoId)}/processing-preview/develop-1`,
  );
  expect(preview.status).toBe(200);
  expect(preview.headers.get("content-type")).toBe("image/png");
  expect(preview.headers.get("slipstream-processing-preview-module")).toBe(
    "darktable",
  );
  expect(preview.headers.get("slipstream-processing-preview-identity")).toMatch(
    /^[0-9a-f]{64}$/,
  );
  expect((await preview.arrayBuffer()).byteLength).toBeGreaterThan(0);

  const exported = await post(
    running.url,
    `/api/photos/${encodeURIComponent(photoId)}/processing-exports`,
    {
      requestId: "browser-composable-export-1",
      stepId: "develop-1",
      expectedRecipeRevision: saved.recipe.revision,
      expectedSourceRevision: sourceRevision,
    },
  );
  expect(exported.status).toBe(201);
  const exportDocument = (await exported.json()) as {
    artifact: {
      artifactId: string;
      outputContract: Record<string, unknown>;
    };
  };
  const artifactId = exportDocument.artifact.artifactId;
  const provenance = await fixtureFetch(
    `${running.url}/api/processing-artifacts/${encodeURIComponent(artifactId)}`,
  );
  expect(provenance.status).toBe(200);
  const provenanceDocument = (await provenance.json()) as {
    artifactId: string;
    input: { binding: { kind: string; photoId: string } };
  };
  expect(provenanceDocument.artifactId).toBe(artifactId);
  expect(provenanceDocument.input.binding).toEqual({
    kind: "original",
    photoId,
    sourceRevision,
  });
  const bytes = await fixtureFetch(
    `${running.url}/api/processing-artifacts/${encodeURIComponent(artifactId)}/bytes`,
  );
  expect(bytes.status).toBe(200);
  expect(bytes.headers.get("slipstream-artifact-module")).toBe("darktable");
  expect((await bytes.arrayBuffer()).byteLength).toBeGreaterThan(0);

  const handoff = await post(
    running.url,
    `/api/photos/${encodeURIComponent(photoId)}/processing-recipe`,
    {
      requestId: "browser-composable-handoff-1",
      expectedRecipeRevision: saved.recipe.revision,
      expectedSourceRevision: sourceRevision,
      currentStepId: "film-1",
      steps: [
        firstStep,
        {
          stepId: "film-1",
          module: "spektrafilm",
          input: {
            kind: "artifact",
            artifactId,
            contract: exportDocument.artifact.outputContract,
          },
          parameters: {
            schemaVersion: "spektrafilm-params-1",
            tree: { scanner: {} },
          },
        },
      ],
    },
  );
  expect(handoff.status).toBe(201);
  const handoffDocument = (await handoff.json()) as {
    recipe: {
      currentStepId: string;
      steps: Array<{ input: { kind: string } }>;
    };
  };
  expect(handoffDocument.recipe.currentStepId).toBe("film-1");
  expect(handoffDocument.recipe.steps[1]?.input.kind).toBe("artifact");
});
