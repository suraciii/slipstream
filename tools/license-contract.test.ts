import { expect, test } from "bun:test";

type CargoManifest = {
  package?: { license?: { workspace?: boolean } };
  workspace?: { package?: { license?: string } };
};

type PackageManifest = { license?: string };

const repositoryRoot = new URL("../", import.meta.url);

async function parseToml(path: string): Promise<CargoManifest> {
  return Bun.TOML.parse(
    await Bun.file(new URL(path, repositoryRoot)).text(),
  ) as CargoManifest;
}

async function parseJson(path: string): Promise<PackageManifest> {
  return (await Bun.file(
    new URL(path, repositoryRoot),
  ).json()) as PackageManifest;
}

test("every active project surface uses the MIT license", async () => {
  const workspace = await parseToml("Cargo.toml");
  expect(workspace.workspace?.package?.license).toBe("MIT");

  const crateManifests = new Bun.Glob("crates/*/Cargo.toml");
  let crateCount = 0;
  for await (const path of crateManifests.scan({
    cwd: repositoryRoot.pathname,
  })) {
    const manifest = await parseToml(path);
    expect(manifest.package?.license?.workspace).toBe(true);
    crateCount += 1;
  }
  expect(crateCount).toBeGreaterThan(0);

  expect((await parseJson("package.json")).license).toBe("MIT");
  expect((await parseJson("apps/web/package.json")).license).toBe("MIT");
  expect(await Bun.file(new URL("LICENSE", repositoryRoot)).text()).toStartWith(
    "MIT License\n",
  );

  const cargoLock = await Bun.file(
    new URL("Cargo.lock", repositoryRoot),
  ).arrayBuffer();
  const cargoLockSha256 = new Bun.CryptoHasher("sha256")
    .update(cargoLock)
    .digest("hex");
  expect(
    await Bun.file(new URL("RUST-LICENSES.html", repositoryRoot)).text(),
  ).toContain(
    `<meta name="slipstream-cargo-lock-sha256" content="${cargoLockSha256}">`,
  );

  const dockerfile = await Bun.file(
    new URL("Dockerfile", repositoryRoot),
  ).text();
  expect(dockerfile).toContain(
    "COPY LICENSE THIRD-PARTY-NOTICES.md RUST-LICENSES.html /usr/share/doc/slipstream/",
  );
  expect(dockerfile).toContain("FROM scratch AS runtime");
  expect(dockerfile).toContain("COPY --from=runtime-rootfs / /");
  expect(dockerfile).toContain("&& rm --force /usr/bin/pebble");
  expect(dockerfile).toContain("&& rm --recursive --force /var/lib/pebble");
  expect(dockerfile).toContain(
    "&& install -d -o 1000 -g 1000 -m 0700 /home/slipstream /state /cache /tmp/slipstream",
  );
  expect(dockerfile).not.toContain("org.opencontainers.image.licenses");
});
