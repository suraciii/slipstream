# Installed Web Application

A Photographer MAY launch Slipstream from a device's application launcher. Installation MUST preserve the same online Photo Library and decision rules as browser use.

[Web Frontend Architecture](../design.md) owns the application lifecycle shared with browser use.

## Installation and Launch

On supported Chromium browsers, Slipstream MUST provide its name and icon for the browser's installation interface and request a standalone application window. Installation remains a browser-controlled action. The ordinary browser interface MUST remain usable without installation.

A new window opened at the application start URL MUST open All Photos. Reactivating an existing window MUST NOT reset its current Destination.

Opening a Destination and explicit Album Resume MUST follow [Library Browser Experience](../../library-browser/experience/spec.md). Installation MUST NOT create a separate Photo Library or copy Original Files.

The application requires a reachable Slipstream server. Installation MUST NOT promise offline launch or offline selection. A disconnected open window MUST follow [Failure Behavior](../../library-browser/spec.md#failure-behavior).

## Deployment and Support

Installation requires trusted HTTPS or browser-supported localhost/127.0.0.1 HTTP. An HTTP LAN address does not qualify for that exception and MUST NOT show installation metadata. Ordinary browser use MUST still support all Library operations there.

Transport and authentication follow [Instance Access](../../access/spec.md). The [supported browser contract](../../deployment/support.md) applies to installed and ordinary browser use. Installation metadata MUST NOT imply support for additional browser engines.
