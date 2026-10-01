import { test, expect, type Page } from "@playwright/test";
import { createHash } from "node:crypto";
import { copyFile, mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  fixtureFetch,
  startBrowserServer,
  type BrowserServer,
} from "./browser-server.js";

let base: string;
let running: BrowserServer;

test.beforeEach(async ({ context }) => {
  base = await mkdtemp(join(tmpdir(), "slipstream-export-browser-"));
  const root = join(base, "originals");
  await mkdir(root);
  for (const name of ["001.jpg", "002.jpg"])
    await copyFile("apps/web/test-fixtures/review.jpg", join(root, name));
  running = await startBrowserServer({ base, root });
  const login = await context.request.post(
    `${running.url}/api/access/session`,
    {
      headers: { Origin: running.url },
      data: { token: running.token },
    },
  );
  expect(login.status()).toBe(204);
  await expect
    .poll(
      async () =>
        (
          (await (await fixtureFetch(`${running.url}/api/status`)).json()) as {
            state: string;
          }
        ).state,
    )
    .toBe("idle");
});

test.afterEach(async () => {
  await running?.close();
  if (base) await rm(base, { recursive: true, force: true });
});

async function openEdit(page: Page) {
  await page.locator("[data-dock-more]").click();
  await expect(page.locator("[data-photo-tools]")).toBeVisible();
  await page.locator("[data-photo-tools-entry='edit']").click();
  await expect(page.locator("[data-photo-tools-view='edit']")).toBeVisible();
}

async function navigate(page: Page, direction: "Next" | "Previous") {
  await page.locator("[data-photo-tools-close]").click();
  await page.getByRole("button", { name: direction, exact: true }).click();
  await openEdit(page);
}

