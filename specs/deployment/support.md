# Support and Release Boundary

This contract distinguishes the supported `0.1` artifact from the broader target capabilities. It does not announce a release or imply publication.

## Supported Scope

A `0.1` deployment supports one Photographer and one configured local or network-mounted Library Folder. The operator selects the Folder; the browser does not select arbitrary server paths.

The supported target is the repository-owned Docker image and Compose configuration on Linux amd64. Chromium is the supported browser; direct host deployments, other architectures, and other browser engines are not supported `0.1` targets.

The container is the native dependency boundary. LibRaw and libjpeg provide RAW embedded-JPEG extraction; libvips and LittleCMS normalize JPEGs. Bun is a Web build dependency, not a production runtime.

The `0.1` artifact without the separately qualified [Instance Access](../access/spec.md) capability has no accounts, authentication, authorization, or request-origin access control. Every client reaching its listener can use the Library, including state-changing routes; `Host`, `Origin`, and forwarded headers do not establish trust.

The server's `SLIPSTREAM_HOST` defaults to `127.0.0.1`. Compose publishes `${SLIPSTREAM_BIND_ADDRESS:-127.0.0.1}:${SLIPSTREAM_PORT:-3000}`; an operator MAY bind another interface, but LAN or Tailscale use is supported only on a trusted network.

Protecting wider exposure is the operator's responsibility. A separately qualified release is required before authentication, migration, and HTTPS expand this artifact's support boundary.

## File and Camera Boundary

Discovery recognizes extensions case-insensitively:

| Kind | Extensions |
| --- | --- |
| RAW | `3fr`, `ari`, `arw`, `bay`, `bmq`, `cap`, `cine`, `cr2`, `cr3`, `cs1`, `dc2`, `dcr`, `dng`, `drf`, `eip` |
| RAW | `erf`, `fff`, `gpr`, `iiq`, `k25`, `kc2`, `kdc`, `mdc`, `mef`, `mos`, `mrw`, `nef`, `nrw`, `obm`, `orf` |
| RAW | `pef`, `ptx`, `pxn`, `qtk`, `r3d`, `raf`, `raw`, `rdc`, `rw2`, `rwl`, `rwz`, `sr2`, `srf`, `srw`, `x3f` |
| JPEG | `jpg`, `jpeg` |

Recognition indexes a candidate; it does not guarantee metadata or an embedded JPEG for every file or camera. LibRaw defines RAW container support; unsupported, corrupt, or resource-limited files remain unavailable rather than receiving a generic RAW rendering.

The qualified Linux image for Sony `ILCE-7CM2` ARW Originals MUST use hash-verified LibRaw `0.22.2`. The representative camera requirement does not imply support for other Sony bodies or ARW revisions.

For that camera, Grid thumbnails and Photo View review Previews MUST use the Original's own `raw-embedded-jpeg`. Their source and dimensions MUST reflect that embedded JPEG, and extraction MUST NOT change the Original.

Release qualification MUST use the Photographer's representative Originals without adding them to the repository. Each additional camera qualification MUST establish the same Preview behavior using that camera's own Originals.

[Photo Previews](../photo-library/previews/spec.md) owns Preview source, detail, and failure behavior. [Photo Library](../photo-library/spec.md) owns Original safety and Library behavior.

## State, Rescan, and Recovery

[Photo Library Identity](../photo-library/identity.md) owns identity, Location Recovery, migration, and Library Folder binding. [Capture-Time Ordering](../photo-library/ordering.md) owns ordering and unavailable-Photo Capture Time.

The supported `0.1` writable state is SQLite schema version 15. State and derivative cache MUST be separate from the Library Folder; [Deployment](spec.md#storage-rules) owns storage admission.

A v14-to-v15 upgrade requires a stopped service and a verified, sidecar-free, consistent pre-upgrade backup. It preserves Photo and Original identity, Rating, Album membership, saved positions, and Original Files while converting legacy Selection State to `unflagged`, `picked`, and `rejected`.

There is no in-place down migration. Rollback MUST stop the v15 service and restore the verified v14 backup before starting a compatible v14 image; a v14 binary rejects v15 state.

Upgrades from v4 or earlier first complete earlier migrations; upgrades from v5 through v13 then complete later migrations. The v4-to-v5 Album migration and v5-to-v6 independent-Photo migration preserve the state defined by their respective contracts.

Rollback across earlier migrations MUST use their verified backups and compatible images. [Deployment](spec.md#backup) owns consistent recovery points and isolated restore admission.

Library Expansion requires a verified current-schema backup after migration and before the offline expansion command. Rollback restores that backup and the prior Library Folder configuration.

The derivative cache is rebuildable and does not own Selection State, Rating, Album membership, or saved position. An isolated restore or cache rebuild MAY use an empty cache; regeneration MUST preserve the state database and MUST NOT modify Original Files.

The operator's detailed backup, restore, and rollback procedure lives with the deployment. On the supported host layout it is `/data/slipstream/RUNBOOK.md`.

## Rollback Artifact Retention

A rollback artifact consists of the immediately preceding deployable image and its compatible, verified state backup. An operator-controlled sealed rollback archive retained outside the repository is also a rollback artifact and MUST remain independently accessible.

The minimum rollback window is 30 consecutive days after replacement deployment production acceptance. Release approval MAY extend, but MUST NOT shorten, that window.

An operator MAY retire an artifact only when all conditions hold:

- The replacement passed release acceptance and an isolated restore from its verified backup.
- The minimum or extended rollback window has ended.
- A tested rollback artifact exists for the current supported deployment.
- No unresolved Original-safety, state-integrity, or service-recovery incident requires the artifact.

Release approval MUST identify retained artifacts and the rollback window. Source changes, a successful build, or a release-gate decision alone do not retire an artifact.

## Known Limitations and Non-Goals

The `0.1` artifact does not support:

- Multiple Photographers, accounts, or authorization.
- Public Internet access or cloud backup.
- XMP writes or Original-file writes.
- AI selection, RAW development, color-managed proofing, or desktop RAW editor parity.
- Arbitrary server-directory selection, continuous filesystem watching, or global disk search.
- Fuzzy image matching or automatic merging of conflicting user state.
- Full-resolution tiling.

The `0.1` artifact does not promise compatibility with every camera.

The `0.1` browser has no visual bulk reorder surface; explicit bulk reorder is available through the local HTTP API. [Library Browser Experience](../library-browser/experience/spec.md) owns browser restoration and Album Resume.

Operators MUST NOT bypass the state store's admitted Library Folder binding or change the Docker volume source beneath it. [Library Identity](../photo-library/identity.md) defines the supported offline ancestor expansion.
