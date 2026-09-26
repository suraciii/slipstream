# Library Management: Standard Metadata Read and Save

Read and Save Metadata crosses two ownership boundaries: Original Files and
Photographer-owned XMP Sidecars. The Product Spec in
[`docs/library-management-metadata.md`](../docs/library-management-metadata.md)
is authoritative for supported fields and user-visible behavior. This design
makes that capability durable without turning the Library state store into an
external metadata cache.

## Design Drivers

- Original Files are Photographer-owned and are never rewritten by Save.
- A Photo is the identity boundary. A same-basename RAW and JPEG remain
  independent, and a Sidecar must not cross that boundary.
- Read evidence must reject a stale Save rather than overwrite another
  application's change.
- Unknown XMP properties, namespaces, structures, and language alternatives
  must survive a Save of supported fields.
- A malformed or unsupported Sidecar must remain visible as a problem; Save
  must not replace it with a reduced document.
- CLI and Web need one semantic operation, not two implementations with
  different fallback or conflict behavior.
- Metadata parsing is bounded and must not follow URLs, execute values, or
  depend on a helper executable being installed.

## Model

A `MetadataTarget` names one current Photo, its Original Location and kind, and
its Sidecar Association. The server derives it from the published Library
snapshot. The core metadata service then revalidates the Original through the
confined Library root before reading or saving.

A Read Metadata result contains:

- the Photo identity and Original Location;
- Association status and the candidate Sidecar path, when one is eligible;
- separate Original embedded, Sidecar, and IPTC IIM field values with
  provenance;
- capture facts from the Original, marked read-only;
- Library Rating as a separate Library fact; and
- an evidence token containing the Original revision, the persisted Photo and
  association generation, a startup-bound session epoch, and the Sidecar
  revision. Missing Sidecar is represented by an explicit absent revision, not
  by an empty file. Every removal, restore, ownership, identity, or Location
  transition increments the persisted generation, and each process start
  mints a new session epoch, so recovery back-and-forth and restart cannot
  make old evidence pass.

Supported writable fields are represented as typed patches. A patch has one of
`set`, `clear`, or `remove`. `clear` is a valid empty value and remains a
property in XMP; `remove` deletes the Sidecar property and reveals fallback
content. Lists preserve order where the field is ordered and collapse exact
duplicates only for Keywords. Language alternatives name each language being
changed; an omitted language is not changed. Language tags compare by
canonical BCP-47 matching, duplicate alternatives within one request are
rejected, and `x-default` is addressed only when explicitly named. When
preserving standard language-alternative validity would require changing a
language the request did not name, Save refuses with the exact required
correction.

The supported field set is exactly the Product Spec's writable set:
`dc:title`, `dc:description`, `photoshop:Headline`, `dc:subject`,
`xmp:Label`, `xmp:Rating`, `dc:creator`, `photoshop:AuthorsPosition`,
`photoshop:Credit`, `photoshop:Source`, `dc:rights`, `xmpRights:UsageTerms`,
`xmpRights:Marked`, and `xmpRights:WebStatement`. Original capture facts are
read-only. Selection State, Album membership, Edit Recipe, and Library Rating
are not Sidecar fields.

## Semantics

### Association and read ownership

The server derives same-directory, same-basename candidates from the current
published Originals. One RAW owns the association. A JPEG owns it only when no
RAW shares the basename. Multiple eligible Originals, duplicate `.xmp`/`.XMP`
files, and unresolved retained associations are ambiguous or unavailable and
cannot be written. An ineligible JPEG still reads its own embedded metadata but
never reads or writes the RAW's Sidecar.

Association ownership outlives the files it named. Permanent Deletion and
Location Recovery record a durable tombstone for the retired Original's known
Sidecar association, keyed by the retired Original or Photo identity and the
prior association. A retained Sidecar whose owner was removed or moved stays
unresolved until an external correction followed by a fresh inspection
establishes a new unambiguous association; a later eligible Original at the
same basename must not silently inherit the Sidecar. The tombstone clears only
through that explicit fresh-inspection transition.

Read opens the Original through `LibraryRoot` and reads bounded embedded XMP,
EXIF capture facts, and IPTC IIM data. Sidecar bytes are opened only through
confined same-directory operations. A valid empty Sidecar property suppresses
fallback. An invalid Sidecar property is reported invalid rather than silently
falling back. The effective value is Sidecar, then embedded XMP, then the
specified IIM counterpart; the underlying source values remain in the result.