for (const lostResponse of [
  "connection",
  "server-unknown",
  "server-error",
] as const) {
  test(`an uncertain Export (${lostResponse}) keeps its Photo's identity without blocking another Photo`, async ({
    page,
  }) => {
    const browse = await fixtureFetch(`${running.url}/api/browse`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Origin: running.url },
      body: JSON.stringify({ source: "library" }),
    });
    const opened = (await browse.json()) as { token: string };
    const listing = await fixtureFetch(
      `${running.url}/api/browse/${opened.token}?start=0&limit=60`,
    );
    const { photos } = (await listing.json()) as {
      photos: Array<{ id: string }>;
    };
    const [first, second] = photos.map((photo) => photo.id);
    if (!first || !second) throw new Error("Expected two Photos");
    await page.route("**/api/processing/capability", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          state: "ready",
          stages: { develop: "ready", film: "unavailable" },
          profiles: [],
        }),
      }),
    );
    const savedExposure = new Map([
      [first, 0],
      [second, 0],
    ]);
    await page.route("**/api/photos/*/edit-recipe", async (route) => {
      const photoId = new URL(route.request().url()).pathname.split("/")[3];
      if (!photoId || !savedExposure.has(photoId))
        throw new Error("Unexpected Photo");
      if (route.request().method() === "POST") {
        const request = JSON.parse(route.request().postData() ?? "{}") as {
          settings: { exposureEv: number };
        };
        savedExposure.set(photoId, request.settings.exposureEv);
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            outcome: "saved",
            recipeVersion: "recipe-1",
            sourceRevision: "source-1",
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          sourceSupport: "supported",
          supportReason: null,
          sourceRevision: "source-1",
          recipe: {
            recipeVersion: "recipe-1",
            exposureEv: savedExposure.get(photoId),
            whiteBalance: { mode: "as-shot" },
          },
          processingAvailable: true,
          controls: {
            exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
            whiteBalanceModes: ["as-shot"],
          },
        }),
      });
    });
    const submissions = new Map<
      string,
      Array<{
        requestId: string;
        target: string;
        expectedRecipeVersion: string;
        expectedSourceRevision: string;
      }>
    >();
    await page.route("**/api/photos/*/exports", async (route) => {
      const photoId = new URL(route.request().url()).pathname.split("/")[3];
      if (!photoId) throw new Error("Export has no Photo");
      if (route.request().method() === "GET") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ exports: [] }),
        });
        return;
      }
      const body = JSON.parse(route.request().postData() ?? "{}") as {
        requestId: string;
        target: string;
        expectedRecipeVersion: string;
        expectedSourceRevision: string;
      };
      const requests = submissions.get(photoId) ?? [];
      requests.push(body);
      submissions.set(photoId, requests);
      if (photoId === first && requests.length === 1) {
        if (lostResponse === "connection") {
          await route.abort("connectionreset");
        } else {
          await route.fulfill({
            status: 500,
            contentType: "application/json",
            body: JSON.stringify({
              error: {
                code:
                  lostResponse === "server-unknown"
                    ? "outcome_unknown"
                    : "internal_error",
              },
            }),
          });
        }
        return;
      }
      await route.fulfill({
        status: requests.length === 1 ? 201 : 200,
        contentType: "application/json",
        body: JSON.stringify({
          exportId: `export-${photoId}`,
          target: body.target,
          recipeVersion: body.expectedRecipeVersion,
          sourceRevision: body.expectedSourceRevision,
          state: "succeeded",
        }),
      });
    });
    await page.route("**/api/exports/*", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          exportId: new URL(route.request().url()).pathname.split("/").at(-1),
          target: "development-tiff",
          state: "succeeded",
          artifact: null,
        }),
      }),
    );

    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(running.url);
    await expect(page.locator("[data-grid-status]")).toContainText(
      "Ready · 2 Photos",
    );
    await page.locator('[data-photo-index="0"]').click();
    await expect(page.locator("[data-review]")).toBeVisible();
    await openEdit(page);
    const card = page.locator('[data-editor-output="development-tiff"]');
    const submit = card.locator('[data-output-action="submit"]');
    await expect(submit).toBeEnabled();
    await submit.click();
    const reconcile = card.locator('[data-output-action="retry"]');
    await expect(reconcile).toBeVisible();
    await expect(submit).toBeDisabled();
    await navigate(page, "Next");
    await expect(page.getByText("2 / 2")).toBeVisible();
    const exposure = page.locator("[data-photo-editor-exposure]");
    await expect(exposure).toBeEnabled();
    await exposure.evaluate((element: HTMLInputElement) => {
      element.value = "0.5";
      element.dispatchEvent(new Event("input", { bubbles: true }));
      element.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await expect.poll(() => savedExposure.get(second)).toBe(0.5);
    await expect(submit).toBeEnabled();
    await submit.click();
    await expect(card.locator('[data-output-action="download"]')).toBeHidden();
    await expect(submit).toBeEnabled();
    await navigate(page, "Previous");
    await expect(submit).toBeDisabled();
    await expect(reconcile).toBeVisible();
    await reconcile.click();
    await expect(submit).toBeEnabled();
    await expect(reconcile).toBeHidden();
    expect(submissions.get(first)).toHaveLength(2);
    expect(submissions.get(first)?.[1]).toEqual(submissions.get(first)?.[0]);
    expect(submissions.get(second)).toHaveLength(1);
    expect(submissions.get(second)?.[0]?.requestId).not.toBe(
      submissions.get(first)?.[0]?.requestId,
    );
  });
}

