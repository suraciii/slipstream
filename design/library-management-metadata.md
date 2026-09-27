# Library Management: Standard Metadata Read and Save

Read and Save Metadata crosses two ownership boundaries: Original Files and
Photographer-owned XMP Sidecars. The Product Spec in
[`docs/library-management-metadata.md`](../docs/library-management-metadata.md)
is authoritative for supported fields and user-visible behavior. This design
makes that capability durable without turning the Library state store into an
external metadata cache, and it defines the only environment in which Save is
admitted.

## Design Drivers

- Original Files are Photographer-owned and are never rewritten by Save.
- A Photo is the identity boundary. A same-basename RAW and JPEG remain
  independent, and a Sidecar must not cross that boundary.
- Read evidence must reject a stale Save rather than overwrite another
  application's change, including across restarts and Location changes.
- Unknown XMP properties, namespaces, structures, and language alternatives
  must survive a Save of supported fields.
- A malformed or unsupported Sidecar must remain visible as a problem; Save
  must not replace it with a reduced document.
- CLI and Web need one semantic operation, not two implementations with
  different fallback or conflict behavior.
- Metadata parsing is bounded and must not follow URLs, execute values, or
  depend on a helper executable being installed for Read.
- Concurrent external writers cannot be coordinated by advisory mechanisms on
  an ordinary writable filesystem. Save therefore runs only inside an enforced
  exclusive save session, defined below.

## Model

A `MetadataTarget` names one current Photo, its Original Location and kind, and
its Sidecar Association. The server derives it from the published Library
snapshot. The metadata service revalidates the Original through the confined
Library Folder before reading or saving.

### Read evidence

A Read Metadata result contains:

- the Photo identity and Original Location;
- Association status and the candidate Sidecar path, when one is eligible;
- separate Original embedded, Sidecar, and IPTC IIM field values with
  provenance;
- capture facts from the Original, marked read-only;
- Library Rating as a separate Library fact; and
- an evidence token.

The evidence token binds the observed state to the Library lifecycle, the
server instance, and the bounded Sidecar content:

- the Original facts (device, inode, size, integer modification seconds and
  nanoseconds);
- the Sidecar revision facts and a digest of the bounded Sidecar content, with
  explicit absent and unavailable states; unavailable evidence never permits
  Save. Statistics alone do not prove unchanged content,
  because an external tool can edit a Sidecar in place while preserving its
  size and modification time;
- the Association generation: a persisted monotonic counter for the Photo that
  every removal, restore, Permanent Deletion, ownership transition, Location
  change, or Library expansion increments; and
- the server instance epoch: a random identifier generated at startup from
  state that survives restarts of the operating system process.

Save accepts a token only when every component still matches. A relocated
Original that later returns to the same Location has a higher Association
generation, so recovery cannot reuse an old token, and no token survives a
server restart.

### Sidecar association record

The Library state store persists one Sidecar Association record per retained
Sidecar: its relative path, the owning Photo, the Sidecar revision last
observed, and a state of `active` or `retained-orphan`. When the owning
Original leaves its Location through Permanent Deletion or a recovery
relocation, the record becomes `retained-orphan` instead of being deleted.

A later eligible Original at the same basename does not inherit the Sidecar.
Read reports the Association as unresolved while a `retained-orphan` record
matches the candidate, and Save refuses it. The state clears only after a fresh
Read observes the Sidecar following an operator's external correction (the
Sidecar changed, disappeared, or was explicitly re-associated through that
inspection); Slipstream never moves, renames, or deletes the Sidecar itself.

### Patches

Supported writable fields are represented as typed patches. A patch has one of
`set`, `clear`, or `remove`. `clear` is a valid empty value and remains a
property in XMP; `remove` deletes the Sidecar property and reveals fallback
content. Lists preserve order where the field is ordered and collapse exact
duplicates only for Keywords. Language alternatives name each language being
changed; an omitted language is not changed.
Language names must be well-formed RFC 5646 tags, including private-use and
grandfathered tags; `x-default` retains its XMP meaning. Read and Save use the
same validator. Subtag registry membership is not checked.

