# Library Management: Read and Save Metadata

Photographers use descriptive metadata, keywords, ratings, and attribution across
photo applications. They and their Agents need to read those facts and save
explicit changes without damaging Originals or discarding another application's
metadata. Read and Save form one complete capability. A read-only implementation
or a Rating-only implementation does not satisfy this specification.

## Capability Boundary

Read Metadata must inspect one identified Photo's embedded metadata and associated
XMP Sidecar. Save Metadata must create or update that Photo's adjacent XMP Sidecar
with explicitly supplied changes. Neither operation must rewrite an Original File.
A downloadable export alone must not be reported as Save Metadata.

The external Agent owns choosing Photos, comparing values, and composing actions.
Slipstream must not introduce a synchronization planner, automatic import, or a
separate Adopt capability. Existing Photo Rating operations remain authoritative
for library Rating. Reading or saving external metadata must not implicitly change
Rating, Selection State, Albums, Capture Time ordering, or an Edit Recipe.

## Field Coverage

The following fields together define the supported capability. The same coverage
must be available for reads and Sidecar saves except for the explicitly read-only
capture facts. Names identify standard properties, not a new command language.

### Description and Organization

- Title: `dc:title`, language alternatives.
- Description or caption: `dc:description`, language alternatives.
- Headline: `photoshop:Headline`, text.
- Keywords: `dc:subject`, an unordered set of text values.
- Label: `xmp:Label`, text. It must not be interpreted as a fixed color or a
  Selection State; external applications may assign different meanings.
- External rating: `xmp:Rating`, the standard values -1 or a real number from
  0 through 5. Raw presence must distinguish missing from explicit 0, even though
  the XMP standard interprets missing Rating as unrated. Read must label that
  default as inferred, not a stored value. -1 must not set Selection
  State. A fractional external rating must not be silently rounded into the
  Library's integer Rating. Adopting a compatible value uses the existing
  explicit Rating operation; incompatible values require a caller's decision.

### Attribution and Rights

- Creators: `dc:creator`, an ordered list of names.
- Creator job title: `photoshop:AuthorsPosition`, text.
- Credit line: `photoshop:Credit`, text.
- Source: `photoshop:Source`, text.
- Copyright notice: `dc:rights`, language alternatives.
- Rights usage terms: `xmpRights:UsageTerms`, language alternatives.
- Copyright status: `xmpRights:Marked`, optional boolean. Missing must not be
  presented as false or as permission to use the Photo.
- Rights information URL: `xmpRights:WebStatement`, text containing a Web URL. Reading metadata must
  not fetch this address or treat its contents as permission or instructions.

### Capture Facts

