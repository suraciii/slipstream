import { createHash } from "node:crypto";
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";

import { expect, test, type Page } from "@playwright/test";

import { startBrowserServer, type BrowserServer } from "./browser-server.js";
import type { PhotoListResponse, PhotoSetResponse } from "./protocol.js";

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
function photoIdFor(relativePath: string): string {
  const originalId = createHash("sha256")
    .update("original\0")
    .update(relativePath)
    .digest("hex");
  return createHash("sha256")
    .update("photo\0")
    .update(originalId)
    .digest("hex");
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
  return running;
}
async function post(url: string, path: string, body: unknown) {
  return fetch(`${url}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}
async function createSet(url: string, name = "Review") {
  const photos = (await (
    await fetch(`${url}/api/photos`)
  ).json()) as PhotoListResponse;
  const created = (await (
    await post(url, "/api/photo-sets", { name })
  ).json()) as {
    photoSets: PhotoSetResponse[];
  };
  const set = created.photoSets.find((item) => item.name === name)!;
  await post(url, `/api/photo-sets/${set.id}/members`, {
    photoIds: photos.photos.map((photo) => photo.id),
  });
  return { setId: set.id, photos: photos.photos };
}
async function startReview(page: Page, url: string, name = "Review") {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(url);
  const escapedName = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  await page
    .getByRole("button", { name: new RegExp(`^${escapedName}(?: |$)`) })
    .click();
  await expect(page.locator("[data-review]")).toBeVisible();
}
async function state(url: string, setId: string) {
  const sets = (await (await fetch(`${url}/api/photo-sets`)).json()) as {
    photoSets: PhotoSetResponse[];
  };
  return sets.photoSets.find((item) => item.id === setId)!;
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
  expect((await state(running.url, setId)).lastReviewedPhotoId).toBeDefined();

  await page.goto("about:blank");
  await running.close();
  servers.splice(servers.indexOf(running), 1);
  running = await server(base, root);
  await page.goto(running.url);
  await page.getByRole("button", { name: /Picks/ }).click();
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
  await expect(page.getByRole("button", { name: "Photo Sets" })).toBeDisabled();
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
  await post(running.url, `/api/photo-sets/${setId}/progress`, {
    photoId: (await state(running.url, setId)).members[0]!.photoId,
  });
  await rm(missing);
  await post(running.url, "/api/scan", {});
  await startReview(page, running.url);
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
  await expect(page.getByText(/No Photo Sets yet/)).toBeVisible();
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
    .poll(async () => (await state(running.url, setId)).lastReviewedPhotoId)
    .toBe((await state(running.url, setId)).members[1]!.photoId);
  await page.getByRole("button", { name: "Photo Sets" }).click();
  await page.getByRole("button", { name: /Progress/ }).click();
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.reload();
  await page.getByRole("button", { name: /Progress/ }).click();
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.getByRole("button", { name: "Select" }).click();
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect
    .poll(async () => (await state(running.url, setId)).lastReviewedPhotoId)
    .toBe((await state(running.url, setId)).members[2]!.photoId);
  await page.goto("about:blank");
  await running.close();
  servers.splice(servers.indexOf(running), 1);
  running = await server(base, root);
  await page.goto(running.url);
  await page.getByRole("button", { name: /Progress/ }).click();
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
  const photos = (await (
    await fetch(`${running.url}/api/photos`)
  ).json()) as PhotoListResponse;
  const zId = photoIdFor("Z.jpg");
  const aId = photoIdFor("A.jpg");
  expect(photos.photos.map((photo) => photo.id)).toEqual([zId, aId]);
  const previewRequests: string[] = [];
  const stateBodies: unknown[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/preview")) previewRequests.push(request.url());
    if (request.url().includes("/state"))
      stateBodies.push(request.postDataJSON());
  });
  await page.goto(running.url);
  await page.getByRole("button", { name: /Library Review/ }).click();
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
  expect(await (await fetch(`${running.url}/api/photo-sets`)).json()).toEqual({
    photoSets: [],
  });
  await page.getByRole("button", { name: "Photo Sets" }).click();
  await page.getByRole("button", { name: /Library Review/ }).click();
  await expect(page.getByText("1 / 2")).toBeVisible();

  const { setId } = await createSet(running.url, "Explicit order");
  await post(running.url, `/api/photo-sets/${setId}/order`, {
    photoIds: [aId, zId],
  });
  await page.reload();
  await page.getByRole("button", { name: /^Explicit order(?: |$)/ }).click();
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
  const zId = photoIdFor("Z.jpg");
  const bId = photoIdFor("B.jpg");
  const previews: string[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/preview")) previews.push(request.url());
  });
  await page.goto(running.url);
  await page.getByRole("button", { name: /Library Review/ }).click();
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
  await page.getByRole("button", { name: "Photo Sets" }).click();
  await page.getByRole("button", { name: /Library Review/ }).click();
  await expect(page.getByText("1 / 3")).toBeVisible();
  await expect.poll(() => previews.some((url) => url.includes(bId))).toBe(true);
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
  const aId = photoIdFor("A.jpg");
  const zId = photoIdFor("Z.jpg");
  const bId = photoIdFor("B.jpg");
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
  await post(running.url, `/api/photo-sets/${setId}/members`, {
    photoIds: [bId],
  });
  await page.route("**/api/photos/*/preview", (route) => route.abort());
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await page.unroute("**/api/photos/*/preview");
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText("2 / 2")).toBeVisible();
  await page.getByRole("button", { name: "Photo Sets" }).click();
  await page.getByRole("button", { name: /^Snapshot(?: |$)/ }).click();
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
  await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeDisabled();
  releaseSuccess();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
  await page.unroute("**/api/photo-sets/*/progress");
  await expect
    .poll(async () => (await state(running.url, setId)).lastReviewedPhotoId)
    .toBe((await state(running.url, setId)).members[2]!.photoId);
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
  await expect(page.getByText("1 / 3")).toBeVisible();

  await page.getByRole("button", { name: "Photo Sets" }).click();
  await rm(join(root, "a.jpg"));
  await rm(join(root, "b.jpg"));
  await post(running.url, "/api/scan", {});
  await page.route("**/api/photo-sets", async (route) => {
    await route.fulfill({
      json: {
        photoSets: [
          {
            ...initial,
            lastReviewedPhotoId: savedId,
            members: initial.members.map((member) => ({
              ...member,
              available: false,
            })),
          },
        ],
      },
    });
  });
  await page.reload();
  await expect
    .poll(async () =>
      page.evaluate(async () => {
        const body = (await (await fetch("/api/photo-sets")).json()) as {
          photoSets: PhotoSetResponse[];
        };
        return body.photoSets[0]?.lastReviewedPhotoId;
      }),
    )
    .toBe(savedId);
  await page.getByRole("button", { name: /^Resume(?: |$)/ }).click();
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect(page.getByRole("button", { name: "Select" })).toBeEnabled();
});
