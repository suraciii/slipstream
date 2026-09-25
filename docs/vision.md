# Product Vision

Slipstream is a photo selection and organization workspace for a Photographer
and the Photographer's own Agent. Its Web application supports direct visual
review on a phone, tablet, or desktop. Its command-line client exposes the same
photo-management capabilities for delegated use. Original Files remain unchanged except for explicit [Permanent Deletion from Trash](library-management-trash.md).

## Goal

A Photographer can complete photo selection and organization away from a
desktop editing application, either directly or through their own Agent, while
seeing trustworthy Photos and being able to inspect and correct the result.

Slipstream reduces the cost of finding, comparing, grouping, and deciding what
to keep. Its focused development capability extends that work through exposure
and white-balance correction, an interoperable TIFF handoff, and a fixed film
simulation. It does not promise a general desktop editing suite.

## Human and Machine Use

Photographers and their Agents are both first-class users of Slipstream.
Product capabilities must be designed for direct human use and programmatic
use. The forms of interaction may differ, but they share Photo identity,
Album semantics, decisions, and Original safety rules.

The Photographer sets the purpose and judges the result. The external Agent
interprets the request, selects and combines available operations, and reports
its work. Slipstream exposes understandable capabilities, enforces their
preconditions, and returns facts about effects, failures, and uncertainty.

The Web application emphasizes Photos, relationships, decisions, and useful
direct controls. The CLI emphasizes discovery, bounded queries, composable
operations, and explicit results. A user can start work through an Agent and
inspect or continue it in the Web, or ask an Agent to use decisions made there.
[Command-Line Use](command-line.md) defines that interaction contract.

Slipstream does not include an Agent. It does not own conversations, planning,
model providers, prompt execution, or Agent memory. External visual reasoning
may use its Previews; such judgments do not become Photo metadata or global
selection decisions without an explicit operation.

## Core Experience

Slipstream serves one Photographer with one existing local or network-mounted
Photo Library containing mostly RAW files and some JPEG files. One configured
Library Folder defines discovery.

The product supports a complete path through these capabilities:

1. Discover and query the Photo Library without loading every Photo fact.
2. Index supported files without moving or changing Originals, and preserve
   identity when exact content evidence proves a moved Original.
3. Manage each supported Original File as an independent Photo.
4. Browse read-only File Locations and explicitly ordered Albums.
5. View each Photo through a trustworthy Preview and inspect available detail.
6. Record Selection State and Rating independently, with truthful persistence
   and conflict behavior.
7. Create and manage Albums without moving files or changing Photo-wide
   decisions merely because membership changes.
8. Inspect results and continue across the CLI and Web using stable identities
   and addressable destinations.

The Web remains a progressively loaded Library Browser with Grid and Photo
views, touch gestures, visible controls, keyboard access, and Album Resume.
The CLI does not reproduce presentation gestures or own browsing positions.

## Preview Trust

A JPEG Photo's Preview uses its own content. A RAW Photo's Preview uses its own
largest usable embedded JPEG. Slipstream must not substitute a sibling JPEG.

This preserves the camera's white balance, picture style or film simulation,
tone treatment, and orientation as encoded by the camera. Slipstream does not
claim to expose all recoverable RAW data or match later RAW-editor output.
Both interfaces must identify Preview Source and detail limits. An external
Agent receives the same evidence boundary as a Photographer. The distinct
Edit Preview and Export contracts are in [Photo Development](photo-development.md).

## Principles

- **Selection and organization first**: Capabilities help the Photographer
  find, group, compare, select, reject, rate, or continue reviewing Photos.
- **Human and machine interaction**: Design capabilities, observable results,
  and failure recovery for both the Photographer and their Agent.
- **One set of facts**: Clients share domain rules and durable state.
- **Camera-produced preview**: Preserve truthful source and detail information.
- **RAW-first independence**: Each Original File has independent Photo state.
- **Original ownership**: Original Files remain in place and unchanged except for separately confirmed [Permanent Deletion from Trash](library-management-trash.md).
- **Touch-native browsing**: Gestures coexist with visible and keyboard controls.
- **Bounded work**: Large Libraries do not require whole-Library transfer,
  client retention, or Preview generation before useful work.
- **Explicit effects**: Report applied, refused, partial, and unknown outcomes.
- **Clear organization**: File Locations describe physical placement; Albums
  express the Photographer's organization.
- **Focused core**: Prefer concrete photo operations over task infrastructure.

## Product Boundaries

Slipstream supports the bounded RAW development path in
[Photo Development](photo-development.md), without a general RAW module editor.
It offers one fixed Film Recipe, not general color-grading or retouching controls.
It is not a cloud backup service or multi-user digital asset management system,
and does not promise parity with a desktop photo editor.

It does not provide a built-in Agent, automatic aesthetic scoring, face
recognition, a semantic index, or a workflow engine. External Agents may combine
its capabilities without making those systems part of Slipstream.

Original writes, public sharing, authentication, and XMP interoperability require
their own contracts. Selection State must not be inferred from Rating or from a
task-specific Album. [The 0.1 support contract](0.1-support-and-release.md)
remains the authority for that release; the target vision does not add CLI or
editing claims to an already qualified release or imply publication.

## Direction

Broader search, comparison aids, and metadata interoperability may extend the
same Photo capabilities when concrete workflows justify them. Every extension
must preserve Original ownership, Preview Trust, inspectable effects, and use
through both appropriate human controls and programmatic operations.

The human/Agent/program relationship follows
[Mohist's philosophy of agent-native applications](https://github.com/suraciii/mohist/blob/master/docs/philosophy.md#chapter-3-agent-native-applications),
applied here to photo selection and organization.
