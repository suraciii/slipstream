#!/usr/bin/env bash
set -euo pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
work_dir=$(mktemp -d)
cleanup() {
  [[ -z "${holder_pid:-}" ]] || kill "$holder_pid" >/dev/null 2>&1 || true
  rm -rf -- "$work_dir"
}
trap cleanup EXIT
mkdir -p "$work_dir/bin" "$work_dir/library" "$work_dir/state" "$work_dir/cache" "$work_dir/backups"
printf 'original bytes' >"$work_dir/library/sample.ARW"
original_sha=$(sha256sum "$work_dir/library/sample.ARW" | awk '{print $1}')
python3 - "$work_dir/state/library.sqlite" "$work_dir/library" "$repo_root/compatibility/sqlite/schema-v4.sql" <<'PY'
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
with open(sys.argv[3], encoding="utf-8") as source:
    connection.executescript(source.read())
connection.execute("INSERT INTO library_metadata VALUES ('canonical_root', ?)", (sys.argv[2],))
connection.execute("INSERT INTO library_metadata VALUES ('probe', 'persisted')")
connection.execute(
    "INSERT INTO original_files VALUES ('o1','a/one.ARW','raw',13,1.0,1,NULL,NULL,'pending',NULL,NULL,NULL,NULL)"
)
connection.execute(
    "INSERT INTO original_files VALUES ('o2','a/two.JPG','jpeg',13,1.0,1,NULL,NULL,'pending',NULL,NULL,NULL,NULL)"
)
connection.execute(
    "INSERT INTO photos VALUES ('one','o1',NULL,0,1,'ready','embedded-raw-jpeg','embedded-raw-jpeg','rev',2560,1707,'key','a/one.ARW','selected',5)"
)
connection.execute(
    "INSERT INTO photos VALUES ('two',NULL,'o2',0,1,'unavailable',NULL,NULL,NULL,NULL,NULL,NULL,'a/two.JPG','rejected',0)"
)
connection.execute("INSERT INTO photo_sets VALUES ('set','Shoot',1)")
connection.execute("INSERT INTO photo_set_members VALUES ('set','one',0)")
connection.execute("INSERT INTO photo_set_members VALUES ('set','two',1)")
connection.execute("INSERT INTO review_progress VALUES ('set','two')")
connection.commit()
connection.close()
PY

cat >"$work_dir/photos.json" <<'JSON'
{"photos":[{"id":"one","available":true,"ambiguous":false,"originals":[{"kind":"raw","available":true}],"selectionState":"selected","rating":5,"preview":{"state":"ready","source":"embedded-raw-jpeg","width":2560,"height":1707}},{"id":"two","available":true,"ambiguous":false,"originals":[{"kind":"jpeg","available":true}],"selectionState":"rejected","rating":0,"preview":{"state":"unavailable"}}]}
JSON
cat >"$work_dir/sets.json" <<'JSON'
{"photoSets":[{"id":"set","name":"Shoot","lastReviewedPhotoId":"two","members":[{"photoId":"one","position":0,"available":true,"selectionState":"selected","rating":5},{"photoId":"two","position":1,"available":true,"selectionState":"rejected","rating":0}]}]}
JSON
python3 - "$work_dir/photos.json" "$work_dir/sets.json" "$work_dir/expected-state.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source:
    photos = json.load(source)["photos"]
with open(sys.argv[2], encoding="utf-8") as source:
    sets = json.load(source)["photoSets"]
with open(sys.argv[3], "w", encoding="utf-8") as output:
    json.dump({"photos": photos, "photoSets": sets}, output)
