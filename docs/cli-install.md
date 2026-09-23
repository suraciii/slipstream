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
archive includes this installation guide, the CLI reference, the Agent guide,
selected background specs linked by the reference, and license notices. Other
links in those background specs may require the source repository at the named
commit. Help and version require no service, token, or native server
libraries. The client needs a Linux amd64 host with a compatible glibc
runtime. Other systems are not qualified by this candidate.

Install the extracted `slipstream` executable on the client machine in a
directory on `PATH`. Keep the Access Token in a separate private regular file
owned by that user with no group or other permissions. Configure the HTTPS
service origin and token path on the client machine:

```sh
export SLIPSTREAM_SERVER_URL=https://photos.example.test
export SLIPSTREAM_ACCESS_TOKEN_FILE=/private/path/to/access-token
slipstream status
```

The operator supplies a trusted HTTPS certificate for the selected origin.
There is no insecure TLS switch. The client does not mount Originals, the
service database, or the service's native image libraries. Before upgrading a
candidate, verify the new checksum, read its reference, and run `status` against
the intended service. An incompatible service is rejected before any write;
keep the previous client candidate until the new one passes that check. An
uncertain write still requires a fresh object read and human or Agent judgment,
not an automatic retry.
