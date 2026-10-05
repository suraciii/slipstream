# Slipstream Context

## Library

**Photo Library**:
The Original Files known to Slipstream together with their Photos, Albums, and selection state. One state store owns one Photo Library.

**Library Folder**:
The configured filesystem directory whose supported descendant files belong to the Photo Library. The Folder defines discovery and read-only containment, not Photo or Original identity.
_Avoid_: Library Root, source root

**Photo**:
One independently managed supported Original File presented for browsing and selection. RAW and same-basename JPEG are independent Photos; their naming must not share decisions, Album membership, or Preview. A Photo keeps its identity when a supported operation changes its Original Location.
_Avoid_: image pair, RAW/JPEG group

**Content Fingerprint**:
A persisted SHA-256 digest of one Original File's complete content together with the size and modification time observed while hashing. A Content Fingerprint is exact-content evidence for Location Recovery, not identity; independent Originals may share one.
_Avoid_: checksum identity, hash as ID

**Location Recovery**:
The automatic or manual restoration of a remembered Original Location for an unavailable Original File: automatically when exact content evidence identifies one unambiguous candidate, manually with explicit confirmation when it cannot. Recovery never modifies or deletes an Original File.
_Avoid_: relink, repair, reimport

**Retire and Bind**:
An explicit recovery action that binds an unavailable Original File to an occupied destination Location. It is available only when the destination Photo is otherwise unreferenced with default decisions and no Album membership. It shows the record that will be retired and must not delete a filesystem file.
_Avoid_: replace, merge, overwrite

**Capture Time**:
The optional camera-recorded local date and time used to order Photos in the Photo Library. Capture Time comes from the Photo's own Original File and does not come from filesystem modification time. It does not determine Album membership order.

**Original File**:
A RAW or JPEG file owned by the Photographer and known to Slipstream under one stable identity. Slipstream must not rewrite it; only explicit Permanent Deletion from Trash may delete it. A supported Library expansion or Location Recovery must not create a new identity for it.

**Original Location**:
The relative directory and filename used to find an Original File beneath the current Library Folder. A Location is not Original File identity.
_Avoid_: Original path, file identity

**Original Folder**:
The Library Folder or one of its descendant filesystem directories, used to physically organize Original Files by their Original Locations. Slipstream presents it read-only. An Original Folder is not an Album and does not own Photo state.
_Avoid_: Album folder, virtual folder

**Library Expansion**:
A controlled replacement of the current Library Folder with an ancestor directory while preserving existing Original File and Photo identities.
_Avoid_: Rebase, relink, root migration

**Album**:
A Photographer-defined, explicitly ordered virtual group of Photos. One Photo may belong to multiple Albums. Album membership does not change an Original File or Original Location.
_Avoid_: Photo Set, Collection, Favorites

**Removed Photo**:
A Photo whose removal marker is set: it keeps its row, identity, Original Location, Rating, Album membership, and Selection State, and it leaves every normal Library source. Removing a Photo never modifies or deletes its Original File.
_Avoid_: permanently deleted Photo, purged Photo

**Removal marker**:
The millisecond a removal was confirmed, stored on the Photo row beside the operation that set it and never reused: every marker is strictly greater than every marker the Library assigned before it. It is what makes Restore a compare-and-set: a restore names the marker it was listed under, so it clears exactly that removal and never a newer one.
_Avoid_: deleted flag, timestamp field, tombstone

**Removal Operation**:
One confirmed attempt to remove a reviewed set of eligible Photos into Trash. It identifies its own effects separately from later Photo state, so retrying the attempt does not create another removal. Targets may come from a browser result or an explicitly identified Photo set.
_Avoid_: batch delete, purge, cleanup job

**Restore**:
The removal action that clears removal markers through a compare-and-set, returning Photos to every normal Library source with the facts they never lost. Restore never rescans, renames, moves, or rewrites an Original File, and it is not Location Recovery.
_Avoid_: undelete, relink, reimport

**Trash**:
The Library-wide view of Removed Photos that remain available for Restore or explicit Permanent Deletion. Trash retains Original Files in place and includes removals from all sessions. It is not operating-system trash.
_Avoid_: Recovery Area, Removed Photos listing as a separate destination

**Permanent Deletion**:
The separately confirmed deletion of a reviewed Original File belonging to a Photo in Trash. It cannot be undone by Slipstream and is distinct from Remove from Library and Restore.

## Metadata

**XMP Sidecar**:
A Photographer-owned external XMP file associated with a Photo and stored separately from its Original File. It carries supported metadata for exchange with other photo applications and does not define Photo identity or modify an Original File.
_Avoid_: sidecar when the kind is unclear, XMP Original, XMP Photo

**Sidecar Association**:
The rule that links one same-directory, same-basename XMP Sidecar to one Photo through its Original Location. A sole RAW Original owns the association; a sole JPEG Original owns it only when no RAW shares the basename. Multiple eligible owners remain ambiguous and have no writable association. A Location or ownership change invalidates prior metadata read evidence. The sidecar does not create a Photo or change its Original identity.
_Avoid_: sidecar identity, filename identity

**Sidecar Metadata**:
The standard descriptive, organizational, and rights metadata carried by an XMP Sidecar. External rating is separate from Library Rating; Selection State, Album membership, and Photo identity are not Sidecar Metadata.
_Avoid_: all metadata, EXIF state

**Read Metadata**:
Inspection of a Photo's embedded metadata and associated XMP Sidecar with their provenance, without changing Library decisions.

