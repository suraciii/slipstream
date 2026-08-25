import { describe, expect, it } from "vitest";

import { defaultNetworkOptions, startupOptions } from "./main.js";

describe("server startup configuration", () => {
  it("defaults to loopback and the canonical database basename", () => {
    expect(
      startupOptions({
        SLIPSTREAM_LIBRARY_ROOT: "/photos",
        SLIPSTREAM_STATE_DIRECTORY: "/state",
        SLIPSTREAM_CACHE_DIRECTORY: "/cache",
      }),
    ).toEqual({
      libraryRoot: "/photos",
      stateDirectory: "/state",
      cacheDirectory: "/cache",
      databaseBasename: "library.sqlite",
      ...defaultNetworkOptions,
    });
  });

  it("allows explicit LAN binding and rejects incomplete or relative paths", () => {
    expect(
      startupOptions({
        SLIPSTREAM_LIBRARY_ROOT: "/photos",
        SLIPSTREAM_STATE_DIRECTORY: "/state",
        SLIPSTREAM_CACHE_DIRECTORY: "/cache",
        SLIPSTREAM_HOST: "0.0.0.0",
        SLIPSTREAM_PORT: "8080",
      }).host,
    ).toBe("0.0.0.0");
    expect(() => startupOptions({})).toThrow("SLIPSTREAM_LIBRARY_ROOT");
    expect(() =>
      startupOptions({
        SLIPSTREAM_LIBRARY_ROOT: "photos",
        SLIPSTREAM_STATE_DIRECTORY: "/state",
        SLIPSTREAM_CACHE_DIRECTORY: "/cache",
      }),
    ).toThrow("absolute");
  });
});
