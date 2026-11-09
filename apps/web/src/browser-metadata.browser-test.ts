import { expect, test, type Page } from "@playwright/test";
import {
  fixture,
  server,
  setupBrowserSmoke,
  writePhotos,
} from "./browser-test-support/fixtures.js";
import {
  closePhotoSurfaces,
  closePhotoTools,
  openPhotoDetailsAdvanced,
  openPhotoToolsView,
  startReview,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

const openDetails = async (page: Page) => {
  await openPhotoToolsView(page, "details");
  await openPhotoDetailsAdvanced(page);
};

// Qualified-looking Read/Save responses are injected for UI coverage only.
// This smoke does not qualify the native Sidecar save environment.
test("metadata ordinary fields retain Photo drafts and explicitly review changed evidence (UI fixture)", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  let revision = 1;
  let savedTitle = "Existing IPTC title";
  let embeddedIim = true;
  let posted: unknown;
  let writable = true;
  let reads = 0;
  await page.route(
    /\/api\/photos\/[^/]+\/external-metadata$/,
    async (route) => {
      const photoId = decodeURIComponent(
        new URL(route.request().url()).pathname.split("/")[3]!,
      );
      if (route.request().method() === "POST") {
        const body = route.request().postDataJSON() as {
          changes: { "dc:title": { languages: { "x-default": string } } };
          evidence: { revision: number };
        };
        posted = body;
        savedTitle = body.changes["dc:title"].languages["x-default"];
        revision++;
        embeddedIim = false;
        await route.fulfill({
          json: {
            photoId,
            sidecarLocation: "fixture.xmp",
            affectedFields: ["dc:title"],
          },
        });
        return;
      }
      reads++;
      await route.fulfill({
        json: {
          photoId,
          originalLocation: "fixture.jpg",
          association: { state: "eligible", candidates: [] },
          libraryRating: 0,
          saveAvailable: writable,
          saveUnavailableReason: writable
            ? undefined
            : "Sidecar access is read-only.",
          captureFacts: {},
          evidence: {
            revision,
            sidecar: { state: "present", location: "fixture.xmp" },
          },
          fields: {
            "dc:title": {
              state: "present",
              writable: true,
              provenance: embeddedIim ? "iptc_iim" : "sidecar",
              value: embeddedIim
                ? savedTitle
                : { "x-default": savedTitle, fr: "Titre conservé" },
              languageAlternativesAvailable: !embeddedIim,
              sources: [],
            },
            "dc:description": {
              state: "present",
              writable: true,
              provenance: "iptc_iim",
              value: "Existing IPTC caption",
              languageAlternativesAvailable: false,
              sources: [],
            },
            "dc:rights": {
              state: "present",
              writable: true,
              provenance: "iptc_iim",
              value: "Existing IPTC copyright",
              languageAlternativesAvailable: false,
              sources: [],
            },
            "dc:subject": { state: "absent", writable: true, sources: [] },
          },
        },
      });
    },
  );
  await startReview(page, running.url, "All Photos");
  await openDetails(page);
  const title = page.locator('[data-metadata-value="dc:title"]');
  await expect(title).toBeVisible();
  await expect(title).toHaveValue("Existing IPTC title");
  const description = page.locator('[data-metadata-value="dc:description"]');
  await expect(description).toHaveValue("Existing IPTC caption");
  const titleField = page.locator('[data-metadata-field="dc:title"]');
  const descriptionField = page.locator(
    '[data-metadata-field="dc:description"]',
  );
  await expect(descriptionField).toContainText("iptc_iim");
  await expect(
    page.getByRole("textbox", { name: "Title language", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("textbox", { name: "Description language", exact: true }),
  ).toHaveCount(0);
  await page
    .getByText("More fields, attribution, and rights", { exact: true })
    .click();
  const copyrightField = page.locator('[data-metadata-field="dc:rights"]');
  await copyrightField.locator("summary").click();
  await page
    .getByLabel("Copyright notice Sidecar operation", { exact: true })
    .selectOption("set");
  await expect(page.locator('[data-metadata-value="dc:rights"]')).toHaveValue(
    "Existing IPTC copyright",
  );
  await expect(
    page.getByRole("textbox", {
      name: "Copyright notice language",
      exact: true,
    }),
  ).toHaveCount(0);
  await page.locator("[data-external-metadata-discard]").click();
  await description.fill("Edited IPTC caption");
  await expect(page.locator("[data-external-metadata-review]")).toContainText(
    "Edited IPTC caption",
  );
  await page.locator("[data-external-metadata-discard]").click();
  await expect(description).toHaveValue("Existing IPTC caption");
  await title.fill("Retained title");
  await expect(page.locator("[data-external-metadata-save]")).toBeDisabled();
  await closePhotoSurfaces(page);
  const beforeReopen = reads;
  await openDetails(page);
  await expect.poll(() => reads).toBeGreaterThan(beforeReopen);
  await expect(title).toHaveValue("Retained title");
  await closePhotoSurfaces(page);
  await page.getByRole("button", { name: "Back to Grid", exact: true }).click();
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  revision++;
  await openDetails(page);
  await expect(title).toHaveValue("Retained title");
  await expect(
    page.locator("[data-external-metadata-accept-inspection]"),
  ).toBeVisible();
  await expect(page.locator("[data-external-metadata-save]")).toBeDisabled();
  await page.locator("[data-external-metadata-accept-inspection]").click();
  await titleField.locator("summary").click();
  await titleField
    .getByRole("button", { name: "Add language", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "Title language", exact: true })
    .fill("__proto__");
  await page
    .getByRole("textbox", { name: "Title language value", exact: true })
    .fill("Selected named value");
  await page.locator("[data-external-metadata-review-button]").click();
  await expect(page.locator("[data-external-metadata-review]")).toContainText(
    "Title: Set value",
  );
  await expect(page.locator("[data-external-metadata-review]")).toContainText(
    "__proto__: Selected named value",
  );
  await page.locator("[data-external-metadata-save]").click();
  await expect(page.locator("[data-external-metadata-status]")).toContainText(
    "Saved Sidecar fixture.xmp",
  );
  await expect(
    titleField.locator('[aria-label="Title language value"]').nth(1),
  ).toHaveValue("Titre conservé");
  expect(posted).toEqual({
    evidence: {
      revision: 2,
      sidecar: { state: "present", location: "fixture.xmp" },
    },
    changes: {
      "dc:title": {
        op: "setLanguages",
        languages: {
          "x-default": "Retained title",
          ["__proto__"]: "Selected named value",
        },
      },
    },
  });
  await title.fill("Discard this");
  await page.locator("[data-external-metadata-discard]").click();
  await expect(title).toHaveValue("Retained title");
  writable = false;
  await page.locator("[data-external-metadata-refresh]").click();
  await expect(
    page.getByText("Metadata editing is unavailable in this deployment.", {
      exact: true,
    }),
  ).toBeVisible();
  // The deployment's technical cause stays inspectable in the evidence
  // disclosure instead of dominating the read-only state.
  const evidenceSummary = page
    .locator("summary")
    .filter({ hasText: /^Sources and Sidecar evidence$/ });
  await evidenceSummary.click();
  await expect(evidenceSummary.locator("..")).toContainText(
    "Sidecar access is read-only.",
  );
  await expect(page.locator('[data-metadata-value="dc:title"]')).toHaveCount(0);
  await expect(page.locator('[data-metadata-field="dc:title"]')).toContainText(
    "Retained title",
  );
});

// Routed responses exercise panel ownership across navigation, not native Save safety.
test("delayed metadata Save settles for its Photo after A to B to A navigation (UI fixture)", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  const titles = new Map<string, string>();
  let photoA = "";
  let photoB = "";
  let posted: { photoId: string; body: unknown } | undefined;
  let releaseSave!: () => void;
  const delayedSave = new Promise<void>((resolve) => {
    releaseSave = resolve;
  });
  await page.route(
    /\/api\/photos\/[^/]+\/external-metadata$/,
    async (route) => {
      const photoId = decodeURIComponent(
        new URL(route.request().url()).pathname.split("/")[3]!,
      );
      if (route.request().method() === "POST") {
        const body = route.request().postDataJSON() as {
          changes: { "dc:title": { languages: { "x-default": string } } };
        };
        posted = { photoId, body };
        await delayedSave;
        titles.set(photoId, body.changes["dc:title"].languages["x-default"]);
        await route.fulfill({
          json: {
            photoId,
            sidecarLocation: `${photoId}.xmp`,
            affectedFields: ["dc:title"],
          },
        });
        return;
      }
      if (!photoA) photoA = photoId;
      else if (photoId !== photoA) photoB = photoId;
      await route.fulfill({
        json: {
          photoId,
          originalLocation: `${photoId}.jpg`,
          association: { state: "eligible", candidates: [] },
          libraryRating: 0,
          saveAvailable: true,
          captureFacts: {},
          evidence: { sidecar: { state: "absent" } },
          fields: {
            "dc:title": {
              state: "present",
              writable: true,
              provenance: "sidecar",
              value: { "x-default": titles.get(photoId) ?? "Existing title" },
              sources: [],
            },
          },
        },
      });
    },
  );
  try {
    await startReview(page, running.url, "All Photos");
    await openDetails(page);
    const title = page.locator('[data-metadata-value="dc:title"]');
    const status = page.locator("[data-external-metadata-status]");
    const discard = page.locator("[data-external-metadata-discard]");
    await expect(title).toHaveValue("Existing title");
    await title.fill("Photo A saved title");
    await page.locator("[data-external-metadata-review-button]").click();
    await page.locator("[data-external-metadata-save]").click();
    await expect.poll(() => posted?.photoId).toBe(photoA);
    await expect(title).toBeDisabled();
    await closePhotoSurfaces(page);
    await page.keyboard.press("ArrowRight");
    await expect(page.getByText("2 / 2", { exact: true })).toBeVisible();
    await openDetails(page);
    await expect(title).toHaveValue("Existing title");
    expect(photoB).not.toBe(photoA);
    await title.fill("Photo B retained draft");
    await closePhotoSurfaces(page);
    await page.keyboard.press("ArrowLeft");
    await expect(page.getByText("1 / 2", { exact: true })).toBeVisible();
    await openDetails(page);
    await expect(title).toHaveValue("Photo A saved title");
    await expect(title).toBeDisabled();
    await expect(discard).toBeDisabled();
    await expect(page.locator("[data-external-metadata-save]")).toBeDisabled();
    releaseSave();
    await expect(status).toContainText(`Saved Sidecar ${photoA}.xmp`);
    await expect(title).toBeEnabled();
    await expect(title).toHaveValue("Photo A saved title");
    await expect(discard).toBeHidden();
    expect(posted).toEqual({
      photoId: photoA,
      body: {
        evidence: { sidecar: { state: "absent" } },
        changes: {
          "dc:title": {
            op: "setLanguages",
            languages: { "x-default": "Photo A saved title" },
          },
        },
      },
    });
    await closePhotoSurfaces(page);
    await page.keyboard.press("ArrowRight");
    await expect(page.getByText("2 / 2", { exact: true })).toBeVisible();
    await openDetails(page);
    await expect(title).toHaveValue("Photo B retained draft");
    await expect(title).toBeEnabled();
    await expect(discard).toBeVisible();
    await expect(status).toContainText(
      "Unsaved changes retained for this Photo.",
    );
    await expect(page.locator("[data-external-metadata-review]")).toContainText(
      "Photo B retained draft",
    );
    await expect(page.locator("[data-external-metadata-save]")).toBeDisabled();
    expect(titles.has(photoB)).toBe(false);
  } finally {
    releaseSave();
  }
});

test("a rejected metadata draft admission keeps its error visible", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const { base, root } = await fixture();
  await writePhotos(root, 33);
  const running = await server(base, root);
  // The real fixture is read-only in this environment; the draft-capacity
  // path needs an editable Title on every Photo, not a Sidecar write.
  await page.route(
    /\/api\/photos\/[^/]+\/external-metadata$/,
    async (route) => {
      if (route.request().method() !== "GET") return route.continue();
      const photoId = decodeURIComponent(
        new URL(route.request().url()).pathname.split("/")[3]!,
      );
      await route.fulfill({
        json: {
          photoId,
          originalLocation: "fixture.jpg",
          association: { state: "eligible", candidates: [] },
          libraryRating: 0,
          saveAvailable: true,
          captureFacts: {},
          evidence: { revision: 1, sidecar: { state: "absent" } },
          fields: {
            "dc:title": {
              state: "absent",
              writable: true,
              provenance: "sidecar",
              sources: [],
            },
          },
        },
      });
    },
  );
  await startReview(page, running.url, "All Photos");
  for (let index = 0; index < 32; index += 1) {
    await openPhotoToolsView(page, "details");
    await openPhotoDetailsAdvanced(page);
    await page
      .locator('[data-metadata-value="dc:title"]')
      .fill(`draft ${index}`);
    await closePhotoTools(page);
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("[data-position]")).toHaveText(
      `${index + 2} / 33`,
    );
  }
  await openPhotoToolsView(page, "details");
  await openPhotoDetailsAdvanced(page);
  await page.locator('[data-metadata-value="dc:title"]').fill("overflow");
  const status = page.locator("[data-external-metadata-status]");
  await expect(status).toBeVisible();
  await expect(status).toContainText("32 metadata drafts");
});