The supported field set is exactly the Product Spec's writable set:
`dc:title`, `dc:description`, `photoshop:Headline`, `dc:subject`,
`xmp:Label`, `xmp:Rating`, `dc:creator`, `photoshop:AuthorsPosition`,
`photoshop:Credit`, `photoshop:Source`, `dc:rights`, `xmpRights:UsageTerms`,
`xmpRights:Marked`, and `xmpRights:WebStatement`. Original capture facts are
read-only. Selection State, Album membership, Edit Recipe, and Library Rating
are not Sidecar fields.

## Semantics

### Association and read ownership

The server derives same-directory, same-basename candidates from actual
confined directory entries, including supported Originals not yet scanned.
One regular RAW owns the association. A regular JPEG owns it only when no
regular RAW shares the basename. Multiple eligible Originals, duplicate
`.xmp`/`.XMP` files, and unresolved `retained-orphan` records are ambiguous or
unavailable and cannot be written. The extension comparison is case-insensitive;
the directory and basename retain their exact spelling. An ineligible JPEG
still reads its own embedded metadata but never reads or writes the RAW's Sidecar.
When a newly eligible RAW displaces a recorded JPEG owner, the state owner
transfers the association atomically and advances the displaced Photo's
association generation. An unchanged retained orphan still blocks Save.

Metadata work runs under the state owner's serialization with scan, recovery,
removal, and restore. One queued Save operation covers inspection, evidence
comparison, supervisor admission and publication, and association recording;
the operation continues even if its caller disconnects. Remove either commits
before Save, causing `photo_removed`, or after the admitted Save settles.
Read records its observation in the same operation and returns the committed
association generation. Library Rating remains a separate fact.
The native-work permit belongs to the queued inspection or Save, then to any
remaining supervisor status check. Disconnecting a caller does not release
capacity while that work is still running.

Metadata associations migrate from the published Film schema v10 to v11.
The migration preserves existing Photos, Library decisions, Albums, Trash,
Edit Recipes, and Export records; it initializes each association generation
to one without importing Sidecar values.

Read opens the Original through `LibraryRoot` and reads bounded embedded XMP,
EXIF capture facts, and IPTC IIM data. Sidecar bytes are opened only through
confined same-directory operations. A valid empty Sidecar property suppresses
fallback. An invalid Sidecar property is reported invalid rather than silently
falling back. The effective value is Sidecar, then embedded XMP, then the
specified IIM counterpart; the underlying source values remain in the result.

### Embedded extraction matrix

Read reports each supported source per Original kind:

| Kind           | EXIF capture facts                        | Embedded XMP                               | IPTC IIM                    |
| -------------- | ----------------------------------------- | ------------------------------------------ | --------------------------- |
| JPEG           | APP1 Exif segment                         | APP1 `http://ns.adobe.com/xap/1.0/` packet | APP13 Photoshop IIM segment |
| TIFF-based RAW | IFD0 and Exif IFD                         | IFD0 tag `0x02BC` (XMP packet)             | IFD0 tag `0x83BB` (IIM)     |
| Non-TIFF RAW   | LibRaw fallback, as used for Capture Time | unavailable by kind                        | unavailable by kind         |

Dimensions are selected per axis: the Exif IFD's `PixelXDimension` and
`PixelYDimension` first, the primary IFD's `ImageWidth` and `ImageLength`
second, and the JPEG's `SOF` marker last, with the reported identifier naming
the selected source. A higher-priority source that is present but invalid,
duplicate, zero, or over its parse limit blocks fallback to the lower-priority
source and reports that state. Orientation comes from the primary IFD only. A
kind without a source reports `unavailable` with the kind, never as absent.
The reader never fetches a `WebStatement` URL.
Scalar TIFF capture values require exactly one value of the declared type.
JPEG frame dimensions accept the standard SOF marker families and require a
complete component table. Malformed shapes report invalid dimensions under
the same source precedence rules.

### XMP document model and preservation

The parser accepts UTF-8 XML and limits packet, node, string, array, and
language-entry sizes. The document model retains, losslessly and semantically:

- every namespace declaration, element, and attribute of `x:xmpmeta` and its
  descendants, including unknown namespaces;
