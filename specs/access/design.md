# Instance Access Architecture

[Instance Access](spec.md) owns observable behavior and session lifetime. This contract owns authentication enforcement, credential lifecycle, and transport boundaries.

## Model and Ownership

An Access Token is an instance-wide bearer credential. A credential generation identifies one issuance, not a person. A Browser Session is an independently revocable random credential derived from one active generation.

Neither token nor session changes Photo or Library identity.

The HTTP application boundary owns verification, session admission, CSRF, request limits, and safe error mapping. Domain operations receive admitted calls; Photo models MUST NOT contain cookies, token digests, or authentication-framework types.

A persistence owner MUST serialize credential and session changes. Local administration owns generation, rotation, and revocation and MUST use the same exclusive state ownership as the service.

Offline administration MUST require a stopped server and refuse while the server owns state. Normal startup MUST NOT disclose or generate credentials automatically.

## Credential and Session State

Local administration MUST generate 32 operating-system CSPRNG bytes for each Access Token and encode them as unpadded base64url. It MUST present the token once and MUST NOT accept short human-selected replacements.

The store MUST retain only the token's SHA-256 digest and an independently generated generation identifier. Digest comparisons MUST be constant time. Fast hashing is suitable because the generated secret has high entropy rather than a human-selected password.

Credentials MUST NOT be logged or placed in process arguments. Authentication state MUST be protected under the admitted state directory.

Each Browser Session MUST use a separate 32-byte CSPRNG secret. The store MUST retain only:

- Its digest.
- Its credential generation.
- Its creation time and absolute expiry.
- A session-bound random 32-byte CSRF secret.

The browser MUST receive the session secret in a host-only cookie without `Domain`. The cookie attributes MUST include `HttpOnly`, `SameSite=Lax`, and `Path=/`. Its maximum age MUST NOT exceed server expiry.

The bounded `access_origins` set MUST contain the configured public origin. Deployment tooling MAY provide the internal `SLIPSTREAM_ACCESS_ORIGINS` JSON array; it is not user configuration.

Each origin MUST be a canonical HTTP or HTTPS origin without path, query, fragment, credentials, or whitespace. Input MUST be canonicalized and validated. Invalid, oversized, duplicate, or empty input MUST fail startup; the public origin remains mandatory.

Without the internal array, the server MUST derive HTTP origins from the configured listener:

| Listener    | Derived origins                                 |
| ----------- | ----------------------------------------------- |
| Loopback    | `localhost` and `127.0.0.1`                     |
| Concrete IP | The configured IP                               |
| Wildcard    | Only reliably enumerated concrete interface IPs |

The selected origin is request-scoped:

```text
if Origin is present:
    select that exact allowed origin
else:
    select the allowed origin matching Host or absolute request URI authority
    if that authority maps to both HTTP and HTTPS, select HTTPS
if Host is present:
    require its authority to match the selected origin
else:
    accept missing Host only for synthetic or internal requests
```

A missing Host MUST NOT grant a new network identity. Forwarded headers MUST NOT participate in origin selection or weaken cookie attributes.

| Selected origin | Cookie profile                    |
| --------------- | --------------------------------- |
| HTTPS           | `__Host-slipstream` with `Secure` |
| HTTP            | `slipstream` without `Secure`     |

The selected profile MUST control cookie issuance, admission, and expiration. The same session record MAY be presented through either explicitly allowed transport, but its cookie MUST be admitted only under the selected profile.