test("a refused edit exposes its conflict and blocks image export", async ({
  page,
}) => {
  await page.route("**/api/processing/capability", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "ready",
        stages: { develop: "ready", film: "unavailable" },
        profiles: [],
      }),
    }),
  );
  await page.route("**/api/photos/*/edit-recipe", (route) => {
    if (route.request().method() === "POST")
      return route.fulfill({
        status: 409,
        contentType: "application/json",
        body: JSON.stringify({
          error: {
            code: "recipe_conflict",
            message: "The saved edit changed.",
          },
        }),
      });
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sourceSupport: "supported",
        supportReason: null,
        sourceRevision: "source-1",
        recipe: {
          recipeVersion: "recipe-1",
          exposureEv: 0,
          whiteBalance: { mode: "as-shot" },
        },
        processingAvailable: true,
        controls: {
          exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
          whiteBalanceModes: ["as-shot"],
        },
      }),
    });
  });

  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toContainText(
    "Ready · 2 Photos",
  );
  await page.locator('[data-photo-index="0"]').click();
  await openEdit(page);
  const exposure = page.locator("[data-photo-editor-exposure]");
  await expect(exposure).toBeEnabled();
  await exposure.evaluate((element: HTMLInputElement) => {
    element.value = "0.5";
    element.dispatchEvent(new Event("input", { bubbles: true }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await expect(page.locator("[data-photo-editor-conflict]")).toBeVisible();
  await expect(
    page.locator(
      '[data-editor-output="development-tiff"] [data-output-action="submit"]',
    ),
  ).toBeDisabled();
});

test("an uncertain edit blocks image export until reconciled", async ({
  page,
}) => {
  await page.route("**/api/processing/capability", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "ready",
        stages: { develop: "ready", film: "unavailable" },
        profiles: [],
      }),
    }),
  );
  await page.route("**/api/photos/*/edit-recipe", (route) => {
    if (route.request().method() === "POST")
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: "{",
      });
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sourceSupport: "supported",
        supportReason: null,
        sourceRevision: "source-1",
        recipe: {
          recipeVersion: "recipe-1",
          exposureEv: 0,
          whiteBalance: { mode: "as-shot" },
        },
        processingAvailable: true,
        controls: {
          exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
          whiteBalanceModes: ["as-shot"],
        },
      }),
    });
  });

  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toContainText(
    "Ready · 2 Photos",
  );
  await page.locator('[data-photo-index="0"]').click();
  await openEdit(page);
  const exposure = page.locator("[data-photo-editor-exposure]");
  await expect(exposure).toBeEnabled();
  await exposure.evaluate((element: HTMLInputElement) => {
    element.value = "0.5";
    element.dispatchEvent(new Event("input", { bubbles: true }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await expect(
    page.locator(
      '[data-editor-output="development-tiff"] [data-output-action="submit"]',
    ),
  ).toBeDisabled();
});

test("a transport-lost edit offers a retry instead of remaining on Saving", async ({
  page,
}) => {
  let writes = 0;
  let savedExposure = 0;
  let savedVersion = "recipe-1";
  await page.route("**/api/processing/capability", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "ready",
        stages: { develop: "ready", film: "unavailable" },
        profiles: [],
      }),
    }),
  );
  await page.route("**/api/photos/*/edit-recipe", (route) => {
    if (route.request().method() === "POST") {
      writes += 1;
      if (writes === 1) return route.abort("failed");
      savedExposure = 0.5;
      savedVersion = "recipe-2";
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          outcome: "saved",
          recipeVersion: savedVersion,
          sourceRevision: "source-1",
        }),
      });
    }
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sourceSupport: "supported",
        supportReason: null,
        sourceRevision: "source-1",
        recipe: {
          recipeVersion: savedVersion,
          exposureEv: savedExposure,
          whiteBalance: { mode: "as-shot" },
        },
        processingAvailable: true,
        controls: {
          exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
          whiteBalanceModes: ["as-shot"],
        },
      }),
    });
  });
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toContainText(
    "Ready · 2 Photos",
  );
  await page.locator('[data-photo-index="0"]').click();
  await openEdit(page);
  const exposure = page.locator("[data-photo-editor-exposure]");
  await expect(exposure).toBeEnabled();
  await exposure.evaluate((element: HTMLInputElement) => {
    element.value = "0.5";
    element.dispatchEvent(new Event("input", { bubbles: true }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await expect.poll(() => writes).toBe(1);
  await page.locator("[data-photo-editor-refresh]").click();
  await expect.poll(() => writes).toBe(2);
  await expect(page.locator("[data-photo-editor-exposure-value]")).toHaveText(
    "0.500 EV",
  );
});

test("a Photo without processing keeps its image actions disabled", async ({
  page,
}) => {
  await page.route("**/api/processing/capability", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "ready",
        stages: { develop: "ready", film: "ready" },
        profiles: [],
      }),
    }),
  );
  await page.route("**/api/photos/*/edit-recipe", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sourceSupport: "supported",
        supportReason: null,
        sourceRevision: "source-1",
        recipe: {
          recipeVersion: "recipe-1",
          exposureEv: 0,
          whiteBalance: { mode: "as-shot" },
        },
        processingAvailable: false,
        controls: {
          exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
          whiteBalanceModes: ["as-shot"],
        },
      }),
    }),
  );
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toContainText(
    "Ready · 2 Photos",
  );
  await page.locator('[data-photo-index="0"]').click();
  await openEdit(page);
  await expect(page.locator("[data-photo-editor-exposure]")).toBeEnabled();
  await expect(page.locator('[data-photo-editor-stage="film"]')).toBeDisabled();
  await expect(
    page.locator(
      '[data-editor-output="development-tiff"] [data-output-action="submit"]',
    ),
  ).toBeDisabled();
});

