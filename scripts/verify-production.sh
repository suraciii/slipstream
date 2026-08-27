#!/usr/bin/env bash
set -euo pipefail

fail() {
  printf 'production verification failed: %s\n' "$1" >&2
  exit 1
}

require_value() {
  local name=$1
  [[ -n "${!name:-}" ]] || fail "set $name"
}

for name in \
  SLIPSTREAM_BASE_URL \
  SLIPSTREAM_EXPECTED_PHOTO_COUNT \
  SLIPSTREAM_EXPECTED_PHOTO_SET \
  SLIPSTREAM_EXPECTED_MEMBER_COUNT \
  SLIPSTREAM_EXPECTED_SCHEMA_VERSION \
  SLIPSTREAM_CONTAINER \
  SLIPSTREAM_LIBRARY_ROOT \
  SLIPSTREAM_STATE_DIRECTORY \
  SLIPSTREAM_CACHE_DIRECTORY \
  SLIPSTREAM_DATABASE_BASENAME \
  SLIPSTREAM_EXPECTED_BIND_ADDRESS \
  SLIPSTREAM_EXPECTED_PORT \
  SLIPSTREAM_EXPECTED_IMAGE \
  SLIPSTREAM_EXPECTED_IMAGE_ID \
  SLIPSTREAM_EXPECTED_COMMIT \
  SLIPSTREAM_EXPECTED_STATE_SNAPSHOT \
  SLIPSTREAM_PREVIEW_PHOTO_ID \
  SLIPSTREAM_EXPECTED_PREVIEW_SOURCE \
  SLIPSTREAM_ORIGINAL_SAMPLE \
  SLIPSTREAM_EXPECTED_ORIGINAL_SHA256; do
  require_value "$name"
done

[[ "$SLIPSTREAM_EXPECTED_PHOTO_COUNT" =~ ^[0-9]+$ ]] || fail 'expected Photo count must be an integer'
[[ "$SLIPSTREAM_EXPECTED_MEMBER_COUNT" =~ ^[0-9]+$ ]] || fail 'expected member count must be an integer'
[[ "$SLIPSTREAM_EXPECTED_SCHEMA_VERSION" =~ ^[0-9]+$ ]] || fail 'expected schema version must be an integer'
[[ "$SLIPSTREAM_EXPECTED_PORT" =~ ^[0-9]+$ ]] && ((SLIPSTREAM_EXPECTED_PORT >= 1 && SLIPSTREAM_EXPECTED_PORT <= 65535)) \
  || fail 'expected port is invalid'
[[ "$SLIPSTREAM_DATABASE_BASENAME" != */* && "$SLIPSTREAM_DATABASE_BASENAME" != '.' && "$SLIPSTREAM_DATABASE_BASENAME" != '..' ]] \
  || fail 'database basename is invalid'
[[ "$SLIPSTREAM_EXPECTED_IMAGE_ID" == sha256:* ]] || fail 'expected image ID must start with sha256:'
[[ "$SLIPSTREAM_EXPECTED_COMMIT" =~ ^[0-9a-f]{7,40}$ ]] || fail 'expected commit must be a lowercase Git SHA'
[[ "$SLIPSTREAM_BASE_URL" != */ ]] || fail 'base URL must not end with /'
[[ -f "$SLIPSTREAM_EXPECTED_STATE_SNAPSHOT" && ! -L "$SLIPSTREAM_EXPECTED_STATE_SNAPSHOT" ]] \
  || fail 'expected state snapshot is missing or is not a regular file'
[[ "$SLIPSTREAM_EXPECTED_PREVIEW_SOURCE" == matching-jpeg || "$SLIPSTREAM_EXPECTED_PREVIEW_SOURCE" == embedded-raw-jpeg ]] \
  || fail 'expected Preview Source is invalid'
for name in SLIPSTREAM_LIBRARY_ROOT SLIPSTREAM_STATE_DIRECTORY SLIPSTREAM_CACHE_DIRECTORY; do
  [[ -d "${!name}" ]] || fail "$name is not a directory"
  canonical=$(realpath -- "${!name}") || fail "$name cannot be canonicalized"
  [[ "${!name}" == "$canonical" ]] || fail "$name must be its canonical absolute path"
done
python3 - "$SLIPSTREAM_LIBRARY_ROOT" "$SLIPSTREAM_STATE_DIRECTORY" "$SLIPSTREAM_CACHE_DIRECTORY" <<'PY' \
  || fail 'Library, state, and cache directories overlap'
