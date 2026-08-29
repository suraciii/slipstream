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

import { expect, test, type Page } from "@playwright/test";

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
  const created = (await (
    await post(url, "/api/photo-sets", { name })
  ).json()) as {
    photoSets: Array<{ id: string; name: string }>;
  };
  const set = created.photoSets.find((item) => item.name === name)!;
  for (let offset = 0; offset < photos.length; offset += 100)
    await post(url, `/api/photo-sets/${set.id}/members`, {
      photoIds: photos.slice(offset, offset + 100),
    });
  return { setId: set.id };
}
async function startReview(page: Page, url: string, name = "Review") {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(url);
  const escapedName = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  await page
    .getByRole("button", { name: new RegExp(`^${escapedName}(?: |$)`) })
    .click();
  await page.getByRole("button", { name: /Photo 1 of/ }).click();
  await expect(page.locator("[data-review]")).toBeVisible();
  await page.waitForFunction(
    () =>
      Boolean(document.querySelector("[data-stage] img")) ||
      document.body.innerText.includes("Preview unavailable"),
  );
}
// Membership order and per-member facts are observable only through a
// fresh Photo Set Browse Snapshot. The resolved open position exposes the
// saved Photo Set position under the unavailable-member fallback rules.
async function state(url: string, setId: string): Promise<SetState> {
  const opened = (await (
    await post(url, "/api/browse", { source: "photo-set", photoSetId: setId })
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

test("starts from a Photo Set, shows facts, accessible controls, and resumes persisted progress after restart", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg());
  await writeFile(join(root, "b.jpg"), await jpeg());
  let running = await server(base, root);
  const { setId } = await createSet(running.url, "Picks");
  await startReview(page, running.url, "Picks");
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
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  // The advanced Photo is the saved Photo Set position.
  await expect
    .poll(async () => (await state(running.url, setId)).position)
    .toBe(1);

  await page.goto("about:blank");
  await running.close();
  servers.splice(servers.indexOf(running), 1);
  running = await server(base, root);
  await page.goto(running.url);
  await page.getByRole("button", { name: /Picks/ }).click();
  await page.getByRole("button", { name: /Photo 2 of 2/ }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await page.getByRole("button", { name: "Previous" }).click();
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
  await startReview(page, running.url);

  await page.keyboard.press("p");
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
  await page.getByRole("button", { name: "Reject" }).click();
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await page.keyboard.press("Control+z");
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
  await page.keyboard.press("ArrowRight");
  await expect(page.getByText("3 / 3")).toBeVisible();
  await page.keyboard.press("ArrowLeft");
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
  await startReview(page, running.url);

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
  await swipe(page, 100, 190);
  await expect(page.getByText("1 / 3")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Back to Grid" }),
  ).toBeDisabled();
  expect((await state(running.url, setId)).members[0]!.selectionState).toBe(
    "undecided",
  );
  releaseMutation();
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.unroute("**/api/photos/*/state");
  expect((await state(running.url, setId)).members[0]!.selectionState).toBe(
    "selected",
  );
  await swipe(page, 250, 150);
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
  await startReview(page, running.url);

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
  await page.getByRole("button", { name: "Retry" }).click();
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
  await startReview(page, running.url);

  await page.getByRole("button", { name: "Select" }).click();
  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();
  await page.getByRole("button", { name: "Select" }).click();
  const firstId = (await state(running.url, setId)).members[0]!.photoId;
  await post(running.url, `/api/photos/${firstId}/state`, {
    field: "selectionState",
    value: "rejected",
  });
  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByText(/no longer available/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Previous" }).click();
  await page.getByRole("button", { name: "Detail Review" }).click();
  await expect(
    page.getByRole("button", { name: "Exit Detail" }),
  ).toHaveAttribute("aria-pressed", "true");
  await swipe(page, 100, 220);
  await expect(page.getByText("1 / 2")).toBeVisible();
  expect((await state(running.url, setId)).members[0]!.selectionState).toBe(
    "rejected",
  );
  await page.getByRole("button", { name: "Next" }).click();
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
  await post(running.url, `/api/photo-sets/${setId}/progress`, {
    photoId: initial.members[0]!.photoId,
  });
  await rm(missing);
  await post(running.url, "/api/scan", {});
  await startReview(page, running.url);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: /Review/ }).click();
  await page.getByRole("button", { name: /Photo 2 of 2/ }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await page.getByRole("button", { name: "Previous" }).click();
  await expect(page.getByText(/Original File is unavailable/)).toBeVisible();
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  expect((await state(running.url, setId)).members[0]).toMatchObject({
    available: false,
    selectionState: "selected",
  });
});

test("shows empty/no-set start states and only uses GET plus same-service POST mutations", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "a.jpg"), await jpeg());
  const running = await server(base, root);
  const methods: string[] = [];
  page.on("request", (request) => methods.push(request.method()));
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await post(running.url, "/api/photo-sets", { name: "Empty" });
  await page.reload();
  const empty = page.getByRole("button", { name: /Empty/ });
  await expect(empty).toBeVisible();
  await expect(empty).toBeDisabled();
  await createSet(running.url, "Ready");
  await page.reload();
  await expect(page.getByRole("button", { name: /Ready/ })).toBeEnabled();
  expect(methods.every((method) => method === "GET" || method === "POST")).toBe(
    true,
  );
});