test("Original reference ignores an earlier edit preview response", async ({
  page,
}) => {
  let releasePreview: (() => void) | undefined;
  const heldPreview = new Promise<void>((resolve) => {
    releasePreview = resolve;
  });
  await page.route("**/api/processing/capability", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "ready",
        stages: { develop: "ready", film: "unavailable" },
        profiles: [],
      }),
    }),
  );
  await page.route("**/api/photos/*/edit-recipe", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sourceSupport: "supported",
        supportReason: null,
        sourceRevision: "source-1",
        recipe: {
          recipeVersion: "recipe-1",
          exposureEv: 0,
          whiteBalance: { mode: "as-shot" },
        },
        processingAvailable: true,
        controls: {
          exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
          whiteBalanceModes: ["as-shot"],
        },
      }),
    }),
  );
  const previewRequest = page.waitForRequest((request) =>
    new URL(request.url()).pathname.includes("/edit-preview/"),
  );
  let previewSettled: (() => void) | undefined;
  const settled = new Promise<void>((resolve) => {
    previewSettled = resolve;
  });
  await page.route("**/api/photos/*/edit-preview/**", async (route) => {
    await heldPreview;
    const photoId = new URL(route.request().url()).pathname.split("/")[3];
    const image = await readFile("apps/web/test-fixtures/review.jpg");
    try {
      await route.fulfill({
        status: 200,
        contentType: "image/jpeg",
        headers: {
          "slipstream-edit-preview-photo-id": photoId ?? "",
          "slipstream-edit-preview-stage": "develop",
          "slipstream-edit-preview-source-revision":
            Buffer.from("source-1").toString("hex"),
          "slipstream-edit-preview-recipe-version": "recipe-1",
          "slipstream-edit-preview-width": "16",
          "slipstream-edit-preview-height": "12",
          "slipstream-edit-preview-display-transform": "display-transform-v1",
          "slipstream-edit-preview-sha256": createHash("sha256")
            .update(image)
            .digest("hex"),
        },
        body: image,
      });
    } catch {
      // A cancelled fetch may prevent delivery after the stage changes.
    } finally {
      previewSettled?.();
    }
  });
  try {
    await page.goto(running.url);
    await expect(page.locator("[data-grid-status]")).toContainText(
      "Ready · 2 Photos",
    );
    await page.locator('[data-photo-index="0"]').click();
    await openEdit(page);
    await previewRequest;
    await expect(page.locator("[data-stage] img")).toBeVisible();
    const cameraPreview = await page
      .locator("[data-stage] img")
      .getAttribute("src");
    expect(cameraPreview).toBeTruthy();
    const original = page.locator('[data-photo-editor-stage="camera"]');
    await original.click();
    await expect(original).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator("[data-stage] img")).toHaveAttribute(
      "src",
      cameraPreview!,
    );
    releasePreview?.();
    await settled;
    await expect(
      page.locator("[data-photo-editor-preview-image]"),
    ).toBeHidden();
    await expect(page.locator("[data-stage] img")).toHaveAttribute(
      "src",
      cameraPreview!,
    );
  } finally {
    releasePreview?.();
  }
});

