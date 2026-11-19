# Trash, removal, Restore, and Location Recovery

Use this skill for explicit Photo removal, Trash Record Cleanup, Restore, and
unavailable-Original recovery. Preserve the returned evidence; never substitute
a current query or a new operation ID for a lost receipt.

## Remove Photos

Inspect the exact rejected Photos and write their `photoId`, `selectionState`,
`decisionVersion`, and `removedAtMs` to a bounded input file:

```text
slipstream photos remove OPERATION_ID --input FILE
slipstream photos removal-operation OPERATION_ID
```

Inspect the operation even after a successful response and reconcile every
Photo. A changed or unavailable Photo is not proof of removal.

## Trash records

`slipstream trash list` shows Locations relative to the server's Library Folder.
Delete intended files only outside Slipstream after checking that namespace.
Then clean application records:

```text
slipstream trash cleanup OPERATION_ID --input FILE
slipstream trash cleanup-operation OPERATION_ID
```

Cleanup removes records and Album membership, not files. Present, replaced,
inaccessible, unsafe, stale, or dependency-blocked targets are refused. An
empty Trash needs no mutation. `trash empty` is the reviewed whole-set form.

## Restore

Use only confirmed removal evidence:

```text
slipstream photos restore OPERATION_ID --input FILE
slipstream photos restore-operation OPERATION_ID
```

A `changed-elsewhere` or `unavailable` result is not restoration. Query the
Photos again before reporting success.

## Location Recovery

Use `slipstream recovery unavailable` to inspect the bounded unavailable set,
`recovery propose` to evaluate reviewed mappings without writing, and
`recovery apply` to commit one reviewed batch. Preserve mapping identity and
markers. Location Recovery is not Trash Restore and never deletes Original
Files.

On an uncertain outcome, inspect current state and say that historical
attribution may remain unknown. Do not retry with a fresh identity merely
because a response was lost.