- multiple `rdf:Description` nodes, their `rdf:about` values and shorthand
  property attributes;
- property value forms: element text, `rdf:resource` references,
  `rdf:parseType="Resource"` structures, and nested `rdf:Bag`, `rdf:Seq`, and
  `rdf:Alt` containers with `rdf:li` items and `xml:lang` qualifiers;
- namespace redeclarations on any element.

Serialization may change whitespace and attribute order; it may not change
names, values, structure, container types, qualifiers, or language tags. Any
construct outside this model — `rdf:ID`, `rdf:nodeID`, XPointer `rdf:about`,
`rdf:parseType` values other than `Resource`, shorthand attributes the model
does not represent, or entity declarations — makes the Sidecar
`unpreservable`: Read still reports supported fields, and Save refuses before
any mutation rather than rebuild a reduced document.

Read-only parsing retains the preservation error while inspecting unrelated
supported fields. It never resolves external entities or expands declared
entities. Mutation and serialization reject a document with a preservation
error. Capture representations retain present, absent, and invalid states for
all thirteen capture fields, separately from the Original facts.

### Language alternatives

`dc:title`, `dc:description`, `dc:rights`, and `xmpRights:UsageTerms` are
language-alternative properties. Language tags are compared by their canonical
BCP 47 form: case-insensitive, with the canonical casing of `x-default`.
Duplicate alternatives in one existing property are reported invalid. A patch
names the exact languages it changes, including `x-default`.

When applying a patch would require changing an unrequested language — for
example, updating the alternative that `x-default` mirrors, when XMP validity
requires the default to follow it — Save refuses and lists every language that
would have to change. Removing one alternative never removes the others.

### Save session and exclusive environment

Save is admitted only when the deployment provides an enforced exclusive save
session. A session has five steps, all under the deployment's control:

1. **Quiesce.** Fence the managed file service in a fixed order: stop its
   listener or refuse new external sessions, let in-flight requests drain
   within a bounded wait, then terminate every service process in its control
   group — signaling only the main process leaves workers alive — and observe
   the control group's emptiness as proof. The service must run with
   client-caching and handle-replay features that could re-apply a write after
   the restart disabled and qualified: no oplocks, SMB leases, directory
   leases, or durable or persistent handles for the managed service. If the
   service does not reach a verified stop within the bounded timeout, Save
   refuses before any mutation and reports Save unavailable with the reason.
2. **Final validation.** Recheck the evidence token: Original path, inode,
   size, and modification time; the Association candidate set, generation, and
   Sidecar name; and the Sidecar revision facts and bounded content digest,
   including the explicit missing state. Validate every requested patch
   against the parsed document. Validation happens inside the session, so no
   external writer can change the Sidecar between this comparison and
   publication.
3. **Publish.** Stage the new document in the same directory under a fresh
   exclusive temporary name, write and flush it, and sync it. Then atomically
   rename it over the observed Sidecar and sync the parent directory so the
   entry itself is durable. A missing Sidecar is created only when the
   evidence also said missing; a raced creation is detected by
   `RENAME_NOREPLACE` semantics and refused. A failed write before publication
   removes only this session's temporary file and leaves the previous Sidecar
   intact.
   The publisher derives ownership again through one retained parent directory
   descriptor and requires the current facts and digest to match the evidence.
   An update retains the observed Sidecar filename, including extension case.
   Before invoking the helper, the supervisor commits a root-owned runtime
   publication record containing a fresh lease token, the exact temporary name,
   and the parent device and inode. The helper derives that same temporary name
   from the inherited lease and opens it exclusively. Recovery may remove a
   staged publication only when that record is present, the recovery fence is
   still proven, the parent identity still matches, and the exact artifact is a
   regular single-link file owned by the configured writer. A missing or invalid
   record is fail-closed: recovery never scans for or deletes temporary-looking
   names. The record is removed before the file service is released; a pending
   record is discarded only from the supervisor private runtime directory.
4. **Verify.** Re-open the published Sidecar without following links and
   confirm its content is the staged document and that the requested values
   are present. Derive the reported result from that committed snapshot.
