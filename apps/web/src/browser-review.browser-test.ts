import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
  chmod,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";

import { expect, test, type Locator, type Page } from "@playwright/test";

import { startBrowserServer, type BrowserServer } from "./browser-server.js";

const sample = process.env.SLIPSTREAM_RAW_SAMPLE;
const temporary: string[] = [];
const servers: BrowserServer[] = [];

test.afterEach(async () => {
  await Promise.all(servers.splice(0).map((server) => server.close()));
  await Promise.all(
    temporary
      .splice(0)
      .map((path) => rm(path, { recursive: true, force: true })),
  );
});

async function jpeg() {
  return readFile(new URL("../test-fixtures/review.jpg", import.meta.url));
}
function withCaptureTime(source: Uint8Array, captureTime: string): Uint8Array {
  const value = new TextEncoder().encode(`${captureTime}\0`);
  const dataOffset = 8 + 2 + 12 + 4;
  const tiff = new Uint8Array(dataOffset + value.length);
  tiff.set([0x49, 0x49, 0x2a, 0, 8, 0, 0, 0]);
  tiff.set([1, 0], 8);
  tiff.set([0x03, 0x90, 2, 0], 10);
  new DataView(tiff.buffer).setUint32(14, value.length, true);
  new DataView(tiff.buffer).setUint32(18, dataOffset, true);
  tiff.set(value, dataOffset);
  const payload = new Uint8Array([69, 120, 105, 102, 0, 0, ...tiff]);
  const app1 = new Uint8Array(payload.length + 4);
  app1.set([
    0xff,
    0xe1,
    (payload.length + 2) >> 8,
    (payload.length + 2) & 0xff,
  ]);
  app1.set(payload, 4);
  return new Uint8Array([...source.slice(0, 2), ...app1, ...source.slice(2)]);
}
async function fixture() {
  const base = await mkdtemp(join(tmpdir(), "slipstream-browser-"));
  temporary.push(base);
  const root = join(base, "originals");
  await mkdir(root);
  return { base, root };
}
async function writePhotos(root: string, count: number) {
  const data = await jpeg();
  for (let index = 0; index < count; index += 1)
    await writeFile(join(root, `${String(index).padStart(3, "0")}.jpg`), data);
}
async function server(base: string, root: string) {
  const running = await startBrowserServer({ base, root });
  servers.push(running);
  // The server binds before its owned startup scan finishes, so tests wait
  // for the Library to settle before driving the UI.
  await expect
    .poll(
      async () => {
        const response = await fetch(`${running.url}/api/status`);
        return ((await response.json()) as { state: string }).state;
      },
      { timeout: 60_000 },
    )
    .toBe("idle");
  return running;
}
async function post(url: string, path: string, body: unknown) {
  return fetch(`${url}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}
type BrowsePhoto = {
  id: string;
  available: boolean;
  selectionState: string;
  rating: number;
};
type SetMember = BrowsePhoto & { photoId: string; position: number };
type SetState = { id: string; position: number; members: SetMember[] };

async function browseWindow(
  url: string,
  token: string,
  start: number,
): Promise<{ start: number; total: number; photos: BrowsePhoto[] }> {
  const window = (await (
    await fetch(`${url}/api/browse/${token}?start=${start}&limit=60`)
  ).json()) as { start: number; total: number; photos: BrowsePhoto[] };
  if (window.start !== start)
    throw new Error("browse window start is inconsistent");
  return window;
}

async function browseIds(url: string): Promise<string[]> {
  const opened = (await (
    await post(url, "/api/browse", { source: "library" })
  ).json()) as { token: string; total: number };
  const ids: string[] = [];
  let start = 0;
  for (;;) {
    const window = await browseWindow(url, opened.token, start);
    if (window.total !== opened.total)
      throw new Error("browse window total is inconsistent");
    ids.push(...window.photos.map((photo) => photo.id));
    start += window.photos.length;
    if (window.photos.length === 0 || start >= opened.total) break;
  }
  await fetch(`${url}/api/browse/${opened.token}`, { method: "DELETE" });
  return ids;
}

async function createSet(url: string, name = "Review") {
  const photos = await browseIds(url);
  const created = (await (await post(url, "/api/albums", { name })).json()) as {
    albums: Array<{ id: string; name: string }>;
  };
  const set = created.albums.find((item) => item.name === name)!;
  for (let offset = 0; offset < photos.length; offset += 100)
    await post(url, `/api/albums/${set.id}/members`, {
      photoIds: photos.slice(offset, offset + 100),
    });
  return { setId: set.id };
}
function progressResponse(page: Page, setId: string, status = 200) {
  return page.waitForResponse(
    (response) =>
      response.url().includes(`/api/albums/${setId}/progress`) &&
      response.request().method() === "POST" &&
      response.status() === status,
  );
}

async function actionWithProgress(
  page: Page,
  setId: string,
  action: () => Promise<unknown>,
) {
  const confirmed = progressResponse(page, setId);
  await action();
  await confirmed;
}

async function waitForGridFrame(page: Page) {
  await expect(page.locator("[data-grid-layer]")).toBeVisible();
  await page.evaluate(
    () =>
      new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
  );
}

async function openPhotoAndWaitForProgress(
  page: Page,
  setId: string,
  photo: Locator,
) {
  const confirmed = progressResponse(page, setId);
  await photo.click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await page.waitForFunction(
    () =>
      Boolean(document.querySelector("[data-stage] img")) ||
      document.body.innerText.includes("Preview unavailable"),
  );
  await confirmed;
}

async function startReview(
  page: Page,
  url: string,
  name = "Review",
  setId?: string,
) {
  await openGrid(page, url, name);
  const photo = page.locator('[data-photo-index="0"]');
  await expect(photo).toHaveAccessibleName(/Photo 1 of/);
  if (setId) {
    await openPhotoAndWaitForProgress(page, setId, photo);
    return;
  }
  await photo.click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await page.waitForFunction(
    () =>
      Boolean(document.querySelector("[data-stage] img")) ||
      document.body.innerText.includes("Preview unavailable"),
  );
}
// Membership order and per-member facts are observable only through a
// fresh Album Browse Snapshot. The resolved open position exposes the
// saved Album position under the unavailable-member fallback rules.
async function state(url: string, setId: string): Promise<SetState> {
  const opened = (await (
    await post(url, "/api/browse", { source: "album", albumId: setId })
  ).json()) as { token: string; total: number; position: number };
  const members: SetMember[] = [];
  let start = 0;
  for (;;) {
    const window = await browseWindow(url, opened.token, start);
    if (window.total !== opened.total)
      throw new Error("browse window total is inconsistent");
    window.photos.forEach((photo, index) =>
      members.push({ ...photo, photoId: photo.id, position: start + index }),
    );
    start += window.photos.length;
    if (window.photos.length === 0 || start >= opened.total) break;
  }
  await fetch(`${url}/api/browse/${opened.token}`, { method: "DELETE" });
  return { id: setId, position: opened.position, members };
}
async function openGrid(page: Page, url: string, name: string) {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(url);
  const escapedName = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  await page
    .getByRole("button", { name: new RegExp(`^${escapedName}(?: |$)`) })
    .click();
  await page.getByText(/^Ready · \d[\d,]* Photos$/).waitFor();
  await waitForGridFrame(page);
}
async function swipe(page: Page, from: number, to: number, y = 320) {
  const preview = page.locator("[data-preview]");
  await preview.dispatchEvent("pointerdown", {
    pointerId: 1,
    isPrimary: true,
    clientX: from,
    clientY: y,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointermove", {
    pointerId: 1,
    isPrimary: true,
    clientX: to,
    clientY: y,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointerup", {
    pointerId: 1,
    isPrimary: true,
    clientX: to,
    clientY: y,
    pointerType: "touch",
  });
}

test("starts from a Album, shows facts, accessible controls, and resumes persisted progress after restart", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg());
  await writeFile(join(root, "b.jpg"), await jpeg());
  let running = await server(base, root);
  const { setId } = await createSet(running.url, "Picks");
  await startReview(page, running.url, "Picks", setId);
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect(page.getByText("Undecided", { exact: true })).toBeVisible();
  await expect(page.getByText("0 stars", { exact: true })).toBeVisible();
  await expect(page.getByText("JPEG", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Limited by camera Preview resolution"),
  ).toBeVisible();
  for (const name of [
    "Select",
    "Reject",
    "Clear",
    "Undo",
    "Previous",
    "Next",
    "Detail Review",
    "Rate 5 stars",
  ])
    await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  // The advanced Photo is the saved Album position.
  await expect
    .poll(async () => (await state(running.url, setId)).position)
    .toBe(1);

  await page.goto("about:blank");
  await running.close();
  servers.splice(servers.indexOf(running), 1);
  running = await server(base, root);
  await page.goto(running.url);
  await page.getByRole("button", { name: /Picks/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /Photo 2 of 2/ }),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await expect(page.getByText("Selected", { exact: true })).toBeVisible();
});

test("visible controls and keyboard share mutation, advance, rating independence, and one-level undo semantics", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url);
  await startReview(page, running.url, "Review", setId);

  await actionWithProgress(page, setId, () => page.keyboard.press("p"));
  await expect(page.getByText("2 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  expect((await state(running.url, setId)).members[0]!.selectionState).toBe(
    "selected",
  );
  await page.keyboard.press("5");
  await expect(page.getByText("5 stars", { exact: true })).toBeVisible();
  expect((await state(running.url, setId)).members[1]).toMatchObject({
    selectionState: "undecided",
    rating: 5,
  });
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Reject" }).click(),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await actionWithProgress(page, setId, () => page.keyboard.press("Control+z"));
  await expect(page.getByText("2 / 3")).toBeVisible();
  await expect(page.getByText("Undecided", { exact: true })).toBeVisible();
  await expect(page.getByText("5 stars", { exact: true })).toBeVisible();
  await expect(page.getByText("Last change undone.")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Clear", exact: true }),
  ).toBeEnabled();
  await page.keyboard.press("u");
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await expect(page.getByText("2 / 3")).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.keyboard.press("ArrowRight"),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
  await actionWithProgress(page, setId, () => page.keyboard.press("ArrowLeft"));
  await expect(page.getByText("2 / 3")).toBeVisible();
});

test("fit-mode Pointer Events show pending feedback, ignore below threshold, and commit right/left only on release", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url);
  await startReview(page, running.url, "Review", setId);

  const preview = page.locator("[data-preview]");
  await preview.dispatchEvent("pointerdown", {
    pointerId: 1,
    isPrimary: true,
    clientX: 120,
    clientY: 320,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointermove", {
    pointerId: 1,
    isPrimary: true,
    clientX: 160,
    clientY: 322,
    pointerType: "touch",
  });
  await expect(page.locator("[data-select-feedback]")).toHaveClass(/pending/);
  expect((await state(running.url, setId)).members[0]!.selectionState).toBe(
    "undecided",
  );
  await preview.dispatchEvent("pointerup", {
    pointerId: 1,
    isPrimary: true,
    clientX: 160,
    clientY: 322,
    pointerType: "touch",
  });
  await expect(page.getByText("1 / 3")).toBeVisible();
  expect((await state(running.url, setId)).members[0]!.selectionState).toBe(
    "undecided",
  );

  let releaseMutation!: () => void;
  const mutationReleased = new Promise<void>((resolve) => {
    releaseMutation = resolve;
  });
  await page.route("**/api/photos/*/state", async (route) => {
    await mutationReleased;
    await route.continue();
  });
  const progressAfterMutation = progressResponse(page, setId);
  await swipe(page, 100, 190);
  await expect(page.getByText("1 / 3")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Back to Grid" }),
  ).toBeDisabled();
  expect((await state(running.url, setId)).members[0]!.selectionState).toBe(
    "undecided",
  );
  releaseMutation();
  await progressAfterMutation;
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.unroute("**/api/photos/*/state");
  expect((await state(running.url, setId)).members[0]!.selectionState).toBe(
    "selected",
  );
  await actionWithProgress(page, setId, () => swipe(page, 250, 150));
  await expect(page.getByText("3 / 3")).toBeVisible();
  expect((await state(running.url, setId)).members[1]!.selectionState).toBe(
    "rejected",
  );
});

test("persistence failure and disconnect do not advance or lie, and explicit Retry recovers in place", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url);
  await startReview(page, running.url, "Review", setId);

  await page.route("**/api/photos/*/state", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: '{"error":"Mutation could not be persisted"}',
    }),
  );
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect(page.getByText(/could not be saved/)).toBeVisible();
  expect((await state(running.url, setId)).members[0]!.selectionState).toBe(
    "undecided",
  );
  await page.unroute("**/api/photos/*/state");

  await page.route("**/api/photos/*/state", (route) => route.abort());
  await page.getByRole("button", { name: "Reject" }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();
  await page.unroute("**/api/photos/*/state");
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Retry" }).click(),
  );
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
});

test("stale undo conflict is visible and zoomed horizontal drag pans without mutating; navigation resets fit", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url);
  await startReview(page, running.url, "Review", setId);

  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Undo" }).click(),
  );
  await expect(page.getByText("1 / 2")).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  const firstId = (await state(running.url, setId)).members[0]!.photoId;
  await post(running.url, `/api/photos/${firstId}/state`, {
    field: "selectionState",
    value: "rejected",
  });
  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByText(/no longer available/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Retry" }).click(),
  );
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();

  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await page.getByRole("button", { name: "Detail Review" }).click();
  await expect(
    page.getByRole("button", { name: "Exit Detail" }),
  ).toHaveAttribute("aria-pressed", "true");
  await swipe(page, 100, 220);
  await expect(page.getByText("1 / 2")).toBeVisible();
  expect((await state(running.url, setId)).members[0]!.selectionState).toBe(
    "rejected",
  );
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Next" }).click(),
  );
  await expect(
    page.getByRole("button", { name: "Detail Review" }),
  ).toHaveAttribute("aria-pressed", "false");
});

test("keeps unavailable Photos ordered and allows their decisions without a Preview", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const missing = join(root, "a.jpg");
  await writeFile(missing, await jpeg());
  await writeFile(join(root, "b.jpg"), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url);
  const initial = await state(running.url, setId);
  await post(running.url, `/api/albums/${setId}/progress`, {
    photoId: initial.members[0]!.photoId,
  });
  await rm(missing);
  await post(running.url, "/api/scan", {});
  await startReview(page, running.url, "Review", setId);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: /Review/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /Photo 2 of 2/ }),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await expect(page.getByText(/Original File is unavailable/)).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  expect((await state(running.url, setId)).members[0]).toMatchObject({
    available: false,
    selectionState: "selected",
  });
});

test("file locations show a bounded tree and open recursive folder sources", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "Trip"));
  await mkdir(join(root, "Trip/day2"));
  await mkdir(join(root, "Trip-extra"));
  await mkdir(join(root, "My Photos"));
  const data = await jpeg();
  for (const name of [
    "root.jpg",
    "Trip/one.jpg",
    "Trip/day2/two.jpg",
    "Trip-extra/three.jpg",
    "My Photos/space.jpg",
  ])
    await writeFile(join(root, name), data);
  const running = await server(base, root);
  await post(running.url, "/api/albums", { name: "Trip" });
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  // File Locations and Albums remain separate sections; a same-name Folder
  // and Album stay distinguishable by section.
  await expect(
    page.getByRole("heading", { name: "File Locations" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Albums" }).first(),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Library Folder/ }),
  ).toBeVisible();

  // Expanding the root loads one bounded direct-child window.
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  await expect(
    page.getByRole("button", { name: /Trip · Subfolders/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Trip-extra 1 Photos" }),
  ).toBeVisible();
  // The same-name Album remains present in its own section.
  await expect(
    page.getByRole("button", { name: /Trip 0 Photos/ }),
  ).toBeVisible();

  // Expanding a child loads its own direct-child window.
  await page.getByRole("button", { name: "Toggle Trip subfolders" }).click();
  await expect(
    page.getByRole("button", { name: /day2 1 Photos/ }),
  ).toBeVisible();

  // Opening the folder source shows the recursive subtree count.
  await page.getByRole("button", { name: /Trip · Subfolders/ }).click();
  await expect(page.getByText("Ready · 2 Photos")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Trip · Folder" }),
  ).toBeVisible();

  // The component-aware rule keeps the same-prefix sibling separate.
  await page.getByRole("button", { name: /Trip-extra 1 Photos/ }).click();
  await expect(page.getByText("Ready · 1 Photos")).toBeVisible();

  // A Folder name containing a space opens through decoded query values.
  await page.getByRole("button", { name: /My Photos 1 Photos/ }).click();
  await expect(page.getByText("Ready · 1 Photos")).toBeVisible();

  // The Library Folder root source covers the whole Published Library.
  await page.getByRole("button", { name: /^Library Folder/ }).click();
  await expect(page.getByText("Ready · 5 Photos")).toBeVisible();
});

test("an empty Library still shows and opens the Library Folder root", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Library Folder 0 Photos/ }),
  ).toBeVisible();
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  await expect(page.getByRole("button", { name: "More Folders" })).toBeHidden();
  await page.getByRole("button", { name: /^Library Folder 0 Photos/ }).click();
  await expect(page.getByText("No Photos in this source")).toBeVisible();
});

test("file location publication values stay unique across server restarts", async () => {
  const { base, root } = await fixture();
  const data = await jpeg();
  await writeFile(join(root, "one.jpg"), data);
  const first = await server(base, root);
  const responseOne = await fetch(
    `${first.url}/api/file-locations?start=0&limit=60`,
  );
  const windowOne = (await responseOne.json()) as { publication: string };
  await first.close();
  const second = await server(base, root);
  const responseTwo = await fetch(
    `${second.url}/api/file-locations?start=0&limit=60`,
  );
  const windowTwo = (await responseTwo.json()) as { publication: string };
  expect(windowOne.publication).not.toBe(windowTwo.publication);
});

test("a failed folder source open reconnects to the same folder, not All Photos", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "shoot"), { recursive: true });
  const data = await jpeg();
  await writeFile(join(root, "shoot/one.jpg"), data);
  await writeFile(join(root, "root.jpg"), data);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  await expect(
    page.getByRole("button", { name: /shoot 1 Photos/ }),
  ).toBeVisible();

  // The first folder-source open fails; the retry must reopen the same
  // folder source instead of silently falling back to All Photos.
  await page.route(
    /\/api\/browse/,
    async (route) => {
      const request = route.request();
      const body = request.postDataBuffer();
      if (
        request.method() === "POST" &&
        body?.toString().includes("folderPath") &&
        body.toString().includes("shoot")
      )
        await route.abort();
      else await route.continue();
    },
    { times: 1 },
  );
  await page.getByRole("button", { name: /shoot 1 Photos/ }).click();
  await expect(
    page.getByText("Could not load this source. Retry to continue."),
  ).toBeVisible();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "shoot · Folder" }),
  ).toBeVisible();
  await expect(page.getByText("Ready · 1 Photos")).toBeVisible();
});

test("a remembered folder source waits for the File Location binding before reopening", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "shoot"));
  const data = await jpeg();
  await writeFile(join(root, "shoot/one.jpg"), data);
  await writeFile(join(root, "root.jpg"), data);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  await expect(
    page.getByRole("button", { name: /shoot 1 Photos/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: /shoot 1 Photos/ }).click();
  await expect(
    page.getByRole("heading", { name: "shoot · Folder" }),
  ).toBeVisible();

  // Both the Folder-source reopen and the File Location binding fail.
  let folderOpens = 0;
  await page.route(/\/api\/browse/, async (route) => {
    const body = route.request().postDataBuffer()?.toString() ?? "";
    if (route.request().method() === "POST" && body.includes("folderPath")) {
      folderOpens += 1;
      await route.abort();
      return;
    }
    await route.continue();
  });
  await page.route(/\/api\/file-locations/, (route) => route.abort());
  await page.getByRole("button", { name: "Refresh" }).click();
  await expect(
    page.getByText("Could not load this source. Retry to continue."),
  ).toBeVisible();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  const opensAfterRefresh = folderOpens;

  // The global Retry cannot bind File Locations, so it must NOT send a
  // publicationless Folder open: the truthful failure stays visible.
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(
    page.getByText("Could not load this source. Retry to continue."),
  ).toBeVisible();
  expect(folderOpens).toBe(opensAfterRefresh);

  // Once the binding and the source route recover, the same Retry reopens
  // the remembered Folder.
  await page.unroute(/\/api\/file-locations/);
  await page.unroute(/\/api\/browse/);
  // The Retry control hides as soon as the overview reconnects, so the
  // click is dispatched before that stability transition can hide it.
  await page.evaluate(() =>
    document.querySelector<HTMLButtonElement>("[data-retry]")?.click(),
  );
  await expect(
    page.getByRole("heading", { name: "shoot · Folder" }),
  ).toBeVisible();
  await expect(page.getByText("Ready · 1 Photos")).toBeVisible();
});

test("delayed File Location responses from a superseded publication are discarded", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "a/sub"), { recursive: true });
  const data = await jpeg();
  await writeFile(join(root, "a/sub/one.jpg"), data);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  await expect(
    page.getByRole("button", { name: /a · Subfolders/ }),
  ).toBeVisible();

  // Deliver one successful child window for `a` only after the publication
  // has been superseded and the browser has reloaded the current root.
  let release: (() => void) | undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(/\/api\/file-locations\?.*parent=a&/, async (route) => {
    const response = await route.fetch();
    await released;
    await route.fulfill({ response });
  });
  await page.getByRole("button", { name: "Toggle a subfolders" }).click();
  await writeFile(join(root, "a/sub/two.jpg"), data);
  await post(running.url, "/api/scan", {});
  await page.waitForFunction(async () => {
    const response = await fetch("/api/overview");
    const overview = (await response.json()) as { scan: { state: string } };
    return overview.scan.state === "idle";
  });
  const rootToggle = page.getByRole("button", {
    name: "Toggle Library Folder subfolders",
  });
  await rootToggle.click();
  await rootToggle.click();
  await expect(
    page.getByText(
      "Scan results changed File Locations. Reloaded the current Folders.",
    ),
  ).toBeVisible();

  // The delayed superseded window must not expand `a`: if it had been
  // accepted, this click would collapse it instead of loading the fresh
  // page, and the fresh recursive count would never appear.
  release!();
  await page.getByRole("button", { name: "Toggle a subfolders" }).click();
  await expect(
    page.getByRole("button", { name: /sub 2 Photos/ }),
  ).toBeVisible();
});

test("failed File Location ranges keep siblings and retry only the failed range", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "shoot/nested"), { recursive: true });
  const data = await jpeg();
  await writeFile(join(root, "shoot/one.jpg"), data);
  await writeFile(join(root, "shoot/nested/two.jpg"), data);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();

  let failing = true;
  await page.route(/\/api\/file-locations\?.*parent=shoot&/, async (route) => {
    if (failing) await route.abort();
    else await route.continue();
  });
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  await expect(
    page.getByRole("button", { name: /shoot · Subfolders/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Toggle shoot subfolders" }).click();
  await expect(
    page.getByText(/Could not load File Locations \(shoot items 1–60\)/),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Library Folder/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", {
      name: /^Retry File Locations \(shoot items 1–60\)/,
    }),
  ).toBeVisible();
  failing = false;
  await page.getByRole("button", { name: /^Retry File Locations/ }).click();
  // Retrying loads only the failed range: the sibling child appears while
  // the already loaded root navigation stays intact.
  await expect(
    page.getByRole("button", { name: /nested 1 Photos/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /shoot · Subfolders/ }),
  ).toBeVisible();
  await expect(page.getByText(/Could not load File Locations/)).toBeHidden();
});

test("file locations reload coherently when a scan replaces the publication", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await mkdir(join(root, "shoot"));
  const data = await jpeg();
  await writeFile(join(root, "shoot/one.jpg"), data);
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await page
    .getByRole("button", { name: "Toggle Library Folder subfolders" })
    .click();
  await expect(
    page.getByRole("button", { name: /shoot 1 Photos/ }),
  ).toBeVisible();

  // A rescan that adds a Folder supersedes the retained publication.
  await mkdir(join(root, "later"));
  await writeFile(join(root, "later/two.jpg"), data);
  await post(running.url, "/api/scan", {});
  await page.waitForFunction(async () => {
    const response = await fetch("/api/overview");
    const overview = (await response.json()) as { scan: { state: string } };
    return overview.scan.state === "idle";
  });
  // Collapsing and re-expanding sends the superseded publication value; the
  // app reloads one coherent current publication instead of mixing windows.
  const rootToggle = page.getByRole("button", {
    name: "Toggle Library Folder subfolders",
  });
  await rootToggle.click();
  await rootToggle.click();
  await expect(
    page.getByText(
      "Scan results changed File Locations. Reloaded the current Folders.",
    ),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /later 1 Photos/ }),
  ).toBeVisible();
});

test("shows empty and no-album start states and only uses same-service requests", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg());
  const running = await server(base, root);
  const methods: string[] = [];
  page.on("request", (request) => methods.push(request.method()));
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await post(running.url, "/api/albums", { name: "Empty" });
  await page.reload();
  const empty = page.getByRole("button", { name: /Empty/ });
  await expect(empty).toBeVisible();
  // Empty Albums stay openable: they are valid sources, not disabled cards.
  await expect(empty).toBeEnabled();
  await empty.click();
  await expect(empty).toHaveClass(/active/);
  await createSet(running.url, "Ready");
  await page.reload();
  await expect(page.getByRole("button", { name: /Ready/ })).toBeEnabled();
  expect(
    methods.every(
      (method) => method === "GET" || method === "POST" || method === "DELETE",
    ),
    methods.join(","),
  ).toBe(true);
});

test("persists manual navigation and advanced current Photo across leave, reload, and restart", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  let running = await server(base, root);
  const { setId } = await createSet(running.url, "Progress");
  await startReview(page, running.url, "Progress", setId);
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Next" }).click(),
  );
  await expect
    .poll(async () => (await state(running.url, setId)).position)
    .toBe(1);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: /Progress/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /Photo 2 of 3/ }),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.reload();
  await page.getByRole("button", { name: /Progress/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /Photo 2 of 3/ }),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect
    .poll(async () => (await state(running.url, setId)).position)
    .toBe(2);
  await page.goto("about:blank");
  await running.close();
  servers.splice(servers.indexOf(running), 1);
  running = await server(base, root);
  await page.goto(running.url);
  await page.getByRole("button", { name: /Progress/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /Photo 3 of 3/ }),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
});

test("binds gestures to their starting Photo and covers exact thresholds, cancellation, and disconnect", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url);
  await startReview(page, running.url, "Review", setId);
  const preview = page.locator("[data-preview]");
  const event = async (type: string, x: number, time: number, pointerId = 41) =>
    preview.evaluate(
      (element, value) => {
        const item = new PointerEvent(value.type, {
          pointerId: value.pointerId,
          isPrimary: true,
          clientX: value.x,
          clientY: 320,
          pointerType: "touch",
          bubbles: true,
        });
        Object.defineProperty(item, "timeStamp", { value: value.time });
        element.dispatchEvent(item);
      },
      { type, x, time, pointerId },
    );

  await event("pointerdown", 100, 0);
  await actionWithProgress(page, setId, () =>
    page.keyboard.press("ArrowRight"),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
  await event("pointerup", 200, 10);
  expect(
    (await state(running.url, setId)).members
      .slice(0, 2)
      .map((x) => x.selectionState),
  ).toEqual(["undecided", "undecided"]);

  await event("pointerdown", 100, 0, 42);
  await event("pointermove", 171, 1000, 42);
  await event("pointerup", 171, 1000, 42);
  expect((await state(running.url, setId)).members[1]!.selectionState).toBe(
    "undecided",
  );
  await actionWithProgress(page, setId, async () => {
    await event("pointerdown", 100, 0, 43);
    await event("pointermove", 172, 1000, 43);
    await event("pointerup", 172, 1000, 43);
  });
  await expect(page.getByText("3 / 3")).toBeVisible();

  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await actionWithProgress(page, setId, async () => {
    await event("pointerdown", 100, 0, 44);
    await event("pointermove", 148, 50, 44);
    await event("pointerup", 148, 50, 44);
  });
  await expect(page.getByText("3 / 3")).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await event("pointerdown", 100, 0, 45);
  await event("pointermove", 148, 1000, 45);
  await event("pointerup", 148, 1000, 45);
  await expect(page.getByText("2 / 3")).toBeVisible();

  await event("pointerdown", 100, 0, 46);
  await event("pointermove", 200, 10, 46);
  await event("pointercancel", 200, 10, 46);
  await expect(page.locator("[data-select-feedback]")).not.toHaveClass(
    /pending/,
  );
  await event("pointerdown", 100, 0, 47);
  await event("pointermove", 200, 10, 47);
  await preview.dispatchEvent("lostpointercapture", { pointerId: 47 });
  await event("pointerup", 200, 10, 47);
  expect((await state(running.url, setId)).members[1]!.selectionState).toBe(
    "selected",
  );

  await page.route("**/api/photos/*/preview", (route) => route.abort());
  await page.reload();
  await page.getByRole("button", { name: /^Review(?: |$)/ }).click();
  await actionWithProgress(page, setId, async () => {
    await page.getByRole("button", { name: /Photo 1 of/ }).click();
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  });
  await event("pointerdown", 100, 0, 48);
  await event("pointermove", 200, 10, 48);
  await expect(page.locator("[data-select-feedback]")).not.toHaveClass(
    /pending/,
  );
});

test("keyboard works from focused buttons, real client deltas pan, and uncertain mutation retires undo", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url);
  await startReview(page, running.url, "Review", setId);

  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Next" }).click(),
  );
  await actionWithProgress(page, setId, () => page.keyboard.press("p"));
  await expect(page.getByText("3 / 3")).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await page.keyboard.press("5");
  await expect(page.getByText("5 stars", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Detail Review" }).click();
  const image = page.locator("[data-stage] img");
  const preview = page.locator("[data-preview]");
  await preview.dispatchEvent("pointerdown", {
    pointerId: 61,
    isPrimary: true,
    clientX: 100,
    clientY: 300,
    pointerType: "touch",
  });
  await preview.dispatchEvent("pointermove", {
    pointerId: 61,
    isPrimary: true,
    clientX: 140,
    clientY: 330,
    pointerType: "touch",
  });
  await expect(image).toHaveCSS("transform", /matrix\(2, 0, 0, 2, 40, 30\)/);
  await preview.dispatchEvent("pointerup", {
    pointerId: 61,
    isPrimary: true,
    clientX: 140,
    clientY: 330,
    pointerType: "touch",
  });
  expect((await state(running.url, setId)).members[1]!.selectionState).toBe(
    "selected",
  );
  await page.getByRole("button", { name: "Exit Detail" }).click();
  await actionWithProgress(page, setId, () => page.keyboard.press("x"));
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await actionWithProgress(page, setId, () => page.keyboard.press("Control+z"));
  await expect(page.getByText("2 / 3")).toBeVisible();

  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Previous" }).click(),
  );
  await page.route("**/api/photos/*/state", async (route) => {
    await route.fetch();
    await route.abort();
  });
  await page.getByRole("button", { name: "Reject" }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
  await page.unroute("**/api/photos/*/state");
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Retry" }).click(),
  );
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
});

test("shows matching JPEG then RAW embedded JPEG through the mobile production Review Session", async ({
  page,
}) => {
  test.skip(!sample, "Set SLIPSTREAM_RAW_SAMPLE for the camera smoke");
  const cameraSample = sample!;
  const before = await sha256(cameraSample);
  const { base, root } = await fixture();
  const raw = join(root, `camera${extname(cameraSample)}`);
  const matching = join(root, "camera.jpg");
  await copyFile(cameraSample, raw);
  await writeFile(matching, await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url);
  await startReview(page, running.url, "Review", setId);
  await expect(page.getByText("JPEG", { exact: true })).toBeVisible();
  await rm(matching);
  await post(running.url, "/api/scan", {});
  await page.reload();
  await page.getByRole("button", { name: /^Review(?: |$)/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /Photo 1 of/ }),
  );
  await expect(
    page.getByText("RAW embedded JPEG", { exact: true }),
  ).toBeVisible();
  expect(await sha256(cameraSample)).toBe(before);
});

async function sha256(path: string): Promise<string> {
  return createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
}

test("Library Review uses server Capture Time order, snapshots it, and stores no progress", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  await writeFile(
    join(root, "A.jpg"),
    withCaptureTime(source, "2026:01:01 10:00:00"),
  );
  await writeFile(
    join(root, "Z.jpg"),
    withCaptureTime(source, "2026:01:01 09:00:00"),
  );
  const running = await server(base, root);
  const ordered = await browseIds(running.url);
  expect(ordered).toHaveLength(2);
  const zId = ordered[0]!;
  const aId = ordered[1]!;
  const previewRequests: string[] = [];
  const stateBodies: unknown[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/preview")) previewRequests.push(request.url());
    if (request.url().includes("/state"))
      stateBodies.push(request.postDataJSON());
  });
  await page.goto(running.url);
  await page.getByRole("button", { name: /Photo 1 of 2/ }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect
    .poll(() => previewRequests.some((url) => url.includes(zId)))
    .toBe(true);
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  expect(stateBodies[0]).toMatchObject({
    field: "selectionState",
    value: "selected",
  });
  expect(stateBodies[0]).not.toHaveProperty("albumId");
  const overview = (await (
    await fetch(`${running.url}/api/overview`)
  ).json()) as {
    albums: unknown[];
  };
  expect(overview.albums).toEqual([]);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: /All Photos/ }).click();
  await page.getByRole("button", { name: /Photo 1 of 2/ }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();

  const { setId } = await createSet(running.url, "Explicit order");
  await post(running.url, `/api/albums/${setId}/order`, {
    photoIds: [aId, zId],
  });
  await page.reload();
  await page.getByRole("button", { name: /^Explicit order(?: |$)/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /Photo 1 of 2/ }),
  );
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect
    .poll(() => previewRequests.some((url) => url.includes(aId)))
    .toBe(true);
});

test("active Library Review keeps its Capture Time snapshot until the next Session", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  await writeFile(
    join(root, "A.jpg"),
    withCaptureTime(source, "2026:01:01 10:00:00"),
  );
  await writeFile(
    join(root, "Z.jpg"),
    withCaptureTime(source, "2026:01:01 09:00:00"),
  );
  const running = await server(base, root);
  const initialIds = await browseIds(running.url);
  const zId = initialIds[0]!;
  const previews: string[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/preview")) previews.push(request.url());
  });
  await page.goto(running.url);
  await page.getByRole("button", { name: /Photo 1 of 2/ }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();
  await expect.poll(() => previews.some((url) => url.includes(zId))).toBe(true);
  await writeFile(
    join(root, "B.jpg"),
    withCaptureTime(source, "2026:01:01 08:00:00"),
  );
  await post(running.url, "/api/scan", {});
  await page.route("**/api/photos/*/preview", (route) => route.abort());
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await page.unroute("**/api/photos/*/preview");
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: /All Photos/ }).click();
  await page.getByRole("button", { name: /Photo 1 of 3/ }).click();
  await expect(page.getByText("1 / 3")).toBeVisible();
  const expandedIds = await browseIds(running.url);
  const bId = expandedIds.find((id) => !initialIds.includes(id));
  expect(bId).toBeDefined();
  await expect
    .poll(() => previews.some((url) => url.includes(bId!)))
    .toBe(true);
});

test("Album Review snapshots explicit members across rescan and reconnect", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  await writeFile(
    join(root, "A.jpg"),
    withCaptureTime(source, "2026:01:01 10:00:00"),
  );
  await writeFile(
    join(root, "Z.jpg"),
    withCaptureTime(source, "2026:01:01 09:00:00"),
  );
  const running = await server(base, root);
  const initialIds = await browseIds(running.url);
  const aId = initialIds[1]!;
  const zId = initialIds[0]!;
  const { setId } = await createSet(running.url, "Snapshot");
  await post(running.url, `/api/albums/${setId}/order`, {
    photoIds: [aId, zId],
  });
  await startReview(page, running.url, "Snapshot", setId);
  await expect(page.getByText("1 / 2")).toBeVisible();

  await writeFile(
    join(root, "B.jpg"),
    withCaptureTime(source, "2026:01:01 08:00:00"),
  );
  await post(running.url, "/api/scan", {});
  const rescannedIds = await browseIds(running.url);
  const bId = rescannedIds.find((id) => !initialIds.includes(id));
  expect(bId).toBeDefined();
  await post(running.url, `/api/albums/${setId}/members`, {
    photoIds: [bId!],
  });
  await page.route("**/api/photos/*/preview", (route) => route.abort());
  await actionWithProgress(page, setId, async () => {
    await page.getByRole("button", { name: "Next" }).click();
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  });
  await page.unroute("**/api/photos/*/preview");
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Retry" }).click(),
  );
  await expect(page.getByText("2 / 2")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: /^Snapshot(?: |$)/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /Photo 2 of 3/ }),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
});

test("reconnect retains confirmed undo and a delayed progress failure blocks the active Session", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url, "Recovery");
  await startReview(page, running.url, "Recovery", setId);
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await page.route("**/api/photos/*/preview", (route) => route.abort());
  await actionWithProgress(page, setId, async () => {
    await page.getByRole("button", { name: "Next" }).click();
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  });
  await page.unroute("**/api/photos/*/preview");
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Retry" }).click(),
  );
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Undo" }).click(),
  );
  await expect(page.getByText("1 / 3")).toBeVisible();

  let releaseFailure!: () => void;
  const failureReleased = new Promise<void>((resolve) => {
    releaseFailure = resolve;
  });
  let failed = false;
  await page.route("**/api/albums/*/progress", async (route) => {
    if (!failed) {
      failed = true;
      await failureReleased;
      await route.fulfill({ status: 503, body: '{"error":"failed"}' });
      return;
    }
    await route.continue();
  });
  const progressFailed = progressResponse(page, setId, 503);
  const progressAfterFailure = progressResponse(page, setId);
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("3 / 3")).toBeVisible();
  releaseFailure();
  await progressFailed;
  await progressAfterFailure;
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();
  await page.unroute("**/api/albums/*/progress");
  let releaseSuccess!: () => void;
  const successReleased = new Promise<void>((resolve) => {
    releaseSuccess = resolve;
  });
  await page.route("**/api/albums/*/progress", async (route) => {
    await successReleased;
    await route.continue();
  });
  const progressRecovered = progressResponse(page, setId);
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();
  releaseSuccess();
  await progressRecovered;
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
  await page.unroute("**/api/albums/*/progress");
  await expect
    .poll(async () => (await state(running.url, setId)).position)
    .toBe(2);
});

test("Album resume wraps past an unavailable saved member and retains it when all are unavailable", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url, "Resume");
  const initial = await state(running.url, setId);
  const savedId = initial.members[2]!.photoId;
  expect(
    initial.members.findIndex((member) => member.photoId === savedId),
  ).toBe(2);
  await post(running.url, `/api/albums/${setId}/progress`, {
    photoId: savedId,
  });
  await rm(join(root, "c.jpg"));
  await post(running.url, "/api/scan", {});
  await page.goto(running.url);
  await page.getByRole("button", { name: /^Resume(?: |$)/ }).click();
  // The saved position becomes durable only when the page's progress write
  // is confirmed, and that write is asynchronous with Photo View. Waiting
  // for the confirmed POST removes the race where a reload could discard a
  // pending write and leave the saved member unchanged.
  const progressConfirmed = page.waitForResponse(
    (response) =>
      response.url().includes(`/api/albums/${setId}/progress`) &&
      response.request().method() === "POST" &&
      response.status() === 200,
  );
  await page.getByRole("button", { name: /Photo 1 of 3/ }).click();
  await expect(page.getByText("1 / 3")).toBeVisible();
  await progressConfirmed;

  await page.getByRole("button", { name: "Back to Grid" }).click();
  await rm(join(root, "a.jpg"));
  await rm(join(root, "b.jpg"));
  await post(running.url, "/api/scan", {});
  // Every member is unavailable now, yet membership is retained; a fresh
  // snapshot resolves the saved position to its member index under the
  // fallback rules (the page moved the saved position to member 0 when it
  // opened Photo 1 earlier).
  await page.reload();
  await expect
    .poll(async () => {
      const retained = await state(running.url, setId);
      return {
        available: retained.members.map((member) => member.available),
        position: retained.position,
      };
    })
    .toEqual({ available: [false, false, false], position: 0 });
  await page.getByRole("button", { name: /^Resume(?: |$)/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /Photo 3 of 3/ }),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
});

test("ready Preview facts render immediately during revalidation", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "photo.jpg"), await jpeg());
  const running = await server(base, root);
  await openGrid(page, running.url, "All Photos");
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.locator("[data-stage] img")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);

  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/photos/*/preview", (route) =>
    held.then(() => route.continue()),
  );
  const revalidated = page.waitForResponse(
    (response) =>
      response.url().includes("/api/photos/") &&
      response.url().endsWith("/preview") &&
      response.status() === 200,
  );
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await expect(page.locator("[data-stage] img")).toBeVisible();
  await expect(page.getByText("Loading Preview…")).toHaveCount(0);
  const ratingResponse = page.waitForResponse(
    (response) =>
      response.url().includes("/api/photos/") &&
      response.url().endsWith("/state") &&
      response.status() === 200,
  );
  await page.getByRole("button", { name: "Rate 5 stars" }).click();
  await ratingResponse;
  await expect(page.getByText("5 stars")).toBeVisible();
  release();
  await revalidated;
  await expect(page.getByText("5 stars")).toBeVisible();
  await page.unroute("**/api/photos/*/preview");
});

test("Grid thumbnails survive virtual re-renders without refetching", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (let index = 0; index < 8; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(2, "0")}.jpg`),
      await jpeg(),
    );
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  const thumbnailRequests: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.endsWith("/thumbnail"))
      thumbnailRequests.push(request.url());
  });
  await page.goto(running.url);
  const loadedThumbnails = () =>
    page.evaluate(
      () =>
        Array.from(
          document.querySelectorAll<HTMLImageElement>(".photo-cell img"),
        ).filter((image) => Boolean(image.getAttribute("src"))).length,
    );
  await expect.poll(loadedThumbnails).toBe(8);
  expect(thumbnailRequests).toHaveLength(8);
  expect(
    await page
      .locator(".photo-cell img")
      .evaluateAll((images) =>
        images.every(
          (image) =>
            image.getAttribute("fetchpriority") === "low" &&
            image.getAttribute("decoding") === "async",
        ),
      ),
  ).toBe(true);
  const viewport = page.locator("[data-grid-viewport]");
  for (let _ = 0; _ < 4; _ += 1) {
    await viewport.evaluate((element) =>
      element.dispatchEvent(new Event("scroll")),
    );
    await expect.poll(loadedThumbnails).toBe(8);
  }
  expect(thumbnailRequests).toHaveLength(8);
  await expect.poll(loadedThumbnails).toBe(8);
});

