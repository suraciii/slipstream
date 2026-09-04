import { expect, test } from "bun:test";

type PackageManifest = Readonly<{
  scripts?: Readonly<Record<string, string>>;
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
] as const;

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

function lockEntries(source: string): string[] {
  const entries = source.split(/\r?\n/).filter(Boolean);
  for (const entry of entries)
    expect(entry).toMatch(/^[a-z0-9][a-z0-9+.-]*=[^\s=]+$/);
  expect(new Set(entries).size).toBe(entries.length);
  return entries;
}

test("Dockerfile pins every non-scratch base image by reviewed digest", async () => {
  const dockerfile = await text("Dockerfile");
  for (const { stage, reference } of baseImages)
    expect(dockerfile).toContain(`FROM ${reference} AS ${stage}`);

  const nonScratchBases = [...dockerfile.matchAll(/^FROM ([^\s]+) AS /gm)].map(
    ([, reference]) => reference!,
  );
  expect(nonScratchBases).toEqual([
    ...baseImages.map(({ reference }) => reference),
    "scratch",
  ]);
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

test("Dockerfile consumes and removes snapshot locks with a pinned CA bootstrap", async () => {
  const dockerfile = await text("Dockerfile");
  expect(
    dockerfile.match(
      /COPY --from=rust-toolchain \/etc\/ssl\/certs\/ca-certificates\.crt \/usr\/local\/share\/slipstream-ca-certificates\.crt/g,
    ),
  ).toHaveLength(2);
  expect(
    dockerfile.match(
      /COPY docker\/apt\/bootstrap-ca\.conf \/etc\/apt\/apt\.conf\.d\/00slipstream-bootstrap-ca/g,
    ),
  ).toHaveLength(2);
  expect(dockerfile).toContain(
    "COPY docker/apt/ubuntu.sources /etc/apt/sources.list.d/ubuntu.sources",
  );
  expect(dockerfile).toContain(
    "COPY docker/apt/build-amd64.lock /tmp/apt-packages.lock",
  );
  expect(dockerfile).toContain(
    "COPY docker/apt/runtime-amd64.lock /tmp/apt-packages.lock",
  );
  expect(dockerfile).toContain(
    "apt-get install --no-install-recommends --yes $(cat /tmp/apt-packages.lock)",
  );
  expect(dockerfile.match(/apt-get update --error-on=any/g)).toHaveLength(2);
  expect(
    dockerfile.match(
      /rm --force \/etc\/apt\/apt\.conf\.d\/00slipstream-bootstrap-ca \/usr\/local\/share\/slipstream-ca-certificates\.crt \/tmp\/apt-packages\.lock/g,
    ),
  ).toHaveLength(2);
  expect(dockerfile).toContain(
    "rm --force /etc/apt/apt.conf.d/00slipstream-bootstrap-ca /usr/local/share/slipstream-ca-certificates.crt",
  );
  expect(dockerfile).not.toContain("Acquire::https::Verify-Peer=false");
  expect(dockerfile).not.toContain("trusted=yes");
  expect(dockerfile).not.toContain("http://snapshot.ubuntu.com");
});

test("the input contract is on the verify -> test:fast -> focused gate path", async () => {
  const scripts = (await packageManifest()).scripts;
  expect(scripts?.["test:container-input"]).toBe(
    "bun test tools/container-input-contract.test.ts",
  );
  expect(scripts?.["test:fast"]?.split(" && ")).toContain(
    "bun run test:container-input",
  );
  expect(scripts?.verify?.split(" && ")).toContain("bun run test:fast");
  expect(await text(".github/workflows/verify.yml")).toContain(
    "- run: bun run verify",
  );
});

test("deployment documentation separates fixed inputs from qualification traceability", async () => {
  const deployment = await text("docs/deployment.md");
  expect(deployment).toContain("`--platform linux/amd64`");
  expect(deployment).toContain("`ubuntu-latest` is a source and test runner");
  expect(deployment).toContain("source reproducibility");
  expect(deployment).toContain("single build's traceability");
  expect(deployment).toContain("OCI revision and immutable digest");
  expect(deployment).toContain("final native package versions");
  expect(deployment).toContain("SBOM");
  expect(deployment).toContain("advisory database timestamp");
});

test("Compose source builds use the one supported platform", async () => {
  expect(await text("compose.yaml")).toContain(
    "services:\n  slipstream:\n    platform: linux/amd64\n",
  );
  expect(await text("design/container-inputs.md")).toContain(
    "Compose source builds are supported only for that platform",
  );
  expect(await text("docs/deployment.md")).toContain(
    "only supported source-build platform",
  );
});