5. **Release.** Restart the file service regardless of outcome. The supervisor
   that owns the session guarantees the restart on success, on failure, and on
   crash of any session participant. If exclusivity is lost during the session
   or the outcome cannot be confirmed, the result is `outcome_unknown`, never
   a false success, a claimed no-change, or an automatic rollback.

The conflict guarantee is content-identity under exclusivity: the session
publishes only when the Sidecar's current bounded content still matches the
observed evidence, and no external writer can intervene between that
comparison and publication. An external change that is later reverted to
byte-identical content is indistinguishable from no change and is treated as
no change. Network filesystems are outside the supported backing stores,
because a failed rename there may still have taken effect and cannot be
classified as a pre-publication refusal.

External applications read and edit the Sidecar through the file service
outside the save session, with no Slipstream protocol. Every later Save
inspects and validates their changes. The file service must apply the writer
identity to every file it creates, so Sidecar ownership stays continuous
between external edits and Save publication. The product does not protect
writes from software that bypasses the managed service and writes the backing
store directly; such access is outside the supported environment.

### Supported deployment shape

The supported writable deployment is:

- a backing tree whose host-side ancestors are private to the deployment: not
  searchable or writable by other host users, so no writer can bypass the
  managed service;
- a writer identity distinct from the Web application's identity, used only
  by the file service's worker processes and the save helper. The Web
  application reaches the backing tree only through its read-only bind;
- directories that are sticky, set-group-id, and group-writable, with Original
  Files owned by the deployment identity and read-only to the writer identity,
  so the writer can create and replace Sidecars but cannot write, replace, or
  unlink an Original. Files that arrive as Originals through the file service
  are re-owned and restricted by deployment provisioning before they are
  admitted as Original Files, because writer-owned files are writable by the
  writer identity. Hardlink creation over deployment-owned Originals by the
  writer identity must be denied — for example by the kernel's protected
  hardlink policy — as a fail-closed provisioning prerequisite;
- the Web application and CLI never writing the backing store; and
- the save helper as the only component that opens a save session, under a
  supervisor that owns the file service lifecycle.

A deployment with a read-only Library, including the default Compose shape,
supports Read and reports Save unavailable with the actionable reason. The
unsupported coordination mechanisms — advisory locks, file leases, and
check-then-rename on a shared writable tree — were probed and defeated by
external writers; they are not part of this design (see Options).

### API boundary

The shared server operations are:

- `GET /api/photos/{id}/external-metadata` for Read Metadata;
- `POST /api/photos/{id}/external-metadata` for checked Save Metadata.

The read result, save request, save result, and error envelope are pinned by
JSON vectors in `compatibility/metadata/` and are the single contract for Web
and CLI:

- The read result carries per-field entries with `state`
  (`present` | `absent` | `invalid` | `unavailable` | `resource_limit`),
  provenance (`sidecar` | `embedded-xmp` | `iptc-iim` | `original`), the value
  in its typed shape, `writable`, and, for language alternatives, the full
  language map. Capture facts carry their standard EXIF identifiers, units,
  and source. The result carries the association status, the evidence token,
  and `save_available` with a reason when false.
- The save request carries the evidence token and explicit `changes`: one
  patch per field, with `set` carrying the typed value (including language
  maps), `clear`, or `remove`. It cannot name a filesystem path.
- The save result carries the affected fields, the verified values, and fresh
  read evidence.
- The error envelope carries one code from the product's failure
  distinctions: `invalid_input`, `unsupported_field`, `photo_missing`,
  `original_unavailable`, `association_unresolved`, `photo_removed`,
  `metadata_malformed`, `evidence_stale`, `save_unavailable`, `permission`,
  `resource_limit`, `storage_failure`, and `outcome_unknown`. HTTP status
  mapping and CLI exit codes derive from the same code table.
  Invalid Photo IDs and malformed request JSON return `400/invalid_input`.
  Requests exceeding the 2 MiB body limit return `413/resource_limit`, including
  refusals made by the body extractor. These admission refusals use the same
  metadata envelope and have no Sidecar effect.

