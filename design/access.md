# Instance Access Architecture

Public browser access must protect every Library operation without introducing
accounts or an identity provider. The existing CLI also needs authenticated
access. [Instance Access](../docs/access.md) owns observable behavior and session
lifetime; this document owns enforcement and lifecycle ordering.

## Model and Ownership

An Access Token is an instance-wide bearer credential. A credential generation
identifies one issuance, not a person. A Browser Session is an independently
revocable random credential derived from one active generation. Neither token
nor session changes Photo or Library identity.

The HTTP application boundary owns verification, session admission, CSRF,
request limits, and safe error mapping. A persistence owner serializes credential
and session changes. Domain operations receive admitted calls; Photo models
must not contain cookies, token digests, or authentication-framework types.
Local administration owns generation, rotation, and revocation. It must use the
same exclusive state ownership as the service: offline administration refuses
while the server owns state, and requires a stopped server. Normal startup must
not disclose or generate credentials automatically.

## Credential and Session State

Generate 32 bytes from the operating system CSPRNG, encode as unpadded base64url,
and present the Access Token once through explicit local administration. Do not
accept short human-selected replacements. Persist only its SHA-256 digest and
an independently generated generation identifier. Compare digests in constant
time. Fast hashing is appropriate for generated high-entropy secrets, not
human-selected passwords. Never log credentials or place them in process
arguments. Protect authentication state under the admitted state directory.

Each Browser Session must use a separate 32-byte CSPRNG secret. Persist only its
digest, credential generation, creation time, and absolute expiry, plus a session-bound random 32-byte CSRF secret. The browser
receives it in a host-only cookie with HttpOnly, SameSite=Lax, Path=/, and no
Domain. HTTPS uses `__Host-slipstream` with Secure; HTTP uses `slipstream` without
Secure. The configured origin selects the cookie for issuance, admission, and
expiration; forwarded headers must not select or weaken it. Its maximum age
must not exceed server expiry. CSRF material is not an authentication credential;
keep it in protected session storage and expose it only to the authenticated
browser through status. Rotate it when a new session is created. Restart
must preserve valid sessions; expiration and generation checks are server-owned.
Never store the Access Token in this cookie or persist either secret in
localStorage, sessionStorage, IndexedDB, or Cache Storage.

Rotation must atomically replace the token digest and generation and invalidate
all sessions. Revocation removes active access and invalidates all sessions.
A session exchange concurrent with rotation must either commit under the current
generation or fail: no old-generation session can authorize a request afterward.
Every request admission checks the current generation and expiry. Revocation
orders with admission, not completion; already-admitted work may settle. Do not
hold an authentication transaction throughout image processing or a mutation.

Storage failure must deny admission. An absent credential on an otherwise valid
new store permits only setup-required public surfaces. Invalid or unreadable
authentication state must fail startup, never be interpreted as a fresh store.
Backup includes authentication state. Before exposing a restored instance,
local rotation must invalidate restored credentials and sessions. A rollback
binary without access enforcement must remain isolated from public traffic.

## Browser and Machine Admission

The browser sends the Access Token once in a POST body to establish a session
over the configured HTTP or HTTPS origin. That route must enforce the origin, bounded body size,
rate limiting, and logging redaction. Issue a fresh session identifier on each
successful exchange. Session status must reveal only current access state and
expiry, not Library details. Logout must be a protected state-changing request
that revokes the presented session and expires its cookie. It is idempotent for
an already-invalid session after request-origin checks.

The CLI sends `Authorization: Bearer` with the Access Token. Validate its digest
and active generation before the same private route dispatch. Do not follow
redirects or transmit credentials to a different origin. A request containing
both cookie and Authorization credentials must return 400 `invalid_request`, never silently
prefer one or fall back after a failed credential. Bearer responses must use the
HTTP authentication challenge and invalid-token semantics of RFC 6750.

Use a default-protected router. Explicit public exceptions are the nonprivate
shell and versioned static assets, access establishment/status, and minimal
`GET`/`HEAD /healthz`. Library status, CLI discovery, browse cursors, derivatives,
metadata, recovery, editing, processing jobs, and Export downloads are private.
Check access before resource existence, file access, HEAD, range processing,
ETag comparison, or 304 responses. Existing tokens in browse URLs are not access
credentials. Public readiness must not disclose access state or Library paths.

