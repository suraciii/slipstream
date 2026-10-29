# Library Management: Standard Metadata Read and Save

Read and Save Metadata crosses two ownership boundaries: Original Files and
Photographer-owned XMP Sidecars. The Product Spec in
[`specs/photo-library/metadata/spec.md`](spec.md)
is authoritative for supported fields and user-visible behavior. This design
makes that capability durable without turning the Library state store into an
external metadata cache, and it defines the only environment in which Save is
admitted.

[Photo Library Design](../design.md) owns Original and Photo ownership; [Library Identity](../identity.md) owns stable identity across Location changes.

## Design Drivers

- Original Files are Photographer-owned and are never rewritten by Save.
- A Photo is the identity boundary. A same-basename RAW and JPEG remain
  independent, and a Sidecar MUST NOT cross that boundary.
- Read evidence MUST reject a stale Save rather than overwrite another
  application's change, including across restarts and Location changes.
- Unknown XMP properties, namespaces, structures, and language alternatives
  MUST survive a Save of supported fields.
- A malformed or unsupported Sidecar MUST remain visible as a problem; Save
  MUST NOT replace it with a reduced document.
- CLI and Web need one semantic operation, not two implementations with
  different fallback or conflict behavior.
- Metadata parsing is bounded and MUST NOT follow URLs, execute values, or
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
  every removal, restore, ownership transition, Location change, or Library
  expansion increments; Trash Record Cleanup invalidates the Photo's evidence
  when it removes the record; and
- the server instance epoch: a random identifier generated at startup from
  state that survives restarts of the operating system process.

Save accepts a token only when every component still matches. A relocated
Original that later returns to the same Location has a higher Association
generation, so recovery cannot reuse an old token, and no token survives a
server restart.

### Sidecar association record

The Library state store persists one Sidecar Association record per retained Sidecar. It records the relative path, owning Photo, last observed Sidecar revision, and state of `active` or `retained-orphan`.

When the owning Original is confirmed absent from its Location or leaves it through recovery relocation, the record becomes `retained-orphan` instead of being deleted.

Trash Record Cleanup retains minimal orphan evidence to prevent silent re-association. It records the path, last observed Sidecar revision, and former owner's historical identity without a live foreign key to the removed Photo.

A later eligible Original at the same basename does not inherit the Sidecar. Read reports the Association as unresolved while a `retained-orphan` record matches the candidate, and Save refuses it.

The state clears only after a fresh Read observes the Sidecar following an operator's external correction. That correction changes or removes the Sidecar or explicitly re-associates it through the inspection.

Slipstream never moves, renames, or deletes the Sidecar itself.

### Patches

Supported writable fields are represented as typed patches: `set`, `clear`, or `remove`. `clear` is a valid empty value that remains a property in XMP; `remove` deletes the Sidecar property and reveals fallback content.

Lists preserve order where the field is ordered and collapse exact duplicates only for Keywords. Language alternatives name each changed language; an omitted language is not changed.

Language names MUST be well-formed RFC 5646 tags, including private-use and grandfathered tags; `x-default` retains its XMP meaning. Read and Save use the same validator; subtag registry membership is not checked.