The CLI exposes the same operations as `photos metadata PHOTO_ID` and
`photos metadata-save PHOTO_ID --input FILE`. Web uses the same read result,
displays provenance and pending patches, and refreshes after a conflict.
Neither path creates an import or synchronization workflow.

### Library ownership

The database remains the owner of Library decisions and Photo identity. It
does not cache Sidecar fields; the Sidecar Association record is ownership and
conflict state, not metadata. Scan, Restore, Permanent Deletion, and Location
Recovery do not apply Sidecar values to Library decisions. The confined
publishing operation is reachable only through the metadata service's
association owner, which derives the Sidecar destination from an admitted
Photo; no general write-by-Library-path entry point is exposed to metadata
callers.

## Options

### Option A: advisory locks or file leases around check-then-rename

`flock` does not bind writers that do not take the lock, and Linux file leases
do not block external `rename` of the directory entry. Probes with disposable
files reproduced both defeats, including data loss for the newly created
Sidecar case. Rejected: the boundary is not enforced against external writers.

### Option B: `RENAME_NOREPLACE` plus revision recheck

This closes silent replacement of a raced creation but cannot make the
publish step conditional on the observed revision of an existing file.
Rejected for updates: it narrows but does not remove the lost-update window.

### Option C: managed file service with an enforced quiesce window — selected

External access flows through one managed file service. The save session stops
that service, verifies the stop, validates, publishes, verifies, and restarts
it, with a supervisor guaranteeing the restart. Qualification must establish
the control-group fence with observed emptiness, disabled client caching,
writer-identity isolation, and interruption recovery before the deployment
admits Save.
Rejected alternative within this option — excluding external applications
permanently — violates interoperability and is not used.

### Option D: a cluster or cooperating filesystem

A filesystem that enforces writer exclusion for uncoordinated writers would
allow in-place sessions, but it constrains the deployment to a specific
filesystem and still needs a writer identity policy. Rejected: Option C
delivers the same guarantee on ordinary storage with a smaller contract.

### Option E: delegate parsing to ExifTool or Exiv2

A subprocess or system library would provide broad format support quickly.
Rejected for the in-process contract: Read must not depend on a helper
executable, helper versions would make preservation and failure semantics
deployment-dependent, and process execution complicates confinement. ExifTool
remains an acceptance fixture, not a runtime dependency.

## Verification

Permanent tests must prove observable behavior rather than parser wiring:

- every declared writable field can be set, cleared, removed, read back, and
  inspected through both CLI and Web;
- language alternatives, ordered Creators, unordered Keywords, Unicode, zero,
  false, fractional Rating, `-1`, and missing values retain their semantics,
  including BCP 47 matching and the `x-default` refusal rule;
- embedded, Sidecar, and IIM provenance plus absent-versus-empty fallback are
  visible for every kind in the extraction matrix;
- RAW/JPEG association cases, duplicate Sidecars, retained orphan records, and
  moved Originals refuse unsafe writes, and the `retained-orphan` state blocks
  and clears through fresh inspection only;
- evidence tokens are rejected after removal, restore, Permanent Deletion,
  relocation that returns to the same Location, Association generation bumps,
  and server restart;
- unknown XMP structures survive saves, every construct outside the document
  model makes Save refuse before mutation, malformed Sidecars are never
  replaced, and one Photo's failed multi-field save leaves the prior Sidecar
  unchanged;
- the save session refuses when the file service cannot be quiesced, publishes
  only while exclusive, reports `outcome_unknown` when verification cannot
  confirm, and restores external access after success, failure, and mid-window
  crash;
- Original bytes, Library Rating, Selection State, Albums, Capture Time
  ordering, and Preview state remain unchanged throughout; and
- external-tool evidence meets the
  [Product Spec acceptance](../docs/library-management-metadata.md#acceptance):
  ExifTool checks every declared writable field in both directions and verifies
  preservation; a named photo application, such as darktable, is evaluated through
  actual metadata input and output in a headless workflow. Record versions, fields,
  directions, naming and association behavior, and limitations. Rendering alone
  does not establish metadata exchange. Distinguish embedded-only JPEG workflows
  from Sidecar support. Lightroom Classic validation is not required, and untested
  applications have no compatibility claim.
