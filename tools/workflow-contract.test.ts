import { expect, test } from "bun:test";

const repositoryRoot = new URL("../", import.meta.url);
const immutableRepositoryAction =
  /^[^/\s]+\/[^@\s]+(?:\/[^@\s]+)*@[0-9a-f]{40}$/;
const canonicalUse = /^\s*(?:- )?uses: ([^\s"'#]+)(?: # (\S.*?))?\s*$/;
const reviewedActions = new Map<string, string>([
  ["actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1", "v7.0.1"],
  ["oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6", "v2.2.0"],
  ["dtolnay/rust-toolchain@032958afbdc797a9164d3bc0b56325c1308924a5", "1.97.1"],
  [
    "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
    "v7.0.1",
  ],
]);

type SourceUse = Readonly<{
  line: number;
  start: number;
  end: number;
  lexicalTarget: string;
  version?: string;
}>;

type BoundUse = SourceUse &
  Readonly<{ inventoryIndex: number; target: string }>;

type WorkflowCheck = Readonly<{
  thirdPartyUses: number;
  violations: ReadonlyArray<string>;
}>;

function mapping(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function structuredUses(source: string): string[] {
  const document = mapping(Bun.YAML.parse(source));
  const jobs = mapping(document?.jobs);
  if (!jobs) return [];

  const uses: string[] = [];
  const add = (value: unknown): void => {
    if (typeof value !== "string")
      throw new Error("job and step uses values must be strings");
    uses.push(value);
  };

  for (const jobValue of Object.values(jobs)) {
    const job = mapping(jobValue);
    if (!job) continue;
    if ("uses" in job) add(job.uses);
    if (!Array.isArray(job.steps)) continue;
    for (const stepValue of job.steps) {
      const step = mapping(stepValue);
      if (step && "uses" in step) add(step.uses);
    }
  }
  return uses;
}

function sourceUses(source: string): SourceUse[] {
  const uses: SourceUse[] = [];
  let offset = 0;

  for (const [lineIndex, line] of source.split(/\r?\n/).entries()) {
    const match = line.match(canonicalUse);
    const target = match?.[1];
    if (target && match.index !== undefined) {
      const start = offset + match.index + match[0].indexOf(target);
      uses.push({
        line: lineIndex + 1,
        start,
        end: start + target.length,
        lexicalTarget: target,
        version: match[2],
      });
    }

    offset += line.length;
    if (source.startsWith("\r\n", offset)) offset += 2;
    else if (source[offset] === "\n") offset += 1;
  }
  return uses;
}

function bindSourceUse(
  source: string,
  inventory: ReadonlyArray<string>,
  candidate: SourceUse,
  index: number,
): BoundUse | undefined {
  const marker = `__slipstream_uses_${index}__`;
  const marked = `${source.slice(0, candidate.start)}${marker}${source.slice(candidate.end)}`;
  let markedInventory: string[];
  try {
    markedInventory = structuredUses(marked);
  } catch {
    return undefined;
  }

  if (markedInventory.length !== inventory.length) return undefined;
  const inventoryIndex = markedInventory.indexOf(marker);
  if (
    inventoryIndex < 0 ||
    markedInventory.lastIndexOf(marker) !== inventoryIndex
  )
    return undefined;
  return { ...candidate, inventoryIndex, target: inventory[inventoryIndex]! };
}

function checkWorkflow(source: string, path: string): WorkflowCheck {
  let inventory: string[];
  try {
    inventory = structuredUses(source);
  } catch (error) {
    return { thirdPartyUses: 0, violations: [`${path}: ${String(error)}`] };
  }

  const bound = sourceUses(source).flatMap((candidate, index) => {
    const use = bindSourceUse(source, inventory, candidate, index);
    return use ? [use] : [];
  });
  const violations: string[] = [];
  if (
    bound.length !== inventory.length ||
    new Set(bound.map(({ inventoryIndex }) => inventoryIndex)).size !==
      inventory.length
  )
    violations.push(
      `${path}: every job and step uses must use the canonical one-line form`,
    );

  let thirdPartyUses = 0;
  for (const use of bound) {
    if (use.lexicalTarget !== use.target)
      violations.push(
        `${path}:${use.line} uses target must be written directly`,
      );
    if (use.target.startsWith("./") || use.target.startsWith("$/")) continue;

    thirdPartyUses += 1;
    const expectedVersion = reviewedActions.get(use.target);
    if (!expectedVersion)
      violations.push(
        `${path}:${use.line} third-party use is not an approved immutable action`,
      );
    else if (use.version !== expectedVersion)
      violations.push(
        `${path}:${use.line} expected version comment ${expectedVersion}`,
      );
  }

  return { thirdPartyUses, violations };
}

const checkout = [...reviewedActions.entries()][0]!;
const stepWorkflow = (step: string): string => `name: Fixture
on: push
jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
${step}
`;

test("reviewed actions are immutable and bind to exact version comments", () => {
  for (const [target, version] of reviewedActions) {
    expect(target).toMatch(immutableRepositoryAction);
    expect(
      checkWorkflow(
        stepWorkflow(`      - uses: ${target} # ${version}`),
        target,
      ),
    ).toEqual({ thirdPartyUses: 1, violations: [] });
  }
});

test("local actions are allowed and unrelated uses text is ignored", () => {
  for (const target of ["./.github/actions/local", "$/actions/local"])
    expect(
      checkWorkflow(stepWorkflow(`      - uses: ${target}`), target),
    ).toEqual({ thirdPartyUses: 0, violations: [] });

  const source = stepWorkflow(`      - name: Context values are not actions
        env:
          uses: actions/example@v1
        run: |
          uses: docker://alpine:latest
      - uses: ${checkout[0]} # ${checkout[1]}`);
  expect(checkWorkflow(source, "unrelated-uses")).toEqual({
    thirdPartyUses: 1,
    violations: [],
  });
});

test("mutable, unknown, container, and inaccurately labelled actions fail closed", () => {
  const invalid = [
    ["mutable", "actions/checkout@v7 # v7.0.1"],
    [
      "unknown",
      "actions/example@0123456789abcdef0123456789abcdef01234567 # v1.0.0",
    ],
    ["container tag", "docker://alpine:3.8"],
    [
      "container digest",
      "docker://alpine@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    ],
    ["wrong comment", `${checkout[0]} # v0.0.0`],
    ["missing comment", checkout[0]],
  ] as const;

  for (const [name, use] of invalid)
    expect(
      checkWorkflow(stepWorkflow(`      - uses: ${use}`), name).violations,
    ).not.toEqual([]);
});

test("actionlint-valid YAML aliases cannot hide lexical action targets", () => {
  const source = `name: Alias fixture
on: push
env:
  ACTION: &checkout ${checkout[0]}
jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - uses: *checkout # ${checkout[1]}
`;
  expect(checkWorkflow(source, "alias").violations).not.toEqual([]);
});

test("qualification workflows use only reviewed immutable actions", async () => {
  const workflows = new Bun.Glob(".github/workflows/**/*.{yml,yaml}");
  const violations: string[] = [];
  let workflowCount = 0;
  let thirdPartyUses = 0;

  for await (const path of workflows.scan({ cwd: repositoryRoot.pathname })) {
    workflowCount += 1;
    const check = checkWorkflow(
      await Bun.file(new URL(path, repositoryRoot)).text(),
      path,
    );
    thirdPartyUses += check.thirdPartyUses;
    violations.push(...check.violations);
  }

  expect(workflowCount).toBeGreaterThan(0);
  expect(thirdPartyUses).toBeGreaterThan(0);
  expect(violations).toEqual([]);
});
