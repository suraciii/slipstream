import { expect, test } from "bun:test";

type PackageManifest = Readonly<{
  scripts?: Readonly<Record<string, string>>;
}>;

type FromInstruction = Readonly<{
  platform?: string;
  reference: string;
  stage?: string;
}>;

type PositionedFromInstruction = FromInstruction &
  Readonly<{
    offset: number;
  }>;

type DockerfileInstruction = Readonly<{
  command: string;
  value: string;
  offset: number;
}>;

type CopyInstruction = Readonly<{
  from?: string;
  sources: readonly string[];
  target: string;
}>;

const repositoryRoot = new URL("../", import.meta.url);

const baseImages = [
  {
    stage: "web-build",
    reference:
      "oven/bun:1.4.0@sha256:5ff609364c049b54eb0ff560ec96319729a972078ef2c755d758f0c6ef89c2d6",
  },
  {
    stage: "rust-toolchain",
    reference:
      "rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97",
  },
  {
    stage: "rust-build",
    reference:
      "ubuntu:26.04@sha256:2260313b31c8c011cd2eebe728008efac1b3982be73eb71348ea2648d2c0e09b",
  },
  {
    stage: "runtime-rootfs",
    reference:
      "ubuntu:26.04@sha256:2260313b31c8c011cd2eebe728008efac1b3982be73eb71348ea2648d2c0e09b",
  },
  { stage: "runtime", reference: "scratch" },
] as const satisfies readonly FromInstruction[];

const packageLocks = {
  "docker/apt/build-amd64.lock": [
    "build-essential=12.12ubuntu2.26.04.2",
    "ca-certificates=20260601~26.04.1",
    "libjpeg-dev=8c-2ubuntu12",
    "liblcms2-dev=2.17-1ubuntu0.2",
    "libraw-dev=0.21.5b-1ubuntu1.1",
    "libvips-dev=8.18.0-1build1",
    "pkg-config=2.5.1-4",
  ],
  "docker/apt/runtime-amd64.lock": [
    "ca-certificates=20260601~26.04.1",
    "curl=8.18.0-1ubuntu2.4",
    "libjpeg-turbo8=2.1.5-4ubuntu4",
    "liblcms2-2=2.17-1ubuntu0.2",
    "libraw23t64=0.21.5b-1ubuntu1.1",
    "libvips42t64=8.18.0-1build1",
  ],
} as const;

async function text(path: string): Promise<string> {
  return Bun.file(new URL(path, repositoryRoot)).text();
}

async function packageManifest(): Promise<PackageManifest> {
  return (await Bun.file(
    new URL("package.json", repositoryRoot),
  ).json()) as PackageManifest;
}

function mapping(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function workflowRunValues(source: string): string[] {
  const document = mapping(Bun.YAML.parse(source));
  const jobs = mapping(document?.jobs);
  if (!jobs) return [];

  const runs: string[] = [];
  for (const jobValue of Object.values(jobs)) {
    const job = mapping(jobValue);
    if (!Array.isArray(job?.steps)) continue;
    for (const stepValue of job.steps) {
      const step = mapping(stepValue);
      if (typeof step?.run === "string") runs.push(step.run);
    }
  }
  return runs;
}

function dockerfileInstructions(source: string): DockerfileInstruction[] {
  const instructions: DockerfileInstruction[] = [];
  let value = "";
  let instructionOffset = 0;
  let offset = 0;

  for (const rawLine of source.split("\n")) {
    const line = rawLine.endsWith("\r") ? rawLine.slice(0, -1) : rawLine;
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) {
      offset += rawLine.length + 1;
      continue;
    }

    if (!value) instructionOffset = offset;
    const continues = /\\\s*$/.test(line);
    const fragment = (continues ? line.replace(/\\\s*$/, "") : line).trim();
    value = value ? `${value} ${fragment}` : fragment;
    if (!continues) {
      const match = /^([a-z]+)(?:\s+(.*))?$/i.exec(value);
      expect(match).not.toBeNull();
      instructions.push({
        command: match![1]!.toUpperCase(),
        value: match![2] ?? "",
        offset: instructionOffset,
      });
      value = "";
    }
    offset += rawLine.length + 1;
  }

  expect(value).toBe("");
  return instructions;
}

