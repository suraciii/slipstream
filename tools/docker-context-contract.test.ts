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

const doubleStar = Symbol("double-star");

type PatternSegment = RegExp | typeof doubleStar;

// Docker evaluates .dockerignore with anchored patterns relative to the
// context root: `*` and `?` never cross `/`, `**` matches any number of
// whole segments including none, and the last matching rule decides.
function compileSegment(segment: string): PatternSegment {
  if (segment === "**") return doubleStar;
  let source = "";
  let index = 0;
  while (index < segment.length) {
    const character = segment[index]!;
    if (character === "*") {
      source += "[^/]*";
    } else if (character === "?") {
      source += "[^/]";
    } else if (character === "[") {
      const end = segment.indexOf("]", index + 1);
      if (end === -1)
        throw new Error(`unsupported unterminated class in: ${segment}`);
      const content = segment
        .slice(index + 1, end)
        .replace(/\\/g, "\\\\")
        .replace(/]/g, "\\]");
      source += `[${content}]`;
      index = end;
    } else {
      source += character.replace(/[.+^${}()|\\]/g, "\\$&");
    }
    index += 1;
  }
  return new RegExp(`^${source}$`);
}

function matchSegments(
  pattern: readonly PatternSegment[],
  path: readonly string[],
): boolean {
  if (pattern.length === 0) return path.length === 0;
  const [head, ...rest] = pattern;
  if (head === doubleStar) {
    for (let skip = 0; skip <= path.length; skip += 1) {
      if (matchSegments(rest, path.slice(skip))) return true;
    }
    return false;
  }
  if (path.length === 0) return false;
  return head.test(path[0]!) && matchSegments(rest, path.slice(1));
}

function dockerContextIncludes(dockerignore: string, path: string): boolean {
  const segments = path.split("/");
  let included = true;
  for (const line of activePatterns(dockerignore)) {
    const negated = line.startsWith("!");
    const pattern = (negated ? line.slice(1) : line)
      .split("/")
      .map(compileSegment);
    if (matchSegments(pattern, segments)) included = negated;
  }
  return included;
}

async function trackedPaths(): Promise<string[]> {
  const child = Bun.spawn(["git", "ls-files", "-z"], {
    cwd: repositoryRoot.pathname,
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (exitCode !== 0)
    throw new Error(stderr.trim() || `git ls-files exited ${exitCode}`);
  return stdout.split("\0").filter(Boolean);
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
  ).toEqual(["!.env.example", "!apps/web/public/icons/*.[pP][nN][gG]"]);

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

test("the Docker context ships exactly the product icons, not photo PNGs", async () => {
  const dockerignore = await Bun.file(
    new URL(".dockerignore", repositoryRoot),
  ).text();
  const patterns = activePatterns(dockerignore);
  const photoRule = "**/*.[pP][nN][gG]";
  const iconRule = "!apps/web/public/icons/*.[pP][nN][gG]";
  // Docker applies the last matching rule, so the exception must follow the
  // broad photo exclusion it re-includes from.
  expect(patterns.indexOf(photoRule)).toBeGreaterThanOrEqual(0);
  expect(patterns.indexOf(iconRule)).toBeGreaterThan(
    patterns.indexOf(photoRule),
  );

  const icons = [
    "apps/web/public/icons/apple-touch-icon.png",
    "apps/web/public/icons/slipstream-192.png",
    "apps/web/public/icons/slipstream-512.png",
    "apps/web/public/icons/slipstream-maskable-512.png",
  ];
  const trackedPngs = (await trackedPaths()).filter((path) =>
    /\.png$/i.test(path),
  );
  expect([...trackedPngs].sort()).toEqual(icons);
  expect(
    trackedPngs.filter((path) => dockerContextIncludes(dockerignore, path)),
  ).toEqual(icons);

  const photos = [
    "photo.png",
    "nested/session/Photo.PNG",
    "apps/photo.pNg",
    "apps/web/public/photo.png",
    "apps/web/public/icons/nested/photo.png",
  ];
  expect(
    photos.filter((path) => dockerContextIncludes(dockerignore, path)),
  ).toEqual([]);

  const tampered = dockerignore.replace(`${iconRule}\n`, "");
  expect(
    trackedPngs.filter((path) => dockerContextIncludes(tampered, path)),
  ).toEqual([]);
});