**Save Metadata**:
An explicit, checked creation or update of supported fields in a Photo's associated XMP Sidecar. It does not rewrite the Original or implicitly update Library decisions.

**Metadata Conflict**:
A refusal to save using obsolete metadata read evidence because the external content or its Photo association changed. A difference between external metadata and a Library value alone is not a Metadata Conflict.
_Avoid_: sync error, overwrite conflict

## Browsing and Selection

**Library Browser**:
The primary interface for viewing Photos from `All Photos`, one Original Folder, or one Album. It provides a progressively loaded Grid View and a focused Photo View.

**Destination**:
One addressable Library Browser view: a source with its view order and Selection State filter, showing either its Grid or one Photo. A Destination resolves against current Library facts when opened; it is not an archival snapshot. It is unrelated to the destination Location used by Retire and Bind.
_Avoid_: page route, permalink

**Resume**:
The explicit action that opens an Album's saved Photo from that Album's Grid. Resume is separate from opening the Album's Grid and is available only while a durable saved position exists.
_Avoid_: auto-reopen, restore position

**Grid View**:
The progressively loaded thumbnail view of the current `All Photos`, Original Folder, or Album source.

**Photo View**:
The focused view of one Photo with Preview, zoom, navigation, Selection State, Rating, and Album membership controls.

**Selection State**:
The keep decision for a Photo: `unflagged`, `picked`, or `rejected`. `Unflagged` means no Pick or Reject flag is recorded; `Picked` and `Rejected` are mutually exclusive review decisions. Selection State is independent of Rating and is not inferred from Sidecar Metadata in the current product boundary.

**Rating**:
An optional zero-to-five-star assessment owned by a Photo. Rating is separate from Selection State and may have a Sidecar Metadata representation.

## Preview

**Preview**:
The JPEG shown in Grid View or Photo View. A JPEG Photo's Preview comes from its own content; a RAW Photo's Preview comes from its own largest usable embedded JPEG. A sibling JPEG must not substitute for a RAW Preview.

**Preview Source**:
The content used for a Preview: `jpeg-original` or `raw-embedded-jpeg`. The legacy names `matching-jpeg` and `embedded-raw-jpeg` existed only in schema version 5, before independent Photos.

**Detail Review**:
Magnified Preview inspection for focus, motion, or expression. It is a Preview Zoom state in Photo View, not a separate mode. Its detail is limited by the Preview resolution.

## Development

**Processing Module**:
A service-internal integration boundary for one photo-processing capability and its input, parameter, and output contracts. Product and Agent language use **Processing Engine** for the capability at this boundary.
_Avoid_: plugin, extension, Film capability

**Processing Engine**:
A photo-processing capability such as darktable or standalone SpektraFilm, with admitted inputs, controls, outputs, and availability.
_Avoid_: Processing Module in product copy, engine process, desktop application, catalog

**Engine Module**:
An addressable editing operation inside a Processing Engine, such as darktable exposure or a SpektraFilm film operation. It owns the controls and qualification for that operation; it is not a separate Processing Engine.
_Avoid_: global control, darktable flag, stage

**Control**:
A product-defined value owned by an Engine Module, with a defined meaning, value rules, and reset behavior.
_Avoid_: native parameter, arbitrary module field

**Processing Step**:
An internal record of one selected Processing Engine invocation, with an explicit input and captured Engine Module controls. It is not a user-visible workflow stage or a cross-service pipeline object.
_Avoid_: workflow node, pipeline stage, latest result

**Processing Artifact**:
The immutable provenance-bearing result of a completed Export that another service may consume as an explicit input. It is not an Original File or a new Photo.
_Avoid_: latest result, temporary Preview

**Edit Recipe**:
The internal complete snapshot of one Photo's current Edit State and captured processing invocation, used for persistence and replay. It is not a cross-service plan or the Agent's daily editing object.
_Avoid_: edit history, pipeline, Preset, darktable sidecar

**Edit State**:
The current confirmed editing state of one Photo: its selected Processing Engine, input binding, controls, and revision. It is the Agent's daily editing object.
_Avoid_: recipe, session, workflow, engine history

**Film Recipe**:
A standalone SpektraFilm configuration that combines film stock, print paper, and processing choices. It is scoped to that engine and is not the whole Photo's Edit State.
_Avoid_: filter, film name as complete recipe, Photo recipe

A **Development Result**:
A scene-referred image produced by an admitted Processing Engine under an
Engine-owned input, Control, and output contract. The term describes the
result's image contract; it does not require a later Film invocation or define a
universal product output.
_Avoid_: Preview, developed Original

A **Film Result**:
A simulated photograph produced by an admitted standalone SpektraFilm
invocation from an input Artifact that meets that invocation's captured input
contract. It is
not a required successor to a Development Result.
_Avoid_: camera Preview, film Original

**Edit Preview**:
A bounded rendition of the current Edit State's processing result for editing and comparison. It is separate from the camera-produced Preview and is not a Processing Artifact.
_Avoid_: Preview when the kind is unclear

**Development Proxy**:
A service-owned, bounded scene-linear Development Result derived from a validated Original and retained as a rebuildable stand-in while that Original is unavailable. It carries the source revision, staged-byte evidence, approved profile, processing bundle, pipeline identity, and artifact identity. It is never an Original, a new Photo, or an Export source.

**Export**:
A separate explicit execution that produces a downloadable Processing Artifact
from the confirmed Edit State, together with its completion outcome. It does
not create or modify an Original File.
_Avoid_: save Original, imported Photo
