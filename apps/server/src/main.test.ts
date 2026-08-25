import { describe, expect, it, vi } from "vitest";

import { defaultServerOptions, startServer } from "./main.js";

describe("server placeholder entry point", () => {
  it("defaults its future endpoint to loopback", () => {
    expect(defaultServerOptions).toEqual({ host: "127.0.0.1", port: 3000 });
  });

  it("reports explicit placeholder configuration when called", () => {
    const log = vi.spyOn(console, "log").mockImplementation(() => undefined);

    startServer({ host: "127.0.0.1", port: 4000 });

    expect(log).toHaveBeenCalledWith(
      "Slipstream server placeholder configured for http://127.0.0.1:4000",
    );
    log.mockRestore();
  });
});
