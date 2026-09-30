# Installed Web Application

A Photographer may launch Slipstream from a device's application launcher
without first finding its browser tab. Installation must preserve the same
online Photo Library and decision rules as browser use.

## Installation and Launch

On supported Chromium browsers, Slipstream must provide its name and icon for
the browser's installation interface and request a standalone application
window. Installation remains a browser-controlled action. The ordinary browser
interface must remain usable without installation.

A new window opened at the application start URL must open All Photos.
Reactivating an existing window must not reset its current Destination.
Opening a Destination and
explicit Album Resume must retain the behavior defined in
[Library Browser Experience](library-browser-experience.md). Installing the
application must not create a separate Photo Library or copy Original Files.

The application requires a reachable Slipstream server. Installation must not
promise offline launch or offline selection. A disconnected open window must
follow [Failure Behavior](library-browsing-and-selection.md#failure-behavior).

## Deployment and Support

Installation requires trusted HTTPS or browser-supported localhost/127.0.0.1
HTTP. An HTTP LAN address does not qualify for that exception and must not show
installation metadata. Ordinary browser use must still support all Library
operations there. Transport and authentication follow [Instance Access](access.md).

The [supported browser contract](0.1-support-and-release.md) applies to both
installed and ordinary browser use. Installation metadata must not imply
support for additional browser engines.
