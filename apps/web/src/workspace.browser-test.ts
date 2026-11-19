import { expect, test } from "@playwright/test";
import { writeFile, readFile } from "node:fs/promises";
import { join } from "node:path";
import {
  browseIds,
  fixture,
  jpeg,
  server,
  setupBrowserSmoke,
} from "./browser-test-support/fixtures.js";
import {
  fileFixture,
  json,
  windowOf,
} from "./browser-test-support/workspace-file-fixtures.js";
import {
  openPhotoToolsView,
  waitForLoadedReviewImage,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

const inputPng = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
  "base64",
);

test("Workspace presents, downloads, and removes files without a tool call", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "workspace.jpg"), await jpeg());
  const running = await server(base, root);
  const [photoId] = await browseIds(running.url);
  if (!photoId) throw new Error("The Workspace smoke requires one Photo");

  const outputBytes = Buffer.from("completed workspace file");
  const input = fileFixture(
    "input",
    inputPng,
    "input-r1",
    "input",
    "input/workspace.png",
  );
  input.imageContract = {
    format: "image/png",
    width: 1,
    height: 1,
    colorSpace: "srgb",
  };
  const output = fileFixture(
    "output",
    outputBytes,
    "output-r1",
    "output",
    "outputs/handoff-output.tif",
  );
  output.imageContract = {
    format: "tiff",
    precision: "uint8",
    transfer: "srgb",
    colorSpace: "gray",
    width: 1,
    height: 1,
  };
  let removed = false;
  let removalAttempts = 0;
  let staleDownload = false;
  const workspace = {
    workspaceId: "workspace-file-view",
    photoId,
    path: "/srv/slipstream/photo-workspace",
    inputPath: "/srv/slipstream/photo-workspace/input/workspace.png",
    inputArtifactId: "input",
    inputAvailability: "ready",
    inputMatchesObservedSource: false,
    execution: { hostId: "service-host", mode: "service-host" },
  };
  const files = () => (removed ? [input] : [input, output]);
  const toolRequests: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (path.includes("/darktable") || path.endsWith("/commands"))
      toolRequests.push(path);
  });
  await page.route(`**/api/photos/${photoId}/workspace`, (route) =>
    json(route, { ...workspace, files: windowOf(files()) }),
  );
  await page.route(
    `**/api/workspaces/workspace-file-view/files?limit=60`,
    (route) => json(route, windowOf(files())),
  );
  await page.route(
    `**/api/workspaces/workspace-file-view/files/input/bytes*`,
    (route) =>
      route.fulfill({
        status: 200,
        contentType: "image/png",
        headers: {
          "Content-Disposition": 'inline; filename="workspace.png"',
        },
        body: inputPng,
      }),
  );
  await page.route(
    `**/api/workspaces/workspace-file-view/files/output/bytes*`,
    (route) => {
      if (staleDownload)
        return json(
          route,
          {
            error: {
              code: "artifact_changed",
              message: "Selected file changed before download.",
              effect: "none",
              details: {},
            },
          },
          409,
        );
      return route.fulfill({
        status: 200,
        contentType: "image/tiff",
        headers: {
          "Content-Disposition": 'attachment; filename="handoff-output.tif"',
        },
        body: outputBytes,
      });
    },
  );
  await page.route(
    `**/api/workspaces/workspace-file-view/files/output`,
    (route) => {
      if (route.request().method() === "DELETE") {
        removalAttempts += 1;
        if (removalAttempts === 1)
          return json(
            route,
            {
              error: {
                code: "artifact_referenced",
                message:
                  "Selected artifact remains referenced or its independent use is unverified.",
                effect: "none",
                details: {
                  blockers: [
                    {
                      kind: "edit-state",
                      name: "darktable main",
                      action: "Remove the saved edit state first",
                      reason: "The output is the retained result",
                    },
                  ],
                },
              },
            },
            409,
          );
        removed = true;
        return json(route, {
          outcome: "removed",
          inputArtifactId: "input",
          reclamation: "pending",
        });
      }
      return json(route, {
        artifactId: "output",
        revision: "output-r1",
        path: "/srv/slipstream/photo-workspace/outputs/handoff-output.tif",
      });
    },
  );

  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${running.url}/?photoId=${photoId}`);
  await waitForLoadedReviewImage(page);
  await openPhotoToolsView(page, "workspace");

  const view = page.locator("[data-photo-tools-view='workspace']");
  await expect(view).toBeVisible();
  await expect(page.locator("[data-workspace-status]")).toHaveText("Ready.");
  await expect(page.locator("[data-workspace-input]")).toContainText(
    "differs from the current Library Original",
  );
  await expect(view).toContainText("Execution host: service-host");
  await expect(view).toContainText(
    "Execution-environment path: /srv/slipstream/photo-workspace",
  );
  await expect(
    view.getByRole("heading", { name: "Input (1)", exact: true }),
  ).toBeVisible();
  await expect(
    view.getByRole("heading", { name: "Outputs (1)", exact: true }),
  ).toBeVisible();
  expect(
    await view.evaluate(
      (element) => element.scrollWidth <= element.clientWidth + 1,
    ),
  ).toBe(true);

  const inputCard = view.locator(
    `article[data-artifact-id="${input.artifactId}"]`,
  );
  await expect(inputCard.locator("img.workspace-file-thumbnail")).toBeVisible();
  const outputCard = view.locator(
    `article[data-artifact-id="${output.artifactId}"]`,
  );
  await expect(
    outputCard.getByText(
      "No display conversion is generated. Download to inspect this file.",
      { exact: true },
    ),
  ).toBeVisible();
  const download = outputCard.getByRole("link", {
    name: "Download Output · handoff-output.tif",
    exact: true,
  });
  await expect(download).toHaveAttribute("download", "handoff-output.tif");
  const downloadPromise = page.waitForEvent("download");
  await download.evaluate((link) => link.removeAttribute("download"));
  await download.click();
  const downloaded = await downloadPromise;
  expect(downloaded.suggestedFilename()).toBe("handoff-output.tif");
  const path = await downloaded.path();
  if (!path) throw new Error("The browser did not retain the Workspace file");
  expect(await readFile(path)).toEqual(outputBytes);

  staleDownload = true;
  await download.click();
  await expect(page.locator("[data-workspace-error]")).toContainText(
    "Selected file changed before download.",
  );
  await expect(page.locator("[data-workspace-status]")).toHaveText(
    "Download failed.",
  );
  staleDownload = false;

  await expect(page.locator("[data-workspace-remove]")).toBeHidden();
  await outputCard
    .getByRole("button", {
      name: "Inspect Output · handoff-output.tif",
      exact: true,
    })
    .click();
  await expect(page.locator("[data-workspace-inspected-heading]")).toHaveText(
    "Inspected file: handoff-output.tif",
  );
  await expect(
    page.locator("[data-workspace-inspected-heading]"),
  ).toBeFocused();
  await expect(page.locator("[data-workspace-file-path]")).toContainText(
    "Execution-environment path",
  );
  await expect(page.locator("[data-workspace-remove]")).toBeVisible();
  await expect(
    page.locator("[data-workspace-inspected-download]"),
  ).toHaveAttribute("href", /revision=output-r1/);
  await expect(
    page.locator("[data-workspace-inspected-download]"),
  ).toHaveAttribute("data-workspace-download", output.artifactId);
  await page.locator("[data-workspace-remove]").click();
  await expect(page.locator("[data-workspace-error]")).toContainText(
    "Remove the saved edit state first",
  );
  await expect(page.locator("[data-workspace-error]")).toBeFocused();
  await expect(page.locator("[data-workspace-status]")).toHaveText(
    "Remove failed.",
  );
  await expect(
    page.locator("[data-workspace-inspected-heading]"),
  ).toBeVisible();
  await page.locator("[data-workspace-remove]").click();
  await expect(page.locator("[data-workspace-status]")).toHaveText(
    "File removed; reclamation pending.",
  );
  await expect(page.locator("[data-workspace-status]")).toBeFocused();
  await expect(outputCard).toHaveCount(0);
  await expect(page.locator("[data-workspace-notice]")).toHaveText(
    "Removed outputs/handoff-output.tif; reclamation is pending.",
  );
  expect(toolRequests).toEqual([]);
});
