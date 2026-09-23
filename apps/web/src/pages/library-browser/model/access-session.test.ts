import { describe, expect, test } from "bun:test";
import {
  createPrivateFetcher,
  exchangeAccessToken,
  readAccessStatus,
  revokeBrowserSession,
  type BrowserFetch,
} from "./access-session.js";

const csrfToken = "A".repeat(43);

function fetcher(
  implementation: (
    input: RequestInfo | URL,
    init?: RequestInit,
  ) => Promise<Response>,
): BrowserFetch {
  return implementation as BrowserFetch;
}

function inputUrl(input: RequestInfo | URL | undefined): string | undefined {
  if (typeof input === "string") return input;
  if (input instanceof URL) return input.href;
  return input?.url;
}

describe("browser access session API", () => {
  test("reads only the minimal anonymous setup state or a complete live session", async () => {
    let anonymousRequest: RequestInit | undefined;
    const anonymous = await readAccessStatus(
      fetcher((_input, init) => {
        anonymousRequest = init;
        return Promise.resolve(
          Response.json({ authenticated: false, configured: true }),
        );
      }),
    );
    expect(anonymous).toEqual({ kind: "anonymous", configured: true });
    expect(anonymousRequest?.method).toBe("GET");
    expect(anonymousRequest?.redirect).toBe("error");

    const authenticated = await readAccessStatus(
      fetcher(() =>
        Promise.resolve(
          Response.json({
            authenticated: true,
            expiresAt: "2026-10-01T12:00:00Z",
            csrfToken,
          }),
        ),
      ),
    );
    expect(authenticated).toEqual({
      kind: "authenticated",
      session: {
        expiresAt: Date.parse("2026-10-01T12:00:00Z"),
        csrfToken,
      },
    });

    const malformed = await readAccessStatus(
      fetcher(() =>
        Promise.resolve(
          Response.json({
            authenticated: true,
            expiresAt: "tomorrow",
            csrfToken: "short",
          }),
        ),
      ),
    );
    expect(malformed).toEqual({ kind: "unavailable" });
  });

  test("sends the token once in the same-origin exchange body", async () => {
    const calls: Array<{
      input: RequestInfo | URL;
      init?: RequestInit | undefined;
    }> = [];
    const outcome = await exchangeAccessToken(
      fetcher((input, init) => {
        calls.push({ input, init });
        return Promise.resolve(new Response(null, { status: 204 }));
      }),
      "private-token",
    );

    expect(outcome).toEqual({ kind: "established" });
    expect(calls).toHaveLength(1);
    expect(inputUrl(calls[0]?.input)).toBe("/api/access/session");
    expect(calls[0]?.init?.method).toBe("POST");
    expect(calls[0]?.init?.credentials).toBe("same-origin");
    expect(calls[0]?.init?.redirect).toBe("error");
    expect(calls[0]?.init?.body).toBe(
      JSON.stringify({ token: "private-token" }),
    );
    expect(new Headers(calls[0]?.init?.headers).has("Authorization")).toBe(
      false,
    );
  });

  test("distinguishes a rejected token and a bounded retry delay", async () => {
    expect(
      await exchangeAccessToken(
        fetcher(() => Promise.resolve(new Response(null, { status: 401 }))),
        "wrong",
      ),
    ).toEqual({ kind: "invalid-token" });

    expect(
      await exchangeAccessToken(
        fetcher(() =>
          Promise.resolve(
            Response.json(
              { error: "rate_limited" },
              {
                status: 429,
                headers: { "Retry-After": "17" },
              },
            ),
          ),
        ),
        "token",
      ),
    ).toEqual({ kind: "rate-limited", retryAfterSeconds: 17 });

    expect(
      await exchangeAccessToken(
        fetcher(() =>
          Promise.resolve(
            Response.json(
              { error: "session_capacity" },
              {
                status: 429,
                headers: { "Retry-After": "86400" },
              },
            ),
          ),
        ),
        "token",
      ),
    ).toEqual({
      kind: "session-capacity",
      retryAfterSeconds: 86_400,
    });
  });

  test("sends logout CSRF and accepts only the specified 204 confirmation", async () => {
    const calls: Array<{
      input: RequestInfo | URL;
      init?: RequestInit | undefined;
    }> = [];
    const outcome = await revokeBrowserSession(
      fetcher((input, init) => {
        calls.push({ input, init });
        return Promise.resolve(new Response(null, { status: 204 }));
      }),
      csrfToken,
    );
    expect(outcome).toBe("confirmed");
    expect(calls).toHaveLength(1);
    expect(inputUrl(calls[0]?.input)).toBe("/api/access/session");
    expect(calls[0]?.init?.method).toBe("DELETE");
    expect(calls[0]?.init?.credentials).toBe("same-origin");
    expect(calls[0]?.init?.redirect).toBe("error");
    expect(new Headers(calls[0]?.init?.headers).get("X-CSRF-Token")).toBe(
      csrfToken,
    );
  });
});

