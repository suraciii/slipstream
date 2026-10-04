import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  allowedLineCount,
  collectFileSizeViolations,
  countLines,
  evaluateFileSize,
  formatFileSizeRatchetFailure,
  parseChangedFiles,
  resolveBaseRef,
} from "./check-file-sizes-core.mjs";

function git(repo, ...args) {
  const env = Object.fromEntries(
    Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_")),
  );
  return execFileSync("git", ["-c", "core.hooksPath=/dev/null", ...args], {
    cwd: repo,
    encoding: "utf8",
    env,
  }).trim();
}

function makeRepository() {
  const repo = mkdtempSync(path.join(tmpdir(), "slipstream-file-size-"));
  git(repo, "init", "-b", "main");
  git(repo, "config", "user.name", "Test");
  git(repo, "config", "user.email", "test@example.com");
  mkdirSync(path.join(repo, "crates"), { recursive: true });
  writeFileSync(
    path.join(repo, "crates", "existing.rs"),
    "one\ntwo\nthree\nfour",
  );
  git(repo, "add", ".");
  git(repo, "commit", "-m", "base");
  git(repo, "remote", "add", "origin", repo);
  git(repo, "fetch", "origin", "main:refs/remotes/origin/main");
  git(repo, "switch", "-c", "feature");
  return repo;
}

test("counts empty, LF, and CRLF content", () => {
  assert.equal(countLines(""), 0);
  assert.equal(countLines("one\n"), 2);
  assert.equal(countLines("one\r\ntwo"), 2);
});

test("new and compliant files use the configured ceiling", () => {
  assert.equal(allowedLineCount(null, 3), 3);
  assert.deepEqual(
    evaluateFileSize({ baseLines: null, candidateLines: 3, maxLines: 3 }),
    { limit: 3, violates: false },
  );
  assert.equal(
    evaluateFileSize({ baseLines: 4, candidateLines: 3, maxLines: 3 }).violates,
    false,
  );
});

test("inherited oversized files may hold or shrink but not grow", () => {
  assert.equal(allowedLineCount(4, 3), 4);
  assert.equal(
    evaluateFileSize({ baseLines: 4, candidateLines: 4, maxLines: 3 }).violates,
    false,
  );
  assert.equal(
    evaluateFileSize({ baseLines: 4, candidateLines: 5, maxLines: 3 }).violates,
    true,
  );
});

test("parses modifications, deletions, and renames from Git NUL output", () => {
  assert.deepEqual(
    parseChangedFiles(
      "M\0crates/a.rs\0D\0crates/b.rs\0R100\0crates/old.rs\0crates/new.rs\0",
    ),
    [
      { status: "M", path: "crates/a.rs" },
      { status: "D", path: "crates/b.rs" },
      {
        status: "R",
        oldPath: "crates/old.rs",
        path: "crates/new.rs",
      },
    ],
  );
});

test("resolves the merge-base and rejects a missing origin/main", () => {
  const repo = makeRepository();
  const base = git(repo, "rev-parse", "HEAD");
  git(repo, "commit", "--allow-empty", "-m", "branch change");
  assert.equal(resolveBaseRef(repo, {}), base);
  git(repo, "update-ref", "-d", "refs/remotes/origin/main");
  assert.throws(
    () => resolveBaseRef(repo, {}),
    /Fetch origin\/main or set CHECK_FILE_SIZES_BASE/,
  );
});

test("checks tracked growth and untracked file ceilings", async () => {
  const repo = makeRepository();
  const baseRef = git(repo, "rev-parse", "HEAD");
  writeFileSync(
    path.join(repo, "crates", "existing.rs"),
    "one\ntwo\nthree\nfour\nfive",
  );
  writeFileSync(path.join(repo, "crates", "new.rs"), "one\ntwo\nthree\nfour");

  const report = await collectFileSizeViolations({
    repoRoot: repo,
    baseRef,
    rules: [{ root: "crates", extensions: new Set([".rs"]), maxLines: 3 }],
  });

  assert.deepEqual(
    report.violations.map((violation) => violation.relativePath).sort(),
    ["crates/existing.rs", "crates/new.rs"],
  );
  assert.deepEqual(
    report.violations.find(
      (violation) => violation.relativePath === "crates/existing.rs",
    ),
    {
      relativePath: "crates/existing.rs",
      baseLines: 4,
      candidateLines: 5,
      limit: 4,
    },
  );
});

function makeMergedRepository() {
  const repo = makeRepository();
  writeFileSync(path.join(repo, "crates", "merged.rs"), "one\ntwo");
  git(repo, "add", ".");
  git(repo, "commit", "-m", "branch change");
  git(repo, "switch", "main");
  git(repo, "merge", "--no-ff", "feature", "-m", "Merge pull request #1");
  return repo;
}

test("grades GitHub Actions runs against the merge base commit HEAD^1", async () => {
  const repo = makeMergedRepository();
  assert.equal(resolveBaseRef(repo, { GITHUB_ACTIONS: "true" }), "HEAD^1");

  const previous = process.env.GITHUB_ACTIONS;
  process.env.GITHUB_ACTIONS = "true";
  try {
    const report = await collectFileSizeViolations({
      repoRoot: repo,
      rules: [{ root: "crates", extensions: new Set([".rs"]), maxLines: 1 }],
    });
    assert.equal(report.baseRef, "HEAD^1");
    assert.deepEqual(report.violations, [
      {
        relativePath: "crates/merged.rs",
        baseLines: null,
        candidateLines: 2,
        limit: 1,
      },
    ]);
  } finally {
    if (previous === undefined) delete process.env.GITHUB_ACTIONS;
    else process.env.GITHUB_ACTIONS = previous;
  }
});

test("names the checkout depth when the base commit is missing", async () => {
  const repo = mkdtempSync(path.join(tmpdir(), "slipstream-file-size-"));
  git(repo, "init", "-b", "main");
  git(repo, "config", "user.name", "Test");
  git(repo, "config", "user.email", "test@example.com");
  writeFileSync(path.join(repo, "README.md"), "root");
  git(repo, "add", ".");
  git(repo, "commit", "-m", "root");

  await assert.rejects(
    collectFileSizeViolations({
      repoRoot: repo,
      baseRef: "HEAD^1",
      rules: [{ root: "crates", extensions: new Set([".rs"]), maxLines: 3 }],
    }),
    /fetch-depth: 2.*CHECK_FILE_SIZES_BASE/,
  );
});

test("explains structural fixes when the ratchet fails", () => {
  const message = formatFileSizeRatchetFailure({
    label: "Test",
    baseRef: "abc123",
    violations: [
      {
        relativePath: "crates/existing.rs",
        baseLines: 4,
        candidateLines: 5,
        limit: 4,
      },
    ],
  }).join("\n");

  assert.match(
    message,
    /Do not remove meaningful spacing or flatten readable code/,
  );
  assert.match(message, /extract genuinely shared helpers or fixtures/);
});