PY
cat >"$work_dir/container.json" <<JSON
[{"Image":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","Config":{"Image":"slipstream:test","User":"1000:1000","Labels":{"org.opencontainers.image.revision":"625e5b4"},"Env":["SLIPSTREAM_LIBRARY_ROOT=$work_dir/library","SLIPSTREAM_STATE_DIRECTORY=/state","SLIPSTREAM_CACHE_DIRECTORY=/cache","SLIPSTREAM_DATABASE_BASENAME=library.sqlite"]},"HostConfig":{"ReadonlyRootfs":true},"State":{"Health":{"Status":"healthy"}},"Mounts":[{"Type":"bind","Source":"$work_dir/library","Destination":"$work_dir/library","RW":false,"Propagation":"rprivate"},{"Type":"bind","Source":"$work_dir/state","Destination":"/state","RW":true,"Propagation":"rprivate"},{"Type":"bind","Source":"$work_dir/cache","Destination":"/cache","RW":true,"Propagation":"rprivate"}],"NetworkSettings":{"Ports":{"3000/tcp":[{"HostIp":"127.0.0.1","HostPort":"7330"}]}}}]
JSON

cat >"$work_dir/bin/curl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
url=${!#}
body_arg=''
for argument in "$@"; do
  case "$argument" in '{'*'}') body_arg=$argument ;; esac
done
case "$url" in
  */healthz) printf '%s' "${FAKE_HEALTH:-{\"status\":\"ok\"}}" ;;
  */api/overview)
    if [[ -n "${FAKE_MUTATE_STATE:-}" ]]; then
      python3 - "$FAKE_MUTATE_STATE" <<'PY'
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
connection.execute("UPDATE library_metadata SET value=value || 'x' WHERE key='probe'")
connection.commit()
connection.close()
PY
    fi
    python3 - "$FAKE_PHOTOS" "$FAKE_SETS" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source: photos = json.load(source)["photos"]
with open(sys.argv[2], encoding="utf-8") as source: sets = json.load(source)["photoSets"]
print(json.dumps({
    "published": True,
    "photoCount": len(photos),
    "scan": {"state": "idle", "completed": len(photos), "total": len(photos)},
    "photoSets": [
        {
            "id": value["id"],
            "name": value["name"],
            "photoCount": len(value["members"]),
            "hasSavedPosition": "lastReviewedPhotoId" in value,
        }
        for value in sets
    ],
}))
PY
    ;;
  */api/browse)
    kind=$(printf '%s' "$body_arg" | python3 -c 'import json, sys; print(json.load(sys.stdin).get("source", ""))')
    if [[ "$kind" == library ]]; then
      python3 - "$FAKE_PHOTOS" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source: photos = json.load(source)["photos"]
print(json.dumps({"token": "tok-library", "total": len(photos), "position": 0}))
PY
    else
      python3 - "$FAKE_SETS" "${FAKE_SET_POSITION:-}" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source:
    value = json.load(source)["photoSets"][0]
members = value["members"]
photo_ids = [member["photoId"] for member in members]
saved = value.get("lastReviewedPhotoId")
saved_index = photo_ids.index(saved) if saved in photo_ids else None

def available(index):
    return members[index]["available"]

if not members:
    position = 0
elif saved_index is not None and available(saved_index):
    position = saved_index
elif saved_index is not None:
    position = next(
        (
            (saved_index + offset) % len(members)
            for offset in range(1, len(members) + 1)
            if available((saved_index + offset) % len(members))
        ),
        saved_index,
    )
else:
    position = next(
        (index for index, member in enumerate(members) if member["available"]),
        saved_index if saved_index is not None else 0,
    )
override = sys.argv[2]
if override:
    position = int(override)
print(json.dumps({"token": "tok-set", "total": len(members), "position": position}))
PY
    fi
    ;;
  */api/browse/tok-library*)
    start=$(printf '%s' "$url" | sed -n 's/.*[?&]start=\([0-9]*\).*/\1/p')
    python3 - "$FAKE_PHOTOS" "$start" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source: photos = json.load(source)["photos"]
start = int(sys.argv[2])
print(json.dumps({"start": start, "total": len(photos), "photos": photos[start:start + 60]}))
PY
    ;;
  */api/browse/tok-set*)
    start=$(printf '%s' "$url" | sed -n 's/.*[?&]start=\([0-9]*\).*/\1/p')
    python3 - "$FAKE_PHOTOS" "$FAKE_SETS" "$start" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source:
    photos = {value["id"]: value for value in json.load(source)["photos"]}
with open(sys.argv[2], encoding="utf-8") as source:
    members = json.load(source)["photoSets"][0]["members"]
start = int(sys.argv[3])
window = []
for member in members[start:start + 60]:
    photo = dict(photos[member["photoId"]])
    photo["available"] = member["available"]
    photo["selectionState"] = member["selectionState"]
    photo["rating"] = member["rating"]
    window.append(photo)
