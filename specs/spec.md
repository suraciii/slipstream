# Product Vision

Slipstream is a photo selection and organization workspace for one Photographer and the Photographer's own external Agent.

## Goal

The Photographer can review an existing local or network-mounted Photo Library without a desktop editing application. Direct visual use works on a phone, tablet, or desktop; delegated use exposes the same photo-management capabilities through the CLI.

Both forms of use MUST let the Photographer inspect and correct the result.

## Human and Machine Use

| Actor | Responsibility |
| --- | --- |
| Photographer | Sets the purpose and judges the result. |
| External Agent | Interprets requests, composes available operations, and reports its work. |
| Slipstream | Exposes capabilities, enforces preconditions, and reports effects, failures, and uncertainty. |

Human and programmatic operations MUST share the same domain facts and safety rules. Their interaction forms MAY differ; presentation gestures and browser-local positions are not CLI capabilities.

Work started through one interface MUST remain inspectable and continuable through the other. [Command-Line Use](command-line/spec.md) owns the delegated interaction contract.

External visual judgments MUST NOT become Photo metadata or selection decisions without an explicit operation.

## Core Experience

One configured Library Folder defines discovery. Each supported Original File is an independent Photo, including RAW and same-basename JPEG files.

| Capability | Product outcome | Owning contract |
| --- | --- | --- |
| Discover and query | Useful work without transferring or retaining the entire Library in a client. | [Photo Library](photo-library/spec.md) |
| Recover Locations | Preserve identity when the recovery contract admits a moved Original. | [Library Identity](photo-library/identity.md) |
| Browse | Read-only Original Folders, progressive Grid and Photo views, and Album Resume. | [Library Browser](library-browser/spec.md) |
| Inspect | A trustworthy Preview with visible source and detail limits. | [Photo Previews](photo-library/previews/spec.md) |
| Decide | Independent Selection State and Rating with truthful persistence and conflict outcomes. | [Library Browser](library-browser/spec.md) |
| Organize | Explicitly ordered Albums without moving files or changing Photo-wide decisions through membership changes. | [Photo Library](photo-library/spec.md) |
| Continue | Stable identities and addressable Destinations across Web and CLI. | [Browser Experience](library-browser/experience/spec.md) |
| Edit and deliver | Independent tools read protected inputs, write workspace files, and expose selected results for download or compatible processing. | [Photo Workspace](photo-development/workspace/spec.md) |

Touch gestures MUST coexist with visible controls and keyboard access. Large Libraries MUST NOT require complete Preview generation before useful review.

## Preview Trust

Both interfaces MUST identify Preview Source and detail limits. [Photo Previews](photo-library/previews/spec.md) owns camera-produced source selection and prohibits cross-Photo substitution.

Preview presentation MUST preserve the camera rendering encoded in the source. It does not claim all recoverable RAW detail or equivalence to later RAW-editor output.

[Photo Development](photo-development/spec.md) owns the separate Edit Preview and Export contracts.

## Product Boundaries

Original Files MUST remain read-only to Slipstream. The Photographer controls external file removal; [Trash Record Cleanup](photo-library/trash/spec.md) removes only eligible application records.

Selection State MUST NOT be inferred from Rating or task-specific Album membership.

| Boundary | Scope |
| --- | --- |
| Editing | Independent headless darktable editing and the fixed standalone Film capability follow [Photo Development](photo-development/spec.md). |
| Engine availability | The product MUST report deployed operations and limitations; it does not promise every desktop GUI feature. |
| Metadata exchange | Explicit XMP interoperability follows [Read and Save Metadata](photo-library/metadata/spec.md), without authorizing Original writes. |
| Access | [Instance Access](access/spec.md) owns authentication; public sharing requires a separate product contract. |
| Release qualification | [Support](deployment/support.md) owns qualified release claims; target specifications do not imply publication or expand an already qualified release. |

Slipstream does not provide:

- an Agent runtime or ownership of its conversations, planning, providers, prompts, or memory;
- automatic aesthetic scoring;
- face recognition;
- a semantic index;
- a workflow engine;
- cloud backup;
- multi-user digital asset management.

External Agents MAY compose Slipstream's photo operations without making those external systems part of the product.
