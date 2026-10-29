import { expect, test } from "@playwright/test";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import {
  fixture,
  server,
  setupBrowserSmoke,
  writePhotos,
  post,
} from "./browser-test-support/fixtures.js";
import {
  openSources,
  openViewOptions,
  applyViewOptions,
  waitForGridFrame,
  openMembershipPanel,
  membershipCheckbox,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

test("thumbnail size commits only on Apply without reopening the source", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 4);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 4 Photos",
  );
  await waitForGridFrame(page);
  const cellWidth = () =>
    page
      .locator('[data-photo-index="0"]')
      .evaluate((cell) => cell.getBoundingClientRect().width);
  const initialWidth = await cellWidth();
  let opens = 0;
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/browse"
    )
      opens++;
  });
  for (const close of ["cancel", "close", "escape", "management"] as const) {
    await openViewOptions(page);
    await page.locator("[data-size-select]").selectOption("large");
    expect(await cellWidth()).toBe(initialWidth);
    if (close === "escape") await page.keyboard.press("Escape");
    else if (close === "management") {
      await page.locator("[data-removed-open]").click();
      await page.locator("[data-removed-close]").click();
    } else await page.locator(`[data-view-options-${close}]`).click();
    await openViewOptions(page);
    await expect(page.locator("[data-size-select]")).toHaveValue("medium");
    await applyViewOptions(page);
    expect(await cellWidth()).toBe(initialWidth);
  }
  await openViewOptions(page);
  await page.locator("[data-size-select]").selectOption("large");
  await applyViewOptions(page);
  await expect.poll(cellWidth).toBeGreaterThan(initialWidth);
  await openViewOptions(page);
  await expect(page.locator("[data-size-select]")).toHaveValue("large");
  await page.locator("[data-view-options-cancel]").click();
  expect(opens).toBe(0);
});

test("failed membership has no false checkboxes and Retry restores confirmed membership", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 1);
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Picks" });
  let fail = true;
  await page.route(/\/api\/photos\/[^/]+\/albums$/, async (route) => {
    if (fail)
      await route.fulfill({ status: 503, json: { error: "unavailable" } });
    else await route.continue();
  });
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 1 Photo",
  );
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await openMembershipPanel(page);
  await expect(page.locator("[data-membership-retry]")).toBeVisible();
  await expect(page.locator("[data-membership-options] input")).toHaveCount(0);
  fail = false;
  await page.locator("[data-membership-retry]").click();
  await expect(membershipCheckbox(page, "Picks")).toBeEnabled();
  await expect(membershipCheckbox(page, "Picks")).not.toBeChecked();
});

test("nested Folder expansion and paging keep focus on their controls", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await Promise.all(
    Array.from({ length: 121 }, async (_, index) => {
      const folder = join(
        root,
        "Parent",
        `Child-${String(index).padStart(3, "0")}`,
      );
      await mkdir(folder, { recursive: true });
      await writePhotos(folder, 1);
    }),
  );
  const running = await server(base, root);
  await page.setViewportSize({ width: 1200, height: 800 });
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toHaveText(
    "Ready · 121 Photos",
  );
  await openSources(page);
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  const expand = page.getByRole("button", { name: "Toggle Parent subfolders" });
  await expand.focus();
  await expand.press("Enter");
  await expect(expand).toHaveAttribute("aria-expanded", "true");
  await expect(expand).toBeFocused();
  const next = page.locator('[data-focus-key="folder:next:Parent"]');
  const previous = page.locator('[data-focus-key="folder:previous:Parent"]');
  await expect(next).toBeEnabled();
  await next.focus();
  await next.press("Enter");
  await expect(
    page.locator(".folder-pager .folder-page-label").last(),
  ).toHaveText("2 / 3");
  await expect(next).toBeFocused();
  await next.press("Enter");
  await expect(
    page.locator(".folder-pager .folder-page-label").last(),
  ).toHaveText("3 / 3");
  await expect(next).toBeDisabled();
  await expect(previous).toBeFocused();
  await previous.press("Enter");
  await expect(
    page.locator(".folder-pager .folder-page-label").last(),
  ).toHaveText("2 / 3");
  await expect(previous).toBeFocused();
  await previous.press("Enter");
  await expect(
    page.locator(".folder-pager .folder-page-label").last(),
  ).toHaveText("1 / 3");
  await expect(previous).toBeDisabled();
  await expect(next).toBeFocused();
  await expand.focus();
  await expand.press("Enter");
  await expect(expand).toHaveAttribute("aria-expanded", "false");
  await expect(expand).toBeFocused();
});