print(json.dumps({"start": start, "total": len(members), "photos": window}))
PY
    ;;
  */api/photos/one/preview) printf '%s' '{"state":"ready","source":"embedded-raw-jpeg","stale":false,"url":"/api/derivatives/one/key.jpg"}' ;;
  */api/derivatives/one/key.jpg)
    if [[ -n "${FAKE_CACHE_DIRECTORY:-}" ]]; then
      mkdir -p "$FAKE_CACHE_DIRECTORY"
      cp "$FAKE_JPEG" "$FAKE_CACHE_DIRECTORY/key.jpg"
    fi
    cat "$FAKE_JPEG"
    ;;
  */) printf '<title>Slipstream</title>' ;;
  *) exit 22 ;;
esac
SH
cat >"$work_dir/bin/docker" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  inspect) cat "$FAKE_DOCKER_INSPECT" ;;
  exec) exit "${FAKE_DOCKER_EXEC_STATUS:-0}" ;;
  *) exit 2 ;;
esac
SH
chmod +x "$work_dir/bin/curl" "$work_dir/bin/docker"

base_env=(
  PATH="$work_dir/bin:$PATH"
  FAKE_PHOTOS="$work_dir/photos.json"
  FAKE_SETS="$work_dir/sets.json"
  FAKE_DOCKER_INSPECT="$work_dir/container.json"
  FAKE_JPEG="$repo_root/apps/web/test-fixtures/review.jpg"
  SLIPSTREAM_BASE_URL=http://127.0.0.1:7330
  SLIPSTREAM_EXPECTED_PHOTO_COUNT=2
  SLIPSTREAM_EXPECTED_PHOTO_SET=Shoot
  SLIPSTREAM_EXPECTED_MEMBER_COUNT=2
  SLIPSTREAM_EXPECTED_SCHEMA_VERSION=4
  SLIPSTREAM_CONTAINER=slipstream-test
  SLIPSTREAM_LIBRARY_ROOT="$work_dir/library"
  SLIPSTREAM_STATE_DIRECTORY="$work_dir/state"
  SLIPSTREAM_CACHE_DIRECTORY="$work_dir/cache"
  SLIPSTREAM_DATABASE_BASENAME=library.sqlite
  SLIPSTREAM_EXPECTED_BIND_ADDRESS=127.0.0.1
  SLIPSTREAM_EXPECTED_PORT=7330
  SLIPSTREAM_EXPECTED_IMAGE=slipstream:test
  SLIPSTREAM_EXPECTED_IMAGE_ID=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  SLIPSTREAM_EXPECTED_COMMIT=625e5b4
  SLIPSTREAM_EXPECTED_STATE_SNAPSHOT="$work_dir/expected-state.json"
  SLIPSTREAM_ORIGINAL_SAMPLE="$work_dir/library/sample.ARW"
  SLIPSTREAM_EXPECTED_ORIGINAL_SHA256="$original_sha"
  SLIPSTREAM_PREVIEW_PHOTO_ID=one
  SLIPSTREAM_EXPECTED_PREVIEW_SOURCE=embedded-raw-jpeg
)

expect_failure() {
  local name=$1
  shift
  if env "${base_env[@]}" "$@" "$repo_root/scripts/verify-production.sh" >"$work_dir/$name.log" 2>&1; then
    printf 'expected failure did not occur: %s\n' "$name" >&2
    exit 1
  fi
}

env "${base_env[@]}" "$repo_root/scripts/verify-production.sh" >"$work_dir/pass.log"
python3 - "$work_dir/sets.json" "$work_dir/photos.json" "$work_dir/sets-no-progress.json" "$work_dir/state-no-progress.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source: sets = json.load(source)
sets["photoSets"][0].pop("lastReviewedPhotoId")
with open(sys.argv[3], "w", encoding="utf-8") as output: json.dump(sets, output)
with open(sys.argv[2], encoding="utf-8") as source: photos = json.load(source)["photos"]
with open(sys.argv[4], "w", encoding="utf-8") as output: json.dump({"photos": photos, "photoSets": sets["photoSets"]}, output)
PY
mkdir -p "$work_dir/state-no-progress"
cp "$work_dir/state/library.sqlite" "$work_dir/state-no-progress/library.sqlite"
python3 - "$work_dir/state-no-progress/library.sqlite" <<'PY'
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
connection.execute("DELETE FROM review_progress")
connection.commit()
connection.close()
PY
python3 - "$work_dir/container.json" "$work_dir/container-no-progress.json" "$work_dir/state" "$work_dir/state-no-progress" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source: value = json.load(source)
for mount in value[0]["Mounts"]:
    if mount["Source"] == sys.argv[3]: mount["Source"] = sys.argv[4]