import os, sys
values = [os.path.realpath(value) for value in sys.argv[1:]]
for index, left in enumerate(values):
    for right in values[index + 1:]:
        if os.path.commonpath([left, right]) in (left, right):
            raise SystemExit("overlap")
PY
SLIPSTREAM_STATE_DATABASE="$SLIPSTREAM_STATE_DIRECTORY/$SLIPSTREAM_DATABASE_BASENAME"
[[ -f "$SLIPSTREAM_STATE_DATABASE" && ! -L "$SLIPSTREAM_STATE_DATABASE" ]] || fail 'state database is missing or is not a regular file'
python3 - "$SLIPSTREAM_BASE_URL" "$SLIPSTREAM_EXPECTED_BIND_ADDRESS" "$SLIPSTREAM_EXPECTED_PORT" <<'PY' \
  || fail 'base URL does not match the expected bind address and port'
import sys, urllib.parse
value = urllib.parse.urlparse(sys.argv[1])
if value.scheme != "http" or value.hostname != sys.argv[2] or value.port != int(sys.argv[3]) or value.path not in ("", "/"):
    raise SystemExit("URL mismatch")
PY

work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' EXIT

[[ "$SLIPSTREAM_EXPECTED_ORIGINAL_SHA256" =~ ^[0-9a-f]{64}$ ]] || fail 'expected Original SHA-256 is invalid'
[[ -f "$SLIPSTREAM_ORIGINAL_SAMPLE" && ! -L "$SLIPSTREAM_ORIGINAL_SAMPLE" ]] || fail 'Original sample is missing or is not a regular file'
canonical_original_sample=$(realpath -- "$SLIPSTREAM_ORIGINAL_SAMPLE") || fail 'Original sample cannot be canonicalized'
[[ "$SLIPSTREAM_ORIGINAL_SAMPLE" == "$canonical_original_sample" ]] || fail 'Original sample must be its canonical path'
[[ "$canonical_original_sample" == "$SLIPSTREAM_LIBRARY_ROOT"/* ]] || fail 'Original sample is outside the Library root'
original_sha=$(sha256sum -- "$canonical_original_sample" | awk '{print $1}')
[[ "$original_sha" == "$SLIPSTREAM_EXPECTED_ORIGINAL_SHA256" ]] || fail 'Original sample SHA-256 does not match before verification'

for suffix in -journal -wal -shm; do
  [[ ! -e "$SLIPSTREAM_STATE_DATABASE$suffix" ]] || fail "SQLite sidecar is present: $SLIPSTREAM_STATE_DATABASE$suffix"
done
python3 - "$SLIPSTREAM_STATE_DATABASE" "$SLIPSTREAM_EXPECTED_SCHEMA_VERSION" "$SLIPSTREAM_LIBRARY_ROOT" <<'PY' \
  || fail 'state database integrity, schema, or Library binding does not match'
import sqlite3, sys
path, expected_version, expected_root = sys.argv[1:]
connection = sqlite3.connect(f"file:{path}?mode=ro&immutable=1", uri=True)
try:
    if connection.execute("PRAGMA quick_check").fetchone() != ("ok",):
        raise SystemExit("quick_check")
    if connection.execute("PRAGMA user_version").fetchone()[0] != int(expected_version):
        raise SystemExit("schema version")
    root = connection.execute("SELECT value FROM library_metadata WHERE key='canonical_root'").fetchone()
    if root != (expected_root,):
        raise SystemExit("Library binding")
finally:
    connection.close()
PY
state_sha=$(sha256sum -- "$SLIPSTREAM_STATE_DATABASE" | awk '{print $1}')

health=$(curl --noproxy '*' --fail --silent --show-error "$SLIPSTREAM_BASE_URL/healthz") \
  || fail '/healthz request failed'
[[ "$health" == '{"status":"ok"}' ]] || fail '/healthz response is not exact'

curl --noproxy '*' --fail --silent --show-error "$SLIPSTREAM_BASE_URL/" >"$work_dir/index.html" \
  || fail 'Web entry request failed'
grep -Fq 'Slipstream' "$work_dir/index.html" || fail 'Web entry does not identify Slipstream'

curl --noproxy '*' --fail --silent --show-error "$SLIPSTREAM_BASE_URL/api/photos" >"$work_dir/photos.json" \
  || fail 'Photo list request failed'
curl --noproxy '*' --fail --silent --show-error "$SLIPSTREAM_BASE_URL/api/photo-sets" >"$work_dir/photo-sets.json" \
  || fail 'Photo Set request failed'

python3 - "$work_dir/photos.json" "$work_dir/photo-sets.json" "$SLIPSTREAM_EXPECTED_STATE_SNAPSHOT" \
  "$SLIPSTREAM_EXPECTED_PHOTO_COUNT" "$SLIPSTREAM_EXPECTED_PHOTO_SET" \
  "$SLIPSTREAM_EXPECTED_MEMBER_COUNT" <<'PY' || fail 'Photo or Photo Set state does not match'
import json, sys
photos_path, sets_path, expected_path, expected_photos, expected_set, expected_members = sys.argv[1:]
with open(photos_path, encoding="utf-8") as source:
    photos = json.load(source)
with open(sets_path, encoding="utf-8") as source:
    sets = json.load(source)
with open(expected_path, encoding="utf-8") as source:
    expected = json.load(source)
if expected != {"photos": photos.get("photos"), "photoSets": sets.get("photoSets")}:
    raise SystemExit("exact persisted state snapshot mismatch")
if set(photos) != {"photos"} or not isinstance(photos["photos"], list):
    raise SystemExit("invalid Photo response")
if len(photos["photos"]) != int(expected_photos):
    raise SystemExit("Photo count mismatch")
photo_fields = {"id", "available", "ambiguous", "originals", "selectionState", "rating", "preview"}
for photo in photos["photos"]:
    if not photo_fields.issubset(photo):
        raise SystemExit("Photo response omits persisted review or Preview facts")
    if photo["selectionState"] not in ("undecided", "selected", "rejected") or not isinstance(photo["rating"], int):
        raise SystemExit("Photo decision facts are invalid")
if set(sets) != {"photoSets"} or not isinstance(sets["photoSets"], list):
    raise SystemExit("invalid Photo Set response")
matches = [value for value in sets["photoSets"] if value.get("name") == expected_set]
if len(matches) != 1:
    raise SystemExit("expected exactly one named Photo Set")
if len(matches[0].get("members", [])) != int(expected_members):
    raise SystemExit("Photo Set member count mismatch")
member_fields = {"photoId", "position", "available", "selectionState", "rating"}
for member in matches[0]["members"]:
    if not member_fields.issubset(member):
        raise SystemExit("Photo Set member omits order or review state")
if "lastReviewedPhotoId" not in matches[0]:
    raise SystemExit("Photo Set progress is not present in the expected acceptance Set")
PY

python3 - "$work_dir/photos.json" "$SLIPSTREAM_PREVIEW_PHOTO_ID" "$SLIPSTREAM_EXPECTED_PREVIEW_SOURCE" <<'PY' \
  || fail 'persisted Preview facts do not match'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source:
    values = json.load(source)["photos"]
matches = [value for value in values if value.get("id") == sys.argv[2]]
if len(matches) != 1:
    raise SystemExit("expected Preview Photo is missing or duplicated")
preview = matches[0].get("preview") or {}
if preview.get("state") != "ready" or preview.get("source") != sys.argv[3]:
    raise SystemExit("Preview facts are not ready from the expected source")
if not isinstance(preview.get("width"), int) or preview["width"] <= 0:
    raise SystemExit("Preview width is invalid")
if not isinstance(preview.get("height"), int) or preview["height"] <= 0:
    raise SystemExit("Preview height is invalid")
PY

docker inspect "$SLIPSTREAM_CONTAINER" >"$work_dir/container.json" \
  || fail 'running container cannot be inspected'

if [[ "${SLIPSTREAM_ALLOW_DERIVED_WRITES:-0}" == 1 ]]; then
  require_value SLIPSTREAM_ISOLATED_RESTORE_ROOT
  require_value SLIPSTREAM_PRODUCTION_STATE_DIRECTORY
  require_value SLIPSTREAM_PRODUCTION_CACHE_DIRECTORY
  command -v vips >/dev/null 2>&1 || fail 'vips CLI is required for complete isolated JPEG decoding'
  [[ -d "$SLIPSTREAM_ISOLATED_RESTORE_ROOT" ]] || fail 'isolated restore root is missing'
  canonical_restore_root=$(realpath -- "$SLIPSTREAM_ISOLATED_RESTORE_ROOT") || fail 'isolated restore root cannot be canonicalized'
  [[ "$SLIPSTREAM_ISOLATED_RESTORE_ROOT" == "$canonical_restore_root" ]] || fail 'isolated restore root must be canonical'
  [[ "$SLIPSTREAM_STATE_DIRECTORY" == "$canonical_restore_root"/* ]] || fail 'derived writes require state below the isolated restore root'
  [[ "$SLIPSTREAM_CACHE_DIRECTORY" == "$canonical_restore_root"/* ]] || fail 'derived writes require cache below the isolated restore root'
  [[ "$SLIPSTREAM_LIBRARY_ROOT" != "$canonical_restore_root" && "$SLIPSTREAM_LIBRARY_ROOT" != "$canonical_restore_root"/* ]] \
    || fail 'isolated restore root must not contain the Library'
  for name in SLIPSTREAM_PRODUCTION_STATE_DIRECTORY SLIPSTREAM_PRODUCTION_CACHE_DIRECTORY; do
    [[ -d "${!name}" ]] || fail "$name is missing"
    production_path=$(realpath -- "${!name}") || fail "$name cannot be canonicalized"
    [[ "${!name}" == "$production_path" ]] || fail "$name must be canonical"
    for isolated_path in "$canonical_restore_root" "$SLIPSTREAM_STATE_DIRECTORY" "$SLIPSTREAM_CACHE_DIRECTORY"; do
      [[ "$isolated_path" != "$production_path" && "$isolated_path" != "$production_path"/* \
        && "$production_path" != "$isolated_path"/* ]] \
        || fail 'isolated restore paths overlap production state/cache'
    done
  done
  [[ "$SLIPSTREAM_EXPECTED_BIND_ADDRESS" == 127.0.0.1 || "$SLIPSTREAM_EXPECTED_BIND_ADDRESS" == ::1 ]] \
    || fail 'derived-write verification must use a loopback listener'
  python3 - "$work_dir/container.json" "$SLIPSTREAM_LIBRARY_ROOT" "$SLIPSTREAM_STATE_DIRECTORY" \
    "$SLIPSTREAM_CACHE_DIRECTORY" "$SLIPSTREAM_DATABASE_BASENAME" "$SLIPSTREAM_EXPECTED_BIND_ADDRESS" \
    "$SLIPSTREAM_EXPECTED_PORT" <<'PY' || fail 'container isolation was not proven before derived writes'
import json, sys
path, library, state, cache, database, bind, port = sys.argv[1:]
with open(path, encoding="utf-8") as source: value = json.load(source)[0]
environment = dict(item.split("=", 1) for item in value.get("Config", {}).get("Env", []) if "=" in item)
if environment.get("SLIPSTREAM_LIBRARY_ROOT") != library or environment.get("SLIPSTREAM_STATE_DIRECTORY") != "/state" or environment.get("SLIPSTREAM_CACHE_DIRECTORY") != "/cache" or environment.get("SLIPSTREAM_DATABASE_BASENAME") != database:
    raise SystemExit("environment")
mounts = {(item.get("Source"), item.get("Destination"), item.get("RW"), item.get("Type"), item.get("Propagation")) for item in value.get("Mounts", [])}
expected = {(library, library, False, "bind", "rprivate"), (state, "/state", True, "bind", "rprivate"), (cache, "/cache", True, "bind", "rprivate")}
if mounts != expected: raise SystemExit("mounts")
ports = value.get("NetworkSettings", {}).get("Ports", {})
if set(key for key, bindings in ports.items() if bindings) != {"3000/tcp"}: raise SystemExit("published ports")
bindings = ports.get("3000/tcp") or []
if len(bindings) != 1 or bindings[0].get("HostIp") != bind or bindings[0].get("HostPort") != port: raise SystemExit("binding")
if value.get("State", {}).get("Health", {}).get("Status") != "healthy": raise SystemExit("health")
PY
  if find "$SLIPSTREAM_CACHE_DIRECTORY" -mindepth 1 -type f -print -quit | grep -q .; then
    fail 'isolated derived-write verification requires no pre-existing cache files'
  fi
  curl --noproxy '*' --fail --silent --show-error \
    "$SLIPSTREAM_BASE_URL/api/photos/$SLIPSTREAM_PREVIEW_PHOTO_ID/preview" >"$work_dir/preview.json" \
    || fail 'isolated Preview request failed'
  preview_url=$(python3 - "$work_dir/preview.json" "$SLIPSTREAM_EXPECTED_PREVIEW_SOURCE" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source:
    value = json.load(source)
if value.get("state") != "ready" or value.get("source") != sys.argv[2] or value.get("stale") is not False:
    raise SystemExit("Preview response is not current from the expected source")
url = value.get("url")
if not isinstance(url, str) or not url.startswith("/api/derivatives/"):
    raise SystemExit("Preview URL")
print(url)
PY
  ) || fail 'isolated Preview response does not match'
  curl --noproxy '*' --fail --silent --show-error "$SLIPSTREAM_BASE_URL$preview_url" >"$work_dir/preview.jpg" \
    || fail 'isolated derivative request failed'
  vips copy "$work_dir/preview.jpg" "$work_dir/decoded.v" >/dev/null 2>&1 \
    || fail 'isolated derivative is not a completely decodable JPEG'
  if ! find "$SLIPSTREAM_CACHE_DIRECTORY" -mindepth 1 -type f -print -quit | grep -q .; then
    fail 'isolated Preview request did not populate the empty cache'
  fi
  curl --noproxy '*' --fail --silent --show-error "$SLIPSTREAM_BASE_URL/api/photos" >"$work_dir/photos-after.json" \
    || fail 'post-Preview Photo state request failed'
  curl --noproxy '*' --fail --silent --show-error "$SLIPSTREAM_BASE_URL/api/photo-sets" >"$work_dir/sets-after.json" \
    || fail 'post-Preview Photo Set state request failed'
  python3 - "$work_dir/photos-after.json" "$work_dir/sets-after.json" "$SLIPSTREAM_EXPECTED_STATE_SNAPSHOT" <<'PY' \
    || fail 'user review state changed during isolated Preview verification'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source: photos = json.load(source)["photos"]
with open(sys.argv[2], encoding="utf-8") as source: sets = json.load(source)["photoSets"]
with open(sys.argv[3], encoding="utf-8") as source: expected = json.load(source)
if {"photos": photos, "photoSets": sets} != expected: raise SystemExit("state mismatch")
PY
elif [[ "${SLIPSTREAM_ALLOW_DERIVED_WRITES:-0}" != 0 ]]; then
  fail 'SLIPSTREAM_ALLOW_DERIVED_WRITES must be 0 or 1'
fi

python3 - "$work_dir/container.json" \
  "$SLIPSTREAM_EXPECTED_IMAGE" "$SLIPSTREAM_EXPECTED_IMAGE_ID" "$SLIPSTREAM_EXPECTED_COMMIT" \
  "$SLIPSTREAM_LIBRARY_ROOT" "$SLIPSTREAM_STATE_DIRECTORY" "$SLIPSTREAM_CACHE_DIRECTORY" \
  "$SLIPSTREAM_DATABASE_BASENAME" "$SLIPSTREAM_EXPECTED_BIND_ADDRESS" "$SLIPSTREAM_EXPECTED_PORT" \
  <<'PY' || fail 'container boundary does not match'
import json, os, sys
(path, expected_image, expected_id, expected_commit, library, state, cache, database, bind, port) = sys.argv[1:]
with open(path, encoding="utf-8") as source:
    values = json.load(source)
if not isinstance(values, list) or len(values) != 1:
    raise SystemExit("invalid docker inspect response")
value = values[0]
if value.get("Config", {}).get("Image") != expected_image:
    raise SystemExit("image tag mismatch")
if value.get("Image") != expected_id:
    raise SystemExit("image ID mismatch")
if value.get("Config", {}).get("User") != "1000:1000":
    raise SystemExit("container user mismatch")
if value.get("HostConfig", {}).get("ReadonlyRootfs") is not True:
    raise SystemExit("root filesystem is writable")
if value.get("State", {}).get("Health", {}).get("Status") != "healthy":
    raise SystemExit("container is not healthy")
labels = value.get("Config", {}).get("Labels") or {}
if labels.get("org.opencontainers.image.revision") != expected_commit:
    raise SystemExit("image revision label mismatch")
environment = {}
for item in value.get("Config", {}).get("Env") or []:
    key, separator, item_value = item.partition("=")
    if separator:
        environment[key] = item_value
expected_environment = {
    "SLIPSTREAM_LIBRARY_ROOT": library,
    "SLIPSTREAM_STATE_DIRECTORY": "/state",
    "SLIPSTREAM_CACHE_DIRECTORY": "/cache",
    "SLIPSTREAM_DATABASE_BASENAME": database,
}
for key, item_value in expected_environment.items():
    if environment.get(key) != item_value:
        raise SystemExit(f"container environment mismatch: {key}")
mount_items = value.get("Mounts", [])
allowed_mounts = {(library, library), (state, "/state"), (cache, "/cache")}
actual_mounts = {(item.get("Source"), item.get("Destination")) for item in mount_items}
if actual_mounts != allowed_mounts or len(mount_items) != len(allowed_mounts):
    raise SystemExit("container has an unexpected or duplicate mount")
mounts = {(item.get("Source"), item.get("Destination")): item for item in mount_items}
original = mounts.get((library, library))
if original is None or original.get("RW") is not False:
    raise SystemExit("Originals mount is missing or writable")
state_mount = mounts.get((state, "/state"), {})
cache_mount = mounts.get((cache, "/cache"), {})
if state_mount.get("RW") is not True:
    raise SystemExit("state mount is missing or read-only")
if cache_mount.get("RW") is not True:
    raise SystemExit("cache mount is missing or read-only")
for item in (original, state_mount, cache_mount):
    if item.get("Type") != "bind" or item.get("Propagation") != "rprivate":
        raise SystemExit("mount is not a private bind mount")
for item in mount_items:
    destination = os.path.normpath(item.get("Destination") or "")
    if destination != library and destination.startswith(library + os.sep):
        raise SystemExit("nested mount shadows the Original tree")
import ipaddress
address = ipaddress.ip_address(bind)
shared_tailscale = address.version == 4 and address in ipaddress.ip_network("100.64.0.0/10")
if address.is_unspecified or not (address.is_loopback or address.is_private or shared_tailscale):
    raise SystemExit("host bind is wildcard or not a trusted local/Tailscale address")
published = value.get("NetworkSettings", {}).get("Ports", {})
other_ports = [key for key, bindings in published.items() if key != "3000/tcp" and bindings]
if other_ports:
    raise SystemExit("unexpected published container port")
ports = published.get("3000/tcp") or []
if len(ports) != 1 or ports[0].get("HostIp") != bind or ports[0].get("HostPort") != port:
    raise SystemExit("host bind or port mismatch")
PY

docker exec "$SLIPSTREAM_CONTAINER" /bin/sh -c '
  ! command -v node >/dev/null 2>&1 &&
  ! command -v bun >/dev/null 2>&1 &&
  ! command -v npm >/dev/null 2>&1 &&
  ! find /app /usr/local \( -iname "*sharp*" -o -iname "*node-api*" -o -iname "*.node" \) -print -quit | grep -q .
' || fail 'runtime container contains Node/Bun/npm/Sharp/Node-API'

if [[ -n "$original_sha" ]]; then
  final_sha=$(sha256sum -- "$SLIPSTREAM_ORIGINAL_SAMPLE" | awk '{print $1}')
  [[ "$final_sha" == "$original_sha" ]] || fail 'Original sample changed during verification'
fi
final_state_sha=$(sha256sum -- "$SLIPSTREAM_STATE_DATABASE" | awk '{print $1}')
if [[ "${SLIPSTREAM_ALLOW_DERIVED_WRITES:-0}" == 0 ]]; then
  [[ "$final_state_sha" == "$state_sha" ]] || fail 'state database changed during read-only verification'
fi

for suffix in -journal -wal -shm; do
  [[ ! -e "$SLIPSTREAM_STATE_DATABASE$suffix" ]] || fail "SQLite sidecar appeared: $SLIPSTREAM_STATE_DATABASE$suffix"
done

printf 'production verification passed\n'
printf 'commit=%s image=%s image_id=%s container=%s photos=%s photo_set=%s members=%s\n' \
  "$SLIPSTREAM_EXPECTED_COMMIT" "$SLIPSTREAM_EXPECTED_IMAGE" "$SLIPSTREAM_EXPECTED_IMAGE_ID" \
  "$SLIPSTREAM_CONTAINER" "$SLIPSTREAM_EXPECTED_PHOTO_COUNT" "$SLIPSTREAM_EXPECTED_PHOTO_SET" \
  "$SLIPSTREAM_EXPECTED_MEMBER_COUNT"
