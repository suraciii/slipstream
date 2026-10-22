import { describe, expect, test } from "bun:test";
import { fetchFileLocationWindow } from "./file-locations.js";

const request = {
  parent: "shoot",
  start: 0,
  limit: 60,
};

const windowBody = {
  publication: "published-1",
  parent: "shoot",
  start: 0,
  limit: 60,
  total: 0,
  children: [],
};

describe("File Location window API", () => {
  test("sends the exact retained query for a parent and publication", async () => {
    let url = "";
    const result = await fetchFileLocationWindow(
      (input) => {
        url = input;
        return Promise.resolve(
          new Response(JSON.stringify(windowBody), { status: 200 }),
        );
      },
      { ...request, publication: "published-1" },
    );

    expect(url).toBe(
      "/api/file-locations?start=0&limit=60&parent=shoot&publication=published-1",
    );
    expect(result).toEqual({ kind: "loaded", window: windowBody });
  });

  test("keeps a 409 answer a publication conflict", async () => {
    const result = await fetchFileLocationWindow(
      () => Promise.resolve(new Response(null, { status: 409 })),
      request,
    );

    expect(result).toEqual({ kind: "publication-conflict" });
  });

  test("keeps a non-409 refusal a failed answer carrying its status", async () => {
    const result = await fetchFileLocationWindow(
      () => Promise.resolve(new Response(null, { status: 503 })),
      request,
    );

    expect(result).toEqual({ kind: "failed", status: 503 });
  });

  test("keeps a lost transport a failed answer with no status", async () => {
    const result = await fetchFileLocationWindow(
      () => Promise.reject(new Error("offline")),
      request,
    );

    expect(result).toEqual({ kind: "failed" });
  });

  test("reports a non-JSON answer as a failed malformed window, never a throw", async () => {
    const result = await fetchFileLocationWindow(
      () =>
        Promise.resolve(
          new Response("<html>proxy error</html>", { status: 200 }),
        ),
      request,
    );

    expect(result).toEqual({ kind: "failed", malformed: true });
  });

  test("reports an empty answer as a failed malformed window, never a throw", async () => {
    const result = await fetchFileLocationWindow(
      () => Promise.resolve(new Response(null, { status: 200 })),
      request,
    );

    expect(result).toEqual({ kind: "failed", malformed: true });
  });
});
