import path from "node:path";
import { fileURLToPath } from "node:url";
import { runFileSizeCheck } from "./check-file-sizes-core.mjs";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

const extensions = (...values) => new Set(values);

await runFileSizeCheck({
  repoRoot: repositoryRoot,
  label: "Slipstream",
  rules: [
    {
      root: "crates",
      extensions: extensions(".rs"),
      maxLines: 1000,
    },
    {
      root: "apps/web/src",
      extensions: extensions(".ts", ".tsx"),
      maxLines: 1000,
    },
    {
      root: "scripts",
      extensions: extensions(".mjs", ".ts"),
      maxLines: 1000,
    },
    // `compatibility/` is contract data, not implementation: its JSON and SQL
    // vectors are already governed by the compatibility inventory test and the
    // schema-version checks, and freezing them here would block the next
    // schema version from being added at all.
  ],
});
