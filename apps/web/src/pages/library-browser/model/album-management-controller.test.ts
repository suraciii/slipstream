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
  const pending: string[] = [];
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
      const admission = start({
        sourceAuthority,
        surface: { kind: surface },
        ...(form ? { form } : {}),
      });
      if (!admission) return { ok: false, latest: false };
      const outcome = await admission.settlement;
      if (outcome.kind !== "persisted") {
        settled.resolve();
        return { ok: false, latest: false };
      }
      settled.resolve();
      return {
        ok: true,
        latest: true,
        ...(outcome.createdAlbum ? { createdAlbum: outcome.createdAlbum } : {}),
        ...(outcome.folderAdd ? { folderAdd: outcome.folderAdd } : {}),
      };
    },
    present: {
      setFormPending: (id) => pending.push(id),
      setFormMessage: () => {},
      dismissForm: (id) => dismissed.push(id),
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
    pending,
    rendered,
    settled,
    switchSource: () =>
      (sourceAuthority = Object.freeze({}) as SourceAuthority),
    switchPhoto: () => (photo = Object.freeze({}) as PhotoAuthority),
  };
};

describe("album management controller fences", () => {
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
