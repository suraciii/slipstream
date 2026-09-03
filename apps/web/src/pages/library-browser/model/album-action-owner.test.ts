import { describe, expect, test } from "bun:test";
import {
  createAlbumActionOwner,
  type AlbumActionFetch,
  type AlbumActionOwner,
} from "./album-action-owner.js";
import type { SourceAuthority } from "./source-grid-owner.js";

type Deferred<T> = Readonly<{
  promise: Promise<T>;
  resolve(value: T): void;
  reject(reason?: unknown): void;
}>;

const deferred = <T>(): Deferred<T> => {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
};

const response = (status = 204): Response => new Response(null, { status });

const jsonResponse = (body: unknown): Response =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });

const albumResponse = (
  albums: ReadonlyArray<{
    id: string;
    name: string;
    photoCount: number;
    hasSavedPosition: boolean;
  }>,
): Response => jsonResponse({ albums });

const defaultSourceAuthority = Object.freeze({}) as SourceAuthority;

const context = (
  options: {
    sourceAuthority?: SourceAuthority;
    ownsSurface?: () => boolean;
    form?: ReturnType<AlbumActionOwner["openForm"]>;
  } = {},
) => ({
  sourceAuthority: options.sourceAuthority ?? defaultSourceAuthority,
  surface: options.ownsSurface
    ? ({ kind: "photo", isCurrent: options.ownsSurface } as const)
    : ({ kind: "summary" } as const),
  ...(options.form ? { form: options.form } : {}),
});

