#!/usr/bin/env bash
set -euo pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

fail() {
  printf 'container verification failed: %s\n' "$1" >&2
  exit 1
}

required_files=(Dockerfile .dockerignore compose.yaml deploy/README.md)
for file in "${required_files[@]}"; do
  [[ -f "$file" ]] || fail "missing $file"
done

# Keep the build and runtime boundaries obvious and pinned. Runtime must not
# accidentally inherit the Bun/Node build toolchain or the rollback addon.
grep -Fqx 'FROM oven/bun:1.4.0 AS web-build' Dockerfile || fail 'Web build is not pinned to Bun 1.4.0'
grep -Fqx 'FROM rust:1.97.1-bookworm AS rust-toolchain' Dockerfile || fail 'Rust toolchain is not pinned to Rust 1.97.1'
grep -Fqx 'FROM ubuntu:26.04 AS rust-build' Dockerfile || fail 'Rust build base is not pinned to Ubuntu 26.04'
grep -Fqx 'FROM ubuntu:26.04 AS runtime' Dockerfile || fail 'runtime base is not Ubuntu 26.04'

runtime_dockerfile=$(awk 'seen { print } /^FROM ubuntu:26.04 AS runtime$/ { seen = 1 }' Dockerfile)
if grep -Eiq 'node|bun|sharp|node-api|node_addon' <<<"$runtime_dockerfile"; then
  fail 'runtime Dockerfile contains a Node/Bun/Sharp/Node-API dependency'
fi
for line in \
  'USER 1000:1000' \
  'STOPSIGNAL SIGTERM' \
  'ENTRYPOINT ["/usr/local/bin/slipstream-server"]'; do
  grep -Fqx "$line" Dockerfile || fail "Dockerfile is missing: $line"
done

[[ -x scripts/verify-container.sh ]] || fail 'verification script must be executable'

if ! command -v docker >/dev/null 2>&1; then
  printf 'static container checks passed (docker is not installed; compose checks skipped)\n'
  exit 0
fi

work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' EXIT
export SLIPSTREAM_IMAGE="${SLIPSTREAM_IMAGE:-slipstream:issue-24-verification}"
export SLIPSTREAM_LIBRARY_ROOT="${SLIPSTREAM_LIBRARY_ROOT:-$work_dir/originals}"
export SLIPSTREAM_STATE_DIRECTORY="${SLIPSTREAM_STATE_DIRECTORY:-$work_dir/state}"
export SLIPSTREAM_CACHE_DIRECTORY="${SLIPSTREAM_CACHE_DIRECTORY:-$work_dir/cache}"
mkdir -p "$SLIPSTREAM_LIBRARY_ROOT" "$SLIPSTREAM_STATE_DIRECTORY" "$SLIPSTREAM_CACHE_DIRECTORY"
export SLIPSTREAM_BIND_ADDRESS="${SLIPSTREAM_BIND_ADDRESS:-127.0.0.1}"
export SLIPSTREAM_PORT="${SLIPSTREAM_PORT:-3000}"

canonical_library=$(realpath -- "$SLIPSTREAM_LIBRARY_ROOT") || fail 'Originals directory cannot be canonicalized'
[[ "$canonical_library" == "$SLIPSTREAM_LIBRARY_ROOT" ]] || fail 'SLIPSTREAM_LIBRARY_ROOT must be its canonical absolute path, not a symlink or alias'

rendered=$(docker compose -f compose.yaml config) || fail 'docker compose config rejected compose.yaml'
for pattern in \
  'user: 1000:1000' \
  'read_only: true' \
  'cap_drop:' \
  '  - ALL' \
  'no-new-privileges:true' \
  'init: true' \
  'restart: unless-stopped' \
  'stop_signal: SIGTERM' \
  'stop_grace_period: 30s'; do
  grep -Fq "$pattern" <<<"$rendered" || fail "rendered Compose configuration is missing: $pattern"
done
grep -Fq 'host_ip: 127.0.0.1' <<<"$rendered" || fail 'default host bind is not loopback'
grep -Fq "target: $SLIPSTREAM_LIBRARY_ROOT" <<<"$rendered" || fail 'Originals bind mount does not preserve the canonical host path'
grep -Fq 'target: /state' <<<"$rendered" || fail 'state bind mount is missing'
grep -Fq 'target: /cache' <<<"$rendered" || fail 'cache bind mount is missing'
grep -Fq 'read_only: true' <<<"$rendered" || fail 'Originals bind mount is not read-only'
grep -Fq 'http://127.0.0.1:3000/healthz' <<<"$rendered" || fail 'healthcheck does not use /healthz'

# The host publication is intentionally configurable for a Tailscale-only
# operator setup while retaining loopback as the safe default.
tailscale_rendered=$(SLIPSTREAM_BIND_ADDRESS=100.64.0.12 docker compose -f compose.yaml config) \
  || fail 'docker compose rejected an explicit Tailscale bind address'
grep -Fq 'host_ip: 100.64.0.12' <<<"$tailscale_rendered" || \
  fail 'explicit Tailscale bind address was not published'

if [[ "${VERIFY_IMAGE:-0}" == "1" ]]; then
  docker image inspect "$SLIPSTREAM_IMAGE" >/dev/null 2>&1 || fail "image not found: $SLIPSTREAM_IMAGE"
  image_user=$(docker image inspect --format '{{.Config.User}}' "$SLIPSTREAM_IMAGE")
  [[ "$image_user" == '1000:1000' ]] || fail "image user is $image_user, expected 1000:1000"
  docker run --rm --entrypoint /bin/sh "$SLIPSTREAM_IMAGE" -c '
    ! command -v node >/dev/null 2>&1 &&
    ! command -v bun >/dev/null 2>&1 &&
    ! command -v npm >/dev/null 2>&1 &&
    ! find /app /usr/local \( -iname "*sharp*" -o -iname "*node-api*" -o -iname "*.node" \) -print -quit | grep -q .
  ' || fail 'runtime image contains Node/Bun/npm/Sharp/Node-API'
  printf 'container image checks passed: %s\n' "$SLIPSTREAM_IMAGE"
else
  printf 'static and Compose checks passed (set VERIFY_IMAGE=1 to inspect a built image)\n'
fi
