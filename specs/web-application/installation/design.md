# Web Installation

An installed window uses the same server-owned Photo Library as a browser tab. [Web Frontend Architecture](../design.md) owns startup and disposal. [Web Async Ownership](../async-ownership.md) owns shared-state settlement.

## Ownership and Launch

Vite publishes installation metadata and icons from the Web application's public directory. The HTML entrypoint links the manifest and icons.

Every manifest-declared icon MUST decode at its declared size.

| Manifest property | Required value                                                                                  |
| ----------------- | ----------------------------------------------------------------------------------------------- |
| Identity          | `/`                                                                                             |
| Scope             | `/`                                                                                             |
| Start URL         | `/`                                                                                             |
| Display           | `standalone`                                                                                    |
| Launch handler    | `focus-existing`                                                                                |
| Shortcuts         | Picked `/?selection=picked`; Unflagged `/?selection=unflagged`; Rejected `/?selection=rejected` |

Installation identity MUST NOT change with a build or Destination. The Library Browser retains ownership of navigation, decisions, and reconnection.

The Launch Handler API consumer MUST apply only a same-origin target outside the application start URL. An ordinary start-URL relaunch therefore leaves the current Destination in place, while a shortcut or explicit deep link replaces it. Every shortcut target is an ordinary Browser Address; no shortcut owns separate Library state or bypasses access.

The application MUST use the browser's installation interface. It MUST NOT register a service worker or add a local mutation queue. This manifest-only boundary avoids a second asset cache and worker lifecycle for the [online installation contract](spec.md).

## Static Delivery

| Resource                                     | Delivery contract                                                                                |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `/manifest.webmanifest`                      | The Rust server MUST use `application/manifest+json` and `Cache-Control: no-cache`.              |
| Public icons under `/icons/`                 | The server MUST use `Cache-Control: no-cache`.                                                   |
| HTML and other unversioned public files      | The server MUST require revalidation.                                                            |
| Versioned Vite build assets under `/assets/` | The server MUST retain immutable caching. Public installation files MUST NOT use this namespace. |

A missing manifest or file under `/icons/` MUST return 404 instead of the SPA document, with revalidation. Its HEAD response MUST have an empty body.

Browser Destinations retain the existing HTML fallback. Installation files MUST use the same confined filesystem reads as other Web resources. [Instance Access Architecture](../../access/design.md) owns private delivery and authentication.
