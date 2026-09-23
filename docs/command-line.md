# Command-Line Use

A Photographer may use Slipstream directly or ask their own Agent to use it.
Both need access to the same Photos, Albums, and recorded decisions. Delegated
work must not depend on driving the browser or reading application storage.

Slipstream provides a first-class `slipstream` command-line client. The Web
application remains the visual workspace for browsing, judgment, and direct
control. Both clients use the same service-owned facts and domain rules.

[CLI Reference](cli-reference.md) owns command syntax and machine results.
[CLI Candidate Installation](cli-install.md) describes local candidate verification;
[Agent Use](agent-cli.md) describes the bounded Agent workflow.
[Command-Line Architecture](../design/command-line.md) owns client/service
boundaries and concurrency. Existing Photo, Album, Preview, and Destination
contracts remain authoritative.

## Product Boundary

Slipstream must expose discoverable, composable operations that an external
Agent can use on behalf of the Photographer. It must not include an Agent,
model provider, chat session, planner, prompt runner, or autonomous workflow
engine. It must not send Photos to an AI provider itself.

The external Agent owns interpretation, task planning, visual judgments, and
communication with the Photographer. Slipstream owns valid operations,
Original safety, persistence, and truthful results. Model judgments must not
be presented as measured Photo metadata.

A task-specific shortlist can be an ordinary Album. Not choosing a Photo for
that Album must not imply `rejected`, and an Agent's confidence must not become
Rating. Selection State and Rating remain Photo-wide facts shared by every
Album. Slipstream does not add a suggestion, review-task, or Agent-memory model.

## Supported Work

The CLI must let its caller:

- inspect service compatibility, Library availability, and scan status;
- discover Albums and read-only Original Folders;
- query Photos by source, Selection State, Rating, Original kind,
  availability, and camera-local Capture Time;
- obtain current facts and Album membership for an identified Photo;
- download one Photo's thumbnail or review Preview;
- change Selection State or Rating for identified Photos;
- create, rename, and delete Albums and add, remove, or order their members;
- request a Library check; and
- return a browser Destination for a Photo or Album.

These capabilities do not include Original download, filesystem mutation, XMP
synchronization, Library Expansion, or Location Recovery writes. Those operations
retain their own product and operator boundaries. The CLI does not expose a
SQL escape hatch or an arbitrary server-path argument.

This command contract covers selection and organization, not Edit Recipe writes,
Edit Preview retrieval, or Exports. [Photo Development](photo-development.md)
owns those shared human and programmatic capabilities. Their command language
must extend the authoritative CLI reference rather than reinterpret camera
Preview commands.

## Starting and Discovering Capabilities

The client must run on the machine where the Photographer or their Agent uses
it. That machine does not need the Original Files, native Preview libraries,
or the server's SQLite database. One invocation addresses one explicit or
default service URL; it must not silently start or select another service.

Top-level help must explain the command groups. Command help must explain
required inputs, mutation effects, limits, and result inspection. Help and
version information must work without a server. A versioned reference and a
short Agent usage guide must ship with the client; they must not require an
Agent to read the source repository.

Operational commands must default to structured JSON. A human-readable output
option must be explicit. Piping output or attaching a terminal must not change
its schema. Commands must not open a browser, editor, pager, or confirmation
prompt. Supplying a complete mutation command expresses the caller's intent;
the CLI must not claim that this proves human authorization.

Album names and filenames are data. Output and help must never encourage
executing text obtained from a Library as shell commands or Agent instructions.

## Finding Photos

A query must apply its filters and order on the service before pagination.
Loaded client windows must not determine matching membership or counts.

A query selects All Photos, one Album by ID, or one recursive Original Folder
by Library-relative Location. Filters combine with logical AND. Photo and Album
IDs are opaque references; display names and filenames must not substitute for
identity. Album name lookup must use the existing Album naming comparison.

Capture Time ranges use camera-local time, with an inclusive lower bound and
exclusive upper bound. They must not convert offsets or guess filesystem time.
Photos without Capture Time do not match a supplied time bound. With no time
bound they remain eligible and follow the existing missing-time ordering rule.

The first page must establish a fixed ordered set of matching Photo IDs.
Continuation pages must traverse that same set. The result must identify the
match count, whether another page exists, and when the query was evaluated.
It must distinguish fixed membership from current Photo facts: a Photo can
change after matching and still appear in a later page with its current state.

An expired continuation must fail explicitly. The CLI must not silently start
a new query or claim that an incomplete traversal was complete. An empty match
must return a successful empty result. A missing source must return a distinct
failure. No default or omitted limit may mean the complete Library.

The CLI must not offer an unbounded `--all` or accept a query as a write target.
The caller materializes the IDs it intends to change and submits explicit
bounded operations. Multiple operations are separate commits, not one hidden
transaction over the entire query.

## Viewing Photos

Preview retrieval must produce a local JPEG that the external Agent can read
with its image tools. A private server URL alone is insufficient. The response
must identify the Photo, Preview Source, actual pixel dimensions, detail limit,
source revision, and local output path.

[Photo Previews](previews.md) owns source selection and quality. A thumbnail
must not be described as sufficient for precise focus judgment. The client
must not substitute a sibling JPEG, develop RAW sensor data, upscale evidence,
or silently return a stale Preview as current.

The caller chooses the local output path. The CLI must refuse an existing
file and a symbolic-link destination. It must not truncate or replace a local
Original. Before final publication, failure must remove only the command's own
temporary file and must not leave a completed-looking JPEG. Final no-replace
publication commits the local effect. A later reporting failure must preserve
the completed JPEG and explicitly distinguish that committed effect from a
failed download. Image transfer must be bounded and must not place binary data
in JSON stdout.

