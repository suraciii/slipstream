#!/usr/bin/env bash
set -euo pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

fail() {
  printf 'state backup failed: %s\n' "$1" >&2
  exit 1
}

state_directory=${SLIPSTREAM_STATE_DIRECTORY:-}
database_basename=${SLIPSTREAM_DATABASE_BASENAME:-library.sqlite}
library_root=${SLIPSTREAM_LIBRARY_ROOT:-}
expected_schema=${SLIPSTREAM_EXPECTED_SCHEMA_VERSION:-}
output=${SLIPSTREAM_BACKUP_OUTPUT:-}
[[ -n "$state_directory" ]] || fail 'set SLIPSTREAM_STATE_DIRECTORY'
[[ -n "$library_root" ]] || fail 'set SLIPSTREAM_LIBRARY_ROOT'
[[ "$expected_schema" =~ ^[0-9]+$ ]] || fail 'set SLIPSTREAM_EXPECTED_SCHEMA_VERSION to an integer'
[[ -n "$output" ]] || fail 'set SLIPSTREAM_BACKUP_OUTPUT'
[[ "$database_basename" != */* && "$database_basename" != '.' && "$database_basename" != '..' ]] \
  || fail 'database basename is invalid'
[[ -d "$state_directory" ]] || fail 'state directory is missing'
canonical_state_directory=$(realpath -- "$state_directory") || fail 'state directory cannot be canonicalized'
[[ "$state_directory" == "$canonical_state_directory" ]] || fail 'state directory must be its canonical absolute path'
state_directory=$canonical_state_directory
[[ -d "$library_root" ]] || fail 'Library root is missing'
canonical_library_root=$(realpath -- "$library_root") || fail 'Library root cannot be canonicalized'
[[ "$library_root" == "$canonical_library_root" ]] || fail 'Library root must be its canonical absolute path'
library_root=$canonical_library_root
database="$state_directory/$database_basename"
[[ -f "$database" && ! -L "$database" ]] || fail 'state database is missing or is not a regular file'
[[ "$output" == /* ]] || fail 'backup output must be an absolute path'
requested_output_parent=$(dirname -- "$output")
[[ -d "$requested_output_parent" ]] || fail 'backup output parent must already exist'
output_parent=$(realpath -- "$requested_output_parent") || fail 'backup output parent cannot be canonicalized'
[[ "$requested_output_parent" == "$output_parent" ]] || fail 'backup output parent must be canonical'
output="$output_parent/$(basename -- "$output")"
[[ "$output" != "$state_directory" && "$output" != "$state_directory"/* ]] || fail 'backup output must be outside the state directory'
[[ "$output" != "$library_root" && "$output" != "$library_root"/* ]] || fail 'backup output must be outside the Library root'
[[ ! -e "$output" ]] || fail 'backup output already exists'

# State currently owns one SQLite database. Reject hidden sidecars, alternate
# databases, symlinks, sockets, or operator scratch files rather than silently
# producing a partial recovery archive.
mapfile -d '' state_entries < <(find "$state_directory" -mindepth 1 -maxdepth 1 -print0)
[[ ${#state_entries[@]} -eq 1 && "${state_entries[0]}" == "$database" ]] \
  || fail 'state directory must contain only the configured database'
for suffix in -journal -wal -shm; do
  [[ ! -e "$database$suffix" ]] || fail "SQLite sidecar is present: $database$suffix"
done

work_dir=$(mktemp -d)
temporary="$output.partial.$$"
cleanup() {
  rm -rf -- "$work_dir"
  rm -f -- "$temporary"
}
trap cleanup EXIT
snapshot_root="$work_dir/$(basename -- "$state_directory")"
mkdir -m 0700 -- "$snapshot_root"
snapshot_database="$snapshot_root/$database_basename"
schema_manifest="$repo_root/compatibility/sqlite/schema-v$expected_schema.json"
[[ -f "$schema_manifest" ]] || fail "canonical schema manifest is missing: schema-v$expected_schema.json"

# sqlite3.Connection.backup creates a transactionally consistent database
# snapshot. This avoids an unsafe sequential file copy even if an operator
# accidentally invokes the command before the stopped-service check in the
# deployment procedure. It never writes the source database.
python3 - "$database" "$snapshot_database" "$expected_schema" "$library_root" "$schema_manifest" <<'PY' || fail 'SQLite consistent snapshot failed'
import json, os, sqlite3, sys
source_path, destination_path, expected_schema, expected_root, manifest_path = sys.argv[1:]

def compact_sql(value):
    return "".join(character for character in value.lower() if not character.isspace() and character not in '\"`[]')

def schema_manifest(connection):
    objects = []
    rows = connection.execute("SELECT type,name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name")
    for kind, name, sql in rows:
        item = {"type": kind, "name": name, "sql": compact_sql(sql)}
        if kind == "table":
            item["columns"] = [
                {"name": row[1], "type": row[2], "notNull": bool(row[3]), "default": row[4], "primaryKey": row[5]}
                for row in connection.execute(f'PRAGMA table_info("{name}")')
            ]
            item["foreignKeys"] = [
                {"table": row[2], "from": row[3], "to": row[4], "onUpdate": row[5], "onDelete": row[6]}
                for row in connection.execute(f'PRAGMA foreign_key_list("{name}")')
            ]
        objects.append(item)
    return {"userVersion": connection.execute("PRAGMA user_version").fetchone()[0], "objects": objects}

with open(manifest_path, encoding="utf-8") as manifest_source:
    expected_manifest = json.load(manifest_source)
source = sqlite3.connect(f"file:{source_path}?mode=ro", uri=True)
destination = sqlite3.connect(destination_path)
try:
    if source.execute("PRAGMA quick_check").fetchone() != ("ok",):
        raise SystemExit("source quick_check failed")
    if source.execute("PRAGMA foreign_key_check").fetchone() is not None:
        raise SystemExit("source foreign_key_check failed")
    if schema_manifest(source) != expected_manifest:
        raise SystemExit("source canonical schema mismatch")
    root = source.execute("SELECT value FROM library_metadata WHERE key='canonical_root'").fetchone()
    if root != (expected_root,):
        raise SystemExit("source Library binding mismatch")
    source.backup(destination)
    if destination.execute("PRAGMA quick_check").fetchone() != ("ok",):
        raise SystemExit("snapshot quick_check failed")
    if destination.execute("PRAGMA foreign_key_check").fetchone() is not None:
        raise SystemExit("snapshot foreign_key_check failed")
    if schema_manifest(destination) != expected_manifest:
        raise SystemExit("snapshot canonical schema mismatch")
    root = destination.execute("SELECT value FROM library_metadata WHERE key='canonical_root'").fetchone()
    if root != (expected_root,):
        raise SystemExit("snapshot Library binding mismatch")
    destination.commit()
finally:
    destination.close()
    source.close()
os.chmod(destination_path, 0o600)
PY
for suffix in -journal -wal -shm; do
  [[ ! -e "$database$suffix" ]] || fail "SQLite sidecar appeared during snapshot: $database$suffix"
  [[ ! -e "$snapshot_database$suffix" ]] || fail "snapshot contains SQLite sidecar: $snapshot_database$suffix"
done
snapshot_sha=$(sha256sum -- "$snapshot_database" | awk '{print $1}')

tar --xattrs --acls -C "$work_dir" -czf "$temporary" "$(basename -- "$state_directory")" \
  || fail 'could not create state archive'
verify_root="$work_dir/verify"
mkdir -- "$verify_root"
tar -xzf "$temporary" -C "$verify_root" || fail 'could not extract state archive for verification'
restored_root="$verify_root/$(basename -- "$state_directory")"
restored="$restored_root/$database_basename"
mapfile -d '' restored_entries < <(find "$restored_root" -mindepth 1 -maxdepth 1 -print0)
[[ ${#restored_entries[@]} -eq 1 && "${restored_entries[0]}" == "$restored" ]] \
  || fail 'verified archive contains an unexpected state entry'
[[ -f "$restored" && ! -L "$restored" ]] || fail 'verified archive does not contain a regular database'
python3 - "$restored" "$expected_schema" "$library_root" "$schema_manifest" <<'PY' || fail 'restored database integrity check failed'
import json, sqlite3, sys
connection = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro&immutable=1", uri=True)

def compact_sql(value):
    return "".join(character for character in value.lower() if not character.isspace() and character not in '\"`[]')

def actual_manifest(connection):
    objects = []
    for kind, name, sql in connection.execute("SELECT type,name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name"):
        item = {"type": kind, "name": name, "sql": compact_sql(sql)}
        if kind == "table":
            item["columns"] = [{"name": row[1], "type": row[2], "notNull": bool(row[3]), "default": row[4], "primaryKey": row[5]} for row in connection.execute(f'PRAGMA table_info("{name}")')]
            item["foreignKeys"] = [{"table": row[2], "from": row[3], "to": row[4], "onUpdate": row[5], "onDelete": row[6]} for row in connection.execute(f'PRAGMA foreign_key_list("{name}")')]
        objects.append(item)
    return {"userVersion": connection.execute("PRAGMA user_version").fetchone()[0], "objects": objects}
with open(sys.argv[4], encoding="utf-8") as source:
    expected_manifest = json.load(source)
try:
    if connection.execute("PRAGMA quick_check").fetchone() != ("ok",):
        raise SystemExit("quick_check")
    if connection.execute("PRAGMA foreign_key_check").fetchone() is not None:
        raise SystemExit("foreign_key_check")
    if actual_manifest(connection) != expected_manifest:
        raise SystemExit("canonical schema")
    root = connection.execute("SELECT value FROM library_metadata WHERE key='canonical_root'").fetchone()
    if root != (sys.argv[3],):
        raise SystemExit("Library binding")
finally:
    connection.close()
PY
restored_sha=$(sha256sum -- "$restored" | awk '{print $1}')
[[ "$restored_sha" == "$snapshot_sha" ]] || fail 'archive database bytes differ from the verified snapshot'

mv -- "$temporary" "$output" || fail 'could not publish verified backup'
trap - EXIT
rm -rf -- "$work_dir"
printf 'verified consistent state backup created: %s\n' "$output"
printf 'database_sha256=%s\n' "$snapshot_sha"