function dockerfileFromInstructions(
  source: string,
): PositionedFromInstruction[] {
  return dockerfileInstructions(source)
    .filter((instruction) => instruction.command === "FROM")
    .map(({ offset, value }) => {
      const match =
        /^(?:--platform=([^\s]+)\s+)?([^\s]+)(?:\s+as\s+([^\s]+))?\s*(?:#.*)?$/i.exec(
          value,
        );
      expect(match).not.toBeNull();
      const [, platform, reference, stage] = match!;
      return {
        ...(platform === undefined ? {} : { platform }),
        reference: reference!,
        ...(stage === undefined ? {} : { stage }),
        offset,
      };
    });
}

function dockerfileStage(
  source: string,
  stage: string,
): readonly DockerfileInstruction[] {
  const instructions = dockerfileInstructions(source);
  const from = dockerfileFromInstructions(source).find(
    (candidate) => candidate.stage?.toLowerCase() === stage.toLowerCase(),
  );
  expect(from).toBeDefined();
  const start = instructions.findIndex(
    (instruction) => instruction.offset === from?.offset,
  );
  expect(start).toBeGreaterThanOrEqual(0);
  const end = instructions.findIndex(
    (instruction, index) => index > start && instruction.command === "FROM",
  );
  return instructions.slice(start + 1, end < 0 ? undefined : end);
}

function lockEntries(source: string): string[] {
  const entries = source.split(/\r?\n/).filter(Boolean);
  for (const entry of entries)
    expect(entry).toMatch(/^[a-z0-9][a-z0-9+.-]*=[^\s=]+$/);
  expect(new Set(entries).size).toBe(entries.length);
  return entries;
}

function copyInstruction(instruction: DockerfileInstruction): CopyInstruction {
  expect(instruction.command).toBe("COPY");
  const fields = instruction.value.split(/\s+/);
  let from: string | undefined;
  while (fields[0]?.startsWith("--")) {
    const flag = fields.shift()!;
    const match = /^--from=(.+)$/.exec(flag);
    if (match) {
      expect(from).toBeUndefined();
      from = match[1]!;
    }
  }
  expect(fields.length).toBeGreaterThanOrEqual(2);
  const target = fields.pop();
  if (target === undefined) throw new Error("COPY instruction has no target");
  return {
    ...(from === undefined ? {} : { from }),
    sources: fields,
    target,
  };
}

function runCommands(instruction: DockerfileInstruction): string[] {
  expect(instruction.command).toBe("RUN");
  return instruction.value
    .split(/\s+&&\s+/)
    .map((command, index) =>
      (index === 0 ? command.replace(/^(?:--[^\s]+\s+)*/, "") : command).trim(),
    );
}

function expectLockedUbuntuStage(
  stage: readonly DockerfileInstruction[],
  lock: string,
): void {
  const update = "apt-get update --error-on=any";
  const install =
    "apt-get install --no-install-recommends --yes $(cat /tmp/apt-packages.lock)";
  const cleanup =
    "rm --force /etc/apt/apt.conf.d/00slipstream-bootstrap-ca /usr/local/share/slipstream-ca-certificates.crt /tmp/apt-packages.lock";
  const copies = stage
    .filter((instruction) => instruction.command === "COPY")
    .map(copyInstruction)
    .filter((copy) =>
      [
        "/usr/local/share/slipstream-ca-certificates.crt",
        "/etc/apt/apt.conf.d/00slipstream-bootstrap-ca",
        "/etc/apt/sources.list.d/ubuntu.sources",
        "/tmp/apt-packages.lock",
      ].includes(copy.target),
    );
  expect(copies).toEqual([
    {
      from: "rust-toolchain",
      sources: ["/etc/ssl/certs/ca-certificates.crt"],
      target: "/usr/local/share/slipstream-ca-certificates.crt",
    },
    {
      sources: ["docker/apt/bootstrap-ca.conf"],
      target: "/etc/apt/apt.conf.d/00slipstream-bootstrap-ca",
    },
    {
      sources: ["docker/apt/ubuntu.sources"],
      target: "/etc/apt/sources.list.d/ubuntu.sources",
    },
    {
      sources: [lock],
      target: "/tmp/apt-packages.lock",
    },
  ] satisfies readonly CopyInstruction[]);

  const aptRuns = stage.filter(
    (instruction) =>
      instruction.command === "RUN" && instruction.value.includes("apt-get"),
  );
  expect(aptRuns).toHaveLength(1);
  const commands = runCommands(aptRuns[0]!);
  expect(commands.filter((command) => command.includes("apt-get"))).toEqual([
    update,
    install,
  ]);
  const updateIndex = commands.indexOf(update);
  const installIndex = commands.indexOf(install);
  const cleanupIndex = commands.indexOf(cleanup);
  expect(updateIndex).toBeGreaterThanOrEqual(0);
  expect(installIndex).toBeGreaterThan(updateIndex);
  expect(cleanupIndex).toBeGreaterThan(installIndex);
  expect(commands.filter((command) => command === cleanup)).toEqual([cleanup]);
}

