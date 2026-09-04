import { expect, test } from "bun:test";

const repositoryRoot = new URL("../", import.meta.url);

function declaredExtensions(source: string, constant: string): string[] {
  const declaration = source.match(
    new RegExp(`const ${constant}: &\\[&str\\] = &\\[(.*?)\\];`, "s"),
  );
  if (!declaration?.[1]) throw new Error(`${constant} declaration is missing`);
  return [...declaration[1].matchAll(/"([a-z0-9]+)"/g)].map(
    ([, extension]) => extension!,
  );
}

function caseInsensitivePattern(extension: string): string {
  const suffix = [...extension]
    .map((character) =>
      /[a-z]/.test(character)
        ? `[${character}${character.toUpperCase()}]`
        : character,
    )
    .join("");
  return `**/*.${suffix}`;
}

function missingPatterns(
  dockerignore: string,
  extensions: readonly string[],
): string[] {
  const patterns = new Set(activePatterns(dockerignore));
  return extensions
    .map(caseInsensitivePattern)
    .filter((pattern) => !patterns.has(pattern));
}

function activePatterns(dockerignore: string): string[] {
  return dockerignore
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#"));
}

function mixedCaseExtension(extension: string): string {
  return [...extension]
    .map((character, index) =>
      index % 2 === 0 ? character.toUpperCase() : character,
    )
    .join("");
}

async function gitIgnoredPaths(paths: readonly string[]): Promise<Set<string>> {
  const check = Bun.spawn(
    ["git", "check-ignore", "--verbose", "--no-index", "--", ...paths],
    {
      cwd: repositoryRoot.pathname,
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(check.stdout).text(),
    new Response(check.stderr).text(),
    check.exited,
  ]);
  if (exitCode !== 0 && exitCode !== 1)
    throw new Error(stderr.trim() || `git check-ignore exited ${exitCode}`);
  return new Set(
    stdout
      .split(/\r?\n/)
      .filter(Boolean)
      .map((line) => {
        const [rule, path] = line.split("\t");
        if (!rule?.startsWith(".gitignore:") || !path)
          throw new Error(
            `Original ignore came from outside .gitignore: ${line}`,
          );
        return path;
      }),
  );
}

test("Git and Docker ignores cover every supported Original extension", async () => {
  const identitySource = await Bun.file(
    new URL("crates/slipstream-core/src/identity.rs", repositoryRoot),
  ).text();
  const extensions = [
    ...declaredExtensions(identitySource, "RAW_EXTENSIONS"),
    ...declaredExtensions(identitySource, "JPEG_EXTENSIONS"),
  ];
  expect(extensions.length).toBeGreaterThan(0);

  const dockerignore = await Bun.file(
    new URL(".dockerignore", repositoryRoot),
  ).text();
  expect(missingPatterns(dockerignore, extensions)).toEqual([]);
  expect(
    activePatterns(dockerignore).filter((line) => line.startsWith("!")),
  ).toEqual(["!.env.example"]);

  const candidates = extensions.flatMap((extension) => {
    const mixedCase = mixedCaseExtension(extension);
    return [
      `photo.${extension}`,
      `photo.${extension.toUpperCase()}`,
      `nested/session/photo.${mixedCase}`,
    ];
  });
  expect([...(await gitIgnoredPaths(candidates))].sort()).toEqual(
    [...candidates].sort(),
  );

  for (const extension of extensions) {
    const glob = new Bun.Glob(caseInsensitivePattern(extension));
    const mixedCase = mixedCaseExtension(extension);
    for (const candidate of [extension, extension.toUpperCase(), mixedCase]) {
      expect(glob.match(`photo.${candidate}`)).toBeTrue();
      expect(glob.match(`nested/session/photo.${candidate}`)).toBeTrue();
    }
  }
  expect(activePatterns(dockerignore)).toContain("**/*.[pP][nN][gG]");

  const firstDockerPattern = caseInsensitivePattern(extensions[0]!);
  expect(
    missingPatterns(
      dockerignore.replace(`${firstDockerPattern}\n`, ""),
      extensions,
    ),
  ).toEqual([firstDockerPattern]);

  const fixturePath = "apps/web/test-fixtures/review.jpg";
  const fixture = await Bun.file(
    new URL(fixturePath, repositoryRoot),
  ).arrayBuffer();
  expect(fixture.byteLength).toBeGreaterThan(0);
  const tracked = Bun.spawn(
    ["git", "ls-files", "--error-unmatch", "--", fixturePath],
    {
      cwd: repositoryRoot.pathname,
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  const [trackedOutput, trackedError, trackedExitCode] = await Promise.all([
    new Response(tracked.stdout).text(),
    new Response(tracked.stderr).text(),
    tracked.exited,
  ]);
  if (trackedExitCode !== 0)
    throw new Error(
      trackedError.trim() || `git ls-files exited ${trackedExitCode}`,
    );
  expect(trackedOutput.trim()).toBe(fixturePath);
});
