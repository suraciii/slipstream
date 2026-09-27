import { promises as fs } from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";

function git(args, cwd, options = {}) {
  const env = Object.fromEntries(
    Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_")),
  );
  return execFileSync("git", ["-c", "core.hooksPath=/dev/null", ...args], {
    cwd,
    env,
    ...options,
  });
}

function toPosixPath(relativePath) {
  return relativePath.split(path.sep).join("/");
}

export function countLines(content) {
  if (content.length === 0) return 0;
  return content.split(/\r?\n/).length;
}

export function allowedLineCount(baseLines, maxLines) {
  return baseLines == null || baseLines <= maxLines ? maxLines : baseLines;
}

export function evaluateFileSize({ baseLines, candidateLines, maxLines }) {
  const limit = allowedLineCount(baseLines, maxLines);
  return { limit, violates: candidateLines > limit };
}

export function resolveBaseRef(repoRoot, env = process.env) {
  if (env.CHECK_FILE_SIZES_BASE) return env.CHECK_FILE_SIZES_BASE;

  if (env.GITHUB_ACTIONS === "true") return "HEAD^1";

  try {
    const mergeBase = git(["merge-base", "origin/main", "HEAD"], repoRoot)
      .toString("utf8")
      .trim();
    const head = git(["rev-parse", "HEAD"], repoRoot).toString("utf8").trim();
    return mergeBase === head ? "HEAD" : mergeBase;
  } catch (error) {
    throw new Error(
      "Could not resolve the file-size base from origin/main. Fetch origin/main or set CHECK_FILE_SIZES_BASE to an explicit commit.",
      { cause: error },
    );
  }
}

export function parseChangedFiles(output) {
  const fields = output.split("\0");
  const changes = [];

  for (let index = 0; index < fields.length - 1;) {
    const status = fields[index++];
    if (status.startsWith("R") || status.startsWith("C")) {
      changes.push({
        status: status[0],
        oldPath: fields[index++],
        path: fields[index++],
      });
    } else {
      changes.push({ status: status[0], path: fields[index++] });
    }
  }

  return changes;
}

function findRule(rules, relativePath) {
  return rules.find(
    (rule) =>
      relativePath.startsWith(`${rule.root}/`) || relativePath === rule.root,
  );
}

function changedFiles({ repoRoot, baseRef }) {
  const output = git(
    ["diff", "--name-status", "-z", "-M", baseRef, "--", "."],
    repoRoot,
  ).toString("utf8");
  const changes = parseChangedFiles(output);
  const trackedPaths = new Set(changes.map((change) => change.path));
  const untracked = git(
    ["ls-files", "--others", "--exclude-standard", "-z", "--", "."],
    repoRoot,
  )
    .toString("utf8")
    .split("\0")
    .filter(Boolean);

  for (const filePath of untracked) {
    if (!trackedPaths.has(filePath))
      changes.push({ status: "A", path: filePath });
  }
  return changes;
}

function readBaseFile(repoRoot, baseRef, filePath) {
  return git(["show", `${baseRef}:${filePath}`], repoRoot, {
    encoding: "utf8",
  }).toString();
}

export async function collectFileSizeViolations({
  repoRoot,
  rules,
  baseRef = resolveBaseRef(repoRoot),
}) {
  git(["cat-file", "-e", `${baseRef}^{commit}`], repoRoot);
  const violations = [];

  for (const change of changedFiles({ repoRoot, baseRef })) {
    if (change.status === "D") continue;

    const relativePath = toPosixPath(change.path);
    const rule = findRule(rules, relativePath);
    if (!rule || !rule.extensions.has(path.extname(relativePath))) continue;
    if (rule.exclude?.some((prefix) => relativePath.startsWith(prefix)))
      continue;

    const candidatePath = path.join(repoRoot, change.path);
    const candidateLines = countLines(await fs.readFile(candidatePath, "utf8"));
    const basePath = change.oldPath ?? change.path;
    const baseContent =
      change.status === "A" ? null : readBaseFile(repoRoot, baseRef, basePath);
    const baseLines = baseContent == null ? null : countLines(baseContent);
    const result = evaluateFileSize({
      baseLines,
      candidateLines,
      maxLines: rule.maxLines,
    });

    if (result.violates) {
      violations.push({
        relativePath,
        baseLines,
        candidateLines,
        limit: result.limit,
      });
    }
  }

  return { baseRef, violations };
}

export async function runFileSizeCheck({ repoRoot, rules, label }) {
  const report = await collectFileSizeViolations({ repoRoot, rules });
  if (report.violations.length === 0) return report;

  console.error(`${label} file size ratchet failed (base ${report.baseRef}):`);
  for (const violation of report.violations) {
    const before = violation.baseLines == null ? "new" : violation.baseLines;
    const delta =
      violation.baseLines == null
        ? ""
        : ` (${violation.candidateLines - violation.baseLines >= 0 ? "+" : ""}${violation.candidateLines - violation.baseLines})`;
    console.error(
      `- ${violation.relativePath}: ${before} -> ${violation.candidateLines}${delta} lines (allowed ${violation.limit})`,
    );
  }
  console.error(
    "Keep new files at or below the limit; files already over it may not grow.",
  );
  process.exitCode = 1;
  return report;
}