for (const width of [1200, 390]) {
  test(`Folder disclosure shows pending work and reopens retained children offline at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 844 });
    const { base, root } = await fixture();
    const child = join(root, "Parent", "Child");
    await mkdir(child, { recursive: true });
    await writePhotos(child, 1);
    const running = await server(base, root);
    const { promise: held, resolve: release } = Promise.withResolvers<void>();
    let requests = 0;
    await page.route(/\/api\/file-locations\?/, async (route) => {
      if (
        new URL(route.request().url()).searchParams.get("parent") === "Parent"
      ) {
        requests += 1;
        await held;
      }
      await route.continue();
    });
    const expand = page.getByRole("button", {
      name: "Toggle Parent subfolders",
    });
    try {
      await page.goto(running.url);
      await expect(page.locator("[data-grid-status]")).toHaveText(
        "Ready · 1 Photo",
      );
      await openSources(page);
      await page
        .getByRole("button", { name: "Toggle Library Folder subfolders" })
        .click();
      await expand.click();
      await expect(
        page
          .getByRole("status")
          .filter({ hasText: "Loading subfolders of Parent" }),
      ).toBeVisible();
      await expect(expand).toBeFocused();
      await expect(expand).toHaveAttribute("aria-expanded", "false");
      await expect(expand).toHaveAttribute("aria-disabled", "true");
      await expand.press("Enter");
      await expect.poll(() => requests).toBe(1);
    } finally {
      release();
    }
    const childLink = page.locator(
      '[data-focus-key="source:folder:Parent/Child"]',
    );
    await expect(childLink).toBeVisible();
    await expect(page.locator(".folder-loading")).toHaveCount(0);
    await expect(expand).toHaveAttribute("aria-disabled", "false");
    await expand.click();
    await expect(childLink).toHaveCount(0);
    await page.context().setOffline(true);
    try {
      await expand.click();
      await expect(childLink).toBeVisible();
      await expect(expand).toBeFocused();
      expect(requests).toBe(1);
    } finally {
      await page.context().setOffline(false);
    }
  });
}

for (const width of [1200, 390]) {
  test(`Folder retries retain focus and remain visible when their parent is hidden at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 844 });
    const { base, root } = await fixture();
    await Promise.all(
      Array.from({ length: 121 }, async (_, index) => {
        const folder = join(
          root,
          "Parent",
          `Child-${String(index).padStart(3, "0")}`,
        );
        await mkdir(folder, { recursive: true });
        await writePhotos(folder, 1);
      }),
    );
    const running = await server(base, root);
    const first = Promise.withResolvers<void>();
    const second = Promise.withResolvers<void>();
    const requests = new Map<number, number>();
    await page.route(/\/api\/file-locations\?/, async (route) => {
      const parameters = new URL(route.request().url()).searchParams;
      const start = Number(parameters.get("start"));
      if (parameters.get("parent") === "Parent" && start > 0) {
        const count = (requests.get(start) ?? 0) + 1;
        requests.set(start, count);
        if (count === 1) {
          await route.abort();
          return;
        }
        await (start === 60 ? first.promise : second.promise);
      }
      await route.continue();
    });
    try {
      await page.goto(running.url);
      await expect(page.locator("[data-grid-status]")).toHaveText(
        "Ready · 121 Photos",
      );
      await openSources(page);
      const rootToggle = page.getByRole("button", {
        name: "Toggle Library Folder subfolders",
      });
      await rootToggle.click();
      const parentToggle = page.getByRole("button", {
        name: "Toggle Parent subfolders",
      });
      await parentToggle.click();
      const more = page.locator('[data-focus-key="folder:next:Parent"]');
      await more.click();
      const retryFirst = page.getByRole("button", {
        name: "Retry Folders (Parent items 61–120)",
      });
      await retryFirst.focus();
      await retryFirst.press("Enter");
      await expect(retryFirst).toBeFocused();
      await expect(retryFirst).toHaveAttribute("aria-disabled", "true");
      await expect(
        page
          .getByRole("status")
          .filter({ hasText: "Retrying Folders (Parent items 61–120)" }),
      ).toBeVisible();
      await retryFirst.press("Enter");
      await expect.poll(() => requests.get(60)).toBe(2);
      first.resolve();
      await expect(retryFirst).toHaveCount(0);
      await expect(parentToggle).toBeFocused();
      await expect(
        page.locator(".folder-pager .folder-page-label").last(),
      ).toHaveText("2 / 3");

      await more.click();
      const retrySecond = page.getByRole("button", {
        name: "Retry Folders (Parent items 121–180)",
      });
      await retrySecond.focus();
      await retrySecond.press("Enter");
      await expect(retrySecond).toBeFocused();
      await expect(parentToggle).toHaveAttribute("aria-disabled", "false");
      await parentToggle.click();
      await expect(parentToggle).toHaveAttribute("aria-expanded", "false");
      await expect(parentToggle).toHaveAttribute("aria-disabled", "true");
      await parentToggle.press("Enter");
      await expect(parentToggle).toHaveAttribute("aria-expanded", "false");
      await rootToggle.click();
      await expect(parentToggle).toHaveCount(0);
      await expect(
        page
          .getByRole("status")
          .filter({ hasText: "Retrying Folders (Parent items 121–180)" }),
      ).toBeVisible();
      await retrySecond.focus();
      await retrySecond.press("Enter");
      await expect.poll(() => requests.get(120)).toBe(2);
      second.resolve();
      await expect(retrySecond).toHaveCount(0);
      await expect(rootToggle).toBeFocused();
      await expect(rootToggle).toHaveAttribute("aria-expanded", "false");
    } finally {
      first.resolve();
      second.resolve();
    }
  });
}

