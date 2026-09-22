# Web Installation

An installed Slipstream window must use the same server-owned Photo Library as
a browser tab. Adding another cache and application lifecycle is unnecessary
for the [online installation contract](../docs/installed-web-app.md).

## Ownership and Launch

Vite publishes installation metadata and icons from the Web application's
public directory. The HTML entrypoint links the manifest and icons. The
manifest has a stable identity `/`, scope `/`, start URL `/`, and standalone
display. Installation identity must not change with a build or Destination.
Library Browser continues to own navigation, decisions, and reconnection.

Use the browser's installation interface. Do not register a service worker or
add a local mutation queue. A manifest-only application satisfies the online
scope without a second asset cache or worker update lifecycle. A worker that
caches an offline application shell is rejected because offline launch is not
required and it adds asset-version and activation coordination.

## Static Delivery

The Rust server must serve `/manifest.webmanifest` as
`application/manifest+json`. The manifest and public icons under `/icons/`
must use `Cache-Control: no-cache` so stable installation URLs can change on
deployment. HTML and other unversioned public files also require revalidation.
Vite owns `/assets/` and emits versioned build assets there; those files retain
immutable caching. Public installation files must not be placed in that
namespace.

A missing manifest or a missing file under `/icons/` must return 404, never the
SPA document, with revalidation and an empty body for HEAD. Browser
Destinations retain the existing HTML fallback. Installation files use the same
confined filesystem reads as other Web resources.

## Verification

Server tests must cover MIME, cache headers, GET and HEAD, missing installation
files, and retained Destination fallback. Browser tests against the production
bundle must load the manifest and decode every declared icon at its declared
size, verify launch metadata, and prove that normal online browsing still loads.
Real-device installation and standalone interaction are separate acceptance
evidence from automated browser tests.