test("persists manual navigation and advanced current Photo across leave, reload, and restart", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  let running = await server(base, root);
  const { setId } = await createSet(running.url, "Progress");
  await startReview(page, running.url, "Progress");
  await page.getByRole("button", { name: "Next" }).click();
  await expect
    .poll(async () => (await state(running.url, setId)).position)
    .toBe(1);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: /Progress/ }).click();
  await page.getByRole("button", { name: /Photo 2 of 3/ }).click();
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.reload();
  await page.getByRole("button", { name: /Progress/ }).click();
  await page.getByRole("button", { name: /Photo 2 of 3/ }).click();
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.getByRole("button", { name: "Select" }).click();
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
  await page.getByRole("button", { name: /Photo 3 of 3/ }).click();
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
  await startReview(page, running.url);
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
  await page.keyboard.press("ArrowRight");
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
  await event("pointerdown", 100, 0, 43);
  await event("pointermove", 172, 1000, 43);
  await event("pointerup", 172, 1000, 43);
  await expect(page.getByText("3 / 3")).toBeVisible();

  await page.getByRole("button", { name: "Previous" }).click();
  await event("pointerdown", 100, 0, 44);
  await event("pointermove", 148, 50, 44);
  await event("pointerup", 148, 50, 44);
  await expect(page.getByText("3 / 3")).toBeVisible();
  await page.getByRole("button", { name: "Previous" }).click();
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
  await page.getByRole("button", { name: /Photo 1 of/ }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
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
  await startReview(page, running.url);

  await page.getByRole("button", { name: "Next" }).click();
  await page.keyboard.press("p");
  await expect(page.getByText("3 / 3")).toBeVisible();
  await page.getByRole("button", { name: "Previous" }).click();
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
  await page.keyboard.press("x");
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await page.keyboard.press("Control+z");
  await expect(page.getByText("2 / 3")).toBeVisible();

  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await page.getByRole("button", { name: "Previous" }).click();
  await page.route("**/api/photos/*/state", async (route) => {
    await route.fetch();
    await route.abort();
  });
  await page.getByRole("button", { name: "Reject" }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
  await page.unroute("**/api/photos/*/state");
  await page.getByRole("button", { name: "Retry" }).click();
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
  await createSet(running.url);
  await startReview(page, running.url);
  await expect(page.getByText("JPEG", { exact: true })).toBeVisible();
  await rm(matching);
  await post(running.url, "/api/scan", {});
  await page.reload();
  await page.getByRole("button", { name: /^Review(?: |$)/ }).click();
  await page.getByRole("button", { name: /Photo 1 of/ }).click();
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
  expect(stateBodies[0]).not.toHaveProperty("photoSetId");
  const overview = (await (
    await fetch(`${running.url}/api/overview`)
  ).json()) as {
    photoSets: unknown[];
  };
  expect(overview.photoSets).toEqual([]);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: /All Photos/ }).click();
  await page.getByRole("button", { name: /Photo 1 of 2/ }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();

  const { setId } = await createSet(running.url, "Explicit order");
  await post(running.url, `/api/photo-sets/${setId}/order`, {
    photoIds: [aId, zId],
  });
  await page.reload();
  await page.getByRole("button", { name: /^Explicit order(?: |$)/ }).click();
  await page.getByRole("button", { name: /Photo 1 of 2/ }).click();
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

test("Photo Set Review snapshots explicit members across rescan and reconnect", async ({
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
  await post(running.url, `/api/photo-sets/${setId}/order`, {
    photoIds: [aId, zId],
  });
  await startReview(page, running.url, "Snapshot");
  await expect(page.getByText("1 / 2")).toBeVisible();

  await writeFile(
    join(root, "B.jpg"),
    withCaptureTime(source, "2026:01:01 08:00:00"),
  );
  await post(running.url, "/api/scan", {});
  const rescannedIds = await browseIds(running.url);
  const bId = rescannedIds.find((id) => !initialIds.includes(id));
  expect(bId).toBeDefined();
  await post(running.url, `/api/photo-sets/${setId}/members`, {
    photoIds: [bId!],
  });
  await page.route("**/api/photos/*/preview", (route) => route.abort());
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await page.unroute("**/api/photos/*/preview");
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.getByRole("button", { name: /^Snapshot(?: |$)/ }).click();
  await page.getByRole("button", { name: /Photo 2 of 3/ }).click();
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
  await startReview(page, running.url, "Recovery");
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await page.route("**/api/photos/*/preview", (route) => route.abort());
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await page.unroute("**/api/photos/*/preview");
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByText("1 / 3")).toBeVisible();

  let releaseFailure!: () => void;
  const failureReleased = new Promise<void>((resolve) => {
    releaseFailure = resolve;
  });
  let failed = false;
  await page.route("**/api/photo-sets/*/progress", async (route) => {
    if (!failed) {
      failed = true;
      await failureReleased;
      await route.fulfill({ status: 503, body: '{"error":"failed"}' });
      return;
    }
    await route.continue();
  });
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("3 / 3")).toBeVisible();
  releaseFailure();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();
  await page.unroute("**/api/photo-sets/*/progress");
  let releaseSuccess!: () => void;
  const successReleased = new Promise<void>((resolve) => {
    releaseSuccess = resolve;
  });
  await page.route("**/api/photo-sets/*/progress", async (route) => {
    await successReleased;
    await route.continue();
  });
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();
  releaseSuccess();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
  await page.unroute("**/api/photo-sets/*/progress");
  await expect
    .poll(async () => (await state(running.url, setId)).position)
    .toBe(2);
});

