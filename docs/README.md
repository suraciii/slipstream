# Product Specs

`docs/` defines the target behavior that Photographers and their Agents can observe or depend on. It is the product specification layer, not a record of source-code structure.

Follow the scoped instructions in [`AGENTS.md`](AGENTS.md) when writing or changing a Product Spec.

## Product Definition

- [Product Vision](vision.md): human and machine use, goals, audience, principles, and product boundaries
- [Command-Line Use](command-line.md): delegated photo workflows, shared state, Preview delivery, effects, and recovery
- [CLI Reference](cli-reference.md): authoritative command syntax, inputs, structured results, errors, and composition examples

## Core Experience

- [Photo Library and Albums](photo-library.md): indexing existing files, read-only Original Folders, virtual Albums, independent RAW and JPEG Photos with Location Recovery, and Original File ownership
- [Library Browser Experience](library-browser-experience.md): responsive screen composition, contextual controls, browser destinations, history restoration, and space acceptance
- [Library Browsing and Selection](library-browsing-and-selection.md): progressive Grid and Photo views, source order, loading feedback, gestures, selection state, rating, detail inspection, and undo
- [Photo Previews](previews.md): acceptable preview sources, visible provenance, quality limits, caching behavior, and failures

## Photo Development

- [Photo Development](photo-development.md): RAW correction, autosave, fixed film simulation, stage previews, exports, shared client behavior, and recovery

## Support and Release

- [Installed Web Application](installed-web-app.md): online installation, launch, and supported deployment

- [0.1 Support and Release Contract](0.1-support-and-release.md): supported environment, file and camera boundary, recovery, limitations, rollback-artifact retention, and 0.1.0 release notes
- [Deployment](deployment.md): image, host storage, and supported Compose operator contract