Cookie-authenticated mutations require a session-bound CSRF token and exact
configured-origin validation. Establishment uses the exact-origin check before
accepting a credential. Reject missing or mismatched browser Origin on these
state-changing routes; same-origin browser GET session status can provide CSRF
material in a no-store response. Explicit Bearer requests do not use ambient
cookies and need no cookie CSRF token; browser requests with an Origin still
must match the configured origin. Do not enable permissive CORS. Match a parsed
scheme/host/port origin, never a substring or a forwarded client identity.

Invalid or expired credentials produce 401 before private processing; rejected
origin/CSRF produces 403; malformed input produces 400; throttling produces 429
with Retry-After. Never redirect API or image failures to successful login HTML.
CLI result mapping belongs exclusively in the CLI reference. [Deployment](../docs/deployment.md#access-administration) owns operator command syntax. No hidden alternate token transport is permitted.

## Bounded Operation

The exchange body is bounded to 256 bytes and the token has exactly 43
base64url characters with canonical encoding. Limit exchanges, successful or
failed, to 20 per rolling 60 seconds per instance and 5 per rolling 60 seconds
per direct peer address. Do not trust forwarded addresses. A proxy therefore
shares its peer budget; this conservative policy is sufficient for one
Photographer. Use a monotonic clock for rate windows and persist wall-clock
session expiry. Rate state may reset on process restart; this is not credential
revocation.

Keep at most 1,024 peer buckets; reclaim expired buckets before insertion and
reject new peers when the set is full. No unbounded peer map or waiting queue
is permitted. Limit simultaneous exchanges to four and reject excess work with
429 and Retry-After: 1. Rate-window exhaustion returns the positive rounded-up
seconds until the relevant oldest counted attempt leaves its window. Bucket
capacity returns Retry-After: 60. Rejected attempts do not extend the window.

Reclaim expired sessions before admission. Admit at most 32 live Browser
Sessions, with no silent eviction. Capacity returns 429, `session_capacity`,
and Retry-After until the earliest session expiry; show that signing out a
session or operator rotation can free capacity earlier. Rotation provides
recovery without permanent lockout. HTTP layer header/body and private request
concurrency limits remain in force. Credential failures on private routes must
not create sessions or per-token state.

## Authentication Wire Contract

These endpoints accept only their specified methods; reject others with 405.
All responses use no-store. Request objects are closed; duplicate keys, trailing
content, or malformed JSON produce 400. Errors use `{ "error": CODE }` with no
credential or internal diagnostics.

- `POST /api/access/session`: JSON `{ "token": TOKEN }`, content type
  application/json and matching Origin. Reject invalid credentials with 401
  `invalid_token`, unconfigured access with 503 `access_unconfigured`, and
  invalid input with 400 `invalid_request`. After durable session creation,
  return 204 with the cookie. A lost response may consume one session slot;
  the browser must not automatically retry establishment.
- `GET /api/access/session`: with a valid cookie, return 200 JSON
  `{ "authenticated": true, "expiresAt": RFC3339_UTC, "csrfToken": SECRET }`.
  Without one, return 200 `{ "authenticated": false, "configured": BOOLEAN }`,
  where configured means an active token exists. This minimal setup signal is
  intentionally public; it reveals no Library facts. An Authorization header returns
  400 `invalid_request`.
  An invalid cookie must be expired in the response. No cross-origin response
  sharing is allowed.
- `DELETE /api/access/session`: with valid cookie, exact Origin and
  `X-CSRF-Token`, revoke and return 204 with an expired cookie. With no valid
  session, exact Origin suffices for idempotent 204 and cookie clearing. A
  wrong Origin or invalid CSRF for a valid session returns 403 `access_denied`.

Use `rate_limited` and `session_capacity` for the corresponding 429 errors.
Authentication storage failure returns 503 `access_unavailable`. Other private
routes return 401 `authentication_required` for missing/invalid access and
403 `access_denied` for failed origin/CSRF. These early rejections confirm no
operation was admitted. Include `WWW-Authenticate: Bearer realm="slipstream"`
on protected-route 401 responses, adding `error="invalid_token"` when a Bearer
credential was supplied. The existing CLI envelope must map these boundary
errors as specified in the CLI reference before ordinary result decoding.

CSRF tokens must compare in constant time and belong to the presented session.
The browser fetches status at startup, on a return to a displayed tab, and
before restoring history. Apply existing request-generation ownership so a
stale status or image completion cannot restore access after local logout.
Status checks share one in-flight request and one session transition owner.
Returning to an already displayed tab checks in the background, keeps its view
on a network failure, and removes it on confirmed rejection. Existing private
requests still rely on server-side admission; this can leave previously rendered
content visible until rejection is observed after remote revocation. On a shared
device, explicit sign-out removes the view in every open tab. Initial entry,
known expiry, and restored history without an available view remain closed until
verified. Image failure checks preserve a valid view when the server is
unreachable.

Browse owns lease release, including disposal. The page supplies its explicit
release callback with authenticated transport before closing the access epoch.
Authentication transport owns credentials, CSRF and redirect policy without
recognizing Browse routes. Ordinary requests remain subject to view admission;
cleanup does not grant a general request-admission bypass.

## Private Content and Transport

Private API, metadata, mutation, editing, and download responses, including
errors, must use `Cache-Control: no-store`. A successfully authenticated
immutable Camera Preview derivative may instead use:

```text literal
Cache-Control: private, max-age=3600, must-revalidate
Vary: Cookie, Authorization
ETag: "<cacheKey>"
```

The policy applies to its successful `200`, `304`, and `HEAD` responses.
Authentication and currentness checks occur before file access, range handling,
`ETag` comparison, or a `304` response. Session, metadata, mutation, editing,
and Export responses remain `no-store`. Keep server-side derivative identity
and cache reuse independent of browser caching; do not add private
service-worker caches. Detach in-flight image sources and private views on
access loss and reject stale asynchronous completions.

Retire the old publicly cacheable image URL namespace. New pages must use the
new protected namespace; old paths must not serve private content or redirect
to a protected image. Purge operator-controlled public caches before exposure.
Changing response headers cannot recall bytes already cached or downloaded.
Keep static asset caching separate from private response policy.

The operator configures one canonical HTTP or HTTPS origin, containing no
credentials, non-root path, query, or fragment. An omitted origin resolves to
`http://localhost:<port>`; it never changes the listener's loopback default.
An explicit listener setting is required for network exposure. Existing HTTPS
deployments may terminate TLS at a proxy. Arbitrary forwarded headers never
grant access or select cookie attributes. Clients verify HTTPS certificates,
warn once per CLI invocation or visibly in the Web UI for HTTP, and never
follow redirects with credentials or silently change scheme.

Request identifiers use `crypto.getRandomValues` CSPRNG bytes available over
HTTP, with UUID version and variant bits. If CSPRNG is unavailable, fail rather
than substitute weak randomness. Download SHA-256 verification uses an
established library and rejects altered bytes even when secure-context Web
Crypto is unavailable. Ordinary HTTP browser use preserves all Library
operations; installation requires a browser secure context.

Selected: adapt the existing origin and session contract to the transport.
Separate access modes or pairing would add configuration and lifecycle state
without changing authentication requirements. Built-in TLS and certificate
management are outside this change.

## Options

Selected: generated instance token plus opaque browser sessions and direct CLI
Bearer access. This preserves native browser image loading, individual browser
logout, and immediate generation-based revocation without identity maintenance.

Rejected for this scope: putting the master token in every browser URL or
browser storage. It leaks the long-lived credential and makes ordinary logout
insufficient. Direct browser Authorization headers also complicate image loading.

Deferred: local accounts and OIDC. They add identity and recovery dependencies
that the single-instance possession model does not require. Self-contained JWTs
would still need revocation state here. Do not build an authentication plugin
system, role engine, or OAuth authorization server.

## Verification

An independent reviewer must derive the expected behavior before implementation.
Coverage must include every private route in both credential modes, unknown
resource requests, direct images/downloads, HEAD/range/conditional responses,
origin and CSRF rejection, mixed credentials, malformed input, limits, expiry,
restart, concurrent rotation/admission, logout, restore, and rollback isolation.
Tests must assert no secret appears in logs, links, CLI output, or browser
storage. Verify interrupted mutations against actual server facts without replay.

Run affected package suites and the repository gate. Exercise the actual HTTPS
proxy with desktop and narrow Chromium: establish access, browse, select, read
through CLI, sign out, restore history, and rotate credentials. Include a warm
cache from the prior release and operator health/acceptance tooling. Mock-only
verification cannot establish a safe public deployment.

## References

- [RFC 6750](https://www.rfc-editor.org/rfc/rfc6750.html): Bearer header transport,
  challenge behavior, TLS, and URL leakage. Local issuance is not an OAuth flow.
- [OWASP Session Management](https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html):
  random identifiers, cookie controls, expiry, and server-side invalidation.
