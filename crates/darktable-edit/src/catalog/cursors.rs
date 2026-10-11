use super::*;
pub(super) fn list(c: &Catalog, args: &Value, kind: &str) -> Result<Value, ToolError> {
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50);
    if limit == 0 || limit > 60 {
        return Err(error("invalid_request", "limit must be 1..60"));
    }
    let mut q =
        c.db.prepare("SELECT record FROM records WHERE kind=? ORDER BY id,version")
            .map_err(io)?;
    let rows = q.query_map([kind], |r| r.get::<_, String>(0)).map_err(io)?;
    let mut records = Vec::new();
    for row in rows {
        let mut v: Value = serde_json::from_str(&row.map_err(io)?).map_err(io)?;
        if let Some(k) = args.get("kind")
            && v["kind"] != *k
        {
            continue;
        }
        if let Some(m) = args.get("moduleId")
            && v["settings"]["moduleId"] != *m
        {
            continue;
        }
        if kind != "resource" {
            v.as_object_mut().unwrap().retain(|k, _| {
                [
                    "id",
                    "version",
                    "kind",
                    "name",
                    "bundleId",
                    "schemaVersion",
                    "createdAt",
                    "availability",
                ]
                .contains(&k.as_str())
            });
        }
        records.push(v);
    }
    let operation = match kind {
        "resource" => "resources list",
        "preset" => "presets list",
        _ => "styles list",
    };
    let state = if c.descriptor["scope"] == "edit-local" {
        Some(c.dir.path())
    } else {
        None
    };
    crate::continuations::window(
        records,
        args,
        operation,
        state,
        json!({"catalog":c.descriptor}),
    )
}