test("hydrated Grid thumbnails render without thumbnail API requests", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (let index = 0; index < 8; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(2, "0")}.jpg`),
      await jpeg(),
    );
  const running = await server(base, root);
  const ids = await browseIds(running.url);
  for (const id of ids) {
    const response = await fetch(`${running.url}/api/photos/${id}/thumbnail`);
    expect(response.ok).toBe(true);
  }

  await page.setViewportSize({ width: 390, height: 844 });
  const thumbnailRequests: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.endsWith("/thumbnail"))
      thumbnailRequests.push(request.url());
  });
  await page.goto(running.url);
  const renderedThumbnails = () => page.locator(".photo-cell img").count();
  await expect.poll(renderedThumbnails).toBe(8);
  expect(thumbnailRequests).toHaveLength(0);
  const thumbnail = page.locator(".photo-cell img").first();
  await expect(thumbnail).toHaveAttribute(
    "src",
    /\/api\/derivatives\/[^/]+\/thumbnail\/[^/]+\.jpg$/,
  );
  await expect(thumbnail).toHaveAttribute("fetchpriority", "low");
  await expect(thumbnail).toHaveAttribute("decoding", "async");
  await thumbnail.click();
  const preview = page.locator("[data-stage] img");
  await expect(preview).toBeVisible();
  await expect(preview).toHaveAttribute("fetchpriority", "high");
  await expect(preview).toHaveAttribute("decoding", "async");
});

test("source switching reaches Ready while Grid derivatives remain held", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 70);
  const running = await server(base, root);
  await createSet(running.url, "Held Derivatives");

  let release!: () => void;
  const derivativesHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let derivativeRequests = 0;
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.includes("/api/derivatives/"))
      derivativeRequests += 1;
  });
  await page.route("**/api/derivatives/**", (route) =>
    derivativesHeld.then(() => route.continue()).catch(() => undefined),
  );
  try {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(running.url);
    await expect(page.getByText(/^Ready · 70 Photos$/)).toBeVisible();
    await expect.poll(() => derivativeRequests).toBeGreaterThan(0);

    const viewport = page.locator("[data-grid-viewport]");
    const requestsBeforeScroll = derivativeRequests;
    const scrolledDerivative = page.waitForRequest((request) =>
      new URL(request.url()).pathname.includes("/api/derivatives/"),
    );
    await viewport.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
      element.dispatchEvent(new Event("scroll"));
    });
    await scrolledDerivative;
    expect(derivativeRequests).toBeGreaterThan(requestsBeforeScroll);

    const pendingGridImage = await page
      .locator(".photo-cell img")
      .first()
      .elementHandle();
    expect(pendingGridImage).not.toBeNull();
    await page.getByRole("button", { name: "Refresh" }).click();
    await expect(page.locator("[data-grid-title]")).toHaveText("All Photos");
    await expect(page.getByText(/^Ready · 70 Photos$/)).toBeVisible();
    expect(
      await pendingGridImage!.evaluate((image) => image.hasAttribute("src")),
    ).toBe(false);

    await page
      .getByRole("button", { name: /^Held Derivatives 70 Photos/ })
      .click();
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Held Derivatives",
    );
    await expect(page.getByText(/^Ready · 70 Photos$/)).toBeVisible();
    expect(derivativeRequests).toBeGreaterThan(1);
  } finally {
    release();
    await page.unroute("**/api/derivatives/**");
  }
});

test("leaving Photo View cancels a pending review image transfer", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "photo.jpg"), await jpeg());
  const running = await server(base, root);
  const [photoId] = await browseIds(running.url);
  const previewResponse = await fetch(
    `${running.url}/api/photos/${photoId}/preview`,
  );
  expect(previewResponse.ok).toBe(true);
  const preview = (await previewResponse.json()) as { url?: string };
  expect(preview.url).toContain("/review/");

  let release!: () => void;
  const reviewHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/derivatives/**/review/**", (route) =>
    reviewHeld.then(() => route.continue()).catch(() => undefined),
  );
  try {
    await page.goto(running.url);
    const reviewRequest = page.waitForRequest((request) =>
      new URL(request.url()).pathname.includes("/review/"),
    );
    await page.locator('[data-photo-index="0"]').click();
    await reviewRequest;
    const pendingReviewImage = await page
      .locator("[data-stage] img")
      .elementHandle();
    expect(pendingReviewImage).not.toBeNull();

    await page.getByRole("button", { name: "Back to Grid" }).click();
    await expect(page.getByText(/^Ready · 1 Photos$/)).toBeVisible();
    expect(
      await pendingReviewImage!.evaluate((image) => image.hasAttribute("src")),
    ).toBe(false);
  } finally {
    release();
    await page.unroute("**/api/derivatives/**/review/**");
  }
});

test("source switching aborts a pending current-Photo Preview request", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 8);
  const running = await server(base, root);
  await createSet(running.url, "Preview Abort Target");

  let release!: () => void;
  const previewHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let heldUrl: string | undefined;
  await page.route("**/api/photos/*/preview", (route) => {
    heldUrl = route.request().url();
    return previewHeld.then(() => route.continue()).catch(() => undefined);
  });
  try {
    await page.goto(running.url);
    await page.locator('[data-photo-index="0"]').click();
    await expect.poll(() => heldUrl).toBeTruthy();
    const canceledUrl = heldUrl!;
    const previewCanceled = page.waitForEvent(
      "requestfailed",
      (request) => request.url() === canceledUrl,
    );

    await page
      .getByRole("button", { name: /^Preview Abort Target 8 Photos/ })
      .click();
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Preview Abort Target",
    );
    await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();
    await previewCanceled;
  } finally {
    release();
    await page.unroute("**/api/photos/*/preview");
  }
});

test("opening Photo View aborts Grid fallback thumbnail requests", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 8);
  const running = await server(base, root);

  let release!: () => void;
  const thumbnailHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let heldUrl: string | undefined;
  await page.route("**/api/photos/*/thumbnail", (route) => {
    heldUrl = route.request().url();
    return thumbnailHeld.then(() => route.continue()).catch(() => undefined);
  });
  try {
    await page.goto(running.url);
    await expect.poll(() => heldUrl).toBeTruthy();
    const canceledUrl = heldUrl!;
    const thumbnailCanceled = page.waitForEvent(
      "requestfailed",
      (request) => request.url() === canceledUrl,
    );

    await page.locator('[data-photo-index="0"]').click();
    await expect(page.getByText("1 / 8")).toBeVisible();
    await thumbnailCanceled;
  } finally {
    release();
    await page.unroute("**/api/photos/*/thumbnail");
  }
});

test("a superseded source open is aborted before the newer source renders", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 8);
  const running = await server(base, root);
  const { setId: firstSetId } = await createSet(running.url, "First Source");
  await createSet(running.url, "Second Source");

  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();

  let release!: () => void;
  const staleGate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let staleHeld = false;
  await page.route("**/api/browse", async (route) => {
    const request = route.request();
    if (
      request.method() === "POST" &&
      (request.postDataJSON() as { albumId?: string }).albumId === firstSetId &&
      !staleHeld
    ) {
      staleHeld = true;
      await staleGate;
    }
    try {
      await route.continue();
    } catch {
      /* a newer source may abort the intercepted request */
    }
  });
  try {
    await page.getByRole("button", { name: /^First Source 8 Photos/ }).click();
    await expect.poll(() => staleHeld).toBe(true);
    const staleCanceled = page.waitForEvent("requestfailed", (request) => {
      if (
        request.method() !== "POST" ||
        new URL(request.url()).pathname !== "/api/browse"
      )
        return false;
      return (
        (request.postDataJSON() as { albumId?: string }).albumId === firstSetId
      );
    });
    await page.getByRole("button", { name: /^Second Source 8 Photos/ }).click();
    await expect(page.locator("[data-grid-title]")).toHaveText("Second Source");
    await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();

    await staleCanceled;
    await expect(page.locator("[data-grid-title]")).toHaveText("Second Source");
    await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();
  } finally {
    release();
    await page.unroute("**/api/browse");
  }
});

test("source changes invalidate queued Grid renders", async ({ page }) => {
  const { base, root } = await fixture();
  await writePhotos(root, 8);
  const running = await server(base, root);
  await createSet(running.url, "Boundary Source");

  const openedResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/browse",
  );
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  const opened = (await (await openedResponse).json()) as { token: string };
  const oldToken = opened.token;
  await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();
  await page.locator('[data-photo-index="0"]').click();
  await expect(page.locator("[data-review]")).toBeVisible();

  let staleWindowRequests = 0;
  await page.route("**/api/browse/**", async (route) => {
    const request = route.request();
    if (
      request.method() === "GET" &&
      request.url().includes(`/api/browse/${oldToken}?`)
    )
      staleWindowRequests += 1;
    await route.continue();
  });
  try {
    await page.evaluate(() => {
      document.querySelector<HTMLButtonElement>("[data-back]")!.click();
      document
        .querySelector<HTMLElement>("[data-grid-viewport]")!
        .dispatchEvent(new Event("scroll"));
      const source = Array.from(
        document.querySelectorAll<HTMLButtonElement>(".source-card"),
      ).find((button) => button.textContent?.startsWith("Boundary Source"));
      if (!source) throw new Error("Boundary source is missing");
      source.click();
      document
        .querySelector<HTMLElement>("[data-grid-viewport]")!
        .dispatchEvent(new Event("scroll"));
    });
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Boundary Source",
    );
    await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();
    await page.evaluate(
      () =>
        new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
    );
    expect(staleWindowRequests).toBe(0);
  } finally {
    await page.unroute("**/api/browse/**");
  }
});

test("source switching cancels the previous pending window", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 120);
  const running = await server(base, root);
  await createSet(running.url, "Abort Target");

  const openedResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/browse",
  );
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  const openedBody = (await (await openedResponse).json()) as {
    token: string;
  };
  const oldBrowseToken = openedBody.token;
  await expect(page.getByText(/^Ready · 120 Photos$/)).toBeVisible();

  let release!: () => void;
  const windowHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let oldWindowUrl: string | undefined;
  await page.route("**/api/browse/**", (route) => {
    const request = route.request();
    if (
      request.method() === "GET" &&
      request.url().includes(`/api/browse/${oldBrowseToken}?`)
    ) {
      oldWindowUrl = request.url();
      return windowHeld.then(() => route.continue()).catch(() => undefined);
    }
    return route.continue();
  });
  try {
    const viewport = page.locator("[data-grid-viewport]");
    await viewport.evaluate((element) => {
      element.scrollTop = 30 * 178;
      element.dispatchEvent(new Event("scroll"));
    });
    await expect.poll(() => oldWindowUrl).toBeTruthy();
    const oldWindowCanceled = page.waitForEvent(
      "requestfailed",
      (request) => request.url() === oldWindowUrl,
    );

    await page
      .getByRole("button", { name: /^Abort Target 120 Photos/ })
      .click();
    await expect(page.locator("[data-grid-title]")).toHaveText("Abort Target");
    await expect(page.getByText(/^Ready · 120 Photos$/)).toBeVisible();
    await oldWindowCanceled;
  } finally {
    release();
    await page.unroute("**/api/browse/**");
  }
});

test("source switching aborts fallback thumbnail requests", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 70);
  const running = await server(base, root);
  await createSet(running.url, "Fallback Abort");

  let release!: () => void;
  const thumbnailGate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let thumbnailRequests = 0;
  await page.route("**/api/photos/*/thumbnail", (route) => {
    thumbnailRequests += 1;
    return thumbnailGate.then(() => route.continue()).catch(() => undefined);
  });
  try {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(running.url);
    await expect(page.getByText(/^Ready · 70 Photos$/)).toBeVisible();
    await expect.poll(() => thumbnailRequests).toBeGreaterThan(0);
    const thumbnailCanceled = page.waitForEvent("requestfailed", (request) =>
      new URL(request.url()).pathname.endsWith("/thumbnail"),
    );

    await page
      .getByRole("button", { name: /^Fallback Abort 70 Photos/ })
      .click();
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Fallback Abort",
    );
    await expect(page.getByText(/^Ready · 70 Photos$/)).toBeVisible();
    await thumbnailCanceled;
  } finally {
    release();
    await page.unroute("**/api/photos/*/thumbnail");
  }
});

test("scroll events coalesce Grid rendering to one animation frame", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 8);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();
  await expect(page.locator(".photo-cell img")).toHaveCount(8);

  const renderCount = await page.evaluate(
    () =>
      new Promise<number>((resolve) => {
        const layer = document.querySelector<HTMLElement>("[data-grid-layer]");
        const viewport = document.querySelector<HTMLElement>(
          "[data-grid-viewport]",
        );
        if (!layer || !viewport) throw new Error("Grid elements are missing");
        const replaceChildren = layer.replaceChildren.bind(layer);
        let count = 0;
        layer.replaceChildren = (...nodes: Node[]) => {
          count += 1;
          replaceChildren(...nodes);
        };
        for (let index = 0; index < 8; index += 1)
          viewport.dispatchEvent(new Event("scroll"));
        requestAnimationFrame(() => resolve(count));
      }),
  );
  expect(renderCount).toBe(1);
});

test("Back to Grid restoration supersedes a queued scroll render", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 8);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText(/^Ready · 8 Photos$/)).toBeVisible();

  const currentCell = page.locator('[data-photo-index="6"]');
  await currentCell.scrollIntoViewIfNeeded();
  await currentCell.click();
  await expect(page.getByText("7 / 8")).toBeVisible();

  const restoredScrollTop = await page.evaluate(
    () =>
      new Promise<number>((resolve) => {
        const viewport = document.querySelector<HTMLElement>(
          "[data-grid-viewport]",
        );
        const back = document.querySelector<HTMLButtonElement>(
          "[data-review] [data-back]",
        );
        if (!viewport || !back) throw new Error("Grid controls are missing");
        viewport.scrollTop = 0;
        viewport.dispatchEvent(new Event("scroll"));
        back.click();
        requestAnimationFrame(() =>
          requestAnimationFrame(() => resolve(viewport.scrollTop)),
        );
      }),
  );

  expect(restoredScrollTop).toBeGreaterThan(0);
  await expect(currentCell).toBeVisible();
});

test("hydrated Grid thumbnail delivery failures stay attached to the Photo", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "photo.jpg"), await jpeg());
  const running = await server(base, root);
  const [photoId] = await browseIds(running.url);
  const response = await fetch(
    `${running.url}/api/photos/${photoId}/thumbnail`,
  );
  expect(response.ok).toBe(true);

  await page.route("**/*", (route) => {
    const pathname = new URL(route.request().url()).pathname;
    if (
      pathname.includes("/api/derivatives/") &&
      pathname.includes("/thumbnail/")
    )
      return route.fulfill({ status: 404, body: "missing derivative" });
    return route.continue();
  });
  const thumbnailRequests: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.endsWith("/thumbnail"))
      thumbnailRequests.push(request.url());
  });
  await page.goto(running.url);
  const image = page.locator(".photo-cell img").first();
  await image.scrollIntoViewIfNeeded();
  await expect(image).toHaveAttribute(
    "alt",
    "Photo 1 of 1 — Thumbnail unavailable",
  );
  expect(thumbnailRequests).toHaveLength(0);
});

test("detached Grid image errors cannot poison the replacement cell", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "photo.jpg"), await jpeg());
  const running = await server(base, root);
  const [photoId] = await browseIds(running.url);
  const response = await fetch(
    `${running.url}/api/photos/${photoId}/thumbnail`,
  );
  expect(response.ok).toBe(true);

  await page.goto(running.url);
  const currentImage = page.locator(".photo-cell img").first();
  await expect(currentImage).toHaveAttribute("src", /\/thumbnail\//);
  const detachedImage = await currentImage.elementHandle();
  expect(detachedImage).not.toBeNull();

  await page.locator("[data-grid-viewport]").evaluate((viewport) => {
    viewport.dispatchEvent(new Event("scroll"));
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  expect(await detachedImage!.evaluate((image) => image.isConnected)).toBe(
    false,
  );
  await detachedImage!.evaluate((image) =>
    image.dispatchEvent(new Event("error")),
  );

  await expect(currentImage).toHaveAttribute("alt", "Photo 1 of 1");
  await expect(currentImage).toHaveAttribute("src", /\/thumbnail\//);
});

test("a completed mutation cannot reopen or advance a superseding source", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  const { setId } = await createSet(running.url, "Mutation Target");
  await page.goto(running.url);
  await page.locator('[data-photo-index="0"]').click();
  await expect(page.getByText("1 / 2")).toBeVisible();

  let release!: () => void;
  const stateHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let held = false;
  await page.route("**/api/photos/*/state", async (route) => {
    held = true;
    await stateHeld;
    try {
      await route.continue();
    } catch {
      /* the page may close only during failed test cleanup */
    }
  });
  try {
    await page.getByRole("button", { name: "Select" }).click();
    await expect.poll(() => held).toBe(true);
    const mutationCompleted = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname.endsWith("/state") &&
        response.status() === 200,
    );

    await page
      .getByRole("button", { name: /^Mutation Target 2 Photos/ })
      .click();
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Mutation Target",
    );
    await expect(page.getByText(/^Ready · 2 Photos$/)).toBeVisible();
    release();
    await mutationCompleted;

    await expect(page.locator("[data-review]")).toBeHidden();
    await expect(page.locator("[data-grid-title]")).toHaveText(
      "Mutation Target",
    );
    await expect
      .poll(
        async () =>
          (await state(running.url, setId)).members[0]!.selectionState,
      )
      .toBe("selected");
  } finally {
    release();
    await page.unroute("**/api/photos/*/state");
  }
});

test("an Undo Preview continuation cannot label or persist a newer Photo", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 2);
  const running = await server(base, root);
  const [firstPhotoId] = await browseIds(running.url);
  await page.goto(running.url);
  await page.locator('[data-photo-index="0"]').click();
  await expect(page.getByText("1 / 2")).toBeVisible();
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();

  let release!: () => void;
  const previewHeld = new Promise<void>((resolve) => {
    release = resolve;
  });
  let held = false;
  await page.route(`**/api/photos/${firstPhotoId}/preview`, async (route) => {
    held = true;
    await previewHeld;
    try {
      await route.continue();
    } catch {
      /* navigating to the newer Photo aborts the Undo Preview */
    }
  });
  try {
    await page.getByRole("button", { name: /^Undo/ }).click();
    await expect.poll(() => held).toBe(true);
    await page.getByRole("button", { name: "Next" }).click();
    await expect(page.getByText("2 / 2")).toBeVisible();
    release();
    await expect(page.getByText("Last change undone.")).toHaveCount(0);
  } finally {
    release();
    await page.unroute(`**/api/photos/${firstPhotoId}/preview`);
  }
});

test("opening a Photo from the Grid persists the Album position", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg", "d.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url, "GridPos");
  await openGrid(page, running.url, "GridPos");
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /^Photo 3 of 4/ }),
  );
  await expect(page.getByText("3 / 4")).toBeVisible();
  await expect
    .poll(async () => (await state(running.url, setId)).position)
    .toBe(2);

  await page.reload();
  let browsePosition: number | undefined;
  page.on("response", (response) => {
    if (
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/browse"
    )
      void response
        .json()
        .then(
          (body: { position?: number }) =>
            (browsePosition = body.position ?? browsePosition),
        )
        .catch(() => undefined);
  });
  await page.getByRole("button", { name: /^GridPos(?: |$)/ }).click();
  await page.getByText(/^Ready · /).waitFor();
  await expect(
    page.getByRole("button", { name: /^Photo 3 of 4/ }),
  ).toBeVisible();
  await expect.poll(() => browsePosition).toBe(2);
});

test("Undo returns to the affected Photo and refreshes its Preview and facts", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url);
  await openGrid(page, running.url, "Review");
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /^Photo 1 of/ }),
  );
  await expect(page.getByText("1 / 3")).toBeVisible();
  await expect(page.locator("[data-stage] img")).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Undo" }).click(),
  );
  await expect(page.getByText("1 / 3")).toBeVisible();
  await expect(page.getByText("Undecided", { exact: true })).toBeVisible();
  await expect(page.getByText("Last change undone.")).toBeVisible();
  await expect(page.locator("[data-stage] img")).toBeVisible();
});

test("Undo clears when the affected Photo leaves the loaded window", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (let index = 0; index < 200; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(3, "0")}.jpg`),
      await jpeg(),
    );
  const running = await server(base, root);
  const { setId } = await createSet(running.url, "Wide");
  await openGrid(page, running.url, "Wide");
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /^Photo 1 of 200/ }),
  );
  await expect(page.getByText("1 / 200")).toBeVisible();
  await actionWithProgress(page, setId, () =>
    page.getByRole("button", { name: "Select" }).click(),
  );
  await expect(page.getByText("2 / 200")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);
  const viewport = page.locator("[data-grid-viewport]");
  await viewport.evaluate((element) => {
    element.scrollTop = 30 * 178;
  });
  await expect(page.locator('[data-photo-index="65"]')).toBeVisible();
  await viewport.evaluate((element) => {
    element.scrollTop = 60 * 178;
  });
  await expect(page.locator('[data-photo-index="125"]')).toBeVisible();
  await viewport.evaluate((element) => {
    element.scrollTop = 90 * 178;
  });
  await expect(page.locator('[data-photo-index="185"]')).toBeVisible();
  await page.keyboard.press("Control+z");
  await expect
    .poll(
      async () => (await state(running.url, setId)).members[0]!.selectionState,
    )
    .toBe("selected");
  expect((await state(running.url, setId)).members[0]!.selectionState).toBe(
    "selected",
  );
});

