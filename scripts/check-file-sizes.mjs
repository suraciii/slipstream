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
      maxLines: 3000,
    },
    {
      root: "apps/web/src",
      extensions: extensions(".ts", ".tsx"),
      maxLines: 3000,
    },
    {
      root: "scripts",
      extensions: extensions(".mjs", ".ts"),
      maxLines: 3000,
    },
    {
      root: "compatibility",
      extensions: extensions(".json", ".sql"),
      maxLines: 0,
    },
  ],
});