describe("AlbumActionOwner", () => {
  test("owns the five Album write routes and bodies", async () => {
    const requests: Array<Readonly<{ path: string; init?: RequestInit }>> = [];
    const fetcher: AlbumActionFetch = (path, init) => {
      requests.push({ path, ...(init ? { init } : {}) });
      return Promise.resolve(
        path === "/api/albums"
          ? albumResponse([
              {
                id: "album-trip",
                name: "Trip",
                photoCount: 0,
                hasSavedPosition: false,
              },
            ])
          : response(),
      );
    };
    const owner = createAlbumActionOwner(fetcher);
    const form = owner.openForm("album-form-1");
    const actions = [
      owner.create("Trip", context({ form })),
      owner.rename("album-1", "Journey", context({ form })),
      owner.delete("album-1", context({ form })),
      owner.addMembership("album-1", "photo-1", context()),
      owner.removeMembership("album-1", "photo-1", context()),
    ];
    if (actions.some((action) => !action))
      throw new Error("expected all Album actions to be admitted");

    const outcomes = await Promise.all(
      actions.map((action) => action!.settlement),
    );
    expect(requests.map((request) => request.path)).toEqual([
      "/api/albums",
      "/api/albums/album-1/rename",
      "/api/albums/album-1/delete",
      "/api/albums/album-1/members",
      "/api/albums/album-1/members/remove",
    ]);
    expect(requests.map((request) => request.init?.body)).toEqual([
      JSON.stringify({ name: "Trip" }),
      JSON.stringify({ name: "Journey" }),
      JSON.stringify({}),
      JSON.stringify({ photoIds: ["photo-1"] }),
      JSON.stringify({ photoId: "photo-1" }),
    ]);
    expect(requests.every((request) => request.init?.method === "POST")).toBe(
      true,
    );
    expect(
      outcomes.every((outcome) => outcome.settlement.kind === "persisted"),
    ).toBe(true);
    for (const outcome of outcomes) owner.finish(outcome.mutation);
    owner.dispose();
  });

  test("returns only an exact, unambiguous created Album identity", async () => {
    const created = {
      id: "album-created",
      name: "Created",
      photoCount: 0,
      hasSavedPosition: false,
    };
    const owner = createAlbumActionOwner(() =>
      Promise.resolve(
        albumResponse([
          {
            id: "album-existing",
            name: "Existing",
            photoCount: 2,
            hasSavedPosition: true,
          },
          created,
        ]),
      ),
    );
    const form = owner.openForm("album-form-create");
    const action = owner.create("Created", context({ form }));
    if (!action) throw new Error("expected create admission");
    const outcome = await action.settlement;
    if (outcome.kind !== "persisted") throw new Error("expected persistence");

    expect(outcome.createdAlbum).toEqual(created);
    expect(Object.isFrozen(outcome.createdAlbum)).toBe(true);
    expect(owner.isFormCurrent(form)).toBe(true);
    owner.finish(outcome.mutation);
    owner.dispose();

    const asciiOnlyOwner = createAlbumActionOwner(() =>
      Promise.resolve(
        albumResponse([
          { ...created, id: "album-accent-upper", name: "Éclair" },
          { ...created, id: "album-accent-lower", name: "éclair" },
        ]),
      ),
    );
    const asciiOnlyForm = asciiOnlyOwner.openForm("album-form-ascii-only");
    const asciiOnlyAction = asciiOnlyOwner.create(
      "Éclair",
      context({ form: asciiOnlyForm }),
    );
    if (!asciiOnlyAction)
      throw new Error("expected ASCII-only create admission");
    const asciiOnlyOutcome = await asciiOnlyAction.settlement;
    if (asciiOnlyOutcome.kind !== "persisted")
      throw new Error("expected ASCII-only persistence");
    expect(asciiOnlyOutcome.createdAlbum?.name).toBe("Éclair");
    asciiOnlyOwner.finish(asciiOnlyOutcome.mutation);
    asciiOnlyOwner.dispose();

    const malformedCases: ReadonlyArray<
      readonly [description: string, body: unknown]
    > = [
      ["malformed Album shape", { albums: [{ ...created, photoCount: "0" }] }],
      [
        "zero exact name matches",
        {
          albums: [
            {
              id: "album-existing",
              name: "Existing",
              photoCount: 0,
              hasSavedPosition: false,
            },
          ],
        },
      ],
      [
        "multiple exact name matches",
        { albums: [created, { ...created, id: "album-duplicate-name" }] },
      ],
      [
        "an ASCII-NOCASE name collision",
        {
          albums: [
            created,
            {
              ...created,
              id: "album-case-collision",
              name: "created",
            },
          ],
        },
      ],
      [
        "a sole case-variant instead of the exact created name",
        { albums: [{ ...created, name: "created" }] },
      ],
      [
        "an ID duplicated by another Album",
        {
          albums: [
            created,
            {
              ...created,
              name: "Existing",
            },
          ],
        },
      ],
      ["a nonempty match", { albums: [{ ...created, photoCount: 1 }] }],
      [
        "a match with saved position",
        { albums: [{ ...created, hasSavedPosition: true }] },
      ],
    ];
    for (const [description, body] of malformedCases) {
      const ambiguousOwner = createAlbumActionOwner(() =>
        Promise.resolve(jsonResponse(body)),
      );
      const ambiguousForm = ambiguousOwner.openForm("album-form-ambiguous");
      const ambiguous = ambiguousOwner.create(
        "Created",
        context({ form: ambiguousForm }),
      );
      if (!ambiguous) throw new Error("expected ambiguous create admission");
      const ambiguousOutcome = await ambiguous.settlement;
      if (ambiguousOutcome.kind !== "failed")
        throw new Error(`expected ${description} to fail`);
      expect(ambiguousOutcome.settlement).toEqual({ kind: "malformed" });
      expect(ambiguousOutcome.connectivity).toBe("unchanged");
      expect(ambiguousOutcome.failureMessage).toBe(
        "The Album could not be created.",
      );
      expect(ambiguousOwner.isFormCurrent(ambiguousForm)).toBe(true);
      ambiguousOwner.finish(ambiguousOutcome.mutation);
      ambiguousOwner.dispose();
    }
  });

  test("suppresses only duplicate membership keys", async () => {
    const held = deferred<Response>();
    const owner = createAlbumActionOwner(() => held.promise);
    const first = owner.addMembership("album-1", "photo-1", context());
    if (!first) throw new Error("expected first membership admission");

    expect(owner.isMembershipAdmitted("add", "album-1", "photo-1")).toBe(true);
    expect(
      owner.addMembership("album-1", "photo-1", context()),
    ).toBeUndefined();
    const independentAlbum = owner.addMembership(
      "album-2",
      "photo-1",
      context(),
    );
    const independentVerb = owner.removeMembership(
      "album-1",
      "photo-1",
      context(),
    );
    if (!independentAlbum || !independentVerb)
      throw new Error("expected independent membership admissions");
    expect(independentAlbum).toBeDefined();
    expect(independentVerb).toBeDefined();

    held.resolve(response());
    const outcomes = await Promise.all([
      first.settlement,
      independentAlbum.settlement,
      independentVerb.settlement,
    ]);
    for (const outcome of outcomes) owner.finish(outcome.mutation);
    expect(owner.isMembershipAdmitted("add", "album-1", "photo-1")).toBe(false);
    owner.dispose();
  });

  test("keeps global latest-wins and Photo surface ownership independent", async () => {
    const older = deferred<Response>();
    const newer = deferred<Response>();
    let request = 0;
    let olderSurfaceCurrent = true;
    const owner = createAlbumActionOwner(() => {
      request += 1;
      return request === 1 ? older.promise : newer.promise;
    });
    const first = owner.addMembership(
      "album-1",
      "photo-1",
      context({ ownsSurface: () => olderSurfaceCurrent }),
    );
    const second = owner.removeMembership(
      "album-2",
      "photo-2",
      context({ ownsSurface: () => true }),
    );
    if (!first || !second) throw new Error("expected independent admissions");

    olderSurfaceCurrent = false;
    older.resolve(response(503));
    const olderOutcome = await first.settlement;
    expect(owner.isLatest(olderOutcome.mutation)).toBe(false);
    expect(owner.canPresent(olderOutcome.surface)).toBe(false);
    expect(olderOutcome.connectivity).toBe("lost-if-latest");

    newer.resolve(response());
    const newerOutcome = await second.settlement;
    expect(owner.isLatest(newerOutcome.mutation)).toBe(true);
    expect(owner.canPresent(newerOutcome.surface)).toBe(true);
    owner.finish(newerOutcome.mutation);
    expect(owner.isLatest(olderOutcome.mutation)).toBe(false);
    owner.finish(olderOutcome.mutation);
    owner.dispose();
  });

  test("classifies answered client failures without losing connectivity", async () => {
    for (const status of [404, 409, 422]) {
      const owner = createAlbumActionOwner(() =>
        Promise.resolve(response(status)),
      );
      const action = owner.create("Duplicate", context());
      if (!action) throw new Error("expected Album action admission");
      const outcome = await action.settlement;
      if (outcome.kind !== "failed") throw new Error("expected rejection");

      expect(outcome.settlement).toEqual({ kind: "rejected", status });
      expect(outcome.connectivity).toBe("unchanged");
      expect(outcome.failureMessage).toBe(
        status === 409
          ? "An Album with this name already exists."
          : "The Album could not be created.",
      );
      owner.finish(outcome.mutation);
      owner.dispose();
    }
  });

  test("classifies service and transport failures for the global latest owner", async () => {
    const serviceOwner = createAlbumActionOwner(() =>
      Promise.resolve(response(503)),
    );
    const service = serviceOwner.rename("album-1", "Lost", context());
    if (!service) throw new Error("expected rename admission");
    const serviceOutcome = await service.settlement;
    if (serviceOutcome.kind !== "failed")
      throw new Error("expected service failure");
    expect(serviceOutcome.connectivity).toBe("lost-if-latest");
    expect(serviceOutcome.failureMessage).toBe(
      "The Album could not be renamed.",
    );
    serviceOwner.finish(serviceOutcome.mutation);
    serviceOwner.dispose();

    const transportOwner = createAlbumActionOwner(() =>
      Promise.reject(new Error("offline")),
    );
    const transport = transportOwner.delete("album-1", context());
    if (!transport) throw new Error("expected delete admission");
    const transportOutcome = await transport.settlement;
    if (transportOutcome.kind !== "failed")
      throw new Error("expected transport failure");
    expect(transportOutcome.settlement).toEqual({ kind: "transport-failed" });
    expect(transportOutcome.connectivity).toBe("lost-if-latest");
    expect(transportOutcome.failureMessage).toBe(
      "The Album could not be deleted.",
    );
    transportOwner.finish(transportOutcome.mutation);
    transportOwner.dispose();
  });

  test("keys form continuation by stable form identity", () => {
    const owner = createAlbumActionOwner(() => Promise.resolve(response()));
    const first = owner.openForm("album-form-1");
    expect(owner.openForm("album-form-1")).toBe(first);
    const second = owner.openForm("album-form-2");
    expect(owner.isFormCurrent(first)).toBe(false);
    expect(owner.isFormCurrent(second)).toBe(true);

    owner.closeForm(first);
    expect(owner.isFormCurrent(second)).toBe(true);
    owner.closeForm(second);
    expect(owner.isFormCurrent(second)).toBe(false);
    owner.dispose();
  });

  test("returns source-guarded current-Album removal metadata", async () => {
    const owner = createAlbumActionOwner(() => Promise.resolve(response()));
    const sourceAuthority = Object.freeze({}) as SourceAuthority;
    const action = owner.removeMembership(
      "album-1",
      "photo-1",
      context({ sourceAuthority, ownsSurface: () => true }),
    );
    if (!action) throw new Error("expected remove admission");
    const outcome = await action.settlement;
    if (outcome.kind !== "persisted") throw new Error("expected persistence");

    expect(outcome.removedFromCurrentAlbum).toEqual({
      albumId: "album-1",
      photoId: "photo-1",
      sourceAuthority,
    });
    expect(Object.isFrozen(outcome)).toBe(true);
    expect(Object.isFrozen(outcome.settlement)).toBe(true);
    expect(Object.isFrozen(outcome.removedFromCurrentAlbum)).toBe(true);
    owner.finish(outcome.mutation);
    owner.dispose();
  });

  test("dispose rejects new actions but lets an admitted write settle silently", async () => {
    const held = deferred<Response>();
    const owner = createAlbumActionOwner(() => held.promise);
    const action = owner.addMembership(
      "album-1",
      "photo-1",
      context({
        ownsSurface: () => true,
      }),
    );
    if (!action) throw new Error("expected admitted membership");

    owner.dispose();
    expect(owner.create("Late", context())).toBeUndefined();
    held.resolve(response());
    const outcome = await action.settlement;
    expect(outcome.settlement.kind).toBe("persisted");
    expect(owner.isLatest(outcome.mutation)).toBe(false);
    expect(owner.canPresent(outcome.surface)).toBe(false);
    owner.finish(outcome.mutation);
  });
});
