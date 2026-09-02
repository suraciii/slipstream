import { describe, expect, test } from "bun:test";
import { createFileLocationOwner } from "./file-location-owner.js";

const windowResponse = (
  publication: string,
  parent: string,
  children: ReadonlyArray<{
    location: string;
    name: string;
    photoCount: number;
    hasDescendantFolders: boolean;
  }> = [],
) =>
  new Response(
    JSON.stringify({
      publication,
      parent,
      start: 0,
      limit: 60,
      total: children.length,
      children,
    }),
    { status: 200 },
  );

const parentFrom = (input: string): string =>
  new URL(input, "http://slipstream.test").searchParams.get("parent") ?? "";

describe("FileLocationOwner", () => {
  test("keeps exact failed ranges while unrelated windows remain usable", async () => {
    let failing = true;
    const requested: string[] = [];
    const owner = createFileLocationOwner((input) => {
      const parent = parentFrom(input);
      requested.push(parent);
      return Promise.resolve(
        parent === "failed" && failing
          ? new Response(null, { status: 503 })
          : windowResponse("published-1", parent),
      );
    });

    expect((await owner.loadWindow("", 0, false)).kind).toBe("loaded");
    const failed = await owner.loadWindow("failed", 0);
    expect(failed.kind).toBe("failed");
    if (failed.kind !== "failed") throw new Error("expected failed range");
    expect(owner.failures()).toEqual([failed.failure]);

    const sibling = await owner.loadWindow("sibling", 0);
    expect(sibling).toMatchObject({
      kind: "loaded",
      markTransportReachable: true,
      remainingNewest: failed.failure,
    });
    expect(owner.window("sibling")).toBeDefined();
    expect(owner.failures()).toEqual([failed.failure]);

    failing = false;
    const recovered = await owner.retry(failed.failure);
    expect(recovered).toMatchObject({
      kind: "loaded",
      recovered: failed.failure,
      markTransportReachable: true,
    });
    expect(owner.failures()).toEqual([]);
    expect((await owner.retry(failed.failure)).kind).toBe("detached");
    expect(requested).toEqual(["", "failed", "sibling", "failed"]);
  });

  test("shares one root bind and detaches every waiter on reset", async () => {
    let settle!: (response: Response) => void;
    let requests = 0;
    const owner = createFileLocationOwner(
      () =>
        new Promise<Response>((resolve) => {
          requests += 1;
          settle = resolve;
        }),
    );

    const first = owner.awaitRootBinding();
    const second = owner.awaitRootBinding();
    await Promise.resolve();
    expect(requests).toBe(1);
    owner.reset();
    expect((await first).kind).toBe("detached");
    expect((await second).kind).toBe("detached");

    settle(windowResponse("stale", ""));
    await Promise.resolve();
    await Promise.resolve();
    expect(owner.publication).toBeUndefined();
  });

  test("reports publication conflicts without rebinding or changing retained state", async () => {
    const owner = createFileLocationOwner((input) => {
      const parent = parentFrom(input);
      return Promise.resolve(
        parent === "expired"
          ? new Response(null, { status: 409 })
          : windowResponse("published-1", parent),
      );
    });

    await owner.loadWindow("", 0, true);
    const retainedRoot = owner.window("");
    const conflict = await owner.loadWindow("expired", 0);
    expect(conflict).toMatchObject({
      kind: "publication-conflict",
      generation: 0,
      parent: "expired",
      page: 0,
    });
    expect(owner.publication).toBe("published-1");
    expect(owner.window("")).toBe(retainedRoot);
  });

  test("bounds expanded direct-child windows while retaining the root", async () => {
    const owner = createFileLocationOwner((input) => {
      const parent = parentFrom(input);
      return Promise.resolve(windowResponse("published-1", parent));
    });

    await owner.loadWindow("", 0, true);
    for (let index = 0; index < 33; index += 1) {
      const outcome = await owner.loadWindow(`folder-${index}`, 0, true);
      expect(owner.accept(outcome)).toBe(true);
    }

    expect(owner.isExpanded("")).toBe(true);
    expect(owner.window("")).toBeDefined();
    expect(owner.window("folder-0")).toBeUndefined();
    expect(owner.window("folder-1")).toBeUndefined();
    expect(owner.window("folder-32")).toBeDefined();
    expect(
      [
        "",
        ...Array.from({ length: 33 }, (_, index) => `folder-${index}`),
      ].filter((parent) => owner.isExpanded(parent)),
    ).toHaveLength(32);
  });

  test("same-parent supersession detaches a late response and keeps the newer page", async () => {
    const pending: Array<{
      url: string;
      settle(response: Response): void;
    }> = [];
    const owner = createFileLocationOwner((url) => {
      if (parentFrom(url) === "")
        return Promise.resolve(windowResponse("published-1", ""));
      return new Promise<Response>((resolve) => {
        pending.push({ url, settle: resolve });
      });
    });
    await owner.loadWindow("", 0, false);

    const older = owner.loadWindow("same", 0);
    await Promise.resolve();
    const newer = owner.loadWindow("same", 1);
    await Promise.resolve();
    expect(pending).toHaveLength(2);
    pending[1]!.settle(
      windowResponse("published-1", "same", [
        {
          location: "same/newer",
          name: "newer",
          photoCount: 1,
          hasDescendantFolders: false,
        },
      ]),
    );
    const newerOutcome = await newer;
    expect(newerOutcome).toMatchObject({ kind: "loaded", page: 1 });
    pending[0]!.settle(
      windowResponse("published-1", "same", [
        {
          location: "same/older",
          name: "older",
          photoCount: 1,
          hasDescendantFolders: false,
        },
      ]),
    );
    const olderOutcome = await older;
    expect(olderOutcome.kind).toBe("detached");
    expect(owner.accept(olderOutcome)).toBe(false);
    expect(owner.accept(newerOutcome)).toBe(true);
    expect(owner.window("same")?.children[0]?.location).toBe("same/newer");
  });

  test("collapse does not detach an already admitted parent window", async () => {
    let settle!: (response: Response) => void;
    let openRequests = 0;
    const owner = createFileLocationOwner((url) => {
      const parent = parentFrom(url);
      if (parent !== "open")
        return Promise.resolve(windowResponse("published-1", parent));
      openRequests += 1;
      if (openRequests === 1)
        return Promise.resolve(windowResponse("published-1", parent));
      return new Promise<Response>((resolve) => {
        settle = resolve;
      });
    });
    expect(owner.accept(await owner.loadWindow("", 0, false))).toBe(true);
    expect(owner.accept(await owner.loadWindow("open", 0, true))).toBe(true);
    const pending = owner.loadWindow("open", 1, true);
    await Promise.resolve();
    expect(owner.collapse("open")).toBe(true);
    settle(windowResponse("published-1", "open"));
    const outcome = await pending;

    expect(outcome.kind).toBe("loaded");
    expect(owner.accept(outcome)).toBe(true);
  });

  test("detaches a mismatched bound publication and sends the exact query", async () => {
    const requested: string[] = [];
    const owner = createFileLocationOwner((url) => {
      requested.push(url);
      const parent = parentFrom(url);
      return Promise.resolve(
        windowResponse(parent === "" ? "published-1" : "published-2", parent),
      );
    });
    await owner.loadWindow("", 0, false);

    const mismatch = await owner.loadWindow("nested folder", 2);
    expect(mismatch.kind).toBe("detached");
    expect(owner.publication).toBe("published-1");
    expect(owner.window("nested folder")).toBeUndefined();
    const query = new URL(requested[1]!, "http://slipstream.test").searchParams;
    expect(Object.fromEntries(query)).toEqual({
      start: "120",
      limit: "60",
      parent: "nested folder",
      publication: "published-1",
    });
  });

  test("replaces only the exact failure identity and retains newest ordering", async () => {
    let recoverA = false;
    const owner = createFileLocationOwner((url) => {
      const parent = parentFrom(url);
      if (parent === "")
        return Promise.resolve(windowResponse("published-1", ""));
      return Promise.resolve(
        parent === "a" && recoverA
          ? windowResponse("published-1", parent)
          : new Response(null, { status: 503 }),
      );
    });
    await owner.loadWindow("", 0, false);

    const firstA = await owner.loadWindow("a", 0);
    const firstB = await owner.loadWindow("b", 0);
    if (firstA.kind !== "failed" || firstB.kind !== "failed")
      throw new Error("expected two failed ranges");
    const replacementA = await owner.retry(firstA.failure);
    if (replacementA.kind !== "failed")
      throw new Error("expected replacement failure");
    expect(replacementA.replaced).toBe(firstA.failure);
    expect(replacementA.failure).not.toBe(firstA.failure);
    expect(owner.failures()).toEqual([firstB.failure, replacementA.failure]);
    expect((await owner.retry(firstA.failure)).kind).toBe("detached");

    recoverA = true;
    const recovered = await owner.retry(replacementA.failure);
    expect(recovered).toMatchObject({
      kind: "loaded",
      recovered: replacementA.failure,
      remainingNewest: firstB.failure,
    });
    expect(owner.failures()).toEqual([firstB.failure]);
  });

  test("dispose suppresses a late root commit", async () => {
    let settle!: (response: Response) => void;
    const owner = createFileLocationOwner(
      () =>
        new Promise<Response>((resolve) => {
          settle = resolve;
        }),
    );
    const binding = owner.awaitRootBinding();
    await Promise.resolve();
    owner.dispose();
    settle(windowResponse("late", ""));

    const outcome = await binding;
    expect(outcome.kind).toBe("detached");
    expect(owner.accept(outcome)).toBe(false);
    expect(owner.publication).toBeUndefined();
    expect(owner.window("")).toBeUndefined();
  });

  test("a root bind requested after dispose settles detached", async () => {
    let requests = 0;
    const owner = createFileLocationOwner(() => {
      requests += 1;
      return Promise.resolve(windowResponse("unexpected", ""));
    });
    owner.dispose();

    const outcome = await owner.awaitRootBinding();
    expect(outcome.kind).toBe("detached");
    expect(owner.accept(outcome)).toBe(false);
    expect(requests).toBe(0);
  });

  test("invalidates produced outcomes after reset or dispose", async () => {
    const owner = createFileLocationOwner((url) => {
      const parent = parentFrom(url);
      if (parent === "conflict")
        return Promise.resolve(new Response(null, { status: 409 }));
      if (parent === "failed")
        return Promise.resolve(new Response(null, { status: 503 }));
      return Promise.resolve(windowResponse("published-1", parent));
    });
    await owner.loadWindow("", 0, false);
    const conflict = await owner.loadWindow("conflict", 0);
    const failure = await owner.loadWindow("failed", 0);
    owner.reset();
    expect(owner.accept(conflict)).toBe(false);
    expect(owner.accept(failure)).toBe(false);
    const currentFailure = await owner.loadWindow("failed", 0);
    owner.dispose();
    owner.dispose();
    expect(owner.accept(currentFailure)).toBe(false);
  });

  test("returns one outcome identity to all shared root-binding joiners", async () => {
    let settle!: (response: Response) => void;
    const owner = createFileLocationOwner(
      () =>
        new Promise<Response>((resolve) => {
          settle = resolve;
        }),
    );
    const first = owner.awaitRootBinding();
    const second = owner.awaitRootBinding();
    await Promise.resolve();
    settle(windowResponse("published-1", ""));

    const [firstOutcome, secondOutcome] = await Promise.all([first, second]);
    expect(firstOutcome).toBe(secondOutcome);
    expect(owner.accept(firstOutcome)).toBe(true);
    expect(owner.accept(secondOutcome)).toBe(true);
  });
});
