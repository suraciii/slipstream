import { expect, test } from "@playwright/test";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  setupBrowserSmoke,
  servers,
  jpeg,
  withCaptureTime,
  fixture,
  server,
  post,
  browseIds,
  createAlbum,
  cli,
  state,
} from "./browser-test-support/fixtures.js";
import {
  actionWithProgress,
  waitForGridFrame,
  expectGridOrder,
  openPhotoAndWaitForProgress,
  startReview,
  openSources,
  openPhotoToolsView,
  closePhotoTools,
  openMembershipPanel,
  membershipCheckbox,
  toggleAlbumMembership,
} from "./browser-test-support/surfaces.js";

setupBrowserSmoke();

test("album management creates, renames, and deletes Albums with confirmation", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("link", { name: /All Photos 1 Photo/ }),
  ).toBeVisible();

  // Create through the inline form.
  await page.getByRole("button", { name: "New Album" }).click();
  await page.getByLabel("Album name").fill("Trip");
  await page.getByRole("button", { name: "Create Album" }).click();
  await expect(page.getByRole("link", { name: /Trip 0 Photos/ })).toBeVisible();
  await expect(page.getByRole("button", { name: "Rename Trip" })).toBeVisible();

  // Rename keeps membership and identity semantics on the card.
  await page.getByRole("button", { name: "Rename Trip" }).click();
  await page.getByLabel("Album name").fill("Journey");
  await page.getByRole("button", { name: "Save Name" }).click();
  await expect(
    page.getByRole("link", { name: /Journey 0 Photos/ }),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: /Trip 0 Photos/ })).toBeHidden();

  // Deleting requires confirmation and states the safety contract.
  await page.getByRole("button", { name: "Delete Journey" }).click();
  await expect(
    page.getByText("Photos and Original Files remain unchanged."),
  ).toBeVisible();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(
    page.getByRole("link", { name: /Journey 0 Photos/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Delete Journey" }).click();
  await page.getByRole("button", { name: "Delete Album" }).click();
  await expect(
    page.getByRole("link", { name: /Journey 0 Photos/ }),
  ).toBeHidden();
  // Originals are untouched: All Photos keeps its count.
  await expect(
    page.getByRole("link", { name: /All Photos 1 Photo/ }),
  ).toBeVisible();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
});

test("the current photo joins and leaves albums from the photo view", async ({
  page,
}) => {
  const { base, root } = await fixture();
  await writeFile(join(root, "one.jpg"), await jpeg());
  const running = await server(base, root);
  const created = (await (
    await post(running.url, "/api/albums", { name: "Picks" })
  ).json()) as { albums: Array<{ id: string; name: string }> };
  const albumId = created.albums.find((album) => album.name === "Picks")!.id;
  await page.goto(running.url);
  await expect(page.getByText("Library ready", { exact: true })).toBeVisible();
  const [photoId] = await browseIds(running.url);
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.getByRole("heading", { name: "All Photos" })).toBeVisible();

  // A Photo that belongs to no Album states that plainly.
  await openPhotoToolsView(page, "albums");
  await expect(page.getByText("Not in any Album yet")).toBeVisible();

  // Adding the current Photo to an Album updates the listed membership and
  // the bounded counts.
  const firstAdd = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname.endsWith("/members") &&
      response.status() === 200,
  );
  await toggleAlbumMembership(page, "Picks");
  await firstAdd;
  await expect(page.locator("[data-status]")).toHaveText("Added to the Album.");
  await expect(page.getByText("Not in any Album yet")).toBeHidden();
  await expect(page.locator("[data-membership-list] li")).toHaveText(["Picks"]);
  await closePhotoTools(page);
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.getByRole("link", { name: /Picks 1 Photo/ })).toBeVisible();

  // A repeated add for an existing member stays one membership: the panel
  // lists the Album once and the counts stay at one Photo.
  await post(running.url, `/api/albums/${albumId}/members`, { photoId });
  await page.getByRole("button", { name: /^Photo 1 of 1/ }).click();
  await expect(page.locator("[data-membership-list] li")).toHaveText(["Picks"]);
  await openMembershipPanel(page);
  await expect(membershipCheckbox(page, "Picks")).toBeChecked();
  await expect
    .poll(async () => (await state(running.url, albumId)).members)
    .toHaveLength(1);
  await closePhotoTools(page);
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(page.getByRole("link", { name: /Picks 1 Photo/ })).toBeVisible();

  // Removing from the open Album source updates the count while the open
  // snapshot keeps its copied order.
  await page.getByRole("link", { name: /Picks 1 Photo/ }).click();
  await expect(page.getByText("Ready · 1 Photo")).toBeVisible();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /^Photo 1 of 1/ }),
  );
  await expect(page.locator("[data-membership-list] li")).toHaveText(["Picks"]);
  await toggleAlbumMembership(page, "Picks");
  await expect(page.locator("[data-status]")).toHaveText(
    "Removed from the Album. It stays in this open view until reopened.",
  );
  await expect(page.getByText("1 / 1")).toBeVisible();
  await expect(page.locator("[data-membership-list] li")).toHaveCount(0);
  await closePhotoTools(page);
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await expect(
    page.getByRole("link", { name: /^Picks 0 Photos$/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: /All Photos 1 Photo/ }),
  ).toBeVisible();
});

