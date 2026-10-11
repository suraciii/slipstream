# CLI Candidate Installation

The Linux amd64 client archive is a candidate, not a published Slipstream
release. Its name includes the CLI package version and the exact source commit.
Qualification does not deploy a server, change a Library, or publish a tag.

Build a candidate from a clean checkout with Python 3, Rust 1.97.1,
Cargo, C and C++17 compilers, CMake, and pkg-config:

```sh
python3 scripts/package-cli.py
```

The command writes a new directory under `dist/` containing one `.tar.gz`
archive and its `.sha256` checksum. It refuses an existing candidate directory
instead of replacing an artifact. To inspect a received candidate, run these
commands from the directory containing the archive and checksum:

```sh
sha256sum -c slipstream-cli-VERSION-gCOMMIT-linux-amd64.tar.gz.sha256
tar -xzf slipstream-cli-VERSION-gCOMMIT-linux-amd64.tar.gz
./slipstream-cli-VERSION-gCOMMIT-linux-amd64/slipstream --version
./slipstream-cli-VERSION-gCOMMIT-linux-amd64/slipstream --help
```

Replace `VERSION` and `COMMIT` with the values in the candidate name. The
archive includes this installation guide, the Agent guide, the complete
specification index and contracts, shared vocabulary, contributor guidance,
and license notices. Contract links remain local. Links to repository-only
instructions, source files, and directories open GitHub at the candidate's
exact commit. Submodule links use the recorded submodule commit in its own
repository. Help and version require no service, token, or native server
libraries. The client needs a Linux amd64 host with a compatible glibc
runtime. Other systems are not qualified by this candidate.

Install the extracted `slipstream` executable on the client machine in a
directory on `PATH`. Keep the Access Token in a separate private regular file
owned by that user with no group or other permissions. Configure the HTTP or
HTTPS service origin and token path on the client machine:

```sh
export SLIPSTREAM_SERVER_URL=https://photos.example.test
export SLIPSTREAM_ACCESS_TOKEN_FILE=/private/path/to/access-token
slipstream status
```

HTTPS uses normal certificate validation; there is no certificate-verification
bypass. An HTTP invocation prints one unencrypted-connection warning to stderr.
The client does not mount Originals, the service database, or the service's
native image libraries. Before upgrading a candidate, verify the new checksum,
read its reference, and run `status` against the intended service. An
incompatible service is rejected before any write; keep the previous client
candidate until the new one passes that check. An uncertain write still
requires a fresh object read and human or Agent judgment, not an automatic
retry.

Use a client and service built from the same candidate source revision. A
contract-1 response can still be incomplete: `incompatible_server` preserves
the advertised versions and identifies a missing or invalid capability field.
Install the matching service candidate before retrying; do not bypass this
check. For darktable, inspect `slipstream darktable --help`, Photo-scoped
capabilities, and the [darktable Agent workflow](agent-cli.md#find-and-edit-a-photo-with-darktable).
The [darktable CLI Reference](../specs/photo-development/darktable/cli.md) specifies the target, rather than
claiming every candidate contains it. An absent command or unavailable capability
must not trigger a generic processing alias or bypass. A working client/service
pair does not qualify native modules, RAW encodings, resources, or output
combinations. Other peer Processing Modules use the
[composition workflow](agent-cli.md#compose-other-processing-modules).

The independent `darktable-edit` executable is shipped in a dedicated archive
when packaging an immutable photo image:

```sh
python3 scripts/package-cli.py --darktable-image sha256:IMAGE_DIGEST
sha256sum -c dist/darktable-edit-VERSION-gCOMMIT-linux-amd64.tar.gz.sha256
tar -xzf dist/darktable-edit-VERSION-gCOMMIT-linux-amd64.tar.gz
./darktable-edit-VERSION-gCOMMIT-linux-amd64/run-darktable-edit --help
```

The archive contains the independent binary and its supervisor, the complete
native installed tree, the resolved runtime shared libraries, exact discovery,
source revision, OS package inventory, and digest manifest. The wrapper selects
the adjacent bundle; direct invocation may instead set `DARKTABLE_EDIT_BUNDLE`
to its absolute `bundle/` path. This is distinct from official `darktable-cli`.
It requires Linux amd64 with the image's compatible glibc loader; an Ubuntu
26.04 execution host or the original immutable image supplies that environment.
It needs no Slipstream service, database, token, or desktop darktable catalog.

Run `slipstream workspace open PHOTO_ID` once. The result discloses the execution
host, access mode, input path, and preview/output directories. Server paths are
accessible on that host or through an explicitly mounted authorized environment;
a remote client's filesystem does not acquire them merely by reading a reply.
On the execution host the simple standalone flow is:

```sh
darktable-edit open INPUT_PATH --state STATE_DIR --preview-directory PREVIEW_DIR --output-directory OUTPUT_DIR
darktable-edit edit show STATE_DIR
darktable-edit edit set STATE_DIR INSTANCE_ID PARAMETER_ID --value 0.5
darktable-edit export create STATE_DIR
darktable-edit preview render STATE_DIR
slipstream workspace files WORKSPACE_ID
```

Choose instance and parameter IDs through discovery. Guard read-dependent edits
with the returned `--if-state` token. Export uses saved output settings and works
before any preview. Completed paths can be read directly by another tool on the
same host; reconciliation discovers them without a registration step. To process
an arbitrary local file, use a private state directory outside Slipstream.

Catalog commands disclose edit-local versus user/configured scope. They do not
use the desktop application's catalog or search another scope after a miss.
Workspace inputs, edits, history, catalogs and final files survive service startup
and remain until explicit reviewed removal; transient receipt retention is
separate. Preserve the full workspace and state roots when backing up; see
[processing backup and acceptance](../tools/processing/README.md).
See the [standalone grammar](../specs/photo-development/darktable/cli.md#grammar-and-identity)
and [Photo Workspace](../specs/photo-development/workspace/spec.md).
