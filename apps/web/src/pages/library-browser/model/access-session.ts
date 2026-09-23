export type BrowserSession = Readonly<{
  expiresAt: number;
  csrfToken: string;
}>;

export type AccessStatus =
  | Readonly<{ kind: "authenticated"; session: BrowserSession }>
  | Readonly<{ kind: "anonymous"; configured: boolean }>
  | Readonly<{ kind: "unavailable" }>;

export type TokenExchange =
  | Readonly<{ kind: "established" }>
  | Readonly<{ kind: "invalid-token" }>
  | Readonly<{ kind: "rate-limited"; retryAfterSeconds: number }>
  | Readonly<{ kind: "session-capacity"; retryAfterSeconds: number }>
  | Readonly<{ kind: "unconfigured" }>
  | Readonly<{ kind: "unavailable" }>
  | Readonly<{ kind: "uncertain" }>;

export type LogoutOutcome = "confirmed" | "unconfirmed";

export type BrowserFetch = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

const ACCESS_SESSION_PATH = "/api/access/session";
const MUTATING_METHODS = new Set(["DELETE", "PATCH", "POST", "PUT"]);

function currentOrigin(): string {
  return typeof window === "undefined"
    ? "https://slipstream.invalid"
    : window.location.origin;
}

function requestUrl(input: RequestInfo | URL): string {
  return typeof input === "string"
    ? input
    : input instanceof URL
      ? input.href
      : input.url;
}

function isSameOriginRequest(input: RequestInfo | URL): boolean {
  const origin = currentOrigin();
  try {
    return new URL(requestUrl(input), origin).origin === origin;
  } catch {
    return false;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isCsrfToken(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9_-]{43}$/.test(value);
}

function retryAfterSeconds(response: Response): number {
  const retryAfter = Number(response.headers.get("Retry-After"));
  return Number.isFinite(retryAfter) && retryAfter > 0
    ? Math.ceil(retryAfter)
    : 1;
}

async function responseErrorCode(
  response: Response,
): Promise<string | undefined> {
  try {
    const body: unknown = await response.json();
    return isRecord(body) && typeof body.error === "string"
      ? body.error
      : undefined;
  } catch {
    return undefined;
  }
}

export async function readAccessStatus(
  fetcher: BrowserFetch,
): Promise<AccessStatus> {
  let response: Response;
  try {
    response = await fetcher(ACCESS_SESSION_PATH, {
      method: "GET",
      credentials: "same-origin",
      cache: "no-store",
      redirect: "error",
    });
  } catch {
    return { kind: "unavailable" };
  }

  if (!response.ok) return { kind: "unavailable" };

  let value: unknown;
  try {
    value = await response.json();
  } catch {
    return { kind: "unavailable" };
  }

  if (!isRecord(value) || typeof value.authenticated !== "boolean")
    return { kind: "unavailable" };

  if (value.authenticated) {
    if (typeof value.expiresAt !== "string" || !isCsrfToken(value.csrfToken))
      return { kind: "unavailable" };
    const expiresAt = Date.parse(value.expiresAt);
    if (!Number.isFinite(expiresAt)) return { kind: "unavailable" };
    return {
      kind: "authenticated",
      session: Object.freeze({ expiresAt, csrfToken: value.csrfToken }),
    };
  }

  if (typeof value.configured !== "boolean") return { kind: "unavailable" };
  return { kind: "anonymous", configured: value.configured };
}

export async function exchangeAccessToken(
  fetcher: BrowserFetch,
  token: string,
): Promise<TokenExchange> {
  let response: Response;
  try {
    response = await fetcher(ACCESS_SESSION_PATH, {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      redirect: "error",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token }),
    });
  } catch {
    return { kind: "uncertain" };
  }

  if (response.status === 204) return { kind: "established" };
  if (response.status === 401) return { kind: "invalid-token" };
  if (response.status === 429) {
    const retryAfter = retryAfterSeconds(response);
    return (await responseErrorCode(response)) === "session_capacity"
      ? { kind: "session-capacity", retryAfterSeconds: retryAfter }
      : { kind: "rate-limited", retryAfterSeconds: retryAfter };
  }
  if (response.status === 503) {
    let body: unknown;
    try {
      body = await response.json();
    } catch {
      return { kind: "unavailable" };
    }
    return isRecord(body) && body.error === "access_unconfigured"
      ? { kind: "unconfigured" }
      : { kind: "unavailable" };
  }
  return { kind: "unavailable" };
}

export async function revokeBrowserSession(
  fetcher: BrowserFetch,
  csrfToken: string,
): Promise<LogoutOutcome> {
  try {
    const response = await fetcher(ACCESS_SESSION_PATH, {
      method: "DELETE",
      credentials: "same-origin",
      cache: "no-store",
      redirect: "error",
      headers: { "X-CSRF-Token": csrfToken },
    });
    return response.status === 204 ? "confirmed" : "unconfirmed";
  } catch {
    return "unconfirmed";
  }
}

/// Adds cookie credentials and the session-bound CSRF token to private API
/// requests. A rejected request is reported once and is never replayed.
export function createPrivateFetcher(
  fetcher: BrowserFetch,
  getCsrfToken: () => string | undefined,
  onUnauthorized: () => void,
  beforeRequest: () => Promise<boolean>,
): BrowserFetch {
  const authenticatedFetch = createAuthenticatedFetcher(
    fetcher,
    getCsrfToken,
    onUnauthorized,
  );
  return async (input, init) => {
    if (!isSameOriginRequest(input))
      throw new Error("private requests must use the current origin");

    if (!(await beforeRequest())) throw new Error("browser session is closed");
    return authenticatedFetch(input, init);
  };
}

// The authenticated transport owns credential policy, independently of view
// admission. Only the composition root supplies it to lease cleanup.
export function createAuthenticatedFetcher(
  fetcher: BrowserFetch,
  getCsrfToken: () => string | undefined,
  onUnauthorized: () => void,
): BrowserFetch {
  return async (input, init) => {
    if (!isSameOriginRequest(input))
      throw new Error("private requests must use the current origin");
    const csrfToken = getCsrfToken();
    if (!csrfToken) throw new Error("browser session is closed");
    const method = (
      init?.method ?? (input instanceof Request ? input.method : "GET")
    ).toUpperCase();
    const inputHeaders =
      typeof Request !== "undefined" && input instanceof Request
        ? input.headers
        : undefined;
    const headers = new Headers(inputHeaders);
    new Headers(init?.headers).forEach((value, name) =>
      headers.set(name, value),
    );

    if (MUTATING_METHODS.has(method)) {
      headers.set("X-CSRF-Token", csrfToken);
    }

    const response = await fetcher(input, {
      ...init,
      method,
      credentials: "same-origin",
      cache: "no-store",
      redirect: "error",
      headers,
    });
    if (response.status === 401) onUnauthorized();
    return response;
  };
}
