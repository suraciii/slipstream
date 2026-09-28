import { describe, expect, test } from "bun:test";
import type { BrowserFetch } from "../model/access-session.js";
import {
  createDevelopmentProxy,
  fetchDevelopmentProxy,
  parseDevelopmentProxyStatus,
  removeDevelopmentProxy,
} from "./development-proxy.js";

const digest = "a".repeat(64);
const proxy = {
  proxyId: digest,
  sourceRevision: "rev-1",
  sourceSize: 1200,
  sourceSha256: digest,
  sourceProfileId: "camera-profile",
  pipelineVersion: "proxy-v1",
  longEdge: 2560,
  width: 2560,
  height: 1707,
  qualityLimit: "2560-long-edge",
  byteLength: 12345,
  sha256: digest,
  createdAt: 1700000000,
};

const response = (body: unknown, status = 200): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

describe("development proxy wire contract", () => {
  test("reads current facts and rejects a proxy identity with the wrong Photo", () => {
    const status = parseDevelopmentProxyStatus(
      { photoId: "photo-1", state: "current", proxy, failure: null },
      "photo-1",
    );
    expect(status?.proxy?.qualityLimit).toBe("2560-long-edge");
    expect(status?.proxy?.sourceRevision).toBe("rev-1");
    expect(
      parseDevelopmentProxyStatus(
        { photoId: "photo-2", state: "current", proxy },
        "photo-1",
      ),
    ).toBeUndefined();
  });

  test("POST carries the guarded source revision and accepts an admitted build", async () => {
    let request: RequestInit | undefined;
    const fetcher: BrowserFetch = (_url, init) => {
      request = init;
      return Promise.resolve(
        response(
          { photoId: "photo-1", state: "building", proxy: null, failure: null },
          202,
        ),
      );
    };
    const result = await createDevelopmentProxy(fetcher, "photo-1", "rev-1");
    expect(result.kind).toBe("ok");
    expect(request?.method).toBe("POST");
    expect(JSON.parse(request?.body as string)).toEqual({
      expectedSourceRevision: "rev-1",
    });
  });

  test("Original removal is idempotent and status reads preserve absent state", async () => {
    const fetcher: BrowserFetch = (url, init) =>
      Promise.resolve(
        (url instanceof Request ? url.url : url.toString()).endsWith(
          "development-proxy",
        ) && init?.method === "DELETE"
          ? response({
              photoId: "photo-1",
              state: "absent",
              proxy: null,
              removed: false,
            })
          : response({
              photoId: "photo-1",
              state: "absent",
              proxy: null,
              failure: null,
            }),
      );
    expect((await removeDevelopmentProxy(fetcher, "photo-1")).kind).toBe("ok");
    expect((await fetchDevelopmentProxy(fetcher, "photo-1")).kind).toBe("ok");
  });
});
