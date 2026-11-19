# Work in a Photo Workspace

Use this skill to create or reopen a Photo Workspace, run an External Tool, and
inspect or remove completed files. Read `slipstream workspace --help` for exact
options.

## Open

```text
slipstream workspace open PHOTO_ID
```

Preserve the workspace ID, absolute path, protected input path, execution host,
and available output directory. The path is an execution-environment path, not
a client-local path. Do not pass an Original Location to a tool.

`slipstream workspace show WORKSPACE_ID` re-reads the directory, input, and
inventory. `slipstream workspace files WORKSPACE_ID --limit 60` lists completed
files; follow `--cursor CURSOR` when a page is not complete.

## Run an External Tool

Choose a trusted local directory or source repository. Verify its source,
revision, dependencies, and build instructions, then build or install it in an
environment that can access the execution host. Slipstream does not install,
execute, sandbox, or attest the tool.

Pass the protected input path and the disclosed output directory to the tool's
own interface. When it finishes, run `slipstream workspace files WORKSPACE_ID`
and inspect the completed file's actual facts. The result is a Workspace
Artifact; do not claim it is a validated Processing Artifact.

Official `darktable-cli` is an External Tool and does not share `darktable-edit`
state. Do not pass an Access Token to an External Tool.

## Inspect and download

```text
slipstream workspace inspect WORKSPACE_ID ARTIFACT_ID
slipstream workspace download WORKSPACE_ID ARTIFACT_ID --file NEW_PATH
```

Download the checked revision. A changed or unavailable file refuses; do not
render again or pick whatever output is newest.

## Remove

Remove only a file whose current revision you inspected:

```text
slipstream workspace remove WORKSPACE_ID ARTIFACT_ID --revision REVISION --request REQUEST_ID
```

Keep the same request identity when an outcome is unknown and reconcile from
`workspace files` before another attempt. Named dependencies and active use
refuse removal. Removing the default input leaves no default; it does not delete
the Library Original or unrelated work.