Read Metadata must expose the Photo's available capture date/time with recorded
subseconds and offset, camera make/model, lens model, image dimensions,
orientation, exposure time, aperture, ISO, and focal length. These are read-only
facts from the Original, with their standard EXIF identifiers, units, and source.
Missing offsets must not be invented. The existing
[Capture Time rules](photo-library.md#capture-time) own Library ordering.

Sidecar representations of these facts must be distinguishable from Original
facts and must not replace them in Library ordering or Preview behavior. Save
must preserve these representations, but must refuse attempts to edit them.

### Coverage Limits

Read must report whether each supported field is present, absent, invalid, or
unavailable, and which fields are writable. Unsupported properties must not be
reported as absent supported properties. Save must reject unsupported fields
before making any change. This capability does not include arbitrary tag editing,
GPS editing, face regions, structured IPTC Extension records, RAW development
settings, or a private Selection State mapping. Those properties must survive
saves to other fields. Their presence alone must not prevent reading supported
fields.

## Sources and Association

Read must return embedded and Sidecar values with separate provenance. For a
supported descriptive field, the displayed external value must use the Sidecar
property when present, otherwise embedded XMP, otherwise its defined IPTC IIM
counterpart. The underlying values and their differences must remain inspectable.
Supported IIM counterparts are Title (2:5), Description (2:120), Headline
(2:105), Keywords (2:25), Creators (2:80), Creator job title (2:85), Credit
(2:110), Source (2:115), and Copyright notice (2:116), using the IPTC-defined
encoding and multiplicity. IIM text has no language map; Read must identify this
limitation instead of inventing translated alternatives. Other writable fields
have no IIM fallback in this capability. Conflicting embedded EXIF attribution
must remain preserved; it must not silently replace the declared XMP/IIM values.
Capture facts must retain
their Original authority. Library Rating must be labeled separately.

An invalid or unreadable Sidecar property must not silently fall back to embedded
metadata and appear valid. A valid empty Sidecar value must suppress fallback.
Removing a Sidecar property must reveal the embedded fallback, if any. Read must
make this distinction visible.

[Sidecar Association](../CONTEXT.md#metadata) owns eligibility: one same-directory,
same-basename XMP Sidecar belongs to the sole eligible RAW, or to the sole JPEG
only when no RAW shares its basename. RAW and same-basename JPEG must remain
independent. The JPEG must not read or overwrite the RAW's Sidecar. Multiple
eligible Originals or candidate Sidecars must be reported as ambiguous; Save
must refuse them. A Photo without an eligible association must remain readable
from its Original and must report that Sidecar saving is unavailable.

A new Sidecar must use the eligible Original's basename with `.xmp` in its own
directory. Existing candidates must be inspected before creation; an existing
`.XMP` must not result in a second Sidecar. Clients must identify Photos, not
supply arbitrary filesystem destinations.

Original Location, Original identity, or association changes must invalidate prior
read evidence. A retained Sidecar whose known owner was removed by Permanent
Deletion or moved by Location Recovery must not silently transfer to another
Photo. Read must identify this unresolved association and Save must refuse it;
external correction followed by a fresh inspection may establish an unambiguous
association. Slipstream must not move or rename the Sidecar automatically.

## Read Behavior

Read must have no metadata mutation effects. It must provide the Photo identity,
Original Location, association status, supported values and provenance, validation
problems, and evidence sufficient to make a checked Save. An absent Sidecar is a
valid observed state, distinct from unreadable or malformed content.

A bounded read must distinguish a resource limit from missing metadata. Metadata
failure must not make an otherwise browsable Photo unavailable. Strings are data,
not instructions, executable content, or markup to run. Parsing must not retrieve
external resources. A Removed Photo may be read; Save requires Restore first.

## Save Behavior

Save must receive one Photo, the observed read evidence, and explicit field
changes. It must not copy all embedded metadata or all Library facts implicitly.
Omitted fields must remain unchanged. Save must distinguish setting a value,
setting a valid empty value, and removing the Sidecar property. A missing property
must never implicitly clear a Library value.

- A keyword change replaces the explicitly supplied set; ordering is not
  meaningful. Exact duplicate values must collapse without case folding.
- A creator change replaces the supplied ordered list; order must survive.
- Language-alternative changes must name the affected languages, including
  `x-default` where used. Unmentioned alternatives must survive. Removing one
  alternative must not delete the others. Changes must preserve standard XMP
  language-alternative validity. If a request would require an unrequested
  language change, Save must refuse with the required correction rather than
  modify that language silently.
- Empty text, an empty list, false, and numeric zero must not be conflated with
  property removal. Numeric and boolean fields do not accept empty text.
- All requested fields must be valid before Save changes any field. One Photo's
  changes must succeed together or leave the prior Sidecar usable and unchanged.

Save must preserve the meaning, types, values, and structure of unmodified
properties, including unknown namespaces and language variants. Byte-for-byte
formatting preservation is not required. If preservation cannot be guaranteed,
Save must refuse instead of rebuilding a reduced Sidecar. It must not replace
malformed XML with a fresh document or erase another application's edit settings.

Save must verify that its observed Original and association still apply and that
the Sidecar has not changed, disappeared, or appeared since Read. Conflicts must
leave the newer content intact and require fresh inspection and a new decision.
The same rule applies to creation: observed absence is not permission to replace
a file that appeared later. Save must not offer an unchecked force overwrite.

Saving explicit external values must not depend on a mutable Library Rating
remaining equal to those values. A caller exporting a Library Rating chooses its
observed value; Save reports exactly that value without claiming both stores are
synchronized. No durable synchronization baseline is required.

## Results and Failure Recovery

A successful Save must identify the Photo, Sidecar, affected fields, verified
saved values, and fresh read evidence. An unchanged request may report no change
only after checking the current content. It must not imply a historical write.

Failures must distinguish invalid input, unsupported field, missing Photo,
unavailable Original, unresolved association, Removed Photo, malformed or
unreadable metadata, stale evidence, permission denial, resource limit, and
storage failure. A missing Sidecar is not a failure when safe creation is allowed.

After connection loss or an interrupted save whose effect cannot be confirmed,
the caller must receive an unknown outcome. Fresh Read establishes current
content, not which caller wrote it. Clients must not retry blindly or report that
nothing happened. A new Save requires fresh evidence. No operation ledger or
exactly-once delivery is implied.

Two concurrent saves based on the same evidence must not silently lose one
another's changes. Restore, removal, Permanent Deletion, or Location Recovery
must not let Save target another Original or a retired association. A successful
save may precede a later removal; that ordering must not be described as both
operations operating on an unchanged Photo.

## Human and Agent Access

The Web must allow inspecting sources, editing supported writable fields, reviewing
pending changes, explicitly saving, and refreshing after conflicts. It must show
Sidecar changes separately from Library Rating changes and must not promise an
external save before confirmation. No specialized import or synchronization
wizard is required.

The CLI must expose the same read and checked-save capability, discoverable field
coverage, limits, and structured outcomes. One-Photo operations are sufficient:
Agents may query and compose them. The application must not silently split,
expand, or automatically continue a multi-Photo task. CLI syntax and examples
belong in the [CLI Reference](cli-reference.md), not this specification.

## Original Safety and Supported Operation

Save grants permission only to create or update the associated XMP Sidecar. It
must not modify, delete, rename, or move Originals, sibling files, or directories.
A deployment with read-only Sidecar access must still support Read and must
report Save as unavailable. Complete delivery must include a supported deployment
that can save adjacent Sidecars while preserving the Original safety boundary;
read-only operation alone does not satisfy delivery.

Library expansion, Location Recovery, rescan, and restart must not apply Sidecar
values to Library decisions or reuse obsolete save evidence. Backups must identify
Sidecars as Photographer-owned files separate from application state. Restoring
application state must not roll back external Sidecars. Fresh inspection after
restore must establish current external content before any save.

## Acceptance

- Create, read, update, clear, remove, and reread every writable field in the
  coverage above. Exercise Unicode, multiple creators, keywords, language
  alternatives, zero, false, fractional rating, -1, and absent values.
- Read Original capture facts and conflicting Sidecar representations without
  changing Library ordering, Rating, Selection State, Albums, or Previews.
- Exercise RAW-only, JPEG-only, RAW plus JPEG, two RAWs sharing a basename,
  duplicate Sidecars, retained orphan Sidecars, and moved Originals. Only the
  unambiguous owner may write; the sibling's facts must remain independent.
- Verify new Sidecar creation, safe updates, preservation of unknown structured
  properties, absent-versus-empty fallback, malformed metadata, permissions,
  bounded parsing, and Original bytes unchanged.
- Change or replace the Sidecar between Read and Save, race two saves, interrupt
  a save, and restart. Observe a confirmed complete result, a refusal preserving
  prior content, or explicit uncertainty; never a false success.
- Read in the CLI, save chosen fields, and inspect them in the Web; do the reverse.
  External metadata must remain separate from Library decisions in both paths.
- Verify interoperability using Adobe Lightroom Classic and ExifTool with recorded
  versions and non-private fixtures. Exercise data written externally and values
  saved by Slipstream. For JPEGs, distinguish an external tool's embedded-only
  workflow from Sidecar support; do not claim automatic JPEG Sidecar consumption.
  Properties an application does not expose must retain valid standard values
  and pass independent inspection. Record per-field compatibility rather than
  claiming universal application support.

## Standards

Standard property semantics follow Adobe's
[XMP Part 1](https://github.com/adobe/XMP-Toolkit-SDK/blob/main/docs/XMPSpecificationPart1.pdf),
[XMP Part 2](https://github.com/adobe/XMP-Toolkit-SDK/blob/main/docs/XMPSpecificationPart2.pdf),
and the [IPTC Photo Metadata Standard](https://www.iptc.org/std/photometadata/specification/IPTC-PhotoMetadata).
These define representation, not permission to change a Photo's Library decisions.