describe("private browser fetch", () => {
  test("adds CSRF only to mutations and never replays a rejected request", async () => {
    const calls: Array<{
      input: RequestInfo | URL;
      init?: RequestInit | undefined;
    }> = [];
    let unauthorized = 0;
    const privateFetch = createPrivateFetcher(
      fetcher((input, init) => {
        calls.push({ input, init });
        return Promise.resolve(new Response(null, { status: 401 }));
      }),
      () => csrfToken,
      () => unauthorized++,
      () => Promise.resolve(true),
    );

    const response = await privateFetch("/api/decision", { method: "POST" });
    expect(response.status).toBe(401);
    expect(calls).toHaveLength(1);
    expect(unauthorized).toBe(1);
    expect(calls[0]?.init?.credentials).toBe("same-origin");
    expect(calls[0]?.init?.cache).toBe("no-store");
    expect(calls[0]?.init?.redirect).toBe("error");
    expect(new Headers(calls[0]?.init?.headers).get("X-CSRF-Token")).toBe(
      csrfToken,
    );

    await privateFetch("/api/status");
    expect(calls).toHaveLength(2);
    expect(new Headers(calls[1]?.init?.headers).has("X-CSRF-Token")).toBe(
      false,
    );
    expect(calls[1]?.init?.redirect).toBe("error");
  });

  test("refuses cross-origin private inputs before sending credentials", async () => {
    let requests = 0;
    let validations = 0;
    const privateFetch = createPrivateFetcher(
      fetcher(() => {
        requests++;
        return Promise.resolve(new Response(null, { status: 200 }));
      }),
      () => csrfToken,
      () => {},
      () => {
        validations++;
        return Promise.resolve(true);
      },
    );

    await expect(
      privateFetch("https://elsewhere.invalid/api/private", {
        method: "POST",
      }),
    ).rejects.toThrow("private requests must use the current origin");
    expect(requests).toBe(0);
    expect(validations).toBe(0);
  });

  test("does not send a request while session revalidation is closed", async () => {
    let requests = 0;
    const privateFetch = createPrivateFetcher(
      fetcher(() => {
        requests++;
        return Promise.resolve(new Response(null, { status: 200 }));
      }),
      () => csrfToken,
      () => {},
      () => Promise.resolve(false),
    );

    let closed = false;
    try {
      await privateFetch("/api/private");
    } catch (error) {
      closed =
        error instanceof Error && error.message === "browser session is closed";
    }
    expect(closed).toBe(true);
    expect(requests).toBe(0);
  });

  test("admits only the keepalive Browse release as teardown cleanup", async () => {
    const calls: Array<{
      input: RequestInfo | URL;
      init?: RequestInit | undefined;
    }> = [];
    const privateFetch = createPrivateFetcher(
      fetcher((input, init) => {
        calls.push({ input, init });
        return Promise.resolve(new Response(null, { status: 204 }));
      }),
      () => csrfToken,
      () => {},
      () => Promise.resolve(false),
    );

    const released = await privateFetch("/api/browse/browse-token", {
      method: "DELETE",
      keepalive: true,
    });
    expect(released.status).toBe(204);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.init?.method).toBe("DELETE");
    expect(calls[0]?.init?.keepalive).toBe(true);
    expect(new Headers(calls[0]?.init?.headers).get("X-CSRF-Token")).toBe(
      csrfToken,
    );

    for (const [input, init] of [
      ["/api/browse/browse-token", { method: "DELETE" }],
      ["/api/other", { method: "DELETE", keepalive: true }],
      ["/api/decision", { method: "POST", keepalive: true }],
    ] as const) {
      let closed = false;
      try {
        await privateFetch(input, init);
      } catch (error) {
        closed =
          error instanceof Error &&
          error.message === "browser session is closed";
      }
      expect(closed).toBe(true);
    }
    expect(calls).toHaveLength(1);
  });
});