test("failed Browse recovery leaves the boundary range retryable", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 130);
  const running = await server(base, root);
  await page.setViewportSize({ width: 390, height: 844 });
  await openGrid(page, running.url, "All Photos");

  let expired = false;
  let boundaryRequests = 0;
  let reopenAttempts = 0;
  let releaseRetry!: () => void;
  const retryGate = new Promise<void>((resolve) => {
    releaseRetry = resolve;
  });
  await page.route(/\/api\/browse/, async (route) => {
    const request = route.request();
    if (
      request.method() === "GET" &&
      new URL(request.url()).searchParams.get("start") === "60"
    ) {
      boundaryRequests += 1;
      if (boundaryRequests === 1) {
        expired = true;
        await route.fulfill({
          status: 404,
          contentType: "application/json",
          body: '{"error":"Browse source expired or not found"}',
        });
        return;
      }
      await retryGate;
      try {
        await route.continue();
      } catch {
        /* the browser may cancel duplicate viewport requests */
      }
      return;
    }
    if (request.method() === "POST" && expired) {
      reopenAttempts += 1;
      if (reopenAttempts === 1) {
        await route.fulfill({ status: 503, body: '{"error":"reopen failed"}' });
        return;
      }
    }
    await route.continue();
  });
  try {
    const viewport = page.locator("[data-grid-viewport]");
    await viewport.evaluate((element) => {
      element.scrollTop = 30 * 178;
      element.dispatchEvent(new Event("scroll"));
    });
    await expect.poll(() => reopenAttempts).toBe(1);
    await expect(page.locator("[data-status]")).toHaveText(
      "This source expired and could not be reopened. Retry the connection.",
    );

    const retriedWindow = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return (
        response.request().method() === "GET" &&
        url.pathname.includes("/api/browse/") &&
        url.searchParams.get("start") === "60" &&
        response.status() === 200
      );
    });
    await viewport.evaluate((element) => {
      element.dispatchEvent(new Event("scroll"));
    });
    await expect.poll(() => boundaryRequests).toBeGreaterThan(1);
    releaseRetry();
    await retriedWindow;
  } finally {
    releaseRetry();
    await page.unroute(/\/api\/browse/);
  }
});