test("a Development Proxy supports editing but cannot start a full Export", async ({
  page,
}) => {
  let submissions = 0;
  await page.route("**/api/processing/capability", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "ready",
        stages: { develop: "ready", film: "ready" },
        profiles: [],
      }),
    }),
  );
  await page.route("**/api/photos/*/edit-recipe", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sourceSupport: "supported",
        supportReason: null,
        sourceRevision: "source-1",
        editSource: "development-proxy",
        editSourceProxyId: "a".repeat(64),
        recipe: {
          recipeVersion: "recipe-1",
          exposureEv: 0,
          whiteBalance: { mode: "as-shot" },
        },
        processingAvailable: true,
        controls: {
          exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
          whiteBalanceModes: ["as-shot"],
        },
      }),
    }),
  );
  await page.route("**/api/photos/*/exports", (route) => {
    if (route.request().method() === "POST") submissions += 1;
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ exports: [] }),
    });
  });

  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toContainText(
    "Ready · 2 Photos",
  );
  await page.locator('[data-photo-index="0"]').click();
  await openEdit(page);
  await expect(page.locator("[data-photo-editor-exposure]")).toBeEnabled();
  await expect(page.locator('[data-photo-editor-stage="film"]')).toBeEnabled();
  await expect(
    page.locator(
      '[data-editor-output="development-tiff"] [data-output-action="submit"]',
    ),
  ).toBeDisabled();
  expect(submissions).toBe(0);
});

test("a failed Export retains its specific failure reason", async ({
  page,
}) => {
  await page.route("**/api/photos/*/exports", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ exports: [{ exportId: "failed-export" }] }),
    }),
  );
  await page.route("**/api/exports/failed-export", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        exportId: "failed-export",
        target: "development-tiff",
        state: "failed",
        failureReason:
          "The processing allowance is insufficient for this output.",
        artifact: null,
      }),
    }),
  );

  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toContainText(
    "Ready · 2 Photos",
  );
  await page.locator('[data-photo-index="0"]').click();
  await openEdit(page);
  await expect(
    page.locator(
      '[data-editor-output="development-tiff"] [data-output-diagnostic]',
    ),
  ).toContainText("The processing allowance is insufficient for this output.");
});

