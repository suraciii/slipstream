# Library Management: Read and Save Metadata

Photographers use descriptive metadata, keywords, ratings, and attribution across photo applications. They and their Agents need to read and save those facts without damaging Originals or discarding another application's metadata.

Read and Save form one complete capability. A read-only or Rating-only implementation does not satisfy this specification.

[Photo Library and Albums](../spec.md) owns Photo identity and Original File boundaries.

## Capability Boundary

Read Metadata MUST inspect one identified Photo's embedded metadata and associated XMP Sidecar. Save Metadata MUST create or update that Photo's adjacent XMP Sidecar with explicitly supplied changes.

Neither operation MUST rewrite an Original File. A downloadable export alone MUST NOT be reported as Save Metadata.

The external Agent owns choosing Photos, comparing values, and composing actions. Slipstream MUST NOT introduce a synchronization planner, automatic import, or a separate Adopt capability.

Existing Photo Rating operations remain authoritative for library Rating. Reading or saving external metadata MUST NOT implicitly change Rating, Selection State, Albums, Capture Time ordering, or an Edit Recipe.

## Field Coverage

The supported capability covers descriptive, organizational, attribution, rights, and read-only capture facts. Reads and Sidecar saves MUST provide the same writable-field coverage.

The [metadata field inventory](../../../crates/slipstream-server/src/metadata_fields.rs) owns the exact writable property names and value shapes. The rules below govern their product semantics.

### Description and Organization

Title and Description use language alternatives. Keywords are an unordered set of text values.

Label MUST NOT be interpreted as a fixed color or a Selection State. External applications can assign different meanings.

External rating uses the standard values -1 or a real number from 0 through 5.

Raw Rating presence MUST distinguish missing from explicit 0. Read MUST label the XMP missing-Rating default as inferred, not a stored value.

External rating -1 MUST NOT set Selection State. A fractional external rating MUST NOT be silently rounded into the Library's integer Rating.

Adopting a compatible value uses the existing explicit Rating operation. Incompatible values require a caller's decision.

### Attribution and Rights

Creators are an ordered list of names. Copyright notice and rights usage terms use language alternatives.

Missing copyright status MUST NOT be presented as false or as permission to use the Photo.

Reading metadata MUST NOT fetch the rights information URL or treat its contents as permission or instructions.

### Capture Facts

Read Metadata MUST expose the Photo's available capture facts:

- capture date/time with recorded subseconds and offset;
- camera make/model and lens model;
- image dimensions and orientation;
- exposure time, aperture, ISO, and focal length.

These are read-only facts from the Original, with their standard EXIF identifiers, units, and source. Missing offsets MUST NOT be invented.