with open(sys.argv[2], "w", encoding="utf-8") as output: json.dump(value, output)
PY
env "${base_env[@]}" FAKE_SETS="$work_dir/sets-no-progress.json" \
  SLIPSTREAM_EXPECTED_STATE_SNAPSHOT="$work_dir/state-no-progress.json" \
  SLIPSTREAM_STATE_DIRECTORY="$work_dir/state-no-progress" \
  FAKE_DOCKER_INSPECT="$work_dir/container-no-progress.json" \
  "$repo_root/scripts/verify-production.sh" >"$work_dir/no-progress-pass.log"
python3 - "$work_dir/expected-state.json" "$work_dir/wrong-saved.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source: value = json.load(source)
value["photoSets"][0]["lastReviewedPhotoId"] = "one"
with open(sys.argv[2], "w", encoding="utf-8") as output: json.dump(value, output)
PY
expect_failure wrong-saved-position SLIPSTREAM_EXPECTED_STATE_SNAPSHOT="$work_dir/wrong-saved.json"
expect_failure wrong-browse-position FAKE_SET_POSITION=0
mkdir -p "$work_dir/restore/state" "$work_dir/restore/cache/metadata/manifests" "$work_dir/restore/cache/metadata/failures"
cp "$work_dir/state/library.sqlite" "$work_dir/restore/state/library.sqlite"
python3 - "$work_dir/container.json" "$work_dir/restore-container.json" "$work_dir/state" "$work_dir/cache" "$work_dir/restore/state" "$work_dir/restore/cache" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source: value = json.load(source)
for mount in value[0]["Mounts"]:
    if mount["Source"] == sys.argv[3]: mount["Source"] = sys.argv[5]
    if mount["Source"] == sys.argv[4]: mount["Source"] = sys.argv[6]
with open(sys.argv[2], "w", encoding="utf-8") as output: json.dump(value, output)
PY
env "${base_env[@]}" \
  FAKE_DOCKER_INSPECT="$work_dir/restore-container.json" \
  FAKE_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_STATE_DIRECTORY="$work_dir/restore/state" \
  SLIPSTREAM_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_ISOLATED_RESTORE_ROOT="$work_dir/restore" \
  SLIPSTREAM_PRODUCTION_STATE_DIRECTORY="$work_dir/state" \
  SLIPSTREAM_PRODUCTION_CACHE_DIRECTORY="$work_dir/cache" \
  SLIPSTREAM_ALLOW_DERIVED_WRITES=1 \
  "$repo_root/scripts/verify-production.sh" >"$work_dir/derived-pass.log"