test("persists manual navigation and advanced current Photo across leave, reload, and restart", async ({
  page,
}) => {
  const { base, root } = await fixture();
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(join(root, name), await jpeg());
  let running = await server(base, root);
  const { albumId } = await createAlbum(running.url, "Progress");
  await startReview(page, running.url, "Progress", albumId);
  await openPhotoToolsView(page, "tools");
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Next", exact: true }).click(),
  );
  await expect
    .poll(async () => (await state(running.url, albumId)).position)
    .toBe(1);
  await closePhotoTools(page);
  await page.getByRole("button", { name: "Back to Grid" }).click();
  await openSources(page);
  await page.getByRole("link", { name: /^Progress \d+ Photos/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 2 of 3/ }),
  );
  await expect(page.getByText("2 / 3")).toBeVisible();
  await page.reload();
  // The Photo address preserves the destination across a reload, so the
  // reloaded document reopens the same Photo instead of the Album Grid.
  await expect(page.getByText("2 / 3")).toBeVisible();
  await openPhotoToolsView(page, "tools");
  await actionWithProgress(page, albumId, () =>
    page.getByRole("button", { name: "Next", exact: true }).click(),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
  await expect
    .poll(async () => (await state(running.url, albumId)).position)
    .toBe(2);
  await page.goto("about:blank");
  await running.close();
  servers.splice(servers.indexOf(running), 1);
  running = await server(base, root);
  await page.goto(running.url);
  await openSources(page);
  await page.getByRole("link", { name: /^Progress \d+ Photos/ }).click();
  await openPhotoAndWaitForProgress(
    page,
    albumId,
    page.getByRole("button", { name: /Photo 3 of 3/ }),
  );
  await expect(page.getByText("3 / 3")).toBeVisible();
});

test("a CLI-created Album opens in the Web with its ordered members and decisions", async ({
  page,
}) => {
  const { base, root } = await fixture();
  const source = await jpeg();
  // Capture Times order the selected Photos differently from filename and
  // insertion order, so the opened Grid proves the CLI query supplied it.
  await writeFile(
    join(root, "one.jpg"),
    withCaptureTime(source, "2026:03:04 10:00:00"),
  );
  await writeFile(
    join(root, "two.jpg"),
    withCaptureTime(source, "2026:01:02 10:00:00"),
  );
  await writeFile(
    join(root, "three.jpg"),
    withCaptureTime(source, "2026:02:03 10:00:00"),
  );
  await writeFile(
    join(root, "four.jpg"),
    withCaptureTime(source, "2026:04:05 10:00:00"),
  );
  await writeFile(
    join(root, "five.jpg"),
    withCaptureTime(source, "2026:05:06 10:00:00"),
  );
  await writeFile(
    join(root, "six.jpg"),
    withCaptureTime(source, "2026:06:07 10:00:00"),
  );
  const running = await server(base, root);
  // Fixture setup only: the source Album and the pre-existing decisions the
  // query filters for. The organization workflow itself uses the CLI alone.
  const ids = await browseIds(running.url);
  expect(ids).toHaveLength(6);
  // The Library lists Capture Time order: two, three, one, four, five, six.
  const [twoId, threeId, oneId, fourId] = ids;
  // The source Album is seeded in filename order (one, two, three, four,
  // five, six), deliberately not Capture Time order, so only the CLI query's
  // --order capture-time-asc can produce the asserted sequence.
  const { albumId: sourceAlbumId } = await createAlbum(
    running.url,
    "Source picks",
    [oneId!, twoId!, threeId!, fourId!, ids[4]!, ids[5]!],
  );
  const decisions = [
    { id: twoId, selectionState: "picked", rating: 4 },
    { id: threeId, selectionState: "picked", rating: 5 },
    { id: oneId, selectionState: "picked", rating: 5 },
    { id: fourId, selectionState: "picked", rating: 4 },
    { id: ids[4], selectionState: "rejected", rating: 3 },
  ];
  for (const decision of decisions) {
    for (const [field, value] of [
      ["selectionState", decision.selectionState],
      ["rating", decision.rating],
    ] as const) {
      const response = await post(
        running.url,
        `/api/photos/${decision.id}/state`,
        { field, value },
      );
      expect(response.ok).toBe(true);
    }
  }

  const queried = await cli(running.url, [
    "photos",
    "list",
    "--album",
    sourceAlbumId,
    "--selection",
    "picked",
    "--rating-min",
    "4",
    "--order",
    "capture-time-asc",
    "--limit",
    "60",
  ]);
  const orderedIds = (
    queried.data as { items: Array<{ id: string }> }
  ).items.map((item) => item.id);
  // Capture Time order — not filename order (four, one, three, two) and not
  // the seeded Album order (one, two, three, four).
  expect(orderedIds).toEqual([twoId, threeId, oneId, fourId]);

  const created = await cli(running.url, [
    "albums",
    "create",
    "--name",
    "CLI 精选",
  ]);
  const album = (
    created.data as {
      album: {
        id: string;
        albumVersion: string;
        webUrl: string;
      };
    }
  ).album;
  const membersPath = join(base, "members.json");
  await writeFile(membersPath, JSON.stringify({ photoIds: orderedIds }));
  const added = await cli(running.url, [
    "albums",
    "add",
    album.id,
    "--input",
    membersPath,
    "--if-version",
    album.albumVersion,
  ]);
  expect((added.data as { addedPhotoIds: string[] }).addedPhotoIds).toEqual(
    orderedIds,
  );

  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(album.webUrl);
  await expect(page.getByText("Ready · 4 Photos")).toBeVisible();
  await waitForGridFrame(page);
  await expectGridOrder(page, orderedIds);
  expect(new URL(page.url()).search).toBe(`?source=album&albumId=${album.id}`);
  const cell = (index: number) => page.locator(`[data-photo-index="${index}"]`);
  for (const [index, rating] of [4, 5, 5, 4].entries()) {
    await expect(cell(index).locator(".cell-state.picked")).toHaveText("✓");
    await expect(cell(index)).toHaveAttribute(
      "aria-label",
      new RegExp(`${rating} stars`),
    );
  }
});
