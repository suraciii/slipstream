import { expect, test } from "bun:test";
import { createFileLocationOwner } from "./file-location-owner.js";
import { createSourceGridOwner } from "./source-grid-owner.js";
import { createSourceOpenOwner } from "./source-open-owner.js";
import type { SourceGridFetch } from "../api/source-grid.js";

const folder = {
  kind: "folder" as const,
  folder: { location: "Trip", name: "Trip" },
  publication: "old",
};
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
};
const opened = () =>
  Response.json({
    token: "browse-1",
    total: 3,
    position: 2,
    selectionCounts: { selected: 0, rejected: 0, undecided: 3 },
  });

function fixture(browse: SourceGridFetch) {
  let publication = "published-1";
  let rootReads = 0;
  const fileLocations = createFileLocationOwner(() => {
    rootReads += 1;
    return Promise.resolve(
      Response.json({
        publication,
        parent: "",
        children: [],
        total: 0,
        start: 0,
      }),
    );
  });
  const sourceGrid = createSourceGridOwner((input, init) =>
    init?.method === "DELETE"
      ? Promise.resolve(new Response(null, { status: 204 }))
      : browse(input, init),
  );
  const notices: string[] = [];
  const rebindFileLocations = async () => {
    publication = "published-2";
    const authority = fileLocations.reset();
    await fileLocations.awaitRootBinding();
    return authority;
  };
  const create = (rebind = rebindFileLocations) =>
    createSourceOpenOwner({
      sourceGrid,
      fileLocations,
      rebindFileLocations: rebind,
      onPublicationConflict: (value) => notices.push(value),
    });
  return {
    sourceGrid,
    fileLocations,
    notices,
    create,
    rebindFileLocations,
    rootReads: () => rootReads,
    dispose() {
      sourceGrid.dispose();
      fileLocations.dispose();
    },
  };
}

test("only confirmed 404 licenses a missing-source fallback", async () => {
  for (const [status, kind] of [
    [404, "missing"],
    [500, "failed"],
  ] as const) {
    const f = fixture(() => Promise.resolve(new Response(null, { status })));
    try {
      expect(await f.create().beginOpen({ kind: "library" }).outcome).toEqual({
        kind,
      });
    } finally {
      f.dispose();
    }
  }
});

test("Folder opens use the current publication and admit its resolved position", async () => {
  let request: Record<string, unknown> | undefined;
  const f = fixture((_input, init) => {
    if (typeof init?.body !== "string") throw new Error("Expected JSON body");
    request = JSON.parse(init.body) as Record<string, unknown>;
    return Promise.resolve(opened());
  });
  try {
    await f.fileLocations.awaitRootBinding();
    const opening = f.create().beginOpen(folder);
    expect(await opening.outcome).toEqual({ kind: "opened", position: 2 });
    expect(request).toEqual({
      source: "folder",
      folderPath: "Trip",
      publication: "published-1",
    });
    expect(f.sourceGrid.isCurrent(opening.authority)).toBe(true);
  } finally {
    f.dispose();
  }
});

test("a current Folder conflict rebinds publication but remains retryable", async () => {
  const f = fixture(() => Promise.resolve(new Response(null, { status: 409 })));
  try {
    expect(await f.create().beginOpen(folder).outcome).toEqual({
      kind: "failed",
    });
    expect(f.fileLocations.publication).toBe("published-2");
    expect(f.notices).toEqual(["published-2"]);
  } finally {
    f.dispose();
  }
});

test("a stale conflict cannot reset the newer destination's File Locations", async () => {
  const pending = deferred<Response>();
  let calls = 0;
  const f = fixture(async () => (++calls === 1 ? pending.promise : opened()));
  try {
    const owner = f.create();
    const stale = owner.beginOpen(folder);
    await owner.beginOpen({ kind: "library" }).outcome;
    pending.resolve(new Response(null, { status: 409 }));
    expect(await stale.outcome).toEqual({ kind: "superseded" });
    expect(f.rootReads()).toBe(0);
    expect(f.notices).toEqual([]);
    expect(f.sourceGrid.kind).toBe("library");
  } finally {
    f.dispose();
  }
});

test("a source superseded during conflict rebinding cannot publish a notice", async () => {
  const entered = deferred<void>();
  const resume = deferred<void>();
  let calls = 0;
  const f = fixture(() =>
    Promise.resolve(
      ++calls === 1 ? new Response(null, { status: 409 }) : opened(),
    ),
  );
  try {
    const owner = f.create(async () => {
      entered.resolve();
      await resume.promise;
      return f.rebindFileLocations();
    });
    const stale = owner.beginOpen(folder);
    await entered.promise;
    await owner.beginOpen({ kind: "library" }).outcome;
    resume.resolve();
    expect(await stale.outcome).toEqual({ kind: "superseded" });
    expect(f.notices).toEqual([]);
    expect(f.sourceGrid.kind).toBe("library");
  } finally {
    f.dispose();
  }
});
