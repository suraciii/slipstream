import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { expect, test } from "bun:test";

const script = await Bun.file(
  new URL("../scripts/prepare-worktree.sh", import.meta.url),
).text();
const scriptPath = new URL("../scripts/prepare-worktree.sh", import.meta.url)
  .pathname;

async function run(
  command: string,
  args: readonly string[],
  cwd: string,
  environment: Record<string, string | undefined> = {},
): Promise<{ output: string; exitCode: number }> {
  const child = Bun.spawn([command, ...args], {
    cwd,
    env: { ...process.env, ...environment },
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { output: `${stdout}${stderr}`, exitCode };
}

async function createFixture(): Promise<{
  root: string;
  bin: string;
  log: string;
  cleanup: () => Promise<void>;
}> {
  const root = await mkdtemp(join(tmpdir(), "slipstream-worktree-preflight-"));
  const bin = join(root, "bin");
  const log = join(root, "commands.log");
  await Bun.write(
    join(root, "package.json"),
    JSON.stringify({
      name: "preflight-fixture",
      packageManager: "bun@1.4.0",
      engines: { bun: "1.4.0" },
    }),
  );
  await Bun.write(
    join(root, "rust-toolchain.toml"),
    '[toolchain]\nchannel = "1.97.1"\n',
  );
  await Bun.write(join(root, "bun.lock"), "bun-lock-fixture\n");
  await Bun.write(join(root, "Cargo.lock"), "cargo-lock-fixture\n");
  await Bun.write(join(root, "README.md"), "fixture\n");
  await Bun.write(
    join(root, "stub-command"),
    `#!/usr/bin/env bash
set -euo pipefail
name="\${STUB_NAME:-$(basename "$0")}"\nprintf '%s %s\\n' "$name" "$*" >> "$PREFLIGHT_LOG"
case "$name:$1" in
  bun:--version)
    printf '%s\\n' "\${BUN_VERSION:-1.4.0}"
    ;;
  bun:-e)
    if [[ "$2" == *"packageManager"* ]]; then
      printf '%s %s\\n' "\${PACKAGE_MANAGER:-bun@1.4.0}" "\${BUN_ENGINE:-1.4.0}"
    else
      printf '%s\\n' "\${RUST_VERSION:-1.97.1}"
    fi
    ;;
  bun:install)
    [[ "\${PREFLIGHT_FAIL:-}" != install ]] || exit 23
    ;;
  rustc:*)
    printf 'rustc %s (fixture)\\n' "\${RUST_VERSION:-1.97.1}"
    ;;
  rustup:*)
    printf '%s-x86_64-unknown-linux-gnu (fixture)\\n' "\${RUST_VERSION:-1.97.1}"
    ;;
  cargo:fmt|cargo:clippy)
    printf '%s fixture\\n' "$1"
    ;;
  cargo:fetch)
    [[ "\${PREFLIGHT_FAIL:-}" != fetch ]] || exit 29
    ;;
  *)
    printf 'unexpected stub command: %s %s\\n' "$name" "$*" >&2
    exit 31
    ;;
esac
`,
  );
  await chmod(join(root, "stub-command"), 0o755);
  await Bun.write(join(root, "gitignore"), "");
  await run("git", ["init", "--quiet"], root);
  await run("git", ["config", "user.email", "preflight@example.test"], root);
  await run("git", ["config", "user.name", "Preflight Test"], root);
  await run("git", ["add", "."], root);
  await run("git", ["commit", "--quiet", "-m", "fixture"], root);

  await Bun.write(
    join(bin, "bun"),
    `#!/usr/bin/env bash
STUB_NAME=bun exec "${join(root, "stub-command")}" "$@"
`,
  );
  await Bun.write(
    join(bin, "rustc"),
    `#!/usr/bin/env bash
STUB_NAME=rustc exec "${join(root, "stub-command")}" "$@"
`,
  );
  await Bun.write(
    join(bin, "rustup"),
    `#!/usr/bin/env bash
STUB_NAME=rustup exec "${join(root, "stub-command")}" "$@"
`,
  );
  await Bun.write(
    join(bin, "cargo"),
    `#!/usr/bin/env bash
STUB_NAME=cargo exec "${join(root, "stub-command")}" "$@"
`,
  );
  for (const command of ["bun", "rustc", "rustup", "cargo"])
    await chmod(join(bin, command), 0o755);

  return {
    root,
    bin,
    log,
    cleanup: () => rm(root, { recursive: true, force: true }),
  };
}

function environment(fixture: { bin: string; log: string }) {
  return {
    PATH: `${fixture.bin}:${process.env.PATH ?? ""}`,
    PREFLIGHT_LOG: fixture.log,
  };
}

test("worktree preflight is fail-fast and non-destructive", () => {
  expect(script).not.toContain("git reset --hard");
  expect(script).not.toContain("git clean -fdx");
  expect(script).not.toContain("git checkout --");
});

test("preflight restores locked dependencies and can be repeated over edits", async () => {
  const fixture = await createFixture();
  try {
    const first = await run(
      scriptPath,
      [fixture.root],
      fixture.root,
      environment(fixture),
    );
    expect(first.exitCode).toBe(0);
    expect(first.output).toContain(`worktree ready: ${fixture.root}`);
    expect(first.output).toContain("branch: ");
    expect(first.output).toContain("base: unavailable");
    expect(first.output).toContain("head: ");

    await writeFile(join(fixture.root, "README.md"), "edited\n");
    await writeFile(join(fixture.root, "staged.txt"), "staged\n");
    await run("git", ["add", "staged.txt"], fixture.root);
    await writeFile(join(fixture.root, "notes.txt"), "untracked\n");

    const second = await run(
      scriptPath,
      [fixture.root],
      fixture.root,
      environment(fixture),
    );
    expect(second.exitCode).toBe(0);
    expect(await Bun.file(join(fixture.root, "README.md")).text()).toBe(
      "edited\n",
    );
    expect(await Bun.file(join(fixture.root, "staged.txt")).text()).toBe(
      "staged\n",
    );
    expect(await Bun.file(join(fixture.root, "notes.txt")).text()).toBe(
      "untracked\n",
    );

    const log = await readFile(fixture.log, "utf8");
    expect(log.match(/^bun install --frozen-lockfile$/gm)?.length).toBe(2);
    expect(log.match(/^cargo fetch --locked$/gm)?.length).toBe(2);
  } finally {
    await fixture.cleanup();
  }
});

test("preflight rejects a toolchain mismatch before installing dependencies", async () => {
  const fixture = await createFixture();
  try {
    const result = await run(scriptPath, [fixture.root], fixture.root, {
      ...environment(fixture),
      BUN_VERSION: "9.9.9",
    });
    expect(result.exitCode).not.toBe(0);
    expect(result.output).toContain("Bun 1.4.0 is required");
    expect(await Bun.file(fixture.log).text()).not.toContain("install");
  } finally {
    await fixture.cleanup();
  }
});

test("preflight stops before Cargo when Bun installation fails", async () => {
  const fixture = await createFixture();
  try {
    const result = await run(scriptPath, [fixture.root], fixture.root, {
      ...environment(fixture),
      PREFLIGHT_FAIL: "install",
    });
    expect(result.exitCode).toBe(23);
    const log = await Bun.file(fixture.log).text();
    expect(log).toContain("bun install --frozen-lockfile");
    expect(log).not.toContain("cargo fetch --locked");
  } finally {
    await fixture.cleanup();
  }
});