test("Library Folder remains collapsible while its retry is pending", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const { base, root } = await fixture();
  await Promise.all(
    Array.from({ length: 61 }, async (_, index) => {
      const folder = join(root, `Folder-${String(index).padStart(3, "0")}`);
      await mkdir(folder, { recursive: true });
      await writePhotos(folder, 1);
    }),
  );
  const running = await server(base, root);
  const held = Promise.withResolvers<void>();
  let requests = 0;
  await page.route(/\/api\/file-locations\?/, async (route) => {
    const parameters = new URL(route.request().url()).searchParams;
    if (!parameters.has("parent") && parameters.get("start") === "60") {
      requests += 1;
      if (requests === 1) {
        await route.abort();
        return;
      }
      await held.promise;
    }
    await route.continue();
  });
  try {
    await page.goto(running.url);
    await expect(page.locator("[data-grid-status]")).toHaveText(
      "Ready · 61 Photos",
    );
    await openSources(page);
    const rootToggle = page.getByRole("button", {
      name: "Toggle Library Folder subfolders",
    });
    await rootToggle.click();
    await page.locator('[data-focus-key="folder:next:"]').click();
    const retry = page.getByRole("button", {
      name: "Retry Folders (Library Folder items 61–120)",
    });
    await retry.focus();
    await retry.press("Enter");
    await expect(retry).toHaveAttribute("aria-disabled", "true");
    await expect(rootToggle).toHaveAttribute("aria-expanded", "true");
    await expect(rootToggle).toHaveAttribute("aria-disabled", "false");
    await rootToggle.click();
    await expect(rootToggle).toHaveAttribute("aria-expanded", "false");
    await retry.focus();
    await expect(
      page
        .getByRole("status")
        .filter({ hasText: "Retrying Folders (Library Folder items 61–120)" }),
    ).toBeVisible();
    held.resolve();
    await expect(retry).toHaveCount(0);
    await expect(rootToggle).toBeFocused();
    await expect(rootToggle).toHaveAttribute("aria-expanded", "false");
  } finally {
    held.resolve();
  }
});

test("long unbroken Folder loading status stays inside narrow Sources", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const { base, root } = await fixture();
  const name = "Folder".repeat(30);
  const child = join(root, name, "Child");
  await mkdir(child, { recursive: true });
  await writePhotos(child, 1);
  const running = await server(base, root);
  const held = Promise.withResolvers<void>();
  await page.route(/\/api\/file-locations\?/, async (route) => {
    if (new URL(route.request().url()).searchParams.get("parent") === name)
      await held.promise;
    await route.continue();
  });
  try {
    await page.goto(running.url);
    await expect(page.locator("[data-grid-status]")).toHaveText(
      "Ready · 1 Photo",
    );
    await openSources(page);
    await page
      .getByRole("button", { name: "Toggle Library Folder subfolders" })
      .click();
    await page
      .getByRole("button", { name: `Toggle ${name} subfolders` })
      .click();
    const status = page
      .getByRole("status")
      .filter({ hasText: `Loading subfolders of ${name}` });
    await expect(status).toBeVisible();
    await expect(status).toHaveText(`Loading subfolders of ${name}…`);
    const list = page.locator("[data-source-list]");
    expect(
      await list.evaluate(
        (element) => element.scrollWidth <= element.clientWidth,
      ),
    ).toBe(true);
    expect(
      await status.evaluate((element) => {
        const statusBounds = element.getBoundingClientRect();
        const listBounds = element
          .closest("[data-source-list]")!
          .getBoundingClientRect();
        return (
          statusBounds.left >= listBounds.left &&
          statusBounds.right <= listBounds.right
        );
      }),
    ).toBe(true);
  } finally {
    held.resolve();
  }
});
