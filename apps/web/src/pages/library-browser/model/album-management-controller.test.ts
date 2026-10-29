import { describe, expect, test } from "bun:test";
import {
  createAlbumActionOwner,
  type AlbumActionFetch,
} from "./album-action-owner.js";
import { createAlbumManagementController } from "./album-management-controller.js";
import type { SourceAuthority } from "./source-grid-owner.js";
import type { PhotoAuthority } from "./photo-owner.js";

type Deferred<T> = Readonly<{ promise: Promise<T>; resolve(value: T): void }>;
const deferred = <T>(): Deferred<T> => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((accept) => (resolve = accept));
  return { promise, resolve };
};
const authority = Object.freeze({}) as SourceAuthority;
const photoAuthority = Object.freeze({}) as PhotoAuthority;
const album = {
  id: "album-1",
  name: "Picks",
  photoCount: 0,
  hasSavedPosition: false,
} as const;
const json = (value: unknown) =>
  new Response(JSON.stringify(value), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
const createBody = (name: string) => ({
  albums: [{ id: "created", name, photoCount: 0, hasSavedPosition: false }],
});

const setup = (answer: Deferred<Response>) => {
  let sourceAuthority = authority;
  let photo = photoAuthority;
  const folder = { authority, path: "/photos", publication: "pub-1" };
  const fetcher: AlbumActionFetch = async () => answer.promise;
  const actions = createAlbumActionOwner(fetcher);
  const navigated: string[] = [];
  const dismissed: string[] = [];
  const continuations: Array<{
    id: string;
    albumId?: string;
    suppressed?: boolean;
  }> = [];
  const pending: string[] = [];
  const messages: Array<{ formId: string; message: string }> = [];
  const pendingNames: Array<{ formId: string; name?: string }> = [];
  let refresh = () => Promise.resolve(true);
  let writes = 0;
  const settled = deferred<void>();
  const rendered: Array<
    Readonly<{ selectedAlbumId: string; pending: boolean; status?: string }>
  > = [];
  const controller = createAlbumManagementController({
    actions,
    albums: () => [album],
    source: {
      get authority() {
        return sourceAuthority;
      },
      current: () => (sourceAuthority === authority ? folder : undefined),
      isCurrent: (value) => value === sourceAuthority,
    },
    photo: {
      get authority() {
        return photo;
      },
      isCurrent: (value) => value === photo,
    },
    mutate: async (start, surface, _capturedPhoto, form) => {
      writes += 1;
      const admission = start({
        sourceAuthority,
        surface: { kind: surface },
        ...(form ? { form } : {}),
      });
      if (!admission) return { ok: false, latest: false };
      const outcome = await admission.settlement;
      if (outcome.kind !== "persisted") {
        settled.resolve();
        return {
          ok: false,
          latest: false,
          failureMessage: outcome.failureMessage,
          unknown:
            admission.noticeKey === "create" &&
            (outcome.settlement.kind === "malformed" ||
              outcome.connectivity === "lost-if-latest"),
        };
      }
      settled.resolve();
      return {
        ok: true,
        latest: true,
        ...(outcome.createdAlbum ? { createdAlbum: outcome.createdAlbum } : {}),
        ...(outcome.folderAdd ? { folderAdd: outcome.folderAdd } : {}),
      };
    },
    refreshOverview: () => refresh(),
    present: {
      setFormPending: (id, _pending, name) => {
        pending.push(id);
        pendingNames.push({ formId: id, ...(name ? { name } : {}) });
      },
      setFormMessage: (formId, message) => messages.push({ formId, message }),
      dismissForm: (id, created, suppressed) => {
        dismissed.push(id);
        continuations.push({
          id,
          ...(created ? { albumId: created.id } : {}),
          ...(suppressed ? { suppressed } : {}),
        });
      },
      renderFolder: (model) => rendered.push(model),
    },
    onCreatedAlbum: (value) => {
      navigated.push(value.id);
      return Promise.resolve();
    },
    onDeletedAlbum: (id) => {
      navigated.push(`deleted:${id}`);
      return Promise.resolve();
    },
  });
  return {
    controller,
    actions,
    dismissed,
    navigated,
    continuations,
    pending,
    messages,
    pendingNames,
    rendered,
    settled,
    setRefresh: (next: () => Promise<boolean>) => (refresh = next),
    writes: () => writes,
    switchSource: () =>
      (sourceAuthority = Object.freeze({}) as SourceAuthority),
    switchPhoto: () => (photo = Object.freeze({}) as PhotoAuthority),
  };
};

test("unknown creation stays locked across close and failed or deferred inspection", async () => {
  const answer = deferred<Response>();
  const state = setup(answer);
  state.controller.openForm({ formId: "first", kind: "create", name: "" });
  const submission = state.controller.submitForm("first", "Uncertain");
  state.controller.closeForm("first");
  state.controller.openForm({ formId: "second", kind: "create", name: "" });
  await state.controller.submitForm("second", "Another");
  expect(state.writes()).toBe(1);
  state.controller.closeForm("second");
  state.switchSource();
  answer.resolve(json({ albums: [] }));
  await submission;
  state.controller.openForm({ formId: "second", kind: "create", name: "" });
  expect(state.messages.at(-1)?.message).toContain("Inspect the Album list");
  state.controller.closeForm("second");
  const waiting = deferred<boolean>();
  state.setRefresh(() => waiting.promise);
  const inspection = state.controller.inspectAlbums();
  state.switchSource();
  state.controller.openForm({ formId: "third", kind: "create", name: "" });
  await state.controller.submitForm("third", "Another");
  expect(state.writes()).toBe(1);
  waiting.resolve(false);
  await inspection;
  await state.controller.submitForm("third", "Another");
  expect(state.writes()).toBe(1);
  state.setRefresh(() => Promise.resolve(true));
  await state.controller.inspectAlbums();
  await state.controller.submitForm("third", "Another");
  expect(state.writes()).toBe(2);
});

describe("album management controller fences", () => {
  test("duplicate create and rename keep their current form recoverable", async () => {
    for (const kind of ["create", "rename"] as const) {
      const answer = deferred<Response>();
      const state = setup(answer);
      state.controller.openForm({
        formId: kind,
        kind,
        albumId: album.id,
        name: kind === "rename" ? "Original" : "",
      });
      const submission = state.controller.submitForm(kind, "Picks");
      answer.resolve(new Response("{}", { status: 409 }));
      await submission;
      expect(state.messages).toEqual([
        { formId: kind, message: "An Album with this name already exists." },
      ]);
      expect(state.pendingNames).toContainEqual({
        formId: kind,
        name: "Picks",
      });
      expect(state.dismissed).toEqual([]);
      expect(state.navigated).toEqual([]);
    }
  });

  test("a failed older form cannot put its error into a replacement form", async () => {
    const answer = deferred<Response>();
    const state = setup(answer);
    state.controller.openForm({ formId: "first", kind: "create", name: "" });
    const submission = state.controller.submitForm("first", "Picks");
    state.controller.openForm({ formId: "second", kind: "create", name: "" });
    answer.resolve(new Response("{}", { status: 409 }));
    await submission;
    expect(state.messages).toEqual([]);
    expect(state.dismissed).toEqual([]);
  });
  test("late settlement cannot dismiss a replacement form or navigate", async () => {
    const answer = deferred<Response>();
    const setupState = setup(answer);
    setupState.controller.openForm({
      formId: "first",
      kind: "create",
      name: "",
    });
    const first = setupState.controller.submitForm("first", "New Album");
    setupState.controller.openForm({
      formId: "second",
      kind: "create",
      name: "",
    });
    answer.resolve(json(createBody("New Album")));
    await first;
    expect(setupState.dismissed).toEqual([]);
    expect(setupState.navigated).toEqual([]);
  });

  test("source or photo supersession suppresses created Album navigation", async () => {
    for (const supersede of ["source", "photo"] as const) {
      const answer = deferred<Response>();
      const state = setup(answer);
      state.controller.openForm({
        formId: supersede,
        kind: "create",
        name: "",
      });
      const submission = state.controller.submitForm(supersede, "Created");
      if (supersede === "source") state.switchSource();
      else state.switchPhoto();
      answer.resolve(json(createBody("Created")));
      await submission;
      expect(state.navigated).toEqual([]);
    }
  });

  test("create captures destination identity at submission, not form opening", async () => {
    const answer = deferred<Response>();
    const state = setup(answer);
    state.controller.openForm({ formId: "create", kind: "create", name: "" });
    state.switchSource();
    state.switchPhoto();
    const submission = state.controller.submitForm("create", "Created");
    answer.resolve(json(createBody("Created")));
    await submission;
    expect(state.navigated).toEqual(["created"]);
    expect(state.dismissed).toEqual(["create"]);
  });

  test("contextual creation returns the new Album without navigation", async () => {
    const answer = deferred<Response>();
    const state = setup(answer);
    state.controller.openForm({
      formId: "context",
      kind: "create",
      name: "",
      contextual: true,
    });
    const submission = state.controller.submitForm("context", "Created");
    answer.resolve(json(createBody("Created")));
    await submission;
    expect(state.continuations).toEqual([
      { id: "context", albumId: "created" },
    ]);
    expect(state.navigated).toEqual([]);
  });

  test("a superseded contextual creation cannot return to its parent", async () => {
    const answer = deferred<Response>();
    const state = setup(answer);
    state.controller.openForm({
      formId: "context",
      kind: "create",
      name: "",
      contextual: true,
    });
    const submission = state.controller.submitForm("context", "Created");
    state.switchSource();
    answer.resolve(json(createBody("Created")));
    await submission;
    expect(state.continuations).toEqual([
      { id: "context", albumId: "created", suppressed: true },
    ]);
    expect(state.navigated).toEqual([]);
  });

  test("folder success is not presented after the source is superseded", async () => {
    const answer = deferred<Response>();
    const state = setup(answer);
    const submission = state.controller.addFolderToAlbum("album-1");
    state.switchSource();
    answer.resolve(
      json({
        albumId: "album-1",
        folderPath: "/photos",
        publication: "pub-1",
        matchedCount: 1,
        addedCount: 1,
        alreadyMemberCount: 0,
        albums: [album],
      }),
    );
    await submission;
    expect(state.rendered.at(-1)).toMatchObject({ pending: true });
    expect(state.rendered.some((model) => model.status !== undefined)).toBe(
      false,
    );
  });

  test("disposal suppresses an admitted create callback", async () => {
    const answer = deferred<Response>();
    const state = setup(answer);
    state.controller.openForm({ formId: "disposed", kind: "create", name: "" });
    const submission = state.controller.submitForm("disposed", "Created");
    state.controller.dispose();
    answer.resolve(json(createBody("Created")));
    await submission;
    expect(state.navigated).toEqual([]);
  });
});