rm -rf "$work_dir/restore/cache"/*
mkdir -p "$work_dir/restore/cache/metadata"
printf stale >"$work_dir/restore/cache/metadata/stale.json"
expect_failure derived-preexisting-cache-file \
  FAKE_DOCKER_INSPECT="$work_dir/restore-container.json" \
  FAKE_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_STATE_DIRECTORY="$work_dir/restore/state" \
  SLIPSTREAM_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_ISOLATED_RESTORE_ROOT="$work_dir/restore" \
  SLIPSTREAM_PRODUCTION_STATE_DIRECTORY="$work_dir/state" \
  SLIPSTREAM_PRODUCTION_CACHE_DIRECTORY="$work_dir/cache" \
  SLIPSTREAM_ALLOW_DERIVED_WRITES=1
rm -rf "$work_dir/restore/cache"/*
expect_failure derived-overlap-equal \
  FAKE_DOCKER_INSPECT="$work_dir/restore-container.json" \
  FAKE_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_STATE_DIRECTORY="$work_dir/restore/state" \
  SLIPSTREAM_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_ISOLATED_RESTORE_ROOT="$work_dir/restore" \
  SLIPSTREAM_PRODUCTION_STATE_DIRECTORY="$work_dir/restore" \
  SLIPSTREAM_PRODUCTION_CACHE_DIRECTORY="$work_dir/cache" \
  SLIPSTREAM_ALLOW_DERIVED_WRITES=1
mkdir "$work_dir/restore/production-state"
expect_failure derived-overlap-production-below \
  FAKE_DOCKER_INSPECT="$work_dir/restore-container.json" \
  FAKE_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_STATE_DIRECTORY="$work_dir/restore/state" \
  SLIPSTREAM_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_ISOLATED_RESTORE_ROOT="$work_dir/restore" \
  SLIPSTREAM_PRODUCTION_STATE_DIRECTORY="$work_dir/restore/production-state" \
  SLIPSTREAM_PRODUCTION_CACHE_DIRECTORY="$work_dir/cache" \
  SLIPSTREAM_ALLOW_DERIVED_WRITES=1
expect_failure derived-overlap-production-ancestor \
  FAKE_DOCKER_INSPECT="$work_dir/restore-container.json" \
  FAKE_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_STATE_DIRECTORY="$work_dir/restore/state" \
  SLIPSTREAM_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_ISOLATED_RESTORE_ROOT="$work_dir/restore" \
  SLIPSTREAM_PRODUCTION_STATE_DIRECTORY="$work_dir/state" \
  SLIPSTREAM_PRODUCTION_CACHE_DIRECTORY="$work_dir" \
  SLIPSTREAM_ALLOW_DERIVED_WRITES=1
mkdir "$work_dir/restore/state/production-cache"
expect_failure derived-overlap-production-inside-state \
  FAKE_DOCKER_INSPECT="$work_dir/restore-container.json" \
  FAKE_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_STATE_DIRECTORY="$work_dir/restore/state" \
  SLIPSTREAM_CACHE_DIRECTORY="$work_dir/restore/cache" \
  SLIPSTREAM_ISOLATED_RESTORE_ROOT="$work_dir/restore" \
  SLIPSTREAM_PRODUCTION_STATE_DIRECTORY="$work_dir/state" \
  SLIPSTREAM_PRODUCTION_CACHE_DIRECTORY="$work_dir/restore/state/production-cache" \
  SLIPSTREAM_ALLOW_DERIVED_WRITES=1
rm -rf "$work_dir/restore/production-state" "$work_dir/restore/state/production-cache"
expect_failure wrong-count SLIPSTREAM_EXPECTED_PHOTO_COUNT=3
python3 - "$work_dir/expected-state.json" "$work_dir/wrong-state.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as source:
    value = json.load(source)
value["photos"][0]["selectionState"] = "undecided"
with open(sys.argv[2], "w", encoding="utf-8") as output:
    json.dump(value, output)
PY
expect_failure wrong-state SLIPSTREAM_EXPECTED_STATE_SNAPSHOT="$work_dir/wrong-state.json"
expect_failure wrong-health FAKE_HEALTH='{"status":"starting"}'
expect_failure missing-original SLIPSTREAM_ORIGINAL_SAMPLE=
expect_failure wrong-original SLIPSTREAM_EXPECTED_ORIGINAL_SHA256=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
printf outside >"$work_dir/outside.ARW"
expect_failure escaped-original SLIPSTREAM_ORIGINAL_SAMPLE="$work_dir/library/../outside.ARW"
ln -s "$work_dir/library/sample.ARW" "$work_dir/library/sample-link.ARW"
expect_failure symlink-original SLIPSTREAM_ORIGINAL_SAMPLE="$work_dir/library/sample-link.ARW"
rm "$work_dir/library/sample-link.ARW"
expect_failure wrong-runtime FAKE_DOCKER_EXEC_STATUS=1
expect_failure wrong-port SLIPSTREAM_EXPECTED_PORT=7331
expect_failure wrong-image SLIPSTREAM_EXPECTED_IMAGE_ID=sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
expect_failure wrong-revision SLIPSTREAM_EXPECTED_COMMIT=deadbee
python3 - "$work_dir/container.json" "$work_dir/container-wrong-env.json" "$work_dir/container-nested-mount.json" "$work_dir/container-extra-port.json" "$work_dir/container-alias-mount.json" "$work_dir/container-wildcard.json" "$work_dir/container-volume.json" <<'PY'
import copy, json, sys
with open(sys.argv[1], encoding="utf-8") as source:
    original = json.load(source)
wrong_env = copy.deepcopy(original)
wrong_env[0]["Config"]["Env"] = [value for value in wrong_env[0]["Config"]["Env"] if not value.startswith("SLIPSTREAM_DATABASE_BASENAME=")]
wrong_env[0]["Config"]["Env"].append("SLIPSTREAM_DATABASE_BASENAME=other.sqlite")
nested = copy.deepcopy(original)
library = next(value.split("=", 1)[1] for value in nested[0]["Config"]["Env"] if value.startswith("SLIPSTREAM_LIBRARY_ROOT="))
nested[0]["Mounts"].append({"Type": "bind", "Source": "/tmp/shadow", "Destination": library + "/nested", "RW": True, "Propagation": "rprivate"})
extra_port = copy.deepcopy(original)
extra_port[0]["NetworkSettings"]["Ports"]["9000/tcp"] = [{"HostIp": "127.0.0.1", "HostPort": "9000"}]
alias = copy.deepcopy(original)
alias[0]["Mounts"].append({"Type": "bind", "Source": library, "Destination": "/writable-original-alias", "RW": True, "Propagation": "rprivate"})
wildcard = copy.deepcopy(original)
wildcard[0]["NetworkSettings"]["Ports"]["3000/tcp"][0]["HostIp"] = "0.0.0.0"
volume = copy.deepcopy(original)
volume[0]["Mounts"][0]["Type"] = "volume"
volume[0]["Mounts"][0]["Propagation"] = ""
for path, value in zip(sys.argv[2:], [wrong_env, nested, extra_port, alias, wildcard, volume]):
    with open(path, "w", encoding="utf-8") as output:
        json.dump(value, output)
PY
expect_failure wrong-container-env FAKE_DOCKER_INSPECT="$work_dir/container-wrong-env.json"
expect_failure nested-original-mount FAKE_DOCKER_INSPECT="$work_dir/container-nested-mount.json"
expect_failure extra-port FAKE_DOCKER_INSPECT="$work_dir/container-extra-port.json"
expect_failure writable-original-alias FAKE_DOCKER_INSPECT="$work_dir/container-alias-mount.json"
expect_failure wildcard-bind FAKE_DOCKER_INSPECT="$work_dir/container-wildcard.json" SLIPSTREAM_EXPECTED_BIND_ADDRESS=0.0.0.0 SLIPSTREAM_BASE_URL=http://0.0.0.0:7330
expect_failure non-bind-mount FAKE_DOCKER_INSPECT="$work_dir/container-volume.json"
cp "$work_dir/state/library.sqlite" "$work_dir/state/decoy.sqlite"
expect_failure decoy-database SLIPSTREAM_DATABASE_BASENAME=decoy.sqlite
rm "$work_dir/state/decoy.sqlite"
for suffix in -journal -wal -shm; do
  touch "$work_dir/state/library.sqlite$suffix"
  expect_failure "sidecar$suffix"
  rm "$work_dir/state/library.sqlite$suffix"
done
expect_failure state-mutation FAKE_MUTATE_STATE="$work_dir/state/library.sqlite"
python3 - "$work_dir/state/library.sqlite" <<'PY'
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
connection.execute("UPDATE library_metadata SET value='persisted' WHERE key='probe'")
connection.commit()
connection.close()
PY

backup="$work_dir/backups/state.tgz"
export SLIPSTREAM_LIBRARY_ROOT="$work_dir/library"
export SLIPSTREAM_EXPECTED_SCHEMA_VERSION=4
if env SLIPSTREAM_STATE_DIRECTORY="$work_dir/state" SLIPSTREAM_BACKUP_OUTPUT="$work_dir/state/new-output/backup.tgz" \
  "$repo_root/scripts/backup-state.sh" >"$work_dir/backup-inside-state.log" 2>&1; then
  printf 'backup output inside state was accepted\n' >&2
  exit 1
fi
[[ ! -e "$work_dir/state/new-output" ]]
if env SLIPSTREAM_STATE_DIRECTORY="$work_dir/state" SLIPSTREAM_BACKUP_OUTPUT="$work_dir/library/backup.tgz" \
  "$repo_root/scripts/backup-state.sh" >"$work_dir/backup-inside-library.log" 2>&1; then
  printf 'backup output inside Library was accepted\n' >&2
  exit 1
fi
[[ ! -e "$work_dir/library/backup.tgz" ]]
env SLIPSTREAM_STATE_DIRECTORY="$work_dir/state" SLIPSTREAM_BACKUP_OUTPUT="$backup" \
  "$repo_root/scripts/backup-state.sh" >"$work_dir/backup-pass.log"
[[ -s "$backup" ]]
if env SLIPSTREAM_STATE_DIRECTORY="$work_dir/state" SLIPSTREAM_BACKUP_OUTPUT="$backup" \
  "$repo_root/scripts/backup-state.sh" >"$work_dir/backup-existing.log" 2>&1; then
  printf 'existing backup path was accepted\n' >&2
  exit 1
fi
touch "$work_dir/state/library.sqlite-shm"
if env SLIPSTREAM_STATE_DIRECTORY="$work_dir/state" SLIPSTREAM_BACKUP_OUTPUT="$work_dir/backups/sidecar.tgz" \
  "$repo_root/scripts/backup-state.sh" >"$work_dir/backup-sidecar.log" 2>&1; then
  printf 'sidecar backup was accepted\n' >&2
  exit 1
fi
rm "$work_dir/state/library.sqlite-shm"

bash -c 'exec 9<"$1"; sleep 30' _ "$work_dir/state/library.sqlite" &
holder_pid=$!
for _ in $(seq 1 50); do
  fuser "$work_dir/state/library.sqlite" >/dev/null 2>&1 && break
  sleep 0.1
done
env SLIPSTREAM_STATE_DIRECTORY="$work_dir/state" SLIPSTREAM_BACKUP_OUTPUT="$work_dir/backups/held.tgz" \
  "$repo_root/scripts/backup-state.sh" >"$work_dir/backup-held.log"
[[ -s "$work_dir/backups/held.tgz" ]]
kill "$holder_pid" >/dev/null 2>&1 || true
wait "$holder_pid" 2>/dev/null || true
holder_pid=''
ln -s "$work_dir/state/library.sqlite" "$work_dir/state/alias.sqlite"
if env SLIPSTREAM_STATE_DIRECTORY="$work_dir/state" SLIPSTREAM_BACKUP_OUTPUT="$work_dir/backups/symlink.tgz" \
  "$repo_root/scripts/backup-state.sh" >"$work_dir/backup-symlink.log" 2>&1; then
  printf 'state-tree symlink was accepted\n' >&2
  exit 1
fi
rm "$work_dir/state/alias.sqlite"
touch "$work_dir/state/other.sqlite-wal"
if env SLIPSTREAM_STATE_DIRECTORY="$work_dir/state" SLIPSTREAM_BACKUP_OUTPUT="$work_dir/backups/extra.tgz" \
  "$repo_root/scripts/backup-state.sh" >"$work_dir/backup-extra.log" 2>&1; then
  printf 'extra state entry was accepted\n' >&2
  exit 1
fi
rm "$work_dir/state/other.sqlite-wal"
for case in bad-schema bad-root; do
  mkdir "$work_dir/$case-state"
  python3 - "$work_dir/$case-state/library.sqlite" "$work_dir/library" "$case" <<'PY'
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
connection.execute(f"PRAGMA user_version = {999 if sys.argv[3] == 'bad-schema' else 4}")
connection.execute("CREATE TABLE library_metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL)")
root = "/wrong/root" if sys.argv[3] == "bad-root" else sys.argv[2]
connection.execute("INSERT INTO library_metadata VALUES ('canonical_root', ?)", (root,))
connection.commit(); connection.close()
PY
  if env SLIPSTREAM_STATE_DIRECTORY="$work_dir/$case-state" SLIPSTREAM_BACKUP_OUTPUT="$work_dir/backups/$case.tgz" \
    "$repo_root/scripts/backup-state.sh" >"$work_dir/backup-$case.log" 2>&1; then
    printf 'invalid backup source was accepted: %s\n' "$case" >&2
    exit 1
  fi
done

printf 'production acceptance controller tests passed\n'
