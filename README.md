# Slipstream

Slipstream is a photo selection and organization workspace for a Photographer and their own Agent. Its target product provides a visual Web application and a machine-friendly CLI over shared photo capabilities, without modifying the original RAW or JPEG files. Slipstream does not include an Agent.

The [product contract](specs/spec.md) and [shared system architecture](specs/design.md) govern Slipstream. The [`specs/` index](specs/README.md) groups Feature contracts, including [Command-Line Use](specs/command-line/spec.md) and the [CLI Reference](specs/command-line/reference.md). [`CONTEXT.md`](CONTEXT.md) defines product and domain vocabulary. [Usage and installation guides](docs/README.md) explain how to use Slipstream. Delivery planning and progress are tracked in [GitHub Issues](https://github.com/suraciii/slipstream/issues).

The production server architecture is Rust; Bun and TypeScript own the Web application, browser tests, and repository tooling. The repository contains no Node rollback server; the sealed rollback artifact is maintained outside the source tree by the operator. See [Rust Server](specs/server.md) for the binding boundary, [Support and Release](specs/deployment/support.md) for the 0.1 support boundary and release notes, [Deployment](specs/deployment/spec.md) for the Docker deployment contract, and [`CONTRIBUTING.md`](CONTRIBUTING.md) for setup, the low-cost `just ci-fast` gate, and the complete `just ci-full` handoff gate.

For local use, follow [server startup](CONTRIBUTING.md#server-startup) and
provision an [Access Token](specs/deployment/spec.md#access-administration), then open
`http://localhost:3000`. No domain, certificate, or reverse proxy is required.
`SLIPSTREAM_PUBLIC_ORIGIN` optionally selects HTTP or HTTPS; network HTTP shows
an unencrypted connection warning. Authentication is required on both transports.

## Web review

- Open **View options** to stage filters, order, and thumbnail size. **Apply**
  commits them; **Reset filters** changes only the draft. A zero-match result
  offers **Clear filters** to immediately remove conditions without changing
  the source or order. Capture Time uses camera-local seconds, with an inclusive
  From bound and an exclusive Before bound.
- Use **Select mode** for batch decisions or **Add to Album**. Choose an Album
  before confirming. **New Album** is also available here and in Photo Albums;
  creating one does not add Photos. Selection remains active after a batch.
  Open **Result** to inspect the outcome and any available compensation.
- In Photo View, **More** groups Review, Photo, Library, and Help. Swipe
  feedback appears only while a gesture is active. The visible review controls
  remain available without gestures.
- **Edit** shows the Photo's Camera reference when available. It is not an Edit
  Preview or proof that an editing tool can run. Inspect tool availability and
  select a tool explicitly; opening Edit does not render a result.
- Open **Trash** from View options to inspect removed Photos and Restore them.
  Location output identifies its server-side namespace. Record cleanup never
  deletes Original Files.

See [Library Browser Experience](specs/library-browser/experience/spec.md) for
navigation and accessibility, [Photo Workspace](specs/photo-development/workspace/spec.md)
for editing, and [Trash](specs/photo-library/trash/spec.md) for recovery boundaries.

## License

Slipstream is available under the [MIT License](LICENSE). Third-party components retain their own terms; see [Third-Party Notices](THIRD-PARTY-NOTICES.md) and the bundled [Rust component licenses](RUST-LICENSES.html).
