# Instance Access

A Photographer needs to use the Photo Library over a public connection without
operating an account system. Slipstream must protect the entire instance with
one Access Token while supporting both the browser and the Photographer's CLI.

## Access Boundary

An Access Token grants all existing Library capabilities. Slipstream must not
maintain user accounts, email addresses, roles, or external identities for this
capability. Giving someone the token grants the same access as the Photographer;
it does not create a separately revocable person or a public Album share.

All Library facts, Photos, Previews, Edit Previews, Exports, and read and write
operations must require valid access. Identifiers and download links must not
confer permission. Only the nonprivate application shell, access entry points,
and minimal health information may be public. Original safety and operation
preconditions continue to apply after access is granted.

## Provisioning and Recovery

The server operator must generate the token through local administration before
opening the Library. Slipstream must generate an unpredictable token, disclose
it once to that operator, and never choose a default token. The operator must
be able to save it in a password manager. Anonymous visitors must not claim or
configure an uninitialized instance.

Without a configured token, the browser must show that access is not configured
and direct the Photographer to the operator. It must not show Library facts.
Invalid authentication storage must fail closed.

The token remains valid until the operator rotates or revokes it. Rotation must
replace the token and invalidate all Browser Sessions. Revocation without a
replacement must close Library access. A lost token must be replaced, not
retrieved. None of these operations may change Photos, Albums, selection,
Rating, saved position, editing intent, Exports, or Original Files.

## Browser Entry

The entry screen must contain an Access token field, Show/Hide control, Open
library button, and help directing the Photographer to the server operator.
It must support paste, password-manager use, Enter submission, visible focus,
announced errors, and a narrow viewport with its keyboard open. Private Photos
must not decorate this screen.

Successful entry must establish a Browser Session and clear the token field.
It must return to a valid requested Destination. Without a requested Destination,
it must open All Photos. It must preserve the explicit Album Resume behavior.
An external return address must never be followed.

Incorrect tokens, temporary throttling, server unavailability, and expired
sessions must have distinct messages. Throttling must show when another attempt
is allowed. Submission must prevent duplicate requests while pending.

A Browser Session must expire seven days after creation without sliding renewal.
This lifetime is a product choice. The token itself is longer lived, so a
Photographer can establish another session. The entry screen must remind users
to sign out on shared devices.

Sources must include Sign out on wide and narrow layouts. Sign out must revoke
only the current Browser Session and remove private content from that browser's
application state. It must not claim successful server revocation if the server
cannot confirm it. Other sessions remain valid until expiry or token rotation.
Someone retaining a copy of the Access Token can establish another session.

## Session Loss and Disconnection

On detected expiry, revocation, or rejection, the browser must stop new writes,
remove private views, and offer access entry at the current Destination. Other
tabs must react to local sign-out; returning tabs and restored history must
validate access before redisplaying private content. A locally known expiry
must hide content even while disconnected.

Requests admitted before revocation can finish. Reopening access must reconcile
current server facts and must not automatically repeat uncertain mutations or
claim they were saved. Ordinary disconnection remains distinct from rejected
access and follows the existing browsing contract while access remains valid.

## CLI and Deployment

The CLI must use the same Access Token for the same instance permissions. It
must not require a browser session or interactive login. It must load the token
from an operator-selected private file, never from a plaintext command argument,
and must not expose it in output, links, or diagnostics. Credential-input syntax
and error mapping belong in [CLI Reference](cli-reference.md).

Public use requires HTTPS and a private backend behind the operator's TLS
proxy. Missing access configuration must never enable anonymous fallback,
including on loopback. [Deployment](deployment.md) owns the transport and
upgrade procedure. [Instance Access Architecture](../design/access.md) owns
credential storage, session enforcement, and caching.

## Acceptance

- An anonymous visitor cannot read a Photo, count, filename, processing result,
  download, or state-changing response through a direct URL or either client.
- A Photographer opens a deep link after entry, selects a Photo, and observes
  the same decision through an authenticated CLI query.
- Sign out, expiry, token rotation, and revoked access reject subsequent
  requests with their specified scope, without losing confirmed Library state.
- Wrong tokens, throttling, disconnection, restored history, and concurrent
  writes give truthful results and never silently open access or repeat writes.
- Upgrade and restore preserve Library data and require valid access before
  public exposure. Previously copied images cannot be remotely erased.
