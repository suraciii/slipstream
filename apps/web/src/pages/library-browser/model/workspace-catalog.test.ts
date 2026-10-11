import { describe, expect, test } from "bun:test";
import { catalogMapping, createCatalogMutation } from "./workspace-catalog.js";

describe("workspace catalog request ownership", () => {
  test("unknown outcome replays exact captured request before refreshing", async () => {
    const calls: string[] = [];
    let refreshes = 0;
    const owner = createCatalogMutation(
      (_path, init) => {
        if (typeof init?.body !== "string")
          throw new Error("Expected string request body");
        calls.push(init.body);
        if (calls.length === 1)
          return Promise.reject(new Error("disconnected"));
        return Promise.resolve(
          Response.json({ outcome: "replayed", requestId: "receipt" }),
        );
      },
      "photo",
      () => {
        refreshes++;
        return Promise.resolve();
      },
    );
    const input = {
      ifState: "observed",
      input: { mapping: [{ entryId: "a", instanceId: null }] },
    };
    const disconnected = owner.submit({
      editId: "edit",
      operation: "styles apply",
      arguments: input,
    });
    expect(disconnected).rejects.toThrow();
    await disconnected.catch(() => undefined);
    input.ifState = "new-state";
    input.input.mapping[0]!.entryId = "changed";
    expect(refreshes).toBe(0);
    expect(() => owner.discard()).toThrow("Replay");
    const blocked = owner.submit({
      editId: "edit",
      operation: "styles apply",
      arguments: {},
    });
    expect(blocked).rejects.toThrow("Resolve");
    await blocked.catch(() => undefined);
    await owner.replay();
    expect(calls[1]).toBe(calls[0]);
    const replayed = JSON.parse(calls[1]!) as {
      arguments: { ifState: string };
    };
    expect(replayed.arguments.ifState).toBe("observed");
    expect(refreshes).toBe(1);
    expect(owner.state().pending).toBeUndefined();
  });
  test("state conflict retains draft and refreshes only after explicit new submission succeeds", async () => {
    let conflict = true,
      refreshes = 0;
    const owner = createCatalogMutation(
      () =>
        Promise.resolve(
          conflict
            ? Response.json(
                {
                  error: {
                    code: "state_conflict",
                    message: "Changed",
                    effect: "none",
                    details: {},
                  },
                },
                { status: 409 },
              )
            : Response.json({ outcome: "saved" }),
        ),
      "photo",
      () => {
        refreshes++;
        return Promise.resolve();
      },
    );
    const conflicted = owner.submit({
      editId: "edit",
      operation: "presets save",
      arguments: { ifState: "old", input: { parameterIds: ["exposure"] } },
    });
    expect(conflicted).rejects.toThrow("Changed");
    await conflicted.catch(() => undefined);
    expect(owner.state().pending?.arguments.input).toEqual({
      parameterIds: ["exposure"],
    });
    expect(refreshes).toBe(0);
    owner.discard();
    conflict = false;
    await owner.submit({
      editId: "edit",
      operation: "presets save",
      arguments: {
        ifState: "inspected",
        input: { parameterIds: ["exposure"] },
      },
    });
    expect(refreshes).toBe(1);
  });
  test("global catalog requests use global authority and preserve transfer handle", async () => {
    let path = "",
      body: Record<string, unknown> = {};
    const owner = createCatalogMutation(
      (url, init) => {
        path =
          typeof url === "string"
            ? url
            : url instanceof URL
              ? url.href
              : url.url;
        if (typeof init?.body !== "string")
          throw new Error("Expected string request body");
        body = JSON.parse(init.body) as Record<string, unknown>;
        return Promise.resolve(Response.json({ outcome: "created" }));
      },
      "photo",
      () => Promise.resolve(),
    );
    await owner.submit({
      global: true,
      editId: null,
      operation: "resources import",
      arguments: { kind: "lut", file: { transferId: "uploaded" } },
    });
    expect(path).toBe("/api/darktable/commands");
    expect(body.editId).toBeNull();
    expect((body.arguments as Record<string, unknown>).file).toEqual({
      transferId: "uploaded",
    });
  });
  test("a response without a confirmed mutation outcome cannot refresh", async () => {
    let refreshes = 0;
    const owner = createCatalogMutation(
      () =>
        Promise.resolve(
          Response.json({ requestId: "accepted", status: "running" }),
        ),
      "photo",
      () => {
        refreshes++;
        return Promise.resolve();
      },
    );
    const unconfirmed = owner.submit({
      editId: "edit",
      operation: "styles apply",
      arguments: { ifState: "observed" },
    });
    expect(unconfirmed).rejects.toThrow("No confirmed");
    await unconfirmed.catch(() => undefined);
    expect(refreshes).toBe(0);
    expect(owner.state().pending).toBeDefined();
  });
});

describe("explicit style entry mapping", () => {
  const entries = [
    { entryId: "entry-a", moduleId: "exposure" },
    { entryId: "entry-b", moduleId: "exposure" },
  ];
  test("every returned entry requires a choice; names and array position grant no mapping", () => {
    expect(() => catalogMapping(entries, {})).toThrow("entry-a");
    expect(() => catalogMapping(entries, { "entry-a": "target" })).toThrow(
      "entry-b",
    );
    expect(
      catalogMapping(entries, { "entry-a": "target", "entry-b": "@new" }),
    ).toEqual([
      { entryId: "entry-a", instanceId: "target" },
      { entryId: "entry-b", instanceId: null },
    ]);
  });
  test("existing target cannot receive duplicate entries; multiple explicit new choices are valid", () => {
    expect(() =>
      catalogMapping(entries, { "entry-a": "same", "entry-b": "same" }),
    ).toThrow("more than one");
    expect(
      catalogMapping(entries, { "entry-a": "@new", "entry-b": "@new" }).every(
        (item) => item.instanceId === null,
      ),
    ).toBe(true);
  });
});