test("an expired Browse snapshot reopens around the current Photo", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (let index = 0; index < 130; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(3, "0")}.jpg`),
      await jpeg(),
    );
  const running = await server(base, root);
  const { setId } = await createSet(running.url, "Expiry");
  await openGrid(page, running.url, "Expiry");
  await openPhotoAndWaitForProgress(
    page,
    setId,
    page.getByRole("button", { name: /^Photo 1 of 130/ }),
  );
  await expect(page.getByText("1 / 130")).toBeVisible();
  const firstId = (await state(running.url, setId)).members[0]!.photoId;
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);
  const reopenBodies: Array<Record<string, unknown>> = [];
  let expiredServed = false;
  await page.route(/\/api\/browse/, async (route) => {
    const request = route.request();
    if (request.method() === "POST") {
      reopenBodies.push(request.postDataJSON() as Record<string, unknown>);
      await route.continue();
      return;
    }
    if (request.method() === "GET" && !expiredServed) {
      expiredServed = true;
      await route.fulfill({
        status: 404,
        contentType: "application/json",
        body: '{"error":"Browse source expired or not found"}',
      });
      return;
    }
    await route.continue();
  });
  await page.locator("[data-grid-viewport]").evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await expect.poll(() => expiredServed).toBe(true);
  await expect
    .poll(() =>
      reopenBodies.some(
        (body) =>
          body.source === "album" &&
          body.albumId === setId &&
          body.photoId === firstId,
      ),
    )
    .toBe(true);
  await expect(
    page.getByRole("button", { name: /^Photo 1 of 130/ }),
  ).toBeVisible();
});

test("navigation promotes an aborted adjacent window to current priority", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writePhotos(root, 70);
  const running = await server(base, root);
  await page.setViewportSize({ width: 1200, height: 1100 });
  await openGrid(page, running.url, "All Photos");
  await page.locator("[data-grid-viewport]").evaluate((viewport) => {
    Object.defineProperty(viewport, "clientWidth", {
      configurable: true,
      value: 900,
    });
    Object.defineProperty(viewport, "clientHeight", {
      configurable: true,
      value: 900,
    });
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await page.locator("[data-grid-viewport]").evaluate((viewport) => {
    viewport.scrollTop = 3 * 178;
    viewport.dispatchEvent(new Event("scroll"));
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await expect(page.locator('[data-photo-index="59"]')).toHaveCount(1);
  await expect(page.locator('[data-photo-index="60"]')).toHaveCount(0);

  let releaseFirst!: () => void;
  const firstGate = new Promise<void>((resolve) => {
    releaseFirst = resolve;
  });
  let boundaryRequests = 0;
  await page.route(
    (url) =>
      url.pathname.startsWith("/api/browse/") &&
      url.searchParams.get("start") === "10",
    async (route) => {
      boundaryRequests += 1;
      if (boundaryRequests === 1) {
        await firstGate;
        try {
          await route.continue();
        } catch {
          /* current navigation aborts the adjacent request */
        }
        return;
      }
      await route.continue();
    },
  );
  try {
    await page.locator('[data-photo-index="59"]').evaluate((cell) => {
      (cell as HTMLButtonElement).click();
    });
    await expect(page.getByText("60 / 70")).toBeVisible();
    await expect.poll(() => boundaryRequests).toBe(1);

    await page.getByRole("button", { name: "Next" }).click();
    await expect.poll(() => boundaryRequests).toBe(2);
    await expect(page.getByText("61 / 70")).toBeVisible();
  } finally {
    releaseFirst();
  }
});

test("a throttled boundary window cannot wedge Photo View or Back to Grid", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (let index = 0; index < 70; index += 1)
    await writeFile(
      join(root, `${String(index).padStart(3, "0")}.jpg`),
      await jpeg(),
    );
  const running = await server(base, root);
  await openGrid(page, running.url, "All Photos");
  let releaseBoundary: () => void = () => undefined;
  const boundaryGate = new Promise<void>((resolve) => {
    releaseBoundary = resolve;
  });
  let boundaryRequests = 0;
  await page.route(
    (url) =>
      url.pathname.startsWith("/api/browse/") &&
      url.searchParams.get("start") === "10",
    async (route) => {
      boundaryRequests += 1;
      await boundaryGate;
      try {
        await route.continue();
      } catch {
        /* Back to Grid aborts the Photo-owned boundary request */
      }
    },
  );
  const viewport = page.locator("[data-grid-viewport]");
  await viewport.evaluate((element) => {
    element.scrollTop = 29 * 178;
  });
  await page.locator('[data-photo-index="59"]').click();
  await expect(page.getByText("60 / 70")).toBeVisible();
  await page.keyboard.press("ArrowRight");
  // The boundary Photo waits for its shared facts; Back to Grid must remain
  // available instead of claiming the unavailable Photo is already open.
  await expect(page.getByText("60 / 70")).toBeVisible();
  // The boundary window is still loading; Back to Grid must stay available.
  await expect(
    page.getByRole("button", { name: "Back to Grid" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.locator("[data-grid-layer]")).toBeVisible();
  releaseBoundary();
  await expect(page.locator('[data-photo-index="60"]')).toBeEnabled();
  // The abandoned open must not wedge the browser: a new Photo opens normally.
  await page.locator('[data-photo-index="57"]').click();
  await expect(page.getByText("58 / 70")).toBeVisible();
  await expect(page.getByRole("button", { name: /^Select/ })).toBeEnabled();
  expect(boundaryRequests).toBeGreaterThanOrEqual(1);
});

test("file locations stay bounded on a 40,000-Photo Library with a large folder hierarchy", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const base = await mkdtemp(join(tmpdir(), "slipstream-browser-40k-folders-"));
  temporary.push(base);
  const root = join(base, "originals");
  await mkdir(root);
  await mkdir(join(base, "state"));
  await mkdir(join(base, "cache"));
  await chmod(join(base, "state"), 0o700);
  // Canonical v5 state with 40,000 direct root Folders x 1 Photo each.
  const generator = `
    const { Database } = await import("bun:sqlite");
    const database = new Database(process.env.STATE_DB);
    database.exec(await Bun.file(process.env.SCHEMA_PATH).text());
    const insertOriginal = database.prepare(
      "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available) VALUES(?1,?2,'jpeg',1,1.0,1)",
    );
    const insertPhoto = database.prepare(
      "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,sort_path) VALUES(?1,?2,0,1,'inspection-pending',?3)",
    );
    const insertBinding = database.prepare(
      "INSERT INTO library_metadata VALUES('canonical_root',?1)",
    );
    database.exec("BEGIN");
    for (let index = 0; index < 40000; index += 1) {
      const path = "f" + String(index).padStart(5, "0") + "/one.jpg";
      const originalId = index.toString(16).padStart(8, "0").repeat(8);
      const photoId = (0x100000 + index).toString(16).padStart(8, "0").repeat(8);
      insertOriginal.run(originalId, path);
      insertPhoto.run(photoId, originalId, path);
    }
    database.exec("COMMIT");
    insertBinding.run(process.env.ROOT);
    database.close();
  `;
  execFileSync("bun", ["-e", generator], {
    env: {
      ...process.env,
      STATE_DB: join(base, "state", "library.sqlite"),
      ROOT: root,
      SCHEMA_PATH: join(process.cwd(), "compatibility/sqlite/schema-v5.sql"),
    },
    stdio: "inherit",
  });

  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();

  // Expanding the root loads exactly one enforced direct-child page out of
  // 40,000 with explicit pager controls.
  await page
    .getByRole("button", {
      name: "Toggle Library Folder subfolders",
    })
    .click();
  const folderCards = page.locator(".source-panel .source-card");
  await expect(folderCards).toHaveCount(62, { timeout: 10_000 });
  await expect(page.getByText("1 / 667")).toBeVisible();
  // Paging to a late position replaces the retained page: the DOM stays at
  // one window no matter how deep the navigation reaches.
  for (let page_index = 2; page_index <= 6; page_index += 1) {
    await page.getByRole("button", { name: "More Folders" }).click();
    await expect(page.getByText(`${page_index} / 667`)).toBeVisible();
  }
  expect(await page.locator(".folder-row .source-card").count()).toBe(61);
  await page.getByRole("button", { name: "Previous Folders" }).click();
  await expect(page.getByText("5 / 667")).toBeVisible();
  expect(await page.locator(".folder-row .source-card").count()).toBe(61);

  // Opening a Folder from the current page stays bounded end to end.
  await page.locator(".folder-child .source-card").first().click();
  await expect(page.getByText("Ready · 1 Photos")).toBeVisible();

  const metrics = await page.evaluate(() => ({
    domCount: document.querySelectorAll("*").length,
    cellCount: document.querySelectorAll(".photo-cell").length,
    folderButtons: document.querySelectorAll(".folder-row").length,
  }));
  expect(metrics.cellCount).toBeLessThan(120);
  expect(metrics.folderButtons).toBe(61);
  expect(metrics.domCount).toBeLessThan(3_000);
});

test("a persisted 40,000-Photo Library is served from persisted state and stays browsable across the startup rescan", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const base = await mkdtemp(join(tmpdir(), "slipstream-browser-40k-"));
  temporary.push(base);
  const root = join(base, "originals");
  await mkdir(root);
  await mkdir(join(base, "state"));
  await mkdir(join(base, "cache"));
  await chmod(join(base, "state"), 0o700);
  // The test process runs under Playwright's Node loader, so the canonical
  // v4 state is generated through Bun's SQLite in a child process.
  const generator = `
    const { Database } = await import("bun:sqlite");
    const database = new Database(process.env.STATE_DB);
    database.exec(await Bun.file(process.env.SCHEMA_PATH).text());
    const insertOriginal = database.prepare(
      "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available) VALUES(?1,?2,'jpeg',1,1.0,1)",
    );
    const insertPhoto = database.prepare(
      "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,sort_path) VALUES(?1,?2,0,1,'inspection-pending',?3)",
    );
    const insertBinding = database.prepare(
      "INSERT INTO library_metadata VALUES('canonical_root',?1)",
    );
    database.exec("BEGIN");
    for (let index = 0; index < 40000; index += 1) {
      const path = String(index).padStart(6, "0") + ".jpg";
      const originalId = index.toString(16).padStart(8, "0").repeat(8);
      const photoId = (0x100000 + index).toString(16).padStart(8, "0").repeat(8);
      insertOriginal.run(originalId, path);
      insertPhoto.run(photoId, originalId, path);
    }
    database.exec("COMMIT");
    insertBinding.run(process.env.ROOT);
    database.close();
  `;
  execFileSync("bun", ["-e", generator], {
    env: {
      ...process.env,
      STATE_DB: join(base, "state", "library.sqlite"),
      ROOT: root,
      SCHEMA_PATH: join(process.cwd(), "compatibility/sqlite/schema-v4.sql"),
    },
    stdio: "inherit",
  });

  const running = await server(base, root);
  const gridMetrics = () =>
    page.evaluate(() => {
      const viewport = document.querySelector<HTMLElement>(
        "[data-grid-viewport]",
      );
      if (!viewport) throw new Error("grid viewport is missing");
      return {
        viewportHeight: viewport.clientHeight,
        scrollHeight: viewport.scrollHeight,
        innerHeight: window.innerHeight,
        cellCount: document.querySelectorAll(".photo-cell").length,
        domCount: document.querySelectorAll("*").length,
      };
    });
  const expectBoundedGrid = async () => {
    const metrics = await gridMetrics();
    expect(metrics.viewportHeight).toBeGreaterThan(0);
    expect(metrics.viewportHeight).toBeLessThanOrEqual(metrics.innerHeight);
    expect(metrics.scrollHeight).toBeGreaterThan(metrics.viewportHeight);
    expect(metrics.cellCount).toBeGreaterThan(0);
    expect(metrics.cellCount).toBeLessThan(200);
    expect(metrics.domCount).toBeLessThan(2_000);
    return metrics;
  };

  // The published Library is served immediately from persisted state; the
  // overview stays bounded instead of transferring 40,000 Photo facts.
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto(running.url);
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "All Photos 40000 Photos" }),
  ).toBeVisible();
  const overviewBytes = await page.evaluate(
    async () => (await (await fetch("/api/overview")).text()).length,
  );
  expect(overviewBytes).toBeLessThan(20_000);
  const initialGrid = await expectBoundedGrid();

  await page.setViewportSize({ width: 1280, height: 1200 });
  await expect
    .poll(async () => (await gridMetrics()).cellCount)
    .toBeGreaterThan(initialGrid.cellCount);
  const tallGrid = await expectBoundedGrid();

  await page.setViewportSize({ width: 1280, height: 720 });
  await expect
    .poll(async () => (await gridMetrics()).cellCount)
    .toBeLessThan(tallGrid.cellCount);
  await expectBoundedGrid();

  const firstCell = page.locator('[data-photo-index="0"]');
  await expect(firstCell).toBeVisible();
  await expect(firstCell).toBeEnabled();
  await firstCell.click();
  await expect(page.getByText("1 / 40000")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();

  const viewport = page.locator("[data-grid-viewport]");
  await viewport.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
    element.dispatchEvent(new Event("scroll"));
  });
  const lastCell = page.locator('[data-photo-index="39999"]');
  await expect(lastCell).toBeVisible({ timeout: 30_000 });
  await expect(lastCell).toBeEnabled();
  await expectBoundedGrid();
  await lastCell.click();
  await expect(page.getByText("40000 / 40000")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await waitForGridFrame(page);
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();

  // Reducing the column count must restore the current late Photo after the
  // virtual canvas grows for the narrower layout.
  await page.setViewportSize({ width: 1100, height: 720 });
  await expect(lastCell).toBeVisible();
  await expectBoundedGrid();

  // Re-check the original mobile viewport contract after exercising the
  // desktop late-window path.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();
  await expectBoundedGrid();

  // The owned startup rescan settles without emptying or reordering the
  // source; late-window protocol browsing stays bounded afterward.
  await expect
    .poll(
      async () => {
        const response = await fetch(`${running.url}/api/status`);
        return ((await response.json()) as { state: string }).state;
      },
      { timeout: 120_000 },
    )
    .toBe("idle");
  const lateWindow: { total: number; photos: unknown[] } = await page.evaluate(
    async () => {
      const opened = (await (
        await fetch("/api/browse", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ source: "library" }),
        })
      ).json()) as { token: string };
      const window = (await (
        await fetch(`/api/browse/${opened.token}?start=39940&limit=60`)
      ).json()) as { total: number; photos: unknown[] };
      return window;
    },
  );
  expect(lateWindow.total).toBe(40_000);
  expect(lateWindow.photos).toHaveLength(60);
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();
});
