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

function dockerfileFromInstructions(
  source: string,
): PositionedFromInstruction[] {
  const sourceLines = source
    .split(/\r?\n/)
    .filter((line) => /^\s*from\b/i.test(line));
  const instructions = [
    ...source.matchAll(
      /^\s*from\s+(?:--platform=([^\s]+)\s+)?([^\s]+)(?:\s+as\s+([^\s]+))?\s*(?:#.*)?$/gim,
    ),
  ].map((match) => {
    const [, platform, reference, stage] = match;
    return {
      ...(platform === undefined ? {} : { platform }),
      reference: reference!,
      ...(stage === undefined ? {} : { stage }),
      offset: match.index!,
    };
  });

  expect(instructions).toHaveLength(sourceLines.length);
  return instructions;
}

function dockerfileStage(source: string, stage: string): string {
  const instructions = dockerfileFromInstructions(source);
  const index = instructions.findIndex(
    (candidate) => candidate.stage?.toLowerCase() === stage.toLowerCase(),
  );
  expect(index).toBeGreaterThanOrEqual(0);
  const start = instructions[index]?.offset;
  if (start === undefined) throw new Error(`missing Dockerfile stage ${stage}`);
  const end = instructions[index + 1]?.offset ?? source.length;
  return source.slice(start, end);
}

function lockEntries(source: string): string[] {
  const entries = source.split(/\r?\n/).filter(Boolean);
  for (const entry of entries)
    expect(entry).toMatch(/^[a-z0-9][a-z0-9+.-]*=[^\s=]+$/);
  expect(new Set(entries).size).toBe(entries.length);
  return entries;
}

function occurrences(source: string, pattern: RegExp): number {
  return [...source.matchAll(pattern)].length;
}

function expectLockedUbuntuStage(stage: string, lock: string): void {
  const caCopy =
    "COPY --from=rust-toolchain /etc/ssl/certs/ca-certificates.crt /usr/local/share/slipstream-ca-certificates.crt";
  const bootstrapCopy =
    "COPY docker/apt/bootstrap-ca.conf /etc/apt/apt.conf.d/00slipstream-bootstrap-ca";
  const sourceCopy =
    "COPY docker/apt/ubuntu.sources /etc/apt/sources.list.d/ubuntu.sources";
  const lockCopy = `COPY ${lock} /tmp/apt-packages.lock`;
  const update = "apt-get update --error-on=any";
  const install =
    "apt-get install --no-install-recommends --yes $(cat /tmp/apt-packages.lock)";
  const cleanup =
    "rm --force /etc/apt/apt.conf.d/00slipstream-bootstrap-ca /usr/local/share/slipstream-ca-certificates.crt /tmp/apt-packages.lock";

  expect(
    occurrences(
      stage,
      /COPY --from=rust-toolchain \/etc\/ssl\/certs\/ca-certificates\.crt \/usr\/local\/share\/slipstream-ca-certificates\.crt/g,
    ),
  ).toBe(1);
  expect(
    occurrences(
      stage,
      /COPY docker\/apt\/bootstrap-ca\.conf \/etc\/apt\/apt\.conf\.d\/00slipstream-bootstrap-ca/g,
    ),
  ).toBe(1);
  expect(
    occurrences(
      stage,
      /COPY docker\/apt\/ubuntu\.sources \/etc\/apt\/sources\.list\.d\/ubuntu\.sources/g,
    ),
  ).toBe(1);
  expect(
    stage.match(/^COPY docker\/apt\/[^\s]+\.lock \/tmp\/apt-packages\.lock$/gm),
  ).toEqual([lockCopy]);
  expect(occurrences(stage, /\bapt-get\s+update\b/g)).toBe(1);
  expect(occurrences(stage, /apt-get update --error-on=any/g)).toBe(1);
  expect(occurrences(stage, /\bapt-get\s+install\b/g)).toBe(1);
  expect(
    occurrences(
      stage,
      /apt-get install --no-install-recommends --yes \$\(cat \/tmp\/apt-packages\.lock\)/g,
    ),
  ).toBe(1);
  expect(
    occurrences(
      stage,
      /rm --force \/etc\/apt\/apt\.conf\.d\/00slipstream-bootstrap-ca \/usr\/local\/share\/slipstream-ca-certificates\.crt \/tmp\/apt-packages\.lock/g,
    ),
  ).toBe(1);
  expect(stage.indexOf(caCopy)).toBeGreaterThanOrEqual(0);
  expect(stage.indexOf(bootstrapCopy)).toBeGreaterThanOrEqual(0);
  expect(stage.indexOf(sourceCopy)).toBeGreaterThanOrEqual(0);
  expect(stage.indexOf(lockCopy)).toBeGreaterThanOrEqual(0);
  expect(stage.indexOf(update)).toBeGreaterThan(stage.indexOf(lockCopy));
  expect(stage.indexOf(install)).toBeGreaterThan(stage.indexOf(update));
  expect(stage.indexOf(cleanup)).toBeGreaterThan(stage.indexOf(install));
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
  expect(await text(".github/workflows/verify.yml")).toContain(
    "- run: bun run verify",
  );
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