test("Dockerfile enumerates every base image and pins each non-scratch input", async () => {
  const instructions = dockerfileFromInstructions(await text("Dockerfile"));
  expect(
    instructions.map(({ platform, reference, stage }) => ({
      ...(platform === undefined ? {} : { platform }),
      reference,
      ...(stage === undefined ? {} : { stage }),
    })),
  ).toEqual(baseImages);

  for (const { reference } of instructions) {
    if (reference.toLowerCase() === "scratch") continue;
    expect(reference).toMatch(/@sha256:[0-9a-f]{64}$/);
  }
});

test("Ubuntu native inputs use one official amd64 snapshot and explicit direct locks", async () => {
  expect(await text("docker/apt/ubuntu.sources")).toBe(`Types: deb
URIs: https://snapshot.ubuntu.com/ubuntu/20260904T000000Z/
Suites: resolute resolute-updates resolute-security
Components: main universe
Architectures: amd64
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
`);
  expect(await text("docker/apt/bootstrap-ca.conf")).toBe(
    'Acquire::https::CaInfo "/usr/local/share/slipstream-ca-certificates.crt";\n',
  );

  for (const [path, expected] of Object.entries(packageLocks))
    expect(lockEntries(await text(path))).toEqual(expected);
});

test("each Ubuntu stage has one pinned CA, source, lock, update, install, and cleanup sequence", async () => {
  const dockerfile = await text("Dockerfile");
  expectLockedUbuntuStage(
    dockerfileStage(dockerfile, "rust-build"),
    "docker/apt/build-amd64.lock",
  );
  expectLockedUbuntuStage(
    dockerfileStage(dockerfile, "runtime-rootfs"),
    "docker/apt/runtime-amd64.lock",
  );
  expect(dockerfile).not.toContain("Acquire::https::Verify-Peer=false");
  expect(dockerfile).not.toContain("trusted=yes");
});

test("the container and Compose focused contracts are wired through test:fast and verify", async () => {
  const scripts = (await packageManifest()).scripts;
  expect(scripts?.["test:container-input"]).toBe(
    "bun test tools/container-input-contract.test.ts",
  );
  expect(scripts?.["test:compose-storage"]).toBe(
    "bun test tools/compose-storage-preflight.test.ts",
  );
  const fastGate = scripts?.["test:fast"]?.split(" && ");
  expect(fastGate).toContain("bun run test:container-input");
  expect(fastGate).toContain("bun run test:compose-storage");
  expect(scripts?.verify?.split(" && ")).toContain("bun run test:fast");
  expect(
    workflowRunValues(await text(".github/workflows/verify.yml")).some(
      (run) => run === "bun run verify",
    ),
  ).toBeTrue();
});

test("deployment documentation distinguishes build inputs from qualification output", async () => {
  const deployment = await text("docs/deployment.md");
  const design = await text("design/container-inputs.md");
  expect(deployment).toContain("`docker buildx build --platform linux/amd64`");
  expect(deployment).toContain("runs only the digest-pinned image");
  expect(deployment).toContain("source reproducibility");
  expect(deployment).toContain("single build's traceability");
  expect(deployment).toContain("OCI revision and immutable digest");
  expect(deployment).toContain("final native package versions");
  expect(deployment).toContain("SBOM");
  expect(deployment).toContain("advisory database timestamp");
  expect(design).toContain("has no `build` input");
});
