# Third-Party Notices

Slipstream includes third-party components. Those components remain under their own licenses.

## Rust components

[`RUST-LICENSES.html`](RUST-LICENSES.html) contains the notices and license texts for the Rust server's locked Linux runtime dependency graph. `Cargo.lock` identifies the exact component versions.

## Web application

The built Web application includes Vite's module-preload helper under the following license:

> MIT License
>
> Copyright (c) 2019-present, VoidZero Inc. and Vite contributors
>
> Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

`bun.lock` identifies the exact build and test dependency versions. Build and test tools other than the Vite helper are not included in the production image.

## Ubuntu runtime components

The production image contains a flattened Ubuntu 26.04 userspace and the native runtime packages `ca-certificates`, `curl`, `libjpeg-turbo8`, `liblcms2-2`, `libraw23t64`, and `libvips42t64`, together with their package dependencies. The image preserves every installed package's copyright and license material under `/usr/share/doc/<package>/copyright`.

LibRaw is available under LGPL-2.1 or CDDL-1.0, and libvips is available under LGPL-2.1. Slipstream dynamically links these and the other native image libraries. Recipients may replace them by rebuilding the image with compatible Ubuntu packages. Ubuntu source package identities and exact installed versions are available from `dpkg-query`; corresponding source is available from the Ubuntu archive and Launchpad.

Redistributors must preserve these notices and the Ubuntu package copyright files. When a package's terms require corresponding source, redistributors must keep that source available for the distributed package version.

Canonical publishes the Ubuntu OCI build recipe and release-specific rootfs references in the [ubuntu-base repository](https://code.launchpad.net/~cloud-images-release-managers/cloud-images/+oci/ubuntu-base/+git/ubuntu-base).
