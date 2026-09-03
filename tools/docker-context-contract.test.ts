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

test("Docker build context excludes every supported Original extension", async () => {
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

  for (const extension of extensions) {
    const glob = new Bun.Glob(caseInsensitivePattern(extension));
    const mixedCase = [...extension]
      .map((character, index) =>
        index % 2 === 0 ? character.toUpperCase() : character,
      )
      .join("");
    for (const candidate of [extension, extension.toUpperCase(), mixedCase]) {
      expect(glob.match(`photo.${candidate}`)).toBeTrue();
      expect(glob.match(`nested/session/photo.${candidate}`)).toBeTrue();
    }
  }
  expect(activePatterns(dockerignore)).toContain("**/*.[pP][nN][gG]");

  const firstPattern = caseInsensitivePattern(extensions[0]!);
  expect(
    missingPatterns(dockerignore.replace(`${firstPattern}\n`, ""), extensions),
  ).toEqual([firstPattern]);
});