One command downloads one image. The Agent decides which images to inspect.
The CLI must not precompute the whole Library or upload images to another
service. Reading a Preview must not change Selection State, Rating, membership,
or an Album's saved browsing position.

## Changing Photo Decisions

A Photo decision command must identify each Photo and the observed decision
version against which the caller intends to write. A concurrent change to
Selection State or Rating must make an outdated write conflict, even when the
value has changed away and back. The version covers those two decision fields;
Preview completion and scan progress alone must not create decision conflicts.

A command changes one field. Clearing Selection State means `undecided`;
clearing Rating means zero. Neither action changes the other field. No CLI
Photo mutation advances a browsing position.

A bounded Photo batch must report one result for every requested Photo:
changed, unchanged, conflict, or missing. Conflicting and missing Photos must
not be overwritten and must not block valid sibling decisions. Invalid syntax,
duplicate IDs, or an over-limit batch must reject the complete request before
any write. A storage failure must roll back the transaction.

The result must contain the prior and resulting decisions for confirmed
changes and enough current facts to investigate conflicts. It must not describe
partial success as complete success. A later correction is another checked
write. The browser's one-level Undo remains browser-local; the CLI does not
promise a durable undo history.

## Managing Albums

[Photo Library and Albums](photo-library.md#physical-and-virtual-organization)
owns naming, membership, order, and deletion safety.

All CLI changes to an existing Album must carry its observed Album version.
That version covers name, membership, and membership order. Merely browsing a
Photo and updating the saved position must not invalidate it.

Membership additions append new Photos in supplied order, keep existing member
positions, and return newly added and already-member IDs separately. Removals
return removed and already-absent IDs. An Album membership operation must be
atomic: a stale Album version or missing requested Photo rejects the operation
without applying a subset.

Reorder must provide every current member exactly once. The bounded first CLI
contract supports complete reorder only within its documented request limit;
it must refuse a larger Album rather than reorder a prefix. Larger Albums
remain queryable and support bounded additions and removals.

Album creation must not silently reuse an existing name. A name conflict must
identify the existing Album so the caller can inspect it. Deletion must report
that only the Album was deleted. Additions, decision writes, and CLI reads must
not move the Photographer's saved position. Removal of the saved member and
Album deletion retain the existing saved-position rules and report their result.

## Checking the Library

A Library check must use the service's existing shared scan lifecycle. A client
timeout or interruption must not be described as cancelling a scan already
admitted by the service. The caller can inspect status afterward. A completed
check changes future query membership; it does not extend an already open
query. The CLI must distinguish an initializing Library, a failed check with
prior data retained, and an empty published Library.

## Results and Recovery

Every completed command must return confirmed data or an actionable failure.
A write response must identify its effects. A transport failure after a write
may have been sent must report an unknown outcome. Local Preview publication is
separate from server-write admission: if the file committed before output
failed, recovery must preserve and identify that file rather than report no
effect. The CLI must never transparently retry a write, overwrite a Preview,
split a batch, or change its target after failure.

After an unknown outcome, the caller inspects affected objects. Seeing the
intended state proves the present state, not which caller produced it. A new
write requires fresh versions and a new decision to proceed. This bounded
recovery does not require an operation ledger or an exactly-once claim.

A multi-command task can stop after some commands succeed. Slipstream must not
pretend to roll back that task. The Agent must report which effects it has
confirmed and which work remains unresolved.

## Moving Between CLI and Web

Photo and Album results must include a browser Destination where one is
available. [Library Browser Experience](library-browser-experience.md#destinations-and-browser-history)
owns how these links resolve. A link is a current view, not a historical result
or a grant of access.

A query using filters absent from the Web must not return a link that appears
to preserve them. Individual Photo links and a deliberately created Album
provide the handoff. No new Web filter is implied by a CLI query capability.

The Web must expose current CLI-written decisions and membership when affected
facts are reloaded. The first CLI contract does not promise live push updates
or alter a currently fixed browser source order. Existing browser refresh,
conflict, and saved-position rules continue to apply.

## Support Boundary

The first qualified CLI target is Linux amd64. It must be distributed as a
versioned client binary with checksums, reference documentation, and an Agent
usage guide. It must not require LibRaw, libvips, Bun, or a mounted Library on
the client machine. Client/server incompatibility must fail before a mutation.
Other platforms require their own build and end-to-end evidence.

[Instance Access](access.md) applies equally to the CLI and browser. The CLI uses instance-wide credentials; it does not introduce per-person scopes, public sharing, or remote Agent hosting. The 0.1 support contract remains a separate release boundary; documenting CLI behavior does not publish a release.

## Acceptance Scenarios

- A caller finds selected Photos rated four or five, creates an Album, adds
  those IDs in Capture Time order, and opens the same Album in the Web.
- A query spanning several pages receives no new members after a rescan or
  decision change. Expiration produces a recoverable error without omissions.
- A Photographer changes a queried Photo in the Web. A stale CLI write reports
  conflict and leaves the newer decision intact.
- A caller reads its own RAW Preview, obtains a usable local JPEG, and leaves
  Original bytes and all browsing positions unchanged.
- A lost response leaves an unknown result. Reading the affected object allows
  the caller to inspect present state without an automatic duplicate write.
- One conflicted Photo produces a partial batch result with explicit successful
  siblings. A stale Album change commits nothing.