test("a failed metadata refresh reopens the status line", async ({ page }) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  let failReads = false;
  await page.route(
    /\/api\/photos\/[^/]+\/external-metadata$/,
    async (route) => {
      if (route.request().method() !== "GET") return route.continue();
      if (failReads) return route.abort("failed");
      const photoId = decodeURIComponent(
        new URL(route.request().url()).pathname.split("/")[3]!,
      );
      await route.fulfill({
        json: {
          photoId,
          originalLocation: "fixture.jpg",
          association: { state: "eligible", candidates: [] },
          libraryRating: 0,
          saveAvailable: true,
          captureFacts: {},
          evidence: { revision: 1, sidecar: { state: "absent" } },
          fields: {
            "dc:title": {
              state: "absent",
              writable: true,
              provenance: "sidecar",
              sources: [],
            },
          },
        },
      });
    },
  );
  await startReview(page, running.url, "All Photos");
  await openDetails(page);
  const status = page.locator("[data-external-metadata-status]");
  await expect(page.locator('[data-metadata-value="dc:title"]')).toBeVisible();
  await expect(status).toBeHidden();
  failReads = true;
  await page.getByRole("button", { name: "Refresh inspection" }).click();
  await expect(status).toBeVisible();
  expect((await status.textContent())?.trim()).not.toBe("");
});