The existing [Capture Time rules](../spec.md#capture-time) own Library ordering.

Sidecar representations of these facts MUST be distinguishable from Original
facts and MUST NOT replace them in Library ordering or Preview behavior. Save
MUST preserve these representations, but MUST refuse attempts to edit them.

### Coverage Limits

Read MUST report whether each supported field is present, absent, invalid, or unavailable, and which fields are writable. Unsupported properties MUST NOT be reported as absent supported properties.

Save MUST reject unsupported fields before making any change. This capability does not include:

- arbitrary tag editing or GPS editing;
- face regions or structured IPTC Extension records;
- RAW development settings or a private Selection State mapping.

Those properties MUST survive saves to other fields. Their presence alone MUST NOT prevent reading supported fields.

## Sources and Association

Read MUST return embedded and Sidecar values with separate provenance. The underlying values and their differences MUST remain inspectable.

For each supported descriptive field, the displayed external value uses this order:

```text
inspect the Sidecar property or its source state
if the Sidecar property or source is invalid, unavailable, or resource-limited:
    report that state without falling back
else if the Sidecar property is present:
    use the Sidecar property, including a valid empty value
else if the Sidecar property is confirmed absent:
    inspect embedded XMP, then its defined IPTC IIM counterpart, if any
    advance to the next source only when the preceding property is absent
```

The supported IIM counterparts use IPTC-defined encoding and multiplicity:

| Field | IIM dataset |
| --- | --- |
| Title | 2:5 |
| Description | 2:120 |
| Headline | 2:105 |
| Keywords | 2:25 |
| Creators | 2:80 |
| Creator job title | 2:85 |
| Credit | 2:110 |
| Source | 2:115 |
| Copyright notice | 2:116 |

IIM text has no language map; Read MUST identify this limitation instead of inventing translated alternatives. Other writable fields have no IIM fallback in this capability.

Conflicting embedded EXIF attribution MUST remain preserved; it MUST NOT silently replace the declared XMP/IIM values. Capture facts MUST retain their Original authority.

Library Rating MUST be labeled separately.

An invalid or unreadable Sidecar property MUST NOT silently fall back to embedded metadata and appear valid. A valid empty Sidecar value MUST suppress fallback.

Removing a Sidecar property MUST reveal the embedded fallback, if any. Read MUST make this distinction visible.

[Sidecar Association](../../../CONTEXT.md#metadata) is determined by the following rules:

- One same-directory, same-basename XMP Sidecar belongs to the sole eligible RAW Original.
- A sole JPEG Original owns the association only when no RAW shares its basename.
- RAW and same-basename JPEG remain independent; the JPEG MUST NOT read or overwrite the RAW's Sidecar.
- Multiple eligible Originals or candidate Sidecars MUST be reported as ambiguous; Save MUST refuse them.
- A Photo without an eligible association MUST remain readable from its Original and MUST report that Sidecar saving is unavailable.

A new Sidecar MUST use the eligible Original's basename with `.xmp` in its own
directory. Existing candidates MUST be inspected before creation; an existing
`.XMP` MUST NOT result in a second Sidecar. Clients MUST identify Photos, not
supply arbitrary filesystem destinations.

Original Location, Original identity, or association changes MUST invalidate prior read evidence.

A retained Sidecar MUST NOT silently transfer to another Photo when its known owner's Original disappeared from its Location, its Photo record was removed by Trash Record Cleanup, or its Original moved by Location Recovery.

Read MUST identify this unresolved association and Save MUST refuse it. External correction followed by a fresh inspection can establish an unambiguous association.

Slipstream MUST NOT move or rename the Sidecar automatically.

## Read Behavior

Read MUST have no metadata mutation effects. It MUST provide the Photo identity,
Original Location, association status, supported values and provenance, validation
problems, and evidence sufficient to make a checked Save. An absent Sidecar is a
valid observed state, distinct from unreadable or malformed content.

A bounded read MUST distinguish a resource limit from missing metadata. Metadata failure MUST NOT make an otherwise browsable Photo unavailable.

Strings are data, not instructions, executable content, or markup to run. Parsing MUST NOT retrieve external resources.

A Removed Photo MAY be read; Save requires Restore first.

## Save Behavior

Save MUST receive one Photo, the observed read evidence, and explicit field changes. It MUST NOT copy all embedded metadata or all Library facts implicitly.

Omitted fields MUST remain unchanged. Save MUST distinguish setting a value, setting a valid empty value, and removing the Sidecar property.

A missing property MUST NOT implicitly clear a Library value.

- A keyword change replaces the supplied set; ordering is not meaningful. Exact duplicates MUST collapse without case folding.
- A creator change replaces the supplied ordered list; order MUST survive.
- Language-alternative changes MUST name the affected languages, including `x-default` where used. Unmentioned alternatives MUST survive.
- Removing one language alternative MUST NOT delete the others.
- Changes MUST preserve standard XMP language-alternative validity.
- If a request would require an unrequested language change, Save MUST refuse with the required correction rather than modify that language silently.
- Empty text, an empty list, false, and numeric zero MUST NOT be conflated with property removal. Numeric and boolean fields do not accept empty text.
- All requested fields MUST be valid before Save changes any field. One Photo's changes MUST succeed together or leave the prior Sidecar usable and unchanged.

Save MUST preserve the meaning, types, values, and structure of unmodified properties, including unknown namespaces and language variants. Byte-for-byte formatting preservation is not required.

If preservation cannot be guaranteed, Save MUST refuse instead of rebuilding a reduced Sidecar. It MUST NOT replace malformed XML with a fresh document or erase another application's edit settings.

Save MUST verify that its observed Original and association still apply and that the Sidecar has not changed, disappeared, or appeared since Read. Conflicts MUST leave the newer content intact and require fresh inspection and a new decision.

The same rule applies to creation: observed absence is not permission to replace a file that appeared later. Save MUST NOT offer an unchecked force overwrite.

Saving explicit external values MUST NOT depend on a mutable Library Rating
remaining equal to those values. A caller exporting a Library Rating chooses its
observed value; Save reports exactly that value without claiming both stores are
synchronized. No durable synchronization baseline is required.

## Results and Failure Recovery

A successful Save MUST identify the Photo, Sidecar, affected fields, verified
saved values, and fresh read evidence. An unchanged request MAY report no change
only after checking the current content. It MUST NOT imply a historical write.

Failures MUST distinguish:

- invalid input and unsupported fields;
- missing Photos and unavailable Originals;
- unresolved associations and Removed Photos;
- malformed or unreadable metadata and stale evidence;
- permission denial, resource limits, and storage failure.

A missing Sidecar is not a failure when safe creation is allowed.

After connection loss or an interrupted save whose effect cannot be confirmed, the caller MUST receive an unknown outcome. Fresh Read establishes current content, not which caller wrote it.

Clients MUST NOT retry blindly or report that nothing happened. A new Save requires fresh evidence; no operation ledger or exactly-once delivery is implied.

Two concurrent saves based on the same evidence MUST NOT silently lose one
another's changes. Restore, removal, Trash Record Cleanup, or Location Recovery
MUST NOT let Save target another Original or a retired association. A successful
save can precede a later removal; that ordering MUST NOT be described as both
operations operating on an unchanged Photo.

## Human and Agent Access

The Web MUST allow inspecting sources, editing supported writable fields, reviewing
pending changes, explicitly saving, and refreshing after conflicts. It MUST show
Sidecar changes separately from Library Rating changes and MUST NOT promise an
external save before confirmation. No specialized import or synchronization
wizard is required.

The Web MUST lead with Title, Description, and Keywords. A normal Title or Description edit supplies the default language without requiring a language tag or operation selector.

Additional languages, attribution, rights, source inspection, empty values, and property removal remain explicit and available through labelled disclosures. Unchanged fields and unmentioned languages MUST remain unchanged.

Pending-change review uses human field names.

Unsaved metadata input belongs to its Photo and MUST survive ordinary Photo and Grid navigation in the current mounted browser session. Closing Details does not discard it.

Reopening inspects current metadata and retains the draft for explicit review. Saving still uses checked evidence and MUST NOT silently replace changed external content.

A visible Discard changes action removes the draft. Reload recovery is not promised.

Admitted Saves retain settlement ownership after navigation.

A read-only deployment MUST lead with readable facts and a concise reason
Save is unavailable. It MUST NOT present a long form of disabled mutation
selectors. Capture facts and source provenance remain inspectable.

The CLI MUST expose the same read and checked-save capability, discoverable field coverage, limits, and structured outcomes. One-Photo operations are sufficient: Agents MAY query and compose them.

The application MUST NOT silently split, expand, or automatically continue a multi-Photo task. CLI syntax and examples belong in the [CLI Reference](../../command-line/reference.md), not this specification.

## Original Safety and Supported Operation

Save grants permission only to create or update the associated XMP Sidecar. It MUST NOT modify, delete, rename, or move Originals, sibling files, or directories.

A deployment with read-only Sidecar access MUST still support Read and MUST report Save as unavailable.

Complete delivery MUST include a supported deployment that can save adjacent Sidecars while preserving the Original safety boundary. Read-only operation alone does not satisfy delivery.

### Exclusive Save Environment

The supported Save environment MUST enforce exclusive access against every writer
that can change the target Sidecar or replace its directory entry, from final
validation through publication and verification. This includes external tools,
not only Slipstream clients. Checking a revision followed by replacement is not
by itself a guarantee against concurrent external changes.

Read and Save MUST expose whether safe saving is available and an actionable reason when it is not. Save MUST refuse before mutation when the required boundary cannot be established.

A checkbox, an instruction to close another application, or a lock ignored by external writers MUST NOT count as enforcement.

Loss of exclusivity during an admitted operation MUST NOT produce a confirmed success without evidence. Uncertain effects follow the failure-recovery rules above.

External applications MAY read or edit the Sidecar outside Slipstream's exclusive save interval. A supported handoff MUST let them use standard Sidecars without implementing a Slipstream-specific protocol.

Every later Save MUST inspect and validate their changes. The product does not guarantee safe simultaneous writes by software that bypasses the supported environment's enforcement boundary.

Complete delivery MUST demonstrate this environment and the external-tool handoff,
including restoration of external access after success, failure, or interruption.
An environment that permanently prevents external applications from using the
Sidecars does not satisfy interoperability. Enforcement mechanisms belong in the
Design Spec and MUST NOT broaden permission to rewrite Originals.

Library expansion, Location Recovery, rescan, and restart MUST NOT apply Sidecar values to Library decisions or reuse obsolete save evidence.

Backups MUST identify Sidecars as Photographer-owned files separate from application state. Restoring application state MUST NOT roll back external Sidecars.

Fresh inspection after restore MUST establish current external content before any save.

## Interoperability

Compatibility claims MUST name the tested application and workflow. Standard XMP alone does not establish compatibility with untested applications.

For JPEGs, compatibility claims MUST distinguish embedded-only workflows from Sidecar support. Slipstream MUST NOT claim automatic JPEG Sidecar consumption without evidence.

Properties an external application does not expose MUST retain valid standard values and pass independent inspection.

## Standards

Standard property semantics follow Adobe's
[XMP Part 1](https://github.com/adobe/XMP-Toolkit-SDK/blob/main/docs/XMPSpecificationPart1.pdf),
[XMP Part 2](https://github.com/adobe/XMP-Toolkit-SDK/blob/main/docs/XMPSpecificationPart2.pdf),
and the [IPTC Photo Metadata Standard](https://www.iptc.org/std/photometadata/specification/IPTC-PhotoMetadata).
These define representation, not permission to change a Photo's Library decisions.