The [product field coverage](spec.md#field-coverage) owns the supported writable fields and read-only capture facts.

## Semantics

### Association and read ownership

The server derives candidates under the [product association rules](spec.md#sources-and-association) from actual confined directory entries, including supported Originals not yet scanned.

Only regular Originals are eligible. The extension comparison is case-insensitive; the directory and basename retain their exact spelling.

When a newly eligible RAW displaces a recorded JPEG owner, the state owner MUST transfer the association atomically and advance the displaced Photo's association generation. An unchanged retained orphan still blocks Save.

Metadata work runs under the state owner's serialization with scan, recovery, removal, restore, and Trash Record Cleanup.

One queued Save covers inspection, evidence comparison, supervisor admission and publication, and association recording. The operation continues even if its caller disconnects.

Remove either commits before Save, causing `photo_removed`, or after the admitted Save settles. Read records its observation in the same operation and returns the committed association generation.

Library Rating remains a separate fact. The native-work permit belongs to the queued inspection or Save, then to any remaining supervisor status check.

Disconnecting a caller does not release capacity while that work is still running.

Metadata associations migrate from the published Film schema v10 to v11.
The migration preserves existing Photos, Library decisions, Albums, Trash,
Edit Recipes, and Export records; it initializes each association generation
to one without importing Sidecar values.

Read opens the Original through `LibraryRoot` and reads bounded embedded XMP, EXIF capture facts, and IPTC IIM data. Sidecar bytes are opened only through confined same-directory operations.

[Sources and Association](spec.md#sources-and-association) owns field precedence and fallback behavior.

### Embedded extraction

Read reports each supported source per Original kind:

| Kind           | EXIF capture facts                        | Embedded XMP                               | IPTC IIM                    |
| -------------- | ----------------------------------------- | ------------------------------------------ | --------------------------- |
| JPEG           | APP1 Exif segment                         | APP1 `http://ns.adobe.com/xap/1.0/` packet | APP13 Photoshop IIM segment |
| TIFF-based RAW | IFD0 and Exif IFD                         | IFD0 tag `0x02BC` (XMP packet)             | IFD0 tag `0x83BB` (IIM)     |
| Non-TIFF RAW   | LibRaw fallback, as used for Capture Time | unavailable by kind                        | unavailable by kind         |

Dimensions are selected separately for each axis in this order:

1. The Exif IFD's `PixelXDimension` and `PixelYDimension`.
2. The primary IFD's `ImageWidth` and `ImageLength`.
3. The JPEG's `SOF` marker.

The reported identifier names the selected source. A higher-priority source that is present but invalid, duplicate, zero, or over its parse limit blocks fallback and reports that state.

Orientation comes from the primary IFD only. A kind without a source reports `unavailable` with the kind, never as absent.

The reader never fetches a `WebStatement` URL. Scalar TIFF capture values require exactly one value of the declared type.

JPEG frame dimensions accept the standard SOF marker families and require a complete component table. Malformed shapes report invalid dimensions under the same source precedence rules.

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

Serialization MAY change whitespace and attribute order. It MUST NOT change
names, values, structure, container types, qualifiers, or language tags.

Any construct outside the document model makes the Sidecar `unpreservable`. These constructs include:

- `rdf:ID` and `rdf:nodeID`;
- XPointer `rdf:about`;
- `rdf:parseType` values other than `Resource`;
- shorthand attributes the model does not represent;
- entity declarations.

Read still reports supported fields. Save refuses before mutation rather than rebuild a reduced document.

Read-only parsing retains the preservation error while inspecting unrelated supported fields. It never resolves external entities or expands declared entities.

Mutation and serialization reject a document with a preservation error. Capture representations retain present, absent, and invalid states for all thirteen capture fields, separately from the Original facts.

### Language alternatives

`dc:title`, `dc:description`, `dc:rights`, and `xmpRights:UsageTerms` are language-alternative properties. Language tags are compared by their canonical BCP 47 form: case-insensitive, with the canonical casing of `x-default`.

Duplicate alternatives in one existing property are reported invalid. A patch names the exact languages it changes, including `x-default`.

When applying a patch would require changing an unrequested language — for
example, updating the alternative that `x-default` mirrors, when XMP validity
requires the default to follow it — Save refuses and lists every language that
would have to change. Removing one alternative never removes the others.

### Save session and exclusive environment

Save is admitted only when the deployment provides an enforced exclusive save session. The deployment controls these ordered phases.

#### Quiesce

The supervisor MUST fence the managed file service in this order:

1. Stop its listener or refuse new external sessions.
2. Let in-flight requests drain within a bounded wait.
3. Terminate every service process in its control group.
4. Observe that the control group is empty.

Signaling only the main process is insufficient because workers can remain alive.

The service MUST disable and qualify client-caching and handle-replay features that could re-apply a write after restart. The managed service MUST NOT use oplocks, SMB leases, directory leases, or durable or persistent handles.

If the service does not reach a verified stop within the bounded timeout, Save MUST refuse before mutation and report Save unavailable with the reason.

#### Final validation

The session MUST recheck the evidence token against:

- Original path, inode, size, and modification time;
- Association candidate set, generation, and Sidecar name;
- Sidecar revision facts and bounded content digest, including the explicit missing state.

The session MUST validate every requested patch against the parsed document. The fence prevents an external writer from intervening between validation and publication.

#### Publish

The publisher MUST derive ownership again through one retained parent directory descriptor. Current facts and digest MUST match the evidence.

The publisher MUST stage the new document in the same directory under a fresh exclusive temporary name, write and flush it, and sync it. It MUST atomically rename it over the observed Sidecar and sync the parent directory.

An update MUST retain the observed Sidecar filename, including extension case. A missing Sidecar is created only when the evidence also said missing; `RENAME_NOREPLACE` semantics detect and refuse a raced creation.

A failed write before publication removes only this session's temporary file and leaves the previous Sidecar intact.

Before invoking the helper, the supervisor MUST commit a root-owned runtime publication record with a fresh lease token, the exact temporary name, and the parent device and inode. The helper derives that same temporary name from the inherited lease and opens it exclusively.

Recovery MAY remove a staged publication only when all these conditions hold:

- the publication record is present;
- the recovery fence is still proven;
- the parent identity still matches;
- the exact artifact is a regular single-link file owned by the configured writer.

A missing or invalid record is fail-closed: recovery MUST NOT scan for or delete temporary-looking names. The record is removed before the file service is released; a pending record is discarded only from the supervisor private runtime directory.

#### Verify

The session MUST reopen the published Sidecar without following links and confirm its content is the staged document and the requested values are present. The reported result is derived from that committed snapshot.

#### Release

The supervisor MUST restart the file service on success, failure, and crash of any session participant. If exclusivity is lost or the outcome cannot be confirmed, the result is `outcome_unknown`, never a false success, claimed no-change, or automatic rollback.

The conflict guarantee is content-identity under exclusivity. The session publishes only when the current bounded Sidecar content matches the observed evidence and no external writer can intervene before publication.

An external change later reverted to byte-identical content is indistinguishable from no change and is treated as no change.

Network filesystems are outside the supported backing stores. A failed rename there can still have taken effect and cannot be classified as a pre-publication refusal.

External applications read and edit the Sidecar through the file service outside the save session, with no Slipstream protocol. Every later Save inspects and validates their changes.

The file service MUST apply the writer identity to every file it creates, so Sidecar ownership stays continuous between external edits and Save publication.

The product does not protect writes from software that bypasses the managed service and writes the backing store directly. Such access is outside the supported environment.

### Supported deployment shape

The supported writable deployment is:

- a backing tree whose host-side ancestors are private to the deployment: not
  searchable or writable by other host users, so no writer can bypass the
  managed service;
- a writer identity distinct from the Web application's identity, used only
  by the file service's worker processes and the save helper. The Web
  application reaches the backing tree only through its read-only bind;
- sticky, set-group-id, group-writable directories with Original Files owned by the deployment identity and read-only to the writer identity;
- provisioning that re-owns and restricts files arriving as Originals through the file service before they are admitted as Original Files;
- denial of hardlink creation over deployment-owned Originals by the writer identity as a fail-closed provisioning prerequisite, for example through the kernel's protected hardlink policy;
- the Web application and CLI never writing the backing store; and
- the save helper as the only component that opens a save session, under a
  supervisor that owns the file service lifecycle.

The directory and ownership rules let the writer create and replace Sidecars without writing, replacing, or unlinking an Original. Writer-owned files need provisioning because the writer identity can write them.

A deployment with a read-only Library, including the default Compose shape, supports Read and reports Save unavailable with the actionable reason.

Advisory locks, file leases, and check-then-rename on a shared writable tree do not enforce the required boundary against external writers.

### API boundary

The shared server operations are:

- `GET /api/photos/{id}/external-metadata` for Read Metadata;
- `POST /api/photos/{id}/external-metadata` for checked Save Metadata.

The read result, save request, save result, and error envelope are pinned by
JSON vectors in `compatibility/metadata/` and are the single contract for Web
and CLI:

- The read result carries per-field state (`present` | `absent` | `invalid` | `unavailable` | `resource_limit`), provenance (`sidecar` | `embedded-xmp` | `iptc-iim` | `original`), typed value, and `writable`.
- Language alternatives include the full language map; capture facts carry their standard EXIF identifiers, units, and source.
- The read result carries association status, evidence token, and `save_available` with a reason when false.
- The save request carries the evidence token and explicit `changes`: one
  patch per field, with `set` carrying the typed value (including language
  maps), `clear`, or `remove`. It cannot name a filesystem path.
- The save result carries the affected fields, the verified values, and fresh
  read evidence.
- The error envelope carries one code from the product's failure distinctions: `invalid_input`, `unsupported_field`, `photo_missing`, `original_unavailable`, `association_unresolved`, `photo_removed`, `metadata_malformed`, `evidence_stale`, `save_unavailable`, `permission`, `resource_limit`, `storage_failure`, or `outcome_unknown`.

HTTP status mapping and CLI exit codes derive from the same code table. Invalid Photo IDs and malformed request JSON return `400/invalid_input`.

Requests exceeding the 2 MiB body limit return `413/resource_limit`, including refusals made by the body extractor. These admission refusals use the same metadata envelope and have no Sidecar effect.

The CLI exposes the same operations as `photos metadata PHOTO_ID` and
`photos metadata-save PHOTO_ID --input FILE`. Web uses the same read result,
displays provenance and pending patches, and refreshes after a conflict.
Neither path creates an import or synchronization workflow.

The mounted Web page retains pending typed patches by Photo identity, with the read evidence under which each draft began. Navigation detaches its presentation but does not discard the draft or cancel an admitted Save.

Reopening reads current facts. Changed evidence requires explicit review of the retained draft before a new Save, preserving the service conflict guard.

Successful Save or explicit Discard removes only that Photo's draft. Teardown releases browser-local drafts.

A bounded store MUST refuse further draft admission visibly rather than silently evict unsaved input.

Ordinary text controls create the same typed patches as advanced controls; there is no separate simplified save operation. Per-Photo draft retention keeps browsing usable without canceling admitted writes.

### Library ownership

The database remains the owner of Library decisions and Photo identity. It does not cache Sidecar fields; the Sidecar Association record is ownership and conflict state, not metadata.

Scan, Restore, Trash Record Cleanup, and Location Recovery do not apply Sidecar values to Library decisions.

The confined publishing operation is reachable only through the metadata service's association owner, which derives the Sidecar destination from an admitted Photo. No general write-by-Library-path entry point is exposed to metadata callers.

## Boundary rationale

The enforced save session protects against external writers that do not honor advisory locks. Qualification MUST establish the control-group fence with observed emptiness, disabled client caching, writer-identity isolation, and interruption recovery before the deployment admits Save.

Read MUST NOT depend on a helper executable. In-process parsing keeps preservation and failure semantics independent of helper versions and avoids expanding the confinement boundary.