The parser accepts UTF-8 XML, validates element nesting and namespace structure,
and limits packet, node, string, array, and language-entry sizes. It stores
unknown XML nodes and attributes in a lossless semantic tree. Serialization may
change whitespace or attribute order, but it preserves unknown values and
structure. The reader never fetches a `WebStatement` URL.

Embedded extraction coverage is explicit per Original format: where bounded
XMP, EXIF, and IPTC IIM segments are read from JPEG and from each supported
RAW extension, and how each missing or unreadable segment reports per-field
`unavailable` rather than silently narrowing coverage. The capture source
model exposes every required capture fact, including image dimensions and
orientation, with its EXIF identifier, unit, and source.

The preservation model covers the RDF/XML forms a Sidecar may contain:
`rdf:resource` attributes, `rdf:parseType="Resource"`, nested structures,
Bag/Seq/Alt containers, namespace redeclarations, and multiple
`rdf:Description` elements. The round-trip invariant is semantic: an
unmodified property must keep its values, types, and structure. A construct
the model cannot represent losslessly makes Save refuse before any change
rather than reserialize a reduced tree.

### Save transaction

Save is admitted only for an active Photo with an available Original and an
eligible, unambiguous Sidecar Association. The request must include the exact
Read evidence and explicit changes.

The write runs inside one exclusive metadata editing session. Admission of
the session is the safety boundary, not the revision check: an ordinary
`stat`-compare-then-`rename` sequence has an unavoidable window in which an
external writer can publish newer content that the rename then destroys, for
updates and for creations alike. Before a session is admitted, every external
writer must be quiesced: external applications are closed or drained, and
already-open writable descriptors and mappings do not survive admission. The
session owns a fresh epoch; a new session or a restart invalidates all prior
evidence. Sessions are sequential by design; concurrent unrestricted external
editing is not a supported environment for Save.

Inside the session, before writing, core rechecks:

1. the Original path, inode, size, modification time, and a content digest
   when the facts are inconclusive;
2. the Association candidate set and selected Sidecar name; and
3. the Sidecar's exact observed content, including the explicit missing state.

Every requested patch is validated before a document is changed. Existing
Sidecar XML is parsed and edited in memory. A missing Sidecar is created only
when the observed evidence also said missing, using
`renameat2(RENAME_NOREPLACE)`; `EEXIST` is a conflict that preserves the
racing file. An existing malformed Sidecar is refused. The output is staged
under an exclusive unpredictable temporary name, flushed and synced, then
atomically renamed. On backends without the required primitives, Save fails
closed instead of falling back to an unchecked rename. A failed write leaves
the previous Sidecar intact.

The application holds its publication/mutation lock while the checked Sidecar
write runs. That serializes Slipstream writes and keeps the Web publication
from claiming a result before the write is confirmed. A successful write
performs a fresh read and returns the verified values and new evidence. A
post-write verification failure is `outcome_unknown`; it is never reported as
no change.

The database remains the owner of Library decisions and Photo identity. It does
not cache Sidecar fields. Restart therefore requires a fresh Read and cannot
reuse stale Save evidence. Scan, Restore, Permanent Deletion, and Location
Recovery do not apply Sidecar values to Library decisions.

### Operating environment and write authority

The Web process keeps its read-only Original access. Adjacent Sidecar writes
are executed by one narrow Sidecar broker: a separately confined component
that receives bounded, association-authorized operations, derives every
destination from an admitted Photo and association, and owns staging and
commit. Callers cannot name arbitrary filesystem destinations, and the broker
exposes no general writable directory capability. The broker is a trusted
component inside the operator boundary; protecting Originals from a
compromised broker requires an additional enforced filesystem or ownership
policy and is not claimed by this design.

The supported deployment requires an identified operator actor who can
quiesce external writers for each editing session and keep the Original and
association topology stable while a checked operation commits. A deployment
with read-only Sidecar access still supports Read and reports Save as
unavailable. Making the Library bind writable for the Web process instead of
deploying the broker is rejected: it would trade an enforced kernel protection
boundary for an in-process promise.

### API boundary

The shared server operations are:

- `GET /api/photos/{id}/external-metadata` for Read Metadata;
- `POST /api/photos/{id}/external-metadata` for checked Save Metadata.

