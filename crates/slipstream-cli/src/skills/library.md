# Find, inspect, and decide

Use this skill to discover Photos, inspect a Preview, and make checked Library
decisions. Read the current facts before every mutation.

## Find

Run `slipstream status` to confirm the service and Library state. Start with
`slipstream albums list` or `slipstream folders list`. Query one Album with:

```text
slipstream photos list --album ALBUM_ID --selection picked --order capture-time-asc --limit 60
```

Use `--rating-min` and `--rating-max` when the request needs them. Preserve the
returned order and total. Follow `nextCursor` with:

```text
slipstream photos list --cursor CURSOR
```

Do not assume the next page is stable while the Library changes. Re-query when
the task needs a captured set.

## Inspect

Run `slipstream photos get PHOTO_ID` for current facts and metadata. Download a
Preview only when the task needs pixels:

```text
slipstream photos preview PHOTO_ID --file NEW_PATH --size review
```

A Preview is a display rendition, not a full-resolution Original. Check its
`sourceRevision`, `source`, and dimensions. `path` and `webUrl` identify the
same Photo. Present the local JPEG when the Agent can display images. Offer
`webUrl` for interactive Web review. Originals stay read-only.

## Decide

For one Photo, use the current decision version returned by the read:

```text
slipstream photos set PHOTO_ID --selection picked --if-version VERSION
```

Use `--rating N` for a Rating and `--selection` for Unflagged, Picked, or
Rejected. For a batch, write only explicitly chosen IDs and observed versions
to a bounded input file and pass it with `--input FILE`.

A conflict means the observed version changed. Read again and make a new
decision; do not silently replace the guard. A partial batch reports each
Photo separately. On `outcome_unknown`, inspect current state and state that
historical attribution may remain unknown.

## Albums

Create with `slipstream albums create --name NAME`. Add or remove members with
`albums add` or `albums remove` and the Album version observed by `albums get`.
Use only the returned IDs and versions; Album names are not identities.
