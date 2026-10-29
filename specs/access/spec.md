# Instance Access

One Access Token grants instance-wide access to the Photographer and the Photographer's CLI. [Access Architecture](design.md) owns credential enforcement and the authentication protocol.

## Access Boundary

An Access Token MUST grant all existing Library capabilities. Slipstream MUST NOT maintain user accounts, email addresses, roles, or external identities for this capability.

Giving someone the token grants the Photographer's access, not a separately revocable person or a public Album share.

Valid access MUST be required for all Library facts and read and write operations. Identifiers and download links MUST NOT confer permission.

An anonymous visitor MUST NOT read private facts or receive a state-changing response through a direct URL or either client. Private content includes:

- Photos, counts, and filenames.
- Previews and Edit Previews.
- Processing results, Exports, and downloads.

Only the nonprivate application shell, access entry points, and minimal health information MAY be public. Original safety and operation preconditions continue to apply after access is granted.

## Provisioning and Recovery

The server operator MUST generate an unpredictable token through local administration before opening the Library. Slipstream MUST disclose it once to the operator and MUST NOT choose a default token.

The operator MUST be able to save the token in a password manager. Anonymous visitors MUST NOT claim or configure an uninitialized instance.

Without a configured token, the browser MUST show that access is not configured and direct the Photographer to the operator. It MUST NOT show Library facts.

Invalid authentication storage MUST fail closed.

The token remains valid until the operator rotates or revokes it. Rotation MUST replace the token and invalidate all Browser Sessions. Revocation without replacement MUST close Library access.

Provisioning, rotation, and revocation MUST NOT change:

- Photos or Original Files.
- Albums.
- Selection or Rating.
- Saved position.
- Editing intent.
- Exports.

A lost token MUST be replaced, not retrieved.

Upgrade and restore MUST preserve Library data and require valid access before public exposure.

Before a restored instance is exposed, local rotation MUST invalidate restored credentials and sessions. A rollback binary without access enforcement MUST remain isolated from public traffic.

## Browser Entry

The entry screen MUST contain:

- An Access token field.
- A Show/Hide control.
- An Open library button.
- Help directing the Photographer to the server operator.

It MUST support paste, password-manager use, and Enter submission.

The screen MUST support visible focus and announced errors. It MUST work in a narrow viewport with its keyboard open. Private Photos MUST NOT decorate it.

Successful entry MUST establish a Browser Session and clear the token field. It MUST return to a valid requested Destination or open All Photos if none was requested. It MUST preserve explicit Album Resume behavior and MUST NOT follow an external return address.

Incorrect tokens, temporary throttling, server unavailability, and expired sessions MUST have distinct messages. Throttling MUST show when another attempt is allowed. Submission MUST prevent duplicate requests while pending.

A Browser Session MUST expire seven days after creation without sliding renewal. The token remains valid independently, so the Photographer can establish another session.

The entry screen MUST remind users to sign out on shared devices.

Sources MUST include Sign out on wide and narrow layouts. Sign out MUST revoke only the current Browser Session and remove private content from that browser's application state.

Sign out MUST NOT claim successful server revocation if the server cannot confirm it. Other sessions remain valid until expiry or token rotation. Someone retaining a copy of the Access Token can establish another session.

## Session Loss and Disconnection

On detected expiry, revocation, or rejection, the browser MUST stop new writes, remove private views, and offer access entry at the current Destination. Other tabs MUST react to local sign-out. A locally known expiry MUST hide content even while disconnected.

Returning to a tab with an already displayed, unexpired Browser Session MUST leave the current view in place while the browser checks access in the background. Confirmed rejection MUST remove the view and open access entry.

A temporarily unreachable server MUST NOT replace the current view with a checking screen or ask the Photographer to retry access. Private requests still require server authorization; ordinary disconnection follows the browsing contract.

Concurrent writes MUST report truthful outcomes.

Reloading a page or restoring history before a private view is available MUST validate access before displaying private content.

Requests admitted before revocation MAY finish. Reopening access MUST reconcile current server facts and MUST NOT automatically repeat uncertain mutations or claim they were saved.

Previously copied images cannot be remotely erased.

## CLI and Deployment

The CLI MUST use the same Access Token for the same instance permissions without requiring a browser session or interactive login. It MUST load the token from an operator-selected private file, never from a plaintext command argument.

The CLI MUST NOT expose the token in output, links, or diagnostics. [CLI Reference](../command-line/reference.md) owns credential-input syntax and error mapping.

The Photographer MAY use any allowed access origin directly. [Access Architecture](design.md#credential-and-session-state) owns origin selection and cookie profiles.

HTTP MUST show an unencrypted connection warning before browser token entry and keep it visible during the session. It MUST warn once per CLI invocation.

HTTP MUST preserve authentication, CSRF protection, and all ordinary browser Library operations, including editing, deletion, and verified downloads. It MUST NOT enable anonymous access, including on loopback.

Clients MUST NOT change the configured scheme after a connection failure. PWA installation remains subject to [Installed Web App](../web-application/installation/spec.md).

[Deployment](../deployment/spec.md) owns configuration and upgrade procedures.