The existing review-capture endpoint remains available for the Photo View's
small capture display. The new endpoint is the complete metadata contract.

The CLI exposes the same operations as `photos metadata PHOTO_ID` and
`photos metadata-save PHOTO_ID --input FILE`. The save input contains one
`evidence` object and explicit `changes`; it cannot name a filesystem path.
Web uses the same read result, displays provenance and pending patches, and
refreshes after a conflict. Neither path creates an import or synchronization
workflow.

The wire contract is normative and shared: one serializable schema defines the
read result, every field patch shape, language maps, the evidence token, and
each field's `present`, `absent`, `invalid`, and `unavailable` state with its
writable flag. One error mapping defines the Product Spec's failure categories
— invalid input, unsupported field, missing Photo, unavailable Original,
unresolved association, Removed Photo, malformed or unreadable metadata, stale
evidence, permission denial, resource limit, storage failure, and unknown
outcome — as stable codes for HTTP and CLI alike. Web and CLI send and report
the same shapes; neither invents a private subset.

## Options

### Option A: delegate to ExifTool or Exiv2

A subprocess or system library would provide broad format support quickly.
It is rejected because the supported deployment does not require either tool,
process execution would complicate confinement and resource limits, and helper
versions would make preservation and failure semantics deployment-dependent.

### Option B: parse and edit a bounded XMP tree in Slipstream — selected

A small in-process parser handles the declared fields and retains unknown
nodes, attributes, namespaces, and values. The Original parser supplies the
existing bounded EXIF facts, while a narrow IPTC IIM reader supplies the
specified fallback fields. This keeps ownership, limits, and checked atomic
writes in one implementation. It costs more field-specific code, but that
cost is explicit and testable against standards fixtures.

### Option C: rebuild a new Sidecar from the declared fields

This is simpler to serialize but would discard unknown namespaces, structured
properties, and another application's edit settings. It violates the Product
Spec's preservation rule and is rejected.

### Option D: stat-compare-then-rename without an exclusion authority

This is the inherited draft algorithm. It is rejected: a separate process can
publish newer Sidecar content between the revision check and the rename, and
the rename destroys it. The violation cannot be repaired by post-write
verification, and the same window lets a creation overwrite a Sidecar that
appeared after the observed absence.

### Option E: advisory locks or file leases as the only exclusion

`flock`, `fcntl` locks, and `F_SETLEASE` are rejected as the sole boundary.
They are advisory or open-based; a writer that does not participate bypasses
them, and a read lease on the old inode does not prevent replacement of the
directory entry. They remain useful only inside an environment where every
writer is already forced to participate.

## Verification

Permanent tests must prove observable behavior rather than parser wiring:

- every declared writable field can be set, cleared, removed, read back, and
  inspected through both CLI and Web;
- language alternatives, ordered Creators, unordered Keywords, Unicode, zero,
  false, fractional Rating, `-1`, and missing values retain their semantics;
- embedded, Sidecar, and IIM provenance plus absent-versus-empty fallback are
  visible;
- RAW/JPEG association cases, duplicate Sidecars, orphaned Sidecars, moved
  Originals, and unresolved associations refuse unsafe writes;
- unknown XMP structures survive, malformed and permission failures preserve
  prior content, concurrent saves produce one success and one conflict, and a
  restart requires fresh evidence;
- Original bytes, Library Rating, Selection State, Albums, Capture Time
  ordering, and Preview state remain unchanged;
- session admission excludes or refuses already-open external writers, and a
  Save attempted outside an admitted session is refused;
- a racing `.XMP` creation, an equal-length in-place external edit with a
  preserved modification time, and an Original replacement during a session
  each produce a precommit refusal that preserves newer content;
- injected failures before, during, and after the commit point leave either
  the prior Sidecar intact or an explicitly unknown outcome, never a false
  success or a rollback over a later writer; and
- direct write, truncate, chmod, unlink, rename, and replace attempts against
  Originals from the Web UID fail, and the broker derives destinations only
  from admitted associations; and
- ExifTool and Lightroom Classic fixtures are inspected with recorded tool
  versions when those tools are available, including the ownership and
  permission state after a Slipstream save that the next external edit
  depends on. Unsupported or embedded-only JPEG workflows are recorded per
  field rather than generalized into a universal compatibility claim.