Origin, Host, cookie profile, and CSRF checks MUST apply consistently where required by [Browser and Machine Admission](#browser-and-machine-admission) and the [Authentication Wire Contract](#authentication-wire-contract). Bearer requests MUST retain their no-cookie behavior and MUST reject a mismatched present Origin or Host.

CSRF material is not an authentication credential. It MUST remain in protected session storage, be exposed only to the authenticated browser through status, and rotate when a new session is created.

Restart MUST preserve valid sessions. Expiration and generation checks are server-owned; status and view revalidation MUST NOT extend the fixed lifetime.

The Access Token MUST NOT be stored in the session cookie. Neither secret MUST be persisted in localStorage, sessionStorage, IndexedDB, or Cache Storage.

Rotation MUST atomically replace the token digest and generation and invalidate all sessions. Revocation MUST remove active access and invalidate all sessions.

A session exchange concurrent with rotation MUST either commit under the current generation or fail. No old-generation session can authorize a request afterward.

Every request admission MUST check the current generation and expiry. Revocation orders with admission, not completion; already-admitted work MAY settle. An authentication transaction MUST NOT be held throughout image processing or a mutation.

Storage failure MUST deny admission. An absent credential on an otherwise valid new store permits only setup-required public surfaces. Invalid or unreadable authentication state MUST fail startup and MUST NOT be interpreted as a fresh store.

Backup MUST include authentication state. [Provisioning and Recovery](spec.md#provisioning-and-recovery) owns restored-credential rotation and rollback isolation.

## Browser and Machine Admission

The browser MUST send the Access Token once in a POST body to establish a session over an allowed HTTP or HTTPS origin. The exchange MUST enforce origin, bounded body size, rate limiting, and logging redaction.

Each successful exchange MUST issue a fresh session identifier. Session status MUST reveal only current access state and expiry, not Library details.

Logout MUST be a protected state-changing request that revokes the presented session and expires its cookie. It MUST be idempotent for an already-invalid session after request-origin checks.

The CLI MUST send `Authorization: Bearer` with the Access Token. The server MUST validate its digest and active generation before the same private route dispatch. Clients MUST NOT follow redirects or transmit credentials to a different origin.

A request containing both cookie and Authorization credentials MUST return 400 `invalid_request`; it MUST NOT prefer either or fall back after a failed credential. Bearer responses MUST use the HTTP authentication challenge and invalid-token semantics of [RFC 6750](https://www.rfc-editor.org/rfc/rfc6750.html). Local issuance is not an OAuth flow.

The router MUST be protected by default. Its explicit public exceptions are:

- The nonprivate shell and versioned static assets.
- Access establishment and status.
- Minimal `GET` and `HEAD /healthz`.

The following surfaces MUST be private:

- Library status and CLI discovery.
- Browse cursors and derivatives.
- Metadata and recovery.
- Editing and processing jobs.
- Export downloads.

Public readiness MUST NOT disclose access state or Library paths.

Access MUST be checked before:

- Resource existence or file access.
- HEAD or range processing.
- ETag comparison or 304 responses.

Existing tokens in browse URLs are not access credentials. No hidden alternate token transport is permitted.

Cookie-authenticated mutations MUST require a session-bound CSRF token and exact allowed-origin validation. Establishment MUST check the exact origin before accepting a credential.

State-changing browser routes MUST reject missing or mismatched Origin. Same-origin browser GET session status MAY provide CSRF material in a no-store response.

Explicit Bearer requests do not use ambient cookies and need no cookie CSRF token. Browser requests with an Origin MUST match an allowed origin. Permissive CORS MUST NOT be enabled.

Origin checks MUST match a parsed scheme, host, and port, never a substring or forwarded client identity. CSRF token comparisons MUST be constant time and bound to the presented session.

Invalid or expired credentials MUST produce 401 before private processing. Rejected origin or CSRF MUST produce 403, malformed input 400, and throttling 429 with Retry-After. API or image failures MUST NOT redirect to successful login HTML.

[CLI Reference](../command-line/reference.md) owns CLI result mapping. [Deployment](../deployment/spec.md#access-administration) owns operator command syntax.

## Bounded Operation

The exchange body MUST be bounded to 256 bytes. The token MUST have exactly 43 base64url characters with canonical encoding.

Exchange limits count successful and failed attempts:

| Scope               | Limit per rolling 60 seconds |
| ------------------- | ---------------------------- |
| Instance            | 20                           |
| Direct peer address | 5                            |

Forwarded addresses MUST NOT be trusted. A proxy shares its direct peer budget, a conservative boundary for one Photographer.

Rate windows MUST use a monotonic clock; session expiry MUST use persisted wall-clock time. Rate state MAY reset on process restart without revoking credentials.

The server MUST keep at most 1,024 peer buckets and reclaim expired buckets before insertion. It MUST reject new peers when the set is full; an unbounded peer map or waiting queue is prohibited.

The server MUST limit simultaneous exchanges to four and reject excess work with 429 and `Retry-After: 1`. Rejected attempts MUST NOT extend the rate window.

Rate-window exhaustion MUST return the positive rounded-up seconds until the relevant oldest counted attempt leaves its window. Bucket capacity MUST return `Retry-After: 60`.

The server MUST reclaim expired sessions before admission and admit at most 32 live Browser Sessions without silent eviction. Capacity MUST return 429, `session_capacity`, and Retry-After until the earliest session expiry.

The browser MUST show that signing out a session or operator rotation can free capacity earlier. Rotation provides recovery without permanent lockout.

HTTP header, body, and private-request concurrency limits remain in force. Credential failures on private routes MUST NOT create sessions or per-token state.

## Authentication Wire Contract

These endpoints MUST accept only their specified methods and reject others with 405. All responses MUST use no-store.

Request objects are closed. Duplicate keys, trailing content, or malformed JSON MUST produce 400. Errors MUST use `{ "error": CODE }` without credentials or internal diagnostics.

| Endpoint                     | Contract                                                                                                                                                                                                                                                                                      |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `POST /api/access/session`   | Accept JSON `{ "token": TOKEN }` with `application/json` and matching Origin. After durable session creation, return 204 with the cookie. Invalid credentials return 401 `invalid_token`; unconfigured access returns 503 `access_unconfigured`; invalid input returns 400 `invalid_request`. |
| `GET /api/access/session`    | A valid cookie returns 200 JSON `{ "authenticated": true, "expiresAt": RFC3339_UTC, "csrfToken": SECRET }`. Otherwise return 200 `{ "authenticated": false, "configured": BOOLEAN }`, where configured means an active token exists. An Authorization header returns 400 `invalid_request`.   |
| `DELETE /api/access/session` | A valid cookie, exact Origin, and `X-CSRF-Token` revoke the session and return 204 with an expired cookie. Without a valid session, exact Origin suffices for idempotent 204 and cookie clearing. Wrong Origin or invalid CSRF for a valid session returns 403 `access_denied`.               |

A lost establishment response MAY consume one session slot; the browser MUST NOT automatically retry establishment.

The status setup signal is intentionally public and MUST reveal no Library facts. An invalid cookie MUST be expired in the status response. Cross-origin response sharing MUST NOT be allowed.

The server MUST use `rate_limited` and `session_capacity` for the corresponding 429 errors. Authentication storage failure MUST return 503 `access_unavailable`.

Other private routes MUST return 401 `authentication_required` for missing or invalid access and 403 `access_denied` for failed origin or CSRF. These early rejections confirm no operation was admitted.

Protected-route 401 responses MUST include `WWW-Authenticate: Bearer realm="slipstream"`. They MUST add `error="invalid_token"` when a Bearer credential was supplied.

The existing CLI envelope MUST map boundary errors as specified in [CLI Reference](../command-line/reference.md) before ordinary result decoding.

The browser MUST fetch status at startup, on return to a displayed tab, and before restoring history. Status checks MUST share one in-flight request and one session transition owner.

Request-generation ownership MUST prevent stale status or image completion from restoring access after local logout. Image failure checks MUST preserve a valid view when the server is unreachable.

The displayed view MAY remain visible after remote revocation until rejection is observed; private requests still rely on server admission. Initial entry, known expiry, and restored history without an available view MUST remain closed until verified. [Session Loss and Disconnection](spec.md#session-loss-and-disconnection) owns the visible transitions.

Browse owns lease release, including disposal. The page MUST supply its explicit release callback with authenticated transport before closing the access epoch.

Authentication transport owns credentials, CSRF, and redirect policy without recognizing Browse routes. Ordinary requests remain subject to view admission; cleanup MUST NOT grant a general request-admission bypass.

## Private Content and Transport

The following private responses, including errors, MUST use `Cache-Control: no-store`:

- APIs and metadata.
- Mutations and editing.
- Downloads.

A successfully authenticated immutable Camera Preview derivative MAY instead use:

```text
Cache-Control: private, max-age=3600, must-revalidate
Vary: Cookie, Authorization
ETag: "<cacheKey>"
```

This derivative policy applies to successful `200`, `304`, and `HEAD` responses. Authentication and currentness checks MUST precede file access, range handling, ETag comparison, or a 304 response.

Server-side derivative identity and cache reuse MUST remain independent of browser caching. Private service-worker caches MUST NOT be added; static asset caching remains separate from private response policy.

Access loss MUST detach in-flight image sources and private views and reject stale asynchronous completions.

Old publicly cacheable image paths MUST NOT serve private content or redirect to a protected image. New pages MUST use the protected namespace. Operator-controlled public caches MUST be purged before exposure.

The operator configures one canonical public HTTP or HTTPS origin without credentials, a non-root path, query, or fragment. An omitted public origin resolves to `http://localhost:<port>` and MUST NOT change the listener's loopback default.

Network exposure MUST require an explicit listener setting. Existing HTTPS deployments MAY terminate TLS at a proxy. Forwarded headers MUST NOT grant access or select cookie attributes.

Clients MUST verify HTTPS certificates and MUST NOT follow redirects with credentials or silently change scheme. [CLI and Deployment](spec.md#cli-and-deployment) owns HTTP warnings and supported browser operations.

Request identifiers MUST use `crypto.getRandomValues` CSPRNG bytes available over HTTP with UUID version and variant bits. If CSPRNG is unavailable, they MUST fail rather than substitute weak randomness.

Download SHA-256 verification MUST use an established library and reject altered bytes even when secure-context Web Crypto is unavailable. Installation requires a browser secure context.

Opaque sessions and direct CLI Bearer access preserve native browser image loading, individual browser logout, and immediate generation-based revocation without maintaining identities. Built-in TLS, certificate management, authentication plugins, role engines, and an OAuth authorization server are outside this boundary.

## References

- [RFC 6750](https://www.rfc-editor.org/rfc/rfc6750.html): Bearer transport and challenge behavior.
- [OWASP Session Management](https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html): random identifiers, cookie controls, expiry, and server-side invalidation.