for (const digestPath of ["native", "worker"] as const) {
  test(`a retained Finished JPEG validates and downloads using ${digestPath} hashing`, async ({
    page,
  }) => {
    if (digestPath === "worker")
      await page.addInitScript(() =>
        Object.defineProperty(crypto, "subtle", { value: undefined }),
      );
    const bytes = Buffer.alloc(1024 * 1024 + 67);
    for (let index = 0; index < bytes.length; index++)
      bytes[index] = index % 251;
    const artifact = {
      exportId: "retained-film",
      filename: "slipstream-film-retained-film.jpg",
      orientation: "landscape",
      sampleFormat: "uint8",
      colorSpace: "sRGB",
      iccEmbedded: true,
      target: "film-jpeg",
      stage: "film",
      contentType: "image/jpeg",
      width: 16,
      height: 12,
      profileIdentity: "fixed-film",
      byteLength: bytes.length,
      sha256: createHash("sha256").update(bytes).digest("hex"),
      expiresAt: "2099-01-01T00:00:00Z",
    };
    await page.route("**/api/processing/capability", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          state: "ready",
          stages: { develop: "ready", film: "unavailable" },
          profiles: [],
        }),
      }),
    );
    await page.route("**/api/photos/*/edit-recipe", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          sourceSupport: "supported",
          supportReason: null,
          sourceRevision: "source-1",
          recipe: {
            recipeVersion: "recipe-1",
            exposureEv: 0,
            whiteBalance: { mode: "as-shot" },
          },
          processingAvailable: true,
          controls: {
            exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
            whiteBalanceModes: ["as-shot"],
          },
        }),
      }),
    );
    await page.route("**/api/photos/*/exports", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ exports: [{ exportId: "retained-film" }] }),
      }),
    );
    await page.route("**/api/exports/retained-film", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          exportId: "retained-film",
          target: "film-jpeg",
          state: "succeeded",
          artifact,
        }),
      }),
    );
    await page.route("**/api/exports/retained-film/artifact", (route) =>
      route.fulfill({
        status: 200,
        contentType: "image/jpeg",
        headers: {
          "slipstream-artifact-export-id": "retained-film",
          "slipstream-artifact-target": artifact.target,
          "slipstream-artifact-stage": artifact.stage,
          "slipstream-artifact-content-type": artifact.contentType,
          "slipstream-artifact-width": String(artifact.width),
          "slipstream-artifact-height": String(artifact.height),
          "slipstream-artifact-profile-identity": artifact.profileIdentity,
          "slipstream-artifact-byte-length": String(artifact.byteLength),
          "slipstream-artifact-sha256": artifact.sha256,
          "slipstream-artifact-expires-at": artifact.expiresAt,
          "slipstream-artifact-filename": artifact.filename,
          "slipstream-artifact-orientation": artifact.orientation,
          "slipstream-artifact-sample-format": artifact.sampleFormat,
          "slipstream-artifact-color-space": artifact.colorSpace,
          "slipstream-artifact-icc-embedded": String(artifact.iccEmbedded),
        },
        body: bytes,
      }),
    );

    await page.goto(running.url);
    await expect(page.locator("[data-grid-status]")).toContainText(
      "Ready · 2 Photos",
    );
    await page.locator('[data-photo-index="0"]').click();
    await openEdit(page);
    await expect(
      page.locator(
        '[data-editor-output="development-tiff"] [data-output-action="submit"]',
      ),
    ).toBeEnabled();
    const downloadButton = page.locator(
      '[data-editor-output="film-jpeg"] [data-output-action="download"]',
    );
    await expect(downloadButton).toBeVisible();
    const download = page.waitForEvent("download");
    await downloadButton.click();
    const file = await download;
    expect(file.suggestedFilename()).toBe("slipstream-film-retained-film.jpg");
    expect(await readFile(await file.path())).toEqual(bytes);
  });
}

test("an admitted Edit Preview remains observable after the old 15-second polling limit", async ({
  page,
}) => {
  await page.clock.install();
  await page.route("**/api/processing/capability", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "ready",
        stages: { develop: "ready", film: "unavailable" },
        profiles: [],
      }),
    }),
  );
  await page.route("**/api/photos/*/edit-recipe", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sourceSupport: "supported",
        supportReason: null,
        sourceRevision: "source-1",
        recipe: {
          recipeVersion: "recipe-1",
          exposureEv: 0,
          whiteBalance: { mode: "as-shot" },
        },
        processingAvailable: true,
        controls: {
          exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
          whiteBalanceModes: ["as-shot"],
        },
      }),
    }),
  );
  let requests = 0;
  await page.route(
    "**/api/photos/*/edit-preview/develop?settings=current",
    (route) => {
      requests += 1;
      return route.fulfill({
        status: requests <= 21 ? 202 : 503,
        contentType: "application/json",
        body:
          requests <= 21
            ? JSON.stringify({ state: "running" })
            : JSON.stringify({ error: { code: "resource_unavailable" } }),
      });
    },
  );

  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toContainText(
    "Ready · 2 Photos",
  );
  await page.locator('[data-photo-index="0"]').click();
  await openEdit(page);
  await expect(page.locator("[data-photo-editor-preview-note]")).toContainText(
    "Rendering the preview",
  );
  for (let count = requests; count <= 21; count += 1) {
    await page.clock.runFor(751);
    await expect.poll(() => requests).toBeGreaterThan(count);
  }
  await expect(page.locator("[data-photo-editor-preview-note]")).toContainText(
    "Could not create the preview right now",
  );
});
