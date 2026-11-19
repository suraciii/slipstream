# Edit, preview, and export with darktable

Use this skill for the optional service-supervised darktable integration. Read
`slipstream darktable --help` and the selected operation's `--help`; the native
grammar is the authority for exact options and schemas.

## Inspect the edit

```text
slipstream darktable edit show --photo PHOTO_ID --edit EDIT_ID
```

The result identifies the workspace input, saved state, revision, availability,
and native schema. Preserve the observed state token and actual input contract.
Discovery is read-only; do not initialize camera defaults or assume operation
IDs.

## Change and render

Use a discovered module or action with the observed guard and a caller-retained
request identity. Read again after a conflict. Never silently refresh a supplied
guard or treat equal settings as proof that an uncertain write committed.

Render a completed Preview from the saved revision:

```text
slipstream darktable preview render --photo PHOTO_ID --edit EDIT_ID --request REQUEST_ID
```

The result identifies the captured revision, display conversion, geometry,
detail limit, and completed file. A prior Preview remains available while a
replacement renders. A late older result is not current.

## Export

Inspect and save a compatible output combination, then run:

```text
slipstream darktable export create --photo PHOTO_ID --edit EDIT_ID --request REQUEST_ID
```

Export captures the confirmed revision; a Preview is not required and is never
upscaled. Keep request and revision identities. A failed or uncertain result
does not discard saved state or earlier completed files.

## Collect the file

The completed Export response carries an ID. Download it through the same
integration when a caller-local file is needed:

```text
slipstream darktable export download EXPORT_ID --file NEW_PATH
```

Run `slipstream workspace files WORKSPACE_ID` to find the completed output and
its checked revision. Download it with `slipstream workspace download
WORKSPACE_ID ARTIFACT_ID --file NEW_PATH`, or hand its actual path to the next
tool on the same execution host after checking that tool's input contract.
Preview quality is not automatically a full-resolution scene-linear input.