test("Photo Set resume wraps past an unavailable saved member and retains it when all are unavailable", async ({
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
  await post(running.url, `/api/photo-sets/${setId}/progress`, {
    photoId: savedId,
  });
  await rm(join(root, "c.jpg"));
  await post(running.url, "/api/scan", {});
  await page.goto(running.url);
  await page.getByRole("button", { name: /^Resume(?: |$)/ }).click();
  await page.getByRole("button", { name: /Photo 1 of 3/ }).click();
  await expect(page.getByText("1 / 3")).toBeVisible();

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
  await page.getByRole("button", { name: /Photo 3 of 3/ }).click();
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
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
  const viewport = page.locator("[data-grid-viewport]");
  for (let _ = 0; _ < 4; _ += 1) {
    await viewport.evaluate((element) =>
      element.dispatchEvent(new Event("scroll")),
    );
    await page.waitForTimeout(50);
  }
  expect(thumbnailRequests).toHaveLength(8);
  await expect.poll(loadedThumbnails).toBe(8);
});

test("opening a Photo from the Grid persists the Photo Set position", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg", "d.jpg"])
    await writeFile(join(root, name), await jpeg());
  const running = await server(base, root);
  const { setId } = await createSet(running.url, "GridPos");
  await openGrid(page, running.url, "GridPos");
  await page.getByRole("button", { name: /^Photo 3 of 4/ }).click();
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
  await createSet(running.url);
  await openGrid(page, running.url, "Review");
  await page.getByRole("button", { name: /^Photo 1 of/ }).click();
  await expect(page.getByText("1 / 3")).toBeVisible();
  await expect(page.locator("[data-stage] img")).toBeVisible();
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.getByRole("button", { name: "Undo" }).click();
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
  await page.getByRole("button", { name: /^Photo 1 of 200/ }).click();
  await expect(page.getByText("1 / 200")).toBeVisible();
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("2 / 200")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.waitForTimeout(300);
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
  await page.getByRole("button", { name: /^Photo 1 of 130/ }).click();
  await expect(page.getByText("1 / 130")).toBeVisible();
  const firstId = (await state(running.url, setId)).members[0]!.photoId;
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await page.waitForTimeout(300);
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
  await expect(
    page.getByRole("button", { name: /^Photo 1 of 130/ }),
  ).toBeVisible();
  expect(expiredServed).toBe(true);
  expect(
    reopenBodies.some(
      (body) =>
        body.source === "photo-set" &&
        body.photoSetId === setId &&
        body.photoId === firstId,
    ),
  ).toBe(true);
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
      await route.continue();
    },
  );
  const viewport = page.locator("[data-grid-viewport]");
  await viewport.evaluate((element) => {
    element.scrollTop = 29 * 178;
  });
  await page.locator('[data-photo-index="59"]').click();
  await expect(page.getByText("60 / 70")).toBeVisible();
  await page.keyboard.press("ArrowRight");
  await expect(page.getByText("61 / 70")).toBeVisible();
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
  // The published Library is served immediately from persisted state; the
  // overview stays bounded instead of transferring 40,000 Photo facts.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(running.url);
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "All Photos 40000 Photos" }),
  ).toBeVisible();
  const overviewBytes = await page.evaluate(
    async () => (await (await fetch("/api/overview")).text()).length,
  );
  expect(overviewBytes).toBeLessThan(20_000);

  await page.getByRole("button", { name: /Photo 1 of 40000/ }).click();
  await expect(page.getByText("1 / 40000")).toBeVisible();
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.getByText("Ready · 40,000 Photos")).toBeVisible();

  // The owned startup rescan settles without emptying or reordering the
  // source; late-window browsing stays bounded afterward.
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
