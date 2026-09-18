#!/usr/bin/env bash

set -euo pipefail

fail() {
  printf 'prepare-worktree: %s\n' "$*" >&2
  exit 1
}

worktree="${1:-$PWD}"
[[ -d "$worktree" ]] || fail "worktree does not exist: $worktree"
worktree=$(realpath -- "$worktree")
cd "$worktree"

root=$(realpath -- "$(git rev-parse --show-toplevel)")
[[ "$root" == "$worktree" ]] || fail "path is not the worktree root: $root"

branch=$(git branch --show-current)
head=$(git rev-parse HEAD)
base=unavailable
if git show-ref --verify --quiet refs/remotes/origin/main; then
  base=$(git merge-base "$head" refs/remotes/origin/main)
fi

read -r package_manager bun_engine <<<"$(bun -e '
  const packageJson = await Bun.file("package.json").json();
  console.log(packageJson.packageManager, packageJson.engines?.bun ?? "");
')"
[[ "$package_manager" == bun@* ]] \
  || fail "package.json must declare Bun as its package manager"
expected_bun=${package_manager#bun@}
[[ -n "$expected_bun" ]] || fail "package.json must declare a Bun version"
[[ "$bun_engine" == "$expected_bun" ]] \
  || fail "package.json packageManager and engines.bun must agree"
[[ "$(bun --version)" == "$expected_bun" ]] \
  || fail "Bun $expected_bun is required"

expected_rust=$(bun -e '
  const toolchain = Bun.TOML.parse(
    await Bun.file("rust-toolchain.toml").text(),
  );
  console.log(toolchain.toolchain?.channel ?? "");
')
[[ -n "$expected_rust" ]] || fail "rust-toolchain.toml must declare a channel"
[[ "$(rustc --version | awk '{print $2}')" == "$expected_rust" ]] \
  || fail "Rust $expected_rust is required"
rustup show active-toolchain | grep -q "^${expected_rust}-" \
  || fail "rust-toolchain.toml did not select Rust $expected_rust"
cargo fmt --version >/dev/null
cargo clippy --version >/dev/null

[[ -f bun.lock ]] || fail "bun.lock is missing"
[[ -f Cargo.lock ]] || fail "Cargo.lock is missing"
bun_lock_before=$(git hash-object -- bun.lock)
cargo_lock_before=$(git hash-object -- Cargo.lock)

bun install --frozen-lockfile
cargo fetch --locked

git diff --check
[[ "$(git hash-object -- bun.lock)" == "$bun_lock_before" ]] \
  || fail "bun install changed bun.lock"
[[ "$(git hash-object -- Cargo.lock)" == "$cargo_lock_before" ]] \
  || fail "cargo fetch changed Cargo.lock"

printf 'worktree ready: %s\n' "$root"
printf 'branch: %s\n' "${branch:-detached}"
printf 'base: %s\n' "$base"
printf 'head: %s\n' "$head"
printf 'Bun %s, Rust %s, locked dependencies restored\n' "$expected_bun" "$expected_rust"
