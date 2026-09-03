const retiredAlbumTerms = String.raw`photo[ _-]?sets?|\$setids?|\bset_?ids?\b|compat[ _-]?set`;

const search = Bun.spawn(
  ["git", "grep", "-n", "-I", "-i", "-E", retiredAlbumTerms, "--", "."],
  { stdout: "pipe", stderr: "pipe" },
);
const [stdout, stderr, exitCode] = await Promise.all([
  new Response(search.stdout).text(),
  new Response(search.stderr).text(),
  search.exited,
]);
if (exitCode !== 0 && exitCode !== 1)
  throw new Error(stderr.trim() || `git grep exited ${exitCode}`);

const ownerPath = "crates/slipstream-core/src/persistence/owner.rs";
const ownerLines = (await Bun.file(ownerPath).text()).split("\n");
const legacyOwnerLines = new Set<number>();
let legacyBlock: string | undefined;
for (const [index, line] of ownerLines.entries()) {
  const start = line.match(/album-language-legacy:start ([a-z0-9-]+)$/);
  const end = line.match(/album-language-legacy:end ([a-z0-9-]+)$/);
  if (start) {
    if (legacyBlock) throw new Error(`nested legacy marker at ${index + 1}`);
    legacyBlock = start[1];
  } else if (end) {
    if (!legacyBlock || end[1] !== legacyBlock)
      throw new Error(`unmatched legacy marker at ${index + 1}`);
    legacyBlock = undefined;
  } else if (legacyBlock) legacyOwnerLines.add(index + 1);
}
if (legacyBlock) throw new Error(`unclosed legacy marker: ${legacyBlock}`);

const allowed = (path: string, line: number, text: string): boolean => {
  if (path === "scripts/check-album-language.ts") return true;
  if (/^compatibility\/sqlite\/schema-v[234]\.(json|sql)$/.test(path))
    return true;
  if (path === ownerPath) return legacyOwnerLines.has(line);
  if (path === "CONTEXT.md") return text.startsWith("_Avoid_: Photo Set,");
  if (path === "compatibility/protocol/browse-vectors.json")
    return text.includes('"source": "photo-set"');
  if (path === "compatibility/protocol/vectors.json")
    return (
      text.includes('"name": "retired-photo-set-create"') ||
      text.includes('"path": "/api/photo-sets"')
    );
  if (path === "design/capture-time-ordering.md")
    return text.includes("under earlier Photo Set names");
  if (path === "design/library-browsing.md")
    return text.includes("Legacy Photo Set routes and source values");
  if (path === "design/photo-organization.md")
    return (
      text.includes("It replaces active `photo_sets`") ||
      text.includes("Legacy `Photo Set` names remain only") ||
      text.startsWith("### Rejected: Retain Photo Set Internally") ||
      text.startsWith("- absence of active `Photo Set` names")
    );
  if (path === "docs/0.1-support-and-release.md")
    return text.includes("legacy v4 Photo Set storage names");
  return false;
};

const violations = stdout
  .split("\n")
  .filter(Boolean)
  .filter((line) => {
    const first = line.indexOf(":");
    const second = line.indexOf(":", first + 1);
    return !allowed(
      line.slice(0, first),
      Number(line.slice(first + 1, second)),
      line.slice(second + 1),
    );
  });

if (violations.length) {
  console.error("Retired Photo Set language appeared outside legacy evidence:");
  for (const violation of violations) console.error(violation);
  process.exit(1);
}

console.log("Album language check passed");
